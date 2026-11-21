//! 접속 API의 클라이언트(결정 0021). 토큰 원문은 저장하지 않는다(해시만).
//! 앱 연동(결정 0043): 클라이언트마다 범위(scopes), 푸시 토큰, 보낸 프레임 상자(`app_outbox`)를 둔다.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension as _, params};

use crate::{Result, Store, parse_opt_ts, parse_ts, ts};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Client {
    pub id: String,
    pub name: String,
    /// app | device
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// 기본 밖의 권한(결정 0043). 예: `security_mode`(권한 모드 바꾸기).
    pub scopes: Vec<String>,
    /// 푸시 등록 토큰과 플랫폼(결정 0043). 토큰은 JSON으로 내보내지 않는다(관리 화면에 보일 까닭이 없다).
    #[serde(skip_serializing)]
    pub push_token: Option<String>,
    pub push_platform: Option<String>,
    pub push_at: Option<DateTime<Utc>>,
    /// 앱이 받았다고 알린 마지막 프레임 번호.
    pub acked_seq: i64,
}

const COLUMNS: &str = "id, name, role, created_at, last_seen_at, revoked_at, scopes, push_token, push_platform, push_at, acked_seq";

struct Raw {
    id: String,
    name: String,
    role: String,
    created_at: String,
    last_seen_at: Option<String>,
    revoked_at: Option<String>,
    scopes: String,
    push_token: Option<String>,
    push_platform: Option<String>,
    push_at: Option<String>,
    acked_seq: i64,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Raw> {
    Ok(Raw {
        id: r.get(0)?,
        name: r.get(1)?,
        role: r.get(2)?,
        created_at: r.get(3)?,
        last_seen_at: r.get(4)?,
        revoked_at: r.get(5)?,
        scopes: r.get(6)?,
        push_token: r.get(7)?,
        push_platform: r.get(8)?,
        push_at: r.get(9)?,
        acked_seq: r.get(10)?,
    })
}

fn finish(c: Raw) -> Result<Client> {
    Ok(Client {
        id: c.id,
        name: c.name,
        role: c.role,
        created_at: parse_ts(&c.created_at)?,
        last_seen_at: parse_opt_ts(c.last_seen_at)?,
        revoked_at: parse_opt_ts(c.revoked_at)?,
        scopes: serde_json::from_str(&c.scopes).unwrap_or_default(),
        push_token: c.push_token,
        push_platform: c.push_platform,
        push_at: parse_opt_ts(c.push_at)?,
        acked_seq: c.acked_seq,
    })
}

/// 보낸 프레임 하나(결정 0043).
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxFrame {
    pub seq: i64,
    pub kind: String,
    pub frame: String,
    pub created_at: DateTime<Utc>,
}

impl Store {
    pub fn create_client(
        &self,
        id: &str,
        name: &str,
        role: &str,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO clients (id, name, role, token_hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, role, token_hash, ts(now)],
        )?;
        Ok(())
    }

    /// 취소되지 않은 클라이언트를 토큰 해시로 찾는다.
    pub fn client_by_token_hash(&self, token_hash: &str) -> Result<Option<Client>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM clients WHERE token_hash = ?1 AND revoked_at IS NULL"
                ),
                [token_hash],
                row,
            )
            .optional()?
            .map(finish)
            .transpose()
    }

    /// 취소되지 않은 클라이언트(ID로).
    pub fn active_client(&self, id: &str) -> Result<Option<Client>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM clients WHERE id = ?1 AND revoked_at IS NULL"),
                [id],
                row,
            )
            .optional()?
            .map(finish)
            .transpose()
    }

    pub fn clients(&self) -> Result<Vec<Client>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM clients ORDER BY created_at DESC"
        ))?;
        let rows = stmt.query_map([], row)?;
        rows.map(|r| finish(r?)).collect()
    }

    pub fn touch_client(&self, id: &str, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "UPDATE clients SET last_seen_at = ?2 WHERE id = ?1",
            params![id, ts(now)],
        )?;
        Ok(())
    }

    pub fn revoke_client(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let done = self.conn.execute(
            "UPDATE clients SET revoked_at = ?2, push_token = NULL WHERE id = ?1 AND revoked_at IS NULL",
            params![id, ts(now)],
        )? == 1;
        // 취소한 클라이언트에게 쌓아 둔 프레임은 버린다(다시 받을 길이 없다).
        self.conn
            .execute("DELETE FROM app_outbox WHERE client_id = ?1", [id])?;
        Ok(done)
    }

    pub fn set_client_scopes(&self, id: &str, scopes: &[String]) -> Result<()> {
        self.conn.execute(
            "UPDATE clients SET scopes = ?2 WHERE id = ?1",
            params![id, serde_json::to_string(scopes)?],
        )?;
        Ok(())
    }

    /// 푸시 토큰을 바꾼다(`None`이면 지운다).
    pub fn set_client_push(
        &self,
        id: &str,
        token: Option<&str>,
        platform: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE clients SET push_token = ?2, push_platform = ?3, push_at = ?4 WHERE id = ?1",
            params![id, token, platform, ts(now)],
        )?;
        Ok(())
    }

    /// 프레임을 상자에 넣고 번호를 돌려준다(클라이언트마다 1부터 늘어난다).
    pub fn outbox_push(
        &self,
        client_id: &str,
        kind: &str,
        frame: &str,
        now: DateTime<Utc>,
    ) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE clients SET out_seq = out_seq + 1 WHERE id = ?1",
            [client_id],
        )?;
        let seq: i64 = tx.query_row(
            "SELECT out_seq FROM clients WHERE id = ?1",
            [client_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO app_outbox (client_id, seq, kind, frame, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![client_id, seq, kind, frame, ts(now)],
        )?;
        tx.commit()?;
        Ok(seq)
    }

    /// `after`보다 뒤의 프레임(오래된 것부터).
    pub fn outbox_after(
        &self,
        client_id: &str,
        after: i64,
        limit: usize,
    ) -> Result<Vec<OutboxFrame>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, kind, frame, created_at FROM app_outbox WHERE client_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![client_id, after, limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|r| {
            let (seq, kind, frame, at) = r?;
            Ok(OutboxFrame {
                seq,
                kind,
                frame,
                created_at: parse_ts(&at)?,
            })
        })
        .collect()
    }

    /// 마지막으로 매긴 번호.
    pub fn outbox_last(&self, client_id: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE((SELECT out_seq FROM clients WHERE id = ?1), 0)",
            [client_id],
            |r| r.get(0),
        )?)
    }

    /// 앱이 여기까지 받았다(줄지 않는다).
    pub fn ack_client(&self, client_id: &str, seq: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE clients SET acked_seq = MAX(acked_seq, ?2) WHERE id = ?1",
            params![client_id, seq],
        )?;
        Ok(())
    }

    /// 오래된 프레임을 버린다. 받았다고 알린 것은 `acked_before`보다, 받지 않은 것도 `before`보다 오래되면.
    pub fn prune_outbox(
        &self,
        acked_before: DateTime<Utc>,
        before: DateTime<Utc>,
    ) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM app_outbox WHERE created_at < ?2
                OR (created_at < ?1 AND seq <= (SELECT acked_seq FROM clients WHERE clients.id = app_outbox.client_id))",
            params![ts(acked_before), ts(before)],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    #[test]
    fn tokens_are_found_by_hash_until_revoked() {
        let store = Store::open_in_memory().unwrap();
        store
            .create_client("c1", "휴대폰 앱", "app", "h1", at(9, 0))
            .unwrap();
        assert_eq!(store.client_by_token_hash("h1").unwrap().unwrap().id, "c1");
        assert!(store.client_by_token_hash("h2").unwrap().is_none());
        store.touch_client("c1", at(9, 5)).unwrap();
        assert_eq!(store.clients().unwrap()[0].last_seen_at, Some(at(9, 5)));
        assert!(store.revoke_client("c1", at(10, 0)).unwrap());
        assert!(store.client_by_token_hash("h1").unwrap().is_none());
        assert!(store.active_client("c1").unwrap().is_none());
        assert!(!store.revoke_client("c1", at(10, 1)).unwrap());
    }

    #[test]
    fn outbox_numbers_replays_and_prunes() {
        let store = Store::open_in_memory().unwrap();
        store
            .create_client("c1", "앱", "app", "h1", at(9, 0))
            .unwrap();
        store
            .create_client("c2", "앱2", "app", "h2", at(9, 0))
            .unwrap();
        assert_eq!(store.outbox_last("c1").unwrap(), 0);
        assert_eq!(
            store
                .outbox_push("c1", "text", "{\"a\":1}", at(9, 1))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .outbox_push("c1", "card", "{\"a\":2}", at(9, 2))
                .unwrap(),
            2
        );
        assert_eq!(
            store.outbox_push("c2", "text", "{}", at(9, 2)).unwrap(),
            1,
            "클라이언트마다 따로"
        );
        let after = store.outbox_after("c1", 1, 10).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!((after[0].seq, after[0].kind.as_str()), (2, "card"));
        store.ack_client("c1", 2).unwrap();
        store.ack_client("c1", 1).unwrap();
        assert_eq!(
            store.active_client("c1").unwrap().unwrap().acked_seq,
            2,
            "줄지 않는다"
        );
        // 받은 것은 짧게, 받지 않은 것은 길게 둔다.
        assert_eq!(store.prune_outbox(at(9, 2), at(8, 0)).unwrap(), 1);
        assert_eq!(store.outbox_after("c1", 0, 10).unwrap().len(), 1);
        // 다 지워져도 번호는 되돌리지 않는다.
        assert_eq!(store.prune_outbox(at(10, 0), at(8, 0)).unwrap(), 1);
        assert_eq!(store.outbox_last("c1").unwrap(), 2);
        assert_eq!(store.outbox_push("c1", "text", "{}", at(9, 3)).unwrap(), 3);
        // 범위·푸시 토큰, 취소하면 상자와 토큰을 버린다.
        store
            .set_client_scopes("c1", &["security_mode".into()])
            .unwrap();
        store
            .set_client_push("c1", Some("tok"), Some("android"), at(9, 4))
            .unwrap();
        let c = store.active_client("c1").unwrap().unwrap();
        assert_eq!(c.scopes, vec!["security_mode".to_owned()]);
        assert_eq!(c.push_token.as_deref(), Some("tok"));
        store.revoke_client("c1", at(9, 5)).unwrap();
        assert!(store.outbox_after("c1", 0, 10).unwrap().is_empty());
        assert!(
            store
                .clients()
                .unwrap()
                .iter()
                .find(|c| c.id == "c1")
                .unwrap()
                .push_token
                .is_none()
        );
    }
}
