//! `luciad run`: 데몬. 관리 페이지, Discord, 스케줄러, 작업자, 주기 점검을 띄운다.
//!
//! - Discord 토큰이 없어도 뜬다(관리 페이지에서 넣는다). 그동안 알림은 기록에만 남는다.
//! - systemd 아래에서는 준비되면 `READY=1`을 보내고, 스케줄러 루프가 watchdog 신호를 보낸다.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::admin::{self, AdminState};
use crate::app;
use crate::check;
use crate::config::Config;
use crate::credentials::Credentials;
use crate::discord_ctl::{DiscordControl, HubChannel};
use crate::engine::Access;
use crate::google_auth::GoogleAuth;
use crate::scheduler;
use crate::sources;
use crate::worker;

const HEALTH_INTERVAL: Duration = Duration::from_secs(600);

pub async fn run(cfg: Config) -> anyhow::Result<()> {
    let cfg = Arc::new(cfg);
    let creds = Arc::new(Credentials::new(
        cfg.credentials_dir.clone(),
        crate::mask::global().clone(),
    ));
    let store = app::open_store(&cfg)?;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("lucia/0.1")
        .build()?;
    let google = GoogleAuth::new(creds.clone(), http.clone());

    let cancel = CancellationToken::new();
    let (inbound_tx, inbound_rx) = mpsc::channel(64);
    let (presses_tx, presses_rx) = mpsc::channel(16);
    // 슬래시 명령(결정 0040): Discord 어댑터의 명령 분기만 보낸다. 승인 데스크가 받는다.
    let (commands_tx, commands_rx) = mpsc::channel(8);
    let discord = DiscordControl::new(
        store.clone(),
        creds.clone(),
        inbound_tx.clone(),
        presses_tx.clone(),
    )
    .with_commands(commands_tx.clone());
    // 접속 API(결정 0021): 자체 앱도 같은 받은 편지함과 버튼 통로로 들어온다. 엔진은 대화 ID로 가르는 채널을 받는다.
    let gateway = crate::gateway::Gateway::new(store.clone(), inbound_tx.clone(), presses_tx);
    // 앱 연동(결정 0043): 외부 노출, 파일, 명령, 푸시.
    gateway.set_public(cfg.public.proxies(), cfg.public.gateway_open());
    gateway.set_files(Arc::new(crate::files::FileStore::new(
        cfg.data_dir.join("files"),
        http.clone(),
        store.clone(),
    )));
    gateway.set_commands(commands_tx);
    let app_push =
        crate::apppush::AppPush::new(gateway.clone(), store.clone(), creds.clone(), http.clone());
    let (push_tx, push_rx) = mpsc::unbounded_channel();
    gateway.set_push(push_tx);
    if let Err(err) = app_push.reload().await {
        tracing::warn!(err, "앱 푸시 자격 증명을 읽지 못했다");
    }
    tokio::spawn(app_push.clone().run(push_rx, cancel.clone()));
    // 앱으로 보낸 프레임 상자 정리(받은 것은 7일, 받지 않은 것도 30일 뒤) [제안].
    tokio::spawn({
        let store = store.clone();
        let cancel = cancel.clone();
        async move {
            loop {
                let now = chrono::Utc::now();
                let _ = store.lock().expect("store lock").prune_outbox(
                    now - chrono::Duration::days(7),
                    now - chrono::Duration::days(30),
                );
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(3600)) => {}
                }
            }
        }
    });
    let channel = || {
        Arc::new(crate::gateway::RouterChannel::new(
            Arc::new(HubChannel::new(discord.clone())),
            gateway.clone(),
            store.clone(),
            None,
        ))
    };
    let hub = channel();
    let runtime = app::build(
        &cfg,
        store.clone(),
        hub,
        Access::Owner,
        None,
        Some(app::AssistSetup {
            google: google.clone(),
            allow_local_files: false,
            presses: presses_rx,
            commands: Some(commands_rx),
            cancel: cancel.clone(),
            discord: Some(discord.clone()),
            inbound: Some(inbound_tx.clone()),
        }),
    )
    .await?;
    let mut runtime = runtime;
    let engine = runtime.engine.clone();
    // 앱 이모지·스티커(결정 0045): 접속 API가 목록·그림을 주고, 바뀌면 연결된 앱에 알린다.
    gateway.set_expressions(runtime.expressions.clone());
    // Lucia의 프로필 이미지(결정 0060): 접속 API가 내주고, 바뀌면 연결된 앱에 알린다.
    gateway.set_profile(runtime.profile.clone());
    // 전용 앱의 기능을 부르는 도구가 접속 API를 찾게 한다(결정 0053).
    let _ = runtime.app.set(gateway.clone());
    tokio::spawn({
        let expressions = runtime.expressions.clone();
        let cancel = cancel.clone();
        async move {
            loop {
                expressions.prune(chrono::Utc::now());
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(6 * 3600)) => {}
                }
            }
        }
    });
    // 작업 줄(결정 0014): 채팅과 따로, 한 번에 하나씩. 진행 알림은 실행 중에도 바로 보낸다.
    if let Some(jobs) = runtime.task_jobs.take() {
        tokio::spawn(crate::engine::run_task_lane(
            engine.clone(),
            jobs,
            cancel.clone(),
        ));
    }
    if let Some(notes) = runtime.task_notes.take() {
        tokio::spawn(crate::tasks::run_notes(
            store.clone(),
            channel(),
            None,
            notes,
        ));
    }

    scheduler::sync_routines(
        &store.lock().expect("store lock"),
        &cfg.routines(),
        runtime.clock.now(),
    )?;

    let (jobs_tx, jobs_rx) = mpsc::channel(64);
    // 앱이 청하는 세션 정리(결정 0056): 앞선 실행 하나와 정리 하나가 각각 시간 제한을 다 써도 답이 들어오게 기다린다.
    gateway.set_jobs(
        jobs_tx.clone(),
        Duration::from_secs(cfg.sandbox.timeout_secs * 2 + 30),
    );
    let admin_state = AdminState::new(
        store.clone(),
        discord.clone(),
        google.clone(),
        creds.clone(),
        cfg.clone(),
        jobs_tx.clone(),
    );
    if let Some(logins) = &runtime.logins {
        admin_state.set_logins(logins.clone());
    }
    if let Some(tools) = &runtime.browser_tools {
        admin_state.set_browser_tools(tools.clone()); // 화면 넘겨받기를 앱에서도(결정 0063, 범위 `browser_handover`가 있는 앱만).
        gateway.set_handover(tools.clone());
    }
    // 화면 넘겨받기가 최대 시간을 넘으면 끝낸다(결정 0017).
    if let Some(tools) = runtime.browser_tools.clone() {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
                }
                tools.expire_handover().await;
            }
        });
    }
    admin_state.set_models(runtime.models.clone());
    // 소유자가 관리 페이지에서 프로필 그림을 정할 때도 Lucia의 도구와 같은 길로 앱·Discord에 닿는다(결정 0064).
    admin_state.set_profile(runtime.profile_access.clone());
    admin_state.set_gateway(gateway.clone());
    admin_state.set_app_push(app_push.clone());
    admin::serve(admin_state.clone(), cancel.clone()).await?;
    gateway.serve(&cfg.gateway.bind, cancel.clone()).await?;

    if let Err(err) = discord.start_from_credentials().await {
        engine.alert(format!("{err:#}")).await;
    }

    tokio::spawn(worker::forward_inbound(
        inbound_rx,
        jobs_tx.clone(),
        Some(engine.clone()),
    ));
    tokio::spawn(worker::run_scheduler(
        store.clone(),
        runtime.clock.clone(),
        jobs_tx,
        cancel.clone(),
    ));
    // 이벤트 입구(결정 0031): 증분 확인 간격마다, 그리고 기기 이벤트가 오면 작업자에게 알린다.
    let (pokes_tx, pokes_rx) = mpsc::unbounded_channel();
    tokio::spawn(crate::events::run_poller(
        store.clone(),
        pokes_tx.clone(),
        cancel.clone(),
    ));
    gateway.set_events(pokes_tx);
    // 감시(결정 0019·0036)는 작업자가 큐 밖 전용 타이머로 확인한다(엔진의 감시 가져오기).
    tokio::spawn(worker::run_worker(
        engine.clone(),
        {
            let google = google.clone();
            move || sources::current(&google)
        },
        crate::backup::SessionBackup::from_config(&cfg, store.clone()).map(Arc::new),
        jobs_rx,
        pokes_rx,
    ));

    // 주기 점검: 새로 실패한 항목만 알린다.
    tokio::spawn({
        let cfg = cfg.clone();
        let engine = engine.clone();
        let cancel = cancel.clone();
        let admin_state = admin_state.clone();
        async move {
            let mut failing: BTreeSet<&'static str> = BTreeSet::new();
            loop {
                let checks = {
                    let cfg = cfg.clone();
                    tokio::task::spawn_blocking(move || check::run_checks(&cfg))
                        .await
                        .unwrap_or_default()
                };
                let now: BTreeSet<&'static str> =
                    checks.iter().filter(|c| !c.ok).map(|c| c.name).collect();
                for c in checks.iter().filter(|c| !c.ok && !failing.contains(c.name)) {
                    engine
                        .alert(format!("점검 실패: {} — {}", c.name, c.detail))
                        .await;
                }
                for name in failing.difference(&now) {
                    tracing::info!(check = name, "점검 회복");
                }
                failing = now;
                admin_state.store_checks(checks).await;
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(HEALTH_INTERVAL) => {}
                }
            }
        }
    });

    let _ = sd_notify::notify(&[
        sd_notify::NotifyState::Ready,
        sd_notify::NotifyState::Status("실행 중"),
    ]);
    tracing::info!("Lucia 데몬 시작");

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    tracing::info!("멈춘다");
    let _ = sd_notify::notify(&[sd_notify::NotifyState::Stopping]);
    // 지금 끊기는 일의 흔적을 지우지 않게 먼저 알린다(결정 0050: 다시 뜬 뒤 복구가 본다).
    engine.set_stopping();
    cancel.cancel();
    runtime.mcp.cancel.cancel();
    // 브라우저를 정상 종료한다(쿠키를 디스크에 쓰게, 결정 0006).
    if let Some(browser) = &runtime.browser {
        let _ = tokio::time::timeout(Duration::from_secs(15), browser.shutdown()).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    Ok(())
}
