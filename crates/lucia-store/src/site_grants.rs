//! 사이트 위임(결정 0028): 사람이 한 번 허용한 사이트를 사람 없는 실행이 다시 묻지 않고 쓴다.
//!
//! 허용은 그 호스트와 하위 도메인이다(공개 접미사 목록 없이, 사람이 고른 호스트 그대로). 만들기·넓히기는
//! 데몬이 사람의 카드 승인이나 관리 페이지에서만 부른다. 여기는 저장과 판단만 한다.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{OptionalExtension as _, params};

use crate::{Result, Store, StoreError, parse_opt_ts, parse_ts, ts};

/// 위임 수 [제안].
pub const MAX_SITE_GRANTS: usize = 50;
/// 이만큼 쓰지 않은 기한 없는 위임은 쓰지 않는다 [제안](관리 페이지에 "오래 안 씀"으로 보인다).
pub const IDLE_DAYS: i64 = 90;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SiteGrant {
    pub id: String,
    pub host: String,
    pub login: bool,
    pub actions: bool,
    pub purpose: String,
    pub origin: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub uses: i64,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl SiteGrant {
    /// 지금 쓸 수 있는가: 거두지 않았고, 기한이 남았고, 기한 없는 것은 오래 놀지 않았다.
    pub fn active(&self, now: DateTime<Utc>) -> bool {
        if self.revoked_at.is_some() {
            return false;
        }
        match self.expires_at {
            Some(t) => t > now,
            None => self.last_used_at.unwrap_or(self.created_at) > now - Duration::days(IDLE_DAYS),
        }
    }

    /// 이 호스트를 덮는가(같거나 하위 도메인).
    pub fn covers(&self, host: &str) -> bool {
        covers(&self.host, host)
    }
}

/// `granted`가 `host`를 덮는가(같거나 하위 도메인). 대소문자와 끝 점은 무시한다.
pub fn covers(granted: &str, host: &str) -> bool {
    let g = granted.trim().trim_end_matches('.').to_ascii_lowercase();
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    !g.is_empty() && (h == g || h.ends_with(&format!(".{g}")))
}

/// 허용할 수 있는 호스트 이름인가(점이 있는 이름, 글자·숫자·하이픈·점만). 최상위 도메인 하나(`kr`)나
/// 흔한 2단 접미사 하나(`ac.kr`, `co.kr`)만 주는 것은 받지 않는다(그 아래 모든 사이트가 열린다).
pub fn valid_host(host: &str) -> std::result::Result<String, String> {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if h.is_empty() || h.len() > 253 {
        return Err("호스트가 비었거나 너무 길다".into());
    }
    if !h
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        || h.starts_with('.')
        || h.contains("..")
    {
        return Err(format!("호스트 이름이 아니다: {h}"));
    }
    let labels: Vec<&str> = h.split('.').collect();
    if labels.len() < 2 {
        return Err(format!("점 있는 호스트여야 한다: {h}"));
    }
    const SECOND_LEVEL: [&str; 12] = [
        "ac", "co", "or", "go", "ne", "re", "pe", "com", "net", "org", "edu", "gov",
    ];
    if labels.len() == 2 && labels[1].len() == 2 && SECOND_LEVEL.contains(&labels[0]) {
        return Err(format!(
            "{h}는 공용 접미사라 그 아래 사이트를 모두 열게 된다. 사이트 호스트를 준다"
        ));
    }
    if h.parse::<std::net::Ipv4Addr>().is_ok() {
        return Err("IP 주소는 허용하지 않는다".into());
    }
    Ok(h)
}

#[derive(Debug, Clone)]
pub struct NewSiteGrant<'a> {
    pub id: &'a str,
    pub host: &'a str,
    pub login: bool,
    pub actions: bool,
    pub purpose: &'a str,
    pub origin: &'a str,
    pub expires_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "id, host, login, actions, purpose, origin, created_at, expires_at, revoked_at, uses, last_used_at";

#[allow(clippy::type_complexity)]
fn row(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<(
    String,
    String,
    i64,
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    Option<String>,
)> {
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
    ))
}

#[allow(clippy::type_complexity)]
fn grant(
    t: (
        String,
        String,
        i64,
        i64,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        i64,
        Option<String>,
    ),
) -> Result<SiteGrant> {
    Ok(SiteGrant {
        id: t.0,
        host: t.1,
        login: t.2 != 0,
        actions: t.3 != 0,
        purpose: t.4,
        origin: t.5,
        created_at: parse_ts(&t.6)?,
        expires_at: parse_opt_ts(t.7)?,
        revoked_at: parse_opt_ts(t.8)?,
        uses: t.9,
        last_used_at: parse_opt_ts(t.10)?,
    })
}

impl Store {
    /// 위임을 만든다. 같은 호스트의 쓸 수 있는 위임이 있으면 넓히기만 한다(로그인·동작은 켜는 쪽으로,
    /// 기한은 더 긴 쪽으로). 만든(또는 넓힌) 위임의 ID.
    pub fn grant_site(&self, g: &NewSiteGrant<'_>, now: DateTime<Utc>) -> Result<String> {
        let host = valid_host(g.host).map_err(StoreError::Invalid)?;
        if let Some(existing) = self
            .site_grants()?
            .into_iter()
            .find(|e| e.host == host && e.active(now))
        {
            let expires = match (existing.expires_at, g.expires_at) {
                (None, _) | (_, None) => None,
                (Some(a), Some(b)) => Some(a.max(b)),
            };
            // "이번만"을 계속 허용으로 넓히면 출처도 바꾼다(맥락·관리 페이지에 보이게).
            let (origin, purpose) = if g.origin == "once" {
                (existing.origin.clone(), existing.purpose.clone())
            } else {
                (g.origin.to_owned(), g.purpose.to_owned())
            };
            self.conn.execute(
                "UPDATE site_grants SET login = ?2, actions = ?3, expires_at = ?4, purpose = ?5, origin = ?6 WHERE id = ?1",
                params![
                    existing.id,
                    i64::from(existing.login || g.login),
                    i64::from(existing.actions || g.actions),
                    expires.map(ts),
                    purpose,
                    origin,
                ],
            )?;
            return Ok(existing.id);
        }
        let live = self
            .site_grants()?
            .into_iter()
            .filter(|e| e.active(now))
            .count();
        if live >= MAX_SITE_GRANTS {
            return Err(StoreError::Invalid(format!(
                "사이트 위임은 {MAX_SITE_GRANTS}개까지다"
            )));
        }
        self.conn.execute(
            &format!("INSERT INTO site_grants ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, 0, NULL)"),
            params![
                g.id,
                host,
                i64::from(g.login),
                i64::from(g.actions),
                g.purpose,
                g.origin,
                ts(now),
                g.expires_at.map(ts),
            ],
        )?;
        Ok(g.id.to_owned())
    }

    pub fn site_grants(&self) -> Result<Vec<SiteGrant>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM site_grants ORDER BY created_at DESC, id"
        ))?;
        let rows = stmt.query_map([], row)?;
        rows.map(|r| grant(r?)).collect()
    }

    pub fn site_grant(&self, id: &str) -> Result<Option<SiteGrant>> {
        let t = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM site_grants WHERE id = ?1"),
                [id],
                row,
            )
            .optional()?;
        t.map(grant).transpose()
    }

    /// 이 호스트를 덮는 쓸 수 있는 위임들(가장 좁은 것부터).
    pub fn site_grants_for(&self, host: &str, now: DateTime<Utc>) -> Result<Vec<SiteGrant>> {
        let mut out: Vec<SiteGrant> = self
            .site_grants()?
            .into_iter()
            .filter(|g| g.active(now) && g.covers(host))
            .collect();
        out.sort_by_key(|g| std::cmp::Reverse(g.host.len()));
        Ok(out)
    }

    /// 썼다고 적는다(관리 페이지의 사용 횟수, 오래 안 씀 판정).
    pub fn touch_site_grant(&self, id: &str, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "UPDATE site_grants SET uses = uses + 1, last_used_at = ?2 WHERE id = ?1",
            params![id, ts(now)],
        )?;
        Ok(())
    }

    /// 거둔다(사람). 이미 거둔 것이면 `false`.
    pub fn revoke_site_grant(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE site_grants SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, ts(now)],
        )? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    fn new<'a>(id: &'a str, host: &'a str) -> NewSiteGrant<'a> {
        NewSiteGrant {
            id,
            host,
            login: false,
            actions: false,
            purpose: "과제 확인",
            origin: "chat",
            expires_at: None,
        }
    }

    #[test]
    fn grants_cover_subdomains_widen_and_expire() {
        let s = Store::open_in_memory().unwrap();
        let now = at(9, 0);
        s.grant_site(&new("g1", "Lms.Example.ac.kr."), now).unwrap();
        let g = s.site_grant("g1").unwrap().unwrap();
        assert_eq!(g.host, "lms.example.ac.kr");
        assert!(g.covers("lms.example.ac.kr") && g.covers("a.lms.example.ac.kr"));
        assert!(!g.covers("example.ac.kr") && !g.covers("evil-lms.example.ac.kr"));
        assert!(!g.covers("lms.example.ac.kr.evil.com"));

        // 같은 호스트는 넓히기만 한다(로그인을 켜도 새 줄이 생기지 않는다).
        let mut wider = new("g2", "lms.example.ac.kr");
        wider.login = true;
        assert_eq!(s.grant_site(&wider, now).unwrap(), "g1");
        assert!(s.site_grant("g1").unwrap().unwrap().login);
        assert_eq!(s.site_grants().unwrap().len(), 1);

        // 이번만(기한 있음)은 기한이 지나면 쓰지 않는다.
        let mut once = new("g3", "example.com");
        once.origin = "once";
        once.expires_at = Some(now + Duration::hours(12));
        s.grant_site(&once, now).unwrap();
        assert_eq!(s.site_grants_for("www.example.com", now).unwrap().len(), 1);
        assert!(
            s.site_grants_for("www.example.com", now + Duration::hours(13))
                .unwrap()
                .is_empty()
        );

        // 오래 안 쓴 기한 없는 위임은 쓰지 않고, 쓰면 다시 산다.
        let later = now + Duration::days(IDLE_DAYS + 1);
        assert!(
            s.site_grants_for("lms.example.ac.kr", later)
                .unwrap()
                .is_empty()
        );
        s.touch_site_grant("g1", later - Duration::days(1)).unwrap();
        assert_eq!(
            s.site_grants_for("lms.example.ac.kr", later).unwrap().len(),
            1
        );

        // 이번만을 계속 허용으로 넓히면 출처도 바뀐다.
        let mut always = new("g4", "example.com");
        always.origin = "egress";
        assert_eq!(s.grant_site(&always, now).unwrap(), "g3");
        let g3 = s.site_grant("g3").unwrap().unwrap();
        assert_eq!(g3.origin, "egress");
        assert!(g3.expires_at.is_none());

        assert!(s.revoke_site_grant("g1", now).unwrap());
        assert!(!s.revoke_site_grant("g1", now).unwrap());
        assert!(
            s.site_grants_for("lms.example.ac.kr", now)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn hosts_that_would_open_too_much_are_refused() {
        for bad in [
            "kr",
            "ac.kr",
            "co.kr",
            "com",
            "",
            "a..b",
            "1.2.3.4",
            "exa mple.com",
            ".x.com",
        ] {
            assert!(valid_host(bad).is_err(), "{bad}");
        }
        for ok in [
            "example.com",
            "lms.example.ac.kr",
            "example.ac.kr",
            "x.co.kr",
        ] {
            assert!(valid_host(ok).is_ok(), "{ok}");
        }
    }
}
