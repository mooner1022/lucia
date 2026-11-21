//! 알림 규칙과 단계적 독촉(결정 0034). 판단은 데몬이 하고, 여기는 저장만 한다.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension as _, params};

use crate::{Result, Store, parse_ts, ts};

/// 열린 독촉 수 [제안].
pub const MAX_OPEN_NUDGES: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NoticeRule {
    pub key: String,
    pub rule: String,
    pub label: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Nudge {
    pub id: String,
    pub title: String,
    pub due_at: DateTime<Utc>,
    pub note: String,
    pub goal: Option<String>,
    pub stage: i64,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

const NUDGE_COLUMNS: &str = "id, title, due_at, note, goal, stage, status, created_at";

type NudgeRaw = (
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
    String,
    String,
);

fn nudge_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<NudgeRaw> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
    ))
}

fn nudge(r: NudgeRaw) -> Result<Nudge> {
    Ok(Nudge {
        id: r.0,
        title: r.1,
        due_at: parse_ts(&r.2)?,
        note: r.3,
        goal: r.4,
        stage: r.5,
        status: r.6,
        created_at: parse_ts(&r.7)?,
    })
}

impl Store {
    pub fn set_notice_rule(
        &self,
        key: &str,
        rule: &str,
        label: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO notice_rules (key, rule, label, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (key) DO UPDATE SET rule = excluded.rule, label = excluded.label,
                created_at = excluded.created_at",
            params![key, rule, label, ts(now)],
        )?;
        Ok(())
    }

    pub fn delete_notice_rule(&self, key: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM notice_rules WHERE key = ?1", [key])?
            > 0)
    }

    pub fn notice_rule(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT rule FROM notice_rules WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn notice_rules(&self) -> Result<Vec<NoticeRule>> {
        let mut stmt = self.conn.prepare(
            "SELECT key, rule, label, created_at FROM notice_rules ORDER BY created_at DESC LIMIT 200",
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
            let (key, rule, label, at) = r?;
            Ok(NoticeRule {
                key,
                rule,
                label,
                created_at: parse_ts(&at)?,
            })
        })
        .collect()
    }

    /// 독촉을 만든다. 열린 것이 상한이면 `Ok(Err(까닭))`.
    #[allow(clippy::too_many_arguments)]
    pub fn create_nudge(
        &self,
        id: &str,
        title: &str,
        due_at: DateTime<Utc>,
        note: &str,
        goal: Option<&str>,
        run: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<std::result::Result<(), String>> {
        let open: i64 = self.conn.query_row(
            "SELECT count(*) FROM nudges WHERE status = 'open'",
            [],
            |r| r.get(0),
        )?;
        if open as usize >= MAX_OPEN_NUDGES {
            return Ok(Err(format!(
                "열린 마감이 {MAX_OPEN_NUDGES}개라 더 만들지 않았다"
            )));
        }
        self.conn.execute(
            "INSERT INTO nudges (id, title, due_at, note, goal, stage, status, run_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, 'open', ?6, ?7, ?7)",
            params![id, title, ts(due_at), note, goal, run, ts(now)],
        )?;
        Ok(Ok(()))
    }

    pub fn nudge(&self, id: &str) -> Result<Option<Nudge>> {
        self.conn
            .query_row(
                &format!("SELECT {NUDGE_COLUMNS} FROM nudges WHERE id = ?1"),
                [id],
                nudge_row,
            )
            .optional()?
            .map(nudge)
            .transpose()
    }

    /// 열린 독촉(마감 순).
    pub fn open_nudges(&self) -> Result<Vec<Nudge>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {NUDGE_COLUMNS} FROM nudges WHERE status = 'open' ORDER BY due_at"
        ))?;
        let rows = stmt.query_map([], nudge_row)?;
        rows.map(|r| nudge(r?)).collect()
    }

    /// 단계를 올린다(더 낮게는 내리지 않는다).
    pub fn raise_nudge(&self, id: &str, stage: i64, now: DateTime<Utc>) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE nudges SET stage = ?2, updated_at = ?3 WHERE id = ?1 AND status = 'open' AND stage < ?2",
            params![id, stage, ts(now)],
        )? > 0)
    }

    /// 닫는다(done | expired | dropped). 열린 것만.
    pub fn close_nudge(&self, id: &str, status: &str, now: DateTime<Utc>) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE nudges SET status = ?2, updated_at = ?3 WHERE id = ?1 AND status = 'open'",
            params![id, status, ts(now)],
        )? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_and_nudges() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        store
            .set_notice_rule("news@x.com", "mute", "뉴스", now)
            .unwrap();
        store
            .set_notice_rule("news@x.com", "briefing", "뉴스", now)
            .unwrap();
        assert_eq!(
            store.notice_rule("news@x.com").unwrap().as_deref(),
            Some("briefing")
        );
        assert_eq!(store.notice_rules().unwrap().len(), 1);
        assert!(store.delete_notice_rule("news@x.com").unwrap());

        let due = now + chrono::Duration::days(2);
        store
            .create_nudge("n1", "과제", due, "", None, None, now)
            .unwrap()
            .unwrap();
        assert!(store.raise_nudge("n1", 2, now).unwrap());
        assert!(
            !store.raise_nudge("n1", 1, now).unwrap(),
            "단계는 내려가지 않는다"
        );
        assert_eq!(store.open_nudges().unwrap()[0].stage, 2);
        assert!(store.close_nudge("n1", "done", now).unwrap());
        assert!(store.open_nudges().unwrap().is_empty());
    }
}
