//! 스케줄 저장. 다음 실행 시각 계산(cron)은 데몬의 스케줄러가 하고, 여기서는 상태만 지킨다.
//!
//! `next_due_at`을 저장해 두므로, 데몬이 멈춰 있던 동안 놓친 실행을 재시작 뒤에 알아챈다.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use crate::{Result, Store, StoreError, parse_opt_ts, ts};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ScheduleKind {
    /// cron 식. 실행할 때마다 다음 시각을 다시 계산한다.
    Cron,
    /// 한 번만(리마인더). 실행 뒤 `next_due_at`이 비워진다.
    Once,
}

impl ScheduleKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cron => "cron",
            Self::Once => "once",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "cron" => Self::Cron,
            "once" => Self::Once,
            other => return Err(StoreError::Corrupt(format!("schedule kind {other:?}"))),
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Schedule {
    pub id: String,
    pub kind: ScheduleKind,
    pub spec: String,
    pub timezone: String,
    pub action: String,
    pub payload: Value,
    pub max_delay_secs: u64,
    pub next_due_at: Option<DateTime<Utc>>,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleOutcome {
    Fired,
    /// 허용 지연을 넘겨 건너뛰었다.
    SkippedLate,
}

impl ScheduleOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fired => "fired",
            Self::SkippedLate => "skipped_late",
        }
    }
}

const COLUMNS: &str = "id, kind, spec, timezone, action, payload, max_delay_secs, next_due_at, \
                       last_fired_at, enabled";

type RawSchedule = (
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    bool,
);

fn raw(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawSchedule> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

fn finish(r: RawSchedule) -> Result<Schedule> {
    let (id, kind, spec, timezone, action, payload, max_delay, next, last, enabled) = r;
    Ok(Schedule {
        id,
        kind: ScheduleKind::parse(&kind)?,
        spec,
        timezone,
        action,
        payload: serde_json::from_str(&payload)?,
        max_delay_secs: max_delay.max(0) as u64,
        next_due_at: parse_opt_ts(next)?,
        last_fired_at: parse_opt_ts(last)?,
        enabled,
    })
}

impl Store {
    /// 스케줄을 통째로 쓴다(없으면 만들고 있으면 덮는다).
    pub fn upsert_schedule(&self, s: &Schedule) -> Result<()> {
        self.conn.execute(
            "INSERT INTO schedules (id, kind, spec, timezone, action, payload, max_delay_secs,
                                    next_due_at, last_fired_at, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT (id) DO UPDATE SET
                kind = excluded.kind, spec = excluded.spec, timezone = excluded.timezone,
                action = excluded.action, payload = excluded.payload,
                max_delay_secs = excluded.max_delay_secs, next_due_at = excluded.next_due_at,
                last_fired_at = excluded.last_fired_at, enabled = excluded.enabled",
            params![
                s.id,
                s.kind.as_str(),
                s.spec,
                s.timezone,
                s.action,
                serde_json::to_string(&s.payload)?,
                s.max_delay_secs as i64,
                s.next_due_at.map(ts),
                s.last_fired_at.map(ts),
                s.enabled,
            ],
        )?;
        Ok(())
    }

    pub fn schedule(&self, id: &str) -> Result<Option<Schedule>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM schedules WHERE id = ?1"),
                [id],
                raw,
            )
            .optional()?
            .map(finish)
            .transpose()
    }

    /// 켜져 있고 `next_due_at <= now`인 스케줄(시각 순).
    pub fn due_schedules(&self, now: DateTime<Utc>) -> Result<Vec<Schedule>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM schedules
             WHERE enabled = 1 AND next_due_at IS NOT NULL AND next_due_at <= ?1
             ORDER BY next_due_at ASC"
        ))?;
        let rows = stmt.query_map([ts(now)], raw)?;
        rows.map(|r| finish(r?)).collect()
    }

    /// 설정에서 사라진 cron 루틴을 끈다(일회성 리마인더는 건드리지 않는다). 끈 개수를 돌려준다.
    pub fn disable_cron_schedules_except(&self, keep: &[String]) -> Result<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM schedules WHERE kind = 'cron' AND enabled = 1")?;
        let ids: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut disabled = 0;
        for id in ids.iter().filter(|id| !keep.contains(id)) {
            disabled += self
                .conn
                .execute("UPDATE schedules SET enabled = 0 WHERE id = ?1", [id])?;
        }
        Ok(disabled)
    }

    /// 켜진 스케줄 전부(다음 시각 순). 상태 화면용.
    pub fn enabled_schedules(&self) -> Result<Vec<Schedule>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM schedules WHERE enabled = 1 ORDER BY next_due_at"
        ))?;
        let rows = stmt.query_map([], raw)?;
        rows.map(|r| finish(r?)).collect()
    }

    /// 다음으로 가장 이른 실행 시각(스케줄러가 잠들 시간을 정할 때).
    pub fn next_schedule_due(&self) -> Result<Option<DateTime<Utc>>> {
        let at: Option<String> = self.conn.query_row(
            "SELECT MIN(next_due_at) FROM schedules WHERE enabled = 1",
            [],
            |r| r.get(0),
        )?;
        parse_opt_ts(at)
    }

    /// 실행(또는 건너뜀) 결정을 기록하고 다음 시각을 옮긴다. 같은 (스케줄, 예정 시각)은 한 번만 기록된다.
    pub fn record_schedule_decision(
        &mut self,
        id: &str,
        due_at: DateTime<Utc>,
        outcome: ScheduleOutcome,
        next_due_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO schedule_runs (schedule_id, due_at, decided_at, outcome)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, ts(due_at), ts(now), outcome.as_str()],
        )? == 1;
        tx.execute(
            "UPDATE schedules SET next_due_at = ?2,
                 last_fired_at = CASE WHEN ?3 THEN ?4 ELSE last_fired_at END
             WHERE id = ?1",
            params![
                id,
                next_due_at.map(ts),
                outcome == ScheduleOutcome::Fired,
                ts(now)
            ],
        )?;
        tx.commit()?;
        Ok(inserted)
    }

    /// 스케줄의 실행 기록 `(예정 시각, 결과)`, 오래된 순.
    pub fn schedule_runs(&self, id: &str) -> Result<Vec<(DateTime<Utc>, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT due_at, outcome FROM schedule_runs WHERE schedule_id = ?1 ORDER BY due_at",
        )?;
        let rows = stmt.query_map([id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.map(|r| {
            let (at, outcome) = r?;
            Ok((crate::parse_ts(&at)?, outcome))
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    fn briefing(next: DateTime<Utc>) -> Schedule {
        Schedule {
            id: "morning".into(),
            kind: ScheduleKind::Cron,
            spec: "30 7 * * *".into(),
            timezone: "Asia/Seoul".into(),
            action: "morning_briefing".into(),
            payload: serde_json::json!({}),
            max_delay_secs: 3600,
            next_due_at: Some(next),
            last_fired_at: None,
            enabled: true,
        }
    }

    #[test]
    fn due_record_and_advance() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_schedule(&briefing(at(7, 30))).unwrap();
        assert!(store.due_schedules(at(7, 29)).unwrap().is_empty());
        let due = store.due_schedules(at(7, 30)).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(store.next_schedule_due().unwrap(), Some(at(7, 30)));

        let first = store
            .record_schedule_decision(
                "morning",
                at(7, 30),
                ScheduleOutcome::Fired,
                Some(at(23, 0)),
                at(7, 31),
            )
            .unwrap();
        assert!(first);
        let again = store
            .record_schedule_decision(
                "morning",
                at(7, 30),
                ScheduleOutcome::Fired,
                Some(at(23, 0)),
                at(7, 32),
            )
            .unwrap();
        assert!(!again, "같은 예정 시각은 한 번만 기록");

        let s = store.schedule("morning").unwrap().unwrap();
        assert_eq!(s.next_due_at, Some(at(23, 0)));
        assert_eq!(s.last_fired_at, Some(at(7, 32)));
        assert!(store.due_schedules(at(8, 0)).unwrap().is_empty());
        assert_eq!(
            store.schedule_runs("morning").unwrap(),
            vec![(at(7, 30), "fired".to_owned())]
        );
    }

    #[test]
    fn skipped_does_not_touch_last_fired_and_once_ends() {
        let mut store = Store::open_in_memory().unwrap();
        let mut once = briefing(at(9, 0));
        once.id = "remind".into();
        once.kind = ScheduleKind::Once;
        store.upsert_schedule(&once).unwrap();
        store
            .record_schedule_decision(
                "remind",
                at(9, 0),
                ScheduleOutcome::SkippedLate,
                None,
                at(12, 0),
            )
            .unwrap();
        let s = store.schedule("remind").unwrap().unwrap();
        assert_eq!(s.next_due_at, None);
        assert_eq!(s.last_fired_at, None);
        assert_eq!(store.next_schedule_due().unwrap(), None);
    }
}
