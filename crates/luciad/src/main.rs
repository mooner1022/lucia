//! Lucia 호스트 데몬.
//!
//! 액터 구성(docs/PLAN.md §3.2): Scheduler, PrecheckRunner, TaskManager,
//! SessionWorker, McpServer, ChannelHub, HealthMonitor.
//! 액터는 tokio mpsc로 메시지를 주고받고, 공유 상태는 SQLite 하나에 모은다.

mod actions;
mod admin;
mod agent;
mod app;
mod apppush;
mod approvals;
mod attention;
mod autonomy;
mod backup;
mod board;
mod browser;
mod calendar;
mod character;
mod check;
mod config;
mod consistency;
mod context;
mod credentials;
mod daemon;
mod delegation;
mod dev;
mod discord_ctl;
mod discord_tools;
mod downloads;
mod engine;
mod events;
mod expressions;
mod files;
mod fileview;
mod gateway;
mod goals;
mod google_auth;
mod imagegen;
mod leak;
mod mask;
mod mcp;
mod models;
mod notes;
mod office;
mod packages;
mod poc;
mod profile;
mod public;
mod push;
mod reports;
mod risk;
mod routines;
mod runctx;
mod runner;
mod scheduler;
mod script;
mod security;
mod segments;
mod session;
mod signals;
mod skills;
mod sources;
mod stickers;
mod store_cli;
mod tasks;
mod tools;
mod toolsearch;
mod totp;
mod watch;
mod worker;
mod workspace;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use lucia_channel::memory::{MemoryChannel, PLATFORM};
use lucia_core::{Allowlist, ConversationId, Inbound, UserId};

#[derive(Debug, Parser)]
#[command(name = "luciad", version, about = "Lucia 호스트 데몬")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, clap::Args)]
struct ConfigArg {
    /// 설정 파일.
    #[arg(long, default_value = "~/.config/lucia/lucia.toml")]
    config: PathBuf,
}

impl ConfigArg {
    fn load(&self) -> anyhow::Result<config::Config> {
        let path = match self.config.strip_prefix("~") {
            Ok(rest) => std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(rest))
                .unwrap_or_else(|| self.config.clone()),
            Err(_) => self.config.clone(),
        };
        config::Config::load(&path)
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 데몬: 관리 페이지, Discord, 스케줄러, 작업자. systemd 아래에서도 이것을 띄운다.
    Run(ConfigArg),
    /// 관리 페이지 초기 비밀번호 설정 링크를 터미널에 찍는다(비밀번호가 없을 때만, 30분 유효).
    AdminSetup {
        #[command(flatten)]
        config: ConfigArg,
        /// 비밀번호를 지우고 새 설정 링크를 만든다(비밀번호를 잊었을 때).
        #[arg(long)]
        reset: bool,
    },
    /// 시작 시 점검: sbx 데몬 권한, 인증, 상주 샌드박스, MCP 포트, 저장소. 모델을 부르지 않는다.
    Check(ConfigArg),
    /// 개발용: 상주 세션에 채팅 한 번(실제 샌드박스, 모델 사용). 답을 출력한다.
    Ask {
        #[command(flatten)]
        config: ConfigArg,
        /// 첨부할 로컬 파일(여러 번 쓸 수 있다).
        #[arg(long = "file")]
        files: Vec<std::path::PathBuf>,
        /// 보낼 말.
        message: String,
    },
    /// 개발용: 아침 브리핑 한 번(바뀐 게 없으면 모델 없이 끝난다).
    Briefing(ConfigArg),
    /// 개발용: 내장 루틴 하나를 지금 돌린다(daily_wrapup, compact, periodic_check, retro, signals:sample, goal:<목표 ID>). 결과를 출력한다.
    Routine {
        #[command(flatten)]
        config: ConfigArg,
        /// 루틴 동작 이름.
        action: String,
    },
    /// 상주 세션 기록을 지금 VM 밖으로 백업한다(데몬은 매일 루틴으로 한다). 모델을 부르지 않는다.
    Backup(ConfigArg),
    /// 저장소(SQLite)의 일관된 사본을 새 파일로 만든다. 설정을 읽지 않고, 저장소는 읽기 전용으로만 열어
    /// 마이그레이션하지 않는다(데몬이 돌고 있어도 된다). 사본은 0600이고, 이미 있는 파일은 덮어쓰지 않으며,
    /// 무결성 검사를 통과하지 못하면 지우고 실패한다. 배포 스크립트가 새 바이너리로 바꾸기 전에 부른다.
    StoreBackup {
        /// 원본 저장소 파일(예: ~/.local/share/lucia/lucia.sqlite).
        #[arg(long)]
        db: PathBuf,
        /// 만들 사본 파일. 이미 있으면 실패한다.
        #[arg(long)]
        out: PathBuf,
    },
    /// 저장소(SQLite)의 배포 전 상태를 `running_tasks=<수>`, `inbox=<수>`, `run_in_flight=<0|1>` 세 줄로
    /// 찍는다(하는 중인 작업, 받은 편지함, 하는 중인 정기 작업). 설정을 읽지 않고 읽기 전용으로만 열어
    /// 마이그레이션하지 않는다(데몬이 돌고 있어도 된다). 파일이 없거나 읽을 수 없으면 실패한다.
    StoreStatus {
        /// 저장소 파일(예: ~/.local/share/lucia/lucia.sqlite).
        #[arg(long)]
        db: PathBuf,
    },
    /// 이 바이너리에 관리 화면(결정 0061)이 온전히 들어 있는지 본다. 배포 스크립트가 설치 전에 부른다.
    UiCheck,
    /// Lucia의 프로필 이미지(결정 0060)를 보거나 사용자가 직접 정한다(처음 값 넣기). 파일 없이 부르면 지금 것을
    /// 알려 주고, 파일을 주면 그 그림으로 정한다(규칙은 Lucia의 도구와 같다: 움직이지 않는 PNG·JPEG, 정사각형,
    /// 128~2048px, 3 MiB). 저장소와 프로필 디렉터리만 고친다: 도는 데몬은 다시 띄워야 알고, Discord 봇의
    /// 아바타는 건드리지 않는다.
    ProfileImage {
        #[command(flatten)]
        config: ConfigArg,
        /// 프로필로 정할 그림 파일.
        file: Option<PathBuf>,
        /// 정한 그림을 없앤다.
        #[arg(long)]
        clear: bool,
    },
    /// Discord 서버 점검(결정 0024): 저장된 봇 토큰으로 REST만 읽는다(게이트웨이에 붙지 않아 데몬과 함께 돌려도
    /// 된다). 봇이 들어간 서버, 관리자 권한, 서버마다 채널 수와 한 채널의 최근 메시지 중 내용이 보이는 수(내용
    /// intent 확인). 메시지 내용은 출력하지 않는다.
    DiscordCheck(ConfigArg),
    /// 스크립트 샌드박스 점검(결정 0025): 코드가 돌고 도구 다리가 오가는지, 바깥에 닿지 않는지, 시간을 넘긴
    /// 코드가 끝나는지. 모델을 부르지 않고 데몬에도 붙지 않는다(운영 데몬과 함께 돌려도 된다).
    ScriptCheck(ConfigArg),
    /// 개발용: 브라우저 샌드박스 점검(결정 0006). 허용 사이트 첫 페이지를 한 번 읽고, VM 안 시험용 로그인
    /// 페이지로 로그인 흐름을 확인한다. 실제 계정으로는 로그인하지 않는다. 모델을 부르지 않는다.
    BrowserCheck {
        #[command(flatten)]
        config: ConfigArg,
        /// 허용 사이트 읽기를 건너뛴다(사이트에 요청을 보내지 않는다).
        #[arg(long)]
        skip_site: bool,
    },
    /// PoC: 같은 세션에 두 번 요청하고 맥락 유지와 report_result 수신을 확인한다.
    Poc(poc::PocArgs),
    /// PoC P6: permission-prompt-tool → request_approval 왕복(허용·거부·무응답).
    PocApproval(poc::ApprovalArgs),
}

/// 개발용 명령의 채널: 메모리 어댑터, 허용 사용자는 "cli" 한 명.
const CLI_USER: &str = "cli";
const CLI_CONVERSATION: &str = "cli";

async fn dev_runtime(
    cfg: &config::Config,
) -> anyhow::Result<(
    app::Runtime<MemoryChannel>,
    lucia_channel::memory::MemoryHandle,
)> {
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    let (adapter, handle) = MemoryChannel::new(4000, tx);
    let allowlist = Allowlist::new().allow(PLATFORM, CLI_USER, UserId("me".into()));
    // 메일·일정 읽기도 시험할 수 있게 Google을 붙인다(메모리 채널은 버튼이 없어 발송 요청은 닫힌다).
    let creds = Arc::new(credentials::Credentials::new(
        cfg.credentials_dir.clone(),
        mask::global().clone(),
    ));
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("lucia/0.1")
        .build()?;
    let (_presses_tx, presses_rx) = tokio::sync::mpsc::channel(1);
    let runtime = app::build(
        cfg,
        app::open_store(cfg)?,
        Arc::new(adapter),
        engine::Access::Fixed(allowlist),
        Some(ConversationId(CLI_CONVERSATION.into())),
        Some(app::AssistSetup {
            google: google_auth::GoogleAuth::new(creds, http),
            allow_local_files: true,
            presses: presses_rx,
            commands: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            discord: None,
            inbound: None,
        }),
    )
    .await?;
    Ok((runtime, handle))
}

fn print_sent(handle: &lucia_channel::memory::MemoryHandle) {
    for (_, note) in handle.take_notes() {
        println!("  · {note}");
    }
    for (_, body) in handle.take_sent() {
        println!("{body}\n───");
    }
    for f in handle.take_files() {
        println!("[파일 메시지] {} · {}", f.body, f.names.join(", "));
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,rmcp=warn".into()),
        )
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(|| mask::MaskingWriter(std::io::stderr()))
        .init();

    // Discord(twilight)의 TLS 공급자.
    let _ = rustls::crypto::ring::default_provider().install_default();

    match Cli::parse().command {
        Command::Run(arg) => daemon::run(arg.load()?).await,
        Command::AdminSetup { config, reset } => {
            let cfg = config.load()?;
            let store = app::open_store(&cfg)?;
            let store = store.lock().expect("store");
            if reset {
                admin::reset_password(&store)?;
                println!("관리 비밀번호를 지웠습니다.");
            }
            let urls = admin::create_setup_link(&store, &cfg, reset)?;
            println!(
                "아래 링크 중 하나를 30분 안에 열어 관리 비밀번호를 정하세요(데몬이 실행 중이어야 합니다):"
            );
            for url in urls {
                println!("  {url}");
            }
            Ok(())
        }
        Command::Check(arg) => {
            let cfg = arg.load()?;
            let checks = check::run_checks(&cfg);
            for c in &checks {
                println!(
                    "{} {:<28} {}",
                    if c.ok { "✓" } else { "✗" },
                    c.name,
                    c.detail
                );
            }
            anyhow::ensure!(checks.iter().all(|c| c.ok), "점검 실패가 있다");
            Ok(())
        }
        Command::Ask {
            config,
            files,
            message,
        } => {
            let cfg = config.load()?;
            let (mut runtime, handle) = dev_runtime(&cfg).await?;
            let attachments = files
                .iter()
                .map(|path| {
                    let path = std::fs::canonicalize(path)?;
                    let size = std::fs::metadata(&path)?.len();
                    Ok(lucia_core::InboundAttachment {
                        url: url::Url::from_file_path(&path)
                            .map_err(|()| anyhow::anyhow!("파일 경로: {}", path.display()))?
                            .to_string(),
                        filename: path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        content_type: None,
                        size,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let outcome = runtime
                .engine
                .handle_inbound(Inbound {
                    platform: PLATFORM.into(),
                    sender: CLI_USER.into(),
                    conversation: ConversationId(CLI_CONVERSATION.into()),
                    body: message,
                    attachments,
                    received_at: chrono::Utc::now(),
                    reply_to: None,
                    venue: None,
                    context: None,
                })
                .await?;
            print_sent(&handle);
            // 모델이 작업을 맡겼으면(결정 0014) 데몬의 작업 줄처럼 여기서 이어 돌린다.
            if let Some(mut jobs) = runtime.task_jobs.take() {
                while let Ok(job) = jobs.try_recv() {
                    tracing::info!(id = %job.id, "작업 실행(개발용)");
                    if let Err(err) = runtime.engine.run_task(job).await {
                        tracing::error!(%err, "작업 실행 실패");
                    }
                    print_sent(&handle);
                }
            }
            // 데몬의 작업자처럼 대화 뒤 compact 조건을 확인한다(결정 0004).
            if let Some(reason) = runtime.engine.compaction_due() {
                let outcome = runtime.engine.compact_resident(reason).await?;
                tracing::info!(?reason, ?outcome, "상주 세션 compact");
                print_sent(&handle);
            }
            runtime.mcp.cancel.cancel();
            // 브라우저를 썼으면 정상 종료한다(쿠키를 디스크에 쓰게, 결정 0006).
            if let Some(browser) = &runtime.browser {
                browser.shutdown().await;
            }
            tracing::info!(?outcome, "채팅 끝");
            Ok(())
        }
        Command::Briefing(arg) => {
            let cfg = arg.load()?;
            let (runtime, handle) = dev_runtime(&cfg).await?;
            scheduler::sync_routines(
                &runtime.store.lock().expect("store"),
                &cfg.routines(),
                runtime.clock.now(),
            )?;
            let outcome = runtime.engine.run_briefing().await?;
            print_sent(&handle);
            runtime.mcp.cancel.cancel();
            // 브라우저를 썼으면 정상 종료한다(쿠키를 디스크에 쓰게, 결정 0006).
            if let Some(browser) = &runtime.browser {
                browser.shutdown().await;
            }
            tracing::info!(?outcome, "브리핑 끝");
            Ok(())
        }
        Command::Routine { config, action } => {
            let cfg = config.load()?;
            let (runtime, handle) = dev_runtime(&cfg).await?;
            match action.as_str() {
                worker::actions::DAILY_WRAPUP => {
                    let out = runtime.engine.daily_wrapup().await?;
                    println!("하루 정리: {out:?}");
                }
                // 상주 세션을 지금 정리한다(관리 화면의 "지금 정리", 앱의 session.compact와 같은 일).
                worker::actions::COMPACT => {
                    let out = runtime
                        .engine
                        .compact_resident(engine::CompactReason::Manual)
                        .await?;
                    println!("세션 정리: {out:?}");
                }
                // 작업 하나를 그대로 돌린다(결정 0018 측정: 상주 대화를 거치지 않고 같은 브리프로).
                a if a.starts_with("task:") => {
                    let id = a.trim_start_matches("task:").to_owned();
                    runtime
                        .engine
                        .run_task(crate::tasks::TaskJob { id, input: None })
                        .await?;
                }
                // 개발 작업(결정 0020): 승인된 것처럼 push(개발 채널에는 버튼이 없다), 끝난 작업 정리.
                a if a.starts_with("push:") => {
                    let id = a.trim_start_matches("push:").to_owned();
                    let dev = crate::dev::DevSandboxes {
                        sbx: cfg.sandbox.sbx.clone(),
                        data_dir: cfg.data_dir.clone(),
                        mcp_port: cfg.mcp.bind.port(),
                    };
                    println!("{}", dev.push(&id).map_err(|e| anyhow::anyhow!(e))?);
                }
                "dev_cleanup" => {
                    runtime.engine.cleanup_dev_tasks().await;
                    println!("정리함");
                }
                // 반복 루틴(결정 0025) 하나를 지금: 스크립트 → (깨우면) Lucia. 쌓인 소식은 출력만 한다.
                a if a.starts_with("routine:") => {
                    let id = a.trim_start_matches("routine:").to_owned();
                    let wake = runtime.engine.fire_routine(&id, None).await;
                    if let Some(w) = wake {
                        println!("깨움: {}", w.note);
                        println!("결과: {:?}", runtime.engine.wake_routine(w).await?);
                    }
                    let store = runtime.store.lock().expect("store lock");
                    for r in store.routine_runs(&id, 3)? {
                        println!(
                            "기록: {} {} {}",
                            r.at,
                            if r.ok { "성공" } else { "실패" },
                            r.text
                        );
                    }
                    for n in store.pending_notices()? {
                        println!("소식: {}", n.body);
                    }
                }
                // 회고(결정 0018): 사람이 완료로 확인한 작업을 회고한다.
                "retro" => {
                    let n = runtime.engine.run_retros().await?;
                    println!("회고: {n}개 작업");
                }
                // 목표 점검(결정 0032) 하나를 지금.
                a if a.starts_with("goal:") => {
                    let id = a.trim_start_matches("goal:").to_owned();
                    println!("점검: {:?}", runtime.engine.run_goal_review(&id).await?);
                    let store = runtime.store.lock().expect("store lock");
                    for l in store.goal_log(&id, 5)? {
                        println!("기록: {} [{}] {}", l.at, l.kind, l.text);
                    }
                    for c in store.goal_children(&id)? {
                        println!(
                            "하위: {} {} (점검이 만듦: {})",
                            c.kind, c.child, c.by_review
                        );
                    }
                }
                // 신호 처리(결정 0031): 시험용 신호 셋을 넣고 2단계(작은 모델)·3단계(Lucia)를 실제로 돌린다.
                "signals:sample" => {
                    {
                        let store = runtime.store.lock().expect("store lock");
                        let now = chrono::Utc::now();
                        let mail = |key: &str, from: &str, subject: &str, snippet: &str| {
                            lucia_store::NewSignal {
                                source: lucia_precheck::google::GMAIL_SOURCE.into(),
                                key: format!("sample:{key}:{}", now.timestamp()),
                                summary: format!("새 메일: {from} — {subject}"),
                                payload: serde_json::json!({
                                    "id": "sample-not-a-real-id", "from": from, "subject": subject,
                                    "snippet": snippet, "labels": ["INBOX"],
                                }),
                                interest: None,
                                verdict: None,
                            }
                        };
                        for s in [
                            mail(
                                "a",
                                "학과 사무실 <office@example.ac.kr>",
                                "[긴급] 내일 오전 9시 기말 발표 순서 변경",
                                "발표 순서가 바뀌었습니다. 첨부 파일을 확인하세요.",
                            ),
                            mail(
                                "b",
                                "Weekly Tech <news@example.com>",
                                "This week in tech",
                                "Top stories this week...",
                            ),
                            mail("c", "친구", "주말에 시간 돼?", "토요일에 밥 먹을래?"),
                        ] {
                            store.add_signal(&s, now)?;
                        }
                    }
                    let n = runtime.engine.classify_signals().await?;
                    {
                        let store = runtime.store.lock().expect("store lock");
                        for s in store.recent_signals(3)? {
                            println!("판정: {} → {:?} ({:?})", s.summary, s.verdict, s.reason);
                        }
                    }
                    println!("지금 {n}건");
                    println!("깨우기: {:?}", runtime.engine.wake_signals().await?);
                }
                worker::actions::PERIODIC_CHECK => {
                    let n = runtime.engine.periodic_check()?;
                    let sent = runtime.engine.flush_notices().await?;
                    println!("주기 점검: 소식 {n}건 쌓음, {sent}건 보냄");
                }
                other => anyhow::bail!("개발용으로 돌릴 수 없는 루틴: {other}"),
            }
            print_sent(&handle);
            runtime.mcp.cancel.cancel();
            Ok(())
        }
        Command::BrowserCheck { config, skip_site } => {
            let cfg = config.load()?;
            browser::devcheck::run(&cfg, skip_site).await
        }
        Command::DiscordCheck(arg) => {
            let cfg = arg.load()?;
            discord_tools::check(&cfg).await
        }
        Command::ScriptCheck(arg) => {
            let cfg = arg.load()?;
            script::check::run(&cfg).await
        }
        Command::ProfileImage {
            config,
            file,
            clear,
        } => {
            let cfg = config.load()?;
            let store = app::open_store(&cfg)?;
            let profile = profile::Profile::new(store.clone(), cfg.data_dir.join("profile"));
            let now = chrono::Utc::now();
            let show = |a: &profile::Avatar| {
                format!(
                    "{} · {} · {}×{} · {}바이트 · {} · {} 정함",
                    a.id,
                    a.mime,
                    a.width,
                    a.height,
                    a.size,
                    a.set_at.format("%Y-%m-%d %H:%M UTC"),
                    a.by
                )
            };
            let log = |text: &str| {
                let _ = store.lock().expect("store lock").add_activity(
                    "profile_image",
                    text,
                    None,
                    now,
                );
            };
            match (file, clear) {
                (Some(_), true) => anyhow::bail!("파일과 --clear를 함께 줄 수 없다"),
                (None, true) => {
                    let had = profile.clear().map_err(|e| anyhow::anyhow!(e))?;
                    if had {
                        log("프로필 그림 없앰(사용자, 명령)");
                    }
                    println!(
                        "{}",
                        if had {
                            "프로필 그림을 없앴다. 도는 데몬은 다시 띄워야 안다."
                        } else {
                            "정한 프로필 그림이 없었다."
                        }
                    );
                }
                (Some(path), false) => {
                    let bytes = std::fs::read(&path)
                        .with_context(|| format!("그림 읽기: {}", path.display()))?;
                    let avatar = profile
                        .set(&bytes, "user", now)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    log(&format!(
                        "프로필 그림 정함(사용자, 명령): {}×{}",
                        avatar.width, avatar.height
                    ));
                    println!("프로필 그림을 정했다: {}", show(&avatar));
                    println!(
                        "도는 데몬은 다시 띄워야 안다(systemctl --user restart luciad). Discord 봇의 아바타는 건드리지 않았다."
                    );
                }
                (None, false) => match profile.avatar() {
                    Some(a) => println!("지금 프로필 그림: {}", show(&a)),
                    None => println!("정한 프로필 그림이 없다."),
                },
            }
            Ok(())
        }
        Command::UiCheck => {
            if !admin::ui_built() {
                anyhow::bail!(
                    "관리 화면이 들어 있지 않다. deploy/build-ui.sh로 화면을 빌드한 뒤 다시 빌드한다"
                );
            }
            let missing = admin::ui_missing_assets();
            if !missing.is_empty() {
                anyhow::bail!(
                    "관리 화면의 틀이 가리키는 파일이 없다: {}",
                    missing.join(", ")
                );
            }
            println!("관리 화면: 들어 있다");
            Ok(())
        }
        Command::Backup(arg) => {
            let cfg = arg.load()?;
            let store = app::open_store(&cfg)?;
            let backup = backup::SessionBackup::from_config(&cfg, store)
                .ok_or_else(|| anyhow::anyhow!("설정에서 백업이 꺼져 있다([backup] enabled)"))?;
            let summary = backup.run(chrono::Utc::now()).await?;
            println!("세션 기록 백업: {summary}");
            Ok(())
        }
        Command::StoreBackup { db, out } => {
            println!("{}", store_cli::backup(&db, &out)?);
            Ok(())
        }
        Command::StoreStatus { db } => {
            println!("{}", store_cli::status(&db)?);
            Ok(())
        }
        Command::Poc(args) => poc::run(args).await,
        Command::PocApproval(args) => poc::run_approval(args).await,
    }
}
