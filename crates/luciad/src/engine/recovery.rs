//! 재시작 복구(결정 0050). 작업 큐는 메모리에만 있어, 데몬이 다시 뜨면 기다리던 일과 하던 일이 흔적 없이
//! 사라졌다. 그래서:
//!
//! - 받은 메시지는 처리가 끝날 때까지 받은 편지함(`inbox` 표)에 둔다.
//!   - 기다리던 것(`queued`)은 다시 뜬 뒤 이어서 처리한다.
//!   - 처리하다 끊긴 것(`running`)은 다시 하지 않고 사용자에게 알린다. 끊긴 실행이 무엇까지 했는지(메일 카드,
//!     그림 생성 같은 것) 데몬이 알 수 없기 때문이다.
//! - 정기 작업은 하는 동안 설정 [`RUNNING_KEY`]에 적어 둔다. 끊겼으면 한 번만 다시 한다(늦지 않았을 때).
//! - 다시 뜰 때 VM에 남은 예전 실행을 끝낸다(호스트 쪽이 죽어도 VM 안의 claude는 계속 돈다).

use chrono::{DateTime, Utc};
use lucia_core::Inbound;

use super::{AgentRunner, ChannelAdapter, Engine, failure_notice};
use crate::scheduler::DueJob;

/// 하는 중인 정기 작업(설정 키).
pub const RUNNING_KEY: &str = "runtime.running";
/// 기다리던 메시지를 이어서 처리하는 한도. 더 오래된 것은 알리기만 한다.
const QUEUED_MAX_AGE_HOURS: i64 = 12;
/// 끊긴 정기 작업을 다시 하는 한도.
const ROUTINE_RETRY_MINUTES: i64 = 60;

/// 받은 편지함의 키.
pub fn inbox_key(msg: &Inbound) -> String {
    format!(
        "{}|{}|{}",
        msg.conversation.0,
        msg.sender,
        msg.received_at
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
    )
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Running {
    due: DueJob,
    started_at: DateTime<Utc>,
    /// 이미 한 번 다시 한 것이다(또 끊기면 다시 하지 않는다).
    retried: bool,
}

/// 다시 뜬 뒤 이어서 할 일.
#[derive(Debug, Default)]
pub struct Recovered {
    pub queued: Vec<Inbound>,
    pub routine: Option<DueJob>,
}

impl<R: AgentRunner, C: ChannelAdapter> Engine<R, C> {
    /// 받은 메시지를 받은 편지함에 넣는다(큐에 넣기 전에).
    pub fn inbox_add(&self, msg: &Inbound) {
        if let Ok(json) = serde_json::to_string(msg) {
            let _ = self
                .store()
                .inbox_add(&inbox_key(msg), &json, self.clock.now());
        }
    }

    pub fn inbox_start(&self, msg: &Inbound) {
        let _ = self.store().inbox_start(&inbox_key(msg), self.clock.now());
    }

    pub fn inbox_done(&self, msg: &Inbound) {
        let _ = self.store().inbox_done(&inbox_key(msg));
    }

    /// 정기 작업을 시작한다고 적는다.
    pub fn mark_running(&self, due: &DueJob, retried: bool) {
        let mark = Running {
            due: due.clone(),
            started_at: self.clock.now(),
            retried,
        };
        if let Ok(json) = serde_json::to_string(&mark) {
            let _ = self
                .store()
                .set_setting(RUNNING_KEY, &json, self.clock.now());
        }
    }

    pub fn clear_running(&self) {
        let _ = self.store().delete_setting(RUNNING_KEY);
    }

    /// 데몬이 멈추는 중이다: 지금 끊기는 일의 흔적을 지우지 않는다(다시 뜬 뒤 복구가 본다).
    pub fn set_stopping(&self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn stopping(&self) -> bool {
        self.stopping.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 다시 뜬 뒤 한 번: 남은 실행을 끝내고, 끊긴 일을 알리고, 이어서 할 일을 돌려준다.
    pub async fn recover(&self) -> Recovered {
        self.runner.kill_orphans().await;
        let now = self.clock.now();
        let mut out = Recovered::default();
        let items = self.store().inbox_items().unwrap_or_default();
        let mut interrupted: Vec<Inbound> = Vec::new();
        let mut stale: Vec<Inbound> = Vec::new();
        for item in items {
            let Ok(msg) = serde_json::from_str::<Inbound>(&item.payload) else {
                let _ = self.store().inbox_done(&item.key);
                continue;
            };
            if item.state == "running" {
                let _ = self.store().inbox_done(&item.key);
                interrupted.push(msg);
            } else if now - item.enqueued_at > chrono::Duration::hours(QUEUED_MAX_AGE_HOURS) {
                let _ = self.store().inbox_done(&item.key);
                stale.push(msg);
            } else {
                out.queued.push(msg);
            }
        }
        for msg in &interrupted {
            let notice = failure_notice(
                "데몬이 다시 시작되면서 하던 답이 끊겼습니다",
                "어디까지 했는지는 알 수 없습니다. 필요하면 다시 말씀해 주세요.",
                msg.venue.is_some(),
            );
            if let Err(err) = self.send(&msg.conversation, notice).await {
                tracing::warn!(%err, "끊긴 답을 알리지 못했다");
            }
        }
        for msg in &stale {
            let notice = failure_notice(
                "데몬이 멈춰 있는 동안 받은 메시지를 처리하지 못했습니다",
                "오래돼서 이어서 하지 않았습니다. 필요하면 다시 말씀해 주세요.",
                msg.venue.is_some(),
            );
            let _ = self.send(&msg.conversation, notice).await;
        }
        if !interrupted.is_empty() || !out.queued.is_empty() || !stale.is_empty() {
            tracing::info!(
                interrupted = interrupted.len(),
                queued = out.queued.len(),
                stale = stale.len(),
                "재시작 복구: 받은 메시지"
            );
        }
        // 정기 작업.
        let mark = self
            .store()
            .setting(RUNNING_KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<Running>(&json).ok());
        self.clear_running();
        if let Some(mark) = mark {
            let fresh = now - mark.started_at <= chrono::Duration::minutes(ROUTINE_RETRY_MINUTES);
            if !mark.retried && fresh {
                tracing::info!(action = %mark.due.action, "재시작 복구: 끊긴 정기 작업을 다시 한다");
                out.routine = Some(mark.due);
            } else {
                self.alert(format!(
                    "데몬이 다시 시작되면서 정기 작업({})이 끊겼습니다. 다시 하지 않았습니다.",
                    mark.due.action
                ))
                .await;
            }
        }
        out
    }
}
