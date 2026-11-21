//! 스케줄러: cron 루틴과 일회성 리마인더(명세 §스케줄링).
//!
//! - 상태(`next_due_at`)는 SQLite에 있어 재시작 뒤에도 유지된다.
//! - 놓친 실행: 예정 시각에서 허용 지연 안이면 실행하고, 넘으면 건너뛰고 기록한다. 여러 번 놓쳤으면
//!   가장 이른 것 하나만 판단하고 다음 시각은 "지금 이후"로 옮긴다(밀린 실행을 몰아서 하지 않는다).
//! - 시계는 주입한다([`Clock`]). 시험에서는 고정 시계를 쓴다.

use std::str::FromStr;
#[cfg(test)]
use std::sync::Mutex;

use anyhow::{Context, bail};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use croner::Cron;
use lucia_store::{Schedule, ScheduleKind, ScheduleOutcome, Store};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[cfg_attr(test, allow(dead_code))]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// 시험용 시계. 직접 옮긴다.
#[cfg(test)]
pub struct ManualClock(Mutex<DateTime<Utc>>);

#[cfg(test)]
impl ManualClock {
    pub fn new(at: DateTime<Utc>) -> Self {
        Self(Mutex::new(at))
    }

    pub fn set(&self, at: DateTime<Utc>) {
        *self.0.lock().unwrap() = at;
    }
}

#[cfg(test)]
impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

/// 설정에서 오는 루틴 정의.
#[derive(Debug, Clone, PartialEq)]
pub struct Routine {
    pub id: String,
    pub cron: String,
    pub timezone: String,
    pub action: String,
    pub max_delay_secs: u64,
}

/// 실행할 차례가 된 일.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DueJob {
    pub schedule_id: String,
    pub action: String,
    pub payload: serde_json::Value,
    pub due_at: DateTime<Utc>,
}

/// `after` 다음(포함하지 않음) 실행 시각. 일회성은 `after`보다 늦을 때만.
pub fn next_after(
    kind: ScheduleKind,
    spec: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> anyhow::Result<Option<DateTime<Utc>>> {
    match kind {
        ScheduleKind::Cron => {
            let tz: Tz = timezone
                .parse()
                .map_err(|e| anyhow::anyhow!("시간대 {timezone:?}: {e}"))?;
            let cron = Cron::from_str(spec).with_context(|| format!("cron 식 {spec:?}"))?;
            let next = cron
                .find_next_occurrence(&after.with_timezone(&tz), false)
                .with_context(|| format!("cron 식 {spec:?}의 다음 시각"))?;
            Ok(Some(next.with_timezone(&Utc)))
        }
        ScheduleKind::Once => {
            let at = DateTime::parse_from_rfc3339(spec)
                .with_context(|| format!("일회성 시각 {spec:?}"))?
                .with_timezone(&Utc);
            Ok((at > after).then_some(at))
        }
    }
}

/// 설정의 루틴을 저장소에 맞춘다. 식·시간대가 그대로면 저장된 `next_due_at`을 유지해
/// 멈춰 있던 동안 놓친 실행을 알아채게 한다. 바뀌었거나 새것이면 `now` 다음으로 잡는다.
/// 설정에서 사라진 루틴은 끈다.
pub fn sync_routines(
    store: &Store,
    routines: &[Routine],
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    for r in routines {
        let existing = store.schedule(&r.id)?;
        let unchanged = existing.as_ref().is_some_and(|s| {
            s.spec == r.cron && s.timezone == r.timezone && s.next_due_at.is_some()
        });
        let next_due_at = match (&existing, unchanged) {
            (Some(s), true) => s.next_due_at,
            _ => next_after(ScheduleKind::Cron, &r.cron, &r.timezone, now)?,
        };
        store.upsert_schedule(&Schedule {
            id: r.id.clone(),
            kind: ScheduleKind::Cron,
            spec: r.cron.clone(),
            timezone: r.timezone.clone(),
            action: r.action.clone(),
            payload: serde_json::json!({}),
            max_delay_secs: r.max_delay_secs,
            next_due_at,
            last_fired_at: existing.and_then(|s| s.last_fired_at),
            enabled: true,
        })?;
    }
    let keep: Vec<String> = routines.iter().map(|r| r.id.clone()).collect();
    let disabled = store.disable_cron_schedules_except(&keep)?;
    if disabled > 0 {
        tracing::info!(disabled, "설정에서 사라진 루틴을 껐다");
    }
    Ok(())
}

/// 때가 된 스케줄을 판단하고 기록한다. 실행할 일만 돌려준다(건너뛴 것은 기록만).
pub fn tick(store: &mut Store, now: DateTime<Utc>) -> anyhow::Result<Vec<DueJob>> {
    let mut jobs = Vec::new();
    for s in store.due_schedules(now)? {
        let Some(due_at) = s.next_due_at else {
            bail!(
                "due_schedules가 next_due_at 없는 스케줄을 돌려줬다: {}",
                s.id
            );
        };
        let late_secs = (now - due_at).num_seconds().max(0) as u64;
        let outcome = if late_secs <= s.max_delay_secs {
            ScheduleOutcome::Fired
        } else {
            ScheduleOutcome::SkippedLate
        };
        let next = match s.kind {
            ScheduleKind::Cron => next_after(s.kind, &s.spec, &s.timezone, now)?,
            ScheduleKind::Once => None,
        };
        let first_time = store.record_schedule_decision(&s.id, due_at, outcome, next, now)?;
        match outcome {
            ScheduleOutcome::Fired if first_time => jobs.push(DueJob {
                schedule_id: s.id.clone(),
                action: s.action.clone(),
                payload: s.payload.clone(),
                due_at,
            }),
            ScheduleOutcome::SkippedLate => tracing::warn!(
                schedule = %s.id, %due_at, late_secs,
                "허용 지연을 넘겨 건너뛰었다"
            ),
            _ => {}
        }
    }
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn kst(d: u32, h: u32, m: u32) -> DateTime<Utc> {
        chrono_tz::Asia::Seoul
            .with_ymd_and_hms(2026, 11, d, h, m, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn morning() -> Routine {
        Routine {
            id: "morning-briefing".into(),
            cron: "30 7 * * *".into(),
            timezone: "Asia/Seoul".into(),
            action: "morning_briefing".into(),
            max_delay_secs: 3600,
        }
    }

    #[test]
    fn next_after_respects_timezone() {
        let next = next_after(
            ScheduleKind::Cron,
            "30 7 * * *",
            "Asia/Seoul",
            kst(16, 8, 0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(next, kst(17, 7, 30));
        // UTC로는 전날 22:30.
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 11, 16, 22, 30, 0).unwrap());
    }

    #[test]
    fn once_in_the_past_has_no_next() {
        let spec = "2026-11-16T09:00:00+09:00";
        assert_eq!(
            next_after(ScheduleKind::Once, spec, "Asia/Seoul", kst(16, 8, 0)).unwrap(),
            Some(kst(16, 9, 0))
        );
        assert_eq!(
            next_after(ScheduleKind::Once, spec, "Asia/Seoul", kst(16, 10, 0)).unwrap(),
            None
        );
    }

    #[test]
    fn fires_once_on_time_then_waits_for_next_day() {
        let mut store = Store::open_in_memory().unwrap();
        sync_routines(&store, &[morning()], kst(16, 6, 0)).unwrap();

        assert!(tick(&mut store, kst(16, 7, 29)).unwrap().is_empty());
        let jobs = tick(&mut store, kst(16, 7, 30)).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].action, "morning_briefing");
        assert_eq!(jobs[0].due_at, kst(16, 7, 30));
        assert!(
            tick(&mut store, kst(16, 7, 31)).unwrap().is_empty(),
            "한 번만"
        );
        assert_eq!(
            store
                .schedule("morning-briefing")
                .unwrap()
                .unwrap()
                .next_due_at,
            Some(kst(17, 7, 30))
        );
    }

    #[test]
    fn missed_run_within_delay_fires_after_restart() {
        let mut store = Store::open_in_memory().unwrap();
        sync_routines(&store, &[morning()], kst(16, 6, 0)).unwrap();
        // 07:30에 데몬이 멈춰 있었다. 08:10에 재시작: 설정을 다시 맞춰도 놓친 예정 시각은 유지된다.
        sync_routines(&store, &[morning()], kst(16, 8, 10)).unwrap();
        let jobs = tick(&mut store, kst(16, 8, 10)).unwrap();
        assert_eq!(jobs.len(), 1, "40분 늦음 < 허용 1시간");
        assert_eq!(jobs[0].due_at, kst(16, 7, 30));
    }

    #[test]
    fn missed_run_beyond_delay_is_skipped_and_recorded() {
        let mut store = Store::open_in_memory().unwrap();
        sync_routines(&store, &[morning()], kst(16, 6, 0)).unwrap();
        // 사흘 동안 멈춰 있다가 19일 10:00에 재시작.
        let jobs = tick(&mut store, kst(19, 10, 0)).unwrap();
        assert!(jobs.is_empty());
        assert_eq!(
            store.schedule_runs("morning-briefing").unwrap(),
            vec![(kst(16, 7, 30), "skipped_late".to_owned())]
        );
        // 밀린 실행을 몰아서 하지 않고 다음 날로 넘어간다.
        assert_eq!(
            store
                .schedule("morning-briefing")
                .unwrap()
                .unwrap()
                .next_due_at,
            Some(kst(20, 7, 30))
        );
    }

    #[test]
    fn removed_routine_stops_firing() {
        let mut store = Store::open_in_memory().unwrap();
        sync_routines(&store, &[morning()], kst(16, 6, 0)).unwrap();
        sync_routines(&store, &[], kst(16, 6, 10)).unwrap();
        assert!(tick(&mut store, kst(16, 7, 30)).unwrap().is_empty());
        assert!(!store.schedule("morning-briefing").unwrap().unwrap().enabled);
    }

    #[test]
    fn changed_spec_is_rescheduled_from_now() {
        let store = Store::open_in_memory().unwrap();
        sync_routines(&store, &[morning()], kst(16, 6, 0)).unwrap();
        let mut later = morning();
        later.cron = "0 8 * * *".into();
        sync_routines(&store, &[later], kst(16, 6, 30)).unwrap();
        assert_eq!(
            store
                .schedule("morning-briefing")
                .unwrap()
                .unwrap()
                .next_due_at,
            Some(kst(16, 8, 0))
        );
    }
}
