//! Claude Code 세션 기록. 상주 세션은 역할별로 활성인 것이 하나다.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

use crate::{Result, Store, StoreError, parse_opt_ts, parse_ts, ts};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SessionRole {
    Resident,
    Task,
    Routine,
}

impl SessionRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Resident => "resident",
            Self::Task => "task",
            Self::Routine => "routine",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "resident" => Self::Resident,
            "task" => Self::Task,
            "routine" => Self::Routine,
            other => return Err(StoreError::Corrupt(format!("session role {other:?}"))),
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SessionRow {
    pub id: String,
    pub role: SessionRole,
    pub sandbox: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub last_context_tokens: u64,
    /// 첫 요청을 이미 보냈는가(`--session-id` 대신 `--resume`을 써야 하는가).
    pub started: bool,
    /// claude가 마지막으로 알려 준 세션 누적 비용(실행 비용은 이것과의 차이).
    pub cost_total_usd: f64,
    pub last_compacted_at: Option<DateTime<Utc>>,
    /// 마지막 compact(또는 시작) 뒤 모델 실행 수.
    pub runs_since_compact: u64,
}

impl Store {
    /// 역할별 활성 세션.
    pub fn active_session(&self, role: SessionRole) -> Result<Option<SessionRow>> {
        self.conn
            .query_row(
                "SELECT id, role, sandbox, status, created_at, last_used_at, last_context_tokens,
                        cost_total_usd, last_compacted_at, runs_since_compact
                 FROM sessions WHERE role = ?1 AND status = 'active'
                 ORDER BY created_at DESC LIMIT 1",
                [role.as_str()],
                |r| {
                    Ok((
                        (
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                        ),
                        (
                            r.get::<_, Option<String>>(5)?,
                            r.get::<_, i64>(6)?,
                            r.get::<_, f64>(7)?,
                            r.get::<_, Option<String>>(8)?,
                            r.get::<_, i64>(9)?,
                        ),
                    ))
                },
            )
            .optional()?
            .map(
                |(
                    (id, role, sandbox, status, created),
                    (last_used, tokens, cost_total, compacted, runs),
                )| {
                    let last_used_at = parse_opt_ts(last_used)?;
                    Ok(SessionRow {
                        id,
                        role: SessionRole::parse(&role)?,
                        sandbox,
                        status,
                        created_at: parse_ts(&created)?,
                        started: last_used_at.is_some(),
                        last_used_at,
                        last_context_tokens: tokens.max(0) as u64,
                        cost_total_usd: cost_total,
                        last_compacted_at: parse_opt_ts(compacted)?,
                        runs_since_compact: runs.max(0) as u64,
                    })
                },
            )
            .transpose()
    }

    /// 새 세션을 활성으로 만든다. 같은 역할의 이전 활성 세션은 `rotated`가 된다.
    pub fn start_session(
        &mut self,
        id: &str,
        role: SessionRole,
        sandbox: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE sessions SET status = 'rotated', rotated_at = ?2
             WHERE role = ?1 AND status = 'active'",
            params![role.as_str(), ts(now)],
        )?;
        tx.execute(
            "INSERT INTO sessions (id, role, sandbox, status, created_at)
             VALUES (?1, ?2, ?3, 'active', ?4)",
            params![id, role.as_str(), sandbox, ts(now)],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 세션을 닫는다. 첫 요청이 실패해 실제로 만들어졌는지 알 수 없는 세션은 다시 쓰지 않는다
    /// (같은 `--session-id`로 다시 열면 CLI가 어떻게 할지 보장되지 않는다).
    pub fn close_session(&self, id: &str, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET status = 'closed', rotated_at = ?2 WHERE id = ?1",
            params![id, ts(now)],
        )?;
        Ok(())
    }

    /// 요청을 보낸 뒤 사용 시각과 컨텍스트 크기를 남긴다.
    pub fn touch_session(&self, id: &str, now: DateTime<Utc>, context_tokens: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET last_used_at = ?2, last_context_tokens = ?3,
                 runs_since_compact = runs_since_compact + 1 WHERE id = ?1",
            params![id, ts(now), context_tokens as i64],
        )?;
        Ok(())
    }

    /// claude가 준 세션 누적 비용을 받아 이번 실행의 비용(차이)을 돌려주고 누적값을 기억한다.
    /// 누적값이 줄었으면(세션이 바뀐 경우 등) 그 값 전체를 이번 비용으로 본다.
    pub fn session_cost_delta(&self, id: &str, cumulative: f64) -> Result<f64> {
        let previous: f64 = self
            .conn
            .query_row(
                "SELECT cost_total_usd FROM sessions WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0.0);
        if cumulative <= 0.0 {
            return Ok(0.0);
        }
        self.conn.execute(
            "UPDATE sessions SET cost_total_usd = ?2 WHERE id = ?1",
            params![id, cumulative],
        )?;
        Ok(if cumulative >= previous {
            cumulative - previous
        } else {
            cumulative
        })
    }

    /// compact를 마쳤다: 컨텍스트 크기를 줄이고 실행 수를 0으로.
    pub fn mark_compacted(&self, id: &str, now: DateTime<Utc>, context_tokens: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET last_compacted_at = ?2, last_context_tokens = ?3,
                 runs_since_compact = 0 WHERE id = ?1",
            params![id, ts(now), context_tokens as i64],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    #[test]
    fn start_touch_and_rotate() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(
            store
                .active_session(SessionRole::Resident)
                .unwrap()
                .is_none()
        );

        store
            .start_session("s1", SessionRole::Resident, Some("lucia"), at(9, 0))
            .unwrap();
        let s1 = store
            .active_session(SessionRole::Resident)
            .unwrap()
            .unwrap();
        assert_eq!(s1.id, "s1");
        assert!(!s1.started, "아직 요청 전이면 --session-id로 연다");

        store.touch_session("s1", at(9, 5), 4200).unwrap();
        let s1 = store
            .active_session(SessionRole::Resident)
            .unwrap()
            .unwrap();
        assert!(s1.started);
        assert_eq!(s1.last_context_tokens, 4200);

        store
            .start_session("s2", SessionRole::Resident, Some("lucia"), at(10, 0))
            .unwrap();
        assert_eq!(
            store
                .active_session(SessionRole::Resident)
                .unwrap()
                .unwrap()
                .id,
            "s2"
        );
    }

    #[test]
    fn cost_delta_and_compaction_bookkeeping() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .start_session("s1", SessionRole::Resident, None, at(9, 0))
            .unwrap();
        assert!((store.session_cost_delta("s1", 0.010).unwrap() - 0.010).abs() < 1e-9);
        assert!((store.session_cost_delta("s1", 0.025).unwrap() - 0.015).abs() < 1e-9);
        assert_eq!(
            store.session_cost_delta("s1", 0.0).unwrap(),
            0.0,
            "비용 없는 실행"
        );
        store.touch_session("s1", at(9, 1), 50_000).unwrap();
        store.touch_session("s1", at(9, 2), 60_000).unwrap();
        let s = store
            .active_session(SessionRole::Resident)
            .unwrap()
            .unwrap();
        assert_eq!(s.runs_since_compact, 2);
        assert!((s.cost_total_usd - 0.025).abs() < 1e-9);
        store.mark_compacted("s1", at(9, 3), 4_000).unwrap();
        let s = store
            .active_session(SessionRole::Resident)
            .unwrap()
            .unwrap();
        assert_eq!((s.runs_since_compact, s.last_context_tokens), (0, 4_000));
        assert_eq!(s.last_compacted_at, Some(at(9, 3)));
    }
}
