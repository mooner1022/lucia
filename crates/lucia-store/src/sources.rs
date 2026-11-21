//! 사전 체크 소스의 기준점과 감지한 변화.
//!
//! 기준점(cursor)을 저장해 두므로 재시작 뒤에도 "바뀐 것"만 이어서 감지한다. 첫 동기화는 기준선만
//! 잡고 변화를 보고하지 않는다(그러지 않으면 첫날 모든 일정이 "새것"이 된다).

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use crate::{Result, Store, parse_opt_ts, parse_ts, ts};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SourceState {
    pub source: String,
    pub cursor: Option<String>,
    pub baseline_at: Option<DateTime<Utc>>,
    pub checked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewSourceChange {
    pub source: String,
    pub change_key: String,
    pub summary: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceChange {
    pub id: i64,
    pub source: String,
    pub detected_at: DateTime<Utc>,
    pub change_key: String,
    pub summary: String,
    pub payload: Value,
}

impl Store {
    pub fn source_state(&self, source: &str) -> Result<Option<SourceState>> {
        self.conn
            .query_row(
                "SELECT source, cursor, baseline_at, checked_at, last_error FROM source_state
                 WHERE source = ?1",
                [source],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()?
            .map(|(source, cursor, baseline, checked, last_error)| {
                Ok(SourceState {
                    source,
                    cursor,
                    baseline_at: parse_opt_ts(baseline)?,
                    checked_at: parse_opt_ts(checked)?,
                    last_error,
                })
            })
            .transpose()
    }

    /// 확인 결과를 한 번에 반영한다: 기준점 갱신, 오류 해제, 변화 추가(중복 무시).
    /// 추가된 변화 수를 돌려준다.
    pub fn commit_source_check(
        &mut self,
        source: &str,
        cursor: Option<&str>,
        changes: &[NewSourceChange],
        now: DateTime<Utc>,
    ) -> Result<usize> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO source_state (source, cursor, baseline_at, checked_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT (source) DO UPDATE SET
                cursor = excluded.cursor,
                baseline_at = COALESCE(source_state.baseline_at, excluded.baseline_at),
                checked_at = excluded.checked_at,
                last_error = NULL, last_error_at = NULL",
            params![source, cursor, ts(now)],
        )?;
        let mut added = 0;
        for c in changes {
            added += tx.execute(
                "INSERT OR IGNORE INTO source_changes (source, detected_at, change_key, summary, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![c.source, ts(now), c.change_key, c.summary, serde_json::to_string(&c.payload)?],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }

    /// 기준점을 버린다(계정을 다시 연결할 때). 다음 확인이 기준선을 새로 잡는다. 이미 감지한 변화는 남긴다.
    pub fn reset_source(&self, source: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM source_state WHERE source = ?1", [source])?;
        Ok(())
    }

    pub fn record_source_error(&self, source: &str, error: &str, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO source_state (source, last_error, last_error_at, checked_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT (source) DO UPDATE SET
                last_error = excluded.last_error, last_error_at = excluded.last_error_at,
                checked_at = excluded.checked_at",
            params![source, error, ts(now)],
        )?;
        Ok(())
    }

    /// 아직 브리핑에 넣지 않은 변화(감지 순).
    pub fn pending_source_changes(&self) -> Result<Vec<SourceChange>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source, detected_at, change_key, summary, payload FROM source_changes
             WHERE consumed_at IS NULL ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (id, source, detected, change_key, summary, payload) = row?;
            Ok(SourceChange {
                id,
                source,
                detected_at: parse_ts(&detected)?,
                change_key,
                summary,
                payload: serde_json::from_str(&payload)?,
            })
        })
        .collect()
    }

    pub fn consume_source_changes(&mut self, ids: &[i64], now: DateTime<Utc>) -> Result<()> {
        let tx = self.conn.transaction()?;
        for id in ids {
            tx.execute(
                "UPDATE source_changes SET consumed_at = ?2 WHERE id = ?1 AND consumed_at IS NULL",
                params![id, ts(now)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    fn change(key: &str) -> NewSourceChange {
        NewSourceChange {
            source: "calendar".into(),
            change_key: key.into(),
            summary: format!("변경 {key}"),
            payload: serde_json::json!({"key": key}),
        }
    }

    #[test]
    fn baseline_then_changes_then_consume() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(store.source_state("calendar").unwrap().is_none());

        // 첫 확인: 기준선만.
        store
            .commit_source_check("calendar", Some("tok-1"), &[], at(6, 0))
            .unwrap();
        let s = store.source_state("calendar").unwrap().unwrap();
        assert_eq!(s.cursor.as_deref(), Some("tok-1"));
        assert_eq!(s.baseline_at, Some(at(6, 0)));

        // 두 번째: 변화 두 개, 하나는 중복.
        let added = store
            .commit_source_check(
                "calendar",
                Some("tok-2"),
                &[change("a@1"), change("a@1"), change("b@1")],
                at(7, 0),
            )
            .unwrap();
        assert_eq!(added, 2);
        let s = store.source_state("calendar").unwrap().unwrap();
        assert_eq!(
            s.baseline_at,
            Some(at(6, 0)),
            "기준선 시각은 처음 것을 유지"
        );

        let pending = store.pending_source_changes().unwrap();
        assert_eq!(pending.len(), 2);
        store
            .consume_source_changes(&[pending[0].id], at(7, 30))
            .unwrap();
        assert_eq!(store.pending_source_changes().unwrap().len(), 1);
    }

    #[test]
    fn reset_starts_a_new_baseline_and_keeps_changes() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .commit_source_check("calendar", Some("tok-1"), &[], at(6, 0))
            .unwrap();
        store
            .commit_source_check("calendar", Some("tok-2"), &[change("a@1")], at(7, 0))
            .unwrap();
        store.reset_source("calendar").unwrap();
        assert!(store.source_state("calendar").unwrap().is_none());
        store
            .commit_source_check("calendar", Some("tok-9"), &[], at(8, 0))
            .unwrap();
        let s = store.source_state("calendar").unwrap().unwrap();
        assert_eq!(s.baseline_at, Some(at(8, 0)), "기준선 시각을 새로 잡는다");
        assert_eq!(store.pending_source_changes().unwrap().len(), 1);
    }

    #[test]
    fn errors_are_recorded_and_cleared() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .record_source_error("mail", "refresh token 만료", at(6, 0))
            .unwrap();
        assert_eq!(
            store
                .source_state("mail")
                .unwrap()
                .unwrap()
                .last_error
                .as_deref(),
            Some("refresh token 만료")
        );
        store
            .commit_source_check("mail", Some("h-9"), &[], at(7, 0))
            .unwrap();
        assert!(
            store
                .source_state("mail")
                .unwrap()
                .unwrap()
                .last_error
                .is_none()
        );
    }
}
