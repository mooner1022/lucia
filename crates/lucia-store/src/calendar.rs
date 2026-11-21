//! 캘린더 쓰기 기록(결정 0033): 직전 상태와 뒤 상태.

use chrono::{DateTime, Utc};
use rusqlite::params;
use serde_json::Value;

use crate::{Result, Store, parse_ts, ts};

#[derive(Debug, Clone, PartialEq)]
pub struct CalendarEdit {
    pub at: DateTime<Utc>,
    pub op: String,
    pub event_id: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

impl Store {
    pub fn add_calendar_edit(
        &self,
        op: &str,
        event_id: &str,
        before: Option<&Value>,
        after: Option<&Value>,
        run: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO calendar_edits (at, op, event_id, before, after, run_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                ts(now),
                op,
                event_id,
                before.map(Value::to_string),
                after.map(Value::to_string),
                run
            ],
        )?;
        Ok(())
    }

    /// 최근 기록(최근 것부터).
    pub fn calendar_edits(&self, limit: usize) -> Result<Vec<CalendarEdit>> {
        let mut stmt = self.conn.prepare(
            "SELECT at, op, event_id, before, after FROM calendar_edits ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        rows.map(|r| {
            let (at, op, event_id, before, after) = r?;
            Ok(CalendarEdit {
                at: parse_ts(&at)?,
                op,
                event_id,
                before: before.map(|b| serde_json::from_str(&b)).transpose()?,
                after: after.map(|a| serde_json::from_str(&a)).transpose()?,
            })
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_keep_before_and_after() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        let before = serde_json::json!({"summary": "a"});
        store
            .add_calendar_edit("update", "e1", Some(&before), None, Some("run"), now)
            .unwrap();
        let e = &store.calendar_edits(5).unwrap()[0];
        assert_eq!(e.before.as_ref().unwrap()["summary"], "a");
        assert!(e.after.is_none());
    }
}
