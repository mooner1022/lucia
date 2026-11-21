//! 관리 화면의 틀(결정 0061). 화면은 정적 Svelte 빌드이고, 데몬은 세 가지를 한다.
//!
//! - **틀 내주기**: 브라우저가 주소로 들어오면(HTML을 바라는 GET) 어느 주소든 같은 `index.html`을 준다.
//!   그 뒤 화면이 같은 주소를 `Accept: application/json`으로 다시 물어 그릴 것을 받는다. 그래서 처리기는
//!   JSON 요청에서만 돈다(일회용 링크나 Google 동의 처리도 화면이 물을 때 한 번만 된다).
//! - **화면 데이터**: 로그인한 화면은 [`page`], 로그인 없이 여는 화면은 [`Standalone`].
//! - **쓰기 동작의 답**: 처리기는 예전처럼 "안내를 남기고 리다이렉트"한다. [`layer`]가 JSON을 바라는 요청의
//!   리다이렉트를 `{"redirect", "flash"}`로 바꿔 준다. 처리기 74개와 그 시험은 그대로다.
//!
//! 틀과 정적 파일은 로그인 없이 열린다(비밀이 없다. 다만 관리 주소의 목록이 들어 있다).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use lucia_store::ApprovalStatus;
use serde::Serialize;
use serde_json::{Value, json};

use super::snapshot::{self, Flash, Page, Snapshot};
use super::{AdminState, session_id};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/admin_ui.rs"));
}

/// 화면 빌드가 바이너리에 들어 있는가(`luciad check`).
pub fn built() -> bool {
    embedded::BUILT
}

const NOT_BUILT: &str = "<!doctype html><html lang=\"ko\"><meta charset=\"utf-8\"><title>Lucia 관리</title><body><h1>관리 화면이 빌드되지 않았습니다</h1><p>이 바이너리에는 화면이 들어 있지 않습니다. <code>deploy/install.sh</code>로 설치하거나 <code>deploy/build-ui.sh</code>를 돌린 뒤 다시 빌드하세요.</p></body></html>";

fn file(name: &str) -> Option<&'static [u8]> {
    embedded::FILES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, bytes)| *bytes)
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "json" | "map" => "application/json",
        "png" => "image/png",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// 화면의 틀(`index.html`). 캐시하지 않는다(새 빌드의 파일 이름을 바로 알게).
pub fn shell() -> Response {
    let body: &'static [u8] = file("index.html").unwrap_or(NOT_BUILT.as_bytes());
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

/// 틀이 가리키는 파일이 모두 바이너리에 들어 있는가(배포 뒤 점검). 빠진 것의 이름을 돌려준다.
pub fn missing_assets() -> Vec<String> {
    let Some(index) = file("index.html") else {
        return vec!["index.html".into()];
    };
    let html = String::from_utf8_lossy(index);
    let mut missing = Vec::new();
    for part in html.split(['"', '\'']) {
        if let Some(name) = part.strip_prefix('/')
            && name.starts_with("assets/")
            && file(name).is_none()
        {
            missing.push(name.to_owned());
        }
    }
    missing
}

/// 빌드가 만든 파일(이름에 내용 해시가 들어 있어 오래 캐시한다).
pub async fn asset(Path(name): Path<String>) -> Response {
    let name = format!("assets/{name}");
    match file(&name) {
        Some(bytes) => (
            [
                (header::CONTENT_TYPE, content_type(&name)),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn favicon() -> Response {
    match file("favicon.svg") {
        Some(bytes) => (
            [
                (header::CONTENT_TYPE, "image/svg+xml"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// 화면이 그릴 것을 묻는 요청인가.
pub fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json"))
}

/// 데몬이 직접 내주는 주소(파일, 그림). 틀로 바꿔 답하지 않는다.
fn raw_path(path: &str) -> bool {
    path == "/favicon.svg"
        || [
            "/assets/",
            "/files/",
            "/expressions/file/",
            "/profile/avatar/file/",
            "/browser/handover/frame/",
        ]
        .iter()
        .any(|p| path.starts_with(p))
}

/// 틀 내주기와 리다이렉트 바꾸기(모듈 설명 참고).
pub async fn layer(State(state): State<Arc<AdminState>>, req: Request, next: Next) -> Response {
    let json = wants_json(req.headers());
    let get = req.method() == Method::GET || req.method() == Method::HEAD;
    if get && !json && !raw_path(req.uri().path()) {
        return shell();
    }
    #[cfg(test)]
    let fixture = if get {
        state
            .fixture
            .get()
            .and_then(|fixture| fixture(&state, req.headers(), req.uri()))
    } else {
        None
    };
    #[cfg(not(test))]
    let fixture: Option<Response> = None;
    let session = session_id(req.headers());
    let response = match fixture {
        Some(response) => response,
        None => next.run(req).await,
    };
    let response = with_name(&state, response).await;
    if !(json && response.status().is_redirection()) {
        return response;
    }
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("/")
        .to_owned();
    // 안내는 이 답에 실어 보낸다(다음 화면을 받을 때까지 세션에 두지 않는다).
    let flash = session.and_then(|id| {
        state
            .sessions
            .lock()
            .expect("sessions lock")
            .get_mut(&id)
            .and_then(|s| s.flash.take())
    });
    // 화면을 물었는데 로그인으로 가라는 답이면 401로 알린다(쓰기 요청의 리다이렉트는 정상 처리다).
    let status = if get && location == "/login" {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::OK
    };
    let mut out = (
        status,
        Json(json!({ "redirect": location, "flash": flash })),
    )
        .into_response();
    for cookie in response.headers().get_all(header::SET_COOKIE) {
        out.headers_mut().append(header::SET_COOKIE, cookie.clone());
    }
    out
}

/// 없는 주소. 화면이 물었으면 "없는 화면"으로 답한다.
pub async fn fallback(headers: HeaderMap) -> Response {
    if wants_json(&headers) {
        Standalone::NotFound.respond_with(StatusCode::NOT_FOUND)
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// 로그인한 화면 하나의 데이터.
pub fn page(page: Page, s: &Snapshot, csrf: &str, flash: Option<&Flash>, extra: Value) -> Value {
    let notices = snapshot::notices(s);
    let (tone, label) = snapshot::overall(&notices);
    let pending = s
        .approvals
        .iter()
        .filter(|a| a.status == ApprovalStatus::Pending)
        .count();
    let mut snap = serde_json::to_value(s).unwrap_or(Value::Null);
    super::derive::decorate(&mut snap, s);
    json!({
        "kind": "page",
        "page": page,
        "path": page.path(),
        "title": page.label(),
        "name": s.profile.name,
        "csrf": csrf,
        "flash": flash,
        "snapshot": snap,
        "extra": extra,
        "notices": notices.iter().map(|n| json!({ "tone": n.tone, "text": n.text, "page": n.page.path() })).collect::<Vec<_>>(),
        "overall": { "tone": tone, "label": label },
        "pending": pending,
        "consts": super::derive::consts(),
    })
}

/// 로그인 없이 여는 화면. 화면 쪽은 `screen.type`으로 가른다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Standalone {
    /// 로그인. `external`이면 2단계 인증 코드 칸도 보인다.
    Login {
        error: Option<String>,
        external: bool,
    },
    /// 비밀번호가 아직 없다(`luciad admin-setup` 안내).
    NoPassword,
    /// 초기 비밀번호 정하기.
    Setup {
        token: String,
        error: Option<String>,
        min_chars: usize,
    },
    SetupInvalid,
    /// Google 동의에서 돌아왔다.
    GoogleReturned {
        ok: bool,
        text: String,
    },
    /// 사이트 로그인 입력(일회용 링크).
    SiteLoginForm {
        site: String,
        origin: String,
        token: String,
        minutes: i64,
        /// 같은 링크로 다시 넣을 수 있는 횟수.
        attempts: u32,
        error: Option<String>,
    },
    /// 로그인하는 중. 화면이 몇 초마다 다시 묻는다.
    SiteLoginBusy {
        site: String,
    },
    SiteLoginDone {
        site: String,
        note: String,
    },
    /// 인증 코드를 기다린다.
    SiteLoginPending {
        site: String,
        note: String,
        code_link: Option<String>,
    },
    SiteLoginFailed {
        site: String,
        reason: String,
    },
    SiteLoginInvalid,
    /// 인증 코드 입력(일회용 링크).
    SiteCodeForm {
        site: String,
        origin: String,
        token: String,
        error: Option<String>,
    },
    NotFound,
}

/// 로그인 없이 여는 화면의 답이라는 표지. [`layer`]가 이 답에 이름을 싣는다(처리기마다 이름을 넘기지 않아도 된다).
#[derive(Clone, Copy)]
struct StandaloneMark;

impl Standalone {
    pub fn respond(self) -> Response {
        self.respond_with(StatusCode::OK)
    }

    pub fn respond_with(self, status: StatusCode) -> Response {
        let mut response = (
            status,
            Json(json!({ "kind": "standalone", "screen": self })),
        )
            .into_response();
        response.extensions_mut().insert(StandaloneMark);
        response
    }
}

/// 로그인 없이 여는 화면의 답에 보이는 이름(`name`)을 싣는다(결정 0064). 화면의 머리글과 탭 제목이 쓴다.
/// 처리기가 끝난 뒤라 저장소를 잠가도 안전하다.
async fn with_name(state: &AdminState, response: Response) -> Response {
    if response.extensions().get::<StandaloneMark>().is_none() {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    parts.extensions.remove::<StandaloneMark>();
    let Ok(bytes) = axum::body::to_bytes(body, 1 << 20).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if let Some(map) = value.as_object_mut() {
        let name = crate::profile::display_name(&state.store());
        map.insert("name".into(), json!(name));
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, axum::body::Body::from(value.to_string()))
}

/// 화면의 스크립트·스타일은 같은 출처의 파일만 쓴다(결정 0061: 결정 0042에서 뺐던 CSP를 다시 건다).
/// 인라인 스크립트가 없으므로 이스케이프에 구멍이 나도 주입된 스크립트는 돌지 않는다.
pub const CSP: HeaderValue = HeaderValue::from_static(
    "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
);
