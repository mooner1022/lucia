//! 일정 기반 방해 금지와 단계적 독촉(결정 0034). 모델을 부르지 않는다.

use chrono::Duration;

use super::{AgentRunner, ChannelAdapter, Engine};
use crate::attention::{self as rules, Step};
use crate::autonomy::keys;

/// 바쁜 구간을 다시 읽는 간격(분).
const BUSY_REFRESH_MINUTES: i64 = 15;
const BUSY_AT_KEY: &str = "quiet.busy_at";

impl<R: AgentRunner, C: ChannelAdapter> Engine<R, C> {
    /// 일정 기반 방해 금지가 켜져 있으면 앞으로 하루의 바쁜 구간을 읽어 둔다(15분마다).
    pub async fn refresh_busy(&self) {
        let now = self.clock.now();
        let (on, fresh, except) = {
            let store = self.store();
            let on = store
                .setting(keys::QUIET_CALENDAR)
                .ok()
                .flatten()
                .as_deref()
                == Some("on");
            let fresh = store
                .setting(BUSY_AT_KEY)
                .ok()
                .flatten()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
                .is_some_and(|t| {
                    now - t.with_timezone(&chrono::Utc) < Duration::minutes(BUSY_REFRESH_MINUTES)
                });
            let except = crate::signals::parse_interests(
                &store
                    .setting(keys::QUIET_CALENDAR_EXCEPT)
                    .ok()
                    .flatten()
                    .unwrap_or_default(),
            );
            (on, fresh, except)
        };
        if !on || fresh {
            return;
        }
        let Some(read) = &self.calendar_read else {
            return;
        };
        match read.upcoming(now, now + Duration::hours(24)).await {
            Ok(events) => {
                let busy = crate::autonomy::busy_from_events(&events, &except);
                let store = self.store();
                let _ = store.set_setting(
                    keys::QUIET_BUSY,
                    &serde_json::to_string(&busy).unwrap_or_else(|_| "[]".into()),
                    now,
                );
                let _ = store.set_setting(BUSY_AT_KEY, &now.to_rfc3339(), now);
            }
            Err(err) => tracing::warn!(%err, "일정 기반 방해 금지: 일정을 읽지 못했다"),
        }
    }

    /// 독촉 단계를 올린다(작업자가 조용할 때마다). 보낸·쌓은 수를 돌려준다.
    pub async fn escalate_nudges(&self) -> anyhow::Result<usize> {
        let now = self.clock.now();
        let tz = self.cfg.timezone;
        let (list, quiet) = {
            let store = self.store();
            (
                store.open_nudges()?,
                crate::autonomy::quiet_hours(&store).is_some_and(|q| q.contains(now, tz)),
            )
        };
        let mut n_done = 0;
        for n in list {
            let Some((stage, step)) = rules::next_step(&n, now, tz) else {
                continue;
            };
            match step {
                Step::Expire => {
                    let store = self.store();
                    if store.close_nudge(&n.id, "expired", now)? {
                        let _ = store.add_activity(
                            "nudge_expired",
                            &crate::mask::global()
                                .mask(&format!("마감 지남 {}: {}", n.id, n.title)),
                            None,
                            now,
                        );
                    }
                }
                Step::Digest(text) => {
                    let store = self.store();
                    store.add_notice(
                        "nudge",
                        Some(&format!("nudge:{}:1", n.id)),
                        &crate::mask::global().mask(&text),
                        now - Duration::days(30),
                        now,
                    )?;
                    store.raise_nudge(&n.id, stage, now)?;
                    n_done += 1;
                }
                // 단독 알림과 카드는 방해 금지(일정 기반 포함)가 끝난 뒤에.
                Step::Alert(text) if !quiet => {
                    let Some(target) = self.notify_target().filter(|_| self.channel.ready()) else {
                        continue;
                    };
                    self.send(&target, text).await?;
                    self.store().raise_nudge(&n.id, stage, now)?;
                    n_done += 1;
                }
                Step::Card if !quiet => {
                    let id = rules::request_nudge_card(&self.store(), &n, now);
                    if let (Some(id), Some(posts)) = (id, &self.approvals) {
                        let _ = posts.send(id);
                    }
                    self.store().raise_nudge(&n.id, stage, now)?;
                    n_done += 1;
                }
                _ => {}
            }
        }
        Ok(n_done)
    }
}
