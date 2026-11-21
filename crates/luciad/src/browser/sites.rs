//! 사이트 프로필(결정 0006·0008).
//!
//! - **내장 프로필**: 로그인 칸의 위치(선택자)를 사이트를 보고 정해 둔 사이트(퀘이사존, 2026-11-16 확인).
//!   로그인 페이지로 튕기는 것을 알아보고 로그인을 바로 요청한다.
//! - **일반 프로필**: 그 밖의 https 사이트. 로그인 페이지에서 보이는 비밀번호 칸 하나와 그 앞의 아이디 칸을
//!   데몬 스크립트가 찾는다. 로그인 토큰·저장된 로그인은 정확한 출처(`https://호스트`)에 묶는다.
//! - 어디로 갈 수 있는지는 프로필이 아니라 차단 목록([`super::egress`])이 정한다.

use sha2::{Digest as _, Sha256};
use url::Url;

/// 로그인 폼.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LoginForm {
    /// 로그인 페이지 주소.
    pub url: String,
    /// 이 경로로 시작하는 페이지는 로그인 페이지다(로그인이 필요해서 튕겨 온 것).
    pub path: String,
    pub username: String,
    pub password: String,
    /// 로그인 유지 체크(있으면 켠다. 세션 쿠키만으로는 몇 시간 뒤 풀린다).
    pub remember: Option<String>,
    /// 로그인 버튼. 없으면 비밀번호 칸에서 Enter.
    pub submit: Option<String>,
    /// 칸을 로그인 때마다 찾는다(일반 프로필). 찾은 칸에 [`AUTO_ATTR`] 표시를 단다.
    pub auto: bool,
}

/// 일반 프로필에서 찾은 칸에 다는 속성.
pub const AUTO_ATTR: &str = "data-lucia-login";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SiteProfile {
    /// 짧은 ID(자격 증명 이름, 도구 인자). 일반 프로필은 호스트.
    pub id: String,
    /// 사람에게 보이는 이름.
    pub name: String,
    /// `https://example.com`(스킴과 호스트만).
    pub origin: String,
    /// 이 프로필의 호스트: `example.com` 또는 `*.example.com`(하위 도메인만).
    pub hosts: Vec<String>,
    pub login: LoginForm,
}

impl SiteProfile {
    /// 일반 프로필: 로그인 페이지 주소의 호스트 하나에만 묶는다.
    pub fn generic(login_url: &Url) -> Result<Self, String> {
        if login_url.scheme() != "https" {
            return Err("로그인은 https 페이지에서만 한다".into());
        }
        let host = login_url
            .host_str()
            .ok_or("호스트가 없다")?
            .to_ascii_lowercase();
        if login_url.port().is_some() {
            return Err("다른 포트의 로그인 페이지는 쓰지 않는다".into());
        }
        let attr = |v: &str| format!("[{AUTO_ATTR}=\"{v}\"]");
        let mut url = login_url.clone();
        url.set_fragment(None);
        Ok(Self {
            id: host.clone(),
            name: host.clone(),
            origin: format!("https://{host}"),
            hosts: vec![host],
            login: LoginForm {
                url: url.to_string(),
                path: url.path().to_owned(),
                username: attr("user"),
                password: attr("pw"),
                remember: Some(attr("remember")),
                submit: Some(attr("submit")),
                auto: true,
            },
        })
    }

    pub fn is_builtin(&self) -> bool {
        !self.login.auto
    }

    /// 이 사이트의 주소인가: 스킴이 `origin`과 같고, 호스트가 목록에 맞고, 사용자 정보·다른 포트가 없다.
    pub fn allows(&self, url: &Url) -> bool {
        let Ok(origin) = Url::parse(&self.origin) else {
            return false;
        };
        url.scheme() == origin.scheme()
            && url.username().is_empty()
            && url.password().is_none()
            && url.port() == origin.port()
            && url
                .host_str()
                .is_some_and(|h| self.hosts.iter().any(|p| host_matches(p, h)))
    }

    /// 로그인 페이지 경로인가.
    pub fn is_login_path(&self, url: &Url) -> bool {
        url.path().starts_with(&self.login.path)
    }

    /// 저장된 비밀번호의 자격 증명 이름(영문·숫자·`-`만 쓸 수 있다). 일반 프로필은 호스트를 읽을 수 있게
    /// 바꾸고 해시 조각을 붙여 겹치지 않게 한다.
    pub fn credential_name(&self) -> String {
        if self.is_builtin() {
            return format!("site-{}", self.id);
        }
        let readable: String = self
            .id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .take(60)
            .collect();
        let hash = Sha256::digest(self.id.as_bytes());
        format!(
            "site-{readable}-{:02x}{:02x}{:02x}{:02x}",
            hash[0], hash[1], hash[2], hash[3]
        )
    }

    /// 사람에게 보일 이름과 출처(예: `퀘이사존 (https://quasarzone.com)`).
    pub fn display(&self) -> String {
        if self.name == self.id && !self.is_builtin() {
            self.origin.clone()
        } else {
            format!("{} ({})", self.name, self.origin)
        }
    }
}

/// 호스트 패턴: 정확히 같거나, `*.x`면 x의 하위 도메인(x 자체는 아님). 대소문자 무시.
pub fn host_matches(pattern: &str, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let pattern = pattern.to_ascii_lowercase();
    match pattern.strip_prefix("*.") {
        Some(base) => host.ends_with(&format!(".{base}")),
        None => host == pattern,
    }
}

/// 내장 사이트(사용자가 고른 첫 사이트: 퀘이사존).
pub fn builtin() -> Vec<SiteProfile> {
    vec![SiteProfile {
        id: "quasarzone".into(),
        name: "퀘이사존".into(),
        origin: "https://quasarzone.com".into(),
        hosts: vec!["quasarzone.com".into(), "*.quasarzone.com".into()],
        login: LoginForm {
            url: "https://quasarzone.com/login".into(),
            path: "/login".into(),
            username: "#login_id".into(),
            password: "#password".into(),
            remember: Some("#login-check".into()),
            submit: Some("#btn-login".into()),
            auto: false,
        },
    }]
}

/// 설정의 사이트 ID들 → 내장 프로필. 모르는 ID는 오류.
pub fn select(ids: &[String]) -> Result<Vec<SiteProfile>, String> {
    let all = builtin();
    ids.iter()
        .map(|id| {
            all.iter()
                .find(|s| &s.id == id)
                .cloned()
                .ok_or_else(|| format!("모르는 사이트 {id:?}(내장: {})", known(&all)))
        })
        .collect()
}

fn known(sites: &[SiteProfile]) -> String {
    sites
        .iter()
        .map(|s| s.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_patterns() {
        assert!(host_matches("quasarzone.com", "quasarzone.com"));
        assert!(host_matches("quasarzone.com", "QuasarZone.com"));
        assert!(!host_matches("quasarzone.com", "img2.quasarzone.com"));
        assert!(host_matches("*.quasarzone.com", "img2.quasarzone.com"));
        assert!(!host_matches("*.quasarzone.com", "quasarzone.com"));
        assert!(
            !host_matches("*.quasarzone.com", "evilquasarzone.com"),
            "점 경계"
        );
        assert!(!host_matches(
            "quasarzone.com",
            "quasarzone.com.evil.example"
        ));
    }

    #[test]
    fn builtin_profile_paths_and_credential_name() {
        let s = &builtin()[0];
        assert!(s.is_builtin());
        assert!(s.allows(&Url::parse("https://img2.quasarzone.com/a.png").unwrap()));
        assert!(!s.allows(&Url::parse("http://quasarzone.com/").unwrap()));
        assert!(!s.allows(&Url::parse("https://quasarzone.com:8443/").unwrap()));
        assert!(s.is_login_path(&Url::parse("https://quasarzone.com/login?nextUrl=%2F").unwrap()));
        assert!(!s.is_login_path(&Url::parse("https://quasarzone.com/bbs/x").unwrap()));
        assert_eq!(s.credential_name(), "site-quasarzone");
        assert!(select(&["quasarzone".into()]).is_ok());
        assert!(select(&["naver".into()]).is_err());
    }

    #[test]
    fn generic_profiles_bind_to_one_https_origin() {
        let s =
            SiteProfile::generic(&Url::parse("https://www.Example.com/member/login#x").unwrap())
                .unwrap();
        assert_eq!(s.id, "www.example.com");
        assert_eq!(s.origin, "https://www.example.com");
        assert_eq!(s.login.url, "https://www.example.com/member/login");
        assert_eq!(s.login.path, "/member/login");
        assert!(s.login.auto && !s.is_builtin());
        assert!(s.allows(&Url::parse("https://www.example.com/a").unwrap()));
        assert!(
            !s.allows(&Url::parse("https://example.com/").unwrap()),
            "정확한 호스트만"
        );
        assert!(!s.allows(&Url::parse("https://evil.www.example.com/").unwrap()));
        let name = s.credential_name();
        assert!(name.starts_with("site-www-example-com-"), "{name}");
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "자격 증명 이름 규칙"
        );
        let other =
            SiteProfile::generic(&Url::parse("https://www-example.com/login").unwrap()).unwrap();
        assert_ne!(
            other.credential_name(),
            name,
            "바꾼 글자가 같아도 겹치지 않는다"
        );
        assert!(SiteProfile::generic(&Url::parse("http://example.com/login").unwrap()).is_err());
        assert!(
            SiteProfile::generic(&Url::parse("https://example.com:8443/login").unwrap()).is_err()
        );
        assert_eq!(s.display(), "https://www.example.com");
        assert_eq!(builtin()[0].display(), "퀘이사존 (https://quasarzone.com)");
    }
}
