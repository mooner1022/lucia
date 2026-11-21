//! 실제 에이전트 실행기: 상주 샌드박스에서 `sbx exec … claude -p`(PoC P2·P4에서 정한 조합).
//!
//! 실행마다 MCP 토큰을 발급해 `--mcp-config`에 넣고, 끝나면 회수한다. 그 토큰으로 들어온 보고만
//! 이 실행의 보고로 친다.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lucia_runner::{ClaudeCommand, RunOutput, SessionMode, sbx_exec_prefix};
use tokio::sync::mpsc;

use crate::agent::{
    AgentRequest, AgentResponse, AgentRunner, RunEvent, RunProfile, RunUsage, SessionStart,
};
use crate::mcp::{Report, ReportResult, RunId, RunTokens};

pub const MCP_SERVER_NAME: &str = "lucia";

/// 모든 실행에 여는 내장 도구: 웹 검색(Anthropic 서버가 검색하므로 VM 네트워크 출구가 필요 없다, 결정 0003).
pub const BUILTIN_TOOLS: &[&str] = &["WebSearch"];

/// 사람이 대화 중인 실행에 여는 Claude Code 내장 도구(결정 0059): 쉘과 파일. 상주 VM 안에서 돈다.
/// 어느 실행에 무엇을 열었는지는 환경 변수 `LUCIA_BUILTIN_TOOLS`로 훅(`plugin/hooks/`)에 알린다(훅은 그 이름만
/// 통과시킨다). 웹 페이지 열기(WebFetch)는 열지 않는다: Claude Code 자신의 연결이라 출구 프록시를 타지 않는다.
pub const CHAT_TOOLS: &[&str] = &["Bash", "Read", "Edit", "Write", "Glob", "Grep"];

/// 상주 VM의 내장 도구에 딸린 것(결정 0059).
pub struct NativeTools {
    /// VM 안의 Bash 환경 파일(`CLAUDE_ENV_FILE`): 통행증이 있으면 쉘 명령을 출구 프록시로 보낸다.
    pub env_file: Option<String>,
    /// 출구 프록시(결정 0058)와 VM에서 본 그 주소. 없으면 쉘은 바깥에 나가지 못한다.
    pub net: Option<(Arc<crate::script::net::ScriptNet>, String)>,
    pub store: Arc<Mutex<lucia_store::Store>>,
}

/// 개발 작업(결정 0020)의 임시 샌드박스에서 여는 내장 도구. 경계는 VM과 네트워크 정책이다.
pub const DEV_TOOLS: &[&str] = &["Bash", "Read", "Edit", "Write", "Glob", "Grep", "WebSearch"];

/// 데몬 실행에서 허용하는 도구: 도구 선언([`crate::tools::TOOLS`])과 실행 종류의 범위로 계산한다(결정 0013).
/// MCP 서버도 같은 계산으로 다시 확인한다.
pub fn allowed_tools(profile: RunProfile, web_search: bool) -> Vec<String> {
    let grants = profile.grants();
    let mut tools: Vec<String> = grants
        .tool_names()
        .map(|t| format!("mcp__{MCP_SERVER_NAME}__{t}"))
        .collect();
    // 도구 찾기·부르기(결정 0022): 보고 말고도 쓸 도구가 있는 실행. 찾은 도구는 서버가 실행 범위로 다시 본다.
    if grants.tool_names().any(|t| t != "report_result") {
        for t in [crate::tools::SEARCH_TOOL, crate::tools::CALL_TOOL] {
            tools.push(format!("mcp__{MCP_SERVER_NAME}__{t}"));
        }
    }
    // 내장 웹 검색은 읽기다. 보고만 하거나 후속 조치만 등록하는 실행에는 주지 않는다.
    if web_search && grants.has_group(crate::tools::Group::Google) {
        tools.extend(BUILTIN_TOOLS.iter().map(|t| (*t).to_owned()));
    }
    tools
}

/// 재개하려던 세션이 VM에 없을 때의 모습(2026-11-16 실측): `result` 한 줄,
/// `subtype=error_during_execution`, `num_turns=0`, stderr에 `No conversation found with session ID`.
pub fn is_missing_session(out: &RunOutput) -> bool {
    out.stderr.contains("No conversation found with session ID")
        && out.result().is_none_or(|r| r.is_error && r.num_turns == 0)
}

/// 실행별 보고함. MCP 서버의 보고를 실행 ID로 나눠 담는다.
#[derive(Clone, Default)]
pub struct ReportInbox(Arc<Mutex<HashMap<RunId, Vec<ReportResult>>>>);

impl ReportInbox {
    /// MCP 서버가 보내는 보고를 받아 담는 작업을 띄운다.
    pub fn spawn_collector(&self, mut rx: mpsc::UnboundedReceiver<Report>) {
        let inbox = self.0.clone();
        tokio::spawn(async move {
            while let Some(report) = rx.recv().await {
                match report.run {
                    Some(run) => inbox
                        .lock()
                        .expect("inbox lock")
                        .entry(run)
                        .or_default()
                        .push(report.result),
                    None => tracing::warn!("실행 ID 없는 보고를 버렸다"),
                }
            }
        });
    }

    pub fn take(&self, run: &RunId) -> Vec<ReportResult> {
        self.0
            .lock()
            .expect("inbox lock")
            .remove(run)
            .unwrap_or_default()
    }
}

pub struct ClaudeAgentRunner {
    pub sbx: PathBuf,
    pub sandbox: String,
    pub workdir: String,
    pub plugin_dir: Option<String>,
    pub character_file: Option<String>,
    /// VM 안에서 본 MCP URL(`http://host.docker.internal:<포트>/mcp`).
    pub mcp_url: String,
    pub tokens: RunTokens,
    pub inbox: ReportInbox,
    pub timeout: Duration,
    /// 작업 세션 실행의 시간 제한(결정 0014: 조사·여러 사이트 확인은 채팅보다 길다).
    pub task_timeout: Duration,
    /// 내장 웹 검색을 열지(`sandbox.web_search`).
    pub web_search: bool,
    /// 사람이 대화 중인 실행의 시간 제한(결정 0059: 쉘로 하는 긴 일을 끊지 않게 따로 둔다).
    pub chat_timeout: Duration,
    /// 사람이 대화 중인 실행에 내장 도구를 연다(`sandbox.native_tools`). `None`이면 열지 않는다.
    pub native: Option<NativeTools>,
}

/// 첫 시도에서 API 응답 머리를 기다리는 시간(밀리초) [제안]. 공식 문서(env-vars, errors): 설정하지 않으면 직접 API는
/// 180초 + 요청 본문 32KB마다 1초, 설정하면 10초~30분. 긴 생각은 머리 뒤의 본문이라 이 시간과 상관없다.
/// 상주 세션의 큰 요청(그림이 쌓인 수백 KB)도 머리가 늦지 않게 넉넉히 90초로 둔다.
const FIRST_BYTE_TIMEOUT_MS: &str = "90000";

impl ClaudeAgentRunner {
    /// 이 실행에 내장 도구를 여는가: 사람이 대화 중인 상주 VM의 실행만(결정 0059).
    fn native_for(&self, request: &AgentRequest) -> Option<&NativeTools> {
        self.native
            .as_ref()
            .filter(|_| request.profile == RunProfile::Chat && request.target.is_none())
    }

    fn command(&self, request: &AgentRequest, token: &str, proxy: Option<&str>) -> ClaudeCommand {
        let mode = match request.session {
            SessionStart::New(id) => SessionMode::New(id),
            SessionStart::Resume(id) => SessionMode::Resume(id),
        };
        let native = self.native_for(request);
        let mut builtin: Vec<String> = if self.web_search {
            BUILTIN_TOOLS.iter().map(|t| (*t).to_owned()).collect()
        } else {
            Vec::new()
        };
        if native.is_some() {
            builtin.extend(CHAT_TOOLS.iter().map(|t| (*t).to_owned()));
        }
        let mut env: Vec<(String, String)> = [
            ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1"),
            // 2분 넘은 MCP 호출이 백그라운드로 넘어가 -p에서 결과를 잃지 않게(문서, PoC P6).
            ("CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS", "0"),
            // API가 응답 머리를 주지 않을 때 첫 시도를 기다리는 시간(2026-11-19 실측: 기본으로 186초를 기다린 뒤에야
            // 다시 보냈다). 공식 문서의 CLAUDE_STREAM_FIRST_BYTE_TIMEOUT_MS. 다시 보낼 때의 대기는 Claude Code가 정한다.
            ("CLAUDE_STREAM_FIRST_BYTE_TIMEOUT_MS", FIRST_BYTE_TIMEOUT_MS),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .to_vec();
        // 훅이 통과시킬 내장 도구(결정 0059). 훅은 이 값이 없으면 WebSearch만 통과시킨다.
        env.push(("LUCIA_BUILTIN_TOOLS".to_owned(), builtin.join(",")));
        if let Some(native) = native {
            if let Some(file) = &native.env_file {
                env.push(("CLAUDE_ENV_FILE".to_owned(), file.clone()));
            }
            // 훅이 되돌릴 수 없는 쉘 명령의 승인을 데몬에 묻는 창구와, 이 실행의 토큰.
            env.push((
                "LUCIA_APPROVAL_URL".to_owned(),
                format!("{}/approval", self.mcp_url.trim_end_matches("/mcp")),
            ));
            env.push(("LUCIA_RUN_TOKEN".to_owned(), token.to_owned()));
            // 쉘 명령이 쓸 출구 프록시(통행증 포함). 환경 파일이 HTTPS_PROXY로 옮긴다. claude 자신은 쓰지 않는다.
            if let Some(proxy) = proxy {
                env.push(("LUCIA_PROXY".to_owned(), proxy.to_owned()));
            }
        }
        let mut cmd = ClaudeCommand::new(&self.sbx, &self.workdir, mode);
        cmd.prefix = sbx_exec_prefix(&self.sandbox, Some(&self.workdir), &env);
        cmd.model = Some(request.model.clone());
        cmd.mcp_config = Some(
            serde_json::json!({
                "mcpServers": { MCP_SERVER_NAME: {
                    "type": "http",
                    "url": self.mcp_url,
                    "headers": { "Authorization": format!("Bearer {token}") },
                }}
            })
            .to_string(),
        );
        cmd.restricted = true;
        cmd.tools = Some(builtin);
        cmd.allowed_tools = allowed_tools(request.profile, self.web_search);
        if native.is_some() {
            cmd.allowed_tools
                .extend(CHAT_TOOLS.iter().map(|t| (*t).to_owned()));
        }
        cmd.permission_prompts = Some("none".into());
        cmd.plugin_dirs = self.plugin_dir.iter().cloned().collect();
        cmd.append_system_prompt_file = self.character_file.as_ref().map(PathBuf::from);
        // 개발 작업(결정 0020): 임시 샌드박스에서, 코드 도구를 열고, 상주 샌드박스의 plugin·캐릭터 마운트는 없다.
        if let Some(target) = &request.target {
            cmd.prefix = sbx_exec_prefix(&target.sandbox, Some(&target.workdir), &env);
            cmd.cwd = PathBuf::from(&self.workdir);
            cmd.plugin_dirs = Vec::new();
            cmd.append_system_prompt_file = None;
            if request.profile == RunProfile::Dev {
                cmd.tools = Some(DEV_TOOLS.iter().map(|t| (*t).to_owned()).collect());
                cmd.allowed_tools
                    .extend(DEV_TOOLS.iter().map(|t| (*t).to_owned()));
            }
        }
        if let Some(budget) = request.budget_usd {
            cmd.extra_args
                .extend(["--max-budget-usd".to_owned(), format!("{budget:.2}")]);
        }
        cmd.timeout = match request.profile {
            RunProfile::Task | RunProfile::TaskLimited | RunProfile::Dev => self.task_timeout,
            // 사람이 보고 있는 대화는 길게 둔다(멈추려면 "그만", 결정 0051). 정기 실행은 예전 상한 그대로다:
            // 멈춘 브리핑이 줄을 오래 막지 않게.
            RunProfile::Chat => self.chat_timeout,
            _ => self.timeout,
        };
        cmd
    }
}

/// stream-json 결과를 사용량과 오류 사유로 바꾼다.
pub fn summarize(out: &RunOutput) -> (RunUsage, Option<String>) {
    let Some(r) = out.result() else {
        let stderr: String = out.stderr.lines().take(3).collect::<Vec<_>>().join(" | ");
        return (
            RunUsage::default(),
            Some(format!(
                "결과 이벤트 없이 끝났다(종료 코드 {:?}): {stderr}",
                out.exit_code
            )),
        );
    };
    let usage = RunUsage {
        model: out.init().and_then(|i| i.model.clone()),
        input_tokens: r.usage.input_tokens,
        output_tokens: r.usage.output_tokens,
        cache_read_tokens: r.usage.cache_read_input_tokens,
        cache_creation_tokens: r.usage.cache_creation_input_tokens,
        context_tokens: r.context_tokens(),
        cost_usd: r.total_cost_usd.unwrap_or(0.0),
        duration_ms: r.duration_ms,
        num_turns: r.num_turns,
    };
    // 로그인 오류는 subtype=success에 is_error=true로 온다(PoC P1). is_error를 먼저 본다.
    let error = r.is_error.then(|| {
        format!(
            "{} / {}: {}",
            r.subtype,
            r.terminal_reason.as_deref().unwrap_or("?"),
            r.result.as_deref().unwrap_or("")
        )
    });
    (usage, error)
}

impl AgentRunner for ClaudeAgentRunner {
    async fn kill_orphans(&self) {
        self.kill_leftovers().await;
    }

    async fn stop(&self, run_id: &str) -> bool {
        match self.tokens.token_of(run_id) {
            Some(token) => self.kill_in_vm(&token).await == "끝냄",
            None => false,
        }
    }

    async fn run(&self, request: AgentRequest) -> AgentResponse {
        let run = RunId(request.run_id.clone());
        let token = self
            .tokens
            .issue_with(run.clone(), request.profile.grants());
        // 쉘의 인터넷 통행증(결정 0058·0059): 내장 도구를 여는 실행에만, 나가는 글 검사가 "막기"가 아닐 때
        // (프록시는 TLS 안을 보지 못한다). 실행이 끝나면 거두고 열린 연결도 끊는다.
        let pass = self.native_for(&request).and_then(|native| {
            let (net, daemon) = native.net.as_ref()?;
            let blocked = native
                .store
                .lock()
                .map_or(true, |store| crate::leak::blocking(&store));
            (!blocked).then(|| (net.issue(), daemon.clone()))
        });
        let proxy = pass.as_ref().map(|(p, daemon)| p.proxy_url(daemon));
        let mut cmd = self.command(&request, &token, proxy.as_deref());
        // 받을 곳이 있으면(채팅) 부분 메시지를 켜서 report_result의 summary를 받는 대로 넘긴다(결정 0005).
        cmd.include_partial_messages = request.events.is_some();
        let sink = request.events.clone();
        let mut report = lucia_runner::report_stream::ReportStream::new(
            &format!("mcp__{MCP_SERVER_NAME}__report_result"),
            "summary",
        );
        // 개발용: stdout 원본을 기록한다(픽스처 만들기). 운영 데몬에서는 설정하지 않는다.
        let record = std::env::var_os("LUCIA_RECORD_DIR")
            .map(|d| PathBuf::from(d).join(format!("{}.jsonl", request.run_id)));
        let result = cmd
            .run_streaming(
                &request.prompt,
                &request.blocks,
                record.as_deref(),
                |event| {
                    // API 다시 보내기는 받을 곳이 없어도(브리핑·루틴) 로그에 남긴다.
                    if let lucia_runner::Event::ApiRetry(r) = event {
                        tracing::warn!(
                            run = %request.run_id,
                            attempt = r.attempt,
                            max = r.max_retries,
                            status = ?r.status,
                            waited_ms = ?r.waited_ms,
                            error = %r.error,
                            "모델 API 요청을 다시 보낸다"
                        );
                    }
                    let Some(sink) = &sink else { return };
                    match event {
                        lucia_runner::Event::ApiRetry(r) => {
                            let _ = sink.send(RunEvent::Retrying {
                                attempt: r.attempt,
                                max_retries: r.max_retries,
                                waited_ms: r.waited_ms,
                                error: r.error.clone(),
                            });
                        }
                        lucia_runner::Event::StreamEvent(ev) => {
                            if let Some(so_far) = report.feed(ev) {
                                let _ = sink.send(RunEvent::ReportSoFar(so_far));
                            }
                        }
                        lucia_runner::Event::Assistant(raw) => {
                            for (name, input) in lucia_runner::stream_json::tool_calls(raw) {
                                let _ = sink.send(RunEvent::ToolUse { name, input });
                            }
                        }
                        _ => {}
                    }
                },
            )
            .await;
        self.tokens.revoke(&token);
        // 쉘이 나간 곳을 활동 기록에 남기고 통행증을 거둔다.
        if let (Some((pass, _)), Some(native)) = (pass, self.native.as_ref()) {
            let (hosts, blocked) = (pass.hosts(), pass.blocked());
            drop(pass);
            if (!hosts.is_empty() || !blocked.is_empty())
                && let Ok(store) = native.store.lock()
            {
                let mut text = format!(
                    "쉘이 나간 곳: {}",
                    if hosts.is_empty() {
                        "없음".to_owned()
                    } else {
                        hosts.join(", ").chars().take(300).collect()
                    }
                );
                if !blocked.is_empty() {
                    text.push_str(&format!(
                        " · 막은 연결 {}개({})",
                        blocked.len(),
                        blocked[0].chars().take(80).collect::<String>()
                    ));
                }
                let _ = store.add_activity(
                    "shell_net",
                    &crate::mask::global().mask(&text),
                    Some(&request.run_id),
                    chrono::Utc::now(),
                );
            }
        }
        let reports = self.inbox.take(&run);
        match result {
            Ok(out) => {
                let (usage, mut error) = summarize(&out);
                let session_missing = is_missing_session(&out);
                if session_missing {
                    error = Some("재개할 세션이 VM에 없다(No conversation found)".into());
                }
                if !out.parse_errors.is_empty() {
                    tracing::warn!(count = out.parse_errors.len(), "stream-json 파싱 오류");
                }
                let compaction = out.compact_boundary().map(|b| crate::agent::Compaction {
                    pre_tokens: b.pre_tokens,
                    post_tokens: b.post_tokens,
                    summary: out.compact_summary().map(str::to_owned),
                });
                let tool_calls = out.tool_calls();
                // 보고를 여러 번 했으면 하나로 합친다(모델의 실수를 실패로 만들지 않는다).
                let given = reports.len();
                let reports = crate::agent::merge_reports(reports, &tool_calls);
                if given > 1 {
                    tracing::warn!(run = %request.run_id, given, "보고가 여러 번 와서 첫 보고를 기준으로 합쳤다");
                }
                AgentResponse {
                    reports,
                    usage,
                    error,
                    session_missing,
                    compaction,
                    tool_calls,
                }
            }
            Err(err) => {
                // 호스트의 sbx exec를 끝내도 VM 안 claude는 끝까지 돈다(2026-11-16 실측: 시간 초과 7초 뒤까지
                // 실행해 토큰을 쓰고 세션에 턴을 남겼다). 실행마다 다른 토큰이 명령줄에 있으므로 그것으로 골라 끝낸다.
                let cleanup = if matches!(err, lucia_runner::RunError::Timeout(_)) {
                    self.kill_in_vm(&token).await
                } else {
                    "정리 안 함".into()
                };
                AgentResponse {
                    reports: crate::agent::merge_reports(reports, &[]),
                    usage: RunUsage::default(),
                    error: Some(format!("실행 실패: {err} (VM 안 프로세스: {cleanup})")),
                    session_missing: false,
                    compaction: None,
                    tool_calls: Vec::new(),
                }
            }
        }
    }
}

impl ClaudeAgentRunner {
    /// 다시 뜰 때: VM에 남은 예전 실행을 모두 끝낸다(결정 0050). 호스트의 `sbx exec`가 죽어도 VM 안의 claude는
    /// 끝까지 돌아 세션에 턴을 남긴다. 데몬이 띄운 실행은 명령줄에 실행별 토큰(16진수 64자)이 있다.
    async fn kill_leftovers(&self) {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            tokio::process::Command::new(&self.sbx)
                .args([
                    "exec",
                    &self.sandbox,
                    "pkill",
                    "-f",
                    "--",
                    "Bearer [0-9a-f]{64}",
                ])
                .kill_on_drop(true)
                .output(),
        )
        .await;
        match output {
            Ok(Ok(o)) if o.status.code() == Some(0) => {
                tracing::warn!("앞선 데몬이 남긴 실행을 VM에서 끝냈다");
            }
            Ok(Ok(_)) => {}
            Ok(Err(err)) => tracing::warn!(%err, "남은 실행을 확인하지 못했다"),
            Err(_) => tracing::warn!("남은 실행 확인이 시간 안에 끝나지 않았다"),
        }
    }

    /// VM 안에서 이 실행의 claude와 그것이 띄운 프로세스(쉘 명령, 결정 0059)를 끝낸다. 명령줄의 실행별 토큰으로
    /// claude를 고르고, 그 자식들을 따라 내려가 함께 끝낸다. 스스로 떨어져 나간 프로세스(setsid 등)는 남을 수 있다.
    async fn kill_in_vm(&self, token: &str) -> String {
        let output = tokio::process::Command::new(&self.sbx)
            .args(["exec", &self.sandbox, "sh", "-c", &kill_tree_script(token)])
            .output()
            .await;
        match output.map(|o| o.status.code()) {
            Ok(Some(0)) => "끝냄".into(),
            Ok(Some(1)) => "이미 없음".into(),
            Ok(code) => format!("끝내기 실패(코드 {code:?})"),
            Err(err) => format!("끝내기 실행 실패: {err}"),
        }
    }
}

/// 토큰이 명령줄에 있는 프로세스와 그 자손을 모두 끝내는 sh 스크립트. 없으면 1로 끝난다.
/// 첫 글자를 `[ ]`로 감싸 이 스크립트를 돌리는 sh 자신의 명령줄은 걸리지 않게 한다.
fn kill_tree_script(token: &str) -> String {
    format!(
        "roots=$(pgrep -f -- '[B]earer {token}') || exit 1\n\
         all=\"$roots\"; frontier=\"$roots\"\n\
         while [ -n \"$frontier\" ]; do\n\
           next=\"\"\n\
           for p in $frontier; do next=\"$next $(pgrep -P \"$p\" | tr '\\n' ' ')\"; done\n\
           frontier=$(echo $next)\n\
           all=\"$all $frontier\"\n\
         done\n\
         kill -9 $all 2>/dev/null\n\
         exit 0"
    )
}

#[cfg(test)]
mod tests {
    use lucia_runner::stream_json::parse_line;

    use super::*;

    /// 훅(`plugin/hooks/`)을 실제로 돌려 본다: 데몬이 알려 준 도구만 통과시키고, 바깥에 쓰는 쉘 명령은 승인으로
    /// 돌린다(결정 0059). 승인 창구가 없으면 막는다.
    #[test]
    fn the_hook_lets_through_only_listed_tools_and_gates_risky_shell_commands() {
        use std::io::Write as _;
        if !crate::script::tests::have_python() {
            eprintln!("python3가 없어 건너뜀");
            return;
        }
        let hook = format!(
            "{}/../../plugin/hooks/only-lucia-tools.sh",
            env!("CARGO_MANIFEST_DIR")
        );
        let run = |tools: Option<&str>, event: serde_json::Value| -> (i32, String) {
            let mut cmd = std::process::Command::new("sh");
            cmd.arg(&hook)
                .env_remove("LUCIA_BUILTIN_TOOLS")
                .env_remove("LUCIA_APPROVAL_URL")
                .env_remove("LUCIA_RUN_TOKEN")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped());
            if let Some(tools) = tools {
                cmd.env("LUCIA_BUILTIN_TOOLS", tools);
            }
            let mut child = cmd.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(event.to_string().as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            (
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        };
        let bash = |command: &str| serde_json::json!({"tool_name": "Bash", "tool_input": {"command": command}});
        let chat = Some("WebSearch,Bash,Read,Edit,Write,Glob,Grep");
        // 데몬이 알려 주지 않으면 예전 규칙: 데몬 도구와 웹 검색만.
        assert_eq!(
            run(
                None,
                serde_json::json!({"tool_name": "mcp__lucia__report_result"})
            )
            .0,
            0
        );
        assert_eq!(
            run(None, serde_json::json!({"tool_name": "WebSearch"})).0,
            0
        );
        assert_eq!(run(None, bash("ls")).0, 2);
        assert_eq!(run(None, serde_json::json!({})).0, 2);
        // 사람 없는 실행(웹 검색만 알림)과 대화.
        assert_eq!(run(Some("WebSearch"), bash("ls")).0, 2);
        assert_eq!(
            run(Some(""), serde_json::json!({"tool_name": "WebSearch"})).0,
            2
        );
        assert_eq!(run(chat, serde_json::json!({"tool_name": "Read"})).0, 0);
        assert_eq!(run(chat, serde_json::json!({"tool_name": "WebFetch"})).0, 2);
        for fine in [
            "ls -la && git status && git log -3",
            "git clone https://github.com/a/b && git pull --rebase && git commit -m fix",
            "curl -fsSL https://example.com/a.json -o a.json",
            "gh pr view 3 && gh repo list && gh api repos/a/b/issues",
            "sudo apt-get install -y ffmpeg && pip install requests",
            "grep -r ssh_config . | wc -l",
        ] {
            assert_eq!(run(chat, bash(fine)).0, 0, "{fine}");
        }
        // 바깥에 쓰는 명령은 승인으로 간다. 승인 창구가 없으면 막는다.
        for risky in [
            "git push origin main",
            "cd repo && git -C . push",
            "gh release create v1.0",
            "gh pr merge 3",
            "gh api -X POST /repos/a/b/issues",
            "gh api repos/a/b/issues -f title=x",
            "npm publish",
            "cargo publish",
            "docker push a/b",
            "twine upload dist/*",
            "curl -X POST https://x.example/api -d a=1",
            "curl -F file=@a https://x.example",
            "curl --json '{}' https://x.example",
            "wget --post-data=a=1 https://x.example",
            "ssh user@host ls",
            "rsync -a . host:/srv",
            "kubectl apply -f a.yaml",
            "http POST https://x.example a=1",
        ] {
            let (code, why) = run(chat, bash(risky));
            assert_eq!(code, 2, "{risky}");
            assert!(why.contains("승인"), "{risky}: {why}");
        }
    }

    /// 같은 설정의 실행기를 하나 더(필드를 바꿔 보려고).
    fn runner_like(r: &ClaudeAgentRunner) -> ClaudeAgentRunner {
        ClaudeAgentRunner {
            sbx: r.sbx.clone(),
            sandbox: r.sandbox.clone(),
            workdir: r.workdir.clone(),
            plugin_dir: r.plugin_dir.clone(),
            character_file: r.character_file.clone(),
            mcp_url: r.mcp_url.clone(),
            tokens: r.tokens.clone(),
            inbox: r.inbox.clone(),
            timeout: r.timeout,
            task_timeout: r.task_timeout,
            web_search: r.web_search,
            chat_timeout: r.chat_timeout,
            native: None,
        }
    }

    fn fixture(name: &str) -> RunOutput {
        let path = format!(
            "{}/../lucia-runner/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let events = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter_map(|l| parse_line(l).unwrap())
            .collect();
        RunOutput {
            events,
            parse_errors: vec![],
            exit_code: Some(0),
            stderr: String::new(),
        }
    }

    #[test]
    fn summarize_success_uses_last_turn_for_context() {
        let (usage, error) = summarize(&fixture("host-2.1.325-poc-request1.jsonl"));
        assert!(error.is_none());
        assert_eq!(usage.context_tokens, 4740);
        assert_eq!(
            usage.input_tokens + usage.cache_read_tokens + usage.cache_creation_tokens,
            9275
        );
        assert!(usage.cost_usd > 0.0);
        assert_eq!(usage.model.as_deref(), Some("claude-haiku-5-5"));
    }

    #[test]
    fn summarize_reports_missing_result() {
        let out = RunOutput {
            events: vec![],
            parse_errors: vec![],
            exit_code: Some(1),
            stderr: "Error: bypassPermissions not supported in restricted mode\n".into(),
        };
        let (_, error) = summarize(&out);
        assert!(error.unwrap().contains("restricted mode"));
    }

    #[tokio::test]
    async fn inbox_routes_reports_by_run() {
        let inbox = ReportInbox::default();
        let (tx, rx) = mpsc::unbounded_channel();
        inbox.spawn_collector(rx);
        let report = |run: &str, s: &str| Report {
            run: Some(RunId(run.into())),
            result: ReportResult {
                status: crate::mcp::ReportStatus::Done,
                summary: s.into(),
                details: None,
                attachments: Vec::new(),
                blocked: None,
            },
        };
        tx.send(report("a", "첫째")).unwrap();
        tx.send(report("b", "둘째")).unwrap();
        tx.send(report("a", "셋째")).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let a = inbox.take(&RunId("a".into()));
        assert_eq!(
            a.iter().map(|r| r.summary.as_str()).collect::<Vec<_>>(),
            ["첫째", "셋째"]
        );
        assert_eq!(inbox.take(&RunId("b".into())).len(), 1);
        assert!(
            inbox.take(&RunId("a".into())).is_empty(),
            "꺼낸 뒤에는 비어 있다"
        );
    }

    #[test]
    fn command_uses_the_poc_profile() {
        let runner = ClaudeAgentRunner {
            sbx: "sbx".into(),
            sandbox: "lucia".into(),
            workdir: "/work".into(),
            plugin_dir: Some("/opt/lucia/plugin".into()),
            character_file: Some("/opt/lucia/character/character.md".into()),
            mcp_url: "http://host.docker.internal:47810/mcp".into(),
            tokens: RunTokens::default(),
            inbox: ReportInbox::default(),
            timeout: Duration::from_secs(30),
            task_timeout: Duration::from_secs(30),
            web_search: true,
            chat_timeout: Duration::from_secs(3600),
            native: Some(NativeTools {
                env_file: Some("/opt/lucia/plugin/env/bash-env.sh".into()),
                net: None,
                store: Arc::new(Mutex::new(lucia_store::Store::open_in_memory().unwrap())),
            }),
        };
        let request = AgentRequest {
            run_id: "r".into(),
            session: SessionStart::Resume(uuid::Uuid::nil()),
            model: "sonnet".into(),
            prompt: "q".into(),
            blocks: Vec::new(),
            events: None,
            profile: RunProfile::Background,
            target: None,
            budget_usd: None,
        };
        let args = runner.command(&request, "tok", None).args();
        let joined = args.join(" ");
        assert!(joined.starts_with("exec -i -w /work -e CLAUDE_CODE_DISABLE_AUTO_MEMORY=1"));
        for must in [
            "--restricted",
            "--strict-mcp-config",
            "--permission-prompts none",
            "--plugin-dir /opt/lucia/plugin",
            "--append-system-prompt-file /opt/lucia/character/character.md",
            "--resume 00000000-0000-0000-0000-000000000000",
            "--model sonnet",
        ] {
            assert!(joined.contains(must), "{must} 없음: {joined}");
        }
        assert!(!joined.contains("dangerously"), "bypass는 쓰지 않는다");
        let i = args.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(
            args[i + 1],
            "WebSearch",
            "사람 없는 실행의 내장 도구는 웹 검색만"
        );
        assert!(
            joined.contains("-e LUCIA_BUILTIN_TOOLS=WebSearch ")
                && !joined.contains("CLAUDE_ENV_FILE")
                && !joined.contains("LUCIA_PROXY")
                && !joined.contains("LUCIA_RUN_TOKEN"),
            "훅에는 웹 검색만 알리고, 쉘 환경·통행증·승인 창구는 주지 않는다: {joined}"
        );
        assert_eq!(
            runner.command(&request, "tok", None).timeout,
            Duration::from_secs(30)
        );
        // 결정 0059: 사람이 대화 중인 실행에는 쉘과 파일 도구를 열고, 훅에 알리고, 쉘에만 통행증을 준다.
        // 시간 제한은 따로 길게 둔다.
        let mut chat_req = request.clone();
        chat_req.profile = RunProfile::Chat;
        let chat = runner.command(
            &chat_req,
            "tok",
            Some("http://lucia:pass@host.docker.internal:47812"),
        );
        assert_eq!(chat.timeout, Duration::from_secs(3600));
        let chat_args = chat.args();
        let chat_joined = chat_args.join(" ");
        let i = chat_args.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(chat_args[i + 1], "WebSearch,Bash,Read,Edit,Write,Glob,Grep");
        let i = chat_args
            .iter()
            .position(|a| a == "--allowedTools")
            .unwrap();
        for tool in [
            "Bash",
            "Read",
            "Write",
            "Edit",
            "mcp__lucia__approval_request",
        ] {
            assert!(chat_args[i + 1].split(',').any(|t| t == tool), "{tool}");
        }
        for must in [
            "-e LUCIA_BUILTIN_TOOLS=WebSearch,Bash,Read,Edit,Write,Glob,Grep ",
            "-e CLAUDE_ENV_FILE=/opt/lucia/plugin/env/bash-env.sh ",
            "-e LUCIA_APPROVAL_URL=http://host.docker.internal:47810/approval ",
            "-e LUCIA_RUN_TOKEN=tok ",
            "-e LUCIA_PROXY=http://lucia:pass@host.docker.internal:47812 ",
            "--restricted",
            "--permission-prompts none",
        ] {
            assert!(chat_joined.contains(must), "{must} 없음: {chat_joined}");
        }
        assert!(
            !chat_joined.contains("HTTPS_PROXY") && !chat_joined.contains("WebFetch"),
            "claude 자신의 프록시는 건드리지 않는다"
        );
        // 통행증이 없으면(끔, 엄격, 나가는 글 막기) 쉘은 열리되 바깥에 나가지 못한다.
        assert!(
            !runner
                .command(&chat_req, "tok", None)
                .args()
                .join(" ")
                .contains("LUCIA_PROXY")
        );
        // 내장 도구를 끄면 대화도 웹 검색만이다.
        let closed = ClaudeAgentRunner {
            native: None,
            ..runner_like(&runner)
        };
        let closed_args = closed.command(&chat_req, "tok", None).args();
        let i = closed_args.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(closed_args[i + 1], "WebSearch");
        assert!(
            kill_tree_script("abc").contains("[B]earer abc")
                && kill_tree_script("abc").contains("pgrep -P")
        );
        assert!(joined.contains("Bearer tok"));
        let i = args.iter().position(|a| a == "--allowedTools").unwrap();
        let allowed: Vec<&str> = args[i + 1].split(',').collect();
        assert!(allowed.contains(&"mcp__lucia__mail_read"));
        assert!(allowed.contains(&"WebSearch"));
        assert!(
            !allowed.contains(&"mcp__lucia__mail_send"),
            "백그라운드 실행은 발송을 요청할 수 없다"
        );
        assert!(
            allowed_tools(RunProfile::Chat, false).contains(&"mcp__lucia__mail_send".to_owned())
        );
        assert!(!allowed_tools(RunProfile::Chat, false).contains(&"WebSearch".to_owned()));
        assert!(
            allowed_tools(RunProfile::Chat, false).contains(&"mcp__lucia__browser_open".to_owned())
        );
        let background = allowed_tools(RunProfile::Background, true);
        assert!(
            background.contains(&"mcp__lucia__browser_open".to_owned())
                && !background
                    .iter()
                    .any(|t| t.contains("browser_click") || t.contains("browser_type")),
            "브리핑은 위임 사이트를 읽기만 한다(결정 0028)"
        );
        // 결정 0011: 채팅에서 등록한 후속 조치는 브라우저(로그인 요청 빼고), 브리핑에서 시작한 것은 없음.
        let followup = allowed_tools(RunProfile::Followup, true);
        assert!(followup.contains(&"mcp__lucia__browser_open".to_owned()));
        assert!(followup.contains(&"mcp__lucia__browser_click".to_owned()));
        assert!(
            followup.contains(&"mcp__lucia__mail_send".to_owned()),
            "결정 0013: 승인 카드를 거치는 메일 발송 요청은 쓴다"
        );
        // 후속 조치 취소는 줄이는 동작이라 열고, 서버가 자기 것만 받는다(결정 0028 I2).
        assert!(followup.contains(&"mcp__lucia__followup_cancel".to_owned()));
        // 로그인 요청은 허용 목록에 있고, 서버가 위임으로 로그인을 허용한 사이트만 받는다(결정 0028).
        assert!(followup.contains(&"mcp__lucia__browser_login".to_owned()));
        for absent in ["memory_delete", "site_grant_propose"] {
            assert!(
                !followup.contains(&format!("mcp__lucia__{absent}")),
                "후속 조치 실행에 {absent} 없음"
            );
        }
        assert!(followup.contains(&"mcp__lucia__followup_create".to_owned()));
        assert!(
            !allowed_tools(RunProfile::FollowupLimited, true)
                .iter()
                .any(|t| t.contains("browser_click") || t.contains("browser_type")),
            "브리핑에서 시작한 후속 조치는 위임 사이트를 읽기만 한다(결정 0028)"
        );
        let chat = allowed_tools(RunProfile::Chat, false);
        assert!(chat.contains(&"mcp__lucia__browser_login".to_owned()));
        assert!(chat.contains(&"mcp__lucia__followup_cancel".to_owned()));
        assert_eq!(
            allowed_tools(RunProfile::ReportOnly, true),
            ["mcp__lucia__report_result"],
            "보고를 다시 청할 때는 보고만 할 수 있다"
        );
        let grants = RunProfile::ReportOnly.grants();
        assert!(!grants.allows("memory_write") && !grants.allows("mail_send"));
        assert!(
            allowed_tools(RunProfile::Chat, false)
                .contains(&"mcp__lucia__memory_delete".to_owned())
        );
        assert!(
            !allowed_tools(RunProfile::Background, true)
                .contains(&"mcp__lucia__memory_delete".to_owned()),
            "외부 글을 읽는 백그라운드 실행은 기억을 지우지 못한다"
        );
        // 결정 0020: 개발 작업은 임시 샌드박스에서 코드 도구를 쓰고, 상주의 plugin·캐릭터는 없다. 비용 상한을 붙인다.
        let mut dev_req = request.clone();
        dev_req.profile = RunProfile::Dev;
        dev_req.target = Some(crate::agent::RunTarget {
            sandbox: "lucia-task-ab12cd34".into(),
            workdir: "/data/devwork/task-ab12cd34".into(),
        });
        dev_req.budget_usd = Some(1.25);
        let dev_args = runner.command(&dev_req, "tok", None).args();
        let dev_joined = dev_args.join(" ");
        assert!(dev_joined.starts_with("exec -i -w /data/devwork/task-ab12cd34"));
        assert!(dev_joined.contains("lucia-task-ab12cd34"));
        assert!(!dev_joined.contains("--plugin-dir") && !dev_joined.contains("character.md"));
        assert!(dev_joined.contains("--max-budget-usd 1.25"));
        let i = dev_args.iter().position(|a| a == "--tools").unwrap();
        assert!(dev_args[i + 1].contains("Bash") && dev_args[i + 1].contains("Edit"));
        let i = dev_args.iter().position(|a| a == "--allowedTools").unwrap();
        assert!(
            dev_args[i + 1].contains("mcp__lucia__dev_push") && dev_args[i + 1].contains("Bash")
        );
        assert!(
            !dev_args[i + 1].contains("browser_open") && !dev_args[i + 1].contains("mail_send")
        );
        // 상주 실행에는 코드 도구가 없다.
        assert!(!joined.contains("Bash"));
        let remember = allowed_tools(RunProfile::Remember, true);
        assert!(remember.contains(&"mcp__lucia__memory_delete".to_owned()));
        assert!(
            remember.iter().all(|t| t.contains("memory_")
                || t.ends_with("report_result")
                || t.ends_with("__tool_search")
                || t.ends_with("__tool_call")),
            "기억 도구와 찾기·부르기만: {remember:?}"
        );
        // 보고만 하는 실행에는 찾기·부르기도 없다(찾을 도구가 없다).
        assert!(
            !allowed_tools(RunProfile::ReportOnly, true)
                .iter()
                .any(|t| t.ends_with("__tool_call"))
        );
        for tool in &allowed {
            assert!(
                tool.starts_with("mcp__lucia__") || BUILTIN_TOOLS.contains(tool),
                "허용 목록 밖의 도구: {tool}"
            );
        }
    }

    #[test]
    fn detects_missing_session_shape() {
        let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":0,"session_id":"x","total_cost_usd":0}"#;
        let out = RunOutput {
            events: vec![parse_line(line).unwrap().unwrap()],
            parse_errors: vec![],
            exit_code: Some(1),
            stderr: "  ✓ sandbox lucia — started\nNo conversation found with session ID: x\n"
                .into(),
        };
        assert!(is_missing_session(&out));
        let other = RunOutput {
            stderr: "something else".into(),
            ..out
        };
        assert!(!is_missing_session(&other));
    }
}
