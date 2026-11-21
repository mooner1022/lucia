//! 작업 레코드(결정 0014). 상태 전이는 [`TaskStatus::can_move_to`]를 지키는 조건부 갱신이고, 모두 기록한다.

use chrono::{DateTime, Utc};
use lucia_core::{Initiator, TaskStatus};
use rusqlite::{OptionalExtension as _, params};

use crate::{Result, Store, StoreError, parse_opt_ts, parse_ts, ts};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Task {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub title: String,
    pub brief: String,
    pub status: TaskStatus,
    pub initiator: Initiator,
    pub source: String,
    pub conversation: Option<String>,
    /// `conversation`이 이 작업만의 스레드다(스레드 안의 말은 이 작업으로 간다).
    pub thread: bool,
    pub session_id: Option<String>,
    pub run_id: Option<String>,
    pub result: Option<String>,
    pub closed_at: Option<DateTime<Utc>>,
    /// general | dev(결정 0020)
    pub kind: String,
    pub repo: Option<String>,
    /// 임시 샌드박스(개발 작업). 정리하면 비운다.
    pub sandbox: Option<String>,
    pub base: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewTask<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub brief: &'a str,
    pub initiator: Initiator,
    pub source: &'a str,
    pub run_id: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskEvent {
    pub at: DateTime<Utc>,
    pub from: Option<TaskStatus>,
    pub to: TaskStatus,
    pub note: String,
}

const COLUMNS: &str = "id, created_at, updated_at, title, brief, status, initiator, source, conversation, thread, session_id, run_id, result, closed_at, kind, repo, sandbox, base";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Task>> {
    let status: String = r.get(5)?;
    let created: String = r.get(1)?;
    let updated: String = r.get(2)?;
    let closed: Option<String> = r.get(13)?;
    let initiator: String = r.get(6)?;
    let thread: i64 = r.get(9)?;
    let id: String = r.get(0)?;
    let title: String = r.get(3)?;
    let brief: String = r.get(4)?;
    let source: String = r.get(7)?;
    let conversation: Option<String> = r.get(8)?;
    let session_id: Option<String> = r.get(10)?;
    let run_id: Option<String> = r.get(11)?;
    let result: Option<String> = r.get(12)?;
    let kind: String = r.get(14)?;
    let repo: Option<String> = r.get(15)?;
    let sandbox: Option<String> = r.get(16)?;
    let base: Option<String> = r.get(17)?;
    Ok((|| {
        Ok(Task {
            status: TaskStatus::parse(&status)
                .ok_or_else(|| StoreError::Corrupt(format!("task status {status:?}")))?,
            created_at: parse_ts(&created)?,
            updated_at: parse_ts(&updated)?,
            closed_at: parse_opt_ts(closed)?,
            initiator: Initiator::parse(&initiator),
            thread: thread != 0,
            id,
            title,
            brief,
            source,
            conversation,
            session_id,
            run_id,
            result,
            kind,
            repo,
            sandbox,
            base,
        })
    })())
}

impl Store {
    /// 작업을 만든다(`created`).
    pub fn create_task(&self, t: &NewTask<'_>, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tasks (id, created_at, updated_at, title, brief, status, initiator, source, run_id)
             VALUES (?1, ?2, ?2, ?3, ?4, 'created', ?5, ?6, ?7)",
            params![
                t.id,
                ts(now),
                t.title,
                t.brief,
                t.initiator.as_str(),
                t.source,
                t.run_id
            ],
        )?;
        self.conn.execute(
            "INSERT INTO task_events (task_id, at, from_status, to_status, note) VALUES (?1, ?2, NULL, 'created', '만듦')",
            params![t.id, ts(now)],
        )?;
        Ok(())
    }

    pub fn task(&self, id: &str) -> Result<Option<Task>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM tasks WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()?
            .transpose()
    }

    /// 끝나지 않은 작업(오래된 것부터).
    pub fn open_tasks(&self) -> Result<Vec<Task>> {
        self.tasks_where(
            "status NOT IN ('done', 'failed', 'cancelled', 'expired') ORDER BY created_at, id",
            [],
        )
    }

    /// 최근에 끝난 작업(새것부터).
    pub fn closed_tasks(&self, limit: usize) -> Result<Vec<Task>> {
        self.tasks_where(
            "status IN ('done', 'failed', 'cancelled', 'expired') ORDER BY closed_at DESC, id LIMIT ?1",
            [limit as i64],
        )
    }

    /// 이 대화(작업 스레드)를 쓰는 끝나지 않은 작업.
    pub fn open_task_in_thread(&self, conversation: &str) -> Result<Option<Task>> {
        Ok(self
            .tasks_where(
                "thread = 1 AND conversation = ?1 AND status NOT IN ('done', 'failed', 'cancelled', 'expired') ORDER BY created_at DESC LIMIT 1",
                [conversation],
            )?
            .into_iter()
            .next())
    }

    /// 작업 스레드였던 대화인가(끝난 작업 포함, 결정 0024: 그 스레드의 말은 봇을 부르지 않아도 받는다).
    pub fn is_task_thread(&self, conversation: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE thread = 1 AND conversation = ?1)",
            [conversation],
            |r| r.get(0),
        )?)
    }

    fn tasks_where<P: rusqlite::Params>(&self, clause: &str, p: P) -> Result<Vec<Task>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {COLUMNS} FROM tasks WHERE {clause}"))?;
        let rows = stmt.query_map(p, from_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    /// 상태를 바꾼다. 허용되지 않은 전이거나 그새 상태가 바뀌었으면 `Ok(false)`(아무것도 하지 않는다).
    pub fn move_task(
        &self,
        id: &str,
        to: TaskStatus,
        note: &str,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let Some(task) = self.task(id)? else {
            return Ok(false);
        };
        if !task.status.can_move_to(to) {
            return Ok(false);
        }
        let closed = to.is_final().then(|| ts(now));
        let n = self.conn.execute(
            "UPDATE tasks SET status = ?3, updated_at = ?4, closed_at = COALESCE(?5, closed_at)
             WHERE id = ?1 AND status = ?2",
            params![id, task.status.as_str(), to.as_str(), ts(now), closed],
        )?;
        if n == 1 {
            self.conn.execute(
                "INSERT INTO task_events (task_id, at, from_status, to_status, note) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, ts(now), task.status.as_str(), to.as_str(), note],
            )?;
        }
        Ok(n == 1)
    }

    /// 진행을 보낼 대화와, 그것이 이 작업만의 스레드인지.
    pub fn set_task_conversation(&self, id: &str, conversation: &str, thread: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET conversation = ?2, thread = ?3 WHERE id = ?1",
            params![id, conversation, thread as i64],
        )?;
        Ok(())
    }

    /// 작업 세션을 정한다. 세션 기록에도 `task` 역할로 남긴다(다른 작업 세션을 교대시키지 않는다).
    pub fn set_task_session(&self, id: &str, session_id: &str, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET session_id = ?2 WHERE id = ?1",
            params![id, session_id],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO sessions (id, role, sandbox, status, created_at) VALUES (?1, 'task', NULL, 'active', ?2)",
            params![session_id, ts(now)],
        )?;
        Ok(())
    }

    pub fn set_task_result(&self, id: &str, result: &str, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET result = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, result, ts(now)],
        )?;
        Ok(())
    }

    /// 진행 메모 등 상태가 바뀌지 않는 기록.
    pub fn note_task(&self, id: &str, note: &str, now: DateTime<Utc>) -> Result<()> {
        let Some(task) = self.task(id)? else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT INTO task_events (task_id, at, from_status, to_status, note) VALUES (?1, ?2, ?3, ?3, ?4)",
            params![id, ts(now), task.status.as_str(), note],
        )?;
        self.conn.execute(
            "UPDATE tasks SET updated_at = ?2 WHERE id = ?1",
            params![id, ts(now)],
        )?;
        Ok(())
    }

    pub fn task_events(&self, id: &str) -> Result<Vec<TaskEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT at, from_status, to_status, note FROM task_events WHERE task_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (at, from, to, note) = row?;
            let status = |s: &str| {
                TaskStatus::parse(s)
                    .ok_or_else(|| StoreError::Corrupt(format!("task status {s:?}")))
            };
            out.push(TaskEvent {
                at: parse_ts(&at)?,
                from: from.as_deref().map(status).transpose()?,
                to: status(&to)?,
                note,
            });
        }
        Ok(out)
    }

    /// 오래 멈춰 있는 작업(답 기다림, 확인 기다림, 승인 대기): `before`보다 오래 바뀌지 않은 것.
    pub fn stale_tasks(&self, before: DateTime<Utc>) -> Result<Vec<Task>> {
        self.tasks_where(
            "status IN ('waiting_approval', 'waiting_input', 'reported') AND updated_at < ?1 ORDER BY updated_at",
            [ts(before)],
        )
    }

    /// 개발 작업으로 표시한다(결정 0020).
    pub fn set_task_dev(&self, id: &str, repo: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET kind = 'dev', repo = ?2 WHERE id = ?1",
            params![id, repo],
        )?;
        Ok(())
    }

    /// 임시 샌드박스와 시작 커밋. `None`이면 정리했다.
    pub fn set_task_sandbox(
        &self,
        id: &str,
        sandbox: Option<&str>,
        base: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET sandbox = ?2, base = COALESCE(?3, base) WHERE id = ?1",
            params![id, sandbox, base],
        )?;
        Ok(())
    }

    /// 끝났는데 임시 샌드박스가 남은 개발 작업(정리할 것).
    pub fn finished_tasks_with_sandbox(&self) -> Result<Vec<Task>> {
        self.tasks_where(
            "sandbox IS NOT NULL AND status IN ('done', 'failed', 'cancelled', 'expired') ORDER BY closed_at",
            [],
        )
    }

    /// 실행 도중 데몬이 멈춘 작업(`running`·`created`).
    pub fn interrupted_tasks(&self) -> Result<Vec<Task>> {
        self.tasks_where("status IN ('created', 'running') ORDER BY created_at", [])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    fn store_with_task() -> Store {
        let store = Store::open_in_memory().unwrap();
        store
            .create_task(
                &NewTask {
                    id: "task-1",
                    title: "노트북 가격 비교",
                    brief: "세 쇼핑몰 최저가",
                    initiator: Initiator::Human,
                    source: "chat",
                    run_id: Some("chat-1"),
                },
                at(9, 0),
            )
            .unwrap();
        store
    }

    #[test]
    fn transitions_are_checked_and_recorded() {
        let store = store_with_task();
        assert!(
            !store
                .move_task("task-1", TaskStatus::Done, "건너뛰기", at(9, 1))
                .unwrap()
        );
        assert!(
            store
                .move_task("task-1", TaskStatus::Running, "시작", at(9, 1))
                .unwrap()
        );
        assert!(
            store
                .move_task("task-1", TaskStatus::Reported, "보고", at(9, 5))
                .unwrap()
        );
        assert!(
            store
                .move_task("task-1", TaskStatus::Done, "사람이 확인", at(9, 6))
                .unwrap()
        );
        assert!(
            !store
                .move_task("task-1", TaskStatus::Running, "다시", at(9, 7))
                .unwrap(),
            "끝난 작업은 움직이지 않는다"
        );
        let t = store.task("task-1").unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Done);
        assert_eq!(t.closed_at, Some(at(9, 6)));
        let events = store.task_events("task-1").unwrap();
        assert_eq!(
            events.iter().map(|e| e.to).collect::<Vec<_>>(),
            [
                TaskStatus::Created,
                TaskStatus::Running,
                TaskStatus::Reported,
                TaskStatus::Done
            ]
        );
        assert!(store.open_tasks().unwrap().is_empty());
        assert_eq!(store.closed_tasks(5).unwrap().len(), 1);
    }

    #[test]
    fn threads_route_to_their_open_task() {
        let store = store_with_task();
        store
            .set_task_conversation("task-1", "discord:77", true)
            .unwrap();
        store
            .set_task_session("task-1", "sess-1", at(9, 0))
            .unwrap();
        assert_eq!(
            store.open_task_in_thread("discord:77").unwrap().unwrap().id,
            "task-1"
        );
        assert!(store.open_task_in_thread("discord:1").unwrap().is_none());
        store
            .move_task("task-1", TaskStatus::Cancelled, "취소", at(9, 2))
            .unwrap();
        assert!(store.open_task_in_thread("discord:77").unwrap().is_none());
        assert!(
            store.is_task_thread("discord:77").unwrap(),
            "끝나도 작업 스레드다"
        );
        assert!(!store.is_task_thread("discord:1").unwrap());
        assert_eq!(
            store.task("task-1").unwrap().unwrap().session_id.as_deref(),
            Some("sess-1")
        );
    }
}
