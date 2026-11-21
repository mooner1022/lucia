//! 관리 페이지용 저장: 설정값, 알림 기록, 관리자 비밀번호 해시, 초기 설정 링크.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

use crate::{Result, Store, parse_ts, ts};

/// 설정 키(관리 페이지에서 정하는 값).
pub mod keys {
    /// 소유자 Discord 사용자 ID. 비어 있으면 아무도 허용하지 않는다.
    pub const OWNER_DISCORD_USER_ID: &str = "owner.discord_user_id";
    /// 브리핑과 알림을 보낼 대화(소유자와의 DM). 예: "discord:123".
    pub const NOTIFY_CONVERSATION: &str = "notify.conversation";
    /// 연결된 Google 계정 메일 주소(상태 화면용).
    pub const GOOGLE_ACCOUNT: &str = "google.account";
    /// 동의에서 실제로 받은 범위(공백으로 나눈 목록). 사용자가 동의 화면에서 일부를 끌 수 있다.
    pub const GOOGLE_SCOPES: &str = "google.scopes";
    /// 마지막 세션 기록 백업(상태 화면용 요약).
    pub const BACKUP_LAST: &str = "backup.last";
    /// 다른 서버에서 부르기(결정 0024): "on"이면 켠다. 없으면 꺼짐.
    pub const DISCORD_GUILD_CALLS: &str = "discord.guild_calls";
    /// 서버에서 Lucia를 부르는 머리말(결정 0024). 없으면 멘션·답장으로만 부른다.
    pub const DISCORD_PREFIX: &str = "discord.prefix";
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Alert {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub text: String,
    pub delivered: bool,
}

impl Store {
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, ts(now)],
        )?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    /// 이 접두어로 시작하는 설정(키 순). 접두어는 글자 그대로 비교한다(`%`·`_`를 특수 문자로 보지 않는다).
    pub fn settings_with_prefix(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT key, value FROM settings WHERE substr(key, 1, length(?1)) = ?1 ORDER BY key",
        )?;
        let rows = stmt
            .query_map([prefix], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 알림을 남긴다. `text`는 마스킹을 거친 글이어야 한다.
    pub fn add_alert(&self, text: &str, delivered: bool, at: DateTime<Utc>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO alerts (at, text, delivered) VALUES (?1, ?2, ?3)",
            params![ts(at), text, delivered],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 같은 글의 알림이 `since` 이후에 있었는가(같은 알림을 되풀이하지 않게).
    pub fn alert_seen_since(&self, text: &str, since: DateTime<Utc>) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM alerts WHERE text = ?1 AND at > ?2)",
            params![text, ts(since)],
            |r| r.get(0),
        )?)
    }

    /// 최근 알림(새것부터).
    pub fn recent_alerts(&self, limit: usize) -> Result<Vec<Alert>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, at, text, delivered FROM alerts ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (id, at, text, delivered) = row?;
            Ok(Alert {
                id,
                at: parse_ts(&at)?,
                text,
                delivered,
            })
        })
        .collect()
    }

    pub fn admin_password_hash(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT password_hash FROM admin_auth WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// 비밀번호 해시를 저장하고 초기 설정 링크를 지운다.
    pub fn set_admin_password_hash(&mut self, hash: &str, now: DateTime<Utc>) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO admin_auth (id, password_hash, updated_at) VALUES (1, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET password_hash = excluded.password_hash,
                                            updated_at = excluded.updated_at",
            params![hash, ts(now)],
        )?;
        tx.execute("DELETE FROM admin_setup", [])?;
        tx.commit()?;
        Ok(())
    }

    /// 비밀번호를 지운다(서버에서 `luciad admin-setup --reset`). 그 뒤로는 설정 링크로만 다시 정한다.
    pub fn clear_admin_password(&self) -> Result<()> {
        self.conn.execute("DELETE FROM admin_auth", [])?;
        Ok(())
    }

    /// 초기 설정 링크 토큰(해시)을 둔다. 이전 것은 바뀐다.
    pub fn set_admin_setup(&self, token_hash: &str, expires_at: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO admin_setup (id, token_hash, expires_at) VALUES (1, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET token_hash = excluded.token_hash,
                                            expires_at = excluded.expires_at",
            params![token_hash, ts(expires_at)],
        )?;
        Ok(())
    }

    /// 초기 설정 링크가 유효한가: 비밀번호가 아직 없고, 해시가 같고, 만료 전.
    pub fn admin_setup_valid(&self, token_hash: &str, now: DateTime<Utc>) -> Result<bool> {
        if self.admin_password_hash()?.is_some() {
            return Ok(false);
        }
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT token_hash, expires_at FROM admin_setup WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((hash, expires)) => hash == token_hash && parse_ts(&expires)? > now,
            None => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    #[test]
    fn settings_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        assert!(
            store
                .setting(keys::OWNER_DISCORD_USER_ID)
                .unwrap()
                .is_none()
        );
        store
            .set_setting(keys::OWNER_DISCORD_USER_ID, "123", at(9, 0))
            .unwrap();
        store
            .set_setting(keys::OWNER_DISCORD_USER_ID, "456", at(9, 1))
            .unwrap();
        assert_eq!(
            store
                .setting(keys::OWNER_DISCORD_USER_ID)
                .unwrap()
                .as_deref(),
            Some("456")
        );
        store.delete_setting(keys::OWNER_DISCORD_USER_ID).unwrap();
        assert!(
            store
                .setting(keys::OWNER_DISCORD_USER_ID)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn settings_by_prefix_match_literally() {
        let store = Store::open_in_memory().unwrap();
        for (k, v) in [
            ("browser.site.b.example.com.username", "me"),
            (
                "browser.site.a.example.com.login_url",
                "https://a.example.com/login",
            ),
            ("browser.sitex", "x"),
            ("browser_site_y", "y"),
            ("other", "z"),
        ] {
            store.set_setting(k, v, at(9, 0)).unwrap();
        }
        let got = store.settings_with_prefix("browser.site.").unwrap();
        assert_eq!(
            got.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            [
                "browser.site.a.example.com.login_url",
                "browser.site.b.example.com.username"
            ],
            "LIKE 특수 문자(_ %)로 넓어지지 않는다"
        );
        assert!(store.settings_with_prefix("browser_").unwrap().len() == 1);
    }

    #[test]
    fn alerts_newest_first() {
        let store = Store::open_in_memory().unwrap();
        store.add_alert("첫째", false, at(9, 0)).unwrap();
        store.add_alert("둘째", true, at(9, 1)).unwrap();
        let alerts = store.recent_alerts(10).unwrap();
        assert_eq!(alerts[0].text, "둘째");
        assert!(alerts[0].delivered);
        assert!(!alerts[1].delivered);
    }

    #[test]
    fn setup_link_only_before_password_and_before_expiry() {
        let mut store = Store::open_in_memory().unwrap();
        store.set_admin_setup("h1", at(10, 0)).unwrap();
        assert!(store.admin_setup_valid("h1", at(9, 59)).unwrap());
        assert!(
            !store.admin_setup_valid("h2", at(9, 59)).unwrap(),
            "다른 토큰"
        );
        assert!(!store.admin_setup_valid("h1", at(10, 0)).unwrap(), "만료");

        store.set_admin_setup("h3", at(11, 0)).unwrap();
        store
            .set_admin_password_hash("$argon2…", at(10, 30))
            .unwrap();
        assert!(
            !store.admin_setup_valid("h3", at(10, 31)).unwrap(),
            "비밀번호가 생기면 링크는 끝"
        );
        assert_eq!(
            store.admin_password_hash().unwrap().as_deref(),
            Some("$argon2…")
        );
    }
}
