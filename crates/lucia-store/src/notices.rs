//! 묶어 보낼 소식(결정 0015). 급한 알림(오류, 인증 만료)은 여기를 거치지 않고 바로 간다.

use chrono::{DateTime, Utc};
use rusqlite::params;

use crate::{Result, Store, parse_ts, ts};

#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub id: i64,
    pub created_at: DateTime<Utc>,
    pub kind: String,
    pub body: String,
}

impl Store {
    /// 소식을 쌓는다. `key`가 있으면 `since` 뒤에 같은 키로 쌓은 것이 있을 때 넣지 않는다(`Ok(false)`).
    pub fn add_notice(
        &self,
        kind: &str,
        key: Option<&str>,
        body: &str,
        since: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        if let Some(key) = key {
            let seen: i64 = self.conn.query_row(
                "SELECT count(*) FROM notices WHERE key = ?1 AND created_at >= ?2",
                params![key, ts(since)],
                |r| r.get(0),
            )?;
            if seen > 0 {
                return Ok(false);
            }
        }
        self.conn.execute(
            "INSERT INTO notices (created_at, kind, key, body) VALUES (?1, ?2, ?3, ?4)",
            params![ts(now), kind, key, body],
        )?;
        Ok(true)
    }

    /// 아직 보내지 않은 소식(오래된 것부터).
    pub fn pending_notices(&self) -> Result<Vec<Notice>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, kind, body FROM notices WHERE sent_at IS NULL ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, at, kind, body) = row?;
            out.push(Notice {
                id,
                created_at: parse_ts(&at)?,
                kind,
                body,
            });
        }
        Ok(out)
    }

    pub fn mark_notices_sent(&self, ids: &[i64], now: DateTime<Utc>) -> Result<()> {
        for id in ids {
            self.conn.execute(
                "UPDATE notices SET sent_at = ?2 WHERE id = ?1",
                params![id, ts(now)],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    #[test]
    fn notices_dedupe_by_key_and_track_sending() {
        let store = Store::open_in_memory().unwrap();
        assert!(
            store
                .add_notice(
                    "periodic_check",
                    Some("task-1:waiting"),
                    "답 기다림",
                    at(9, 0),
                    at(10, 0)
                )
                .unwrap()
        );
        assert!(
            !store
                .add_notice(
                    "periodic_check",
                    Some("task-1:waiting"),
                    "답 기다림",
                    at(9, 0),
                    at(11, 0)
                )
                .unwrap(),
            "같은 키는 기간 안에 한 번"
        );
        assert!(
            store
                .add_notice("periodic_check", None, "기타", at(9, 0), at(11, 0))
                .unwrap()
        );
        let pending = store.pending_notices().unwrap();
        assert_eq!(pending.len(), 2);
        store
            .mark_notices_sent(&pending.iter().map(|n| n.id).collect::<Vec<_>>(), at(12, 0))
            .unwrap();
        assert!(store.pending_notices().unwrap().is_empty());
    }
}
