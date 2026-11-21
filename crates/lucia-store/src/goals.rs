//! 목표(결정 0032). 판단은 데몬(`luciad::goals`)이 하고, 여기는 저장만 한다.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension as _, params};

use crate::{Result, Store, StoreError, parse_opt_ts, parse_ts, ts};

/// 열린 목표 수 [제안].
pub const MAX_OPEN_GOALS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum GoalStatus {
    Active,
    /// 사람 차례(위임·결정·자료가 필요하다).
    Waiting,
    Done,
    Abandoned,
}

impl GoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Abandoned => "abandoned",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "active" => Self::Active,
            "waiting" => Self::Waiting,
            "done" => Self::Done,
            "abandoned" => Self::Abandoned,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "진행 중",
            Self::Waiting => "사람 차례",
            Self::Done => "완료",
            Self::Abandoned => "포기",
        }
    }

    pub fn open(self) -> bool {
        matches!(self, Self::Active | Self::Waiting)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Goal {
    pub id: String,
    pub title: String,
    pub purpose: String,
    pub success: String,
    pub deadline: Option<DateTime<Utc>>,
    pub status: GoalStatus,
    pub interests: String,
    pub review_every_hours: i64,
    pub next_review_at: Option<DateTime<Utc>>,
    pub conversation: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewGoal<'a> {
    pub title: &'a str,
    pub purpose: &'a str,
    pub success: &'a str,
    pub deadline: Option<DateTime<Utc>>,
    pub interests: &'a str,
    pub review_every_hours: i64,
    pub next_review_at: DateTime<Utc>,
    pub conversation: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GoalLog {
    pub at: DateTime<Utc>,
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GoalChild {
    pub goal: String,
    pub kind: String,
    pub child: String,
    pub by_review: bool,
}

const COLUMNS: &str = "id, title, purpose, success, deadline, status, interests, review_every_hours, \
     next_review_at, conversation, created_at, updated_at, closed_at";

type Raw = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    String,
    String,
    Option<String>,
);

fn raw(r: &rusqlite::Row<'_>) -> rusqlite::Result<Raw> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
    ))
}

fn goal(r: Raw) -> Result<Goal> {
    Ok(Goal {
        id: r.0,
        title: r.1,
        purpose: r.2,
        success: r.3,
        deadline: parse_opt_ts(r.4)?,
        status: GoalStatus::parse(&r.5)
            .ok_or_else(|| StoreError::Corrupt(format!("목표 상태: {}", r.5)))?,
        interests: r.6,
        review_every_hours: r.7,
        next_review_at: parse_opt_ts(r.8)?,
        conversation: r.9,
        created_at: parse_ts(&r.10)?,
        updated_at: parse_ts(&r.11)?,
        closed_at: parse_opt_ts(r.12)?,
    })
}

impl Store {
    /// 목표를 만든다. 열린 목표가 상한이면 `Ok(Err(까닭))`.
    pub fn create_goal(
        &self,
        id: &str,
        g: &NewGoal<'_>,
        now: DateTime<Utc>,
    ) -> Result<std::result::Result<(), String>> {
        let open: i64 = self.conn.query_row(
            "SELECT count(*) FROM goals WHERE status IN ('active', 'waiting')",
            [],
            |r| r.get(0),
        )?;
        if open as usize >= MAX_OPEN_GOALS {
            return Ok(Err(format!(
                "열린 목표가 {MAX_OPEN_GOALS}개라 더 만들지 않았다. 끝난 목표를 닫은 뒤 다시 만든다."
            )));
        }
        self.conn.execute(
            &format!(
                "INSERT INTO goals ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7, ?8, ?9, ?10, ?10, NULL)"
            ),
            params![
                id,
                g.title,
                g.purpose,
                g.success,
                g.deadline.map(ts),
                g.interests,
                g.review_every_hours,
                ts(g.next_review_at),
                g.conversation,
                ts(now)
            ],
        )?;
        self.add_goal_log(id, "status", "목표를 만들었다", now)?;
        Ok(Ok(()))
    }

    pub fn goal(&self, id: &str) -> Result<Option<Goal>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM goals WHERE id = ?1"),
                [id],
                raw,
            )
            .optional()?
            .map(goal)
            .transpose()
    }

    /// 목표 목록(열린 것 먼저, 최근 것부터).
    pub fn goals(&self, include_closed: bool) -> Result<Vec<Goal>> {
        let filter = if include_closed {
            ""
        } else {
            "WHERE status IN ('active', 'waiting')"
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM goals {filter}
             ORDER BY status IN ('active', 'waiting') DESC, created_at DESC LIMIT 50"
        ))?;
        let rows = stmt.query_map([], raw)?;
        rows.map(|r| goal(r?)).collect()
    }

    /// 칸을 고친다(주어진 것만). 닫힌 목표는 고치지 않는다(`Ok(false)`).
    #[allow(clippy::too_many_arguments)]
    pub fn update_goal(
        &self,
        id: &str,
        title: Option<&str>,
        success: Option<&str>,
        deadline: Option<Option<DateTime<Utc>>>,
        interests: Option<&str>,
        review_every_hours: Option<i64>,
        next_review_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE goals SET
                title = COALESCE(?2, title),
                success = COALESCE(?3, success),
                deadline = CASE WHEN ?4 THEN ?5 ELSE deadline END,
                interests = COALESCE(?6, interests),
                review_every_hours = COALESCE(?7, review_every_hours),
                next_review_at = COALESCE(?8, next_review_at),
                updated_at = ?9
             WHERE id = ?1 AND status IN ('active', 'waiting')",
            params![
                id,
                title,
                success,
                deadline.is_some(),
                deadline.flatten().map(ts),
                interests,
                review_every_hours,
                next_review_at.map(ts),
                ts(now)
            ],
        )?;
        Ok(n > 0)
    }

    /// 상태를 바꾼다. 닫힌 목표는 다시 열지 않는다(`Ok(false)`).
    pub fn set_goal_status(
        &self,
        id: &str,
        status: GoalStatus,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let closed = (!status.open()).then(|| ts(now));
        let n = self.conn.execute(
            "UPDATE goals SET status = ?2, updated_at = ?3, closed_at = ?4
             WHERE id = ?1 AND status IN ('active', 'waiting')",
            params![id, status.as_str(), ts(now), closed],
        )?;
        Ok(n > 0)
    }

    pub fn add_goal_log(
        &self,
        goal: &str,
        kind: &str,
        text: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO goal_log (goal, at, kind, text) VALUES (?1, ?2, ?3, ?4)",
            params![goal, ts(now), kind, text],
        )?;
        Ok(())
    }

    /// 진행 기록(최근 것부터).
    pub fn goal_log(&self, goal: &str, limit: usize) -> Result<Vec<GoalLog>> {
        let mut stmt = self.conn.prepare(
            "SELECT at, kind, text FROM goal_log WHERE goal = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![goal, limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|r| {
            let (at, kind, text) = r?;
            Ok(GoalLog {
                at: parse_ts(&at)?,
                kind,
                text,
            })
        })
        .collect()
    }

    /// 이 시각 뒤의 점검 수(하루 상한).
    pub fn goal_reviews_since(&self, goal: &str, since: DateTime<Utc>) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM goal_log WHERE goal = ?1 AND kind = 'review' AND at >= ?2",
            params![goal, ts(since)],
            |r| r.get(0),
        )?)
    }

    /// 하위 항목을 잇는다(이미 다른 목표에 이어져 있으면 그대로 둔다: `Ok(false)`).
    pub fn link_goal_child(
        &self,
        goal: &str,
        kind: &str,
        child: &str,
        by_review: bool,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO goal_children (goal, kind, child, by_review, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![goal, kind, child, by_review, ts(now)],
        )?;
        Ok(n > 0)
    }

    pub fn unlink_goal_child(&self, goal: &str, kind: &str, child: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM goal_children WHERE goal = ?1 AND kind = ?2 AND child = ?3",
            params![goal, kind, child],
        )? > 0)
    }

    pub fn goal_children(&self, goal: &str) -> Result<Vec<GoalChild>> {
        let mut stmt = self.conn.prepare(
            "SELECT goal, kind, child, by_review FROM goal_children WHERE goal = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map([goal], |r| {
            Ok(GoalChild {
                goal: r.get(0)?,
                kind: r.get(1)?,
                child: r.get(2)?,
                by_review: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 이 항목이 어느 목표의 하위 항목인가.
    pub fn goal_parent(&self, kind: &str, child: &str) -> Result<Option<GoalChild>> {
        Ok(self
            .conn
            .query_row(
                "SELECT goal, kind, child, by_review FROM goal_children WHERE kind = ?1 AND child = ?2",
                params![kind, child],
                |r| {
                    Ok(GoalChild {
                        goal: r.get(0)?,
                        kind: r.get(1)?,
                        child: r.get(2)?,
                        by_review: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_goal(next: DateTime<Utc>) -> NewGoal<'static> {
        NewGoal {
            title: "12월 원서 접수",
            purpose: "원서를 기한 안에 낸다",
            success: "접수 완료 메일",
            deadline: None,
            interests: "입학처",
            review_every_hours: 24,
            next_review_at: next,
            conversation: Some("dm"),
        }
    }

    #[test]
    fn goals_open_close_and_children() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        store
            .create_goal("goal-a", &new_goal(now), now)
            .unwrap()
            .unwrap();
        let g = store.goal("goal-a").unwrap().unwrap();
        assert_eq!(g.status, GoalStatus::Active);
        assert_eq!(store.goals(false).unwrap().len(), 1);

        assert!(
            store
                .link_goal_child("goal-a", "routine", "routine-1", true, now)
                .unwrap()
        );
        assert!(
            !store
                .link_goal_child("goal-b", "routine", "routine-1", false, now)
                .unwrap(),
            "한 목표에만"
        );
        let p = store.goal_parent("routine", "routine-1").unwrap().unwrap();
        assert_eq!(p.goal, "goal-a");
        assert!(p.by_review);

        assert!(
            store
                .update_goal(
                    "goal-a",
                    None,
                    Some("합격 발표"),
                    Some(None),
                    None,
                    Some(12),
                    None,
                    now
                )
                .unwrap()
        );
        assert_eq!(
            store.goal("goal-a").unwrap().unwrap().review_every_hours,
            12
        );

        store.add_goal_log("goal-a", "review", "점검", now).unwrap();
        assert_eq!(
            store
                .goal_reviews_since("goal-a", now - chrono::Duration::hours(1))
                .unwrap(),
            1
        );

        assert!(
            store
                .set_goal_status("goal-a", GoalStatus::Abandoned, now)
                .unwrap()
        );
        assert!(
            !store
                .set_goal_status("goal-a", GoalStatus::Active, now)
                .unwrap(),
            "닫힌 목표는 다시 열지 않는다"
        );
        assert!(store.goals(false).unwrap().is_empty());
        assert!(
            !store
                .update_goal("goal-a", Some("x"), None, None, None, None, None, now)
                .unwrap()
        );
    }

    #[test]
    fn open_goals_are_capped() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        for i in 0..MAX_OPEN_GOALS {
            store
                .create_goal(&format!("g{i}"), &new_goal(now), now)
                .unwrap()
                .unwrap();
        }
        assert!(
            store
                .create_goal("over", &new_goal(now), now)
                .unwrap()
                .is_err()
        );
    }
}
