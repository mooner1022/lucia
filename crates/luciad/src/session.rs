//! 감시가 페이지를 받는 세 방식(결정 0036): 쿠키 없이 HTTP, 로그인 세션을 빌려 HTTP, 브라우저 감시 탭.
//!
//! - **세션 빌려 읽기**: 로그인된 브라우저에서 그 주소로 갈 쿠키만 꺼내(CDP `Network.getCookies`) 데몬이 HTTP로 읽는다.
//!   - 쿠키는 이 메모리에만 둔다. 디스크·로그·저장소에 남기지 않고, 데몬이 다시 뜨면 비어 처음 쓸 때 다시 꺼낸다.
//!   - 쿠키는 그 호스트(위임이 덮는 범위)로만 붙인다. 넘겨주기가 밖으로 가면 붙이지 않는다.
//!   - 응답이 준 쿠키는 이 상자만 고친다(브라우저에 되쓰지 않는다).
//!   - 브라우저의 사용자 에이전트를 그대로 쓴다.
//! - **다시 받기**(로그인이 풀렸을 때): 브라우저에서 쿠키를 다시 꺼내 보고, 같으면 위임에 로그인이 있고 저장된
//!   로그인이 있을 때만 브라우저로 다시 로그인한다(호스트당 6시간에 한 번, 계정 잠금을 피하려고).
//! - **브라우저 읽기**: 대화용 탭과 따로 둔 감시 탭에서 연다. 다른 조작이 브라우저를 쥐고 있으면 그 회차는 건너뛴다.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use lucia_store::Store;

use crate::browser::logins::{SavedLogin, SiteLogins};
use crate::browser::{BoxFut, Browser, BrowserError};
use crate::watch::{Fetch, FetchError, SafeFetch, Via};

/// 같은 호스트에 위임된 로그인을 다시 하는 간격 [제안].
pub const RELOGIN_GAP_HOURS: i64 = 6;

#[derive(Clone, Default)]
struct Jar {
    cookies: Vec<(String, String)>,
    user_agent: String,
}

impl Jar {
    fn header(&self) -> String {
        self.cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn merge(&mut self, set: &[(String, String)]) {
        for (k, v) in set {
            match self.cookies.iter_mut().find(|(n, _)| n == k) {
                Some(c) => c.1 = v.clone(),
                None => self.cookies.push((k.clone(), v.clone())),
            }
        }
    }
}

pub struct WatchReader {
    plain: Arc<SafeFetch>,
    browser: Option<Arc<dyn Browser>>,
    logins: Option<Arc<SiteLogins>>,
    store: Arc<Mutex<Store>>,
    jars: Mutex<HashMap<String, Jar>>,
    relogins: Mutex<HashMap<String, DateTime<Utc>>>,
}

impl WatchReader {
    pub fn new(
        plain: Arc<SafeFetch>,
        browser: Option<Arc<dyn Browser>>,
        logins: Option<Arc<SiteLogins>>,
        store: Arc<Mutex<Store>>,
    ) -> Self {
        Self {
            plain,
            browser,
            logins,
            store,
            jars: Mutex::default(),
            relogins: Mutex::default(),
        }
    }

    fn host(url: &str) -> Result<String, FetchError> {
        url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            .ok_or_else(|| FetchError::Other("호스트가 없다".into()))
    }

    /// 브라우저에서 그 주소의 쿠키를 꺼낸다.
    async fn export(&self, url: &str) -> Result<Jar, FetchError> {
        let browser = self.browser.as_ref().ok_or_else(|| {
            FetchError::Other("브라우저가 꺼져 있어 로그인 세션을 쓸 수 없다".into())
        })?;
        let got = browser.cookies(url).await.map_err(browser_err)?;
        Ok(Jar {
            cookies: got.cookies,
            user_agent: got.user_agent,
        })
    }

    async fn read_session(&self, url: &str) -> Result<String, FetchError> {
        let got = self.session_request(url, None).await?;
        Ok(crate::watch::decode_body(&got.bytes, &got.content_type))
    }

    /// 로그인 세션으로 GET. `file_max`가 있으면 파일로 받는다(종류를 가리지 않는다, 결정 0057).
    async fn session_request(
        &self,
        url: &str,
        file_max: Option<u64>,
    ) -> Result<crate::watch::FetchedFile, FetchError> {
        let host = Self::host(url)?;
        let cached = self.jars.lock().expect("jars lock").get(&host).cloned();
        let jar = match cached {
            Some(j) => j,
            None => {
                let j = self.export(url).await?;
                self.jars
                    .lock()
                    .expect("jars lock")
                    .insert(host.clone(), j.clone());
                j
            }
        };
        // 쿠키는 그 호스트와, 위임이 덮는 호스트로 넘겨질 때만 붙인다(스킴이 내려가면 붙이지 않는다).
        let grant = {
            let store = self.store.lock().expect("store lock");
            crate::delegation::grant_for(&store, &host, Utc::now()).map(|g| g.host)
        };
        let origin = url::Url::parse(url).map_err(|e| FetchError::Other(e.to_string()))?;
        let header = jar.header();
        let cookie = |hop: &url::Url| {
            (cookie_allowed(&origin, grant.as_deref(), hop) && !header.is_empty())
                .then(|| header.clone())
        };
        let ua = (!jar.user_agent.is_empty()).then_some(jar.user_agent.as_str());
        let got = match file_max {
            Some(max) => self.plain.request_file(url, ua, cookie, max).await?,
            None => self.plain.request_page(url, ua, cookie).await?,
        };
        if !got.set_cookies.is_empty()
            && let Some(j) = self.jars.lock().expect("jars lock").get_mut(&host)
        {
            j.merge(&got.set_cookies);
        }
        Ok(got)
    }

    async fn read_browser(&self, url: &str) -> Result<String, FetchError> {
        let browser = self
            .browser
            .as_ref()
            .ok_or_else(|| FetchError::Other("브라우저가 꺼져 있다".into()))?;
        let (html, arrived) = browser.read_page(url).await.map_err(browser_err)?;
        if url::Url::parse(&arrived).is_ok_and(|u| crate::watch::looks_like_login_url(&u)) {
            return Err(FetchError::Login("로그인 페이지로 넘어감".into()));
        }
        Ok(html)
    }

    async fn do_refresh(&self, url: &str) -> Result<(), FetchError> {
        let host = Self::host(url)?;
        // 1. 브라우저 쪽 세션이 더 새것일 수 있다.
        let fresh = self.export(url).await?;
        let changed = {
            let jars = self.jars.lock().expect("jars lock");
            jars.get(&host)
                .is_none_or(|old| old.cookies != fresh.cookies)
        };
        if changed && !fresh.cookies.is_empty() {
            self.jars.lock().expect("jars lock").insert(host, fresh);
            return Ok(());
        }
        // 2. 위임에 로그인이 있고 저장된 로그인이 있을 때만, 묻지 않고 다시 로그인한다.
        let now = Utc::now();
        let delegated = {
            let store = self.store.lock().expect("store lock");
            // 권한 모드 엄격(결정 0040)이면 위임의 "로그인"을 쓰지 않는다.
            !crate::security::strict(&store)
                && crate::delegation::grant_for(&store, &host, now).is_some_and(|g| g.login)
        };
        let logins = self
            .logins
            .as_ref()
            .ok_or_else(|| FetchError::Login("로그인을 다룰 수 없다".into()))?;
        let site = url::Url::parse(url)
            .ok()
            .and_then(|u| logins.profile_for(&u).ok())
            .ok_or_else(|| FetchError::Login("로그인 사이트를 알 수 없다".into()))?;
        if !delegated || !logins.has_saved(&site.id) {
            return Err(FetchError::Login(
                "로그인을 위임하지 않았거나 저장된 로그인이 없다".into(),
            ));
        }
        {
            let mut tried = self.relogins.lock().expect("relogins lock");
            if tried
                .get(&host)
                .is_some_and(|t| now - *t < Duration::hours(RELOGIN_GAP_HOURS))
            {
                return Err(FetchError::Login(format!(
                    "{RELOGIN_GAP_HOURS}시간 안에 이미 다시 로그인해 봤다"
                )));
            }
            tried.insert(host.clone(), now);
        }
        match logins
            .login_delegated(&site.id, "로그인 위임(페이지 감시)")
            .await
        {
            Ok(SavedLogin::LoggedIn(_)) => {
                let jar = self.export(url).await?;
                self.jars.lock().expect("jars lock").insert(host, jar);
                Ok(())
            }
            Ok(SavedLogin::NeedsCode(_)) => Err(FetchError::Login(
                "다시 로그인하려면 인증 코드가 필요하다".into(),
            )),
            Err(e) => Err(FetchError::Login(format!("다시 로그인하지 못했다: {e}"))),
        }
    }
}

/// 빌린 쿠키를 이 주소(넘겨준 곳 포함)에 붙여도 되는가: 처음 주소와 같은 호스트이거나 위임이 덮는 호스트이고,
/// https에서 꺼낸 쿠키를 http로 보내지 않는다.
pub fn cookie_allowed(origin: &url::Url, grant: Option<&str>, hop: &url::Url) -> bool {
    if origin.scheme() == "https" && hop.scheme() != "https" {
        return false;
    }
    let (Some(o), Some(h)) = (origin.host_str(), hop.host_str()) else {
        return false;
    };
    let (o, h) = (o.to_ascii_lowercase(), h.to_ascii_lowercase());
    h == o || grant.is_some_and(|g| lucia_store::host_covers(g, &h))
}

fn browser_err(e: BrowserError) -> FetchError {
    match e {
        BrowserError::Busy => FetchError::Busy,
        other => FetchError::Other(other.to_string()),
    }
}

impl Fetch for WatchReader {
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<String, String>> {
        self.plain.fetch(url)
    }

    fn fetch_via<'a>(&'a self, url: &'a str, via: Via) -> BoxFut<'a, Result<String, FetchError>> {
        Box::pin(async move {
            match via {
                Via::Plain => self.plain.request(url, None, |_| None).await,
                Via::Session => self.read_session(url).await,
                Via::Browser => self.read_browser(url).await,
            }
        })
    }

    fn download<'a>(
        &'a self,
        url: &'a str,
        via: Via,
        max: u64,
    ) -> BoxFut<'a, Result<crate::watch::FetchedFile, FetchError>> {
        Box::pin(async move {
            match via {
                Via::Plain => self.plain.request_file(url, None, |_| None, max).await,
                Via::Session => self.session_request(url, Some(max)).await,
                Via::Browser => Err(FetchError::Other(
                    "파일은 로그인 없이 또는 로그인 세션으로만 받는다".into(),
                )),
            }
        })
    }

    fn refresh<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<(), FetchError>> {
        Box::pin(self.do_refresh(url))
    }

    fn keep_browser(&self, on: bool) {
        if let Some(b) = &self.browser {
            b.set_keep_alive(on);
        }
    }

    fn browser_running(&self) -> bool {
        self.browser.as_ref().is_some_and(|b| b.running())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::SessionCookies;
    use crate::browser::fake::{FakeBrowser, FakeNotifier, FakeSecrets};
    use crate::browser::logins::LoginsConfig;

    struct Setup {
        reader: WatchReader,
        browser: Arc<FakeBrowser>,
        store: Arc<Mutex<Store>>,
    }

    fn setup(secrets: FakeSecrets) -> Setup {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let browser = Arc::new(FakeBrowser::default());
        let logins = Arc::new(SiteLogins::new(
            browser.clone(),
            Arc::new(secrets),
            store.clone(),
            Arc::new(FakeNotifier::default()),
            None,
            crate::browser::sites::builtin(),
            LoginsConfig {
                form_base: "http://192.168.0.2:8780".into(),
                input_ttl: chrono::Duration::minutes(10),
                approval_ttl: chrono::Duration::minutes(30),
            },
        ));
        let egress = Arc::new(crate::browser::egress::Egress::new(vec![]));
        let plain = Arc::new(SafeFetch {
            proxy: crate::browser::proxy::Proxy::system(egress.clone()),
            egress,
        });
        Setup {
            reader: WatchReader::new(plain, Some(browser.clone()), Some(logins), store.clone()),
            browser,
            store,
        }
    }

    fn session(v: &str) -> SessionCookies {
        SessionCookies {
            cookies: vec![("sid".into(), v.into())],
            user_agent: "Mozilla/5.0 Chrome".into(),
        }
    }

    const URL: &str = "https://quasarzone.com/bbs/qb_saleinfo";

    #[test]
    fn cookies_go_only_to_the_same_host_or_the_grant() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        let origin = u("https://cafe.example.com/board");
        assert!(cookie_allowed(
            &origin,
            None,
            &u("https://cafe.example.com/login?x=1")
        ));
        assert!(
            !cookie_allowed(&origin, None, &u("https://m.example.com/")),
            "위임 없으면 같은 호스트만"
        );
        assert!(cookie_allowed(
            &origin,
            Some("example.com"),
            &u("https://m.example.com/")
        ));
        assert!(!cookie_allowed(
            &origin,
            Some("example.com"),
            &u("https://evil-example.com/")
        ));
        assert!(!cookie_allowed(
            &origin,
            Some("example.com"),
            &u("https://example.com.evil.net/")
        ));
        assert!(
            !cookie_allowed(&origin, None, &u("http://cafe.example.com/board")),
            "https 쿠키를 http로 보내지 않는다"
        );
        let plain = u("http://old.example.com/");
        assert!(cookie_allowed(&plain, None, &u("http://old.example.com/b")));
    }

    /// 로그인이 풀리면: 브라우저 쪽 세션이 새것이면 그것을, 같으면 위임된 로그인(저장된 로그인)을, 6시간에 한 번.
    #[tokio::test]
    async fn refresh_takes_a_newer_session_then_relogs_only_when_delegated() {
        let secrets = FakeSecrets::default();
        secrets
            .0
            .lock()
            .unwrap()
            .insert("site-quasarzone".into(), r#"{"password":"pw"}"#.into());
        let s = setup(secrets);
        s.browser.sessions.lock().unwrap().push_back(session("a"));
        assert!(s.reader.refresh(URL).await.is_ok(), "처음 받은 세션");
        // 같은 세션이고 로그인을 위임하지 않았다.
        let r = s.reader.refresh(URL).await;
        assert!(
            matches!(&r, Err(FetchError::Login(w)) if w.contains("위임")),
            "{r:?}"
        );
        {
            let store = s.store.lock().unwrap();
            store
                .set_setting("browser.site.quasarzone.username", "me", chrono::Utc::now())
                .unwrap();
            store
                .grant_site(
                    &lucia_store::NewSiteGrant {
                        id: "g1",
                        host: "quasarzone.com",
                        login: true,
                        actions: false,
                        purpose: "장터 감시",
                        origin: "admin",
                        expires_at: None,
                    },
                    chrono::Utc::now(),
                )
                .unwrap();
        }
        assert!(s.reader.refresh(URL).await.is_ok(), "위임된 로그인");
        assert!(
            s.browser
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|(k, a)| k == "login" && a == "quasarzone:me")
        );
        let r = s.reader.refresh(URL).await;
        assert!(
            matches!(&r, Err(FetchError::Login(w)) if w.contains("6시간")),
            "{r:?}"
        );
    }

    /// 브라우저 읽기: 로그인 페이지로 넘어가면 로그인 문제, 바쁘면 건너뛰기.
    #[tokio::test]
    async fn browser_reads_report_login_and_busy() {
        let s = setup(FakeSecrets::default());
        {
            let mut reads = s.browser.reads.lock().unwrap();
            reads.push_back(Ok((
                "<input type=password>".into(),
                "https://quasarzone.com/login?next=x".into(),
            )));
            reads.push_back(Ok(("<a href='/bbs/1'>글</a>".into(), URL.into())));
            reads.push_back(Err(BrowserError::Busy));
        }
        assert!(matches!(
            s.reader.fetch_via(URL, Via::Browser).await,
            Err(FetchError::Login(_))
        ));
        assert!(s.reader.fetch_via(URL, Via::Browser).await.is_ok());
        assert_eq!(
            s.reader.fetch_via(URL, Via::Browser).await,
            Err(FetchError::Busy)
        );
        s.reader.keep_browser(true);
        assert!(
            s.browser
                .keep_alive
                .load(std::sync::atomic::Ordering::SeqCst)
        );
    }
}
