//! 받은 편지함(결정 0050): 받은 메시지를 처리가 끝날 때까지 남긴다(작업 큐는 메모리에만 있다).

use chrono::{DateTime, Utc};
use rusqlite::params;

use crate::{Result, Store, parse_ts, ts};

#[derive(Debug, Clone, PartialEq)]
pub struct InboxItem {
    pub key: String,
    /// 메시지(JSON).
    pub payload: String,
    /// queued | running
    pub state: String,
    pub enqueued_at: DateTime<Utc>,
}

impl Store {
    /// 받은 메시지를 넣는다(같은 키가 있으면 그대로 둔다).
    pub fn inbox_add(&self, key: &str, payload: &str, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO inbox (key, payload, state, enqueued_at) VALUES (?1, ?2, 'queued', ?3)",
            params![key, payload, ts(now)],
        )?;
        Ok(())
    }

    /// 처리를 시작했다.
    pub fn inbox_start(&self, key: &str, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "UPDATE inbox SET state = 'running', started_at = ?2 WHERE key = ?1",
            params![key, ts(now)],
        )?;
        Ok(())
    }

    /// 처리가 끝났다(성공이든 실패든 답이나 알림이 갔다).
    pub fn inbox_done(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM inbox WHERE key = ?1", [key])?;
        Ok(())
    }

    /// 남아 있는 것(받은 순).
    pub fn inbox_items(&self) -> Result<Vec<InboxItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT key, payload, state, enqueued_at FROM inbox ORDER BY enqueued_at ASC, key ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|r| {
            let (key, payload, state, at) = r?;
            Ok(InboxItem {
                key,
                payload,
                state,
                enqueued_at: parse_ts(&at)?,
            })
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    #[test]
    fn items_stay_until_done() {
        let store = Store::open_in_memory().unwrap();
        let at = |m| Utc.with_ymd_and_hms(2026, 11, 20, 9, m, 0).unwrap();
        store.inbox_add("a", "{\"n\":1}", at(1)).unwrap();
        store.inbox_add("b", "{\"n\":2}", at(2)).unwrap();
        store.inbox_add("a", "{\"n\":9}", at(3)).unwrap();
        store.inbox_start("a", at(4)).unwrap();
        let items = store.inbox_items().unwrap();
        assert_eq!(
            items
                .iter()
                .map(|i| (i.key.as_str(), i.state.as_str(), i.payload.as_str()))
                .collect::<Vec<_>>(),
            [("a", "running", "{\"n\":1}"), ("b", "queued", "{\"n\":2}")]
        );
        store.inbox_done("a").unwrap();
        store.inbox_done("없는 것").unwrap();
        assert_eq!(store.inbox_items().unwrap().len(), 1);
    }
}
