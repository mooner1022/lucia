//! 설정에서 실행 환경을 조립한다: 저장소, MCP 서버, 실제 실행기, 엔진.
//! `luciad ask`·`briefing`(개발용)과 이후 `luciad run`(데몬)이 같은 조립을 쓴다.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use lucia_channel::ChannelAdapter;
use lucia_core::{ButtonPress, ConversationId};
use lucia_precheck::google::assist::GoogleAssist;
use lucia_precheck::google::{CALENDAR_BASE, GMAIL_BASE};
use lucia_store::Store;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::approvals::{ApprovalDesk, run_desk};
use crate::config::Config;
use crate::context::Budget;
use crate::engine::{Access, Engine, EngineConfig};
use crate::google_auth::GoogleAuth;
use crate::mcp::{self, AssistAccess, McpHandle, MemoryAccess, RunTokens, ServeOptions};
use crate::runner::{ClaudeAgentRunner, ReportInbox};
use crate::scheduler::{Clock, SystemClock};

pub struct Runtime<C> {
    pub store: Arc<Mutex<Store>>,
    pub engine: Arc<Engine<ClaudeAgentRunner, C>>,
    pub mcp: McpHandle,
    pub clock: Arc<dyn Clock>,
    /// 브라우저(결정 0006·0008). 켜 두었고 승인 데스크가 있을 때만.
    pub browser: Option<Arc<crate::browser::sandbox::SbxBrowser>>,
    pub logins: Option<Arc<crate::browser::logins::SiteLogins>>,
    pub browser_tools: Option<Arc<crate::browser::tools::BrowserTools>>,
    /// 작업 줄과 진행 알림을 받는 쪽(결정 0014). 데몬이 가져가 돌린다.
    pub task_jobs: Option<mpsc::UnboundedReceiver<crate::tasks::TaskJob>>,
    pub task_notes: Option<mpsc::UnboundedReceiver<crate::tasks::TaskNote>>,
    /// 앱 이모지·스티커(결정 0045). 데몬이 접속 API와 관리 페이지에 잇는다.
    pub expressions: Arc<crate::expressions::Expressions>,
    /// Lucia의 프로필 이미지(결정 0060). 데몬이 접속 API에 잇는다.
    pub profile: Arc<crate::profile::Profile>,
    /// 프로필 그림을 정하고 Discord 봇 아바타에 적용하는 묶음(결정 0064). 데몬이 관리 페이지에 잇는다.
    pub profile_access: crate::profile::ProfileAccess,
    /// 주 모델(결정 0055). 데몬이 관리 페이지에 잇는다.
    pub models: crate::models::MainModel,
    /// 전용 앱의 기능을 부르는 도구가 접속 API를 찾는 자리(결정 0053). 데몬이 채운다.
    pub app: mcp::AppSlot,
}

pub fn open_store(cfg: &Config) -> anyhow::Result<Arc<Mutex<Store>>> {
    std::fs::create_dir_all(&cfg.data_dir)
        .with_context(|| format!("데이터 디렉터리: {}", cfg.data_dir.display()))?;
    let store = Store::open(cfg.db_path())
        .with_context(|| format!("저장소: {}", cfg.db_path().display()))?;
    Ok(Arc::new(Mutex::new(store)))
}

/// 메일·일정 도구와 승인 데스크에 필요한 것(결정 0003).
pub struct AssistSetup {
    pub google: Arc<GoogleAuth>,
    /// 개발용(`luciad ask --file`): 첨부로 로컬 파일 주소(file://)를 받는다.
    pub allow_local_files: bool,
    /// 채널 어댑터가 보내는 승인 버튼 누름.
    pub presses: mpsc::Receiver<ButtonPress>,
    /// 슬래시 명령(결정 0040). 데몬만 준다.
    pub commands: Option<mpsc::Receiver<lucia_channel::SlashCommand>>,
    pub cancel: CancellationToken,
    /// Discord 서버 도구와 그 승인 실행(결정 0024). 데몬만 준다.
    pub discord: Option<Arc<crate::discord_ctl::DiscordControl>>,
    /// 대화 입구(제안 카드의 [진행]이 사용자의 말로 넣는다, 결정 0035). 데몬만 준다.
    pub inbound: Option<mpsc::Sender<lucia_core::Inbound>>,
}

/// 실행 환경을 만든다. `notify`는 브리핑과 알림을 보낼 대화의 기본값(관리 페이지 설정이 우선).
/// `assist`가 있으면 메일·일정 도구를 열고 승인 데스크 작업을 띄운다.
pub async fn build<C: ChannelAdapter>(
    cfg: &Config,
    store: Arc<Mutex<Store>>,
    channel: Arc<C>,
    access: Access,
    notify: Option<ConversationId>,
    assist: Option<AssistSetup>,
) -> anyhow::Result<Runtime<C>> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let tz = cfg.tz()?;
    // 개발 작업의 임시 샌드박스(결정 0020).
    let dev = Arc::new(crate::dev::DevSandboxes {
        sbx: cfg.sandbox.sbx.clone(),
        data_dir: cfg.data_dir.clone(),
        mcp_port: cfg.mcp.bind.port(),
    });
    let files = {
        let http = reqwest::Client::builder()
            .user_agent("lucia/0.1")
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        let mut fs = crate::files::FileStore::new(cfg.data_dir.join("files"), http, store.clone());
        if assist.as_ref().is_some_and(|a| a.allow_local_files) {
            fs = fs.allow_local_files();
        }
        Arc::new(fs)
    };

    // 앱 이모지·스티커(결정 0045): 도구·엔진·접속 API·관리 페이지가 같은 서비스를 쓴다.
    let expressions =
        crate::expressions::Expressions::new(store.clone(), cfg.data_dir.join("expressions"));
    // Lucia의 프로필 이미지(결정 0060): 도구가 정하고, 접속 API가 앱에 내준다.
    let profile = crate::profile::Profile::new(store.clone(), cfg.data_dir.join("profile"));
    // 주 모델(결정 0055): 사용자가 고른 값이 있으면 그것을, 없으면 설정 파일 값을 쓴다.
    let models =
        crate::models::MainModel::new(cfg.models.chat.clone(), cfg.models.briefing.clone());
    models.load(&store.lock().expect("store lock"));

    // 전용 앱의 기능(결정 0053): 접속 API는 데몬이 띄우므로 자리만 만들어 둔다.
    let app_slot: mcp::AppSlot = Arc::default();
    // 이미지 생성(결정 0046): 키는 관리 페이지에서 넣은 자격 증명에서 그때그때 읽는다.
    let images = crate::imagegen::ImageGen::new(
        store.clone(),
        files.clone(),
        Arc::new(crate::credentials::Credentials::new(
            cfg.credentials_dir.clone(),
            crate::mask::global().clone(),
        )),
        reqwest::Client::builder()
            .user_agent("lucia/0.1")
            .connect_timeout(Duration::from_secs(15))
            .build()?,
        tz,
    );

    let mut browser = None;
    let mut logins = None;
    let mut browser_tools: Option<Arc<crate::browser::tools::BrowserTools>> = None;
    let mut held_posts = None;
    // 되돌릴 수 없는 바깥 동작의 승인(결정 0059). 승인 데스크가 없는 실행에서는 카드를 올리지 못해 늘 거절한다.
    let mut actions = None;
    let mut discord_access = None;
    // 서버 스티커(결정 0044)는 Discord 제어가 있을 때만.
    let setup_discord = assist.as_ref().and_then(|a| a.discord.clone());
    let assist_access = match assist {
        Some(setup) => {
            let (posts_tx, posts_rx) = mpsc::unbounded_channel();
            held_posts = Some(posts_tx.clone());
            let action_gate =
                crate::actions::ActionGate::new(store.clone(), Some(posts_tx.clone()));
            actions = Some(action_gate.clone());
            let client = |g: &Arc<GoogleAuth>| {
                GoogleAssist::new(g.http().clone(), g.clone(), GMAIL_BASE, CALENDAR_BASE)
            };
            // 브라우저(결정 0006·0008): 재로그인·동작 승인을 같은 데스크가 다룬다.
            if cfg.browser.enabled {
                use crate::browser::egress::{Egress, SETTING_BLOCKED};
                let blocked = store
                    .lock()
                    .expect("store lock")
                    .setting(SETTING_BLOCKED)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let egress = Arc::new(Egress::new(
                    Egress::parse_list(&blocked).unwrap_or_else(|e| {
                        tracing::warn!(err = %e, "저장된 차단 목록을 읽지 못해 빈 목록으로 시작한다");
                        Vec::new()
                    }),
                ));
                let proxy = crate::browser::proxy::Proxy::system(egress.clone());
                proxy
                    .clone()
                    .serve(cfg.browser.proxy_bind, setup.cancel.clone())
                    .await
                    .with_context(|| format!("브라우저 프록시 시작: {}", cfg.browser.proxy_bind))?;
                let sbx = crate::browser::sandbox::SbxBrowser::new(
                    crate::browser::sandbox::SbxConfig {
                        sbx: cfg.sandbox.sbx.clone(),
                        sandbox: cfg.browser.sandbox.clone(),
                        idle: Duration::from_secs(cfg.browser.idle_minutes * 60),
                        proxy: format!(
                            "socks5://{}:{}",
                            cfg.mcp.host_alias,
                            cfg.browser.proxy_bind.port()
                        ),
                    },
                    egress.clone(),
                );
                sbx.spawn_reaper(setup.cancel.clone());
                let sites = crate::browser::sites::select(&cfg.browser.sites)
                    .map_err(|e| anyhow::anyhow!("browser.sites: {e}"))?;
                let service = Arc::new(crate::browser::logins::SiteLogins::new(
                    sbx.clone(),
                    Arc::new(crate::credentials::Credentials::new(
                        cfg.credentials_dir.clone(),
                        crate::mask::global().clone(),
                    )),
                    store.clone(),
                    Arc::new(crate::browser::logins::ChannelNotifier {
                        channel: channel.clone(),
                        store: store.clone(),
                        fallback: notify.clone(),
                    }),
                    Some(posts_tx.clone()),
                    sites,
                    crate::browser::logins::LoginsConfig {
                        form_base: cfg.login_form_base(),
                        input_ttl: chrono::Duration::minutes(cfg.browser.login_input_minutes),
                        approval_ttl: chrono::Duration::minutes(30),
                    },
                ));
                browser_tools = Some(Arc::new(crate::browser::tools::BrowserTools::new(
                    service.clone(),
                    egress,
                    Some(proxy),
                    store.clone(),
                    Some(posts_tx.clone()),
                    crate::browser::tools::ToolsConfig {
                        tz,
                        ..crate::browser::tools::ToolsConfig::default()
                    },
                )));
                browser = Some(sbx);
                logins = Some(service);
            }
            let desk = Arc::new(
                ApprovalDesk::new(
                    store.clone(),
                    channel.clone(),
                    access.clone(),
                    tz,
                    notify.clone(),
                )
                .with_builtin_kinds(
                    client(&setup.google),
                    logins
                        .clone()
                        .map(|l| l as Arc<dyn crate::approvals::SiteLoginExec>),
                    browser_tools
                        .clone()
                        .map(|t| t as Arc<dyn crate::approvals::BrowserActionExec>),
                )
                .with_dev(Some(dev.clone()))
                .register(crate::actions::ActionKind { gate: action_gate })
                .with_models(models.clone())
                .with_calendar(Some(
                    Arc::new(client(&setup.google)) as Arc<dyn crate::calendar::CalendarExec>
                ))
                .with_suggestions(setup.inbound.clone().map(|inbound| {
                    Arc::new(crate::reports::InboundSuggestions {
                        inbound,
                        store: store.clone(),
                        notify: notify.clone(),
                    }) as Arc<dyn crate::reports::SuggestionExec>
                }))
                .with_discord(setup.discord.clone().map(|control| {
                    Arc::new(crate::discord_tools::DiscordExec { control })
                        as Arc<dyn crate::approvals::DiscordActionExec>
                })),
            );
            discord_access =
                setup
                    .discord
                    .clone()
                    .map(|control| crate::discord_tools::DiscordAccess {
                        control,
                        store: store.clone(),
                        posts: Some(posts_tx.clone()),
                        approval_ttl: chrono::Duration::hours(cfg.sandbox.approval_ttl_hours),
                        tz,
                    });
            tokio::spawn(run_desk(
                desk,
                posts_rx,
                setup.presses,
                setup.commands,
                setup.cancel,
            ));
            Some(AssistAccess {
                assist: Arc::new(client(&setup.google)),
                google: setup.google,
                store: store.clone(),
                posts: posts_tx,
                approval_ttl: chrono::Duration::hours(cfg.sandbox.approval_ttl_hours),
                tz,
            })
        }
        None => None,
    };

    // 작업(결정 0014): 작업 줄과 진행 알림 통로.
    let (task_jobs_tx, task_jobs_rx) = mpsc::unbounded_channel();
    let (task_notes_tx, task_notes_rx) = mpsc::unbounded_channel();
    let task_access = crate::tasks::TaskAccess {
        store: store.clone(),
        jobs: task_jobs_tx.clone(),
        notes: task_notes_tx,
        dev: Some(dev.clone()),
        posts: held_posts.clone(),
    };

    let actions = actions.unwrap_or_else(|| crate::actions::ActionGate::new(store.clone(), None));
    let tokens = RunTokens::default();
    // 공개 페이지 가져오기(감시·스크립트 fetch): 브라우저 프록시와 같은 검사(차단 목록, 검사한 주소로만 연결).
    // 갈 수 있는 곳(차단 목록): 브라우저가 켜져 있으면 그것과 같은 목록을 쓴다(관리 페이지에서 고치면 함께 바뀐다).
    let egress = match &browser_tools {
        Some(t) => t.egress().clone(),
        None => {
            let user = store
                .lock()
                .expect("store lock")
                .setting(crate::browser::egress::SETTING_BLOCKED)
                .ok()
                .flatten()
                .map(|v| crate::browser::egress::Egress::parse_list(&v).unwrap_or_default())
                .unwrap_or_default();
            Arc::new(crate::browser::egress::Egress::new(user))
        }
    };
    let fetch: Arc<dyn crate::watch::Fetch> = {
        let egress = egress.clone();
        let plain = Arc::new(crate::watch::SafeFetch {
            proxy: crate::browser::proxy::Proxy::system(egress.clone()),
            egress,
        });
        // 감시의 세 방식(결정 0036): 쿠키 없이, 로그인 세션을 빌려, 브라우저 감시 탭.
        Arc::new(crate::session::WatchReader::new(
            plain,
            logins.as_ref().map(|l| l.browser().clone()),
            logins.clone(),
            store.clone(),
        ))
    };
    // 웹에서 파일 받기(결정 0057): 위 가져오기와 같은 검사로 받아 파일 보관소에 둔다. 도구(`file_download`)와
    // 브라우저가 시작하려던 내려받기, 스크립트(`lucia.download`)가 함께 쓴다.
    let downloads = Arc::new(crate::downloads::Downloads::new(
        fetch.clone(),
        files.clone(),
        store.clone(),
    ));
    if let Some(tools) = &browser_tools {
        tools.set_downloads(downloads.clone());
        tools.set_actions(actions.clone());
    }
    // 스크립트 출구 프록시(결정 0058): 사람이 대화 중인 실행의 코드가 바깥에 닿는 길. 브라우저 출구와 같은 검사.
    // 포트를 잡지 못하면(같은 설정으로 다른 프로세스가 도는 중) 코드는 바깥에 나가지 못할 뿐 나머지는 그대로 돈다.
    let script_net = if cfg.scripts.enabled && cfg.scripts.net {
        let net = crate::script::net::ScriptNet::new(crate::browser::proxy::Proxy::system(
            egress.clone(),
        ));
        match net
            .clone()
            .serve(cfg.scripts.proxy_bind, CancellationToken::new())
            .await
        {
            Ok(_) => Some((
                net,
                format!("{}:{}", cfg.mcp.host_alias, cfg.scripts.proxy_bind.port()),
            )),
            Err(err) => {
                tracing::warn!(%err, bind = %cfg.scripts.proxy_bind, "스크립트 출구 프록시를 띄우지 못했다(코드는 바깥에 직접 나가지 못한다)");
                None
            }
        }
    } else {
        None
    };
    // 스크립트(결정 0025): 전용 샌드박스에서 돌고, 도구는 이 데몬의 MCP 서버를 실행별 토큰으로 다시 부른다.
    let scripts = {
        let runner = cfg.scripts.enabled.then(|| {
            let url = Arc::new(std::sync::OnceLock::new());
            let _ = url.set(format!("http://{}/mcp", cfg.mcp.bind));
            Arc::new(
                crate::script::ScriptRunner::new(
                    Arc::new(crate::script::SbxLauncher {
                        sbx: cfg.sandbox.sbx.clone(),
                        sandbox: cfg.scripts.sandbox.clone(),
                    }),
                    Arc::new(crate::script::McpBridge {
                        url,
                        tokens: tokens.clone(),
                    }),
                    Some(fetch.clone()),
                )
                .with_files(Some(files.clone()))
                .with_downloads(Some(downloads.clone()))
                .with_net(script_net.clone()),
            )
        });
        // 패키지 설치(결정 0047): 설치하는 동안만 그 샌드박스에 패키지 색인을 연다.
        let packages = cfg.scripts.enabled.then(|| {
            let packages = crate::packages::Packages::new(
                Arc::new(crate::packages::SbxSandbox {
                    sbx: cfg.sandbox.sbx.clone(),
                    name: cfg.scripts.sandbox.clone(),
                    // 출구 프록시로 가는 허용 규칙은 남기고, 없으면 건다(끄면 치운다).
                    keep: cfg
                        .scripts
                        .net
                        .then(|| format!("localhost:{}", cfg.scripts.proxy_bind.port())),
                }),
                store.clone(),
            );
            // 앞선 실행이 설치 중에 죽어 남긴 허용 규칙을 치운다.
            tokio::spawn({
                let packages = packages.clone();
                async move { packages.sweep().await }
            });
            packages
        });
        crate::script::tools::ScriptAccess {
            runner,
            packages,
            store: store.clone(),
            posts: held_posts.clone(),
            tz,
            chat_timeout: Duration::from_secs(cfg.scripts.chat_timeout_secs),
            routine_timeout: Duration::from_secs(cfg.scripts.routine_timeout_secs),
        }
    };
    let inbox = ReportInbox::default();
    let (reports_tx, reports_rx) = mpsc::unbounded_channel();
    inbox.spawn_collector(reports_rx);
    // 교차 정합성 검사의 일정 읽기(결정 0033): 비서 도구와 같은 Google 클라이언트.
    let calendar_read = assist_access
        .as_ref()
        .map(|a| a.assist.clone() as Arc<dyn crate::consistency::CalendarRead>);
    // 프로필 그림을 정하는 곳(도구와 관리 페이지가 같은 것을 쓴다, 결정 0060·0064).
    let profile_access = crate::profile::ProfileAccess {
        profile: profile.clone(),
        discord: setup_discord.clone().map(|control| {
            Arc::new(crate::profile::DiscordAvatar {
                control,
                files: files.clone(),
            }) as Arc<dyn crate::profile::AvatarHost>
        }),
        files: files.clone(),
    };
    let mcp = mcp::serve(
        ServeOptions {
            bind: cfg.mcp.bind,
            extra_allowed_hosts: vec![cfg.mcp.host_alias.clone()],
            tokens: tokens.clone(),
            approvals: None,
            approval_timeout: Duration::from_secs(30),
            memory: Some(MemoryAccess {
                store: store.clone(),
                tz,
                posts: held_posts.clone(),
            }),
            assist: assist_access,
            files: Some(files.clone()),
            browser: browser_tools.clone(),
            tasks: Some(task_access),
            skills: Some(crate::skills::SkillAccess {
                store: store.clone(),
                files: crate::skills::SkillFiles::new(&cfg.data_dir),
                posts: held_posts.clone(),
            }),
            discord: discord_access,
            scripts: Some(scripts.clone()),
            watch_fetch: Some(fetch.clone()),
            actions: Some(actions.clone()),
            profile: Some(profile_access.clone()),
            workspace: cfg
                .sandbox
                .native_tools
                .then(|| crate::workspace::Workspace::new(cfg.sandbox.workdir.clone())),
            expressions: Some(expressions.clone()),
            images: Some(images),
            app: Some(app_slot.clone()),
        },
        reports_tx,
    )
    .await
    .with_context(|| format!("MCP 서버 시작: {}", cfg.mcp.bind))?;

    let workdir = cfg
        .sandbox
        .workdir
        .to_str()
        .context("작업 공간 경로가 UTF-8이 아니다")?
        .to_owned();
    let runner = ClaudeAgentRunner {
        sbx: cfg.sandbox.sbx.clone(),
        sandbox: cfg.sandbox.name.clone(),
        workdir,
        plugin_dir: cfg.sandbox.plugin_dir.clone(),
        character_file: cfg.sandbox.character_file.clone(),
        mcp_url: mcp.url_for(&cfg.mcp.host_alias),
        tokens,
        inbox,
        timeout: Duration::from_secs(cfg.sandbox.timeout_secs),
        task_timeout: Duration::from_secs(cfg.sandbox.task_timeout_secs),
        web_search: cfg.sandbox.web_search,
        chat_timeout: Duration::from_secs(cfg.sandbox.chat_timeout_secs),
        // 결정 0059: 사람이 대화 중인 실행에 내장 도구(쉘·파일)를 열고, 쉘의 인터넷은 출구 프록시의 통행증으로.
        // Bash 환경 파일은 읽기 전용 plugin 마운트에 있다.
        native: cfg
            .sandbox
            .native_tools
            .then(|| crate::runner::NativeTools {
                env_file: cfg
                    .sandbox
                    .plugin_dir
                    .as_ref()
                    .map(|dir| format!("{}/env/bash-env.sh", dir.trim_end_matches('/'))),
                net: script_net.clone(),
                store: store.clone(),
            }),
    };
    // 상주 샌드박스가 출구 프록시 포트에 닿게 한다(내장 도구를 열고 프록시가 떠 있을 때만. 아니면 치운다).
    // 프록시는 통행증이 있어야 지나가므로, 통행증을 받지 못한 실행은 닿아도 나가지 못한다.
    if cfg.scripts.enabled && cfg.scripts.net {
        let (sbx, name, target, want) = (
            cfg.sandbox.sbx.clone(),
            cfg.sandbox.name.clone(),
            format!("localhost:{}", cfg.scripts.proxy_bind.port()),
            cfg.sandbox.native_tools && script_net.is_some(),
        );
        tokio::spawn(async move {
            crate::packages::ensure_allow_rule(&sbx, &name, &target, want).await;
        });
    }
    let held_posts_engine = held_posts;
    let engine_cfg = EngineConfig {
        timezone: tz,
        sandbox: Some(cfg.sandbox.name.clone()),
        chat_model: cfg.models.chat.clone(),
        briefing_model: cfg.models.briefing.clone(),
        retro_model: cfg.models.retro.clone(),
        classify_model: cfg.models.classify.clone(),
        native_tools: cfg
            .sandbox
            .native_tools
            .then(|| cfg.sandbox.workdir.display().to_string()),
        notify_conversation: notify,
        rotate_context_tokens: cfg.session.rotate_context_tokens,
        rotate_after: chrono::Duration::days(cfg.session.rotate_after_days),
        compaction: cfg
            .session
            .compaction
            .then(|| crate::engine::CompactionPolicy {
                at_tokens: cfg.session.compact_context_tokens,
                idle_after: chrono::Duration::minutes(cfg.session.idle_compact_minutes),
                idle_min_tokens: cfg.session.idle_compact_min_tokens,
            }),
        budget: Budget::default(),
    };
    let mut engine = Engine::new(
        engine_cfg,
        store.clone(),
        runner,
        channel,
        access,
        clock.clone(),
    )
    .with_files(files);
    if let Some(posts) = held_posts_engine {
        engine = engine.with_approvals(posts);
    }
    let engine = Arc::new(
        engine
            .with_calendar_read(calendar_read)
            .with_task_jobs(task_jobs_tx)
            .with_dev(dev)
            .with_scripts(Some(scripts))
            .with_watch_fetch(Some(fetch.clone()))
            .with_expressions(Some(expressions.clone()))
            .with_models(models.clone())
            // 서버 스티커(결정 0044): Discord가 있을 때만.
            .with_stickers(setup_discord.clone().map(|control| {
                crate::stickers::Stickers::new(Arc::new(crate::stickers::DiscordStickers(control)))
            })),
    );

    Ok(Runtime {
        store,
        engine,
        mcp,
        clock,
        browser,
        logins,
        browser_tools,
        task_jobs: Some(task_jobs_rx),
        task_notes: Some(task_notes_rx),
        expressions,
        profile,
        profile_access,
        models,
        app: app_slot,
    })
}
