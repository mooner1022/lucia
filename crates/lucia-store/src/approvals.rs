//! 외부 쓰기 승인과 활동 기록(결정 0003).
//!
//! 승인 요청은 내용을 고정해 저장하고, 상태 전이는 조건부 갱신으로만 한다. 그래서 버튼을 두 번 눌러도
//! `pending → approved`는 한 번만 성공하고, 만료된 요청은 승인되지 않는다.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Row, params};
use serde_json::Value;

use crate::{Result, Store, StoreError, parse_opt_ts, parse_ts, ts};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
    Sent,
    Failed,
}

impl ApprovalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Expired => "expired",
            Self::Sent => "sent",
            Self::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "pending" => Self::Pending,
            "approved" => Self::Approved,
            "denied" => Self::Denied,
            "expired" => Self::Expired,
            "sent" => Self::Sent,
            "failed" => Self::Failed,
            other => return Err(StoreError::Corrupt(format!("승인 상태 {other:?}"))),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewApproval {
    pub id: String,
    pub kind: String,
    pub payload: Value,
    pub summary: String,
    pub run_id: Option<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Approval {
    pub id: String,
    pub kind: String,
    pub payload: Value,
    pub summary: String,
    pub status: ApprovalStatus,
    pub requested_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub run_id: Option<String>,
    pub conversation: Option<String>,
    pub card_ref: Option<String>,
    pub result: Option<String>,
    /// 카드 없이 데몬이 바로 하기로 정한 요청(해제 모드, 결정 0040).
    pub auto: bool,
}

/// 결정 시도의 결과.
#[derive(Debug, Clone, PartialEq)]
pub enum Decided {
    /// 이번 시도로 상태가 바뀌었다.
    Changed(Box<Approval>),
    /// 이미 결정됐거나 만료됐다(지금 상태).
    AlreadyFinal(ApprovalStatus),
    /// 그런 요청이 없다.
    NotFound,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Activity {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub kind: String,
    pub summary: String,
    pub run_id: Option<String>,
}

const COLUMNS: &str = "id, kind, payload, summary, status, requested_at, expires_at, decided_at, \
                       run_id, conversation, card_ref, result, auto";

fn approval_row(r: &Row<'_>) -> rusqlite::Result<[Option<String>; 13]> {
    let mut out: [Option<String>; 13] = Default::default();
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = r.get(i)?;
    }
    Ok(out)
}

fn approval_from(cols: [Option<String>; 13]) -> Result<Approval> {
    let [
        id,
        kind,
        payload,
        summary,
        status,
        requested,
        expires,
        decided,
        run_id,
        conversation,
        card_ref,
        result,
        auto,
    ] = cols;
    let need = |v: Option<String>, name: &str| {
        v.ok_or_else(|| StoreError::Corrupt(format!("승인 {name}이 비었다")))
    };
    Ok(Approval {
        id: need(id, "id")?,
        kind: need(kind, "kind")?,
        payload: serde_json::from_str(&need(payload, "payload")?)?,
        summary: need(summary, "summary")?,
        status: ApprovalStatus::parse(&need(status, "status")?)?,
        requested_at: parse_ts(&need(requested, "requested_at")?)?,
        expires_at: parse_ts(&need(expires, "expires_at")?)?,
        decided_at: parse_opt_ts(decided)?,
        run_id,
        conversation,
        card_ref,
        result,
        auto: auto.is_some(),
    })
}

impl Store {
    pub fn create_approval(&self, new: &NewApproval, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO approvals (id, kind, payload, summary, status, requested_at, expires_at, run_id)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7)",
            params![
                new.id,
                new.kind,
                serde_json::to_string(&new.payload)?,
                new.summary,
                ts(now),
                ts(new.expires_at),
                new.run_id
            ],
        )?;
        Ok(())
    }

    /// 카드 없이 바로 하기로 표시한다(해제 모드, 결정 0040). 요청을 만든 직전에만 부른다.
    pub fn set_approval_auto(&self, id: &str, why: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE approvals SET auto = ?2 WHERE id = ?1 AND status = 'pending'",
            params![id, why],
        )?;
        Ok(())
    }

    pub fn approval(&self, id: &str) -> Result<Option<Approval>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM approvals WHERE id = ?1"),
                [id],
                approval_row,
            )
            .optional()?
            .map(approval_from)
            .transpose()
    }

    /// 카드를 올린 곳을 기록한다.
    pub fn set_approval_card(&self, id: &str, conversation: &str, card_ref: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE approvals SET conversation = ?2, card_ref = ?3 WHERE id = ?1",
            params![id, conversation, card_ref],
        )?;
        Ok(())
    }

    /// `pending`이고 만료 전인 요청만 `approved`나 `denied`로 바꾼다. 만료가 지났으면 `expired`로 닫는다.
    pub fn decide_approval(&self, id: &str, approve: bool, now: DateTime<Utc>) -> Result<Decided> {
        let status = if approve { "approved" } else { "denied" };
        let changed = self.conn.execute(
            "UPDATE approvals SET status = ?2, decided_at = ?3
             WHERE id = ?1 AND status = 'pending' AND expires_at > ?3",
            params![id, status, ts(now)],
        )?;
        if changed == 1 {
            let approval = self
                .approval(id)?
                .ok_or_else(|| StoreError::Corrupt(format!("방금 바꾼 승인 {id}가 없다")))?;
            return Ok(Decided::Changed(Box::new(approval)));
        }
        // 만료가 지난 대기 요청은 여기서 닫는다.
        self.conn.execute(
            "UPDATE approvals SET status = 'expired', decided_at = ?2
             WHERE id = ?1 AND status = 'pending' AND expires_at <= ?2",
            params![id, ts(now)],
        )?;
        Ok(match self.approval(id)? {
            Some(a) => Decided::AlreadyFinal(a.status),
            None => Decided::NotFound,
        })
    }

    /// 승인된 요청의 실행 결과를 남긴다(`approved → sent | failed`).
    pub fn finish_approval(
        &self,
        id: &str,
        sent: bool,
        result: &str,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE approvals SET status = ?2, result = ?3, finished_at = ?4
             WHERE id = ?1 AND status = 'approved'",
            params![id, if sent { "sent" } else { "failed" }, result, ts(now)],
        )?;
        Ok(changed == 1)
    }

    /// 만료가 지난 대기 요청을 닫고 돌려준다(카드를 고치고 알리기 위해).
    pub fn expire_approvals(&self, now: DateTime<Utc>) -> Result<Vec<Approval>> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM approvals WHERE status = 'pending' AND expires_at <= ?1",
            )?;
            stmt.query_map([ts(now)], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            let changed = self.conn.execute(
                "UPDATE approvals SET status = 'expired', decided_at = ?2
                 WHERE id = ?1 AND status = 'pending'",
                params![id, ts(now)],
            )?;
            if changed == 1
                && let Some(a) = self.approval(&id)?
            {
                out.push(a);
            }
        }
        Ok(out)
    }

    /// 실행하다 멈춘 요청(`approved`)을 실패로 닫는다. 보냈는지 알 수 없으므로 다시 보내지 않는다.
    pub fn fail_interrupted_approvals(&self, now: DateTime<Utc>) -> Result<Vec<Approval>> {
        let ids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM approvals WHERE status = 'approved'")?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            if self.finish_approval(
                &id,
                false,
                "데몬이 실행 중에 멈춰 결과를 모른다(보낸편지함을 확인)",
                now,
            )? && let Some(a) = self.approval(&id)?
            {
                out.push(a);
            }
        }
        Ok(out)
    }

    /// 최근 요청(관리 페이지용).
    pub fn recent_approvals(&self, limit: usize) -> Result<Vec<Approval>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM approvals ORDER BY requested_at DESC, rowid DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map([limit as i64], approval_row)?;
        rows.map(|r| approval_from(r?)).collect()
    }

    pub fn add_activity(
        &self,
        kind: &str,
        summary: &str,
        run_id: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO activity (at, kind, summary, run_id) VALUES (?1, ?2, ?3, ?4)",
            params![ts(now), kind, summary, run_id],
        )?;
        Ok(())
    }

    pub fn recent_activity(&self, limit: usize) -> Result<Vec<Activity>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, at, kind, summary, run_id FROM activity ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (id, at, kind, summary, run_id) = row?;
            Ok(Activity {
                id,
                at: parse_ts(&at)?,
                kind,
                summary,
                run_id,
            })
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    fn request(store: &Store, id: &str, expires: DateTime<Utc>) {
        store
            .create_approval(
                &NewApproval {
                    id: id.into(),
                    kind: "mail_send".into(),
                    payload: serde_json::json!({"to": ["a@example.com"]}),
                    summary: "메일 a@example.com".into(),
                    run_id: Some("chat-1".into()),
                    expires_at: expires,
                },
                at(9, 0),
            )
            .unwrap();
    }

    #[test]
    fn approve_once_even_when_pressed_twice() {
        let store = Store::open_in_memory().unwrap();
        request(&store, "r1", at(21, 0));
        store.set_approval_card("r1", "discord:1", "m-9").unwrap();

        let first = store.decide_approval("r1", true, at(9, 5)).unwrap();
        let Decided::Changed(a) = first else {
            panic!("첫 승인은 바뀐다: {first:?}")
        };
        assert_eq!(a.status, ApprovalStatus::Approved);
        assert_eq!(a.card_ref.as_deref(), Some("m-9"));
        assert_eq!(
            store.decide_approval("r1", true, at(9, 6)).unwrap(),
            Decided::AlreadyFinal(ApprovalStatus::Approved),
            "두 번째 누름은 아무것도 바꾸지 않는다"
        );
        assert_eq!(
            store.decide_approval("r1", false, at(9, 6)).unwrap(),
            Decided::AlreadyFinal(ApprovalStatus::Approved)
        );

        assert!(
            store
                .finish_approval("r1", true, "msg-1", at(9, 7))
                .unwrap()
        );
        assert!(
            !store
                .finish_approval("r1", true, "msg-2", at(9, 8))
                .unwrap(),
            "결과는 한 번만"
        );
        let a = store.approval("r1").unwrap().unwrap();
        assert_eq!(a.status, ApprovalStatus::Sent);
        assert_eq!(a.result.as_deref(), Some("msg-1"));
    }

    #[test]
    fn expired_requests_cannot_be_approved() {
        let store = Store::open_in_memory().unwrap();
        request(&store, "late", at(10, 0));
        assert_eq!(
            store.decide_approval("late", true, at(10, 0)).unwrap(),
            Decided::AlreadyFinal(ApprovalStatus::Expired),
            "만료 시각과 같으면 만료"
        );
        assert_eq!(
            store.decide_approval("nope", true, at(9, 0)).unwrap(),
            Decided::NotFound
        );

        request(&store, "sweep", at(11, 0));
        request(&store, "fresh", at(23, 0));
        let expired = store.expire_approvals(at(12, 0)).unwrap();
        assert_eq!(
            expired.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            vec!["sweep"]
        );
        assert!(store.expire_approvals(at(12, 0)).unwrap().is_empty());
        assert_eq!(
            store.approval("fresh").unwrap().unwrap().status,
            ApprovalStatus::Pending
        );
    }

    #[test]
    fn interrupted_sends_are_closed_not_retried() {
        let store = Store::open_in_memory().unwrap();
        request(&store, "r1", at(21, 0));
        store.decide_approval("r1", true, at(9, 1)).unwrap();
        let closed = store.fail_interrupted_approvals(at(9, 30)).unwrap();
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].status, ApprovalStatus::Failed);
        assert!(
            store
                .fail_interrupted_approvals(at(9, 31))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn activity_is_listed_newest_first() {
        let store = Store::open_in_memory().unwrap();
        store
            .add_activity("mail_search", "메일 검색: from:a", Some("chat-1"), at(9, 0))
            .unwrap();
        store
            .add_activity("memory_write", "핵심 기억 개정 3", None, at(9, 1))
            .unwrap();
        let list = store.recent_activity(10).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].kind, "memory_write");
        assert_eq!(list[1].run_id.as_deref(), Some("chat-1"));
    }
}
