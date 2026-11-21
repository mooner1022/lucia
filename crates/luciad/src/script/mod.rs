//! 스크립트(결정 0025, 명세 §도구화 사다리 2단계 "스킬 안의 반복 단계를 실행 가능한 스크립트로 만든다.
//! 샌드박스 안에서만 실행한다"): Lucia가 쓴 Python 코드를 스크립트 샌드박스에서 돌린다.
//!
//! - **어디서**: 전용 샌드박스(`deploy/create-script-sandbox.sh`, 기본 `lucia-script`). 마운트가 없고, 네트워크는
//!   데몬의 스크립트 출구 프록시 포트 하나만 허용한다. 상주 샌드박스(훅의 이중 방어)와 브라우저 샌드박스(쿠키)에
//!   닿지 않는다.
//! - **통로**: `sbx exec -i`의 표준 입출력 하나. 코드·입력·상태를 첫 줄로 넣고, 코드가 부르는 도구는 한 줄씩 묻고
//!   답한다([`harness.py`](harness.py)).
//! - **바깥**: 도구 다리와, 사람이 대화 중인 실행에만 주는 출구 프록시 통행증([`net`], 결정 0058). 통행증이 없는
//!   스크립트(루틴, 사람 없는 실행)는 바깥에 닿지 못한다.
//! - **도구**: 다리는 데몬 MCP 서버를 실행별 토큰으로 다시 부른다([`McpBridge`]). 그래서 실행 범위 확인, 승인 카드,
//!   인젝션 보류, 활동 기록이 모델이 부를 때와 같다. 범위는 부른 실행의 범위에서 사람 없음·자기 호출 갈래를 뺀 것
//!   ([`crate::tools::Policy::for_script`])이고, 루틴·스킬 스크립트는 만들 때 선언한 도구로 더 좁힌다.
//! - **끝내기**: 호스트에서 `sbx exec`를 끊어도 VM 안 프로세스는 남는다(2026-11-17 실측). 그래서 VM 안에서
//!   `timeout -s KILL`로 감싸고, 시간을 넘기면 실행 표지로 찾아 한 번 더 끝낸다.

pub mod check;
pub mod net;
pub mod tools;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::browser::BoxFut;
use crate::mcp::{RunId, RunTokens};
use crate::tools::Policy;

/// 샌드박스 안에서 도는 실행기.
pub const HARNESS: &str = include_str!("harness.py");

/// 실행 하나에서 부를 수 있는 도구 수 [제안].
pub const MAX_CALLS: usize = 200;
/// 모은 print 글(문자 수) [제안].
pub const PRINTED_MAX: usize = 16_000;
/// 스크립트가 읽는 파일 하나의 상한(결정 0057): 웹에서 받을 수 있는 크기와 같다. [`READ_CHUNK`]씩 나눠 보낸다.
pub const FILE_READ_MAX: u64 = crate::downloads::MAX_BYTES;
/// 스크립트에 한 번에 보내는 파일 조각. base64로 한 줄에 실린다.
pub const READ_CHUNK: usize = 4 * 1024 * 1024;
/// 스크립트가 만드는 파일 하나의 상한(결정 0047의 8 MB를 0057에서 올렸다). base64로 한 줄에 실어 온다.
pub const FILE_SAVE_MAX: usize = 30 * 1024 * 1024;
/// 스크립트 하나가 만들거나 받을 수 있는 파일 수 [제안].
const MAX_SAVED_FILES: usize = 30;
/// 한 줄(도구 부르기·끝 보고) 상한. 만드는 파일 하나(base64)가 들어갈 만큼.
const LINE_MAX: usize = 44 * 1024 * 1024;
/// 돌려줄 값(JSON) 상한. 넘으면 앞부분만 글로.
const RESULT_MAX: usize = 64 * 1024;
/// 다음 실행 상태(JSON) 상한.
pub const STATE_MAX: usize = 256 * 1024;
const STDERR_MAX: usize = 8 * 1024;
/// 동시에 도는 스크립트 [제안].
const SLOTS: usize = 2;

/// JSON으로 답하는 도구(스크립트에서 부를 때). 나머지는 글을 준다.
pub const JSON_TOOLS: &[&str] = &[
    "mail_search",
    "mail_read",
    "calendar_events",
    "discord_messages",
];

/// 스크립트가 부를 수 없는 도구: 보고, 찾기·부르기, 스스로를 부르는 것, 새 자동 실행을 거는 것.
/// 갈래로도 빠지지만([`Policy::for_script`]) 이름으로 한 번 더 막는다.
const NEVER: &[&str] = &[
    "report_result",
    "tool_search",
    "tool_call",
    "request_approval",
    "code_run",
    "skill_run",
    "lib_install",
    "routine_create",
    "routine_update",
    "routine_delete",
    "followup_create",
    "task_start",
    "dev_start",
    "watch_propose",
    "skill_save",
    // 로그인과 위임은 스크립트가 하지 않는다(결정 0028 I1).
    "browser_login",
    "site_grant_propose",
];

/// 스크립트에서 부를 수 없는 도구인가.
pub fn never_from_script(name: &str) -> bool {
    NEVER.contains(&name)
}

/// 스크립트 하나.
#[derive(Debug, Clone)]
pub struct ScriptJob {
    pub code: String,
    /// `lucia.input`.
    pub input: Value,
    /// `lucia.state`(지난번에 남긴 것). 객체.
    pub state: Value,
    /// 도구 범위(이미 [`Policy::for_script`]를 거친 것).
    pub grants: Policy,
    /// 활동 기록·승인에 묶일 실행 ID(부른 실행에서 이어진 이름).
    pub run: String,
    /// 선언한 도구(루틴·스킬 스크립트). `None`이면 범위 안 모두.
    pub declared: Option<Vec<String>>,
    /// `lucia.fetch`(공개 페이지 가져오기)를 허용하는가.
    pub fetch: bool,
    /// 가져올 수 있는 호스트(사람 없는 실행이면 사이트 위임, 결정 0029). `None`이면 출구 검사만.
    pub fetch_hosts: Option<Vec<String>>,
    /// 로그인 세션으로 가져올 수 있는 호스트(`lucia.fetch(url, session=True)`, 결정 0036): 사이트 위임한 호스트.
    /// `None`이면 쓰지 못한다(루틴·스킬은 `fetch_session`을 선언해야 한다).
    pub session_hosts: Option<Vec<String>>,
    /// 파일 읽기·만들기(`lucia.read_file`, `lucia.save_file`, 결정 0047)를 허용하는가.
    pub files: bool,
    /// 코드가 바깥 인터넷에 직접 나가게 하는가(출구 프록시 통행증, 결정 0058). 사람이 대화 중인 실행만.
    pub net: bool,
    pub timeout: Duration,
}

/// 루틴에 주는 효과(`lucia.notify` 등).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Effects {
    /// (글, 묶어 보내기)
    pub notify: Vec<(String, bool)>,
    /// Lucia를 깨울 때: (무엇을 볼지, 자료)
    pub wake: Option<(String, Value)>,
    /// 다음 실행을 지금부터 몇 분 뒤로.
    pub next_minutes: Option<f64>,
    /// 루틴을 끝낸다(까닭).
    pub done: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ScriptOutcome {
    pub ok: bool,
    pub printed: String,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub effects: Effects,
    /// 성공했을 때 남길 상태.
    pub state: Option<Value>,
    /// 부른 도구(fetch는 `fetch <호스트>`).
    pub calls: Vec<String>,
    /// 스크립트가 만든 파일(`lucia.save_file`).
    pub files: Vec<lucia_store::FileRecord>,
    /// 코드가 출구 프록시로 나간 호스트(결정 0058).
    pub net_hosts: Vec<String>,
    /// 출구 프록시가 막은 연결("호스트: 까닭").
    pub net_blocked: Vec<String>,
    pub elapsed: Duration,
}

impl ScriptOutcome {
    fn failed(why: impl Into<String>, calls: Vec<String>, started: Instant) -> Self {
        Self {
            ok: false,
            error: Some(why.into()),
            calls,
            elapsed: started.elapsed(),
            ..Self::default()
        }
    }

    /// 모델에게 돌려줄 글(외부 데이터로 감싸기 전). `max`바이트까지.
    pub fn text(&self, max: usize) -> String {
        let mut out = String::new();
        if !self.ok {
            out.push_str("실패.\n");
        }
        if !self.printed.trim().is_empty() {
            out.push_str("[출력]\n");
            out.push_str(self.printed.trim_end());
            out.push('\n');
        }
        if let Some(v) = &self.result {
            out.push_str("[결과]\n");
            out.push_str(&serde_json::to_string_pretty(v).unwrap_or_default());
            out.push('\n');
        }
        if let Some(e) = &self.error {
            out.push_str("[오류]\n");
            out.push_str(e.trim_end());
            out.push('\n');
            if e.contains("ModuleNotFoundError") {
                out.push_str("(없는 패키지는 lib_install로 설치한 뒤 다시 돌린다)\n");
            }
        }
        if !self.files.is_empty() {
            out.push_str("[만든 파일]\n");
            for f in &self.files {
                out.push_str(&format!(
                    "- id={} · {} · {}{}\n",
                    f.id,
                    f.name,
                    crate::files::human_size(f.size),
                    if f.direction == crate::files::WEB {
                        " · 웹에서 받음"
                    } else {
                        ""
                    }
                ));
            }
        }
        if !self.net_blocked.is_empty() {
            out.push_str("[데몬 프록시가 막은 연결]\n");
            for b in &self.net_blocked {
                out.push_str(&format!("- {b}\n"));
            }
        }
        let fx = &self.effects;
        if !fx.notify.is_empty()
            || fx.wake.is_some()
            || fx.next_minutes.is_some()
            || fx.done.is_some()
        {
            out.push_str("[루틴 효과]\n");
            for (t, batch) in &fx.notify {
                out.push_str(&format!(
                    "- 알림{}: {t}\n",
                    if *batch { "(묶음)" } else { "" }
                ));
            }
            if let Some((note, _)) = &fx.wake {
                out.push_str(&format!("- Lucia 깨우기: {note}\n"));
            }
            if let Some(m) = fx.next_minutes {
                out.push_str(&format!("- 다음 실행: {m}분 뒤\n"));
            }
            if let Some(d) = &fx.done {
                out.push_str(&format!("- 루틴 끝내기: {d}\n"));
            }
        }
        if out.is_empty() {
            out.push_str("(출력 없음)\n");
        }
        out.push_str(&format!(
            "[도구 {}번, {:.1}초]",
            self.calls.len(),
            self.elapsed.as_secs_f64()
        ));
        clip(&out, max)
    }
}

/// 바이트 상한까지 자른다(문자 경계 유지).
pub fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…(길어서 잘림)", &text[..end])
}

/// 스크립트를 띄우는 곳.
pub trait Launcher: Send + Sync {
    /// `tag`는 VM 안에서 이 실행을 찾을 표지, `timeout`은 VM 안 상한.
    fn command(&self, tag: &str, timeout: Duration) -> Command;
    /// 시간을 넘긴 실행을 끝낸다.
    fn kill<'a>(&'a self, tag: &'a str) -> BoxFut<'a, ()>;
}

/// 스크립트 샌드박스(`sbx exec -i`).
pub struct SbxLauncher {
    pub sbx: PathBuf,
    pub sandbox: String,
}

impl Launcher for SbxLauncher {
    fn command(&self, tag: &str, timeout: Duration) -> Command {
        let mut cmd = Command::new(&self.sbx);
        cmd.args([
            "exec",
            "-i",
            &self.sandbox,
            "timeout",
            "-s",
            "KILL",
            "-k",
            "2",
        ])
        .arg(format!("{}", timeout.as_secs().max(1)))
        .args(["python3", "-I", "-u", "-c", HARNESS, tag]);
        cmd
    }

    fn kill<'a>(&'a self, tag: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async move {
            // 첫 글자를 [ ]로 감싸 이 sh 자신의 명령줄은 걸리지 않게 한다.
            let pattern = format!("[{}]{}", &tag[..1], &tag[1..]);
            let script = format!(
                "for p in /proc/[0-9]*; do tr '\\0' ' ' < $p/cmdline 2>/dev/null | grep -q -- '{pattern}' && kill -9 ${{p#/proc/}}; done; true"
            );
            let out = Command::new(&self.sbx)
                .args(["exec", &self.sandbox, "sh", "-c", &script])
                .output()
                .await;
            if let Err(err) = out {
                tracing::warn!(%err, tag, "시간을 넘긴 스크립트를 끝내지 못했다");
            }
        })
    }
}

/// 코드가 부르는 도구를 실제로 부르는 곳.
pub trait ToolBridge: Send + Sync {
    fn session(&self, run: String, grants: Policy) -> Box<dyn ToolSession>;
}

/// 스크립트 실행 하나 동안의 도구 연결.
pub trait ToolSession: Send {
    /// 도구 결과 글(도구가 돌려준 그대로). 연결 실패만 `Err`.
    fn call<'a>(
        &'a mut self,
        name: &'a str,
        args: Map<String, Value>,
    ) -> BoxFut<'a, Result<String, String>>;
    fn close(&mut self) -> BoxFut<'_, ()>;
}

/// 데몬 MCP 서버를 실행별 토큰으로 다시 부른다.
pub struct McpBridge {
    /// `http://127.0.0.1:<포트>/mcp`. 서버가 뜬 뒤 채운다.
    pub url: Arc<OnceLock<String>>,
    pub tokens: RunTokens,
}

impl ToolBridge for McpBridge {
    fn session(&self, run: String, grants: Policy) -> Box<dyn ToolSession> {
        Box::new(McpSession {
            url: self.url.get().cloned(),
            tokens: self.tokens.clone(),
            run,
            grants,
            token: None,
            client: None,
        })
    }
}

struct McpSession {
    url: Option<String>,
    tokens: RunTokens,
    run: String,
    grants: Policy,
    token: Option<String>,
    client: Option<rmcp::service::RunningService<rmcp::RoleClient, ()>>,
}

impl McpSession {
    async fn connect(&mut self) -> Result<(), String> {
        if self.client.is_some() {
            return Ok(());
        }
        let url = self.url.clone().ok_or("데몬 도구 서버 주소가 아직 없다")?;
        let token = self.tokens.issue_with(RunId(self.run.clone()), self.grants);
        self.token = Some(token.clone());
        let transport = rmcp::transport::StreamableHttpClientTransport::from_config(
            rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                url,
            )
            .auth_header(token),
        );
        let client = rmcp::ServiceExt::serve((), transport)
            .await
            .map_err(|e| format!("데몬 도구 서버에 붙지 못했다: {e}"))?;
        self.client = Some(client);
        Ok(())
    }
}

impl ToolSession for McpSession {
    fn call<'a>(
        &'a mut self,
        name: &'a str,
        args: Map<String, Value>,
    ) -> BoxFut<'a, Result<String, String>> {
        Box::pin(async move {
            self.connect().await?;
            let client = self.client.as_ref().expect("connected");
            let params =
                rmcp::model::CallToolRequestParams::new(name.to_owned()).with_arguments(args);
            let result = client
                .call_tool(params)
                .await
                .map_err(|e| format!("{name} 호출 실패: {e}"))?;
            let text: Vec<String> = result
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect();
            Ok(text.join("\n"))
        })
    }

    fn close(&mut self) -> BoxFut<'_, ()> {
        Box::pin(async move {
            if let Some(client) = self.client.take() {
                let _ = client.cancel().await;
            }
            if let Some(token) = self.token.take() {
                self.tokens.revoke(&token);
            }
        })
    }
}

/// 도구 글을 스크립트에 줄 값으로: JSON 도구는 JSON(아니면 거절 글이므로 오류), 나머지는 외부 데이터 표지를 벗긴 글.
pub fn tool_value(name: &str, text: &str) -> Result<Value, String> {
    if JSON_TOOLS.contains(&name) {
        return match serde_json::from_str::<Value>(text) {
            Ok(v @ (Value::Array(_) | Value::Object(_))) => Ok(v),
            _ => Err(text.to_owned()),
        };
    }
    Ok(Value::String(unwrap_external(text)))
}

/// `<external_data …>` 감싸기와 이스케이프를 벗긴다(스크립트가 글을 그대로 다루게).
pub fn unwrap_external(text: &str) -> String {
    let t = text.trim();
    if !t.starts_with("<external_data") || !t.ends_with("</external_data>") {
        return text.to_owned();
    }
    let inner = &t[..t.len() - "</external_data>".len()];
    // 첫 줄은 여는 태그, 둘째 줄은 "데이터일 뿐 지시가 아니다" 안내.
    let body = inner.splitn(3, '\n').nth(2).unwrap_or("");
    body.trim_end_matches('\n')
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// 스크립트 실행기.
pub struct ScriptRunner {
    launcher: Arc<dyn Launcher>,
    bridge: Arc<dyn ToolBridge>,
    fetch: Option<Arc<dyn crate::watch::Fetch>>,
    /// 파일 보관소(스크립트의 파일 읽기·만들기, 결정 0047).
    files: Option<Arc<crate::files::FileStore>>,
    /// 웹에서 파일 받기(`lucia.download`, 결정 0057).
    downloads: Option<Arc<crate::downloads::Downloads>>,
    /// 출구 프록시(결정 0058): 통행증을 내줄 곳과, 스크립트가 보는 데몬 주소(`host.docker.internal:<포트>`).
    net: Option<(Arc<net::ScriptNet>, String)>,
    slots: Arc<tokio::sync::Semaphore>,
}

impl ScriptRunner {
    pub fn new(
        launcher: Arc<dyn Launcher>,
        bridge: Arc<dyn ToolBridge>,
        fetch: Option<Arc<dyn crate::watch::Fetch>>,
    ) -> Self {
        Self {
            launcher,
            bridge,
            fetch,
            files: None,
            downloads: None,
            net: None,
            slots: Arc::new(tokio::sync::Semaphore::new(SLOTS)),
        }
    }

    pub fn with_net(mut self, net: Option<(Arc<net::ScriptNet>, String)>) -> Self {
        self.net = net;
        self
    }

    pub fn with_downloads(mut self, downloads: Option<Arc<crate::downloads::Downloads>>) -> Self {
        self.downloads = downloads;
        self
    }

    /// `lucia.fetch`·`lucia.download`가 이 주소로 나가도 되는가. 되면 (호스트, 방식).
    /// 사람 없는 실행은 위임한 사이트만(결정 0029), 로그인 세션은 위임한 사이트만(결정 0037).
    fn fetch_gate(
        &self,
        job: &ScriptJob,
        url: &str,
        session: bool,
        calls: usize,
    ) -> Result<(String, crate::watch::Via), String> {
        if calls >= MAX_CALLS {
            return Err(format!("도구는 한 번에 {MAX_CALLS}번까지 부른다"));
        }
        let host = url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        let covers = |hosts: &[String]| hosts.iter().any(|g| lucia_store::host_covers(g, &host));
        if session {
            let Some(hosts) = &job.session_hosts else {
                return Err(
                    "이 스크립트에서는 로그인 세션으로 가져올 수 없다(루틴·스킬은 fetch_session을 선언한다)"
                        .into(),
                );
            };
            if self.fetch.is_none() {
                return Err("이 데몬에는 페이지 가져오기가 없다".into());
            }
            if !covers(hosts) {
                return Err(format!(
                    "{host}는 사용자가 위임한 사이트가 아니라 로그인 세션으로 가져오지 않는다"
                ));
            }
            return Ok((host, crate::watch::Via::Session));
        }
        if !job.fetch {
            return Err(
                "이 스크립트에서는 fetch를 쓸 수 없다(브라우저를 쓸 수 있는 실행이거나, 루틴·스킬에 fetch를 선언해야 한다)"
                    .into(),
            );
        }
        if self.fetch.is_none() {
            return Err("이 데몬에는 페이지 가져오기가 없다".into());
        }
        if job.fetch_hosts.as_ref().is_some_and(|hosts| !covers(hosts)) {
            return Err(format!(
                "{host}는 사용자가 위임한 사이트가 아니라 사람 없는 실행에서는 가져오지 않는다"
            ));
        }
        Ok((host, crate::watch::Via::Plain))
    }

    /// `lucia.download`: 주소의 파일을 받아 파일로 남긴다(결정 0057). 나갈 수 있는 곳은 `lucia.fetch`와 같다.
    async fn download_op(
        &self,
        job: &ScriptJob,
        msg: &Value,
        calls: &mut Vec<String>,
        made: &mut Vec<lucia_store::FileRecord>,
    ) -> Result<Value, String> {
        let downloads = match (&self.downloads, job.files) {
            (Some(d), true) => d,
            _ => return Err("이 스크립트에서는 파일을 받을 수 없다".into()),
        };
        if made.len() >= MAX_SAVED_FILES {
            return Err(format!(
                "파일은 한 번에 {MAX_SAVED_FILES}개까지 만들거나 받는다"
            ));
        }
        let url = msg["url"].as_str().unwrap_or("");
        let session = msg["session"].as_bool() == Some(true);
        let (host, via) = self.fetch_gate(job, url, session, calls.len())?;
        calls.push(format!(
            "{} {host}",
            if session {
                "download_session"
            } else {
                "download"
            }
        ));
        let got = downloads
            .fetch_now(url, via, msg["name"].as_str(), Some(&job.run))
            .await?;
        let f = &got.file;
        let value =
            json!({"id": f.id, "name": f.name, "mime": f.mime, "kind": f.kind, "size": f.size});
        made.push(got.file);
        Ok(value)
    }

    pub fn with_files(mut self, files: Option<Arc<crate::files::FileStore>>) -> Self {
        self.files = files;
        self
    }

    /// 스크립트의 파일 요청(`file_read`, `file_save`). 바이트는 base64로 오간다.
    fn file_op(
        &self,
        job: &ScriptJob,
        op: &str,
        msg: &Value,
        calls: &mut Vec<String>,
        made: &mut Vec<lucia_store::FileRecord>,
    ) -> Result<Value, String> {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        if calls.len() >= MAX_CALLS {
            return Err(format!("도구는 한 번에 {MAX_CALLS}번까지 부른다"));
        }
        let files = match (&self.files, job.files) {
            (Some(f), true) => f,
            _ => return Err("이 스크립트에서는 파일을 읽거나 만들 수 없다".into()),
        };
        let describe = |f: &lucia_store::FileRecord| json!({"id": f.id, "name": f.name, "mime": f.mime, "kind": f.kind, "size": f.size});
        if op != "file_save" {
            // 요청의 `id`는 통로의 순번이라 파일 id는 `file`로 온다.
            let id = msg["file"].as_str().unwrap_or("");
            let record = files
                .get(id)
                .ok_or_else(|| format!("그런 파일이 없다: {id}"))?;
            if op == "file_info" {
                calls.push(format!("file_info {}", record.id));
                return Ok(describe(&record));
            }
            if record.size > FILE_READ_MAX {
                return Err(format!(
                    "파일이 너무 크다({}까지)",
                    crate::files::human_size(FILE_READ_MAX)
                ));
            }
            // 큰 파일은 조각으로 보낸다(실행기가 `offset`을 올려 가며 이어 읽는다). 첫 조각만 부른 것으로 센다.
            let offset = msg["offset"].as_u64().unwrap_or(0);
            let bytes = files.read_range(&record, offset, READ_CHUNK)?;
            if offset == 0 {
                calls.push(format!("file_read {}", record.id));
            }
            let mut value = describe(&record);
            value["data"] = json!(b64.encode(bytes));
            return Ok(value);
        }
        if made.len() >= MAX_SAVED_FILES {
            return Err(format!("파일은 한 번에 {MAX_SAVED_FILES}개까지 만든다"));
        }
        let bytes = b64
            .decode(msg["data"].as_str().unwrap_or(""))
            .map_err(|_| "파일 내용을 풀지 못했다".to_owned())?;
        if bytes.len() > FILE_SAVE_MAX {
            return Err(format!(
                "파일이 너무 크다({}까지)",
                crate::files::human_size(FILE_SAVE_MAX as u64)
            ));
        }
        let record = files.save_made(
            msg["name"].as_str().unwrap_or("file"),
            &bytes,
            Some(&job.run),
            chrono::Utc::now(),
        )?;
        calls.push(format!("file_save {}", record.name));
        let value = describe(&record);
        made.push(record);
        Ok(value)
    }

    pub async fn run(&self, job: ScriptJob) -> ScriptOutcome {
        let started = Instant::now();
        let Ok(_slot) = self.slots.acquire().await else {
            return ScriptOutcome::failed("실행기가 닫혔다", Vec::new(), started);
        };
        let tag = format!("lucia-run-{}", uuid::Uuid::new_v4().simple());
        let mut session = self.bridge.session(job.run.clone(), job.grants);
        let mut calls = Vec::new();
        let mut made = Vec::new();
        // 바깥에 나가도 되는 실행이면 통행증을 내준다. 실행이 끝나면(아래에서 버리면) 거두고 열린 연결도 끊는다.
        let pass = self
            .net
            .as_ref()
            .filter(|_| job.net)
            .map(|(net, daemon)| (net.issue(), daemon.as_str()));
        let proxy = pass.as_ref().map(|(p, daemon)| p.proxy_url(daemon));
        let host_limit = job.timeout + Duration::from_secs(8);
        let outcome = tokio::time::timeout(
            host_limit,
            self.drive(
                &job,
                &tag,
                proxy.as_deref(),
                session.as_mut(),
                &mut calls,
                &mut made,
                started,
            ),
        )
        .await;
        session.close().await;
        let (net_hosts, net_blocked) = pass
            .map(|(p, _)| (p.hosts(), p.blocked()))
            .unwrap_or_default();
        let mut outcome = match outcome {
            Ok(o) => o,
            Err(_) => {
                self.launcher.kill(&tag).await;
                ScriptOutcome::failed(
                    format!("시간 초과({}초)", job.timeout.as_secs()),
                    calls,
                    started,
                )
            }
        };
        // 실패해도 이미 만든 파일은 남아 있다: 무엇이 생겼는지 알린다.
        outcome.files = made;
        outcome.net_hosts = net_hosts;
        outcome.net_blocked = net_blocked;
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    async fn drive(
        &self,
        job: &ScriptJob,
        tag: &str,
        proxy: Option<&str>,
        session: &mut dyn ToolSession,
        calls: &mut Vec<String>,
        made: &mut Vec<lucia_store::FileRecord>,
        started: Instant,
    ) -> ScriptOutcome {
        let mut cmd = self.launcher.command(tag, job.timeout);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return ScriptOutcome::failed(
                    format!("스크립트 샌드박스를 띄우지 못했다: {e}"),
                    Vec::new(),
                    started,
                );
            }
        };
        let mut stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        let stderr_task = tokio::spawn(read_capped(stderr, STDERR_MAX));
        let first = json!({
            "code": job.code,
            "input": job.input,
            "state": job.state,
            "printed_max": PRINTED_MAX,
            // 출구 프록시 주소(통행증 포함). 명령줄이 아니라 표준 입력으로 넘긴다(호스트의 프로세스 목록에 남지 않게).
            "proxy": proxy,
        });
        if let Err(e) = stdin.write_all(format!("{first}\n").as_bytes()).await {
            return ScriptOutcome::failed(
                format!("코드를 넘기지 못했다: {e}"),
                Vec::new(),
                started,
            );
        }
        let mut reader = BufReader::new(stdout);
        let mut end: Option<Value> = None;
        loop {
            let line = match read_line_capped(&mut reader, LINE_MAX).await {
                Ok(Some(l)) => l,
                Ok(None) => break,
                Err(e) => {
                    return ScriptOutcome::failed(e, std::mem::take(calls), started);
                }
            };
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let id = msg["id"].clone();
            let reply = match msg["op"].as_str() {
                Some("end") => {
                    end = Some(msg);
                    break;
                }
                Some("call") => {
                    let name = msg["tool"].as_str().unwrap_or("").to_owned();
                    let args = msg["args"].as_object().cloned().unwrap_or_default();
                    match self.check_call(job, &name, calls.len()) {
                        Err(why) => Err(why),
                        Ok(()) => {
                            calls.push(name.clone());
                            match session.call(&name, args).await {
                                Ok(text) => tool_value(&name, &text),
                                Err(e) => Err(e),
                            }
                        }
                    }
                }
                Some("fetch") => {
                    let url = msg["url"].as_str().unwrap_or("").to_owned();
                    let session = msg["session"].as_bool() == Some(true);
                    match (
                        self.fetch_gate(job, &url, session, calls.len()),
                        &self.fetch,
                    ) {
                        (Err(why), _) => Err(why),
                        (Ok(_), None) => Err("이 데몬에는 페이지 가져오기가 없다".into()),
                        (Ok((host, via)), Some(f)) if session => {
                            calls.push(format!("fetch_session {host}"));
                            f.fetch_via(&url, via)
                                .await
                                .map(Value::String)
                                .map_err(|e| e.to_string())
                        }
                        (Ok((host, _)), Some(f)) => {
                            calls.push(format!("fetch {host}"));
                            f.fetch(&url).await.map(Value::String)
                        }
                    }
                }
                Some("download") => self.download_op(job, &msg, calls, made).await,
                Some(op @ ("file_read" | "file_info" | "file_save")) => {
                    self.file_op(job, op, &msg, calls, made)
                }
                _ => Err("모르는 요청".to_owned()),
            };
            let answer = match reply {
                Ok(value) => json!({"id": id, "ok": true, "value": value}),
                Err(error) => {
                    json!({"id": id, "ok": false, "error": crate::mask::global().mask(&error)})
                }
            };
            if stdin
                .write_all(format!("{answer}\n").as_bytes())
                .await
                .is_err()
            {
                break;
            }
        }
        drop(stdin);
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        let stderr = stderr_task.await.unwrap_or_default();
        match end {
            Some(end) => finish(end, std::mem::take(calls), started),
            None if started.elapsed() + Duration::from_millis(300) >= job.timeout => {
                ScriptOutcome::failed(
                    format!("시간 초과({}초)", job.timeout.as_secs().max(1)),
                    std::mem::take(calls),
                    started,
                )
            }
            None => {
                let tail = stderr.trim();
                ScriptOutcome::failed(
                    if tail.is_empty() {
                        "스크립트가 끝 보고 없이 멈췄다(시간 초과나 메모리 부족일 수 있다)"
                            .to_owned()
                    } else {
                        format!("스크립트가 끝 보고 없이 멈췄다:\n{tail}")
                    },
                    std::mem::take(calls),
                    started,
                )
            }
        }
    }

    fn check_call(&self, job: &ScriptJob, name: &str, made: usize) -> Result<(), String> {
        if made >= MAX_CALLS {
            return Err(format!("도구는 한 번에 {MAX_CALLS}번까지 부른다"));
        }
        if NEVER.contains(&name) {
            return Err(format!("스크립트에서는 {name}을(를) 부를 수 없다"));
        }
        if let Some(declared) = &job.declared
            && !declared.iter().any(|d| d == name)
        {
            return Err(format!(
                "선언하지 않은 도구 {name}: 이 스크립트가 쓸 도구는 만들 때 tools에 적는다"
            ));
        }
        if !job.grants.allows(name) {
            return Err(format!("이 스크립트에서는 {name}을(를) 쓸 수 없다"));
        }
        Ok(())
    }
}

/// 끝 보고를 결과로.
fn finish(end: Value, calls: Vec<String>, started: Instant) -> ScriptOutcome {
    let fx = &end["effects"];
    let notify = fx["notify"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|n| {
                    let t = n["text"].as_str()?.trim();
                    (!t.is_empty()).then(|| (t.to_owned(), n["batch"].as_bool().unwrap_or(false)))
                })
                .collect()
        })
        .unwrap_or_default();
    let wake = fx["wake"].as_object().map(|w| {
        (
            w["note"].as_str().unwrap_or("").to_owned(),
            w["data"].clone(),
        )
    });
    let mut result = match end["result"].clone() {
        Value::Null => None,
        v => Some(v),
    };
    if let Some(v) = &result {
        let s = v.to_string();
        if s.len() > RESULT_MAX {
            result = Some(Value::String(clip(&s, RESULT_MAX)));
        }
    }
    let ok = end["ok"].as_bool().unwrap_or(false);
    let state = end["state"].clone();
    let state = (ok && state.is_object() && state.to_string().len() <= STATE_MAX).then_some(state);
    let mut error = end["error"].as_str().map(str::to_owned);
    if ok && state.is_none() && end["state"].is_object() {
        error = Some(format!(
            "lucia.state가 {}KB를 넘어 남기지 않았다",
            STATE_MAX / 1024
        ));
    }
    ScriptOutcome {
        ok,
        printed: end["printed"].as_str().unwrap_or("").to_owned(),
        result,
        error,
        effects: Effects {
            notify,
            wake,
            next_minutes: fx["next"].as_f64(),
            done: fx["done"].as_str().map(str::to_owned),
        },
        state,
        calls,
        files: Vec::new(),
        net_hosts: Vec::new(),
        net_blocked: Vec::new(),
        elapsed: started.elapsed(),
    }
}

/// 상한까지 읽고 나머지는 버린다.
async fn read_capped<R: AsyncRead + Unpin>(mut r: R, max: usize) -> String {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = max.saturating_sub(out.len());
                out.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 줄 하나(상한 있음). 끝이면 `None`.
async fn read_line_capped<R: AsyncBufReadExt + Unpin>(
    r: &mut R,
    max: usize,
) -> Result<Option<String>, String> {
    let mut line = Vec::new();
    loop {
        let buf = r.fill_buf().await.map_err(|e| e.to_string())?;
        if buf.is_empty() {
            return Ok((!line.is_empty()).then(|| String::from_utf8_lossy(&line).into_owned()));
        }
        match buf.iter().position(|b| *b == b'\n') {
            Some(i) => {
                line.extend_from_slice(&buf[..i]);
                r.consume(i + 1);
                return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
            }
            None => {
                let n = buf.len();
                line.extend_from_slice(buf);
                r.consume(n);
            }
        }
        if line.len() > max {
            return Err("스크립트가 보낸 줄이 너무 길다".into());
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
