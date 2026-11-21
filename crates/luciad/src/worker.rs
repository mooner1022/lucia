//! 작업 큐와 루프.
//!
//! - 상주 세션 하나에 작업자 하나: 채팅과 루틴을 큐에서 하나씩 처리한다. 같은 세션에 `claude -p`가
//!   동시에 붙지 않게 하기 위해서다(PLAN §2.4). 브리핑 도중에 온 채팅은 뒤에서 기다린다.
//! - 스케줄러 루프는 때가 된 루틴을 큐에 넣고, 다음 예정 시각(최대 1분)까지 잔다.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lucia_channel::ChannelAdapter;
use lucia_core::Inbound;
use lucia_precheck::Source;
use lucia_store::Store;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::agent::AgentRunner;
use crate::engine::Engine;
use crate::engine::routines::RoutineWake;
use crate::scheduler::{self, Clock, DueJob};

/// 유휴 compact를 확인하는 주기.
const IDLE_CHECK: Duration = Duration::from_secs(300);
/// 감시 타이머 주기(결정 0036). 감시마다 간격은 따로 본다.
const WATCH_TICK: Duration = Duration::from_secs(60);

// 채팅이 커도(답장 원문·장소, 결정 0024) 큐에 64개까지라 상자에 넣지 않는다.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Job {
    Chat(Inbound),
    Routine(DueJob),
    /// 앱이 청한 세션 정리(결정 0056): 차례가 되면 정리하고 결과를 돌려준다. 기다리는 쪽이 떠났으면 하지 않는다.
    /// 맥락이 `min_tokens`보다 작으면 하지 않는다(0이면 크기를 보지 않는다).
    Compact {
        min_tokens: u64,
        reply: tokio::sync::oneshot::Sender<CompactReply>,
    },
}

/// 청한 세션 정리의 결과(결정 0056).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactReply {
    /// 정리했다(앞뒤 맥락 토큰).
    Compacted { pre: u64, post: u64 },
    /// 마지막 정리 뒤에 실행이 없어 다시 하지 않았다(모델을 부르지 않았다).
    Unchanged { tokens: u64 },
    /// 맥락이 청한 쪽이 정한 크기보다 작아 하지 않았다(모델을 부르지 않았다).
    Small { tokens: u64 },
    /// 정리할(시작된) 상주 세션이 없다.
    NoSession,
    /// 정리하지 못해 세션을 닫았다. 다음 요청은 새 세션에서 시작한다.
    Reset(String),
    /// 정리를 돌리지 못했다(세션은 그대로다).
    Error(String),
}

/// 청한 세션 정리 하나(결정 0056). 관리 화면의 "지금 정리"와 같은 일이지만, 마지막 정리 뒤에 실행이 없으면
/// 모델을 부르지 않는다(끊겨서 다시 부른 요청이 정리를 한 번 더 돌리지 않게).
async fn compact_on_request<R, C>(engine: &Engine<R, C>, min_tokens: u64) -> CompactReply
where
    R: AgentRunner,
    C: ChannelAdapter,
{
    use crate::engine::CompactOutcome;
    match engine.resident_compact_state() {
        None => return CompactReply::NoSession,
        Some((tokens, 0)) => return CompactReply::Unchanged { tokens },
        Some((tokens, _)) if tokens < min_tokens => return CompactReply::Small { tokens },
        Some(_) => {}
    }
    let reason = crate::engine::CompactReason::Manual;
    match engine.compact_resident(reason).await {
        Ok(outcome) => {
            let (ok, text) = compact_text(&outcome, reason);
            engine.record_last_run(actions::COMPACT, ok, &text);
            match outcome {
                CompactOutcome::Compacted { pre, post } => CompactReply::Compacted { pre, post },
                CompactOutcome::NoSession => CompactReply::NoSession,
                CompactOutcome::Failed(why) => CompactReply::Reset(why),
            }
        }
        Err(err) => {
            let why = format!("{err:#}");
            engine.record_last_run(actions::COMPACT, false, &why);
            CompactReply::Error(why)
        }
    }
}

/// 루틴 동작 이름.
pub mod actions {
    /// 사전 체크 후 아침 브리핑.
    pub const MORNING_BRIEFING: &str = "morning_briefing";
    /// 사전 체크만(변화는 다음 브리핑에 쓴다).
    pub const PRECHECK: &str = "precheck";
    /// 상주 세션 기록을 VM 밖으로 복사한다(모델 없음).
    pub const SESSION_BACKUP: &str = "session_backup";
    /// 상주 세션을 지금 compact한다(관리 화면의 "지금 정리", 결정 0004).
    pub const COMPACT: &str = "compact";
    /// 관리 화면의 "기억해 줘"(결정 0007). 페이로드 `{"text": "..."}`.
    pub const REMEMBER: &str = "remember";
    /// 후속 조치 발사(결정 0010). 페이로드 `{"id": "followup-…"}`.
    pub const FOLLOWUP: &str = crate::autonomy::FOLLOWUP_ACTION;
    /// 하루 정리(결정 0016): 그날 대화를 기억에 반영하고, 정했으면 오래된 원본을 지운다.
    pub const DAILY_WRAPUP: &str = "daily_wrapup";
    /// 주기 점검(결정 0015, 모델 없음): 진행 상태를 훑어 소식을 쌓는다.
    pub const PERIODIC_CHECK: &str = "periodic_check";
}

/// 동작별 마지막 실행 결과(관리 화면의 "지금 실행" 옆에 보인다). 설정 `last_run.<동작>`에 JSON으로 둔다.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LastRun {
    pub at: chrono::DateTime<chrono::Utc>,
    pub ok: bool,
    pub text: String,
}

pub fn last_run_key(action: &str) -> String {
    format!("last_run.{action}")
}

fn compact_text(
    outcome: &crate::engine::CompactOutcome,
    reason: crate::engine::CompactReason,
) -> (bool, String) {
    use crate::engine::CompactOutcome;
    match outcome {
        CompactOutcome::Compacted { pre, post } => (
            true,
            format!(
                "{} 정리: {}→{} 토큰",
                reason.label(),
                crate::admin::num(*pre),
                crate::admin::num(*post)
            ),
        ),
        CompactOutcome::NoSession => (true, "정리할 세션이 없습니다".into()),
        CompactOutcome::Failed(why) => (false, format!("정리하지 못해 새 세션으로 넘어감: {why}")),
    }
}

/// 큐가 닫힐 때까지 작업을 하나씩 처리한다. `sources`는 루틴 때마다 불러 지금 쓸 소스를 만든다
/// (관리 페이지에서 Google을 연결·해제하면 다음 확인부터 반영된다). 세션 백업도 큐에서 하므로
/// 복사하는 동안 같은 세션에 `claude -p`가 돌지 않는다.
pub async fn run_worker<R, C, S, F>(
    engine: Arc<Engine<R, C>>,
    sources: F,
    backup: Option<Arc<crate::backup::SessionBackup>>,
    mut jobs: mpsc::Receiver<Job>,
    mut pokes: mpsc::UnboundedReceiver<crate::events::Poke>,
) where
    R: AgentRunner + 'static,
    C: ChannelAdapter + 'static,
    S: Source,
    F: Fn() -> Vec<S> + Send + 'static,
{
    // 유휴 compact 확인 주기(결정 0004). 작업 큐 안에서 하므로 채팅·루틴과 겹치지 않는다.
    let mut tick = tokio::time::interval(IDLE_CHECK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 반복 루틴(결정 0025): 스크립트는 큐 밖에서 돌고, Lucia를 깨울 일만 이 통로로 큐에 들어온다.
    let (wake_tx, mut wake_rx) = mpsc::unbounded_channel::<RoutineWake>();
    // 신호 2단계(결정 0031)가 큐 밖에서 끝나면 "지금" 것을 넘기러 큐로 돌아온다.
    let (signal_tx, mut signal_rx) = mpsc::unbounded_channel::<()>();
    // 감시(결정 0036): 큐 밖 전용 타이머. 새 변화가 있으면 큐로 알려 루틴·분류를 잇는다.
    let watcher = engine.has_watch_fetch().then(|| {
        let engine = engine.clone();
        let done = signal_tx.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(WATCH_TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut hosts = std::collections::HashMap::new();
            let mut alerted = false;
            loop {
                tick.tick().await;
                // 한 바퀴가 패닉해도 타이머는 살아 다음 바퀴를 돈다(처음 한 번만 알린다).
                let round = std::panic::AssertUnwindSafe(engine.run_watch_round(&mut hosts));
                match futures_util::FutureExt::catch_unwind(round).await {
                    Ok(n) if n > 0 => {
                        let _ = done.send(());
                    }
                    Ok(_) => {}
                    Err(_) => {
                        tracing::error!("감시 확인 중 패닉");
                        if !alerted {
                            alerted = true;
                            engine
                                .alert("페이지 감시 확인 중 오류가 났습니다(다음 바퀴부터 다시 확인합니다). 로그를 확인해 주세요.".into())
                                .await;
                        }
                    }
                }
            }
        })
    });
    // 시작할 때와 조용할 때마다: 발사 스케줄을 잃은 후속 조치·루틴을 다시 건다(결정 0010·0025).
    engine.rearm_followups().await;
    engine.rearm_routines();
    engine.rearm_goals();
    // 재시작 복구(결정 0050): 남은 실행을 끝내고, 끊긴 일을 알리고, 기다리던 메시지와 끊긴 정기 작업을 먼저 한다.
    let recovered = engine.recover().await;
    let mut retry_routine = recovered.routine.is_some();
    // 복구가 도는 동안 도착한 메시지는 받은 편지함과 큐 양쪽에 있다: 복구로 처리한 것은 큐에서 왔을 때 넘긴다.
    let mut recovered_keys: std::collections::HashSet<String> = recovered
        .queued
        .iter()
        .map(crate::engine::recovery::inbox_key)
        .collect();
    let mut pending: std::collections::VecDeque<Job> = recovered
        .routine
        .into_iter()
        .map(Job::Routine)
        .chain(recovered.queued.into_iter().map(Job::Chat))
        .collect();
    loop {
        let next = if let Some(job) = pending.pop_front() {
            Next::Job(job)
        } else {
            tokio::select! {
            job = jobs.recv() => match job {
                Some(Job::Chat(msg))
                    if recovered_keys.remove(&crate::engine::recovery::inbox_key(&msg)) =>
                {
                    continue;
                }
                Some(job) => Next::Job(job),
                None => break,
            },
            Some(w) = wake_rx.recv() => Next::Wake(w),
            Some(p) = pokes.recv() => Next::Poke(p),
            Some(()) = signal_rx.recv() => Next::Signals,
            _ = tick.tick() => {
                engine.rearm_followups().await;
                engine.rearm_routines();
                engine.rearm_goals();
                engine.expire_stale_tasks();
                if let Err(err) = engine.flush_notices().await {
                    tracing::warn!(%err, "소식 묶음을 보내지 못했다");
                }
                Next::Idle
            }
            }
        };
        let result = match next {
            Next::Job(Job::Chat(msg)) => {
                // 처리하는 동안 받은 편지함에 "하는 중"으로 둔다. 끝나면(답이나 실패 알림이 갔다) 지운다.
                engine.inbox_start(&msg);
                let done = engine.handle_inbound(msg.clone()).await.map(|_| ());
                // 멈추는 중에 끊긴 것이면 남겨 둔다: 다시 뜬 뒤 복구가 사용자에게 알린다.
                if !engine.stopping() {
                    engine.inbox_done(&msg);
                }
                done
            }
            Next::Job(Job::Compact { min_tokens, reply }) => {
                if reply.is_closed() {
                    // 기다리다 떠났으면 하지 않는다: 그 사이 모델을 바꿨다면 늦은 정리는 손해다.
                    tracing::info!("기다리는 쪽이 떠난 세션 정리 요청이라 건너뛴다");
                } else {
                    // 정기 작업처럼 하는 동안 적어 둔다(배포가 끝나기를 기다리고, 끊기면 다시 뜬 뒤 한 번 다시 한다).
                    let due = DueJob {
                        schedule_id: "app-compact".into(),
                        action: actions::COMPACT.into(),
                        payload: serde_json::Value::Null,
                        due_at: chrono::Utc::now(),
                    };
                    engine.mark_running(&due, std::mem::take(&mut retry_routine));
                    let done = compact_on_request(&engine, min_tokens).await;
                    if !engine.stopping() {
                        engine.clear_running();
                    }
                    let _ = reply.send(done);
                }
                Ok(())
            }
            Next::Job(Job::Routine(due)) if due.action == crate::routines::ACTION => {
                let id = due.payload["id"].as_str().unwrap_or("").to_owned();
                spawn_fire(&engine, &wake_tx, id, None);
                Ok(())
            }
            Next::Job(Job::Routine(due)) if due.action == crate::routines::WAKE_ACTION => {
                let w = RoutineWake {
                    id: due.payload["id"].as_str().unwrap_or("").to_owned(),
                    note: due.payload["note"].as_str().unwrap_or("").to_owned(),
                    data: due.payload["data"].clone(),
                };
                wake(&engine, w).await
            }
            Next::Job(Job::Routine(due)) => {
                // 하는 동안 적어 둔다(후속 조치는 스스로 다시 걸리므로 뺀다). 끊기면 다시 뜬 뒤 한 번 다시 한다.
                let tracked = due.action != actions::FOLLOWUP;
                if tracked {
                    engine.mark_running(&due, std::mem::take(&mut retry_routine));
                }
                let done = run_routine(
                    &engine,
                    &sources,
                    backup.as_deref(),
                    &due.action,
                    &due.payload,
                )
                .await;
                if tracked && !engine.stopping() {
                    engine.clear_running();
                }
                done
            }
            Next::Wake(w) => wake(&engine, w).await,
            Next::Poke(crate::events::Poke::Check) => {
                // 밀린 확인 요청은 한 번으로 합친다(이벤트는 그대로 받는다).
                while let Ok(p) = pokes.try_recv() {
                    if let crate::events::Poke::Event(ev) = p {
                        engine.record_event(&ev);
                    }
                }
                run_routine(
                    &engine,
                    &sources,
                    backup.as_deref(),
                    actions::PRECHECK,
                    &serde_json::Value::Null,
                )
                .await
            }
            Next::Poke(crate::events::Poke::Event(ev)) => {
                engine.record_event(&ev);
                Ok(())
            }
            // 신호 분류가 끝났거나 감시가 새 변화를 남겼다(뒤의 루틴·분류 잇기는 아래에서).
            Next::Signals => wake_signals(&engine).await,
            // 조용할 때: 일정 기반 방해 금지 구간을 읽고, 마감 독촉을 올리고, 미뤄 둔 "지금" 신호(방해 금지 끝)를 넘긴다.
            Next::Idle => {
                engine.refresh_busy().await;
                if let Err(err) = engine.escalate_nudges().await {
                    tracing::warn!(%err, "마감 독촉 실패");
                }
                wake_signals(&engine).await
            }
        };
        if let Err(err) = result {
            // 멈추는 중에 끊긴 실행의 오류는 알리지 않는다(다시 뜬 뒤 복구가 알린다).
            if engine.stopping() {
                tracing::info!(%err, "멈추는 중에 끊긴 작업");
            } else {
                engine.alert(format!("작업 처리 중 오류: {err:#}")).await;
            }
        }
        // 사전 체크가 찾은 변화로 돌 루틴(결정 0025).
        for (id, changes) in engine.take_change_fires() {
            spawn_fire(&engine, &wake_tx, id, Some(changes));
        }
        // 분류를 기다리는 신호가 있으면 큐 밖에서 분류한다(결정 0031).
        spawn_classify(&engine, &signal_tx);
        // 실행 뒤(크기)와 조용할 때(유휴) compact.
        if let Some(reason) = engine.compaction_due() {
            match engine.compact_resident(reason).await {
                Ok(outcome) => {
                    let (ok, text) = compact_text(&outcome, reason);
                    engine.record_last_run(actions::COMPACT, ok, &text);
                }
                Err(err) => engine.alert(format!("세션 정리 중 오류: {err:#}")).await,
            }
        }
    }
    if let Some(w) = watcher {
        w.abort();
    }
}

/// 작업 큐에서 꺼낸 것. 큐(64개)와 같은 까닭으로 상자에 넣지 않는다.
#[allow(clippy::large_enum_variant)]
enum Next {
    Job(Job),
    Wake(RoutineWake),
    Poke(crate::events::Poke),
    Signals,
    Idle,
}

/// 신호 2단계를 큐 밖에서 돌린다(한 번에 하나). "지금"이 있으면 큐로 알린다.
fn spawn_classify<R, C>(engine: &Arc<Engine<R, C>>, done: &mpsc::UnboundedSender<()>)
where
    R: AgentRunner + 'static,
    C: ChannelAdapter + 'static,
{
    if !engine.begin_classify() {
        return;
    }
    let engine = engine.clone();
    let done = done.clone();
    tokio::spawn(async move {
        let result = engine.classify_signals().await;
        engine.end_classify();
        match result {
            Ok(n) if n > 0 => {
                let _ = done.send(());
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(%err, "신호 분류 실패"),
        }
    });
}

/// 신호 3단계(큐 안).
async fn wake_signals<R: AgentRunner, C: ChannelAdapter>(
    engine: &Engine<R, C>,
) -> anyhow::Result<()> {
    let outcome = engine.wake_signals().await?;
    if outcome != crate::engine::signals::SignalWake::None {
        tracing::info!(?outcome, "신호로 Lucia를 깨움");
    }
    Ok(())
}

/// 반복 루틴 하나를 큐 밖에서 돌린다(스크립트가 오래 걸려도 채팅을 막지 않게). 깨울 일이면 큐로 돌려보낸다.
fn spawn_fire<R, C>(
    engine: &Arc<Engine<R, C>>,
    wakes: &mpsc::UnboundedSender<RoutineWake>,
    id: String,
    changes: Option<serde_json::Value>,
) where
    R: AgentRunner + 'static,
    C: ChannelAdapter + 'static,
{
    let engine = engine.clone();
    let wakes = wakes.clone();
    tokio::spawn(async move {
        if let Some(w) = engine.fire_routine(&id, changes).await {
            let _ = wakes.send(w);
        }
    });
}

/// 루틴이 Lucia를 깨운다(큐 안).
async fn wake<R: AgentRunner, C: ChannelAdapter>(
    engine: &Engine<R, C>,
    w: RoutineWake,
) -> anyhow::Result<()> {
    let id = w.id.clone();
    let outcome = engine.wake_routine(w).await;
    tracing::info!(id, ?outcome, "루틴이 Lucia를 깨움");
    outcome.map(|_| ())
}

/// 루틴 하나를 실행하고 마지막 결과를 남긴다.
async fn run_routine<R, C, S, F>(
    engine: &Engine<R, C>,
    sources: &F,
    backup: Option<&crate::backup::SessionBackup>,
    action: &str,
    payload: &serde_json::Value,
) -> anyhow::Result<()>
where
    R: AgentRunner,
    C: ChannelAdapter,
    S: Source,
    F: Fn() -> Vec<S>,
{
    let precheck = |result: &anyhow::Result<usize>| match result {
        Ok(n) => engine.record_last_run(actions::PRECHECK, true, &format!("새 변화 {n}건")),
        Err(err) => engine.record_last_run(actions::PRECHECK, false, &format!("{err:#}")),
    };
    match action {
        actions::MORNING_BRIEFING => {
            let checked = engine.run_prechecks(&mut sources()).await;
            precheck(&checked);
            checked?;
            let outcome = engine.run_briefing().await;
            let (ok, text) = match &outcome {
                Ok(crate::engine::BriefingOutcome::NoChange) => {
                    (true, "바뀐 것이 없어 모델 없이 끝남".to_owned())
                }
                Ok(crate::engine::BriefingOutcome::Sent) => (true, "브리핑을 보냄".to_owned()),
                Ok(crate::engine::BriefingOutcome::Failed(why)) => (false, why.clone()),
                Err(err) => (false, format!("{err:#}")),
            };
            engine.record_last_run(actions::MORNING_BRIEFING, ok, &text);
            outcome.map(|_| ())
        }
        actions::PRECHECK => {
            let checked = engine.run_prechecks(&mut sources()).await;
            precheck(&checked);
            checked.map(|_| ())
        }
        actions::SESSION_BACKUP => {
            // 보관 기간이 지난 첨부·생성 파일을 지운다(결정 0005). 백업이 꺼졌거나 실패해도 한다.
            if let Some(files) = engine.files() {
                let removed = files.prune(chrono::Utc::now());
                if removed > 0 {
                    tracing::info!(removed, "오래된 파일 정리");
                }
            }
            match backup {
                Some(b) => {
                    let result = b.run(chrono::Utc::now()).await;
                    match &result {
                        Ok(summary) => {
                            tracing::info!(%summary, "세션 기록 백업");
                            let short = summary.split(" → ").next().unwrap_or(summary);
                            engine.record_last_run(actions::SESSION_BACKUP, true, short);
                        }
                        Err(err) => engine.record_last_run(
                            actions::SESSION_BACKUP,
                            false,
                            &format!("{err:#}"),
                        ),
                    }
                    result
                        .map(|_| ())
                        .map_err(|e| e.context("세션 기록 백업 실패"))
                }
                None => {
                    engine.record_last_run(
                        actions::SESSION_BACKUP,
                        false,
                        "설정에서 백업이 꺼져 있습니다",
                    );
                    Ok(())
                }
            }
        }
        actions::COMPACT => {
            let reason = crate::engine::CompactReason::Manual;
            let outcome = engine.compact_resident(reason).await?;
            let (ok, text) = compact_text(&outcome, reason);
            engine.record_last_run(actions::COMPACT, ok, &text);
            Ok(())
        }
        actions::REMEMBER => {
            let text = payload["text"].as_str().unwrap_or("").trim();
            if text.is_empty() {
                engine.record_last_run(actions::REMEMBER, false, "빈 요청");
                return Ok(());
            }
            match engine.remember(text).await {
                Ok(Ok(summary)) => {
                    engine.record_last_run(actions::REMEMBER, true, &summary);
                    Ok(())
                }
                Ok(Err(why)) => {
                    engine.record_last_run(actions::REMEMBER, false, &why);
                    Ok(())
                }
                Err(err) => {
                    engine.record_last_run(actions::REMEMBER, false, &format!("{err:#}"));
                    Err(err)
                }
            }
        }
        crate::reports::WEEKLY_ACTION | crate::reports::EVENING_ACTION => {
            let result = if action == crate::reports::WEEKLY_ACTION {
                engine.run_weekly_review().await
            } else {
                engine.run_evening_preview().await
            };
            let (ok, text) = match &result {
                Ok(o) => (
                    !matches!(o, crate::engine::reports::ReportOutcome::Failed(_)),
                    format!("{o:?}"),
                ),
                Err(err) => (false, format!("{err:#}")),
            };
            engine.record_last_run(action, ok, &text);
            result.map(|_| ())
        }
        crate::consistency::ACTION => {
            let result = engine.run_consistency_check().await;
            let (ok, text) = match &result {
                Ok(o) => (
                    true,
                    format!(
                        "겹침·이동 {}건, 메일 후보 {}건{}",
                        o.conflicts,
                        o.candidates,
                        if o.sent.is_some() {
                            ", 알림 보냄"
                        } else {
                            ""
                        }
                    ),
                ),
                Err(err) => (false, format!("{err:#}")),
            };
            engine.record_last_run(crate::consistency::ACTION, ok, &text);
            result.map(|_| ())
        }
        crate::goals::REVIEW_ACTION => {
            let id = payload["id"].as_str().unwrap_or("");
            let outcome = engine.run_goal_review(id).await;
            tracing::info!(id, ?outcome, "목표 점검");
            outcome.map(|_| ())
        }
        actions::FOLLOWUP => {
            let id = payload["id"].as_str().unwrap_or("");
            let outcome = engine.run_followup(id).await;
            let (ok, text) = match &outcome {
                Ok(crate::engine::FollowupOutcome::Done) => (true, format!("{id} 함")),
                Ok(crate::engine::FollowupOutcome::Deferred(why)) => {
                    (true, format!("{id} 미룸: {why}"))
                }
                Ok(crate::engine::FollowupOutcome::Skipped) => (true, format!("{id} 이미 닫힘")),
                Ok(crate::engine::FollowupOutcome::Failed(why)) => (false, format!("{id}: {why}")),
                Err(err) => (false, format!("{id}: {err:#}")),
            };
            engine.record_last_run(actions::FOLLOWUP, ok, &text);
            if let Err(err) = &outcome {
                // 열린 채 스케줄 없이 남지 않게 실패로 닫고 알린다(다시 하면 모델을 또 부른다).
                engine.fail_followup(id, &format!("{err:#}")).await;
            }
            outcome.map(|_| ())
        }
        actions::DAILY_WRAPUP => {
            use crate::engine::wrapup::WrapupOutcome;
            let result = engine.daily_wrapup().await;
            let pruned = |n: usize| {
                if n > 0 {
                    format!(", 원본 {n}건 지움")
                } else {
                    String::new()
                }
            };
            match &result {
                Ok(WrapupOutcome::Nothing { deleted }) => engine.record_last_run(
                    actions::DAILY_WRAPUP,
                    true,
                    &format!("새 대화가 없어 모델을 부르지 않음{}", pruned(*deleted)),
                ),
                Ok(WrapupOutcome::Skipped(why)) => {
                    engine.record_last_run(actions::DAILY_WRAPUP, true, &format!("건너뜀: {why}"))
                }
                Ok(WrapupOutcome::Done { summary, deleted }) => engine.record_last_run(
                    actions::DAILY_WRAPUP,
                    true,
                    &format!("{summary}{}", pruned(*deleted)),
                ),
                Err(err) => {
                    engine.record_last_run(actions::DAILY_WRAPUP, false, &format!("{err:#}"))
                }
            }
            result.map(|_| ())
        }
        actions::PERIODIC_CHECK => {
            let result = engine.periodic_check();
            match &result {
                Ok(n) => engine.record_last_run(
                    actions::PERIODIC_CHECK,
                    true,
                    &if *n == 0 {
                        "훑을 것이 없음".to_owned()
                    } else {
                        format!("소식 {n}건을 모아 둠")
                    },
                ),
                Err(err) => {
                    engine.record_last_run(actions::PERIODIC_CHECK, false, &format!("{err:#}"))
                }
            }
            if let Err(err) = engine.flush_notices().await {
                tracing::warn!(%err, "소식 묶음을 보내지 못했다");
            }
            result.map(|_| ())
        }
        other => Err(anyhow::anyhow!("모르는 루틴 동작: {other}")),
    }
}

/// 들어오는 메시지를 작업 큐로 옮긴다.
pub async fn forward_inbound<R, C>(
    mut inbound: mpsc::Receiver<Inbound>,
    jobs: mpsc::Sender<Job>,
    engine: Option<Arc<Engine<R, C>>>,
) where
    R: AgentRunner + 'static,
    C: ChannelAdapter + 'static,
{
    while let Some(msg) = inbound.recv().await {
        // 실행 중에 온 "그만"은 큐 뒤에서 기다리면 소용이 없다: 여기서 바로 끝낸다(결정 0051).
        if let Some(engine) = &engine
            && engine.try_stop(&msg).await
        {
            continue;
        }
        // 큐는 메모리에만 있다: 처리가 끝날 때까지 받은 편지함에도 둔다(결정 0050).
        if let Some(engine) = &engine {
            engine.inbox_add(&msg);
        }
        if jobs.send(Job::Chat(msg)).await.is_err() {
            break;
        }
    }
}

/// 스케줄러 루프: 때가 된 루틴을 큐에 넣는다.
pub async fn run_scheduler(
    store: Arc<Mutex<Store>>,
    clock: Arc<dyn Clock>,
    jobs: mpsc::Sender<Job>,
    cancel: CancellationToken,
) {
    loop {
        // systemd watchdog: 스케줄러 루프가 돌고 있다는 신호. 루프가 멈추면 신호도 멈춰 systemd가 재시작한다.
        let _ = sd_notify::notify(&[sd_notify::NotifyState::Watchdog]);
        let now = clock.now();
        let (due, next) = {
            let mut store = store.lock().expect("store lock");
            let due = scheduler::tick(&mut store, now);
            let next = store.next_schedule_due().ok().flatten();
            (due, next)
        };
        match due {
            Ok(due) => {
                for job in due {
                    if jobs.send(Job::Routine(job)).await.is_err() {
                        return;
                    }
                }
            }
            Err(err) => tracing::error!(%err, "스케줄 판단 실패"),
        }
        let sleep = next
            .map(|at| (at - clock.now()).to_std().unwrap_or(Duration::ZERO))
            .unwrap_or(Duration::from_secs(60))
            .min(Duration::from_secs(60));
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(sleep.max(Duration::from_millis(200))) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use chrono::{TimeZone, Utc};
    use lucia_channel::memory::{MemoryChannel, PLATFORM};
    use lucia_core::{Allowlist, ConversationId, UserId};
    use lucia_precheck::fake::FakeSource;

    use super::*;
    use crate::agent::FakeRunner;
    use crate::engine::EngineConfig;
    use crate::scheduler::ManualClock;

    #[tokio::test]
    async fn one_run_at_a_time_and_chat_waits_behind_briefing() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let runner = FakeRunner::with_delay(Duration::from_millis(30));
        let (in_tx, _in_rx) = mpsc::channel(8);
        let (adapter, channel) = MemoryChannel::new(2000, in_tx);
        let clock = Arc::new(ManualClock::new(
            Utc.with_ymd_and_hms(2026, 11, 16, 22, 30, 0).unwrap(),
        ));
        let engine = Arc::new(Engine::new(
            EngineConfig::for_tests(),
            store.clone(),
            runner.clone(),
            Arc::new(adapter),
            crate::engine::Access::Fixed(Allowlist::new().allow(
                PLATFORM,
                "me",
                UserId("me".into()),
            )),
            clock,
        ));
        // 브리핑이 모델을 부르도록 사용자 말을 하나 남겨 둔다.
        store
            .lock()
            .unwrap()
            .add_episode(&lucia_store::NewEpisode::user(
                "dm",
                "내일 발표 준비",
                Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap(),
            ))
            .unwrap();

        let (jobs_tx, jobs_rx) = mpsc::channel(16);
        let (_pokes_tx, pokes_rx) = mpsc::unbounded_channel();
        let worker = tokio::spawn(run_worker(
            engine,
            || vec![FakeSource::new("calendar")],
            None,
            jobs_rx,
            pokes_rx,
        ));
        jobs_tx
            .send(Job::Routine(DueJob {
                schedule_id: "morning".into(),
                action: actions::MORNING_BRIEFING.into(),
                payload: serde_json::json!({}),
                due_at: Utc::now(),
            }))
            .await
            .unwrap();
        for text in ["첫 질문", "둘째 질문"] {
            jobs_tx
                .send(Job::Chat(Inbound {
                    platform: PLATFORM.into(),
                    sender: "me".into(),
                    conversation: ConversationId("dm".into()),
                    body: text.into(),
                    attachments: Vec::new(),
                    received_at: Utc::now(),
                    reply_to: None,
                    venue: None,
                }))
                .await
                .unwrap();
        }
        drop(jobs_tx);
        worker.await.unwrap();

        assert_eq!(
            runner.max_in_flight.load(Ordering::SeqCst),
            1,
            "같은 세션에 동시에 붙지 않는다"
        );
        let kinds: Vec<String> = runner
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| {
                r.prompt
                    .split("<request kind=\"")
                    .nth(1)
                    .unwrap()
                    .split('"')
                    .next()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["morning_briefing", "chat", "chat"],
            "들어온 순서대로"
        );
        assert_eq!(channel.take_sent().len(), 3);
    }

    /// 이벤트 입구(결정 0031)가 작업자를 거쳐 끝까지 간다: 확인 요청 → 사전 체크(1단계) → 큐 밖 분류(2단계)
    /// → 큐로 돌아와 알림(3단계).
    #[tokio::test]
    async fn a_check_poke_flows_through_classify_and_wake() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let now = Utc::now();
        // 기준선은 이미 잡혀 있다.
        store
            .lock()
            .unwrap()
            .commit_source_check(lucia_precheck::google::GMAIL_SOURCE, Some("h1"), &[], now)
            .unwrap();
        let runner = FakeRunner::default();
        runner.then(FakeRunner::reply("1 now 내일 마감 공지"));
        runner.then(FakeRunner::reply(
            "교수님 공지가 왔어요: 과제 마감이 내일로 당겨졌어요.",
        ));
        let (in_tx, _in_rx) = mpsc::channel(8);
        let (adapter, channel) = MemoryChannel::new(2000, in_tx);
        let engine = Arc::new(Engine::new(
            EngineConfig::for_tests(),
            store.clone(),
            runner.clone(),
            Arc::new(adapter),
            crate::engine::Access::Fixed(Allowlist::new()),
            Arc::new(ManualClock::new(now)),
        ));
        let change = lucia_precheck::Change {
            key: "msg:m1".into(),
            summary: "새 메일: 교수 — 과제 마감 변경".into(),
            payload: serde_json::json!({"id": "m1", "from": "교수 <p@example.ac.kr>", "subject": "과제 마감 변경", "labels": ["INBOX"]}),
        };
        let (jobs_tx, jobs_rx) = mpsc::channel(4);
        let (pokes_tx, pokes_rx) = mpsc::unbounded_channel();
        let worker = tokio::spawn(run_worker(
            engine,
            move || {
                vec![
                    FakeSource::new(lucia_precheck::google::GMAIL_SOURCE).then(Ok(
                        lucia_precheck::CheckOutput {
                            cursor: Some("h2".into()),
                            changes: vec![change.clone()],
                        },
                    )),
                ]
            },
            None,
            jobs_rx,
            pokes_rx,
        ));
        pokes_tx.send(crate::events::Poke::Check).unwrap();
        let mut sent = Vec::new();
        for _ in 0..200 {
            sent.extend(channel.take_sent());
            if !sent.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        drop(jobs_tx);
        drop(pokes_tx);
        worker.await.unwrap();
        assert!(
            sent.iter().any(|(_, b)| b.contains("과제 마감")),
            "알림이 가야 한다: {sent:?}"
        );
        let profiles: Vec<_> = runner
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.profile)
            .collect();
        assert_eq!(
            profiles,
            vec![
                crate::agent::RunProfile::ReportOnly,
                crate::agent::RunProfile::Background
            ]
        );
    }

    #[tokio::test]
    async fn scheduler_loop_enqueues_due_routines_and_inbound_is_forwarded() {
        let at = Utc.with_ymd_and_hms(2026, 11, 16, 22, 30, 0).unwrap(); // 서울 17일 07:30
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        scheduler::sync_routines(
            &store.lock().unwrap(),
            &[scheduler::Routine {
                id: "morning".into(),
                cron: "30 7 * * *".into(),
                timezone: "Asia/Seoul".into(),
                action: actions::MORNING_BRIEFING.into(),
                max_delay_secs: 3600,
            }],
            at - chrono::Duration::hours(1),
        )
        .unwrap();
        let clock = Arc::new(ManualClock::new(at));
        let (jobs_tx, mut jobs_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let looping = tokio::spawn(run_scheduler(store, clock, jobs_tx.clone(), cancel.clone()));

        match tokio::time::timeout(Duration::from_secs(2), jobs_rx.recv()).await {
            Ok(Some(Job::Routine(due))) => {
                assert_eq!(due.action, actions::MORNING_BRIEFING);
                assert_eq!(due.due_at, at);
            }
            other => panic!("루틴이 큐에 들어와야 한다: {other:?}"),
        }
        cancel.cancel();
        looping.await.unwrap();

        let (in_tx, in_rx) = mpsc::channel(4);
        let forward = tokio::spawn(forward_inbound::<FakeRunner, MemoryChannel>(
            in_rx, jobs_tx, None,
        ));
        in_tx
            .send(Inbound {
                platform: PLATFORM.into(),
                sender: "me".into(),
                conversation: ConversationId("dm".into()),
                body: "안녕".into(),
                attachments: Vec::new(),
                received_at: Utc::now(),
                reply_to: None,
                venue: None,
            })
            .await
            .unwrap();
        drop(in_tx);
        forward.await.unwrap();
        assert!(matches!(jobs_rx.recv().await, Some(Job::Chat(m)) if m.body == "안녕"));
    }

    #[tokio::test]
    async fn unknown_routine_action_is_alerted() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let (in_tx, _in_rx) = mpsc::channel(8);
        let (adapter, channel) = MemoryChannel::new(2000, in_tx);
        let engine = Arc::new(Engine::new(
            EngineConfig::for_tests(),
            store,
            FakeRunner::default(),
            Arc::new(adapter),
            crate::engine::Access::Fixed(Allowlist::new()),
            Arc::new(ManualClock::new(Utc::now())),
        ));
        let (jobs_tx, jobs_rx) = mpsc::channel(4);
        let (_pokes_tx, pokes_rx) = mpsc::unbounded_channel();
        let worker = tokio::spawn(run_worker(
            engine,
            Vec::<FakeSource>::new,
            None,
            jobs_rx,
            pokes_rx,
        ));
        jobs_tx
            .send(Job::Routine(DueJob {
                schedule_id: "x".into(),
                action: "launch_rockets".into(),
                payload: serde_json::json!({}),
                due_at: Utc::now(),
            }))
            .await
            .unwrap();
        drop(jobs_tx);
        worker.await.unwrap();
        let sent = channel.take_sent();
        assert!(sent[0].1.contains("모르는 루틴 동작"));
    }

    /// 결정 0051: 실행 중에 온 "그만"은 큐를 거치지 않는다. 받은 메시지의 길에서 바로 그 실행을 끝내고,
    /// 모델에게는 가지 않는다. 그 뒤의 보통 메시지는 차례대로 처리된다.
    /// 청한 세션 정리(결정 0056): 앞선 대화가 끝난 뒤에 정리하고 결과를 돌려준다. 다시 청하면 모델을 부르지
    /// 않는다. 기다리는 쪽이 떠난 요청은 하지 않는다.
    #[tokio::test]
    async fn requested_compaction_waits_its_turn_replies_and_is_not_repeated() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let (chan_tx, _chan_rx) = mpsc::channel(8);
        let (adapter, _channel) = MemoryChannel::new(2000, chan_tx);
        let runner = FakeRunner::with_delay(Duration::from_millis(30));
        let engine = Arc::new(Engine::new(
            EngineConfig::for_tests(),
            store.clone(),
            runner.clone(),
            Arc::new(adapter),
            crate::engine::Access::Fixed(Allowlist::new().allow(
                PLATFORM,
                "me",
                UserId("me".into()),
            )),
            Arc::new(ManualClock::new(Utc::now())),
        ));
        let (jobs_tx, jobs_rx) = mpsc::channel(8);
        let (_pokes_tx, pokes_rx) = mpsc::unbounded_channel();
        let worker = tokio::spawn(run_worker(
            engine.clone(),
            Vec::<FakeSource>::new,
            None,
            jobs_rx,
            pokes_rx,
        ));
        let say = |body: &str| {
            Job::Chat(Inbound {
                platform: PLATFORM.into(),
                sender: "me".into(),
                conversation: ConversationId("dm".into()),
                body: body.into(),
                attachments: Vec::new(),
                received_at: Utc::now(),
                reply_to: None,
                venue: None,
            })
        };
        let summary = "Summary:\n사용자는 라떼를 좋아한다.";
        let compact = |min_tokens, reply| Job::Compact { min_tokens, reply };

        // 대화가 먼저 큐에 있으면 그 뒤에 정리한다.
        runner.then(FakeRunner::reply("네."));
        runner.then(FakeRunner::compacted(6_000, 800, summary));
        jobs_tx.send(say("안녕")).await.unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        jobs_tx.send(compact(0, tx)).await.unwrap();
        assert_eq!(
            rx.await.unwrap(),
            CompactReply::Compacted {
                pre: 6_000,
                post: 800
            }
        );
        {
            let requests = runner.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert!(requests[0].prompt.contains("안녕"));
            assert!(requests[1].prompt.starts_with("/compact "));
        }
        let marker = || {
            store
                .lock()
                .unwrap()
                .setting(crate::engine::recovery::RUNNING_KEY)
                .unwrap()
        };
        assert_eq!(marker(), None, "끝나면 하는 중 표시를 지운다");

        // 다시 청하면(답을 못 받은 앱) 모델을 부르지 않는다.
        let (tx, rx) = tokio::sync::oneshot::channel();
        jobs_tx.send(compact(0, tx)).await.unwrap();
        assert_eq!(rx.await.unwrap(), CompactReply::Unchanged { tokens: 800 });
        assert_eq!(runner.calls(), 2);

        // 기다리는 쪽이 떠난 요청은 하지 않는다. 뒤의 요청이 정리한다.
        runner.then(FakeRunner::reply("네."));
        runner.then(FakeRunner::compacted(1_500, 700, summary));
        jobs_tx.send(say("하나 더")).await.unwrap();
        let (gone, gone_rx) = tokio::sync::oneshot::channel();
        drop(gone_rx);
        jobs_tx.send(compact(0, gone)).await.unwrap();
        // 맥락이 청한 크기보다 작으면 하지 않는다.
        let (tx, rx) = tokio::sync::oneshot::channel();
        jobs_tx.send(compact(1_000_000, tx)).await.unwrap();
        assert!(matches!(rx.await.unwrap(), CompactReply::Small { .. }));
        assert_eq!(runner.calls(), 3);
        let (tx, rx) = tokio::sync::oneshot::channel();
        jobs_tx.send(compact(1, tx)).await.unwrap();
        assert_eq!(
            rx.await.unwrap(),
            CompactReply::Compacted {
                pre: 1_500,
                post: 700
            }
        );
        assert_eq!(runner.calls(), 4, "떠난 요청은 모델을 부르지 않았다");
        assert_eq!(marker(), None);
        worker.abort();
    }

    /// 시작된 세션이 없으면 정리하지 않는다.
    #[tokio::test]
    async fn requested_compaction_without_a_session_does_nothing() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let (chan_tx, _chan_rx) = mpsc::channel(8);
        let (adapter, _channel) = MemoryChannel::new(2000, chan_tx);
        let runner = FakeRunner::with_delay(Duration::from_millis(1));
        let engine = Engine::new(
            EngineConfig::for_tests(),
            store,
            runner.clone(),
            Arc::new(adapter),
            crate::engine::Access::Fixed(Allowlist::new()),
            Arc::new(ManualClock::new(Utc::now())),
        );
        assert_eq!(
            compact_on_request(&engine, 0).await,
            CompactReply::NoSession
        );
        assert_eq!(runner.calls(), 0);
    }

    #[tokio::test]
    async fn stop_message_bypasses_the_queue() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let (chan_tx, _chan_rx) = mpsc::channel(8);
        let (adapter, channel) = MemoryChannel::new(2000, chan_tx);
        let runner = FakeRunner::with_delay(Duration::from_secs(30));
        let engine = Arc::new(Engine::new(
            EngineConfig::for_tests(),
            store.clone(),
            runner.clone(),
            Arc::new(adapter),
            crate::engine::Access::Fixed(Allowlist::new().allow(
                PLATFORM,
                "me",
                lucia_core::UserId("me".into()),
            )),
            Arc::new(ManualClock::new(Utc::now())),
        ));
        let (in_tx, in_rx) = mpsc::channel(8);
        let (jobs_tx, jobs_rx) = mpsc::channel(8);
        let (_pokes_tx, pokes_rx) = mpsc::unbounded_channel();
        let forward = tokio::spawn(forward_inbound(in_rx, jobs_tx, Some(engine.clone())));
        let worker = tokio::spawn(run_worker(
            engine.clone(),
            Vec::<FakeSource>::new,
            None,
            jobs_rx,
            pokes_rx,
        ));
        let say = |body: &str| Inbound {
            platform: PLATFORM.into(),
            sender: "me".into(),
            conversation: ConversationId("dm".into()),
            body: body.into(),
            attachments: Vec::new(),
            received_at: Utc::now(),
            reply_to: None,
            venue: None,
        };
        in_tx.send(say("이 열 곳을 비교해 줘")).await.unwrap();
        for _ in 0..400 {
            if !runner.requests.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let started = std::time::Instant::now();
        in_tx.send(say("그만")).await.unwrap();
        let mut sent: Vec<String> = Vec::new();
        for _ in 0..600 {
            sent.extend(channel.take_sent().into_iter().map(|(_, b)| b));
            if sent.iter().any(|b| b.contains("하던 일을 멈췄습니다")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            sent.iter().any(|b| b.contains("하던 일을 멈췄습니다")),
            "{sent:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "30초를 기다리지 않는다"
        );
        // "그만"은 모델에게 가지 않았고 받은 편지함에도 남지 않았다.
        let kinds: Vec<String> = runner
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| {
                let p = &r.prompt;
                let i = p.find("<request").unwrap_or(0);
                format!(
                    "{:?} {}",
                    r.profile,
                    p[i..].chars().take(60).collect::<String>()
                )
            })
            .collect();
        assert_eq!(kinds.len(), 1, "{kinds:#?}");
        assert!(store.lock().unwrap().inbox_items().unwrap().is_empty());
        drop(in_tx);
        forward.await.unwrap();
        worker.await.unwrap();
    }

    /// 결정 0050: 데몬이 다시 뜨면 기다리던 메시지는 이어서 처리하고, 처리하다 끊긴 것은 다시 하지 않고 알린다.
    /// 끊긴 정기 작업은 한 번만 다시 한다.
    #[tokio::test]
    async fn restart_recovers_queued_messages_and_reports_interrupted_work() {
        use crate::engine::recovery::RUNNING_KEY;
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let now = Utc::now();
        let build = |store: Arc<Mutex<Store>>| {
            let (in_tx, _in_rx) = mpsc::channel(8);
            let (adapter, channel) = MemoryChannel::new(2000, in_tx);
            let runner = FakeRunner::default();
            let engine = Arc::new(Engine::new(
                EngineConfig::for_tests(),
                store,
                runner.clone(),
                Arc::new(adapter),
                crate::engine::Access::Fixed(Allowlist::new().allow(
                    PLATFORM,
                    "me",
                    lucia_core::UserId("me".into()),
                )),
                Arc::new(ManualClock::new(now)),
            ));
            (engine, runner, channel)
        };
        let message = |body: &str, secs: i64| Inbound {
            platform: PLATFORM.into(),
            sender: "me".into(),
            conversation: ConversationId("dm".into()),
            body: body.into(),
            attachments: Vec::new(),
            received_at: now + chrono::Duration::seconds(secs),
            reply_to: None,
            venue: None,
        };
        // 앞선 데몬: 하나는 처리하다 끊겼고, 하나는 큐에서 기다리고 있었고, 정기 작업 하나를 하던 중이었다.
        {
            let (old, _, _) = build(store.clone());
            let interrupted = message("이건 하다가 끊김", 0);
            old.inbox_add(&interrupted);
            old.inbox_start(&interrupted);
            old.inbox_add(&message("이건 기다리던 것", 1));
            old.mark_running(
                &DueJob {
                    schedule_id: "x".into(),
                    action: "launch_rockets".into(),
                    payload: serde_json::json!({}),
                    due_at: now,
                },
                false,
            );
        }
        // 다시 뜬 데몬.
        let (engine, runner, channel) = build(store.clone());
        let (jobs_tx, jobs_rx) = mpsc::channel(4);
        let (_pokes_tx, pokes_rx) = mpsc::unbounded_channel();
        let worker = tokio::spawn(run_worker(
            engine.clone(),
            Vec::<FakeSource>::new,
            None,
            jobs_rx,
            pokes_rx,
        ));
        drop(jobs_tx);
        worker.await.unwrap();
        let sent: Vec<String> = channel.take_sent().into_iter().map(|(_, b)| b).collect();
        // 끊긴 답은 알리기만 한다. 기다리던 메시지는 처리한다. 끊긴 정기 작업은 다시 했다(모르는 동작이라 오류 알림).
        assert!(
            sent.iter()
                .any(|b| b.contains("다시 시작되면서 하던 답이 끊겼습니다")),
            "{sent:?}"
        );
        assert!(
            sent.iter().any(|b| b.contains("모르는 루틴 동작")),
            "{sent:?}"
        );
        let prompts: Vec<String> = runner
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.prompt.clone())
            .collect();
        assert_eq!(prompts.len(), 1, "끊긴 것은 다시 돌리지 않는다");
        assert!(prompts[0].contains("이건 기다리던 것"));
        // 흔적은 모두 치워졌다.
        {
            let s = store.lock().unwrap();
            assert!(s.inbox_items().unwrap().is_empty());
            assert!(s.setting(RUNNING_KEY).unwrap().is_none());
        }

        // 이미 한 번 다시 한 정기 작업이 또 끊겼으면 다시 하지 않고 알린다.
        engine.mark_running(
            &DueJob {
                schedule_id: "x".into(),
                action: "launch_rockets".into(),
                payload: serde_json::json!({}),
                due_at: now,
            },
            true,
        );
        let again = engine.recover().await;
        assert!(again.routine.is_none() && again.queued.is_empty());
        let sent: Vec<String> = channel.take_sent().into_iter().map(|(_, b)| b).collect();
        assert!(
            sent.iter().any(|b| b.contains("다시 하지 않았습니다")),
            "{sent:?}"
        );

        // 멈추는 중에 끊긴 메시지는 받은 편지함에 남는다(다시 뜬 뒤 알린다).
        let (stopping, _, _) = build(store.clone());
        stopping.set_stopping();
        let (jobs_tx, jobs_rx) = mpsc::channel(4);
        let (_pokes_tx, pokes_rx) = mpsc::unbounded_channel();
        let worker = tokio::spawn(run_worker(
            stopping.clone(),
            Vec::<FakeSource>::new,
            None,
            jobs_rx,
            pokes_rx,
        ));
        let last = message("멈추는 중에 온 것", 5);
        stopping.inbox_add(&last);
        jobs_tx.send(Job::Chat(last)).await.unwrap();
        drop(jobs_tx);
        worker.await.unwrap();
        let left = store.lock().unwrap().inbox_items().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].state, "running");
    }
}
