//! 주기 점검과 알림 묶기(결정 0015).
//!
//! - 주기 점검: 진행 상태를 코드로 훑어(답을 기다리는 작업, 곧 만료되는 승인 카드) 소식으로 쌓는다. 훑을 것이
//!   없으면 그대로 끝나고 모델을 부르지 않는다(명세 "훑을 항목이 없으면 코드 단계에서 끝내 토큰을 쓰지 않는다").
//! - 알림 묶기: 급하지 않은 소식은 바로 보내지 않고 모았다가, 가장 오래된 것이 30분 넘었거나 5건이 모이면
//!   메시지 하나로 보낸다. 방해 금지 시간에는 보내지 않는다. 급한 알림(오류·인증 만료)은 `alert`로 바로 간다.

use chrono::{DateTime, Duration, Utc};
use lucia_core::TaskStatus;

use super::{AgentRunner, ChannelAdapter, Engine};

/// 소식 묶음을 보내기까지 기다리는 시간 [제안].
pub const BATCH_WINDOW_MINUTES: i64 = 30;
/// 이만큼 모이면 기다리지 않고 보낸다 [제안].
pub const BATCH_MAX: usize = 5;
/// 같은 항목을 다시 알리기까지 [제안].
const RENOTIFY_HOURS: i64 = 24;
/// 답을 이만큼 기다린 작업을 알린다 [제안].
const WAITING_HOURS: i64 = 24;
/// 만료까지 이만큼 남은 승인 카드를 알린다 [제안].
const EXPIRING_HOURS: i64 = 3;

/// 주기 점검이 찾은 것 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckItem {
    pub key: String,
    pub text: String,
}

impl<R: AgentRunner, C: ChannelAdapter> Engine<R, C> {
    /// 진행 상태를 코드로 훑는다(모델 없음).
    pub fn check_items(&self, now: DateTime<Utc>) -> anyhow::Result<Vec<CheckItem>> {
        let tz = self.cfg.timezone;
        let store = self.store();
        let mut items = Vec::new();
        for t in store.open_tasks()? {
            let waiting = matches!(t.status, TaskStatus::WaitingInput | TaskStatus::Reported);
            if waiting && t.updated_at < now - Duration::hours(WAITING_HOURS) {
                let what = if t.status == TaskStatus::Reported {
                    "결과를 확인해 주세요"
                } else {
                    "답을 기다립니다"
                };
                items.push(CheckItem {
                    key: format!("{}:{}", t.id, t.status.as_str()),
                    text: format!(
                        "작업 \"{}\"이(가) {} 동안 {what}.",
                        t.title,
                        since(now - t.updated_at)
                    ),
                });
            }
        }
        for a in store.recent_approvals(50)? {
            if a.status == lucia_store::ApprovalStatus::Pending
                && !crate::attention::quiet_expiry(&a.kind)
                && a.expires_at > now
                && a.expires_at < now + Duration::hours(EXPIRING_HOURS)
            {
                items.push(CheckItem {
                    key: format!("approval:{}", a.id),
                    text: format!(
                        "승인 카드가 {}에 만료됩니다: {}",
                        a.expires_at.with_timezone(&tz).format("%H:%M"),
                        a.summary
                    ),
                });
            }
        }
        Ok(items)
    }

    /// 주기 점검: 찾은 것을 소식으로 쌓는다(같은 항목은 하루 한 번). 쌓은 수를 돌려준다.
    pub fn periodic_check(&self) -> anyhow::Result<usize> {
        let now = self.clock.now();
        let items = self.check_items(now)?;
        let store = self.store();
        let mut added = 0;
        for item in items {
            if store.add_notice(
                "periodic_check",
                Some(&item.key),
                &crate::mask::global().mask(&item.text),
                now - Duration::hours(RENOTIFY_HOURS),
                now,
            )? {
                added += 1;
            }
        }
        Ok(added)
    }

    /// 모인 소식을 보낼 때가 됐으면 하나로 묶어 보낸다. 보낸 소식 수를 돌려준다.
    pub async fn flush_notices(&self) -> anyhow::Result<usize> {
        let now = self.clock.now();
        let (pending, quiet) = {
            let store = self.store();
            (
                store.pending_notices()?,
                crate::autonomy::quiet_hours(&store),
            )
        };
        let Some(oldest) = pending.first() else {
            return Ok(0);
        };
        if quiet.is_some_and(|q| q.contains(now, self.cfg.timezone)) {
            return Ok(0);
        }
        let due = pending.len() >= BATCH_MAX
            || oldest.created_at <= now - Duration::minutes(BATCH_WINDOW_MINUTES);
        if !due || !self.channel.ready() {
            return Ok(0);
        }
        let Some(target) = self.notify_target() else {
            return Ok(0);
        };
        let mut body = String::from("📬 모아 보는 소식");
        for n in &pending {
            body.push_str("\n- ");
            body.push_str(&n.body);
        }
        self.send(&target, body).await?;
        let ids: Vec<i64> = pending.iter().map(|n| n.id).collect();
        self.store().mark_notices_sent(&ids, now)?;
        Ok(ids.len())
    }
}

fn since(d: Duration) -> String {
    if d.num_days() >= 1 {
        format!("{}일", d.num_days())
    } else {
        format!("{}시간", d.num_hours().max(1))
    }
}
