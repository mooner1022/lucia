//! 관리 웹 페이지(D8, `docs/decisions/0002-admin-page.md`): 설정과 연결 상태.
//!
//! - 집 LAN과 localhost에만 바인드한다(설정 `admin.bind`). MCP와 다른 포트다.
//! - 로그인이 있어야 열린다. 첫 비밀번호는 `luciad admin-setup`이 터미널에 찍는 일회용 링크로 정한다.
//! - 상태를 바꾸는 폼은 모두 CSRF 토큰을 확인한다. 쿠키는 HttpOnly, SameSite=Strict.
//! - Host 헤더가 바인드 주소가 아니면 거부한다(DNS 리바인딩 방지).
//! - 외부 노출(결정 0043): 믿는 리버스 프록시를 거친 요청은 "외부"다. 공개 Host만 받고, 로그인에 2단계 인증(TOTP)이
//!   필요하며, 쿠키에 `Secure`를 붙이고 HSTS를 보낸다. 초기 설정 링크와 2단계 인증 설정은 집 안에서만 연다.
//! - 비밀 값은 입력만 받는다. 화면에는 "설정됨"만 보이고, 로그에도 남기지 않는다.

mod derive;
mod extra;
#[cfg(test)]
mod fixtures;
mod setup;
mod snapshot;
mod spa;
#[cfg(test)]
mod tests;

pub use spa::{built as ui_built, missing_assets as ui_missing_assets};

pub use snapshot::num;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use chrono::Utc;
use lucia_store::Store;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::check::Check;
use crate::config::Config;
use crate::credentials::Credentials;
use crate::discord_ctl::DiscordControl;
use crate::google_auth::{self, GoogleAuth, PendingConsent};
use crate::public::Origin;
use serde_json::Value;
use snapshot::{
    ActivityFilter, BackupView, DiscordView, Flash, GoogleView, Page, Snapshot, SourceView, Tone,
    back_path,
};
use spa::Standalone;

const COOKIE: &str = "lucia_admin";
const CLEAR_COOKIE: &str = "lucia_admin=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0";
const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);
const MIN_PASSWORD_CHARS: usize = 12;
const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(300);
const CHECKS_TTL: Duration = Duration::from_secs(300);
/// 외부 로그인 실패 전체 한도(결정 0043 [제안]): 한 시간에 이만큼 틀리면 외부 로그인을 한 시간 막고 알린다(여러
/// 주소로 나눠 맞히는 것을 막는다). 집 안 로그인은 막지 않는다.
const EXTERNAL_FAILURES_PER_HOUR: usize = 20;
const EXTERNAL_BLOCK: Duration = Duration::from_secs(3600);
/// 주소별 실패 기록을 이만큼까지만 둔다(인터넷 훑기로 끝없이 늘지 않게).
const MAX_FAILURE_ENTRIES: usize = 4096;
/// 2단계 인증을 설정하는 동안 비밀을 들고 있는 시간.
const TOTP_PENDING: Duration = Duration::from_secs(600);

/// 점검 결과 캐시: (만든 때, 화면에 보일 시각, 결과).
type CachedChecks = (Instant, chrono::DateTime<Utc>, Vec<Check>);

struct Session {
    csrf: String,
    expires: Instant,
    flash: Option<Flash>,
    /// 외부(공개 주소)에서 로그인한 세션. 집 안 요청에는 쓰지 않고, 반대도 그렇다(결정 0043).
    external: bool,
}

#[derive(Default)]
struct Failures {
    count: u32,
    locked_until: Option<Instant>,
    last: Option<Instant>,
}

/// 외부 로그인 실패 전체 기록(결정 0043).
#[derive(Default)]
struct ExternalFailures {
    recent: std::collections::VecDeque<Instant>,
    blocked_until: Option<Instant>,
}

pub struct AdminState {
    store: Arc<Mutex<Store>>,
    discord: Arc<DiscordControl>,
    google: Arc<GoogleAuth>,
    /// 진행 중인 Google 동의(state → PKCE verifier 등).
    consents: Mutex<HashMap<String, PendingConsent>>,
    /// "지금 실행" 버튼: 작업 큐에 넣는다(채팅·루틴과 겹치지 않게).
    jobs: tokio::sync::mpsc::Sender<crate::worker::Job>,
    creds: Arc<Credentials>,
    config: Arc<Config>,
    allowed_hosts: Vec<String>,
    sessions: Mutex<HashMap<String, Session>>,
    failures: Mutex<HashMap<IpAddr, Failures>>,
    checks: tokio::sync::Mutex<Option<CachedChecks>>,
    /// 사이트 로그인(결정 0006). 브라우저를 켰을 때만.
    logins: std::sync::OnceLock<Arc<crate::browser::logins::SiteLogins>>,
    /// 브라우저 도구(결정 0008): 차단 목록, 프록시가 막은 연결.
    browser_tools: std::sync::OnceLock<Arc<crate::browser::tools::BrowserTools>>,
    /// 캐릭터 파일의 호스트 경로(결정 0009). 처음 쓸 때 sbx 마운트에서 찾는다.
    character_path: std::sync::OnceLock<std::path::PathBuf>,
    /// 접속 API(결정 0021): 취소한 클라이언트의 연결을 끊고, 연결 상태를 보인다.
    gateway: std::sync::OnceLock<Arc<crate::gateway::Gateway>>,
    /// 믿는 리버스 프록시(결정 0043).
    proxies: crate::public::Proxies,
    /// 공개 Host(`host[:port]`). 없으면 외부 요청을 받지 않는다.
    public_host: Option<String>,
    /// 2단계 인증을 설정 중인 세션의 비밀(세션 ID → (base32, 시작 시각)).
    totp_pending: Mutex<HashMap<String, (String, Instant)>>,
    /// 마지막으로 받은 TOTP 칸(같은 코드를 다시 쓰지 못하게).
    totp_last: Mutex<Option<i64>>,
    external_failures: Mutex<ExternalFailures>,
    /// 앱 푸시(결정 0043).
    app_push: std::sync::OnceLock<Arc<crate::apppush::AppPush>>,
    /// 주 모델(결정 0055). 데몬이 엔진과 같은 것을 붙인다. 붙이지 않았으면 저장소에서 읽어 만든다.
    models: std::sync::OnceLock<crate::models::MainModel>,
    /// 프로필 그림을 정하는 곳(결정 0064). Lucia의 도구와 같은 것을 붙인다(앱·Discord에 닿는 길이 같다).
    /// 없으면 그림은 올리지 못하고, 이름은 저장소에만 적는다.
    profile: std::sync::OnceLock<crate::profile::ProfileAccess>,
    /// 시험: 화면이 물을 때 시험 데이터로 답한다(없으면 처리기로 넘어간다). 화면 검사(브라우저)가 쓴다.
    #[cfg(test)]
    fixture: std::sync::OnceLock<FixturePages>,
}

#[cfg(test)]
type FixturePages =
    Box<dyn Fn(&AdminState, &HeaderMap, &axum::http::Uri) -> Option<Response> + Send + Sync>;

impl AdminState {
    pub fn new(
        store: Arc<Mutex<Store>>,
        discord: Arc<DiscordControl>,
        google: Arc<GoogleAuth>,
        creds: Arc<Credentials>,
        config: Arc<Config>,
        jobs: tokio::sync::mpsc::Sender<crate::worker::Job>,
    ) -> Arc<Self> {
        let mut allowed_hosts = Vec::new();
        for addr in &config.admin.bind {
            allowed_hosts.push(addr.to_string());
            if addr.ip().is_loopback() {
                allowed_hosts.push(format!("localhost:{}", addr.port()));
            }
        }
        Arc::new(Self {
            store,
            discord,
            google,
            consents: Mutex::default(),
            jobs,
            creds,
            allowed_hosts,
            sessions: Mutex::default(),
            failures: Mutex::default(),
            checks: tokio::sync::Mutex::new(None),
            logins: std::sync::OnceLock::new(),
            browser_tools: std::sync::OnceLock::new(),
            character_path: std::sync::OnceLock::new(),
            gateway: std::sync::OnceLock::new(),
            proxies: config.public.proxies(),
            public_host: config.public.admin_host(),
            totp_pending: Mutex::default(),
            totp_last: Mutex::new(None),
            external_failures: Mutex::default(),
            app_push: std::sync::OnceLock::new(),
            models: std::sync::OnceLock::new(),
            profile: std::sync::OnceLock::new(),
            #[cfg(test)]
            fixture: std::sync::OnceLock::new(),
            config,
        })
    }

    /// 브라우저를 켰으면 사이트 로그인을 붙인다(입력 링크, 저장된 로그인 지우기, 상태).
    pub fn set_logins(&self, logins: Arc<crate::browser::logins::SiteLogins>) {
        let _ = self.logins.set(logins);
    }

    pub fn set_gateway(&self, gateway: Arc<crate::gateway::Gateway>) {
        let _ = self.gateway.set(gateway);
    }

    pub fn set_app_push(&self, push: Arc<crate::apppush::AppPush>) {
        let _ = self.app_push.set(push);
    }

    /// 엔진이 쓰는 주 모델을 붙인다(결정 0055). 여기서 바꾸면 다음 실행부터 엔진이 그 모델을 쓴다.
    pub fn set_models(&self, models: crate::models::MainModel) {
        let _ = self.models.set(models);
    }

    /// 프로필 그림을 정하는 곳을 붙인다(결정 0064). 소유자가 올린 그림도 Lucia의 도구와 같은 길로 앱과 Discord에 닿는다.
    pub fn set_profile(&self, profile: crate::profile::ProfileAccess) {
        let _ = self.profile.set(profile);
    }

    /// 로그인한 뒤 처음 보여 줄 화면(결정 0064): 대화할 곳이 없고 초기 설정 안내를 끄지 않았으면 초기 설정, 아니면 홈.
    fn landing(&self) -> &'static str {
        let (token, owner, app, dismissed) = {
            let store = self.store();
            (
                self.creds
                    .exists(crate::credentials::names::DISCORD_BOT_TOKEN),
                store
                    .setting(lucia_store::keys::OWNER_DISCORD_USER_ID)
                    .ok()
                    .flatten()
                    .is_some(),
                crate::gateway::notify_client(&store).is_some(),
                setup::dismissed(&store),
            )
        };
        setup::landing(setup::channel_ready(token, owner, app), dismissed)
    }

    fn models(&self) -> crate::models::MainModel {
        self.models
            .get_or_init(|| {
                let models = crate::models::MainModel::new(
                    self.config.models.chat.clone(),
                    self.config.models.briefing.clone(),
                );
                models.load(&self.store());
                models
            })
            .clone()
    }

    /// 캐릭터 파일(결정 0009). 경로는 처음 쓸 때 상주 샌드박스의 마운트에서 찾는다.
    async fn character_file(&self) -> Result<crate::character::CharacterFile, String> {
        if let Some(p) = self.character_path.get() {
            return Ok(crate::character::CharacterFile::new(p.clone()));
        }
        let vm = self
            .config
            .sandbox
            .character_file
            .as_deref()
            .ok_or("설정 sandbox.character_file이 없다")?;
        let path =
            crate::character::resolve(&self.config.sandbox.sbx, &self.config.sandbox.name, vm)
                .await?;
        let _ = self.character_path.set(path.clone());
        Ok(crate::character::CharacterFile::new(path))
    }

    /// 시험용: 캐릭터 파일 경로를 정해 둔다.
    #[cfg(test)]
    pub fn set_character_path(&self, path: std::path::PathBuf) {
        let _ = self.character_path.set(path);
    }

    /// 브라우저를 켰으면 차단 목록과 프록시 기록을 붙인다.
    pub fn set_browser_tools(&self, tools: Arc<crate::browser::tools::BrowserTools>) {
        let _ = self.browser_tools.set(tools);
    }

    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().expect("store lock")
    }

    /// 주기 점검 결과를 넣어 둔다(첫 화면이 점검을 기다리지 않게).
    pub async fn store_checks(&self, checks: Vec<Check>) {
        *self.checks.lock().await = Some((Instant::now(), Utc::now(), checks));
    }
}

/// 무작위 토큰(약 244비트).
fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    Ok(Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow::anyhow!("비밀번호 해시: {e}"))?
        .to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

/// 초기 설정 링크를 만든다(비밀번호가 없을 때만). 터미널에 찍을 URL들을 돌려준다.
pub fn create_setup_link(
    store: &Store,
    config: &Config,
    reset: bool,
) -> anyhow::Result<Vec<String>> {
    if store.admin_password_hash()?.is_some() && !reset {
        anyhow::bail!("이미 비밀번호가 있다. 잊었으면 `luciad admin-setup --reset`");
    }
    let token = random_token();
    store.set_admin_setup(
        &sha256_hex(&token),
        Utc::now() + chrono::Duration::minutes(30),
    )?;
    Ok(config
        .admin
        .bind
        .iter()
        .map(|addr| format!("http://{addr}/setup?token={token}"))
        .collect())
}

/// 비밀번호를 지운다(`--reset`). 그 뒤로는 설정 링크로만 다시 정할 수 있다.
pub fn reset_password(store: &Store) -> anyhow::Result<()> {
    store.clear_admin_password()?;
    Ok(())
}

// ---------- 미들웨어 ----------

async fn guard(State(state): State<Arc<AdminState>>, mut req: Request, next: Next) -> Response {
    // 누가 보냈나(결정 0043): 믿는 프록시를 거친 요청만 외부다. 소켓 정보가 없으면(시험) 루프백으로 본다.
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
        .unwrap_or(IpAddr::from([127, 0, 0, 1]));
    let origin = match crate::public::resolve(peer, req.headers(), &state.proxies) {
        Ok(origin) => origin,
        Err(why) => {
            tracing::warn!(%peer, why, "관리: 프록시 요청을 거절했다");
            return (StatusCode::BAD_REQUEST, "출처를 알 수 없는 요청").into_response();
        }
    };
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let host_ok = match origin {
        Origin::Local(_) => state.allowed_hosts.iter().any(|h| h == host),
        // 외부는 공개 Host 하나만(공개 주소를 정하지 않았으면 외부 요청을 받지 않는다).
        Origin::External(_) => state
            .public_host
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case(host)),
    };
    if !host_ok {
        return (StatusCode::BAD_REQUEST, "허용되지 않은 Host").into_response();
    }
    // 집 안에서 만든 세션을 밖에서(또는 반대로) 쓰지 않는다. 쿠키는 호스트마다 따로라 보통은 일어나지 않는다.
    if let Some(id) = session_id(req.headers()) {
        let mismatch = state
            .sessions
            .lock()
            .expect("sessions lock")
            .get(&id)
            .is_some_and(|s| s.external != origin.external());
        if mismatch {
            // 화면이 물은 것이면 다른 로그아웃된 경우와 같은 꼴로 답한다(화면은 리다이렉트를 따라가지 않는다).
            let mut response = if spa::wants_json(req.headers()) {
                (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(serde_json::json!({ "redirect": "/login", "flash": null })),
                )
                    .into_response()
            } else {
                Redirect::to("/login").into_response()
            };
            response
                .headers_mut()
                .insert(header::SET_COOKIE, HeaderValue::from_static(CLEAR_COOKIE));
            return response;
        }
    }
    req.extensions_mut().insert(origin);
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    if origin.external() {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    if !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    for (name, value) in [
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    // 화면이 같은 출처의 스크립트 파일만 쓰게 되어(결정 0061) 결정 0042에서 뺐던 CSP를 다시 건다.
    headers.insert(header::CONTENT_SECURITY_POLICY, spa::CSP);
    response
}

fn session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_owned())
}

/// 로그인한 세션이면 (세션 ID, CSRF, 일회성 안내)를 돌려준다.
fn current_session(
    state: &AdminState,
    headers: &HeaderMap,
) -> Option<(String, String, Option<Flash>)> {
    let id = session_id(headers)?;
    let mut sessions = state.sessions.lock().expect("sessions lock");
    sessions.retain(|_, s| s.expires > Instant::now());
    let session = sessions.get_mut(&id)?;
    Some((id, session.csrf.clone(), session.flash.take()))
}

/// 로그인돼 있는가(알림은 꺼내지 않는다: 이어서 `show`가 꺼내 보인다).
fn logged_in(state: &AdminState, headers: &HeaderMap) -> bool {
    let Some(id) = session_id(headers) else {
        return false;
    };
    state
        .sessions
        .lock()
        .expect("sessions lock")
        .get(&id)
        .is_some_and(|s| s.expires > Instant::now())
}

fn flash(state: &AdminState, id: &str, tone: Tone, text: String) {
    if let Some(s) = state.sessions.lock().expect("sessions lock").get_mut(id) {
        s.flash = Some(Flash {
            tone,
            text: crate::mask::global().mask(&text),
        });
    }
}

/// 결과에 따라 안내의 결을 정한다.
fn tone_of<T, E>(result: &Result<T, E>) -> Tone {
    if result.is_ok() { Tone::Ok } else { Tone::Bad }
}

/// 상태를 바꾸는 요청이 거부된 이유.
enum Denied {
    Login,
    Csrf,
}

impl IntoResponse for Denied {
    fn into_response(self) -> Response {
        match self {
            Denied::Login => Redirect::to("/login").into_response(),
            Denied::Csrf => (StatusCode::FORBIDDEN, "CSRF 토큰이 맞지 않는다").into_response(),
        }
    }
}

/// 상태를 바꾸는 요청: 로그인과 CSRF를 확인한다.
fn authorize(state: &AdminState, headers: &HeaderMap, csrf: &str) -> Result<String, Denied> {
    match current_session(state, headers) {
        Some((id, expected, _)) if expected == csrf => Ok(id),
        Some(_) => Err(Denied::Csrf),
        None => Err(Denied::Login),
    }
}

// ---------- 초기 설정, 로그인 ----------

#[derive(Deserialize)]
struct SetupQuery {
    token: Option<String>,
}

async fn setup_form(
    State(state): State<Arc<AdminState>>,
    Extension(origin): Extension<Origin>,
    Query(q): Query<SetupQuery>,
) -> Response {
    // 초기 설정 링크는 서버 터미널에서 받은 사람의 것이다. 밖에서는 열지 않는다(결정 0043).
    if origin.external() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let token = q.token.unwrap_or_default();
    let valid = state
        .store()
        .admin_setup_valid(&sha256_hex(&token), Utc::now())
        .unwrap_or(false);
    if !valid {
        return Standalone::SetupInvalid.respond();
    }
    Standalone::Setup {
        token,
        error: None,
        min_chars: MIN_PASSWORD_CHARS,
    }
    .respond()
}

#[derive(Deserialize)]
struct SetupForm {
    token: String,
    password: String,
    confirm: String,
}

async fn setup_submit(
    State(state): State<Arc<AdminState>>,
    Extension(origin): Extension<Origin>,
    Form(f): Form<SetupForm>,
) -> Response {
    if origin.external() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let valid = state
        .store()
        .admin_setup_valid(&sha256_hex(&f.token), Utc::now())
        .unwrap_or(false);
    if !valid {
        return (StatusCode::FORBIDDEN, "초기 설정 링크가 유효하지 않다").into_response();
    }
    if f.password != f.confirm {
        return Standalone::Setup {
            token: f.token,
            error: Some("두 비밀번호가 다릅니다. 다시 입력하세요.".into()),
            min_chars: MIN_PASSWORD_CHARS,
        }
        .respond();
    }
    if f.password.chars().count() < MIN_PASSWORD_CHARS {
        return Standalone::Setup {
            token: f.token,
            error: Some(format!(
                "비밀번호는 {MIN_PASSWORD_CHARS}자 이상이어야 합니다."
            )),
            min_chars: MIN_PASSWORD_CHARS,
        }
        .respond();
    }
    let hash = match tokio::task::spawn_blocking(move || hash_password(&f.password)).await {
        Ok(Ok(hash)) => hash,
        _ => return (StatusCode::INTERNAL_SERVER_ERROR, "비밀번호 해시 실패").into_response(),
    };
    if state
        .store
        .lock()
        .expect("store lock")
        .set_admin_password_hash(&hash, Utc::now())
        .is_err()
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, "저장 실패").into_response();
    }
    tracing::info!("관리: 비밀번호를 정했다");
    Redirect::to("/login").into_response()
}

async fn login_form(
    State(state): State<Arc<AdminState>>,
    Extension(origin): Extension<Origin>,
    headers: HeaderMap,
) -> Response {
    if current_session(&state, &headers).is_some() {
        return Redirect::to(state.landing()).into_response();
    }
    if state.store().admin_password_hash().ok().flatten().is_none() {
        return Standalone::NoPassword.respond();
    }
    let external = origin.external();
    if external && !state.creds.exists(crate::totp::CREDENTIAL) {
        return login_screen(Some(NO_TOTP_OUTSIDE), true).respond();
    }
    login_screen(None, external).respond()
}

fn login_screen(error: Option<&str>, external: bool) -> Standalone {
    Standalone::Login {
        error: error.map(str::to_owned),
        external,
    }
}

const NO_TOTP_OUTSIDE: &str = "밖에서 로그인하려면 2단계 인증이 필요합니다. 집 네트워크에서 관리 페이지 설정 화면의 \"2단계 인증\"을 먼저 켜세요.";

#[derive(Deserialize)]
struct LoginForm {
    password: String,
    /// 2단계 인증 코드(외부 로그인만, 결정 0043).
    #[serde(default)]
    code: Option<String>,
}

/// 주소별 실패를 센다. 기록이 너무 많으면 오래된 것부터 버린다(인터넷 훑기로 끝없이 늘지 않게).
fn note_failure(map: &mut HashMap<IpAddr, Failures>, ip: IpAddr) {
    let now = Instant::now();
    if map.len() >= MAX_FAILURE_ENTRIES && !map.contains_key(&ip) {
        map.retain(|_, f| {
            f.locked_until.is_some_and(|u| u > now)
                || f.last.is_some_and(|l| now.duration_since(l) < LOCKOUT)
        });
        if map.len() >= MAX_FAILURE_ENTRIES
            && let Some(oldest) = map.iter().min_by_key(|(_, f)| f.last).map(|(k, _)| *k)
        {
            map.remove(&oldest);
        }
    }
    let entry = map.entry(ip).or_default();
    entry.count += 1;
    entry.last = Some(now);
    if entry.count >= MAX_FAILURES {
        entry.locked_until = Some(now + LOCKOUT);
        entry.count = 0;
    }
}

impl AdminState {
    /// 외부 로그인이 전체 한도로 막혀 있나.
    fn external_blocked(&self) -> bool {
        let mut f = self
            .external_failures
            .lock()
            .expect("external failures lock");
        match f.blocked_until {
            Some(until) if until > Instant::now() => true,
            Some(_) => {
                f.blocked_until = None;
                f.recent.clear();
                false
            }
            None => false,
        }
    }

    /// 외부 로그인 실패를 센다. 한도를 막 넘었으면 `true`(알릴 때).
    fn note_external_failure(&self) -> bool {
        let mut f = self
            .external_failures
            .lock()
            .expect("external failures lock");
        let now = Instant::now();
        while f
            .recent
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(3600))
        {
            f.recent.pop_front();
        }
        f.recent.push_back(now);
        if f.recent.len() >= EXTERNAL_FAILURES_PER_HOUR && f.blocked_until.is_none() {
            f.blocked_until = Some(now + EXTERNAL_BLOCK);
            return true;
        }
        false
    }

    /// 소유자 알림 대화로 한 줄 보낸다(외부 로그인, 보안 설정 변경).
    async fn notify_owner(&self, body: String) {
        use lucia_channel::ChannelAdapter as _;
        let (app, target) = {
            let store = self.store();
            (
                crate::gateway::notify_client(&store),
                store
                    .setting(lucia_store::keys::NOTIFY_CONVERSATION)
                    .ok()
                    .flatten(),
            )
        };
        // 알림 받을 곳이 앱(앱 모두 포함, 결정 0066)이고 닿을 수 있으면 앱으로(결정 0043), 아니면 Discord DM으로.
        if let (Some(client), Some(gw)) = (app, self.gateway.get())
            && gw.reachable(&client)
            && gw
                .send(lucia_core::Outbound::Text {
                    conversation: crate::gateway::conversation(&client),
                    body: body.clone(),
                })
                .await
                .is_ok()
        {
            return;
        }
        if let Some(conversation) = target {
            use lucia_channel::ChannelAdapter;
            let hub = crate::discord_ctl::HubChannel::new(self.discord.clone());
            let _ = hub
                .send(lucia_core::Outbound::Text {
                    conversation: lucia_core::ConversationId(conversation),
                    body,
                })
                .await;
        }
    }
}

async fn login_submit(
    State(state): State<Arc<AdminState>>,
    Extension(origin): Extension<Origin>,
    Form(f): Form<LoginForm>,
) -> Response {
    let ip = origin.ip();
    let external = origin.external();
    let too_many =
        |text: &str| login_screen(Some(text), external).respond_with(StatusCode::TOO_MANY_REQUESTS);
    if external && state.external_blocked() {
        return too_many(
            "밖에서의 로그인 실패가 많아 한 시간 동안 막았습니다. 집 네트워크에서는 로그인할 수 있습니다.",
        );
    }
    {
        let failures = state.failures.lock().expect("failures lock");
        if let Some(until) = failures.get(&ip).and_then(|f| f.locked_until)
            && until > Instant::now()
        {
            return too_many("로그인 시도가 너무 많습니다. 5분 뒤 다시 하세요.");
        }
    }
    let Some(hash) = state.store().admin_password_hash().ok().flatten() else {
        return Redirect::to("/login").into_response();
    };
    // 밖에서는 2단계 인증이 있어야 한다. 없으면 비밀번호를 보지도 않는다(맞는지 알려 주지 않게).
    let totp_secret = if external {
        match state.creds.read(crate::totp::CREDENTIAL).await {
            Ok(Some(secret)) => Some(secret),
            _ => {
                return login_screen(Some(NO_TOTP_OUTSIDE), true)
                    .respond_with(StatusCode::FORBIDDEN);
            }
        }
    } else {
        None
    };
    let password = f.password;
    let mut ok = tokio::task::spawn_blocking(move || verify_password(&password, &hash))
        .await
        .unwrap_or(false);
    if ok && let Some(secret) = &totp_secret {
        let code = f.code.unwrap_or_default();
        let mut last = state.totp_last.lock().expect("totp lock");
        match crate::totp::verify(secret, &code, Utc::now().timestamp(), *last) {
            Some(step) => *last = Some(step),
            None => ok = false,
        }
    }
    if !ok {
        note_failure(&mut state.failures.lock().expect("failures lock"), ip);
        let alarm = external && state.note_external_failure();
        tracing::warn!(%ip, external, "관리: 로그인 실패");
        if alarm {
            let _ = state.store().add_activity(
                "admin_login",
                "밖에서의 관리 페이지 로그인 실패가 한 시간에 한도를 넘어 외부 로그인을 한 시간 막았다",
                None,
                Utc::now(),
            );
            state
                .notify_owner(format!(
                    "⚠️ 관리 페이지에 밖에서 로그인하려는 실패가 한 시간에 {EXTERNAL_FAILURES_PER_HOUR}번을 넘어 외부 로그인을 한 시간 막았습니다(마지막 주소 {ip}). 집 네트워크에서는 로그인할 수 있습니다."
                ))
                .await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let text = if external {
            "비밀번호나 인증 코드가 틀렸습니다."
        } else {
            "비밀번호가 틀렸습니다."
        };
        return login_screen(Some(text), external).respond_with(StatusCode::UNAUTHORIZED);
    }
    state.failures.lock().expect("failures lock").remove(&ip);
    let id = random_token();
    state.sessions.lock().expect("sessions lock").insert(
        id.clone(),
        Session {
            csrf: random_token(),
            expires: Instant::now() + SESSION_TTL,
            flash: None,
            external,
        },
    );
    // 밖에서는 HTTPS(프록시가 TLS를 맡는다)이므로 Secure를 붙인다. 집 안은 평문 HTTP라 붙이지 않는다(결정 0002).
    let cookie = format!(
        "{COOKIE}={id}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{}",
        SESSION_TTL.as_secs(),
        if external { "; Secure" } else { "" }
    );
    // 첫 화면: 대화할 곳이 아직 없으면 초기 설정으로(결정 0064). 정해 둔 곳이 따로 없으니 이것이 기본 첫 화면이다.
    let mut response = Redirect::to(state.landing()).into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    tracing::info!(%ip, external, "관리: 로그인");
    if external {
        let _ = state.store().add_activity(
            "admin_login",
            &format!("관리 페이지 외부 로그인({ip})"),
            None,
            Utc::now(),
        );
        state
            .notify_owner(format!(
                "🔑 관리 페이지에 밖에서 로그인했습니다(주소 {ip}). 본인이 아니면 집 네트워크에서 비밀번호와 2단계 인증을 바꾸세요."
            ))
            .await;
    }
    response
}

#[derive(Deserialize)]
struct CsrfForm {
    csrf: String,
    /// 처리 뒤 돌아갈 화면([`back_path`]가 정해진 목록으로 거른다).
    #[serde(default)]
    back: Option<String>,
}

async fn logout(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    if let Ok(id) = authorize(&state, &headers, &f.csrf) {
        state.sessions.lock().expect("sessions lock").remove(&id);
    }
    let mut response = Redirect::to("/login").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, HeaderValue::from_static(CLEAR_COOKIE));
    response
}

// ---------- 상태 화면 ----------

async fn cached_checks(
    state: &Arc<AdminState>,
    refresh: bool,
) -> (Vec<Check>, Option<chrono::DateTime<Utc>>) {
    let mut cache = state.checks.lock().await;
    if !refresh
        && let Some((at, when, checks)) = cache.as_ref()
        && at.elapsed() < CHECKS_TTL
    {
        return (checks.clone(), Some(*when));
    }
    let config = state.config.clone();
    let checks = tokio::task::spawn_blocking(move || crate::check::run_checks(&config))
        .await
        .unwrap_or_default();
    let now = Utc::now();
    *cache = Some((Instant::now(), now, checks.clone()));
    (checks, Some(now))
}

fn last_run(store: &Store, action: &str) -> Option<crate::worker::LastRun> {
    store
        .setting(&crate::worker::last_run_key(action))
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
}

/// 화면에 쓸 데이터를 모은다.
async fn gather(state: &Arc<AdminState>) -> Snapshot {
    use crate::worker::actions;
    let tz = state.config.tz().unwrap_or(chrono_tz::Asia::Seoul);
    let now = Utc::now();
    let (checks, checks_at) = cached_checks(state, false).await;
    let fcm_project = match state.app_push.get() {
        Some(p) => p.project().await,
        None => None,
    };
    let consent_minutes_left = state
        .consents
        .lock()
        .expect("consents lock")
        .values()
        .map(|p| p.started.elapsed())
        .filter(|e| *e < google_auth::CONSENT_TTL)
        .min()
        .map(|e| ((google_auth::CONSENT_TTL - e).as_secs() / 60 + 1) as i64);
    // 브라우저 상태는 저장소 잠금을 잡기 전에 모은다: `SiteLogins::status`가 같은 저장소를 잠그므로,
    // 잠금을 쥔 채 부르면 교착된다(2026-11-17 관리 페이지가 멈춘 원인).
    let tools = state.browser_tools.get();
    let browser = state.logins.get().map(|l| snapshot::BrowserView {
        sandbox: state.config.browser.sandbox.clone(),
        running: l.browser().running(),
        idle_minutes: state.config.browser.idle_minutes,
        sites: l.status(),
        blocked: tools.map(|t| t.egress().user()).unwrap_or_default(),
        recent_blocks: tools
            .and_then(|t| t.proxy())
            .map(|p| p.recent_blocks(8))
            .unwrap_or_default(),
    });
    // 봇이 들어간 서버(결정 0024): 저장소를 잠그기 전에, 오래 걸리면 건너뛴다(5분 동안 기억한다).
    let servers = if state.discord.rest().is_some() {
        match tokio::time::timeout(Duration::from_secs(3), state.discord.servers(false)).await {
            Ok(r) => Some(r),
            Err(_) => Some(Err("Discord가 늦게 답해 건너뛰었습니다".to_owned())),
        }
    } else {
        None
    };
    let store = state.store();
    let local_midnight = now
        .with_timezone(&tz)
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|d| d.and_local_timezone(tz).single())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or(now - chrono::Duration::hours(24));
    let setting = |key: &str| store.setting(key).ok().flatten();
    Snapshot {
        browser,
        now,
        tz,
        discord: DiscordView {
            state: state.discord.state(),
            token_set: state
                .creds
                .exists(crate::credentials::names::DISCORD_BOT_TOKEN),
            owner: setting(lucia_store::keys::OWNER_DISCORD_USER_ID),
            notify: setting(lucia_store::keys::NOTIFY_CONVERSATION),
            guild_calls: crate::discord_ctl::guild_calls_on(&store),
            prefix: setting(lucia_store::keys::DISCORD_PREFIX),
            content_refused: state.discord.content_refused(),
            servers,
        },
        google: GoogleView {
            has_client: state.google.has_client(),
            connected: state.google.connected(),
            account: setting(lucia_store::keys::GOOGLE_ACCOUNT),
            scopes: setting(lucia_store::keys::GOOGLE_SCOPES),
            consent_minutes_left,
            sources: [
                (lucia_precheck::google::CALENDAR_SOURCE, "캘린더"),
                (lucia_precheck::google::GMAIL_SOURCE, "메일"),
            ]
            .into_iter()
            .map(|(name, label)| SourceView {
                label,
                state: store.source_state(name).ok().flatten(),
            })
            .collect(),
        },
        checks,
        checks_at,
        today: store.spend_between(local_midnight, now).unwrap_or_default(),
        week: store
            .spend_between(local_midnight - chrono::Duration::days(6), now)
            .unwrap_or_default(),
        // 후속 조치 스케줄은 "Lucia가 약속한 일" 카드에 따로 보인다.
        schedules: store
            .enabled_schedules()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.action != crate::autonomy::FOLLOWUP_ACTION)
            .collect(),
        session: store
            .active_session(lucia_store::SessionRole::Resident)
            .ok()
            .flatten(),
        memories: store.memories().unwrap_or_default(),
        last_remember: last_run(&store, crate::worker::actions::REMEMBER),
        compact_at_tokens: state
            .config
            .session
            .compaction
            .then_some(state.config.session.compact_context_tokens),
        rotate_tokens: state.config.session.rotate_context_tokens,
        backup: BackupView {
            enabled: state.config.backup.enabled,
            cron: state.config.backup.cron.clone(),
            keep_days: state.config.backup.keep_days,
            last: setting(lucia_store::keys::BACKUP_LAST),
        },
        alerts: store.recent_alerts(15).unwrap_or_default(),
        approvals: store.recent_approvals(15).unwrap_or_default(),
        activity: store.recent_activity(80).unwrap_or_default(),
        approval_ttl_hours: state.config.sandbox.approval_ttl_hours,
        last_precheck: last_run(&store, actions::PRECHECK),
        last_briefing: last_run(&store, actions::MORNING_BRIEFING),
        last_backup: last_run(&store, actions::SESSION_BACKUP),
        last_compact: last_run(&store, actions::COMPACT),
        followups_open: store.open_followups().unwrap_or_default(),
        goals: store
            .goals(true)
            .unwrap_or_default()
            .into_iter()
            .take(10)
            .map(|g| crate::admin::snapshot::GoalView {
                log: store.goal_log(&g.id, 8).unwrap_or_default(),
                children: store.goal_children(&g.id).unwrap_or_default(),
                goal: g,
            })
            .collect(),
        followups_closed: store.closed_followups(5).unwrap_or_default(),
        tasks_open: store.open_tasks().unwrap_or_default(),
        tasks_closed: store.closed_tasks(5).unwrap_or_default(),
        daily_cap: crate::autonomy::cap(&store, crate::autonomy::keys::DAILY_USD),
        weekly_cap: crate::autonomy::cap(&store, crate::autonomy::keys::WEEKLY_USD),
        quiet_hours: crate::autonomy::fixed_quiet_hours(&store).map(|q| q.display()),
        quiet_calendar: store
            .setting(crate::autonomy::keys::QUIET_CALENDAR)
            .ok()
            .flatten()
            .as_deref()
            == Some("on"),
        quiet_except: store
            .setting(crate::autonomy::keys::QUIET_CALENDAR_EXCEPT)
            .ok()
            .flatten()
            .unwrap_or_default(),
        quiet_busy: crate::autonomy::busy_intervals(&store)
            .iter()
            .filter(|(_, e)| *e > Utc::now())
            .count(),
        notice_rules: store.notice_rules().unwrap_or_default(),
        nudges: store.open_nudges().unwrap_or_default(),
        routine_wakes: store
            .setting(crate::autonomy::keys::ROUTINE_WAKES)
            .ok()
            .flatten(),
        dev_cost_cap: store
            .setting(crate::dev::keys::COST_CAP_USD)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok()),
        dev_allow_hosts: store
            .setting(crate::dev::keys::ALLOW_HOSTS)
            .ok()
            .flatten()
            .unwrap_or_default(),
        dev_local_roots: store
            .setting(crate::dev::keys::LOCAL_ROOTS)
            .ok()
            .flatten()
            .unwrap_or_default(),
        clients: store
            .clients()
            .unwrap_or_default()
            .into_iter()
            .map(|c| {
                let online =
                    c.revoked_at.is_none() && state.gateway.get().is_some_and(|g| g.online(&c.id));
                (c, online)
            })
            .collect(),
        gateway_binds: state.config.gateway.bind.clone(),
        access: crate::admin::snapshot::AccessView {
            admin_url: state.config.public.admin_url.clone(),
            gateway_url: state.config.public.gateway_url.clone(),
            totp_enabled: state.creds.exists(crate::totp::CREDENTIAL),
            totp_pending: None,
            session_external: false,
            external_blocked: state.external_blocked(),
        },
        expressions: expressions_of(state).map(|x| x.catalog()),
        images: crate::admin::snapshot::ImagesView {
            key_set: state.creds.exists(crate::imagegen::CREDENTIAL),
            daily_cap: crate::imagegen::daily_cap(&store),
            today_usd: crate::imagegen::spent_today(&store, tz, now),
            recent: store.recent_generated_images(5).unwrap_or_default(),
        },
        app: crate::admin::snapshot::AppView {
            notify_client: crate::gateway::notify_client(&store),
            push_preview: store
                .setting(crate::gateway::keys::PUSH_PREVIEW)
                .ok()
                .flatten()
                .is_some_and(|v| v == "1"),
            fcm_ready: state.gateway.get().is_some_and(|g| g.push_ready()),
            fcm_project,
        },
        profile: snapshot::ProfileView {
            name: crate::profile::display_name(&store),
            name_set: setting(crate::profile::NAME_KEY).is_some_and(|n| !n.trim().is_empty()),
            avatar: crate::profile::stored_avatar(&store),
        },
        setup: snapshot::SetupView {
            dismissed: setup::dismissed(&store),
            config_path: state
                .config
                .source
                .as_ref()
                .map(|p| p.display().to_string()),
        },
        routines: store
            .routines()
            .unwrap_or_default()
            .into_iter()
            .map(|r| crate::admin::snapshot::RoutineView {
                next: store
                    .schedule(&r.id)
                    .ok()
                    .flatten()
                    .filter(|s| s.enabled)
                    .and_then(|s| s.next_due_at),
                runs: store.routine_runs(&r.id, 5).unwrap_or_default(),
                routine: r,
            })
            .collect(),
        scripts_sandbox: state
            .config
            .scripts
            .enabled
            .then(|| state.config.scripts.sandbox.clone()),
        site_grants: store.site_grants().unwrap_or_default(),
        kept_files: store.kept_files().unwrap_or_default(),
        egress_block: crate::leak::blocking(&store),
        signals: {
            use crate::signals::{self as sig, keys};
            let today = Utc::now()
                .with_timezone(&state.config.tz().unwrap_or(chrono_tz::Asia::Seoul))
                .format("%Y-%m-%d")
                .to_string();
            crate::admin::snapshot::SignalsView {
                enabled: sig::enabled(&store),
                consistency: crate::consistency::enabled(&store),
                weekly: crate::reports::enabled(&store, crate::reports::WEEKLY_KEY),
                evening: crate::reports::enabled(&store, crate::reports::EVENING_KEY),
                poll_minutes: crate::events::poll_minutes(&store),
                interests: store
                    .setting(keys::INTERESTS)
                    .ok()
                    .flatten()
                    .unwrap_or_default(),
                classify_daily: sig::daily_cap(
                    &store,
                    keys::CLASSIFY_DAILY,
                    sig::CLASSIFY_DAILY_DEFAULT,
                ),
                wake_daily: sig::daily_cap(&store, keys::WAKE_DAILY, sig::WAKE_DAILY_DEFAULT),
                classify_used: sig::used_today(&store, keys::CLASSIFY_COUNT, &today),
                wake_used: sig::used_today(&store, keys::WAKE_COUNT, &today),
                recent: store.recent_signals(15).unwrap_or_default(),
            }
        },
        watches: store
            .watches()
            .unwrap_or_default()
            .into_iter()
            .map(|w| {
                let state = store
                    .source_state(&format!("{}{}", crate::watch::SOURCE_PREFIX, w.id))
                    .ok()
                    .flatten();
                (w, state)
            })
            .collect(),
        watch_now_daily: crate::watch::now_daily(&store),
        security_mode: crate::security::mode(&store),
        keep_days: store
            .setting(crate::engine::wrapup::keys::KEEP_DAYS)
            .ok()
            .flatten()
            .and_then(|v| v.trim().parse().ok()),
        task_channel: store
            .setting(crate::tasks::CHANNEL_KEY)
            .ok()
            .flatten()
            .and_then(|c| {
                lucia_channel::discord::channel_of(&lucia_core::ConversationId(c))
                    .map(|n| n.to_string())
            }),
    }
}

/// 로그인한 화면 하나의 데이터를 답한다(결정 0061: 그리는 것은 브라우저의 화면이다). 로그인하지 않았으면
/// 로그인으로 가라고 답한다. `extra`는 화면마다 다른 덧붙임이다.
async fn show(state: &Arc<AdminState>, headers: &HeaderMap, page: Page, extra: Value) -> Response {
    let Some((sid, csrf, flash)) = current_session(state, headers) else {
        return Redirect::to("/login").into_response();
    };
    let mut snapshot = gather(state).await;
    // 세션마다 다른 것(결정 0043): 밖에서 로그인했나, 설정 중인 2단계 인증 비밀.
    snapshot.access.session_external = state
        .sessions
        .lock()
        .expect("sessions lock")
        .get(&sid)
        .is_some_and(|s| s.external);
    snapshot.access.totp_pending = state
        .totp_pending
        .lock()
        .expect("totp lock")
        .get(&sid)
        .filter(|(_, at)| at.elapsed() < TOTP_PENDING)
        .map(|(secret, _)| {
            (
                secret.clone(),
                crate::totp::otpauth_uri(secret, "관리 페이지"),
            )
        });
    axum::Json(spa::page(page, &snapshot, &csrf, flash.as_ref(), extra)).into_response()
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn index(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Query(callback): Query<CallbackQuery>,
) -> Response {
    // Google 동의 뒤 루프백으로 돌아온 경우(서버에서 SSH 터널로 열었을 때). Google에서 넘어오는 이동에는
    // SameSite=Strict 쿠키가 실리지 않으므로 로그인 세션 없이 처리한다. 대신 관리자가 로그인한 상태에서
    // 시작한 동의의 일회용 state(약 244비트, 10분)가 맞아야만 처리된다.
    if callback.code.is_some() || callback.error.is_some() {
        let (ok, text) = match (callback.code, callback.state, callback.error) {
            (_, _, Some(error)) => (
                false,
                format!("Google이 동의를 돌려주지 않았습니다: {error}"),
            ),
            (Some(code), Some(consent_state), None) => {
                finish_consent(&state, &consent_state, &code).await
            }
            _ => (false, "Google 응답에 state가 없습니다.".to_owned()),
        };
        let text = crate::mask::global().mask(&text);
        return Standalone::GoogleReturned { ok, text }.respond();
    }
    show(&state, &headers, Page::Overview, Value::Null).await
}

async fn connections_page(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    show(&state, &headers, Page::Connections, Value::Null).await
}

async fn start_page(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    show(&state, &headers, Page::Start, Value::Null).await
}

#[derive(Deserialize)]
struct ActivityQuery {
    kind: Option<String>,
}

async fn activity_page(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Query(q): Query<ActivityQuery>,
) -> Response {
    let filter = ActivityFilter::parse(q.kind.as_deref());
    show(&state, &headers, Page::Activity, extra::activity(filter)).await
}

async fn system_page(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    show(&state, &headers, Page::System, Value::Null).await
}

async fn work_page(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    show(&state, &headers, Page::Work, Value::Null).await
}

async fn automation_page(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    show(&state, &headers, Page::Automation, Value::Null).await
}

async fn browser_page(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    show(&state, &headers, Page::Browser, extra::browser_sites()).await
}

async fn expressions_page(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    show(&state, &headers, Page::Expressions, Value::Null).await
}

async fn settings_safety_page(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
) -> Response {
    show(&state, &headers, Page::Safety, Value::Null).await
}

async fn settings_notify_page(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
) -> Response {
    show(&state, &headers, Page::Notify, Value::Null).await
}

async fn settings_work_page(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    show(&state, &headers, Page::Tasks, Value::Null).await
}

// ---------- 설정 변경 ----------

#[derive(Deserialize)]
struct TokenForm {
    csrf: String,
    token: String,
}

async fn discord_token(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<TokenForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = state.discord.set_token(&f.token).await;
    let text = match &result {
        Ok(bot) => {
            tracing::info!(bot = %bot.name, "관리: Discord 토큰 저장");
            format!("토큰을 저장했습니다. 봇 {}로 연결합니다.", bot.name)
        }
        Err(err) => format!("토큰을 저장하지 않았습니다: {err:#}"),
    };
    flash(&state, &id, tone_of(&result), text);
    Redirect::to("/connections").into_response()
}

async fn discord_token_remove(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = state.discord.remove_token().await;
    let text = match &result {
        Ok(()) => "토큰을 지우고 Discord 연결을 끊었습니다.".to_owned(),
        Err(err) => format!("토큰을 지우지 못했습니다: {err:#}"),
    };
    flash(&state, &id, tone_of(&result), text);
    Redirect::to("/connections").into_response()
}

#[derive(Deserialize)]
struct OwnerForm {
    csrf: String,
    user_id: String,
}

async fn discord_owner(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<OwnerForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = state.discord.set_owner(&f.user_id).await;
    let text = match &result {
        Ok(()) => {
            tracing::info!("관리: 소유자 설정");
            "소유자를 저장하고 DM으로 확인 메시지를 보냈습니다.".to_owned()
        }
        Err(err) => format!("소유자를 저장하지 않았습니다: {err:#}"),
    };
    flash(&state, &id, tone_of(&result), text);
    Redirect::to("/connections").into_response()
}

async fn discord_owner_remove(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = state.discord.clear_owner();
    let text = match &result {
        Ok(()) => "소유자를 지웠습니다. 이제 아무 메시지도 받지 않습니다.".to_owned(),
        Err(err) => format!("지우지 못했습니다: {err:#}"),
    };
    flash(&state, &id, tone_of(&result), text);
    Redirect::to("/connections").into_response()
}

async fn discord_test(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let target = state
        .store()
        .setting(lucia_store::keys::NOTIFY_CONVERSATION)
        .ok()
        .flatten();
    let (tone, text) = match target {
        None => (
            Tone::Bad,
            "알림 대화가 없습니다. 먼저 소유자를 저장하세요.".to_owned(),
        ),
        Some(conversation) => {
            use lucia_channel::ChannelAdapter;
            let hub = crate::discord_ctl::HubChannel::new(state.discord.clone());
            match hub
                .send(lucia_core::Outbound::Text {
                    conversation: lucia_core::ConversationId(conversation),
                    body: "관리 페이지에서 보낸 시험 메시지입니다.".into(),
                })
                .await
            {
                Ok(_) => (Tone::Ok, "시험 메시지를 보냈습니다.".to_owned()),
                Err(err) => (Tone::Bad, format!("보내지 못했습니다: {err}")),
            }
        }
    };
    flash(&state, &id, tone, text);
    Redirect::to("/connections").into_response()
}

async fn checks_refresh(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let (checks, _) = cached_checks(&state, true).await;
    let failed = checks.iter().filter(|c| !c.ok).count();
    if failed == 0 {
        flash(
            &state,
            &id,
            Tone::Ok,
            format!("다시 점검했습니다. {}개 모두 정상입니다.", checks.len()),
        );
    } else {
        flash(
            &state,
            &id,
            Tone::Bad,
            format!("다시 점검했습니다. {failed}개가 실패했습니다."),
        );
    }
    Redirect::to(back_path(f.back.as_deref())).into_response()
}

// ---------- Google ----------

/// 동의를 마친다: state 확인 → 코드 교환 → 계정 주소 저장. (성공했는가, 안내 문구)를 돌려준다.
async fn finish_consent(
    state: &Arc<AdminState>,
    consent_state: &str,
    code: &str,
) -> (bool, String) {
    let pending = {
        let mut consents = state.consents.lock().expect("consents lock");
        consents.retain(|_, p| p.started.elapsed() < google_auth::CONSENT_TTL);
        consents.remove(consent_state)
    };
    let Some(pending) = pending else {
        return (
            false,
            "이 동의 요청을 찾을 수 없습니다(만료됐거나 이미 썼습니다). \"Google로 연결\"을 다시 누르세요.".into(),
        );
    };
    let scopes = match state.google.complete(&pending, code).await {
        Ok(scopes) => scopes,
        Err(err) => return (false, format!("Google 연결 실패: {err:#}")),
    };
    let previous = state
        .store()
        .setting(lucia_store::keys::GOOGLE_ACCOUNT)
        .ok()
        .flatten();
    let gmail = lucia_precheck::google::Gmail::new(
        state.google.http().clone(),
        state.google.clone(),
        lucia_precheck::google::GMAIL_BASE,
    );
    let email = gmail.account_email().await;
    {
        let store = state.store();
        let now = Utc::now();
        if let Ok(email) = &email {
            let _ = store.set_setting(lucia_store::keys::GOOGLE_ACCOUNT, email, now);
        }
        match &scopes {
            Some(granted) => {
                let _ = store.set_setting(lucia_store::keys::GOOGLE_SCOPES, granted, now);
            }
            None => {
                let _ = store.delete_setting(lucia_store::keys::GOOGLE_SCOPES);
            }
        }
    }
    tracing::info!("관리: Google 연결");
    let mut text = match &email {
        Ok(email) => format!("Google 계정 {email}을(를) 연결했습니다."),
        Err(err) => format!("연결은 됐지만 계정 확인에 실패했습니다: {err}."),
    };
    for (scope, what) in [
        (google_auth::SCOPE_GMAIL_READ, "메일 읽기"),
        (google_auth::SCOPE_CALENDAR_READ, "캘린더 읽기"),
        (google_auth::SCOPE_GMAIL_SEND, "메일 발송"),
        (google_auth::SCOPE_CALENDAR_WRITE, "캘린더 쓰기"),
    ] {
        if scopes
            .as_deref()
            .is_some_and(|g| !g.split_whitespace().any(|x| x == scope))
        {
            text.push_str(&format!(
                " {what} 권한을 받지 못했습니다(동의 화면에서 꺼졌을 수 있음). 쓰려면 다시 연결하세요."
            ));
        }
    }

    // 같은 계정으로 다시 동의하면(범위 추가 등) 기준점을 그대로 둔다. 다른 계정이거나 처음이면
    // 지금을 기준선으로 잡는다. 이제부터 온 메일과 바뀐 일정이 사전 체크의 변화가 된다.
    let same_account = email
        .as_ref()
        .ok()
        .is_some_and(|e| previous.as_deref() == Some(e.as_str()));
    let has_cursors = {
        let store = state.store();
        [
            lucia_precheck::google::CALENDAR_SOURCE,
            lucia_precheck::google::GMAIL_SOURCE,
        ]
        .iter()
        .all(|name| {
            store
                .source_state(name)
                .ok()
                .flatten()
                .is_some_and(|s| s.cursor.is_some())
        })
    };
    if same_account && has_cursors {
        text.push_str(" 같은 계정이라 사전 체크 기준점을 그대로 둡니다.");
        return (true, text);
    }
    let mut sources = crate::sources::google_sources(&state.google);
    let failed = crate::sources::rebaseline(&mut sources, &state.store).await;
    if failed.is_empty() {
        tracing::info!("관리: Google 기준선을 잡았다");
        text.push_str(" 지금 시점을 기준선으로 잡았습니다. 이제부터 들어오는 메일과 바뀌는 일정을 사전 체크(정해진 주기 또는 \"지금 사전 체크\")에서 감지합니다.");
    } else {
        let names: Vec<String> = failed
            .iter()
            .map(|(name, why)| format!("{name}({why})"))
            .collect();
        tracing::warn!(failed = %names.join(", "), "Google 기준선을 일부 잡지 못했다");
        text.push_str(&format!(
            " 다만 기준선을 잡지 못한 소스가 있습니다: {}. 다음 사전 체크에서 다시 잡습니다(그 사이에 온 것은 변화로 보지 않습니다).",
            names.join(", ")
        ));
    }
    (true, text)
}

#[derive(Deserialize)]
struct ClientForm {
    csrf: String,
    json: String,
}

async fn google_client(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<ClientForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = state.google.set_client(&f.json).await;
    let text = match &result {
        Ok(_) => {
            tracing::info!("관리: Google 클라이언트 저장");
            "클라이언트 JSON을 저장했습니다. 이제 \"Google로 연결\"을 누르세요.".to_owned()
        }
        Err(err) => format!("저장하지 않았습니다: {err:#}"),
    };
    flash(&state, &id, tone_of(&result), text);
    Redirect::to("/connections").into_response()
}

// ---------- 초기 설정과 프로필(결정 0064) ----------

/// 프로필 그림 올리기의 요청 크기 상한. 규칙(3 MiB)보다 넉넉히 받아 규칙을 어긴 그림에는 어긴 것을 모두 알려 준다
/// (기본 상한 2 MiB에서 끊기면 3 MiB 안의 그림도 올라가지 않고, 큰 그림은 이유 없이 거절된다).
const PROFILE_UPLOAD_LIMIT: usize = 3 * crate::profile::MAX_BYTES as usize;

/// 처리 뒤 돌아갈 화면: 온 화면(정해 둔 목록에 있을 때), 모르면 초기 설정.
fn back_or_start(back: Option<&str>) -> &'static str {
    back.map_or(Page::Start.path(), |b| back_path(Some(b)))
}

/// 홈의 초기 설정 안내 끄기.
async fn start_dismiss(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let saved = state
        .store()
        .set_setting(setup::DISMISSED_KEY, "1", Utc::now());
    let (tone, text) = match saved {
        Ok(()) => (
            Tone::Ok,
            "홈의 초기 설정 안내를 껐습니다. 초기 설정은 메뉴에서 언제든 다시 열 수 있습니다."
                .to_owned(),
        ),
        Err(e) => (Tone::Bad, format!("저장하지 못했습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Overview.path()).into_response()
}

/// 홈의 초기 설정 안내 다시 켜기.
async fn start_reopen(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let saved = state.store().delete_setting(setup::DISMISSED_KEY);
    let (tone, text) = match saved {
        Ok(()) => (
            Tone::Ok,
            "초기 설정이 끝나지 않았으면 홈에 안내를 다시 보입니다.".to_owned(),
        ),
        Err(e) => (Tone::Bad, format!("저장하지 못했습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Start.path()).into_response()
}

#[derive(Deserialize)]
struct ProfileNameForm {
    csrf: String,
    /// 비우면 기본 이름으로 되돌린다.
    #[serde(default)]
    name: String,
    #[serde(default)]
    back: Option<String>,
}

/// 관리 화면과 앱에 보일 이름 정하기. Lucia가 자기를 부르는 이름과 말투(캐릭터)와는 따로다.
async fn profile_name(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<ProfileNameForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let now = Utc::now();
    // 프로필이 붙어 있으면 거기로(연결된 앱에 알린다), 아니면 저장소에만 적는다.
    let result = match state.profile.get() {
        Some(access) => access.profile.set_name(&f.name, now),
        None => crate::profile::save_name(&state.store(), &f.name, now),
    };
    let (tone, text) = match result {
        Ok(name) => {
            let reset = f.name.trim().is_empty();
            let _ = state.store().add_activity(
                "profile_name",
                &if reset {
                    format!("프로필 이름: 기본값 “{name}”으로(관리 페이지)")
                } else {
                    format!("프로필 이름: “{name}”(관리 페이지)")
                },
                None,
                now,
            );
            tracing::info!("관리: 프로필 이름 변경");
            if reset {
                (
                    Tone::Ok,
                    format!("이름을 기본값(“{name}”)으로 되돌렸습니다."),
                )
            } else {
                (
                    Tone::Ok,
                    format!("이름을 “{name}”(으)로 정했습니다. 관리 화면과 앱에 보입니다."),
                )
            }
        }
        Err(why) => (Tone::Bad, format!("이름을 저장하지 않았습니다: {why}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(back_or_start(f.back.as_deref())).into_response()
}

/// 프로필 그림 올리기(multipart). 소유자가 정하는 것이라 `owner`로 남고, Lucia의 도구와 같은 규칙·같은 길
/// (앱에 알림, 연결된 Discord 봇의 아바타)이다. 그림은 검사만 하고 고치지 않는다.
async fn profile_avatar_upload(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Response {
    let mut csrf = None;
    let mut back = None;
    let mut file: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return avatar_upload_error(&err),
        };
        let name = field.name().unwrap_or("").to_owned();
        let data = match field.bytes().await {
            Ok(data) => data,
            Err(err) => return avatar_upload_error(&err),
        };
        match name.as_str() {
            "csrf" => csrf = Some(String::from_utf8_lossy(&data).into_owned()),
            "back" => back = Some(String::from_utf8_lossy(&data).into_owned()),
            "file" => file = Some(data.to_vec()),
            _ => {}
        }
    }
    let id = match authorize(&state, &headers, csrf.as_deref().unwrap_or("")) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let now = Utc::now();
    let (tone, text) = match (state.profile.get().cloned(), file.filter(|b| !b.is_empty())) {
        (None, _) => (Tone::Bad, NO_PROFILE.to_owned()),
        (_, None) => (Tone::Bad, "파일이 없습니다.".to_owned()),
        (Some(access), Some(bytes)) => match crate::profile::check_for_owner(&bytes)
            .and_then(|_| access.profile.set(&bytes, "owner", now))
        {
            Err(why) => (Tone::Bad, format!("올리지 않았습니다: {why}")),
            Ok(avatar) => {
                // 저장소 잠금을 쥐지 않고 Discord에 적용한다(느릴 수 있다).
                let discord = access.apply_discord(&avatar, &bytes, now).await;
                let size = crate::files::human_size(avatar.size);
                let _ = state.store().add_activity(
                    "profile_image",
                    &format!(
                        "프로필 그림 바꿈(관리 페이지): {}×{}, {size}",
                        avatar.width, avatar.height
                    ),
                    None,
                    now,
                );
                tracing::info!("관리: 프로필 그림 변경");
                (
                    Tone::Ok,
                    format!(
                        "프로필 그림을 바꿨습니다({}×{}, {size}). 연결된 앱에 바로 알렸습니다(꺼져 있는 앱은 다음에 켤 때 받습니다). {discord}",
                        avatar.width, avatar.height
                    ),
                )
            }
        },
    };
    flash(&state, &id, tone, text);
    Redirect::to(back_or_start(back.as_deref())).into_response()
}

const NO_PROFILE: &str = "이 데몬은 프로필 그림을 다루지 않습니다.";

/// 올리는 도중에 읽지 못했다: 요청 크기 상한을 넘었으면 그렇게, 아니면 형식이 깨졌다고 알린다(글로 거절한다).
fn avatar_upload_error(err: &axum::extract::multipart::MultipartError) -> Response {
    if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "그림을 받지 못했습니다: 파일이 너무 큽니다. {}까지 올릴 수 있습니다.",
                crate::files::human_size(crate::profile::MAX_BYTES)
            ),
        )
            .into_response();
    }
    (StatusCode::BAD_REQUEST, format!("업로드 형식: {err}")).into_response()
}

/// 프로필 그림 지우기: 앱은 기본 모습으로, Discord 봇은 데몬이 바꾼 적이 있으면 원래 아바타로 돌아간다.
async fn profile_avatar_remove(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let now = Utc::now();
    let (tone, text) = match state.profile.get().cloned() {
        None => (Tone::Bad, NO_PROFILE.to_owned()),
        Some(access) => match access.profile.clear() {
            Err(why) => (Tone::Bad, format!("지우지 못했습니다: {why}")),
            Ok(had) => {
                let discord = access.clear_discord(now).await;
                if had {
                    let _ = state.store().add_activity(
                        "profile_image",
                        "프로필 그림 없앰(관리 페이지)",
                        None,
                        now,
                    );
                    tracing::info!("관리: 프로필 그림 지움");
                }
                (
                    Tone::Ok,
                    format!(
                        "{} {discord}",
                        if had {
                            "프로필 그림을 지웠습니다. 연결된 앱에 바로 알렸습니다."
                        } else {
                            "정한 프로필 그림이 없었습니다."
                        }
                    ),
                )
            }
        },
    };
    flash(&state, &id, tone, text);
    Redirect::to(back_or_start(f.back.as_deref())).into_response()
}

/// 지금(과 아직 남은 예전) 프로필 그림. id가 내용 해시라 바뀌지 않는다. 로그인한 사람만 받는다.
async fn profile_avatar_file(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !logged_in(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some((mime, bytes)) = state.profile.get().and_then(|a| a.profile.file(&id)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, mime),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response()
}

// ---------- 이미지 생성(결정 0046) ----------

#[derive(Deserialize)]
struct ImageKeyForm {
    csrf: String,
    #[serde(default)]
    key: String,
}

/// OpenAI API 키 넣기. 암호화해 저장하고 다시 보이지 않는다. 키가 맞는지는 처음 만들 때 드러난다.
async fn images_key(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<ImageKeyForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let key = f.key.trim();
    let plausible = (20..=400).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_graphic());
    let (tone, text) = if !plausible {
        (
            Tone::Bad,
            "키를 저장하지 않았습니다: 공백 없는 20~400자의 API 키를 넣으세요.".to_owned(),
        )
    } else {
        match state.creds.write(crate::imagegen::CREDENTIAL, key).await {
            Ok(()) => {
                let _ = state.store().add_activity(
                    "image_admin",
                    "이미지 생성: OpenAI API 키를 넣었다",
                    None,
                    Utc::now(),
                );
                (
                    Tone::Ok,
                    "OpenAI API 키를 저장했습니다. 이제 Lucia에게 그림을 부탁할 수 있습니다."
                        .to_owned(),
                )
            }
            Err(err) => (Tone::Bad, format!("키를 저장하지 않았습니다: {err:#}")),
        }
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Connections.path()).into_response()
}

async fn images_key_remove(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = state.creds.remove(crate::imagegen::CREDENTIAL).await;
    let text = match &result {
        Ok(()) => {
            let _ = state.store().add_activity(
                "image_admin",
                "이미지 생성: OpenAI API 키를 지웠다",
                None,
                Utc::now(),
            );
            "OpenAI API 키를 지웠습니다. 그림 만들기가 꺼집니다.".to_owned()
        }
        Err(err) => format!("키를 지우지 못했습니다: {err:#}"),
    };
    flash(&state, &id, tone_of(&result), text);
    Redirect::to(Page::Connections.path()).into_response()
}

#[derive(Deserialize)]
struct ImageBudgetForm {
    csrf: String,
    #[serde(default)]
    daily: String,
}

/// 이미지 하루 예산(달러). 비우면 기본값, 0이면 만들지 않는다.
async fn images_budget(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<ImageBudgetForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let daily = f.daily.trim();
    let value = if daily.is_empty() {
        Ok(String::new())
    } else {
        match daily.parse::<f64>() {
            Ok(v) if v.is_finite() && (0.0..=100.0).contains(&v) => Ok(format!("{v}")),
            _ => Err("0에서 100 사이의 숫자(달러)를 넣으세요."),
        }
    };
    let (tone, text) = match value {
        Ok(v) => {
            let saved = state
                .store()
                .set_setting(crate::imagegen::keys::DAILY_USD, &v, Utc::now());
            match saved {
                Ok(()) if v.is_empty() => (
                    Tone::Ok,
                    format!(
                        "이미지 하루 예산을 기본값(${:.2})으로 돌렸습니다.",
                        crate::imagegen::DEFAULT_DAILY_USD
                    ),
                ),
                Ok(()) => (Tone::Ok, format!("이미지 하루 예산을 ${v}로 정했습니다.")),
                Err(err) => (Tone::Bad, format!("저장하지 못했습니다: {err}")),
            }
        }
        Err(why) => (Tone::Bad, format!("저장하지 않았습니다: {why}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Connections.path()).into_response()
}

// ---------- 이모지·스티커(결정 0045) ----------

/// 앱 이모지·스티커 서비스(접속 API가 들고 있다).
fn expressions_of(state: &AdminState) -> Option<Arc<crate::expressions::Expressions>> {
    state.gateway.get().and_then(|g| g.expressions().cloned())
}

const NO_EXPRESSIONS: &str = "이 데몬은 이모지·스티커를 다루지 않습니다.";

/// 목록의 그림(미리보기). 서명으로 확인한 PNG·WebP·GIF만 저장되어 있다.
async fn expression_file(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !logged_in(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some((info, bytes)) = expressions_of(&state).and_then(|x| x.file(&id)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (axum::http::header::CONTENT_TYPE, info.mime),
            (
                axum::http::header::CACHE_CONTROL,
                "private, max-age=31536000, immutable".to_owned(),
            ),
            (
                axum::http::header::X_CONTENT_TYPE_OPTIONS,
                "nosniff".to_owned(),
            ),
        ],
        bytes,
    )
        .into_response()
}

/// 이모지·스티커 올리기(multipart). 그림은 검사만 하고 고치지 않는다.
async fn expression_add(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Response {
    let mut text: HashMap<String, String> = HashMap::new();
    let mut file: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => {
                return (StatusCode::BAD_REQUEST, format!("업로드 형식: {err}")).into_response();
            }
        };
        let name = field.name().unwrap_or("").to_owned();
        let data = match field.bytes().await {
            Ok(data) => data,
            Err(err) => {
                return (StatusCode::BAD_REQUEST, format!("업로드 읽기: {err}")).into_response();
            }
        };
        if name == "file" {
            file = Some(data.to_vec());
        } else {
            text.insert(name, String::from_utf8_lossy(&data).into_owned());
        }
    }
    let get = |k: &str| text.get(k).map(String::as_str).unwrap_or("").trim();
    let id = match authorize(&state, &headers, get("csrf")) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = (|| {
        let x = expressions_of(&state).ok_or(NO_EXPRESSIONS)?;
        let kind = crate::expressions::Kind::parse(get("kind")).ok_or("종류를 고르세요.")?;
        let bytes = file.filter(|b| !b.is_empty()).ok_or("파일이 없습니다.")?;
        let meta = crate::expressions::Meta {
            label: get("label").to_owned(),
            description: get("description").to_owned(),
            tags: crate::expressions::parse_tags(get("tags")),
            category: get("category").to_owned(),
        };
        x.add(kind, get("name"), &meta, &bytes, "admin", Utc::now())
            .map(|item| (kind, item))
    })();
    let (tone, text) = match result {
        Ok((kind, item)) => {
            let _ = state.store().add_activity(
                "expression_add",
                &format!("{} 등록(관리 페이지): {}", kind.label(), item.name),
                None,
                Utc::now(),
            );
            (
                Tone::Ok,
                format!("{} \"{}\"을(를) 올렸습니다.", kind.label(), item.name),
            )
        }
        Err(e) => (Tone::Bad, format!("올리지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Expressions.path()).into_response()
}

#[derive(Deserialize)]
struct ExpressionEditForm {
    csrf: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    category: String,
}

async fn expression_update(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(eid): Path<String>,
    Form(f): Form<ExpressionEditForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = match expressions_of(&state) {
        Some(x) => x.update(
            &eid,
            &crate::expressions::Meta {
                label: f.label,
                description: f.description,
                tags: crate::expressions::parse_tags(&f.tags),
                category: f.category,
            },
            Utc::now(),
        ),
        None => Err(NO_EXPRESSIONS.into()),
    };
    let (tone, text) = match result {
        Ok(()) => {
            let _ = state.store().add_activity(
                "expression_edit",
                &format!("이모지·스티커 고침(관리 페이지): {eid}"),
                None,
                Utc::now(),
            );
            (Tone::Ok, "저장했습니다.".to_owned())
        }
        Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Expressions.path()).into_response()
}

async fn expression_delete(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(eid): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = match expressions_of(&state) {
        Some(x) => x.remove(&eid, Utc::now()),
        None => Err(NO_EXPRESSIONS.into()),
    };
    let (tone, text) = match result {
        Ok(item) => {
            let _ = state.store().add_activity(
                "expression_remove",
                &format!("이모지·스티커 지움(관리 페이지): {}", item.name),
                None,
                Utc::now(),
            );
            (Tone::Ok, format!("\"{}\"을(를) 지웠습니다.", item.name))
        }
        Err(e) => (Tone::Bad, format!("지우지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Expressions.path()).into_response()
}

#[derive(Deserialize)]
struct ExpressionCategoryForm {
    csrf: String,
    id: String,
    label: String,
    #[serde(default)]
    sort: String,
    #[serde(default)]
    icon: String,
}

/// 분류 만들기·고치기(같은 ID가 있으면 고친다).
async fn expression_category_save(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<ExpressionCategoryForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = match expressions_of(&state) {
        Some(x) => x.put_category(
            &f.id,
            &f.label,
            f.sort.trim().parse::<i64>().ok().map(|n| n.clamp(0, 999)),
            Some(f.icon.as_str()),
        ),
        None => Err(NO_EXPRESSIONS.into()),
    };
    let (tone, text) = match result {
        Ok(()) => (Tone::Ok, "분류를 저장했습니다.".to_owned()),
        Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Expressions.path()).into_response()
}

async fn expression_category_delete(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(cid): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = match expressions_of(&state) {
        Some(x) => x.remove_category(&cid, Utc::now()),
        None => Err(NO_EXPRESSIONS.into()),
    };
    let (tone, text) = match result {
        Ok(()) => (Tone::Ok, "분류를 지웠습니다.".to_owned()),
        Err(e) => (Tone::Bad, format!("지우지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Expressions.path()).into_response()
}

/// 올린 클라이언트 JSON 파일의 최대 크기. Desktop app 클라이언트 JSON은 1 KB 안팎이다.
const MAX_CLIENT_JSON_BYTES: usize = 16 * 1024;

/// 클라이언트 JSON 파일 올리기(multipart). 폼의 `csrf`가 파일보다 먼저 온다.
async fn google_client_upload(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Response {
    let mut csrf = None;
    let mut file: Option<Vec<u8>> = None;
    let mut too_large = false;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => {
                return (StatusCode::BAD_REQUEST, format!("업로드 형식: {err}")).into_response();
            }
        };
        let name = field.name().unwrap_or("").to_owned();
        let data = match field.bytes().await {
            Ok(data) => data,
            Err(err) => {
                return (StatusCode::BAD_REQUEST, format!("업로드 읽기: {err}")).into_response();
            }
        };
        match name.as_str() {
            "csrf" => csrf = Some(String::from_utf8_lossy(&data).into_owned()),
            "file" if data.len() > MAX_CLIENT_JSON_BYTES => too_large = true,
            "file" => file = Some(data.to_vec()),
            _ => {}
        }
    }
    let id = match authorize(&state, &headers, csrf.as_deref().unwrap_or("")) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let (tone, text) = match (too_large, file) {
        (true, _) => (
            Tone::Bad,
            format!(
                "파일이 너무 큽니다({}KB까지). 클라이언트 JSON 파일이 맞는지 확인하세요.",
                MAX_CLIENT_JSON_BYTES / 1024
            ),
        ),
        (false, None) => (Tone::Bad, "파일이 없습니다.".to_owned()),
        (false, Some(bytes)) => match String::from_utf8(bytes) {
            Err(_) => (Tone::Bad, "UTF-8 텍스트 파일이 아닙니다.".to_owned()),
            Ok(json) => match state.google.set_client(&json).await {
                Ok(_) => {
                    tracing::info!("관리: Google 클라이언트 저장(파일)");
                    (
                        Tone::Ok,
                        "클라이언트 JSON을 저장했습니다. 이제 \"Google로 연결\"을 누르세요."
                            .to_owned(),
                    )
                }
                Err(err) => (Tone::Bad, format!("저장하지 않았습니다: {err:#}")),
            },
        },
    };
    flash(&state, &id, tone, text);
    Redirect::to("/connections").into_response()
}

async fn google_connect(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let Some(loopback) = state
        .config
        .admin
        .bind
        .iter()
        .find(|a| a.ip().is_loopback())
    else {
        flash(
            &state,
            &id,
            Tone::Bad,
            "admin.bind에 127.0.0.1 주소가 있어야 Google 동의를 받을 수 있습니다.".into(),
        );
        return Redirect::to("/connections").into_response();
    };
    let client = match state.google.client().await {
        Ok(c) => c,
        Err(err) => {
            flash(
                &state,
                &id,
                Tone::Bad,
                format!("먼저 클라이언트 JSON을 저장하세요: {err:#}"),
            );
            return Redirect::to("/connections").into_response();
        }
    };
    let redirect_uri = format!("http://127.0.0.1:{}/", loopback.port());
    let (url, pending) = google_auth::consent_url(&client, &redirect_uri);
    state
        .consents
        .lock()
        .expect("consents lock")
        .insert(pending.state.clone(), pending);
    Redirect::to(&url).into_response()
}

#[derive(Deserialize)]
struct PasteForm {
    csrf: String,
    url: String,
}

async fn google_paste(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<PasteForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let (ok, text) = match google_auth::parse_callback(&f.url) {
        Ok((code, consent_state)) => finish_consent(&state, &consent_state, &code).await,
        Err(err) => (false, format!("{err:#}")),
    };
    flash(&state, &id, if ok { Tone::Ok } else { Tone::Bad }, text);
    Redirect::to("/connections").into_response()
}

async fn google_disconnect(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = state.google.disconnect().await;
    let text = match &result {
        Ok(()) => {
            let store = state.store();
            let _ = store.delete_setting(lucia_store::keys::GOOGLE_ACCOUNT);
            let _ = store.delete_setting(lucia_store::keys::GOOGLE_SCOPES);
            "Google 연결을 끊었습니다(토큰 철회).".to_owned()
        }
        Err(err) => format!("끊지 못했습니다: {err:#}"),
    };
    flash(&state, &id, tone_of(&result), text);
    Redirect::to("/connections").into_response()
}

async fn run_now(
    state: &Arc<AdminState>,
    headers: &HeaderMap,
    form: &CsrfForm,
    action: &str,
    label: &str,
) -> Response {
    let id = match authorize(state, headers, &form.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let job = crate::worker::Job::Routine(crate::scheduler::DueJob {
        schedule_id: "manual".into(),
        action: action.into(),
        payload: serde_json::json!({}),
        due_at: Utc::now(),
    });
    let result = state.jobs.try_send(job);
    let text = match &result {
        Ok(()) => format!(
            "{label}을(를) 작업 큐에 넣었습니다. 진행 중인 대화가 있으면 그 뒤에 실행되고, 결과는 \"지금 실행\"의 마지막 결과에 보입니다."
        ),
        Err(err) => format!("작업 큐에 넣지 못했습니다: {err}"),
    };
    flash(
        state,
        &id,
        if result.is_ok() {
            Tone::Info
        } else {
            Tone::Bad
        },
        text,
    );
    Redirect::to(back_path(form.back.as_deref())).into_response()
}

async fn run_precheck(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    run_now(
        &state,
        &headers,
        &f,
        crate::worker::actions::PRECHECK,
        "사전 체크",
    )
    .await
}

async fn run_briefing(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    run_now(
        &state,
        &headers,
        &f,
        crate::worker::actions::MORNING_BRIEFING,
        "브리핑",
    )
    .await
}

async fn run_backup(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    run_now(
        &state,
        &headers,
        &f,
        crate::worker::actions::SESSION_BACKUP,
        "세션 기록 백업",
    )
    .await
}

async fn run_compact(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    run_now(
        &state,
        &headers,
        &f,
        crate::worker::actions::COMPACT,
        "세션 정리",
    )
    .await
}

// ---------- 기억(결정 0007) ----------

/// 주소의 항목 이름이 형식에 맞는가(저장소 규칙과 같다).
fn valid_memory_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= lucia_store::memory_limits::SLUG_MAX
        && !slug.starts_with('-')
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[derive(Deserialize)]
struct MemoryQuery {
    q: Option<String>,
}

async fn memory_page(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Query(q): Query<MemoryQuery>,
) -> Response {
    let query = q.q.unwrap_or_default();
    show(&state, &headers, Page::Memory, extra::memory_list(&query)).await
}

async fn memory_entry_page(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Response {
    if !logged_in(&state, &headers) {
        return Redirect::to("/login").into_response();
    }
    let found = if valid_memory_slug(&slug) {
        let store = state.store();
        store.memory(&slug).ok().flatten().map(|e| {
            let history = store.memory_history(&slug, 20).unwrap_or_default();
            (e, history)
        })
    } else {
        None
    };
    match found {
        Some((entry, history)) => {
            show(
                &state,
                &headers,
                Page::Memory,
                extra::memory_entry(&entry, &history),
            )
            .await
        }
        None => {
            let page = show(&state, &headers, Page::Memory, extra::memory_missing()).await;
            (StatusCode::NOT_FOUND, page).into_response()
        }
    }
}

async fn memory_delete(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = if valid_memory_slug(&slug) {
        let store = state.store();
        let title = store
            .memory(&slug)
            .ok()
            .flatten()
            .map(|e| e.title)
            .unwrap_or_default();
        let now = Utc::now();
        let deleted = store.delete_memory(
            &slug,
            lucia_store::MemoryAuthor::User,
            None,
            "관리 페이지에서 지움",
            now,
        );
        if let Ok(true) = deleted {
            let _ = store.add_activity(
                "memory_delete",
                &format!("기억 지움: {title}([[{slug}]]) — 관리 페이지에서 지움"),
                None,
                now,
            );
        }
        deleted.map_err(|e| e.to_string())
    } else {
        Err("항목 이름이 잘못됐다".to_owned())
    };
    let (tone, text) = match &result {
        Ok(true) => {
            tracing::info!(%slug, "관리: 기억 항목 지움");
            (
                Tone::Ok,
                "기억 항목을 지웠습니다. 지운 내용은 변경 기록에 남습니다.".to_owned(),
            )
        }
        Ok(false) => (Tone::Bad, "이미 없는 항목입니다.".to_owned()),
        Err(why) => (Tone::Bad, format!("지우지 못했습니다: {why}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Memory.path()).into_response()
}

/// 기억 항목을 예전 개정으로 되돌린다(결정 0027: 하루 기억 변경 확인 카드 대신). 사용자 변경으로 남긴다.
async fn memory_restore(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path((slug, rev)): Path<(String, i64)>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = (|| -> Result<String, String> {
        if !valid_memory_slug(&slug) {
            return Err("항목 이름이 잘못됐다".into());
        }
        let store = state.store();
        let r = store
            .memory_revision(rev)
            .map_err(|e| e.to_string())?
            .filter(|r| r.slug == slug && r.action == lucia_store::MemoryAction::Write)
            .ok_or("이 항목의 개정이 아니다")?;
        let current = store.memory(&slug).map_err(|e| e.to_string())?;
        let now = Utc::now();
        let reason = format!("관리 페이지에서 개정 {rev}으로 되돌림");
        store
            .write_memory(
                &lucia_store::MemoryWrite {
                    slug: &slug,
                    section: r.section,
                    title: &r.title,
                    summary: &r.summary,
                    body: &r.body,
                },
                current.map(|c| c.revision),
                lucia_store::MemoryAuthor::User,
                None,
                &reason,
                now,
            )
            .map_err(|e| e.to_string())?;
        let _ = store.add_activity(
            "memory_restore",
            &format!("기억 되돌림: {}([[{slug}]]) — {reason}", r.title),
            None,
            now,
        );
        Ok(r.title)
    })();
    let (tone, text) = match result {
        Ok(title) => {
            tracing::info!(%slug, rev, "관리: 기억 항목 되돌림");
            (
                Tone::Ok,
                format!("‘{title}’을 개정 {rev}의 내용으로 되돌렸습니다."),
            )
        }
        Err(why) => (Tone::Bad, format!("되돌리지 못했습니다: {why}")),
    };
    flash(&state, &id, tone, text);
    if valid_memory_slug(&slug) {
        Redirect::to(&format!("/memory/{slug}")).into_response()
    } else {
        Redirect::to(Page::Memory.path()).into_response()
    }
}

#[derive(Deserialize)]
struct TellForm {
    csrf: String,
    text: String,
}

/// "기억해 줘": 작업 큐에 넣는다(대화·루틴과 겹치지 않게). 결과는 기억 화면의 마지막 결과에 보인다.
async fn memory_tell(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<TellForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let text = f.text.trim();
    let (tone, message) = if text.is_empty() || text.chars().count() > 1000 {
        (Tone::Bad, "1~1000자로 적어 주세요.".to_owned())
    } else {
        let job = crate::worker::Job::Routine(crate::scheduler::DueJob {
            schedule_id: "manual".into(),
            action: crate::worker::actions::REMEMBER.into(),
            payload: serde_json::json!({ "text": text }),
            due_at: Utc::now(),
        });
        match state.jobs.try_send(job) {
            Ok(()) => {
                tracing::info!("관리: 기억해 줘 요청");
                (
                    Tone::Info,
                    "Lucia에게 맡겼습니다. 진행 중인 대화가 있으면 그 뒤에 하고, 결과는 이 화면에 보입니다."
                        .to_owned(),
                )
            }
            Err(err) => (Tone::Bad, format!("작업 큐에 넣지 못했습니다: {err}")),
        }
    };
    flash(&state, &id, tone, message);
    Redirect::to(Page::Memory.path()).into_response()
}

// ---------- 자동 실행(결정 0010) ----------

#[derive(Deserialize)]
struct LimitsForm {
    csrf: String,
    #[serde(default)]
    daily: String,
    #[serde(default)]
    weekly: String,
    #[serde(default)]
    quiet: String,
    #[serde(default)]
    routine_wakes: String,
}

/// 비용 상한과 방해 금지 시간대 저장. 비우면 끈다.
#[derive(Deserialize)]
struct KeepForm {
    csrf: String,
    #[serde(default)]
    keep_days: String,
}

/// 대화 원본 보관 기간(결정 0016). 비우면 지우지 않는다. 되돌릴 수 없으므로 하한을 둔다.
async fn wrapup_settings(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<KeepForm>,
) -> Response {
    use crate::engine::wrapup::{KEEP_DAYS_MIN, keys};
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let raw = f.keep_days.trim();
    let result = if raw.is_empty() {
        state
            .store()
            .delete_setting(keys::KEEP_DAYS)
            .map(|_| "보관 기간을 비웠습니다. 대화 원본을 지우지 않습니다.".to_owned())
            .map_err(|e| e.to_string())
    } else {
        match raw.parse::<i64>() {
            Ok(d) if (KEEP_DAYS_MIN..=3650).contains(&d) => state
                .store()
                .set_setting(keys::KEEP_DAYS, &d.to_string(), Utc::now())
                .map(|_| format!("저장했습니다. 하루 정리 때 {d}일보다 오래된 대화 원본을 지웁니다(되돌릴 수 없습니다)."))
                .map_err(|e| e.to_string()),
            _ => Err(format!("보관 기간은 {KEEP_DAYS_MIN}~3650일입니다.")),
        }
    };
    let (tone, text) = match result {
        Ok(t) => (Tone::Ok, t),
        Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Tasks.path()).into_response()
}

#[derive(Deserialize)]
struct GuildCallsForm {
    csrf: String,
    #[serde(default)]
    calls: Option<String>,
    #[serde(default)]
    prefix: String,
}

/// 다른 서버에서 부르기(결정 0024): 켜고 끄기와 머리말. 켜고 끄면 Discord에 다시 붙는다(intent가 바뀐다).
async fn discord_guild_calls(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<GuildCallsForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let on = f.calls.is_some();
    let prefix = f.prefix.trim();
    if !prefix.is_empty() && !lucia_channel::discord::valid_prefix(prefix) {
        flash(
            &state,
            &id,
            Tone::Bad,
            "저장하지 않았습니다: 머리말은 1~20자이고 줄바꿈·백틱(`)이 없어야 합니다.".to_owned(),
        );
        return Redirect::to(Page::Connections.path()).into_response();
    }
    let was = crate::discord_ctl::guild_calls_on(&state.store());
    let saved = {
        let store = state.store();
        let now = Utc::now();
        let r1 = if on {
            store.set_setting(lucia_store::keys::DISCORD_GUILD_CALLS, "on", now)
        } else {
            store.delete_setting(lucia_store::keys::DISCORD_GUILD_CALLS)
        };
        let r2 = if prefix.is_empty() {
            store.delete_setting(lucia_store::keys::DISCORD_PREFIX)
        } else {
            store.set_setting(lucia_store::keys::DISCORD_PREFIX, prefix, now)
        };
        r1.and(r2).map_err(|e| e.to_string())
    };
    let (tone, text) = match saved {
        Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
        Ok(()) => {
            state.discord.apply_settings();
            // 켜고 끄기가 바뀌었거나 내용 intent가 거절된 상태면 다시 붙는다.
            if was != on || state.discord.content_refused() {
                state.discord.retry_guild();
                match state.discord.start_from_credentials().await {
                    Ok(()) => (
                        Tone::Ok,
                        if on {
                            "저장하고 Discord에 다시 붙었습니다. 봇이 들어간 서버에서 멘션이나 머리말로 Lucia를 부를 수 있습니다.".to_owned()
                        } else {
                            "껐습니다. 다른 서버에서는 부를 수 없습니다.".to_owned()
                        },
                    ),
                    Err(e) => (
                        Tone::Warn,
                        format!("저장했지만 Discord에 다시 붙지 못했습니다: {e:#}"),
                    ),
                }
            } else {
                (Tone::Ok, "저장했습니다.".to_owned())
            }
        }
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Connections.path()).into_response()
}

#[derive(Deserialize)]
struct TaskChannelForm {
    csrf: String,
    #[serde(default)]
    channel: String,
}

/// 작업 스레드를 열 Discord 채널(결정 0014). 바꾸면 Discord 연결을 다시 붙인다(intent가 바뀐다).
async fn task_channel(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<TaskChannelForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let raw = f.channel.trim();
    let result = if raw.is_empty() {
        state
            .store()
            .delete_setting(crate::tasks::CHANNEL_KEY)
            .map_err(|e| e.to_string())
    } else if raw.len() > 25 || !raw.chars().all(|c| c.is_ascii_digit()) || raw.starts_with('0') {
        Err("채널 ID는 숫자입니다(Discord 개발자 모드에서 채널을 우클릭 → ID 복사).".to_owned())
    } else {
        let conv = lucia_channel::discord::conversation_for(raw.parse().unwrap_or(0));
        state
            .store()
            .set_setting(crate::tasks::CHANNEL_KEY, &conv.0, Utc::now())
            .map_err(|e| e.to_string())
    };
    let (tone, text) = match result {
        Ok(()) => {
            // intent가 바뀌므로 게이트웨이를 다시 붙인다.
            state.discord.retry_guild();
            let restart = state.discord.start_from_credentials().await;
            match restart {
                Ok(()) if raw.is_empty() => (
                    Tone::Ok,
                    "작업 채널을 비웠습니다. 작업은 DM에서 합니다.".to_owned(),
                ),
                Ok(()) => (
                    Tone::Ok,
                    "저장하고 Discord에 다시 붙었습니다. 다음 작업부터 이 채널에 스레드를 엽니다."
                        .to_owned(),
                ),
                Err(e) => (
                    Tone::Warn,
                    format!("저장했지만 Discord에 다시 붙지 못했습니다: {e:#}"),
                ),
            }
        }
        Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Tasks.path()).into_response()
}

async fn autonomy_limits(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<LimitsForm>,
) -> Response {
    use crate::autonomy::{QuietHours, keys};
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let money = |raw: &str, what: &str| -> Result<String, String> {
        let raw = raw.trim().trim_start_matches('$');
        if raw.is_empty() {
            return Ok(String::new());
        }
        match raw.parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 && v <= 10_000.0 => Ok(format!("{v}")),
            _ => Err(format!("{what}은 0보다 큰 달러 금액이어야 합니다")),
        }
    };
    let result = (|| -> Result<(), String> {
        let daily = money(&f.daily, "하루 상한")?;
        let weekly = money(&f.weekly, "주 상한")?;
        let quiet = match f.quiet.trim() {
            "" => String::new(),
            q => QuietHours::parse(q)?.display(),
        };
        let wakes = match f.routine_wakes.trim() {
            "" => String::new(),
            w => match w.parse::<i64>() {
                Ok(n) if crate::routines::DAILY_WAKES_RANGE.contains(&n) => n.to_string(),
                _ => return Err("루틴이 하루에 깨우는 횟수는 1~200입니다".into()),
            },
        };
        let store = state.store();
        let now = Utc::now();
        for (key, value) in [
            (keys::DAILY_USD, daily),
            (keys::WEEKLY_USD, weekly),
            (keys::QUIET_HOURS, quiet),
            (keys::ROUTINE_WAKES, wakes),
        ] {
            if value.is_empty() {
                store.delete_setting(key).map_err(|e| e.to_string())?;
            } else {
                store
                    .set_setting(key, &value, now)
                    .map_err(|e| e.to_string())?;
            }
        }
        // 상한을 바꾸면 그날 알림도 다시 할 수 있게 한다.
        store
            .delete_setting(keys::CAP_NOTICE)
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    let (tone, text) = match result {
        Ok(()) => {
            tracing::info!("관리: 자동 실행 한도 저장");
            (
                Tone::Ok,
                "저장했습니다. 다음 자동 실행부터 적용됩니다.".to_owned(),
            )
        }
        Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Safety.path()).into_response()
}

/// 후속 조치 취소(사용자).
async fn followup_cancel(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(fid): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = {
        let store = state.store();
        let closed = store.close_followup(
            &fid,
            lucia_store::FollowupStatus::Cancelled,
            "관리 페이지에서 사용자가 취소",
            Utc::now(),
        );
        if let Ok(true) = closed {
            let _ = store.add_activity(
                "followup_cancel",
                &format!("후속 조치 취소({fid}): 관리 페이지"),
                None,
                Utc::now(),
            );
        }
        closed
    };
    let (tone, text) = match result {
        Ok(true) => (Tone::Ok, "취소했습니다.".to_owned()),
        Ok(false) => (Tone::Warn, "이미 끝났거나 없는 후속 조치입니다.".to_owned()),
        Err(e) => (Tone::Bad, format!("취소하지 못했습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(back_path(f.back.as_deref())).into_response()
}

/// 목표 닫기·지금 점검(사용자, 결정 0032): `done` | `abandon` | `review`.
async fn goal_action(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path((gid, verb)): Path<(String, String)>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let now = Utc::now();
    let store = state.store();
    let (tone, text) = match verb.as_str() {
        "done" | "abandon" => {
            let status = if verb == "done" {
                lucia_store::GoalStatus::Done
            } else {
                lucia_store::GoalStatus::Abandoned
            };
            match crate::goals::close(&store, &gid, status, "관리 페이지에서 사용자가 닫음", now)
            {
                Ok(t) => (Tone::Ok, t),
                Err(e) => (Tone::Warn, format!("닫지 않았습니다: {e}")),
            }
        }
        "review" => match store.goal(&gid) {
            Ok(Some(g)) if g.status.open() => {
                let _ = store.update_goal(&gid, None, None, None, None, None, Some(now), now);
                match crate::goals::arm(&store, &gid, now) {
                    Ok(()) => (Tone::Ok, "곧 점검합니다(1분 안).".to_owned()),
                    Err(e) => (Tone::Bad, format!("점검을 걸지 못했습니다: {e}")),
                }
            }
            _ => (Tone::Warn, "열린 목표가 아닙니다.".to_owned()),
        },
        _ => (Tone::Bad, "알 수 없는 동작입니다.".to_owned()),
    };
    drop(store);
    flash(&state, &id, tone, text);
    Redirect::to(back_path(f.back.as_deref())).into_response()
}

/// 작업 취소(사용자, 결정 0014).
async fn task_cancel(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(tid): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = {
        let store = state.store();
        let moved = store.move_task(
            &tid,
            lucia_core::TaskStatus::Cancelled,
            "관리 페이지에서 사용자가 취소",
            Utc::now(),
        );
        if let Ok(true) = moved {
            let _ = store.add_activity(
                "task_cancel",
                &format!("작업 취소({tid}): 관리 페이지"),
                None,
                Utc::now(),
            );
        }
        moved
    };
    let (tone, text) = match result {
        Ok(true) => (Tone::Ok, "작업을 취소했습니다.".to_owned()),
        Ok(false) => (Tone::Warn, "이미 끝났거나 없는 작업입니다.".to_owned()),
        Err(e) => (Tone::Bad, format!("취소하지 못했습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(back_path(f.back.as_deref())).into_response()
}

/// 브리핑에서 등록한 후속 조치 하나에 브라우저를 허용한다(결정 0013). 사용자가 그 메모를 보고 정한다.
async fn followup_allow_browser(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(fid): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = {
        let store = state.store();
        let changed = store.set_followup_origin(&fid, crate::autonomy::ORIGIN_USER);
        if let Ok(true) = changed {
            let _ = store.add_activity(
                "followup_browser",
                &format!("후속 조치 브라우저 허용({fid}): 관리 페이지"),
                None,
                Utc::now(),
            );
        }
        changed
    };
    let (tone, text) = match result {
        Ok(true) => (
            Tone::Ok,
            "이 후속 조치는 그 시각에 브라우저를 씁니다.".to_owned(),
        ),
        Ok(false) => (Tone::Warn, "이미 끝났거나 없는 후속 조치입니다.".to_owned()),
        Err(e) => (Tone::Bad, format!("바꾸지 못했습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(back_path(f.back.as_deref())).into_response()
}

// ---------- 개발 위임(결정 0020) ----------

#[derive(Deserialize)]
struct DevForm {
    csrf: String,
    #[serde(default)]
    cost_cap: String,
    #[serde(default)]
    allow_hosts: String,
    #[serde(default)]
    local_roots: String,
}

async fn dev_settings(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<DevForm>,
) -> Response {
    use crate::dev::keys;
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = (|| -> Result<(), String> {
        let cap = f.cost_cap.trim().trim_start_matches('$');
        let cap = if cap.is_empty() {
            String::new()
        } else {
            match cap.parse::<f64>() {
                Ok(v) if v > 0.0 && v <= 100.0 => format!("{v}"),
                _ => return Err("비용 상한은 0보다 크고 100달러 이하입니다".into()),
            }
        };
        // 열 주소: 호스트[:포트] 줄마다. 사설 주소는 받지 않는다.
        let mut hosts = Vec::new();
        for line in f
            .allow_hosts
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            let host = line.split(':').next().unwrap_or("");
            crate::browser::egress::normalize_domain(host)?;
            if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                crate::browser::egress::Egress::check_ip_literal(ip)?;
            }
            hosts.push(line.to_owned());
        }
        let mut roots = Vec::new();
        for line in f
            .local_roots
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            let path = std::path::Path::new(line);
            if !path.is_absolute() || !path.is_dir() {
                return Err(format!("있는 절대 경로가 아닙니다: {line}"));
            }
            roots.push(line.to_owned());
        }
        let store = state.store();
        let now = Utc::now();
        for (key, value) in [
            (keys::COST_CAP_USD, cap),
            (keys::ALLOW_HOSTS, hosts.join("\n")),
            (keys::LOCAL_ROOTS, roots.join("\n")),
        ] {
            if value.is_empty() {
                store.delete_setting(key).map_err(|e| e.to_string())?;
            } else {
                store
                    .set_setting(key, &value, now)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    })();
    let (tone, text) = match result {
        Ok(()) => (
            Tone::Ok,
            "저장했습니다. 다음 개발 작업부터 적용됩니다.".to_owned(),
        ),
        Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Tasks.path()).into_response()
}

// ---------- 접속 API 클라이언트(결정 0021) ----------

#[derive(Deserialize)]
struct ApiClientForm {
    csrf: String,
    #[serde(default)]
    name: String,
    /// 범위 체크 상자(결정 0043): 있으면 켠다.
    #[serde(default)]
    scope_security_mode: Option<String>,
    /// Lucia가 이 앱의 기능을 부르게 한다(결정 0053).
    #[serde(default)]
    scope_app_tools: Option<String>,
    /// 브라우저 화면 넘겨받기를 이 앱에서(결정 0063).
    #[serde(default)]
    scope_browser_handover: Option<String>,
    /// 다른 대화(Discord DM, 다른 앱)의 파일을 이 앱에서 받는다(결정 0066).
    #[serde(default)]
    scope_owner_files: Option<String>,
}

fn form_scopes(f: &ApiClientForm) -> Vec<String> {
    let mut out = Vec::new();
    if f.scope_security_mode.is_some() {
        out.push(crate::gateway::scopes::SECURITY_MODE.to_owned());
    }
    if f.scope_app_tools.is_some() {
        out.push(crate::gateway::scopes::APP_TOOLS.to_owned());
    }
    if f.scope_browser_handover.is_some() {
        out.push(crate::gateway::scopes::BROWSER_HANDOVER.to_owned());
    }
    if f.scope_owner_files.is_some() {
        out.push(crate::gateway::scopes::OWNER_FILES.to_owned());
    }
    out
}

/// 앱 토큰 발급. 토큰은 이 응답의 안내에 한 번만 보이고, 저장소에는 해시만 남는다.
async fn client_new(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<ApiClientForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let name = f.name.trim();
    let result = (|| -> Result<String, String> {
        if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
            return Err("이름은 1~40자 한 줄입니다.".into());
        }
        let store = state.store();
        let active = store
            .clients()
            .map_err(|e| e.to_string())?
            .iter()
            .filter(|c| c.revoked_at.is_none())
            .count();
        if active >= crate::gateway::MAX_CLIENTS {
            return Err(format!(
                "클라이언트는 {}개까지입니다. 쓰지 않는 것을 취소하세요.",
                crate::gateway::MAX_CLIENTS
            ));
        }
        let token = crate::gateway::new_token();
        let client_id = crate::gateway::new_client_id();
        store
            .create_client(
                &client_id,
                name,
                crate::gateway::Role::App.as_str(),
                &crate::gateway::token_hash(&token),
                Utc::now(),
            )
            .map_err(|e| e.to_string())?;
        let scopes = form_scopes(&f);
        store
            .set_client_scopes(&client_id, &scopes)
            .map_err(|e| e.to_string())?;
        let _ = store.add_activity(
            "client_admin",
            &format!("앱 토큰 발급: {name} ({client_id}, 범위 {scopes:?})"),
            None,
            Utc::now(),
        );
        Ok(token)
    })();
    let (tone, text) = match result {
        Ok(token) => (
            Tone::Ok,
            format!("{name}의 토큰입니다. 지금 한 번만 보이니 앱에 넣어 두세요: {token}"),
        ),
        Err(e) => (Tone::Bad, e),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Connections.path()).into_response()
}

/// 클라이언트 취소: 토큰이 더는 듣지 않고, 연결은 바로 끊는다.
async fn client_revoke(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(cid): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let revoked = state.store().revoke_client(&cid, Utc::now());
    let (tone, text) = match revoked {
        Ok(true) => {
            if let Some(g) = state.gateway.get() {
                g.disconnect(&cid);
            }
            let _ = state.store().add_activity(
                "client_admin",
                &format!("앱 토큰 취소: {cid}"),
                None,
                Utc::now(),
            );
            (Tone::Ok, "취소했습니다. 연결도 끊었습니다.".to_owned())
        }
        Ok(false) => (Tone::Bad, "없거나 이미 취소한 클라이언트입니다.".to_owned()),
        Err(e) => (Tone::Bad, e.to_string()),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Connections.path()).into_response()
}

/// 클라이언트 범위 바꾸기(결정 0043). 바꾼 범위는 다음 프레임부터 듣는다.
async fn client_scopes(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(cid): Path<String>,
    Form(f): Form<ApiClientForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let scopes = form_scopes(&f);
    let result = (|| -> Result<(), String> {
        let store = state.store();
        store
            .active_client(&cid)
            .map_err(|e| e.to_string())?
            .ok_or("없거나 취소한 클라이언트입니다.")?;
        store
            .set_client_scopes(&cid, &scopes)
            .map_err(|e| e.to_string())?;
        let _ = store.add_activity(
            "client_admin",
            &format!("앱 토큰 범위: {cid} → {scopes:?}"),
            None,
            Utc::now(),
        );
        Ok(())
    })();
    let (tone, text) = match result {
        Ok(()) => (Tone::Ok, "범위를 바꿨습니다.".to_owned()),
        Err(e) => (Tone::Bad, e),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Connections.path()).into_response()
}

#[derive(Deserialize)]
struct AppNotifyForm {
    csrf: String,
    /// 비우면 Discord DM.
    #[serde(default)]
    client: String,
}

/// 알림 받을 곳(결정 0043): Discord DM 또는 앱 모두(결정 0066·0067).
async fn app_notify(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<AppNotifyForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let client = f.client.trim();
    let result = (|| -> Result<String, String> {
        let store = state.store();
        if client.is_empty() {
            store
                .delete_setting(crate::gateway::keys::NOTIFY_CLIENT)
                .map_err(|e| e.to_string())?;
            let _ =
                store.add_activity("client_admin", "알림 받을 곳: Discord DM", None, Utc::now());
            return Ok("알림을 Discord DM으로 받습니다.".into());
        }
        // 앱으로 받는다(결정 0067: 앱은 기기가 여럿이어도 한 대화라 앱 하나를 고르는 것은 없다). 예전 화면이
        // 앱 하나의 ID를 보내도 앱 모두로 둔다. 취소하지 않은 앱이 하나라도 있을 때만(없으면 알림이 갈 곳이 없다).
        if client != crate::gateway::ALL_APPS
            && store
                .active_client(client)
                .map_err(|e| e.to_string())?
                .filter(|c| c.role == crate::gateway::Role::App.as_str())
                .is_none()
        {
            return Err("없거나 취소한 앱입니다.".into());
        }
        if crate::gateway::active_apps(&store).is_empty() {
            return Err("취소하지 않은 앱이 없습니다. 앱 토큰을 먼저 발급하세요.".into());
        }
        store
            .set_setting(
                crate::gateway::keys::NOTIFY_CLIENT,
                crate::gateway::ALL_APPS,
                Utc::now(),
            )
            .map_err(|e| e.to_string())?;
        let _ = store.add_activity("client_admin", "알림 받을 곳: 앱 모두", None, Utc::now());
        Ok("알림·브리핑·승인 카드를 앱에 둡니다. 모든 앱이 같은 대화를 받고, 사람이 있는 기기에서 울리며, 보지 않으면 휴대폰(푸시)으로 넘깁니다.".into())
    })();
    let (tone, text) = match result {
        Ok(t) => (Tone::Ok, t),
        Err(e) => (Tone::Bad, e),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Connections.path()).into_response()
}

#[derive(Deserialize)]
struct PushPreviewForm {
    csrf: String,
    #[serde(default)]
    preview: Option<String>,
}

/// 푸시 미리보기 켜고 끄기.
async fn app_push_preview(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<PushPreviewForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let on = f.preview.is_some();
    let saved = state.store().set_setting(
        crate::gateway::keys::PUSH_PREVIEW,
        if on { "1" } else { "0" },
        Utc::now(),
    );
    let (tone, text) = match saved {
        Ok(()) if on => (
            Tone::Ok,
            "푸시에 글 앞부분을 싣습니다(Google을 거칩니다).".to_owned(),
        ),
        Ok(()) => (Tone::Ok, "푸시에 고정 문구만 싣습니다.".to_owned()),
        Err(e) => (Tone::Bad, e.to_string()),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Connections.path()).into_response()
}

#[derive(Deserialize)]
struct FcmForm {
    csrf: String,
    #[serde(default)]
    json: String,
}

/// FCM 서비스 계정 JSON 넣기. 확인한 뒤 암호화해 저장하고, 화면에는 프로젝트 ID만 보인다.
async fn app_fcm(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<FcmForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = match state.app_push.get() {
        Some(push) => push.install(&f.json).await,
        None => Err("이 데몬은 푸시를 쓰지 않습니다.".into()),
    };
    let (tone, text) = match result {
        Ok(project) => {
            let _ = state.store().add_activity(
                "client_admin",
                &format!("앱 푸시(FCM) 자격 증명: 프로젝트 {project}"),
                None,
                Utc::now(),
            );
            (
                Tone::Ok,
                format!("푸시를 켰습니다(Firebase 프로젝트 {project})."),
            )
        }
        Err(e) => (Tone::Bad, e),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Connections.path()).into_response()
}

async fn app_fcm_remove(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = match state.app_push.get() {
        Some(push) => push.remove().await,
        None => Ok(()),
    };
    let (tone, text) = match result {
        Ok(()) => {
            let _ = state.store().add_activity(
                "client_admin",
                "앱 푸시(FCM) 자격 증명을 지웠다",
                None,
                Utc::now(),
            );
            (
                Tone::Ok,
                "푸시 자격 증명을 지웠습니다. 끊긴 앱의 글은 Discord로 갑니다.".to_owned(),
            )
        }
        Err(e) => (Tone::Bad, e),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Connections.path()).into_response()
}

/// 시험 푸시 한 번.
async fn app_push_test(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(cid): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let token = state
        .store()
        .active_client(&cid)
        .ok()
        .flatten()
        .and_then(|c| c.push_token);
    let result = match (state.app_push.get(), token) {
        (None, _) => Err("이 데몬은 푸시를 쓰지 않습니다.".to_owned()),
        (_, None) => Err("이 앱은 푸시 토큰을 등록하지 않았습니다.".to_owned()),
        (Some(push), Some(token)) => push
            .send(&crate::gateway::PushJob {
                client_id: cid.clone(),
                token,
                kind: "text".into(),
                seq: 0,
                preview: "시험 푸시입니다.".into(),
                high: false,
            })
            .await
            .map_err(|e| e.to_string()),
    };
    let (tone, text) = match result {
        Ok(()) => (Tone::Ok, "시험 푸시를 보냈습니다.".to_owned()),
        Err(e) => (Tone::Bad, format!("보내지 못했습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Connections.path()).into_response()
}

// ---------- 페이지 감시(결정 0019) ----------

/// 감시 조작(사람): 멈추기·다시 켜기·지우기.
async fn watch_action(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path((wid, verb)): Path<(String, String)>,
    Form(f): Form<CsrfForm>,
) -> Response {
    use lucia_store::WatchStatus as W;
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let now = Utc::now();
    let to = match verb.as_str() {
        "pause" => Some(W::Paused),
        "resume" => Some(W::Active),
        "remove" => Some(W::Removed),
        _ => None,
    };
    let result = match to {
        None => Err("할 수 없는 조작입니다.".to_owned()),
        Some(to) => {
            let store = state.store();
            match store.watch(&wid) {
                Ok(Some(w)) if w.status != W::Pending || to == W::Removed => {
                    // 다시 켜면 기준선부터 다시 잡는다(멈춘 동안 바뀐 것이 한꺼번에 변화가 되지 않게).
                    // 로그인으로 멈춘 게시판 감시는 그대로 둔다(그 사이 올라온 글을 알린다, 한꺼번에 30개까지).
                    if to == W::Active && w.paused_reason != lucia_store::watch_paused::LOGIN {
                        let _ =
                            store.reset_source(&format!("{}{}", crate::watch::SOURCE_PREFIX, wid));
                    }
                    store
                        .set_watch_status(&wid, to, now)
                        .map(|_| format!("{}: {}", w.what, to.label()))
                        .map_err(|e| e.to_string())
                }
                Ok(Some(_)) => Err("승인 대기 중인 감시는 Discord 카드에서 정합니다.".to_owned()),
                Ok(None) => Err("없는 감시입니다.".to_owned()),
                Err(e) => Err(e.to_string()),
            }
        }
    };
    if result.is_ok() {
        let _ = state.store().add_activity(
            "watch_admin",
            &format!("감시 {wid}: {verb}(관리 페이지)"),
            None,
            now,
        );
    }
    let (tone, text) = match result {
        Ok(t) => (Tone::Ok, t),
        Err(e) => (Tone::Bad, e),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Automation.path()).into_response()
}

#[derive(Deserialize)]
struct WatchEditForm {
    csrf: String,
    interval: String,
    #[serde(default)]
    include: String,
    #[serde(default)]
    exclude: String,
    #[serde(default)]
    notify: Option<String>,
    #[serde(default)]
    fetcher: Option<String>,
}

/// 감시 고치기(사람, 결정 0036): 간격, 키워드, 알림 방식, 읽는 방식.
async fn watch_edit(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(wid): Path<String>,
    Form(f): Form<WatchEditForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let now = Utc::now();
    let result = (|| {
        let store = state.store();
        let w = store
            .watch(&wid)
            .map_err(|e| e.to_string())?
            .ok_or("없는 감시입니다.")?;
        let min = crate::watch::interval_min(w.is_items());
        let interval = f
            .interval
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|m| (min..=crate::watch::INTERVAL_MAX).contains(m))
            .ok_or(format!(
                "간격은 {min}~{}분입니다.",
                crate::watch::INTERVAL_MAX
            ))?;
        let words = |raw: &str| {
            crate::board::words(raw)
                .into_iter()
                .collect::<Vec<_>>()
                .join("\n")
        };
        let include = words(&f.include);
        let exclude = words(&f.exclude);
        let notify = match f.notify.as_deref().map(str::trim) {
            Some(n) if w.is_items() => Some(
                *lucia_store::watch_notify::ALL
                    .iter()
                    .find(|a| **a == n)
                    .ok_or("알림 방식이 잘못됐습니다.")?,
            ),
            _ => None,
        };
        // 읽는 방식을 바꿔도 본 글 기록(키)은 같으므로 기준선을 다시 잡지 않는다.
        let fetcher = match f.fetcher.as_deref().map(str::trim) {
            Some(v) if w.is_items() => Some(
                *lucia_store::watch_fetcher::ALL
                    .iter()
                    .find(|a| **a == v)
                    .ok_or("읽는 방식이 잘못됐습니다.")?,
            ),
            _ => None,
        };
        store
            .update_watch(
                &wid,
                &lucia_store::WatchEdit {
                    interval_minutes: Some(interval),
                    include_words: w.is_items().then_some(include.as_str()),
                    exclude_words: w.is_items().then_some(exclude.as_str()),
                    notify,
                    fetcher,
                    ..lucia_store::WatchEdit::default()
                },
                now,
            )
            .map_err(|e| e.to_string())?;
        let _ = store.add_activity(
            "watch_admin",
            &format!("감시 {wid}: 고침(관리 페이지)"),
            None,
            now,
        );
        Ok::<_, String>(format!("{}: 고쳤습니다.", w.what))
    })();
    let (tone, text) = match result {
        Ok(t) => (Tone::Ok, t),
        Err(e) => (Tone::Bad, format!("고치지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Automation.path()).into_response()
}

#[derive(Deserialize)]
struct SecurityForm {
    csrf: String,
    mode: String,
    #[serde(default)]
    back: Option<String>,
}

/// 권한 모드 바꾸기(결정 0040): 활동 기록과 Discord 알림을 남긴다.
async fn security_mode(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<SecurityForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let back = back_path(f.back.as_deref());
    let Some(mode) = crate::security::Mode::parse(&f.mode) else {
        flash(&state, &id, Tone::Bad, "모르는 권한 모드입니다.".into());
        return Redirect::to(back).into_response();
    };
    let changed = crate::security::set_mode(&state.store(), mode, "관리 페이지", Utc::now());
    let (tone, text) = match changed {
        Ok(true) => {
            state
                .notify_owner(format!(
                    "관리 페이지에서 권한 모드를 바꿨습니다.\n{}",
                    crate::security::describe(mode)
                ))
                .await;
            (
                Tone::Ok,
                format!("권한 모드를 {}(으)로 바꿨습니다.", mode.label()),
            )
        }
        Ok(false) => (Tone::Ok, format!("이미 {} 모드입니다.", mode.label())),
        Err(e) => (Tone::Bad, format!("바꾸지 못했습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(back).into_response()
}

#[derive(Deserialize)]
struct TotpForm {
    csrf: String,
    #[serde(default)]
    code: String,
}

/// 2단계 인증을 바꾸는 일은 집 안에서 로그인한 세션만(결정 0043): 밖에서 가로챈 세션으로 인증을 끄거나 바꾸지 못하게.
fn local_only(state: &AdminState, id: &str, origin: Origin) -> Result<(), String> {
    let external = state
        .sessions
        .lock()
        .expect("sessions lock")
        .get(id)
        .is_some_and(|s| s.external);
    if origin.external() || external {
        return Err("2단계 인증은 집 네트워크에서만 바꿀 수 있습니다.".into());
    }
    Ok(())
}

/// 2단계 인증 설정 시작: 새 비밀을 이 세션에 잠시 들고 있다가, 코드를 맞히면 저장한다.
async fn totp_start(
    State(state): State<Arc<AdminState>>,
    Extension(origin): Extension<Origin>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let back = back_path(f.back.as_deref());
    match local_only(&state, &id, origin) {
        Ok(()) => {
            let mut pending = state.totp_pending.lock().expect("totp lock");
            pending.retain(|_, (_, at)| at.elapsed() < TOTP_PENDING);
            pending.insert(id.clone(), (crate::totp::new_secret(), Instant::now()));
            drop(pending);
            flash(
                &state,
                &id,
                Tone::Info,
                "인증 앱에 아래 키를 넣고, 앱이 보여 주는 6자리 코드를 입력하세요(10분 안에)."
                    .into(),
            );
        }
        Err(e) => flash(&state, &id, Tone::Bad, e),
    }
    Redirect::to(back).into_response()
}

/// 2단계 인증 켜기: 설정 중인 비밀로 코드를 맞히면 자격 증명으로 저장한다.
async fn totp_confirm(
    State(state): State<Arc<AdminState>>,
    Extension(origin): Extension<Origin>,
    headers: HeaderMap,
    Form(f): Form<TotpForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result: Result<(), String> = async {
        local_only(&state, &id, origin)?;
        let secret = state
            .totp_pending
            .lock()
            .expect("totp lock")
            .get(&id)
            .filter(|(_, at)| at.elapsed() < TOTP_PENDING)
            .map(|(s, _)| s.clone())
            .ok_or("설정 시간이 지났습니다. 다시 시작하세요.")?;
        let step = crate::totp::verify(&secret, &f.code, Utc::now().timestamp(), None)
            .ok_or("코드가 맞지 않습니다. 인증 앱의 시계가 맞는지 보고 새 코드를 넣으세요.")?;
        state
            .creds
            .write(crate::totp::CREDENTIAL, &secret)
            .await
            .map_err(|e| format!("저장하지 못했습니다: {e}"))?;
        *state.totp_last.lock().expect("totp lock") = Some(step);
        state.totp_pending.lock().expect("totp lock").remove(&id);
        Ok(())
    }
    .await;
    let (tone, text) = match result {
        Ok(()) => {
            let _ = state.store().add_activity(
                "admin_security",
                "관리 페이지 2단계 인증을 켰다",
                None,
                Utc::now(),
            );
            state
                .notify_owner("🔐 관리 페이지 2단계 인증을 켰습니다. 밖에서 로그인할 때 인증 앱의 코드가 필요합니다.".into())
                .await;
            (Tone::Ok, "2단계 인증을 켰습니다.".to_owned())
        }
        Err(e) => (Tone::Bad, e),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Safety.path()).into_response()
}

/// 2단계 인증 끄기: 지금 코드를 한 번 더 확인한다. 끄면 밖에서는 로그인할 수 없다.
async fn totp_disable(
    State(state): State<Arc<AdminState>>,
    Extension(origin): Extension<Origin>,
    headers: HeaderMap,
    Form(f): Form<TotpForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result: Result<(), String> = async {
        local_only(&state, &id, origin)?;
        let secret = state
            .creds
            .read(crate::totp::CREDENTIAL)
            .await
            .map_err(|e| format!("읽지 못했습니다: {e}"))?
            .ok_or("2단계 인증이 꺼져 있습니다.")?;
        let last = *state.totp_last.lock().expect("totp lock");
        crate::totp::verify(&secret, &f.code, Utc::now().timestamp(), last)
            .ok_or("코드가 맞지 않습니다.")?;
        state
            .creds
            .remove(crate::totp::CREDENTIAL)
            .await
            .map_err(|e| format!("지우지 못했습니다: {e}"))?;
        // 밖에서 로그인해 둔 세션도 끝낸다(이제 밖에서는 로그인할 수 없다).
        state
            .sessions
            .lock()
            .expect("sessions lock")
            .retain(|_, s| !s.external);
        Ok(())
    }
    .await;
    let (tone, text) = match result {
        Ok(()) => {
            let _ = state.store().add_activity(
                "admin_security",
                "관리 페이지 2단계 인증을 껐다",
                None,
                Utc::now(),
            );
            state
                .notify_owner("🔓 관리 페이지 2단계 인증을 껐습니다. 이제 밖에서는 관리 페이지에 로그인할 수 없습니다.".into())
                .await;
            (
                Tone::Ok,
                "2단계 인증을 껐습니다. 밖에서는 로그인할 수 없습니다.".to_owned(),
            )
        }
        Err(e) => (Tone::Bad, e),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Safety.path()).into_response()
}

#[derive(Deserialize)]
struct WatchSettingsForm {
    csrf: String,
    now_daily: String,
}

/// 감시 전체 설정: 바로 알림 하루 상한.
async fn watch_settings(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<WatchSettingsForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = match f.now_daily.trim().parse::<i64>() {
        Ok(n) if crate::watch::NOW_DAILY_RANGE.contains(&n) => state
            .store()
            .set_setting(crate::watch::NOW_DAILY_KEY, &n.to_string(), Utc::now())
            .map(|_| {
                format!(
                    "저장했습니다. 감시 하나가 하루에 바로 보내는 새 글 알림은 {n}건까지입니다."
                )
            })
            .map_err(|e| e.to_string()),
        _ => Err(format!(
            "{}~{}건입니다.",
            crate::watch::NOW_DAILY_RANGE.start(),
            crate::watch::NOW_DAILY_RANGE.end()
        )),
    };
    let (tone, text) = match result {
        Ok(t) => (Tone::Ok, t),
        Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Automation.path()).into_response()
}

/// 반복 루틴(결정 0025): 멈추기·다시 켜기·켜기(승인 대기)·지금 실행·지우기.
#[derive(Deserialize)]
struct GrantForm {
    csrf: String,
    host: String,
    #[serde(default)]
    purpose: String,
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    actions: Option<String>,
}

/// 사이트 위임을 사람이 직접 만든다(결정 0028). 같은 호스트가 있으면 넓힌다.
async fn grant_add(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<GrantForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let (login, actions) = (f.login.is_some(), f.actions.is_some());
    let purpose = match f.purpose.trim() {
        "" => "관리 페이지에서 위임".to_owned(),
        p => p.chars().take(120).collect(),
    };
    let gid = format!("grant-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let result = state.store().grant_site(
        &lucia_store::NewSiteGrant {
            id: &gid,
            host: &f.host,
            login,
            actions,
            purpose: &purpose,
            origin: "admin",
            expires_at: None,
        },
        Utc::now(),
    );
    let (tone, text) = match result {
        Ok(_) => {
            let host = lucia_store::valid_host(&f.host).unwrap_or_default();
            let _ = state.store().add_activity(
                "site_granted",
                &format!("관리 페이지에서 사이트 위임: {host}"),
                None,
                Utc::now(),
            );
            (
                Tone::Ok,
                format!(
                    "{host}를 위임했습니다: {}",
                    crate::delegation::scope_text(login, actions)
                ),
            )
        }
        Err(e) => (Tone::Bad, format!("위임하지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Browser.path()).into_response()
}

/// 사이트 위임을 거둔다(다음 실행부터 빠진다).
async fn grant_revoke(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(gid): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let store = state.store();
    let host = store
        .site_grant(&gid)
        .ok()
        .flatten()
        .map(|g| g.host)
        .unwrap_or_default();
    let (tone, text) = match store.revoke_site_grant(&gid, Utc::now()) {
        Ok(true) => {
            let _ = store.add_activity(
                "site_revoked",
                &format!("사이트 위임 거둠: {host}"),
                None,
                Utc::now(),
            );
            (
                Tone::Ok,
                format!("{host} 위임을 거뒀습니다. 다음 실행부터 묻습니다."),
            )
        }
        Ok(false) => (Tone::Warn, "이미 거둔 위임입니다.".to_owned()),
        Err(e) => (Tone::Bad, format!("거두지 못했습니다: {e}")),
    };
    drop(store);
    flash(&state, &id, tone, text);
    Redirect::to(Page::Browser.path()).into_response()
}

#[derive(Deserialize)]
struct SignalsForm {
    csrf: String,
    #[serde(default)]
    enabled: Option<String>,
    #[serde(default)]
    consistency: Option<String>,
    #[serde(default)]
    weekly: Option<String>,
    #[serde(default)]
    evening: Option<String>,
    #[serde(default)]
    poll_minutes: String,
    #[serde(default)]
    interests: String,
    #[serde(default)]
    classify_daily: String,
    #[serde(default)]
    wake_daily: String,
}

/// 이벤트 확인 간격과 신호 처리 설정(결정 0031).
async fn signals_settings(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<SignalsForm>,
) -> Response {
    use crate::signals::{self as sig, keys};
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let check = || -> Result<(u64, i64, i64, String), String> {
        let poll = f
            .poll_minutes
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|m| crate::events::POLL_MINUTES_RANGE.contains(m))
            .ok_or("확인 간격은 1~60분입니다.")?;
        let cap = |raw: &str, what: &str| {
            raw.trim()
                .parse::<i64>()
                .ok()
                .filter(|n| sig::DAILY_RANGE.contains(n))
                .ok_or(format!("{what}은(는) 0~500번입니다."))
        };
        let classify = cap(&f.classify_daily, "하루 분류 상한")?;
        let wake = cap(&f.wake_daily, "하루 깨우기 상한")?;
        let lines = sig::parse_interests(&f.interests);
        Ok((poll, classify, wake, lines.join("\n")))
    };
    let (tone, text) = match check() {
        Ok((poll, classify, wake, interests)) => {
            let store = state.store();
            let now = Utc::now();
            let saved = (|| -> lucia_store::Result<()> {
                store.set_setting(crate::events::POLL_MINUTES_KEY, &poll.to_string(), now)?;
                store.set_setting(keys::CLASSIFY_DAILY, &classify.to_string(), now)?;
                store.set_setting(keys::WAKE_DAILY, &wake.to_string(), now)?;
                store.set_setting(keys::INTERESTS, &interests, now)?;
                store.set_setting(
                    keys::ENABLED,
                    if f.enabled.is_some() { "on" } else { "off" },
                    now,
                )?;
                store.set_setting(
                    crate::consistency::ENABLED_KEY,
                    if f.consistency.is_some() { "on" } else { "off" },
                    now,
                )?;
                store.set_setting(
                    crate::reports::WEEKLY_KEY,
                    if f.weekly.is_some() { "on" } else { "off" },
                    now,
                )?;
                store.set_setting(
                    crate::reports::EVENING_KEY,
                    if f.evening.is_some() { "on" } else { "off" },
                    now,
                )?;
                Ok(())
            })();
            match saved {
                Ok(()) => (
                    Tone::Ok,
                    format!("저장했습니다. {poll}분마다 확인합니다(다음 확인부터)."),
                ),
                Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
            }
        }
        Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Notify.path()).into_response()
}

#[derive(Deserialize)]
struct QuietCalendarForm {
    csrf: String,
    #[serde(default)]
    on: Option<String>,
    #[serde(default)]
    except: String,
}

/// 일정 기반 방해 금지(결정 0034).
async fn quiet_calendar(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<QuietCalendarForm>,
) -> Response {
    use crate::autonomy::keys;
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let now = Utc::now();
    let store = state.store();
    let except = crate::signals::parse_interests(&f.except).join("\n");
    let saved = store
        .set_setting(
            keys::QUIET_CALENDAR,
            if f.on.is_some() { "on" } else { "off" },
            now,
        )
        .and_then(|_| store.set_setting(keys::QUIET_CALENDAR_EXCEPT, &except, now))
        // 다음 조용할 때 바로 다시 읽게 한다.
        .and_then(|_| store.delete_setting("quiet.busy_at").map(|_| ()));
    drop(store);
    let (tone, text) = match saved {
        Ok(()) if f.on.is_some() => (
            Tone::Ok,
            "일정 있는 동안도 조용히 합니다(5분 안에 일정을 읽습니다).".to_owned(),
        ),
        Ok(()) => (Tone::Ok, "일정 기반 방해 금지를 껐습니다.".to_owned()),
        Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Safety.path()).into_response()
}

#[derive(Deserialize)]
struct RuleForm {
    csrf: String,
    key: String,
}

/// 알림 규칙 지우기(결정 0034).
async fn notice_rule_delete(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<RuleForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let (tone, text) = match state.store().delete_notice_rule(&f.key) {
        Ok(true) => (
            Tone::Ok,
            "규칙을 지웠습니다. 그 보낸 곳의 알림은 다시 분류합니다.".to_owned(),
        ),
        Ok(false) => (Tone::Warn, "없는 규칙입니다.".to_owned()),
        Err(e) => (Tone::Bad, format!("지우지 못했습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Notify.path()).into_response()
}

// ---------- 파일 보관함(결정 0052) ----------

/// 보관한 파일 내려받기. 저장소에 있는 파일만, 파일 디렉터리 안의 것만 준다.
async fn file_download(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(fid): Path<String>,
) -> Response {
    if !logged_in(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(record) = state.store().file(&fid).ok().flatten() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let dir = state.config.data_dir.join("files");
    let path = dir.join(&record.rel_path);
    let inside = path
        .canonicalize()
        .ok()
        .zip(dir.canonicalize().ok())
        .is_some_and(|(p, d)| p.starts_with(d));
    let Some(bytes) = inside.then(|| std::fs::read(&path).ok()).flatten() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let encoded =
        percent_encoding::utf8_percent_encode(&record.name, percent_encoding::NON_ALPHANUMERIC);
    (
        [
            // 브라우저가 열지 않고 내려받게 한다(파일 내용은 믿지 않는다).
            (
                axum::http::header::CONTENT_TYPE,
                "application/octet-stream".to_owned(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename*=UTF-8''{encoded}"),
            ),
            (
                axum::http::header::X_CONTENT_TYPE_OPTIONS,
                "nosniff".to_owned(),
            ),
            (axum::http::header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        bytes,
    )
        .into_response()
}

/// 보관함에서 뺀다(파일은 30일 규칙으로 돌아간다).
async fn file_unkeep(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(fid): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let removed = state.store().unkeep_file(&fid).unwrap_or(false);
    let (tone, text) = if removed {
        let _ = state.store().add_activity(
            "file_keep",
            &format!("보관함에서 뺌(관리 페이지): {fid}"),
            None,
            Utc::now(),
        );
        (
            Tone::Ok,
            "보관함에서 뺐습니다. 받거나 만든 지 30일이 지난 파일은 다음 정리 때 지워집니다."
                .to_owned(),
        )
    } else {
        (Tone::Bad, "보관함에 없는 파일입니다.".to_owned())
    };
    flash(&state, &id, tone, text);
    Redirect::to(back_path(f.back.as_deref())).into_response()
}

/// 챙기던 마감을 처리함으로(결정 0034).
async fn nudge_done(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(nid): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let text = crate::attention::done(&state.store(), &nid, None, Utc::now());
    flash(&state, &id, Tone::Ok, text);
    Redirect::to(back_path(f.back.as_deref())).into_response()
}

#[derive(Deserialize)]
struct EgressForm {
    csrf: String,
    mode: String,
}

/// 출구 검사 모드(결정 0029): 기록만(기본) / 막기.
async fn egress_mode(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<EgressForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let (tone, text) = match f.mode.as_str() {
        m @ ("log" | "block") => {
            match state
                .store()
                .set_setting(crate::leak::MODE_KEY, m, Utc::now())
            {
                Ok(()) => (
                    Tone::Ok,
                    if m == "block" {
                        "출구 검사: 사용자 자료가 실린 주소·입력은 열지 않거나 승인을 받습니다."
                            .to_owned()
                    } else {
                        "출구 검사: 기록만 합니다.".to_owned()
                    },
                ),
                Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
            }
        }
        _ => (Tone::Bad, "알 수 없는 모드입니다.".to_owned()),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Safety.path()).into_response()
}

async fn routine_action(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path((rid, verb)): Path<(String, String)>,
    Form(f): Form<CsrfForm>,
) -> Response {
    use lucia_store::RoutineStatus as S;
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let now = Utc::now();
    let tz = state.config.tz().unwrap_or(chrono_tz::Asia::Seoul);
    let result: Result<String, String> = (|| {
        let mut store = state.store();
        let r = store
            .routine(&rid)
            .map_err(|e| e.to_string())?
            .ok_or("없는 루틴입니다.")?;
        match verb.as_str() {
            "pause" if r.status == S::Active => {
                store
                    .set_routine_status(&rid, S::Paused, now)
                    .map_err(|e| e.to_string())?;
                crate::routines::disarm(&store, &rid).map_err(|e| e.to_string())?;
                Ok(format!("{}: 멈춤", r.name))
            }
            // 다시 켜기와(관리 페이지에서의) 승인: 켜고 다음 실행을 건다. 이어진 실패 수는 0으로.
            "resume" if matches!(r.status, S::Paused | S::Review) => {
                store
                    .set_routine_status(&rid, S::Active, now)
                    .map_err(|e| e.to_string())?;
                let mut on = r.clone();
                on.status = S::Active;
                on.next_at = None;
                let next = crate::routines::arm(&store, &on, now, tz).map_err(|e| e.to_string())?;
                Ok(match next {
                    Some(at) => format!(
                        "{}: 켬, 다음 실행 {}",
                        r.name,
                        at.with_timezone(&tz).format("%m-%d %H:%M")
                    ),
                    None => format!("{}: 켬(새 변화가 있을 때 돕니다)", r.name),
                })
            }
            "run" if r.status == S::Active => {
                let job = crate::worker::Job::Routine(crate::scheduler::DueJob {
                    schedule_id: "manual".into(),
                    action: crate::routines::ACTION.into(),
                    payload: serde_json::json!({ "id": rid }),
                    due_at: now,
                });
                state
                    .jobs
                    .try_send(job)
                    .map_err(|e| format!("작업 큐에 넣지 못했습니다: {e}"))?;
                Ok(format!(
                    "{}: 지금 실행합니다. 결과는 아래 기록에 남습니다.",
                    r.name
                ))
            }
            "remove" => {
                crate::routines::disarm(&store, &rid).map_err(|e| e.to_string())?;
                store.delete_routine(&rid).map_err(|e| e.to_string())?;
                Ok(format!("{}: 지움", r.name))
            }
            _ => Err("지금 상태에서 할 수 없는 조작입니다.".to_owned()),
        }
    })();
    if result.is_ok() {
        let _ = state.store().add_activity(
            "routine_admin",
            &format!("루틴 {rid}: {verb}(관리 페이지)"),
            None,
            now,
        );
    }
    let (tone, text) = match result {
        Ok(t) => (Tone::Ok, t),
        Err(e) => (Tone::Bad, e),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Automation.path()).into_response()
}

// ---------- 스킬(결정 0018) ----------

async fn skills_page(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    if !logged_in(&state, &headers) {
        return Redirect::to("/login").into_response();
    }
    let list = state.store().skills(&[]).unwrap_or_default();
    show(&state, &headers, Page::Skills, extra::skills(&list)).await
}

async fn skill_page(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Response {
    if !logged_in(&state, &headers) {
        return Redirect::to("/login").into_response();
    }
    let (skill, revisions) = {
        let store = state.store();
        (
            store.skill(&slug).ok().flatten(),
            store.skill_revisions(&slug).unwrap_or_default(),
        )
    };
    let Some(skill) = skill else {
        return Redirect::to("/skills").into_response();
    };
    show(
        &state,
        &headers,
        Page::Skills,
        extra::skill_detail(&skill, &revisions),
    )
    .await
}

/// 스킬 조작(사람): 고정·풀기·끄기·되살리기·승인.
async fn skill_action(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path((slug, verb)): Path<(String, String)>,
    Form(f): Form<CsrfForm>,
) -> Response {
    use lucia_store::SkillStatus as S;
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let now = Utc::now();
    let result = {
        let store = state.store();
        let skill = store.skill(&slug).ok().flatten();
        let done =
            |r: lucia_store::Result<bool>, text: String| r.map(|_| text).map_err(|e| e.to_string());
        match (skill, verb.as_str()) {
            (None, _) => Err("없는 스킬입니다.".to_owned()),
            (Some(_), "pin") => done(
                store.set_skill_pinned(&slug, true, now),
                "고정했습니다.".into(),
            ),
            (Some(_), "unpin") => done(
                store.set_skill_pinned(&slug, false, now),
                "고정을 풀었습니다.".into(),
            ),
            (Some(_), "archive") => done(
                store.set_skill_status(&slug, S::Archived, now),
                "껐습니다(보관).".into(),
            ),
            (Some(s), "restore") => {
                // 사이트·외부 쓰기가 든 스킬을 보관에서 되살리면 다시 승인을 거친다.
                let to = if crate::skills::needs_approval(&s.footprint) && s.status == S::Archived {
                    S::Pending
                } else {
                    S::Trial
                };
                done(
                    store.set_skill_status(&slug, to, now),
                    format!("되살렸습니다({}).", to.label()),
                )
            }
            (Some(s), "approve") if s.status == S::Pending => {
                let to = if s.source == "chat" {
                    S::Active
                } else {
                    S::Trial
                };
                done(
                    store.set_skill_status(&slug, to, now),
                    format!("승인했습니다({}).", to.label()),
                )
            }
            _ => Err("할 수 없는 조작입니다.".to_owned()),
        }
    };
    if result.is_ok() {
        let _ = state.store().add_activity(
            "skill_admin",
            &format!("스킬 {slug}: {verb}(관리 페이지)"),
            None,
            now,
        );
    }
    let (tone, text) = match result {
        Ok(t) => (Tone::Ok, t),
        Err(e) => (Tone::Bad, e),
    };
    flash(&state, &id, tone, text);
    Redirect::to(&format!("/skills/{slug}")).into_response()
}

// ---------- 화면 넘겨받기(결정 0017) ----------

#[derive(Deserialize)]
struct HandoverQuery {
    #[serde(default)]
    tab: Option<String>,
    #[serde(default)]
    auto: Option<String>,
}

async fn handover_page(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Query(q): Query<HandoverQuery>,
) -> Response {
    if !logged_in(&state, &headers) {
        return Redirect::to("/login").into_response();
    }
    let tools = state.browser_tools.get().cloned();
    let (left, tabs) = match &tools {
        Some(t) => {
            t.expire_handover().await;
            match t.handover_left() {
                Some(left) => (Some(left), t.handover_tabs().await.unwrap_or_default()),
                None => (None, Vec::new()),
            }
        }
        None => (None, Vec::new()),
    };
    let tab = q
        .tab
        .filter(|id| tabs.iter().any(|t| &t.id == id))
        .or_else(|| tabs.first().map(|t| t.id.clone()));
    let auto = q.auto.is_some();
    let available = tools.is_some();
    // 하는 중이고 자동 새로 고침을 켰으면 화면이 3초마다 다시 묻는다(`refresh`).
    let refresh = (auto && left.is_some()).then_some(3);
    show(
        &state,
        &headers,
        Page::Browser,
        extra::handover(available, left, &tabs, tab.as_deref(), auto, refresh),
    )
    .await
}

async fn handover_start(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let Some(tools) = state.browser_tools.get() else {
        flash(&state, &id, Tone::Bad, "브라우저가 꺼져 있습니다.".into());
        return Redirect::to("/browser/handover").into_response();
    };
    if let Err(e) = tools.begin_handover().await {
        flash(&state, &id, Tone::Bad, format!("넘겨받지 못했습니다: {e}"));
    } else {
        tracing::info!("관리: 브라우저 화면 넘겨받기 시작");
    }
    Redirect::to("/browser/handover").into_response()
}

async fn handover_end(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    if let Some(tools) = state.browser_tools.get() {
        let (tone, text) = match tools.end_handover("사용자가 끝냄").await {
            Ok(()) => (
                Tone::Ok,
                "넘겨받기를 끝냈습니다. Lucia가 다시 브라우저를 씁니다.".to_owned(),
            ),
            Err(e) => (
                Tone::Warn,
                format!("끝냈지만 탭을 정리하지 못했습니다: {e}"),
            ),
        };
        flash(&state, &id, tone, text);
    }
    tracing::info!("관리: 브라우저 화면 넘겨받기 끝");
    Redirect::to("/browser/handover").into_response()
}

async fn handover_frame(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(tab): Path<String>,
) -> Response {
    if !logged_in(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(tools) = state.browser_tools.get() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let tab = tab.trim_end_matches(".jpg");
    match tools.handover_frame(tab).await {
        Ok(frame) => (
            [
                (axum::http::header::CONTENT_TYPE, "image/jpeg"),
                (axum::http::header::CACHE_CONTROL, "no-store"),
            ],
            frame.jpeg,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct HandoverAct {
    csrf: String,
    tab: String,
    kind: String,
    #[serde(rename = "at.x", default)]
    x: Option<u32>,
    #[serde(rename = "at.y", default)]
    y: Option<u32>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    dy: i32,
}

/// 넘겨받은 화면에 입력을 보낸다. 넣은 글과 누른 자리는 기록하지 않는다.
async fn handover_act(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<HandoverAct>,
) -> Response {
    use crate::browser::HandInput;
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let back = format!(
        "/browser/handover?tab={}",
        url::form_urlencoded::byte_serialize(f.tab.as_bytes()).collect::<String>()
    );
    let Some(tools) = state.browser_tools.get() else {
        return Redirect::to(&back).into_response();
    };
    let input = match f.kind.as_str() {
        "click" => match (f.x, f.y) {
            (Some(x), Some(y)) => Some(HandInput::Click { x, y }),
            _ => None,
        },
        "text" if !f.text.is_empty() && f.text.chars().count() <= 2000 => {
            Some(HandInput::Text(f.text.clone()))
        }
        "key" => Some(HandInput::Key(f.key.clone())),
        "scroll" => Some(HandInput::Scroll(f.dy.clamp(-3000, 3000))),
        _ => None,
    };
    if let Some(input) = input
        && let Err(e) = tools.handover_input(&f.tab, &input).await
    {
        flash(&state, &id, Tone::Bad, format!("보내지 못했습니다: {e}"));
    }
    // 화면이 바뀔 시간을 조금 준다.
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    Redirect::to(&back).into_response()
}

// ---------- 캐릭터(결정 0009) ----------

async fn character_page(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    if !logged_in(&state, &headers) {
        return Redirect::to("/login").into_response();
    }
    let loaded = match state.character_file().await {
        Ok(file) => file
            .read()
            .map(|content| (file.path.display().to_string(), content)),
        Err(e) => Err(e),
    };
    let history = state.store().character_revisions(30).unwrap_or_default();
    let pace = state
        .store()
        .setting(crate::segments::PACE_KEY)
        .ok()
        .flatten()
        .and_then(|v| crate::segments::Pace::parse(&v))
        .unwrap_or(crate::segments::Pace::DEFAULT);
    let models = state.models();
    show(
        &state,
        &headers,
        Page::Character,
        extra::character(&loaded, &history, pace, &models),
    )
    .await
}

#[derive(Deserialize)]
struct PaceForm {
    csrf: String,
    pace: String,
}

/// 메시지 나눠 보내기 간격(결정 0026). 다음 답부터 적용된다.
async fn character_pace(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<PaceForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let (tone, text) = match crate::segments::Pace::parse(&f.pace) {
        None => (Tone::Bad, "알 수 없는 간격입니다.".to_owned()),
        Some(pace) => {
            match state
                .store()
                .set_setting(crate::segments::PACE_KEY, pace.key(), Utc::now())
            {
                Ok(()) => {
                    tracing::info!(pace = pace.key(), "관리: 메시지 간격 저장");
                    (
                        Tone::Ok,
                        format!(
                            "메시지 사이 간격을 ‘{}’(으)로 저장했습니다. 다음 답부터 적용됩니다.",
                            pace.label()
                        ),
                    )
                }
                Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
            }
        }
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Character.path()).into_response()
}

#[derive(Deserialize)]
struct ModelForm {
    csrf: String,
    model: String,
}

/// 주 모델(결정 0055). 다음 실행부터 적용된다.
async fn character_model(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<ModelForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let wanted = f.model.trim();
    let to = if crate::models::DEFAULT_WORDS.contains(&wanted) {
        Some(None)
    } else {
        crate::models::parse(wanted).map(Some)
    };
    let (tone, text) = match to {
        None => (Tone::Bad, "알 수 없는 모델입니다.".to_owned()),
        Some(to) => {
            let models = state.models();
            let changed = models.set(&state.store(), to, "관리 페이지", Utc::now());
            match changed {
                Ok(true) => (
                    Tone::Ok,
                    format!(
                        "주 모델을 ‘{}’(으)로 바꿨습니다. {}",
                        models.label(),
                        crate::models::CHANGE_NOTE
                    ),
                ),
                Ok(false) => (Tone::Ok, format!("이미 ‘{}’입니다.", models.label())),
                Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
            }
        }
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Character.path()).into_response()
}

#[derive(Deserialize)]
struct CharacterForm {
    csrf: String,
    content: String,
    base: String,
    #[serde(default)]
    note: String,
}

fn saved_flash(result: Result<crate::character::Saved, String>) -> (Tone, String) {
    match result {
        Ok(saved) => {
            let mut text = match saved.revision {
                Some(r) => format!(
                    "저장했습니다(개정 {r}). 상주 대화에는 다음 세션 정리(압축)나 새 세션부터 적용됩니다. 바로 적용하려면 ‘지금 정리해 적용’을 누르세요."
                ),
                None => "바뀐 것이 없어 그대로 두었습니다.".to_owned(),
            };
            for w in &saved.warnings {
                text.push(' ');
                text.push_str(w);
            }
            let tone = if saved.warnings.is_empty() {
                Tone::Ok
            } else {
                Tone::Warn
            };
            (tone, text)
        }
        Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
    }
}

async fn character_save(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<CharacterForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = match state.character_file().await {
        Ok(file) => file.save(&state.store, &f.content, &f.base, &f.note, Utc::now()),
        Err(e) => Err(e),
    };
    if let Ok(saved) = &result {
        tracing::info!(revision = ?saved.revision, "관리: 캐릭터 파일 저장");
    }
    let (tone, text) = saved_flash(result);
    flash(&state, &id, tone, text);
    Redirect::to(Page::Character.path()).into_response()
}

async fn character_revision_page(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(rev): Path<i64>,
) -> Response {
    if !logged_in(&state, &headers) {
        return Redirect::to("/login").into_response();
    }
    let found = state.store().character_revision(rev).ok().flatten();
    let current = match state.character_file().await {
        Ok(file) => file.read().ok(),
        Err(_) => None,
    };
    match found {
        Some(r) => {
            let is_current = current.as_deref() == Some(r.content.as_str());
            show(
                &state,
                &headers,
                Page::Character,
                extra::character_revision(&r, is_current),
            )
            .await
        }
        None => {
            let page = show(
                &state,
                &headers,
                Page::Character,
                extra::character_revision_missing(),
            )
            .await;
            (StatusCode::NOT_FOUND, page).into_response()
        }
    }
}

async fn character_restore(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Path(rev): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = match state.character_file().await {
        Ok(file) => file.restore(&state.store, rev, Utc::now()),
        Err(e) => Err(e),
    };
    tracing::info!(rev, ok = result.is_ok(), "관리: 캐릭터 파일 되돌리기");
    let (tone, text) = saved_flash(result);
    flash(&state, &id, tone, text);
    Redirect::to(Page::Character.path()).into_response()
}

// ---------- 브라우저(결정 0006) ----------

#[derive(Deserialize)]
struct SiteQuery {
    site: String,
}

async fn browser_forget(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Query(q): Query<SiteQuery>,
    Form(f): Form<CsrfForm>,
) -> Response {
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = match state.logins.get() {
        Some(logins) => logins.forget(&q.site).await,
        None => Err(anyhow::anyhow!("브라우저가 꺼져 있다")),
    };
    let text = match &result {
        Ok(()) => {
            tracing::info!(site = %q.site, "관리: 저장된 사이트 로그인 지움");
            "저장된 로그인을 지웠습니다.".to_owned()
        }
        Err(err) => format!("지우지 못했습니다: {err:#}"),
    };
    flash(&state, &id, tone_of(&result), text);
    Redirect::to(Page::Browser.path()).into_response()
}

#[derive(Deserialize)]
struct BlockedForm {
    csrf: String,
    #[serde(default)]
    list: String,
}

/// 차단 목록 저장(결정 0008): 형식을 확인하고 저장한 뒤 바로 프록시에 적용한다.
async fn browser_blocked(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(f): Form<BlockedForm>,
) -> Response {
    use crate::browser::egress::{Egress, SETTING_BLOCKED};
    let id = match authorize(&state, &headers, &f.csrf) {
        Ok(id) => id,
        Err(r) => return r.into_response(),
    };
    let result = match (state.browser_tools.get(), Egress::parse_list(&f.list)) {
        (None, _) => Err("브라우저가 꺼져 있다".to_owned()),
        (_, Err(e)) => Err(e),
        (Some(tools), Ok(list)) => {
            let saved = state
                .store()
                .set_setting(SETTING_BLOCKED, &list.join("\n"), Utc::now());
            match saved {
                Ok(()) => {
                    let n = list.len();
                    tools.egress().set_user(list);
                    tracing::info!(count = n, "관리: 브라우저 차단 목록을 바꿈");
                    Ok(n)
                }
                Err(e) => Err(format!("저장하지 못했다: {e}")),
            }
        }
    };
    let (tone, text) = match result {
        Ok(n) => (
            Tone::Ok,
            format!("차단 목록을 저장했습니다({n}개). 바로 적용됩니다."),
        ),
        Err(e) => (Tone::Bad, format!("저장하지 않았습니다: {e}")),
    };
    flash(&state, &id, tone, text);
    Redirect::to(Page::Browser.path()).into_response()
}

#[derive(Deserialize)]
struct SiteLoginForm {
    username: String,
    password: String,
    #[serde(default)]
    save: Option<String>,
}

async fn site_login_page(
    State(state): State<Arc<AdminState>>,
    Path(token): Path<String>,
) -> Response {
    use crate::browser::logins::{FormOutcome, FormStatus};
    let minutes = state.config.browser.login_input_minutes;
    let Some(logins) = state.logins.get() else {
        return Standalone::SiteLoginInvalid.respond_with(StatusCode::NOT_FOUND);
    };
    // 결과 화면(결정 0038): 로그인은 요청과 떨어져 돌고, 이 화면이 상태를 보인다.
    match logins.form_status(&token) {
        FormStatus::Invalid => {
            Standalone::SiteLoginInvalid.respond_with(StatusCode::NOT_FOUND)
        }
        FormStatus::Ready { site, error } => Standalone::SiteLoginForm {
            site: site.name.clone(),
            origin: site.origin.clone(),
            token,
            minutes,
            attempts: crate::browser::logins::INPUT_ATTEMPTS,
            error: error
                .map(|e| crate::mask::global().mask(&format!("로그인하지 못했습니다: {e}"))),
        }
        .respond(),
        // 화면이 몇 초마다 다시 묻는다.
        FormStatus::Busy { site_name } => Standalone::SiteLoginBusy { site: site_name }.respond(),
        FormStatus::Finished(FormOutcome::Done { site_name, note }) => Standalone::SiteLoginDone {
            site: site_name,
            note,
        }
        .respond(),
        FormStatus::Finished(FormOutcome::Pending {
            site_name,
            note,
            code_link,
        }) => Standalone::SiteLoginPending {
            site: site_name,
            note,
            code_link,
        }
        .respond(),
        FormStatus::Finished(FormOutcome::Failed { reason, .. }) => Standalone::SiteLoginFailed {
            site: "사이트".into(),
            reason: crate::mask::global().mask(&format!(
                "{reason}. 이 링크로 {}번 시도해 닫았습니다. 다시 하려면 Lucia에게 로그인을 다시 요청해 주세요.",
                crate::browser::logins::INPUT_ATTEMPTS
            )),
        }
        .respond(),
        FormStatus::Finished(_) => {
            Standalone::SiteLoginInvalid.respond_with(StatusCode::NOT_FOUND)
        }
    }
}

#[derive(Deserialize)]
struct SiteCodeForm {
    code: String,
}

async fn site_code_page(
    State(state): State<Arc<AdminState>>,
    Path(token): Path<String>,
) -> Response {
    match state.logins.get().and_then(|l| l.code_form_peek(&token)) {
        Some(site) => Standalone::SiteCodeForm {
            site: site.name.clone(),
            origin: site.origin.clone(),
            token,
            error: None,
        }
        .respond(),
        None => Standalone::SiteLoginInvalid.respond_with(StatusCode::NOT_FOUND),
    }
}

async fn site_code_submit(
    State(state): State<Arc<AdminState>>,
    Path(token): Path<String>,
    Form(f): Form<SiteCodeForm>,
) -> Response {
    let Some(logins) = state.logins.get() else {
        return Standalone::SiteLoginInvalid.respond_with(StatusCode::NOT_FOUND);
    };
    let Some(site) = logins.code_form_peek(&token) else {
        return Standalone::SiteLoginInvalid.respond_with(StatusCode::NOT_FOUND);
    };
    // 코드와 토큰은 로그에 남기지 않는다. 브라우저 쪽 일은 요청과 떨어진 작업으로 돌린다(결정 0038): 제출을 다시
    // 눌러 이 요청이 버려져도 코드 입력은 끝까지 한다.
    let job = {
        let logins = logins.clone();
        let token = token.clone();
        tokio::spawn(async move { logins.code_form_submit(&token, &f.code).await })
    };
    let result = job
        .await
        .unwrap_or_else(|e| Err(format!("코드 입력 작업이 멈췄다: {e}")));
    match result {
        Ok(note) => {
            tracing::info!(site = %site.name, "관리: 인증 코드 입력 → 로그인함");
            Standalone::SiteLoginDone {
                site: site.name.clone(),
                note,
            }
            .respond()
        }
        Err(why) => {
            tracing::info!("관리: 인증 코드 입력 → 실패");
            Standalone::SiteLoginFailed {
                site: site.name.clone(),
                reason: why,
            }
            .respond()
        }
    }
}

async fn site_login_submit(
    State(state): State<Arc<AdminState>>,
    Path(token): Path<String>,
    Form(f): Form<SiteLoginForm>,
) -> Response {
    let Some(logins) = state.logins.get() else {
        return Standalone::SiteLoginInvalid.respond_with(StatusCode::NOT_FOUND);
    };
    // 토큰, 아이디, 비밀번호는 로그에 남기지 않는다. 로그인은 떨어진 작업으로 돌리고(결정 0038) 결과 화면으로 보낸다
    // (POST → 303 → GET): 수십 초 걸리는 요청을 붙잡지 않으니 연결이 끊기거나 다시 보내져도 로그인이 버려지지 않는다.
    if logins.spawn_submit(&token, f.username, f.password, f.save.is_some()) {
        tracing::info!("관리: 사이트 로그인 입력 → 시작(또는 이미 하는 중)");
        Redirect::to(&format!("/site-login/{token}")).into_response()
    } else {
        Standalone::SiteLoginInvalid.respond_with(StatusCode::NOT_FOUND)
    }
}

pub fn router(state: Arc<AdminState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/connections", get(connections_page))
        .route("/start", get(start_page))
        .route("/start/dismiss", post(start_dismiss))
        .route("/start/reopen", post(start_reopen))
        .route("/profile/name", post(profile_name))
        .route(
            "/profile/avatar",
            post(profile_avatar_upload).layer(DefaultBodyLimit::max(PROFILE_UPLOAD_LIMIT)),
        )
        .route("/profile/avatar/remove", post(profile_avatar_remove))
        .route("/profile/avatar/file/{id}", get(profile_avatar_file))
        .route("/activity", get(activity_page))
        .route("/system", get(system_page))
        .route("/work", get(work_page))
        .route("/automation", get(automation_page))
        .route("/browser", get(browser_page))
        .route("/settings", get(settings_safety_page))
        .route("/settings/notify", get(settings_notify_page))
        .route("/settings/work", get(settings_work_page))
        .route("/assets/{*name}", get(spa::asset))
        .route("/favicon.svg", get(spa::favicon))
        .route("/setup", get(setup_form).post(setup_submit))
        .route("/login", get(login_form).post(login_submit))
        .route("/logout", post(logout))
        .route("/discord/token", post(discord_token))
        .route("/discord/token/remove", post(discord_token_remove))
        .route("/discord/owner", post(discord_owner))
        .route("/discord/owner/remove", post(discord_owner_remove))
        .route("/discord/test", post(discord_test))
        .route("/discord/guild-calls", post(discord_guild_calls))
        .route("/checks/refresh", post(checks_refresh))
        .route("/google/client", post(google_client))
        .route("/google/client/upload", post(google_client_upload))
        .route("/google/connect", post(google_connect))
        .route("/google/paste", post(google_paste))
        .route("/google/disconnect", post(google_disconnect))
        .route("/run/precheck", post(run_precheck))
        .route("/run/briefing", post(run_briefing))
        .route("/run/backup", post(run_backup))
        .route("/run/compact", post(run_compact))
        .route("/browser/forget", post(browser_forget))
        .route("/browser/blocked", post(browser_blocked))
        .route("/character", get(character_page).post(character_save))
        .route("/character/pace", post(character_pace))
        .route("/character/model", post(character_model))
        .route("/autonomy/limits", post(autonomy_limits))
        .route("/followups/{fid}/cancel", post(followup_cancel))
        .route("/goals/{gid}/{verb}", post(goal_action))
        .route(
            "/followups/{fid}/allow-browser",
            post(followup_allow_browser),
        )
        .route("/tasks/{tid}/cancel", post(task_cancel))
        .route("/tasks/channel", post(task_channel))
        .route("/watches/{wid}/{verb}", post(watch_action))
        .route("/watch-edit/{wid}", post(watch_edit))
        .route("/watch-settings", post(watch_settings))
        .route("/security/mode", post(security_mode))
        .route("/totp/start", post(totp_start))
        .route("/totp/confirm", post(totp_confirm))
        .route("/totp/disable", post(totp_disable))
        .route("/routines/{rid}/{verb}", post(routine_action))
        .route("/grants/add", post(grant_add))
        .route("/grants/{gid}/revoke", post(grant_revoke))
        .route("/egress/mode", post(egress_mode))
        .route("/signals/settings", post(signals_settings))
        .route("/signals/rules/delete", post(notice_rule_delete))
        .route("/quiet/calendar", post(quiet_calendar))
        .route("/nudges/{nid}/done", post(nudge_done))
        .route("/clients/new", post(client_new))
        .route("/clients/{cid}/revoke", post(client_revoke))
        .route("/clients/{cid}/scopes", post(client_scopes))
        .route("/app/notify", post(app_notify))
        .route("/app/push-preview", post(app_push_preview))
        .route("/app/fcm", post(app_fcm))
        .route("/app/fcm/remove", post(app_fcm_remove))
        .route("/app/push-test/{cid}", post(app_push_test))
        .route("/files/{fid}/download", get(file_download))
        .route("/files/{fid}/unkeep", post(file_unkeep))
        .route("/images/key", post(images_key))
        .route("/images/key/remove", post(images_key_remove))
        .route("/images/budget", post(images_budget))
        .route("/expressions", get(expressions_page))
        .route("/expressions/file/{id}", get(expression_file))
        .route("/expressions/add", post(expression_add))
        .route("/expressions/{eid}/update", post(expression_update))
        .route("/expressions/{eid}/delete", post(expression_delete))
        .route(
            "/expressions/categories/save",
            post(expression_category_save),
        )
        .route(
            "/expressions/categories/{cid}/delete",
            post(expression_category_delete),
        )
        .route("/dev/settings", post(dev_settings))
        .route("/skills", get(skills_page))
        .route("/skills/{slug}", get(skill_page))
        .route("/skills/{slug}/{verb}", post(skill_action))
        .route("/browser/handover", get(handover_page))
        .route("/browser/handover/start", post(handover_start))
        .route("/browser/handover/end", post(handover_end))
        .route("/browser/handover/act", post(handover_act))
        .route("/browser/handover/frame/{tab}", get(handover_frame))
        .route("/wrapup/settings", post(wrapup_settings))
        .route("/character/rev/{rev}", get(character_revision_page))
        .route("/character/rev/{rev}/restore", post(character_restore))
        .route("/memory", get(memory_page))
        .route("/memory/tell", post(memory_tell))
        .route("/memory/{slug}", get(memory_entry_page))
        .route("/memory/{slug}/delete", post(memory_delete))
        .route("/memory/{slug}/rev/{rev}/restore", post(memory_restore))
        // 로그인 세션 없이 여는 일회용 입력 링크(Discord에서 넘어오면 SameSite=Strict 쿠키가 실리지 않는다).
        // 토큰이 곧 권한이다: 10분, 한 사이트, 성공하면 닫히고 실패하면 3번까지(결정 0006·0038 [제안]).
        .route(
            "/site-login/{token}",
            get(site_login_page).post(site_login_submit),
        )
        // 인증 코드 입력(결정 0017): 토큰이 곧 권한이다(5분, 한 번).
        .route(
            "/site-login/code/{token}",
            get(site_code_page).post(site_code_submit),
        )
        .fallback(spa::fallback)
        // 안쪽: 틀 내주기와 리다이렉트 바꾸기. 바깥: 출처·Host 확인과 보안 머리.
        .layer(middleware::from_fn_with_state(state.clone(), spa::layer))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// 바인드 주소마다 띄운다.
pub async fn serve(state: Arc<AdminState>, cancel: CancellationToken) -> anyhow::Result<()> {
    for addr in state.config.admin.bind.clone() {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("관리 페이지 바인드 {addr}: {e}"))?;
        let app = router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(async move { cancel.cancelled_owned().await })
                .await;
            if let Err(err) = result {
                tracing::error!(%err, %addr, "관리 페이지가 멈췄다");
            }
        });
        tracing::info!(%addr, "관리 페이지");
    }
    Ok(())
}

#[cfg(test)]
mod password_tests {
    use super::*;

    #[test]
    fn password_hash_roundtrip() {
        let hash = hash_password("correct horse battery").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("wrong", &hash));
        assert!(!verify_password("x", "not a hash"));
    }

    #[test]
    fn sha256_is_hex() {
        assert_eq!(sha256_hex("abc").len(), 64);
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
