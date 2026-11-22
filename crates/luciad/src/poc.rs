//! PoC 경로: 같은 세션에 요청을 두 번 보내고, 두 번째 요청이 첫 요청의 맥락을 기억하는지와
//! 구조화된 결과 보고(`report_result`)가 오는지 확인한다(명세 §로드맵 1의 완료 기준).
//! 새 세션으로 같은 질문을 하는 대조 요청도 보낸다.
//!
//! - `--sandbox`가 있으면 `sbx exec`로 샌드박스 안 Claude Code를 실행하고(PoC P4·P5),
//!   VM에서 MCP 인증과 외부 접속 차단도 확인한다.
//! - 없으면 호스트의 `claude`로 돌린다. 그 결과는 "호스트 사전 확인"일 뿐 PoC 통과가 아니다.
//!
//! 실행 조합은 P2에서 정한 것을 쓴다: `--restricted`, 내장 도구 없음, MCP 도구만 허용,
//! 권한 프롬프트 없음(`--permission-prompts none`), auto-memory 끔.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use lucia_runner::{ClaudeCommand, RunOutput, SessionMode, sbx_exec_prefix};
use tokio::sync::mpsc;
use uuid::Uuid;

use std::sync::{Arc, Mutex};

use crate::mcp::{
    self, ApprovalDecision, ApprovalRequest, McpHandle, Report, ReportStatus, RunId, RunTokens,
    ServeOptions,
};

const MCP_SERVER_NAME: &str = "lucia";
const SANDBOX_HOST_ALIAS: &str = "host.docker.internal";

#[derive(Debug, clap::Args)]
pub struct PocArgs {
    /// claude가 실행될 작업 디렉터리. 레포 밖의 디렉터리를 쓴다.
    /// 샌드박스면 VM 안에서도 같은 경로로 마운트되어 있어야 한다.
    #[arg(long)]
    pub workdir: PathBuf,
    /// 이 샌드박스 안에서 실행한다(`sbx exec`).
    #[arg(long)]
    pub sandbox: Option<String>,
    /// sbx 실행 파일.
    #[arg(long, default_value = "sbx")]
    pub sbx: PathBuf,
    /// 호스트에서 직접 돌릴 때의 claude 실행 파일.
    #[arg(long, default_value = "claude")]
    pub claude: PathBuf,
    /// 모델(별칭 가능). 비우면 CLI 기본값.
    #[arg(long)]
    pub model: Option<String>,
    /// stream-json 원본을 남길 디렉터리.
    #[arg(long)]
    pub record_dir: Option<PathBuf>,
    /// MCP 서버 주소. 샌드박스에서 쓰려면 네트워크 정책에서 허용한 고정 포트를 쓴다.
    #[arg(long, default_value = "127.0.0.1:0")]
    pub mcp_bind: SocketAddr,
    /// claude가 MCP 서버를 부를 때 쓰는 호스트 이름.
    /// 기본값: 샌드박스면 host.docker.internal, 아니면 127.0.0.1.
    #[arg(long)]
    pub mcp_host: Option<String>,
    /// 요청 하나의 제한 시간(초).
    #[arg(long, default_value_t = 240)]
    pub timeout_secs: u64,
}

/// 실행 조합 중 실험마다 다른 부분.
#[derive(Debug, Clone)]
struct Profile {
    tools: Vec<String>,
    allowed_tools: Vec<String>,
    permission_prompts: &'static str,
    permission_prompt_tool: Option<String>,
    env: Vec<(String, String)>,
}

impl Profile {
    /// 결과 보고만 하는 실행: 내장 도구 없음, 권한 프롬프트 없음.
    fn report_only() -> Self {
        Self {
            tools: Vec::new(),
            allowed_tools: vec![tool_name("report_result")],
            permission_prompts: "none",
            permission_prompt_tool: None,
            env: vec![("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "1".into())],
        }
    }

    /// 권한 확인을 데몬 MCP(`request_approval`)에 맡기는 실행(PoC P6).
    fn with_approval_tool(tools: Vec<String>) -> Self {
        let mut env = Self::report_only().env;
        // 2분 넘게 걸린 MCP 호출은 백그라운드로 넘어가 -p에서 결과를 잃는다(문서). 승인 대기에는 끈다.
        env.push(("CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS".into(), "0".into()));
        Self {
            tools,
            allowed_tools: vec![tool_name("report_result")],
            permission_prompts: "host",
            permission_prompt_tool: Some(tool_name("request_approval")),
            env,
        }
    }
}

fn tool_name(tool: &str) -> String {
    format!("mcp__{MCP_SERVER_NAME}__{tool}")
}

struct Poc<'a> {
    args: &'a PocArgs,
    server: McpHandle,
    tokens: RunTokens,
    rx: mpsc::UnboundedReceiver<Report>,
    mcp_host: String,
    profile: Profile,
}

struct Outcome {
    out: RunOutput,
    reports: Vec<Report>,
    run: RunId,
}

impl Outcome {
    /// 보고가 정확히 하나이고, 그 보고가 이 실행의 토큰으로 왔는가.
    fn single_report_from_this_run(&self) -> Option<&Report> {
        match self.reports.as_slice() {
            [r] if r.run.as_ref() == Some(&self.run) => Some(r),
            _ => None,
        }
    }
}

impl Poc<'_> {
    fn command(&self, mode: SessionMode, mcp_config: String) -> anyhow::Result<ClaudeCommand> {
        let env = self.profile.env.clone();
        let mut cmd = match &self.args.sandbox {
            Some(sandbox) => {
                let workdir = self
                    .args
                    .workdir
                    .to_str()
                    .context("작업 디렉터리 경로가 UTF-8이 아니다")?;
                let mut cmd = ClaudeCommand::new(&self.args.sbx, &self.args.workdir, mode);
                cmd.prefix = sbx_exec_prefix(sandbox, Some(workdir), &env);
                cmd
            }
            None => {
                let mut cmd = ClaudeCommand::new(&self.args.claude, &self.args.workdir, mode);
                cmd.env = env;
                cmd
            }
        };
        cmd.model = self.args.model.clone();
        cmd.mcp_config = Some(mcp_config);
        cmd.restricted = true;
        cmd.tools = Some(self.profile.tools.clone());
        cmd.allowed_tools = self.profile.allowed_tools.clone();
        cmd.permission_prompts = Some(self.profile.permission_prompts.into());
        cmd.permission_prompt_tool = self.profile.permission_prompt_tool.clone();
        cmd.timeout = Duration::from_secs(self.args.timeout_secs);
        Ok(cmd)
    }

    /// 요청 하나: 토큰 발급 → 실행 → 토큰 회수 → 그동안 온 보고 수거.
    async fn request(
        &mut self,
        label: &str,
        mode: SessionMode,
        prompt: &str,
    ) -> anyhow::Result<Outcome> {
        let run = RunId(format!("{label}-{}", Uuid::new_v4().simple()));
        let token = self.tokens.issue(run.clone());
        let mcp_config = serde_json::json!({
            "mcpServers": { MCP_SERVER_NAME: {
                "type": "http",
                "url": self.server.url_for(&self.mcp_host),
                "headers": { "Authorization": format!("Bearer {token}") },
            }}
        })
        .to_string();

        let record = self
            .args
            .record_dir
            .as_ref()
            .map(|d| d.join(format!("{label}.jsonl")));
        let result = self
            .command(mode, mcp_config)?
            .run(prompt, record.as_deref())
            .await;
        self.tokens.revoke(&token);
        let out = result?;

        let mut reports = Vec::new();
        while let Ok(report) = self.rx.try_recv() {
            reports.push(report);
        }
        print_run(label, &out, &reports, &run);
        Ok(Outcome { out, reports, run })
    }
}

pub async fn run(args: PocArgs) -> anyhow::Result<()> {
    if args.sandbox.is_none() {
        std::fs::create_dir_all(&args.workdir)
            .with_context(|| format!("작업 디렉터리를 만들 수 없다: {}", args.workdir.display()))?;
    }
    if let Some(dir) = &args.record_dir {
        std::fs::create_dir_all(dir)?;
    }

    let in_sandbox = args.sandbox.is_some();
    let mcp_host = args.mcp_host.clone().unwrap_or_else(|| {
        if in_sandbox {
            SANDBOX_HOST_ALIAS
        } else {
            "127.0.0.1"
        }
        .to_owned()
    });

    let tokens = RunTokens::default();
    let (tx, rx) = mpsc::unbounded_channel::<Report>();
    let server = mcp::serve(
        ServeOptions {
            bind: args.mcp_bind,
            extra_allowed_hosts: vec![SANDBOX_HOST_ALIAS.to_owned()],
            tokens: tokens.clone(),
            approvals: None,
            approval_timeout: Duration::from_secs(1),
            memory: None,
            assist: None,
            files: None,
            browser: None,
            tasks: None,
            skills: None,
            discord: None,
            scripts: None,
            watch_fetch: None,
            actions: None,
            profile: None,
            workspace: None,
            expressions: None,
            notes: None,
            images: None,
            app: None,
        },
        tx,
    )
    .await?;

    let mut poc = Poc {
        args: &args,
        server,
        tokens,
        rx,
        mcp_host,
        profile: Profile::report_only(),
    };
    println!(
        "실행 위치: {}\nMCP 서버: {} (claude는 {}로 접근)",
        args.sandbox
            .as_deref()
            .map_or("호스트".to_owned(), |s| format!("샌드박스 {s}")),
        poc.server.addr,
        poc.server.url_for(&poc.mcp_host),
    );

    let session = Uuid::new_v4();
    let secret = format!("고래{}", &Uuid::new_v4().simple().to_string()[..4]);
    println!("세션: {session}\n기억할 단어: {secret}\n");

    let first = format!(
        "PoC 요청 1/2. 이번 대화에서 기억할 단어는 '{secret}'이다. 단어를 기억했다는 것을 \
         report_result 도구로 보고하라(status는 done, summary는 한 문장, 단어는 summary에 쓰지 말 것)."
    );
    let r1 = poc
        .request("request1", SessionMode::New(session), &first)
        .await?;

    let recall = "앞 요청에서 기억하라고 한 단어가 무엇이었는지 report_result 도구로 보고하라. \
                  알면 status는 done, summary에는 그 단어만 쓴다. 모르면 status는 needs_input, \
                  summary에는 '모름'이라고 쓴다.";
    let r2 = poc
        .request(
            "request2",
            SessionMode::Resume(session),
            &format!("PoC 요청 2/2. {recall}"),
        )
        .await?;

    // 대조군: 같은 작업 디렉터리에서 새 세션으로 같은 질문. 단어를 알면 안 된다.
    let r3 = poc
        .request(
            "control",
            SessionMode::New(Uuid::new_v4()),
            &format!("PoC 대조 요청. {recall}"),
        )
        .await?;

    let net = match &args.sandbox {
        Some(sandbox) => Some(network_checks(&poc, sandbox).await),
        None => None,
    };
    poc.server.cancel.cancel();

    let session_of = |o: &Outcome| o.out.result().map(|r| r.session_id.clone());
    let same_session = session_of(&r1) == Some(session.to_string())
        && session_of(&r2) == Some(session.to_string());
    let correlated = [&r1, &r2, &r3]
        .iter()
        .all(|o| o.single_report_from_this_run().is_some());
    let remembered = r2.single_report_from_this_run().is_some_and(|r| {
        r.result.status == ReportStatus::Done && r.result.summary.contains(&secret)
    });
    let control_clean = r3
        .single_report_from_this_run()
        .is_some_and(|r| !r.result.summary.contains(&secret));

    println!("판정");
    println!("  요청 1·2가 같은 세션: {}", yes_no(same_session));
    println!(
        "  요청마다 report_result 정확히 한 번, 그 실행의 토큰으로: {}",
        yes_no(correlated)
    );
    println!("  요청 2 보고에 요청 1의 단어: {}", yes_no(remembered));
    println!("  새 세션(대조)은 단어를 모름: {}", yes_no(control_clean));
    let net_ok = match &net {
        Some(checks) => {
            for (name, ok, detail) in checks {
                println!("  {name}: {} ({detail})", yes_no(*ok));
            }
            checks.iter().all(|(_, ok, _)| *ok)
        }
        None => true,
    };

    if !(same_session && correlated && remembered && control_clean && net_ok) {
        bail!("PoC 완료 기준을 만족하지 못했다");
    }
    Ok(())
}

#[derive(Debug, clap::Args)]
pub struct ApprovalArgs {
    #[command(flatten)]
    pub base: PocArgs,
    /// 승인 응답을 기다리는 시간(초). HTTP MCP 요청 한도(60초)보다 짧게.
    #[arg(long, default_value_t = 15)]
    pub approval_timeout_secs: u64,
    /// 돌릴 경우: allow, deny, timeout.
    #[arg(long, value_delimiter = ',', default_value = "allow,deny,timeout")]
    pub cases: Vec<String>,
}

const APPROVAL_MARKER: &str = "/tmp/lucia-p6-marker";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Allow,
    Deny,
    /// 답하지 않는다(시간 초과 거부를 기대).
    Silent,
}

type SeenApprovals = Arc<Mutex<Vec<(Option<RunId>, serde_json::Map<String, serde_json::Value>)>>>;

/// PoC P6: permission-prompt-tool → 데몬 MCP `request_approval` → 응답(허용·거부·무응답) 왕복.
pub async fn run_approval(args: ApprovalArgs) -> anyhow::Result<()> {
    let base = &args.base;
    let in_sandbox = base.sandbox.is_some();
    let mcp_host = base.mcp_host.clone().unwrap_or_else(|| {
        if in_sandbox {
            SANDBOX_HOST_ALIAS
        } else {
            "127.0.0.1"
        }
        .to_owned()
    });
    if let Some(dir) = &base.record_dir {
        std::fs::create_dir_all(dir)?;
    }
    let approval_timeout = Duration::from_secs(args.approval_timeout_secs);

    let tokens = RunTokens::default();
    let (tx, rx) = mpsc::unbounded_channel::<Report>();
    let (approvals_tx, mut approvals_rx) = mpsc::unbounded_channel::<ApprovalRequest>();
    let server = mcp::serve(
        ServeOptions {
            bind: base.mcp_bind,
            extra_allowed_hosts: vec![SANDBOX_HOST_ALIAS.to_owned()],
            tokens: tokens.clone(),
            approvals: Some(approvals_tx),
            approval_timeout,
            memory: None,
            assist: None,
            files: None,
            browser: None,
            tasks: None,
            skills: None,
            discord: None,
            scripts: None,
            watch_fetch: None,
            actions: None,
            profile: None,
            workspace: None,
            expressions: None,
            notes: None,
            images: None,
            app: None,
        },
        tx,
    )
    .await?;

    // 승인 응답 역할(3단계에서는 Discord 버튼이 맡는다).
    let answer = Arc::new(Mutex::new(Answer::Deny));
    let seen: SeenApprovals = Arc::default();
    let held = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn({
        let (answer, seen, held) = (answer.clone(), seen.clone(), held.clone());
        async move {
            while let Some(req) = approvals_rx.recv().await {
                seen.lock()
                    .unwrap()
                    .push((req.run.clone(), req.input.clone()));
                let current = *answer.lock().unwrap();
                match current {
                    Answer::Allow => {
                        let _ = req.respond.send(ApprovalDecision::Allow);
                    }
                    Answer::Deny => {
                        let _ = req
                            .respond
                            .send(ApprovalDecision::Deny("PoC: 사용자가 거부했다.".into()));
                    }
                    Answer::Silent => held.lock().unwrap().push(req.respond),
                }
            }
        }
    });

    let mut poc = Poc {
        args: base,
        server,
        tokens,
        rx,
        mcp_host,
        profile: Profile::with_approval_tool(vec!["Bash".into()]),
    };
    println!(
        "실행 위치: {} / 승인 대기 {}초\n",
        base.sandbox
            .as_deref()
            .map_or("호스트".to_owned(), |s| format!("샌드박스 {s}")),
        approval_timeout.as_secs()
    );

    let prompt = format!(
        "PoC P6. Bash 도구로 정확히 다음 명령 하나만 실행하라: touch {APPROVAL_MARKER} . \
         다른 명령은 실행하지 마라. 실행됐으면 report_result(status는 done)로, 권한이 거부됐으면 \
         report_result(status는 failed, summary에 거부 사유)로 보고하라."
    );

    let mut all_ok = true;
    for case in &args.cases {
        let (current, expect_marker) = match case.as_str() {
            "allow" => (Answer::Allow, true),
            "deny" => (Answer::Deny, false),
            "timeout" => (Answer::Silent, false),
            other => bail!("모르는 경우: {other}"),
        };
        *answer.lock().unwrap() = current;
        marker(base, MarkerOp::Remove).await;

        let started = std::time::Instant::now();
        let outcome = poc
            .request(
                &format!("approval-{case}"),
                SessionMode::New(Uuid::new_v4()),
                &prompt,
            )
            .await?;
        let elapsed = started.elapsed();
        let created = marker(base, MarkerOp::Exists).await;

        let asked: Vec<_> = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(run, _)| run.as_ref() == Some(&outcome.run))
            .map(|(_, input)| input.clone())
            .collect();
        for input in &asked {
            println!(
                "  승인 요청 필드: {:?}\n  승인 요청 내용: {}",
                input.keys().collect::<Vec<_>>(),
                serde_json::Value::Object(input.clone())
            );
        }
        let denials = outcome
            .out
            .result()
            .map_or(0, |r| r.permission_denials.len());
        let mut ok = !asked.is_empty() && created == expect_marker;
        if current == Answer::Silent {
            ok &= elapsed >= approval_timeout;
        }
        if !expect_marker {
            ok &= denials > 0;
        }
        all_ok &= ok;
        println!(
            "  판정[{case}]: {} (승인 요청 {}건, 표식 {}, 거부 {}건, {:.1}초)\n",
            yes_no(ok),
            asked.len(),
            if created { "생김" } else { "없음" },
            denials,
            elapsed.as_secs_f64()
        );
    }
    held.lock().unwrap().clear();
    poc.server.cancel.cancel();

    if !all_ok {
        bail!("P6 기대와 다른 경우가 있다");
    }
    Ok(())
}

enum MarkerOp {
    Remove,
    Exists,
}

/// 표식 파일 지우기/확인. 샌드박스면 VM 안, 아니면 호스트.
async fn marker(args: &PocArgs, op: MarkerOp) -> bool {
    match (&args.sandbox, op) {
        (Some(sandbox), MarkerOp::Remove) => {
            let _ = tokio::process::Command::new(&args.sbx)
                .args(["exec", sandbox, "rm", "-f", APPROVAL_MARKER])
                .output()
                .await;
            true
        }
        (Some(sandbox), MarkerOp::Exists) => tokio::process::Command::new(&args.sbx)
            .args(["exec", sandbox, "test", "-e", APPROVAL_MARKER])
            .status()
            .await
            .is_ok_and(|s| s.success()),
        (None, MarkerOp::Remove) => {
            let _ = std::fs::remove_file(APPROVAL_MARKER);
            true
        }
        (None, MarkerOp::Exists) => std::path::Path::new(APPROVAL_MARKER).exists(),
    }
}

/// VM 안에서 curl로: MCP 엔드포인트 인증과 허용 목록 밖 외부 접속 차단을 확인한다.
async fn network_checks(poc: &Poc<'_>, sandbox: &str) -> Vec<(&'static str, bool, String)> {
    let url = poc.server.url_for(&poc.mcp_host);
    let valid = poc.tokens.issue(RunId("network-check".into()));
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let mcp_status = |auth: Option<String>| {
        let mut args = vec![
            "-s".to_owned(),
            "-o".to_owned(),
            "/dev/null".to_owned(),
            "-w".to_owned(),
            "%{http_code}".to_owned(),
            "-X".to_owned(),
            "POST".to_owned(),
            "-H".to_owned(),
            "Content-Type: application/json".to_owned(),
            "-H".to_owned(),
            "Accept: application/json, text/event-stream".to_owned(),
            "--data".to_owned(),
            body.to_owned(),
        ];
        if let Some(auth) = auth {
            args.extend(["-H".to_owned(), format!("Authorization: {auth}")]);
        }
        args.push(url.clone());
        args
    };

    let no_token = vm_curl(poc, sandbox, mcp_status(None)).await;
    let bad_token = vm_curl(
        poc,
        sandbox,
        mcp_status(Some("Bearer not-a-real-token".into())),
    )
    .await;
    let good_token = vm_curl(poc, sandbox, mcp_status(Some(format!("Bearer {valid}")))).await;
    poc.tokens.revoke(&valid);
    let egress = vm_curl(
        poc,
        sandbox,
        vec![
            "-s".into(),
            "-o".into(),
            "/dev/null".into(),
            "-w".into(),
            "%{http_code}".into(),
            "--max-time".into(),
            "15".into(),
            "https://example.com/".into(),
        ],
    )
    .await;

    vec![
        ("VM→MCP 토큰 없음은 401", no_token == "401", no_token),
        ("VM→MCP 틀린 토큰은 401", bad_token == "401", bad_token),
        (
            "VM→MCP 올바른 토큰은 통과(401 아님)",
            !good_token.is_empty() && good_token != "401" && good_token != "000",
            good_token,
        ),
        (
            "VM→허용 목록 밖(example.com) 차단",
            egress != "200" && !egress.is_empty(),
            egress,
        ),
    ]
}

async fn vm_curl(poc: &Poc<'_>, sandbox: &str, curl_args: Vec<String>) -> String {
    let output = tokio::process::Command::new(&poc.args.sbx)
        .args(["exec", sandbox, "curl"])
        .args(&curl_args)
        .output()
        .await;
    match output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        Err(err) => format!("실행 실패: {err}"),
    }
}

fn print_run(label: &str, out: &RunOutput, reports: &[Report], run: &RunId) {
    println!("[{label}]");
    if let Some(init) = out.init() {
        println!(
            "  init: claude {} / 모델 {} / 권한 모드 {} / 도구 {:?} / MCP {} 연결 {}",
            init.claude_code_version.as_deref().unwrap_or("?"),
            init.model.as_deref().unwrap_or("?"),
            init.permission_mode.as_deref().unwrap_or("?"),
            init.tools,
            MCP_SERVER_NAME,
            yes_no(init.mcp_connected(MCP_SERVER_NAME)),
        );
    }
    println!("  도구 호출: {:?}", out.tool_uses());
    match out.result() {
        Some(r) => println!(
            "  result: {} / 오류 {} / 턴 {} / {} ms / 컨텍스트 {} / ${:.4} / 거부 {}건",
            r.subtype,
            r.is_error,
            r.num_turns,
            r.duration_ms,
            r.context_tokens(),
            r.total_cost_usd.unwrap_or(0.0),
            r.permission_denials.len(),
        ),
        None => println!("  result 이벤트 없음 (종료 코드 {:?})", out.exit_code),
    }
    if reports.is_empty() {
        println!("  보고 없음");
    }
    for r in reports {
        let from_this_run = r.run.as_ref() == Some(run);
        println!(
            "  보고: {:?} / {} (이 실행의 토큰: {})",
            r.result.status,
            r.result.summary,
            yes_no(from_this_run)
        );
    }
    if !out.parse_errors.is_empty() {
        println!(
            "  파싱 오류 {}건: {:?}",
            out.parse_errors.len(),
            out.parse_errors
        );
    }
    let stderr = out.stderr.trim();
    if !stderr.is_empty() {
        println!(
            "  stderr: {}",
            stderr.lines().take(5).collect::<Vec<_>>().join(" | ")
        );
    }
    println!();
}

fn yes_no(ok: bool) -> &'static str {
    if ok { "예" } else { "아니오" }
}
