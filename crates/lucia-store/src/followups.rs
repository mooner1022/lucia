//! 후속 조치(결정 0010). 발사는 같은 ID의 일회성 스케줄이 맡고, 여기에는 약속의 내용과 상태를 둔다.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension as _, params};

use crate::{Result, Store, StoreError, parse_opt_ts, parse_ts, ts};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum FollowupStatus {
    Open,
    Done,
    Cancelled,
    Failed,
}

impl FollowupStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "open" => Self::Open,
            "done" => Self::Done,
            "cancelled" => Self::Cancelled,
            "failed" => Self::Failed,
            other => return Err(StoreError::Corrupt(format!("followup status {other:?}"))),
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "예정",
            Self::Done => "함",
            Self::Cancelled => "취소",
            Self::Failed => "실패",
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Followup {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub due_at: DateTime<Utc>,
    pub task: String,
    pub detail: String,
    pub conversation: Option<String>,
    pub run_id: Option<String>,
    pub origin: String,
    pub status: FollowupStatus,
    pub closed_at: Option<DateTime<Utc>>,
    pub result: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewFollowup<'a> {
    pub id: &'a str,
    pub due_at: DateTime<Utc>,
    pub task: &'a str,
    pub detail: &'a str,
    pub conversation: Option<&'a str>,
    pub run_id: Option<&'a str>,
    pub origin: &'a str,
}

const COLUMNS: &str =
    "id, created_at, due_at, task, detail, conversation, run_id, origin, status, closed_at, result";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<[Option<String>; 11]> {
    let mut out: [Option<String>; 11] = Default::default();
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = r.get(i)?;
    }
    Ok(out)
}

fn from_row(c: [Option<String>; 11]) -> Result<Followup> {
    let [
        id,
        created_at,
        due_at,
        task,
        detail,
        conversation,
        run_id,
        origin,
        status,
        closed_at,
        result,
    ] = c;
    let need = |v: Option<String>| v.ok_or_else(|| StoreError::Corrupt("followup 빈 칸".into()));
    Ok(Followup {
        id: need(id)?,
        created_at: parse_ts(&need(created_at)?)?,
        due_at: parse_ts(&need(due_at)?)?,
        task: need(task)?,
        detail: need(detail)?,
        conversation,
        run_id,
        origin: need(origin)?,
        status: FollowupStatus::parse(&need(status)?)?,
        closed_at: parse_opt_ts(closed_at)?,
        result,
    })
}

impl Store {
    pub fn create_followup(&self, f: &NewFollowup<'_>, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO followups (id, created_at, due_at, task, detail, conversation, run_id, origin, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'open')",
            params![
                f.id,
                ts(now),
                ts(f.due_at),
                f.task,
                f.detail,
                f.conversation,
                f.run_id,
                f.origin
            ],
        )?;
        Ok(())
    }

    pub fn followup(&self, id: &str) -> Result<Option<Followup>> {
        let c = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM followups WHERE id = ?1"),
                [id],
                row,
            )
            .optional()?;
        c.map(from_row).transpose()
    }

    /// 열린 후속 조치(이른 것부터).
    pub fn open_followups(&self) -> Result<Vec<Followup>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM followups WHERE status = 'open' ORDER BY due_at, id"
        ))?;
        let rows = stmt.query_map([], row)?;
        rows.map(|r| from_row(r?)).collect()
    }

    /// 최근에 닫힌 것(새것부터).
    pub fn closed_followups(&self, limit: usize) -> Result<Vec<Followup>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM followups WHERE status != 'open' ORDER BY closed_at DESC, id LIMIT ?1"
        ))?;
        let rows = stmt.query_map([limit as i64], row)?;
        rows.map(|r| from_row(r?)).collect()
    }

    /// `since` 뒤에 발사된(닫힌) 후속 조치 수(하루 발사 상한).
    pub fn followups_closed_since(&self, since: DateTime<Utc>) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM followups WHERE status IN ('done', 'failed') AND closed_at >= ?1",
            [ts(since)],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// 열린 것만 닫는다. 닫았으면 `true`(이미 닫혔으면 `false`).
    pub fn close_followup(
        &self,
        id: &str,
        status: FollowupStatus,
        result: &str,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE followups SET status = ?2, result = ?3, closed_at = ?4 WHERE id = ?1 AND status = 'open'",
            params![id, status.as_str(), result, ts(now)],
        )?;
        if n == 1 && status == FollowupStatus::Cancelled {
            self.conn.execute(
                "UPDATE schedules SET enabled = 0, next_due_at = NULL WHERE id = ?1",
                [id],
            )?;
        }
        Ok(n == 1)
    }

    /// 열린 후속 조치의 출처를 바꾼다(결정 0013: 사용자가 관리 페이지에서 브라우저를 허용하면 `user`).
    pub fn set_followup_origin(&self, id: &str, origin: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE followups SET origin = ?2 WHERE id = ?1 AND status = 'open'",
            params![id, origin],
        )?;
        Ok(n == 1)
    }

    /// 열린 후속 조치의 시각을 옮긴다(방해 금지 시간 등). 스케줄도 함께 옮긴다.
    pub fn reschedule_followup(&self, id: &str, due_at: DateTime<Utc>) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE followups SET due_at = ?2 WHERE id = ?1 AND status = 'open'",
            params![id, ts(due_at)],
        )?;
        if n == 1 {
            self.conn.execute(
                "UPDATE schedules SET next_due_at = ?2, enabled = 1 WHERE id = ?1",
                params![id, ts(due_at)],
            )?;
        }
        Ok(n == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    fn new<'a>(id: &'a str, due: DateTime<Utc>) -> NewFollowup<'a> {
        NewFollowup {
            id,
            due_at: due,
            task: "치과 예약 확인",
            detail: "",
            conversation: Some("dm"),
            run_id: Some("chat-1"),
            origin: "chat",
        }
    }

    #[test]
    fn lifecycle_open_close_once_and_reschedule() {
        let store = Store::open_in_memory().unwrap();
        store
            .create_followup(&new("f1", at(18, 0)), at(9, 0))
            .unwrap();
        store
            .create_followup(&new("f2", at(12, 0)), at(9, 0))
            .unwrap();
        let open = store.open_followups().unwrap();
        assert_eq!(
            open.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["f2", "f1"]
        );
        assert_eq!(open[0].status, FollowupStatus::Open);
        assert!(store.reschedule_followup("f2", at(13, 0)).unwrap());
        assert_eq!(store.followup("f2").unwrap().unwrap().due_at, at(13, 0));
        assert!(
            store
                .close_followup("f2", FollowupStatus::Done, "확인함", at(13, 1))
                .unwrap()
        );
        assert!(
            !store
                .close_followup("f2", FollowupStatus::Failed, "x", at(13, 2))
                .unwrap(),
            "한 번만 닫힌다"
        );
        assert_eq!(store.followups_closed_since(at(13, 0)).unwrap(), 1);
        assert!(
            store
                .close_followup("f1", FollowupStatus::Cancelled, "필요 없음", at(13, 3))
                .unwrap()
        );
        assert!(store.open_followups().unwrap().is_empty());
        assert_eq!(store.closed_followups(5).unwrap().len(), 2);
        assert!(
            !store.reschedule_followup("f1", at(20, 0)).unwrap(),
            "닫힌 것은 못 옮긴다"
        );
    }
}
