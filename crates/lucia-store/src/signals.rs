//! 신호 처리 기록(결정 0031). 판단은 데몬(`luciad::signals`)이 하고, 여기는 저장만 한다.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use rusqlite::params;
use serde_json::Value;

use crate::{Result, Store, parse_opt_ts, parse_ts, ts};

/// 판정 값.
pub mod verdict {
    /// 1단계(코드)나 2단계(분류)가 버렸다.
    pub const DROP: &str = "drop";
    /// 소식으로 모은다.
    pub const DIGEST: &str = "digest";
    /// 지금 Lucia에게 넘긴다.
    pub const NOW: &str = "now";
    /// 분류 전에 브리핑이 다뤘다.
    pub const BRIEFING: &str = "briefing";
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewSignal {
    pub source: String,
    pub key: String,
    pub summary: String,
    pub payload: Value,
    pub interest: Option<String>,
    /// 1단계에서 이미 정했으면(버림) 그 판정과 까닭.
    pub verdict: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Signal {
    pub id: i64,
    pub source: String,
    pub key: String,
    pub summary: String,
    pub payload: Value,
    pub interest: Option<String>,
    pub verdict: Option<String>,
    pub reason: Option<String>,
    pub received_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub woke_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "id, source, key, summary, payload, interest, verdict, reason, received_at, decided_at, woke_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawSignal> {
    Ok(RawSignal {
        id: r.get(0)?,
        source: r.get(1)?,
        key: r.get(2)?,
        summary: r.get(3)?,
        payload: r.get(4)?,
        interest: r.get(5)?,
        verdict: r.get(6)?,
        reason: r.get(7)?,
        received_at: r.get(8)?,
        decided_at: r.get(9)?,
        woke_at: r.get(10)?,
    })
}

struct RawSignal {
    id: i64,
    source: String,
    key: String,
    summary: String,
    payload: String,
    interest: Option<String>,
    verdict: Option<String>,
    reason: Option<String>,
    received_at: String,
    decided_at: Option<String>,
    woke_at: Option<String>,
}

impl RawSignal {
    fn into_signal(self) -> Result<Signal> {
        Ok(Signal {
            id: self.id,
            source: self.source,
            key: self.key,
            summary: self.summary,
            payload: serde_json::from_str(&self.payload)?,
            interest: self.interest,
            verdict: self.verdict,
            reason: self.reason,
            received_at: parse_ts(&self.received_at)?,
            decided_at: parse_opt_ts(self.decided_at)?,
            woke_at: parse_opt_ts(self.woke_at)?,
        })
    }
}

impl Store {
    /// 신호를 넣는다. 같은 (소스, 키)가 이미 있으면 넣지 않는다(`Ok(None)`).
    pub fn add_signal(&self, s: &NewSignal, now: DateTime<Utc>) -> Result<Option<i64>> {
        let (verdict, reason) = match &s.verdict {
            Some((v, r)) => (Some(v.as_str()), Some(r.as_str())),
            None => (None, None),
        };
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO signals
                (source, key, summary, payload, interest, verdict, reason, received_at, decided_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, CASE WHEN ?6 IS NULL THEN NULL ELSE ?8 END)",
            params![
                s.source,
                s.key,
                s.summary,
                serde_json::to_string(&s.payload)?,
                s.interest,
                verdict,
                reason,
                ts(now)
            ],
        )?;
        Ok((n > 0).then(|| self.conn.last_insert_rowid()))
    }

    fn query_signals(&self, filter: &str, args: impl rusqlite::Params) -> Result<Vec<Signal>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {COLUMNS} FROM signals WHERE {filter}"))?;
        let rows = stmt.query_map(args, row)?;
        rows.map(|r| r?.into_signal()).collect()
    }

    /// 2단계 분류를 기다리는 신호(오래된 것부터).
    pub fn pending_signals(&self, limit: usize) -> Result<Vec<Signal>> {
        self.query_signals(
            "verdict IS NULL ORDER BY id LIMIT ?1",
            [i64::try_from(limit).unwrap_or(i64::MAX)],
        )
    }

    /// 지금 알릴 것으로 정했지만 아직 Lucia에게 넘기지 않은 것(방해 금지로 미룬 것 포함).
    pub fn waiting_signal_wakes(&self) -> Result<Vec<Signal>> {
        self.query_signals("verdict = 'now' AND woke_at IS NULL ORDER BY id", [])
    }

    pub fn signal(&self, id: i64) -> Result<Option<Signal>> {
        Ok(self.query_signals("id = ?1", [id])?.into_iter().next())
    }

    /// 최근 신호(관리 페이지).
    pub fn recent_signals(&self, limit: usize) -> Result<Vec<Signal>> {
        self.query_signals(
            "1 ORDER BY id DESC LIMIT ?1",
            [i64::try_from(limit).unwrap_or(i64::MAX)],
        )
    }

    /// 판정을 적는다. 이미 정한 것은 바꾸지 않는다(`Ok(false)`).
    pub fn decide_signal(
        &self,
        id: i64,
        verdict: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE signals SET verdict = ?2, reason = ?3, decided_at = ?4
             WHERE id = ?1 AND verdict IS NULL",
            params![id, verdict, reason, ts(now)],
        )?;
        Ok(n > 0)
    }

    /// 지금 알릴 것을 Lucia에게 넘겼다.
    pub fn mark_signals_woke(&self, ids: &[i64], now: DateTime<Utc>) -> Result<()> {
        for id in ids {
            self.conn.execute(
                "UPDATE signals SET woke_at = ?2 WHERE id = ?1 AND woke_at IS NULL",
                params![id, ts(now)],
            )?;
        }
        Ok(())
    }

    /// 지금 알릴 것을 소식으로 돌린다(상한, 비용, 오래 미뤄짐).
    pub fn demote_signal(&self, id: i64, reason: &str, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "UPDATE signals SET verdict = 'digest', reason = ?2, decided_at = ?3
             WHERE id = ?1 AND verdict = 'now' AND woke_at IS NULL",
            params![id, reason, ts(now)],
        )?;
        Ok(())
    }

    /// 브리핑이 다룬 변화의 신호: 아직 분류 전이거나 넘기기 전이면 "브리핑"으로 닫는다(두 번 알리지 않게).
    pub fn settle_signals_by_briefing(
        &self,
        changes: &[(String, String)],
        now: DateTime<Utc>,
    ) -> Result<()> {
        for (source, key) in changes {
            self.conn.execute(
                "UPDATE signals SET verdict = 'briefing', reason = '브리핑에서 다룸', decided_at = ?3
                 WHERE source = ?1 AND key = ?2
                   AND (verdict IS NULL OR (verdict = 'now' AND woke_at IS NULL))",
                params![source, key, ts(now)],
            )?;
        }
        Ok(())
    }

    /// 이미 사용자에게 알린 변화(소식이 실제로 나갔거나 Lucia가 알린 것)의 (소스, 키). 방해 금지로 아직 쌓여
    /// 있는 소식은 알린 것이 아니다.
    pub fn notified_signal_keys(&self) -> Result<HashSet<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.source, s.key FROM signals s
             WHERE (s.verdict = 'now' AND s.woke_at IS NOT NULL)
                OR (s.verdict = 'digest' AND EXISTS (
                    SELECT 1 FROM notices n WHERE n.key = 'signal:' || s.id AND n.sent_at IS NOT NULL))",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 브리핑이 다룬 변화의 아직 나가지 않은 신호 소식을 거둔다(브리핑 뒤에 같은 것이 또 가지 않게).
    pub fn drop_unsent_signal_notices(&self, changes: &[(String, String)]) -> Result<usize> {
        let mut n = 0;
        for (source, key) in changes {
            n += self.conn.execute(
                "DELETE FROM notices WHERE sent_at IS NULL AND key IN (
                    SELECT 'signal:' || id FROM signals WHERE source = ?1 AND key = ?2)",
                params![source, key],
            )?;
        }
        Ok(n)
    }

    /// 판정별 개수(이 시각 이후 정한 것). 관리 페이지와 주간 보고에 쓴다.
    pub fn signal_counts_since(&self, since: DateTime<Utc>) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(verdict, 'pending'), count(*) FROM signals
             WHERE received_at >= ?1 GROUP BY 1 ORDER BY 1",
        )?;
        let rows = stmt.query_map([ts(since)], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn new(key: &str) -> NewSignal {
        NewSignal {
            source: "gmail".into(),
            key: key.into(),
            summary: format!("새 메일 {key}"),
            payload: json!({"id": key}),
            interest: None,
            verdict: None,
        }
    }

    #[test]
    fn signals_flow_once_and_settle() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        let a = store.add_signal(&new("msg:a"), now).unwrap().unwrap();
        assert!(
            store.add_signal(&new("msg:a"), now).unwrap().is_none(),
            "같은 변화는 한 번"
        );
        let b = store.add_signal(&new("msg:b"), now).unwrap().unwrap();
        let mut dropped = new("msg:c");
        dropped.verdict = Some((verdict::DROP.into(), "프로모션".into()));
        store.add_signal(&dropped, now).unwrap();
        assert_eq!(store.pending_signals(10).unwrap().len(), 2);

        assert!(
            store
                .decide_signal(a, verdict::NOW, "관심 발신자", now)
                .unwrap()
        );
        assert!(
            !store
                .decide_signal(a, verdict::DIGEST, "다시", now)
                .unwrap(),
            "한 번 정하면 그대로"
        );
        assert_eq!(store.waiting_signal_wakes().unwrap().len(), 1);

        // 브리핑이 먼저 다루면 넘기지 않는다.
        store
            .settle_signals_by_briefing(
                &[
                    ("gmail".into(), "msg:a".into()),
                    ("gmail".into(), "msg:b".into()),
                ],
                now,
            )
            .unwrap();
        assert!(store.waiting_signal_wakes().unwrap().is_empty());
        assert!(store.pending_signals(10).unwrap().is_empty());
        assert_eq!(
            store.signal(b).unwrap().unwrap().verdict.as_deref(),
            Some("briefing")
        );

        let d = store.add_signal(&new("msg:d"), now).unwrap().unwrap();
        store.decide_signal(d, verdict::NOW, "급함", now).unwrap();
        store.mark_signals_woke(&[d], now).unwrap();
        let notified = store.notified_signal_keys().unwrap();
        assert!(notified.contains(&("gmail".to_owned(), "msg:d".to_owned())));
        assert!(!notified.contains(&("gmail".to_owned(), "msg:a".to_owned())));
    }
}
