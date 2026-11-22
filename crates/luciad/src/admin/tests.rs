//! 라우터 시험: 실제 라우터에 요청을 보내 로그인·CSRF·Host·보안 헤더·돌아갈 화면 제한을 확인한다.
//! 외부 호출(sbx 점검, Google, Discord)은 하지 않는다(점검은 미리 채운 캐시를 쓴다).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tokio::sync::mpsc;
use tower::ServiceExt;

use super::*;

const HOST: &str = "127.0.0.1:18781";
const SID: &str = "test-session";
const CSRF: &str = "good-csrf";
const JSON: &str = "application/json";

struct Fixture {
    state: Arc<AdminState>,
    jobs: mpsc::Receiver<crate::worker::Job>,
    _dir: TempDir,
}

struct TempDir(std::path::PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn fixture() -> Fixture {
    fixture_with("").await
}

/// 공개 주소(결정 0043)가 있는 설정.
const PUBLIC_HOST: &str = "lucia.example.com";
const PUBLIC: &str = r#"
[public]
admin_url = "https://lucia.example.com"
trusted_proxies = ["172.18.0.11"]
"#;

async fn fixture_with(extra: &str) -> Fixture {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::temp_dir().join(format!(
        "lucia-admin-test-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.display();
    let cfg = Config::parse(&format!(
        r#"
data_dir = "{d}/data"
credentials_dir = "{d}/creds"

[sandbox]
name = "lucia"
workdir = "{d}/ws"

[admin]
bind = ["{HOST}"]
{extra}
"#
    ))
    .unwrap();
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let creds = Arc::new(Credentials::new(
        cfg.credentials_dir.clone(),
        crate::mask::global().clone(),
    ));
    let (inbound, _) = mpsc::channel(1);
    let (presses, _) = mpsc::channel(1);
    let discord = DiscordControl::new(store.clone(), creds.clone(), inbound, presses);
    let google = GoogleAuth::new(creds.clone(), reqwest::Client::new());
    let (jobs_tx, jobs) = mpsc::channel(8);
    let state = AdminState::new(store, discord, google, creds, Arc::new(cfg), jobs_tx);
    // 점검은 sbx를 부르므로 캐시를 미리 채운다.
    state
        .store_checks(vec![Check {
            name: "저장소",
            ok: true,
            detail: "정상".into(),
        }])
        .await;
    Fixture {
        state,
        jobs,
        _dir: TempDir(dir),
    }
}

impl Fixture {
    fn log_in(&self) {
        self.state.sessions.lock().unwrap().insert(
            SID.into(),
            Session {
                csrf: CSRF.into(),
                expires: Instant::now() + Duration::from_secs(600),
                flash: None,
                external: false,
            },
        );
    }

    /// 프록시(172.18.0.11)를 거쳐 밖(`ip`)에서 온 요청.
    async fn send_outside(&self, mut req: Request<Body>, ip: &str) -> Response {
        req.headers_mut()
            .insert("x-forwarded-for", ip.parse().unwrap());
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([172, 18, 0, 11], 50000))));
        router(self.state.clone()).oneshot(req).await.unwrap()
    }

    async fn send(&self, mut req: Request<Body>) -> Response {
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
        router(self.state.clone()).oneshot(req).await.unwrap()
    }
}

/// 화면이 데이터를 묻는 GET(`Accept: application/json`). 처리기가 돈다(결정 0061).
fn get(path: &str, logged_in: bool) -> Request<Body> {
    get_as(path, logged_in, true)
}

/// 브라우저가 주소로 들어올 때의 GET(HTML을 바란다). 데몬은 처리기를 돌리지 않고 화면의 틀을 내준다.
/// 파일·그림 주소(`/files/…`, `/expressions/file/…`)는 이 요청으로 받는다(`<img>`와 내려받기는 JSON을 바라지 않는다).
fn get_html(path: &str, logged_in: bool) -> Request<Body> {
    get_as(path, logged_in, false)
}

fn get_as(path: &str, logged_in: bool, json: bool) -> Request<Body> {
    let mut b = Request::get(path).header("host", HOST);
    if json {
        b = b.header("accept", JSON);
    }
    if logged_in {
        b = b.header("cookie", format!("{COOKIE}={SID}"));
    }
    b.body(Body::empty()).unwrap()
}

/// 폼을 보내는 POST. JSON을 바라지 않으면 처리기의 답(리다이렉트)을 그대로 받는다.
fn post(path: &str, logged_in: bool, form: &str) -> Request<Body> {
    post_as(path, logged_in, form, false)
}

/// 화면이 보내는 POST(`Accept: application/json`): 리다이렉트는 `{"redirect", "flash"}`로 바뀐다.
fn post_json(path: &str, logged_in: bool, form: &str) -> Request<Body> {
    post_as(path, logged_in, form, true)
}

fn post_as(path: &str, logged_in: bool, form: &str, json: bool) -> Request<Body> {
    let mut b = Request::post(path)
        .header("host", HOST)
        .header("content-type", "application/x-www-form-urlencoded");
    if json {
        b = b.header("accept", JSON);
    }
    if logged_in {
        b = b.header("cookie", format!("{COOKIE}={SID}"));
    }
    b.body(Body::from(form.to_owned())).unwrap()
}

/// 화면 하나를 따로 스레드(자기 런타임)에서 그린다. 교착되면 시험 스레드는 막히지 않고 `None`을 받는다
/// (같은 스레드에서 그리면 막힌 동기 잠금이 런타임까지 멈춰 시간 제한도 듣지 않는다).
fn render_within(state: Arc<AdminState>, path: &str, secs: u64) -> Option<StatusCode> {
    let path = path.to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let status = rt.block_on(async move {
            let mut req = get(&path, true);
            req.extensions_mut()
                .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
            router(state).oneshot(req).await.unwrap().status()
        });
        let _ = tx.send(status);
    });
    rx.recv_timeout(Duration::from_secs(secs)).ok()
}

fn location(r: &Response) -> &str {
    r.headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

async fn body(r: Response) -> String {
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// 답의 본문을 JSON으로 읽는다.
async fn json(r: Response) -> Value {
    let text = body(r).await;
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("JSON이 아니다({e}): {text}"))
}

/// 로그인한 세션으로 화면 하나의 데이터(`kind == "page"`)를 받는다. 이 요청이 안내를 꺼낸다.
async fn page_json(f: &Fixture, path: &str) -> Value {
    let r = f.send(get(path, true)).await;
    assert_eq!(r.status(), StatusCode::OK, "{path}");
    let v = json(r).await;
    assert_eq!(v["kind"], "page", "{path}");
    v
}

/// 로그인 없이 여는 화면(`kind == "standalone"`)의 `screen`.
fn screen(v: &Value) -> &Value {
    assert_eq!(v["kind"], "standalone", "{v}");
    &v["screen"]
}

/// 안내 한 줄의 글(없으면 빈 글).
fn flash_text(v: &Value) -> &str {
    v["flash"]["text"].as_str().unwrap_or("")
}

/// 상태를 바꾸는 POST(로그아웃 제외)와 각자 필요한 필드.
const POSTS: &[(&str, &str)] = &[
    ("/security/mode", "&mode=open"),
    ("/discord/token", "&token=x"),
    ("/discord/token/remove", ""),
    ("/discord/owner", "&user_id=1"),
    ("/discord/owner/remove", ""),
    ("/discord/test", ""),
    ("/discord/guild-calls", "&prefix=!x"),
    ("/checks/refresh", ""),
    ("/google/client", "&json=%7B%7D"),
    ("/google/connect", ""),
    ("/google/paste", "&url=x"),
    ("/google/disconnect", ""),
    ("/run/precheck", ""),
    ("/run/briefing", ""),
    ("/run/backup", ""),
    ("/browser/forget?site=quasarzone", ""),
    ("/memory/tell", "&text=x"),
    ("/memory/x/delete", ""),
    ("/memory/x/rev/1/restore", ""),
    ("/grants/add", "&host=example.com"),
    ("/grants/x/revoke", ""),
    ("/egress/mode", "&mode=block"),
    (
        "/signals/settings",
        "&poll_minutes=5&classify_daily=1&wake_daily=1",
    ),
    ("/run/compact", ""),
    ("/character", "&content=x&base=y"),
    ("/character/rev/1/restore", ""),
    ("/autonomy/limits", "&daily=1"),
    ("/followups/followup-x/cancel", ""),
    ("/goals/goal-x/abandon", ""),
    ("/quiet/calendar", "&on=1"),
    ("/signals/rules/delete", "&key=a@b.c"),
    ("/nudges/nudge-x/done", ""),
    ("/followups/followup-x/allow-browser", ""),
    ("/tasks/task-x/cancel", ""),
    ("/tasks/channel", "&channel=1"),
    ("/wrapup/settings", "&keep_days=30"),
    ("/browser/handover/start", ""),
    ("/skills/x/pin", ""),
    ("/watches/w1/pause", ""),
    ("/routines/routine-x/pause", ""),
    ("/dev/settings", "&cost_cap=2"),
    ("/clients/new", "&name=x"),
    ("/clients/cl-x/revoke", ""),
    ("/files/f_x/unkeep", ""),
    ("/images/key", "&key=x"),
    ("/images/key/remove", ""),
    ("/images/budget", "&daily=1"),
    ("/start/dismiss", ""),
    ("/start/reopen", ""),
    ("/profile/name", "&name=x"),
    ("/profile/avatar/remove", ""),
    ("/expressions/e_x/update", ""),
    ("/expressions/e_x/delete", ""),
    ("/expressions/categories/save", "&id=a&label=b"),
    ("/expressions/categories/a/delete", ""),
    ("/browser/handover/end", ""),
    ("/browser/handover/act", "&tab=x&kind=key&key=enter"),
];

#[tokio::test]
async fn pages_need_login() {
    let f = fixture().await;
    for path in [
        "/",
        "/connections",
        "/activity",
        "/memory",
        "/memory/x",
        "/character",
        "/character/rev/1",
        "/system",
        "/work",
        "/automation",
        "/browser",
        "/settings",
        "/settings/notify",
        "/settings/work",
        "/browser/handover",
        "/skills",
        "/skills/x",
        "/start",
    ] {
        // 화면이 데이터를 물으면 로그인으로 가라고 401로 답한다(결정 0061).
        let r = f.send(get(path, false)).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{path}");
        let v = json(r).await;
        assert_eq!(v["redirect"], "/login", "{path}");
        assert!(v["flash"].is_null(), "{path}");
        assert!(
            v.get("snapshot").is_none(),
            "{path}: 데이터는 나가지 않는다"
        );
    }
    // 넘겨받은 화면 이미지는 로그인해야 받는다(결정 0017). 그림 주소는 틀로 바뀌지 않고 처리기가 답한다.
    assert_eq!(
        f.send(get_html("/browser/handover/frame/x.jpg", false))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn foreign_host_is_rejected() {
    let f = fixture().await;
    let req = Request::get("/login")
        .header("host", "evil.example")
        .body(Body::empty())
        .unwrap();
    assert_eq!(f.send(req).await.status(), StatusCode::BAD_REQUEST);
}

/// 모든 답에 붙는 보안 머리(CSP 포함, 결정 0061).
fn assert_security_headers(r: &Response, what: &str) {
    let h = r.headers();
    assert_eq!(
        h.get("content-security-policy"),
        Some(&spa::CSP),
        "{what}: CSP"
    );
    assert_eq!(h.get("x-frame-options").unwrap(), "DENY", "{what}");
    assert_eq!(
        h.get("x-content-type-options").unwrap(),
        "nosniff",
        "{what}"
    );
    assert_eq!(h.get("referrer-policy").unwrap(), "no-referrer", "{what}");
}

#[tokio::test]
async fn security_headers_and_static_files() {
    let f = fixture().await;
    // 화면 데이터(로그인 없이 여는 화면, 로그인한 화면)와 틀 모두에 붙는다.
    let r = f.send(get("/login", false)).await;
    assert_security_headers(&r, "로그인 화면 데이터");
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
    f.log_in();
    let r = f.send(get("/", true)).await;
    assert_security_headers(&r, "로그인한 화면 데이터");
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
    let r = f.send(get_html("/", false)).await;
    assert_security_headers(&r, "틀");
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
    // 없는 파일도, 틀이 아닌 답도 마찬가지다.
    let r = f.send(get_html("/assets/nope.js", false)).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    assert_security_headers(&r, "없는 파일");
    let r = f.send(post("/login", false, "password=x")).await;
    assert_security_headers(&r, "POST 답");

    // 정적 파일은 빌드가 만든 `/assets/…`뿐이다. 없는 파일은 틀이 아니라 404다(시험에는 화면 빌드가 없다).
    for accept_json in [true, false] {
        let r = f.send(get_as("/assets/nope.js", false, accept_json)).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "json={accept_json}");
    }
    let r = f.send(get_html("/favicon.svg", false)).await;
    if spa::built() {
        assert_eq!(r.headers().get("content-type").unwrap(), "image/svg+xml");
    } else {
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }
    // 예전 `/static/…`은 없다: 스타일 시트가 아니다(JSON을 바라면 없는 화면, HTML을 바라면 틀).
    for path in ["/static/app.css", "/static/favicon.svg"] {
        let r = f.send(get(path, false)).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{path}");
        let v = json(r).await;
        assert_eq!(screen(&v)["type"], "not_found", "{path}");
        let r = f.send(get_html(path, false)).await;
        let kind = r
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(!kind.starts_with("text/css"), "{path}: {kind}");
        assert!(!kind.starts_with("image/"), "{path}: {kind}");
        assert!(!body(r).await.contains(":root"), "{path}");
    }
}

#[tokio::test]
async fn logged_in_pages_render_and_flash_shows_once() {
    let f = fixture().await;
    f.log_in();
    // (요청 주소, 화면 키, 화면 주소)
    for (path, page, page_path) in [
        ("/", "overview", "/"),
        ("/connections", "connections", "/connections"),
        ("/activity?kind=mail", "activity", "/activity"),
        ("/system", "system", "/system"),
        ("/work", "work", "/work"),
        ("/automation", "automation", "/automation"),
        ("/browser", "browser", "/browser"),
        ("/settings", "safety", "/settings"),
        ("/settings/notify", "notify", "/settings/notify"),
        ("/settings/work", "tasks", "/settings/work"),
        ("/memory?q=%EA%B8%B0", "memory", "/memory"),
        ("/skills", "skills", "/skills"),
        ("/expressions", "expressions", "/expressions"),
        ("/character", "character", "/character"),
        ("/browser/handover", "browser", "/browser"),
        ("/start", "start", "/start"),
    ] {
        let v = page_json(&f, path).await;
        assert_eq!(v["page"], page, "{path}");
        assert_eq!(
            v["name"], "Lucia",
            "{path}: 보이는 이름이 실린다(결정 0064)"
        );
        assert_eq!(v["path"], page_path, "{path}");
        assert_eq!(v["csrf"], CSRF, "{path}");
        assert!(v["title"].as_str().is_some_and(|t| !t.is_empty()), "{path}");
        assert!(v["snapshot"].is_object(), "{path}");
        assert!(v["flash"].is_null(), "{path}");
        assert!(
            v["notices"].is_array() && v["overall"].is_object(),
            "{path}"
        );
        assert!(v["pending"].is_number(), "{path}");
    }
    // 화면마다 다른 덧붙임: 활동은 고른 거르개, 기억은 찾는 말.
    let v = page_json(&f, "/activity?kind=mail").await;
    assert_eq!(v["extra"]["filter"], "mail");
    let v = page_json(&f, "/memory?q=%EA%B8%B0").await;
    assert_eq!(v["extra"]["view"], "list");
    assert_eq!(v["extra"]["query"], "기");

    // 안내는 글 그대로 실려 한 번만 보인다(이스케이프는 화면의 몫이다).
    flash(&f.state, SID, Tone::Bad, "<b>실패</b>".into());
    let v = page_json(&f, "/").await;
    assert_eq!(v["flash"]["text"], "<b>실패</b>");
    assert_eq!(v["flash"]["tone"], "bad");
    let v = page_json(&f, "/").await;
    assert!(v["flash"].is_null(), "안내는 한 번만");
}

#[tokio::test]
async fn every_post_checks_login_and_csrf() {
    let f = fixture().await;
    for (path, extra) in POSTS {
        let r = f
            .send(post(path, false, &format!("csrf={CSRF}{extra}")))
            .await;
        assert_eq!(location(&r), "/login", "로그인 없이 {path}");
    }
    f.log_in();
    for (path, extra) in POSTS {
        let r = f.send(post(path, true, &format!("csrf=bad{extra}"))).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "틀린 CSRF로 {path}");
    }
    // 파일 올리기(multipart)도 같다.
    let boundary = "XBOUNDARY";
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\nbad\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"c.json\"\r\nContent-Type: application/json\r\n\r\n{{}}\r\n\
         --{boundary}--\r\n"
    );
    let req = Request::post("/google/client/upload")
        .header("host", HOST)
        .header("cookie", format!("{COOKIE}={SID}"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(multipart))
        .unwrap();
    assert_eq!(f.send(req).await.status(), StatusCode::FORBIDDEN);
}

/// 화면이 로그인 없이(또는 세션이 끝난 채) 데이터를 물으면 로그인으로 가라고 401로 알린다. 데이터는 나가지 않는다.
#[tokio::test]
async fn json_get_without_login_answers_401_with_login_redirect() {
    let f = fixture().await;
    // 세션이 없다. 쿠키가 없을 때와, 서버가 모르는(끝난) 세션 쿠키일 때.
    for logged_in in [false, true] {
        let r = f.send(get("/work", logged_in)).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "cookie={logged_in}");
        assert_eq!(
            r.headers().get("content-type").unwrap(),
            "application/json",
            "cookie={logged_in}"
        );
        assert_security_headers(&r, "401");
        let v = json(r).await;
        assert_eq!(v["redirect"], "/login");
        assert!(v["flash"].is_null());
        assert!(
            v.get("snapshot").is_none() && v.get("csrf").is_none(),
            "{v}"
        );
    }
    // 로그인하면 같은 주소가 데이터로 답한다.
    f.log_in();
    assert_eq!(page_json(&f, "/work").await["page"], "work");
    // 세션이 끝나면(만료) 다시 401이다.
    f.state
        .sessions
        .lock()
        .unwrap()
        .get_mut(SID)
        .unwrap()
        .expires = Instant::now();
    let r = f.send(get("/work", true)).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json(r).await["redirect"], "/login");
}

/// 화면이 보내는 쓰기 요청(JSON): 처리기의 리다이렉트는 `{"redirect", "flash"}`로 바뀌고, 안내는 이 답이 가져간다.
#[tokio::test]
async fn json_post_converts_the_redirect_and_flash_is_not_repeated() {
    let mut f = fixture().await;
    f.log_in();
    let r = f
        .send(post_json(
            "/images/key",
            true,
            &form(&[("csrf", CSRF), ("key", "short")]),
        ))
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers().get("location").is_none(), "리다이렉트가 아니다");
    assert_eq!(r.headers().get("content-type").unwrap(), "application/json");
    assert_security_headers(&r, "JSON POST");
    let v = json(r).await;
    assert_eq!(v["redirect"], "/connections");
    assert_eq!(v["flash"]["tone"], "bad");
    assert!(flash_text(&v).contains("키를 저장하지 않았습니다"), "{v}");
    assert!(
        f.state.sessions.lock().unwrap()[SID].flash.is_none(),
        "안내는 이 답이 가져갔다"
    );
    // 다음 화면에는 같은 안내가 다시 나오지 않는다.
    let v = page_json(&f, "/connections").await;
    assert!(v["flash"].is_null(), "안내는 한 번만: {v}");

    // 성공한 요청도 같다(성공 안내, 돌아갈 화면은 고른 것).
    let r = f
        .send(post_json(
            "/run/backup",
            true,
            &format!("csrf={CSRF}&back=%2Fsystem"),
        ))
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json(r).await;
    assert_eq!(v["redirect"], "/system");
    assert_eq!(v["flash"]["tone"], "info");
    assert!(
        flash_text(&v).contains("세션 기록 백업을(를) 작업 큐에 넣었습니다"),
        "{v}"
    );
    assert!(matches!(
        f.jobs.try_recv(),
        Ok(crate::worker::Job::Routine(_))
    ));
    // 알 수 없는 돌아갈 곳은 홈으로 돌린다.
    let r = f
        .send(post_json(
            "/run/compact",
            true,
            &format!("csrf={CSRF}&back=https%3A%2F%2Fevil.example%2F"),
        ))
        .await;
    assert_eq!(json(r).await["redirect"], "/");

    // JSON을 바라지 않는 POST는 그대로 303이고, 안내는 세션에 남아 다음 화면이 한 번 보인다.
    let r = f
        .send(post(
            "/images/key",
            true,
            &form(&[("csrf", CSRF), ("key", "short")]),
        ))
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/connections");
    assert!(f.state.sessions.lock().unwrap()[SID].flash.is_some());
    let v = page_json(&f, "/connections").await;
    assert!(flash_text(&v).contains("키를 저장하지 않았습니다"));
    assert!(page_json(&f, "/connections").await["flash"].is_null());

    // 로그인하지 않은 쓰기 요청의 리다이렉트도 바뀐다(처리된 요청이라 200이고, 로그인 화면으로 가라는 뜻이다).
    for (path, extra) in POSTS {
        let r = f
            .send(post_json(path, false, &format!("csrf={CSRF}{extra}")))
            .await;
        assert_eq!(r.status(), StatusCode::OK, "{path}");
        let v = json(r).await;
        assert_eq!(v["redirect"], "/login", "{path}");
        assert!(v["flash"].is_null(), "{path}");
    }
}

/// JSON을 바라는 쓰기 요청도 CSRF 토큰이 틀리면 거절하고(403) 아무것도 바꾸지 않는다.
#[tokio::test]
async fn json_post_with_wrong_csrf_is_refused_and_changes_nothing() {
    let mut f = fixture().await;
    f.log_in();
    for (path, extra) in POSTS {
        let r = f
            .send(post_json(path, true, &format!("csrf=bad{extra}")))
            .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "틀린 CSRF로 {path}");
        assert!(r.headers().get("location").is_none(), "{path}");
        assert_security_headers(&r, path);
        assert!(!body(r).await.contains("redirect"), "{path}");
    }
    // 파일 올리기(multipart)도 같다.
    let boundary = "XBOUNDARY";
    let multipart_body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\nbad\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"c.json\"\r\nContent-Type: application/json\r\n\r\n{{}}\r\n\
         --{boundary}--\r\n"
    );
    let req = Request::post("/google/client/upload")
        .header("host", HOST)
        .header("accept", JSON)
        .header("cookie", format!("{COOKIE}={SID}"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(multipart_body))
        .unwrap();
    assert_eq!(f.send(req).await.status(), StatusCode::FORBIDDEN);
    let mut req = multipart(&[("csrf", "bad"), ("kind", "emoji"), ("name", "x")], None);
    req.headers_mut().insert("accept", JSON.parse().unwrap());
    assert_eq!(f.send(req).await.status(), StatusCode::FORBIDDEN);

    // 아무것도 바뀌지 않았다: 작업도 활동 기록도 안내도 설정도 없다.
    assert!(f.jobs.try_recv().is_err(), "큐에 들어간 작업이 없다");
    {
        // 저장소 잠금은 아래 `.await` 전에 놓는다(블록으로 범위를 닫는다).
        let store = f.state.store();
        assert!(store.recent_activity(5).unwrap().is_empty());
        assert_eq!(
            crate::security::mode(&store),
            crate::security::Mode::Standard,
            "권한 모드가 그대로다"
        );
        assert!(store.site_grants().unwrap().is_empty());
        assert!(store.clients().unwrap().is_empty());
    }
    assert!(f.state.sessions.lock().unwrap()[SID].flash.is_none());

    // 대조: 같은 요청이 맞는 토큰이면 처리된다.
    let r = f
        .send(post_json(
            "/security/mode",
            true,
            &format!("csrf={CSRF}&mode=strict"),
        ))
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        crate::security::mode(&f.state.store()),
        crate::security::Mode::Strict
    );
}

#[tokio::test]
async fn run_buttons_queue_jobs_and_return_only_to_known_pages() {
    let mut f = fixture().await;
    f.log_in();
    let r = f
        .send(post(
            "/run/backup",
            true,
            &format!("csrf={CSRF}&back=%2Fsystem"),
        ))
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/system");
    let Some(crate::worker::Job::Routine(job)) = f.jobs.recv().await else {
        panic!("작업이 큐에 들어가야 한다");
    };
    assert_eq!(job.action, crate::worker::actions::SESSION_BACKUP);

    for evil in [
        "https%3A%2F%2Fevil.example%2F",
        "%2F%2Fevil.example",
        "%2Fsystem%2F..%2F",
    ] {
        let r = f
            .send(post(
                "/run/compact",
                true,
                &format!("csrf={CSRF}&back={evil}"),
            ))
            .await;
        assert_eq!(location(&r), "/", "{evil}");
    }
    let Some(crate::worker::Job::Routine(job)) = f.jobs.recv().await else {
        panic!("작업");
    };
    assert_eq!(job.action, crate::worker::actions::COMPACT);
}

/// 비밀번호를 정해 둔 상태(`correct horse battery`).
fn set_password(f: &Fixture) {
    let hash = hash_password("correct horse battery").unwrap();
    f.state
        .store()
        .set_admin_password_hash(&hash, Utc::now())
        .unwrap();
}

/// 답의 `Set-Cookie`에서 `이름=값`만(요청의 `Cookie`에 그대로 쓴다).
fn cookie_pair(r: &Response) -> String {
    r.headers()
        .get("set-cookie")
        .expect("세션 쿠키")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn login_shows_errors_inline_and_sets_a_strict_cookie() {
    let f = fixture().await;
    set_password(&f);
    // 틀린 비밀번호: 처리기는 JSON 요청이 아니어도 로그인 화면 데이터로 답한다(401).
    let r = f.send(post("/login", false, "password=wrong")).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert!(r.headers().get("set-cookie").is_none());
    let v = json(r).await;
    let login = screen(&v);
    assert_eq!(login["type"], "login");
    assert_eq!(login["error"], "비밀번호가 틀렸습니다.");
    assert_eq!(login["external"], false);

    let r = f
        .send(post("/login", false, "password=correct+horse+battery"))
        .await;
    // 대화할 곳이 아직 없는 새 설치라 첫 화면은 초기 설정이다(결정 0064).
    assert_eq!(location(&r), "/start");
    let cookie = r.headers().get("set-cookie").unwrap().to_str().unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
}

/// 화면이 보내는 로그인(JSON POST): 맞으면 `{"redirect": 첫 화면}`과 세션 쿠키, 틀리면 오류가 실린 로그인 화면(401).
/// 첫 화면은 새 설치(대화할 곳이 없음)면 초기 설정이다(결정 0064. 규칙은 `login_lands_on_the_setup_screen_only_while_it_is_unfinished`).
#[tokio::test]
async fn login_by_json_post_answers_redirect_or_login_screen() {
    let f = fixture().await;
    // 비밀번호가 아직 없으면 그렇게 알린다.
    let r = f.send(get("/login", false)).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json(r).await;
    assert_eq!(screen(&v)["type"], "no_password");

    set_password(&f);
    let r = f.send(get("/login", false)).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json(r).await;
    assert_eq!(screen(&v)["type"], "login");
    assert!(screen(&v)["error"].is_null());
    assert_eq!(screen(&v)["external"], false);

    // 실패: 401과 로그인 화면(오류 포함), 세션은 만들지 않는다.
    let r = f.send(post_json("/login", false, "password=wrong")).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert!(r.headers().get("set-cookie").is_none());
    let v = json(r).await;
    assert!(v.get("redirect").is_none(), "{v}");
    assert_eq!(screen(&v)["type"], "login");
    assert_eq!(screen(&v)["error"], "비밀번호가 틀렸습니다.");
    assert!(f.state.sessions.lock().unwrap().is_empty());

    // 성공: 200 `{"redirect": "/start"}`(새 설치). 리다이렉트가 바뀌어도 쿠키는 남는다.
    let r = f
        .send(post_json("/login", false, "password=correct+horse+battery"))
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    let cookie = r.headers().get("set-cookie").unwrap().to_str().unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    assert!(cookie.contains(&format!("{COOKIE}=")));
    let pair = cookie_pair(&r);
    let v = json(r).await;
    assert_eq!(v["redirect"], "/start");
    assert!(v["flash"].is_null());

    // 그 쿠키로 화면을 받을 수 있고, 로그인한 채 로그인 주소를 물으면 첫 화면으로 보낸다.
    let mut req = get("/", false);
    req.headers_mut().insert("cookie", pair.parse().unwrap());
    let r = f.send(req).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json(r).await;
    assert_eq!(v["kind"], "page");
    assert_eq!(v["page"], "overview");
    assert!(v["csrf"].as_str().is_some_and(|c| !c.is_empty()));
    let mut req = get("/login", false);
    req.headers_mut().insert("cookie", pair.parse().unwrap());
    let r = f.send(req).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(json(r).await["redirect"], "/start");
}

#[tokio::test]
async fn google_return_page_needs_no_session() {
    let f = fixture().await;
    let r = f.send(get("/?code=c&state=unknown", false)).await;
    assert_eq!(r.status(), StatusCode::OK);
    let text = body(r).await;
    let v: Value = serde_json::from_str(&text).unwrap();
    let back = screen(&v);
    assert_eq!(back["type"], "google_returned");
    assert_eq!(back["ok"], false);
    assert!(back["text"].as_str().unwrap().contains("찾을 수 없습니다"));
    // 로그인 없이 열리는 화면이라 세션에 묶인 값(CSRF 토큰)은 싣지 않는다.
    assert!(!text.contains("csrf"), "{text}");
}

/// 틀 내주기(결정 0061): HTML을 바라는 GET은 어느 주소든(로그인 없이도) 같은 틀을 받고, 처리기는 돌지 않는다.
#[tokio::test]
async fn shell_is_served_for_html_gets_without_login_or_handlers() {
    let f = fixture().await;
    let shell_body = body(spa::shell()).await;
    assert!(shell_body.to_lowercase().contains("<html"));
    for path in [
        "/",
        "/memory/x",
        "/work",
        "/login",
        "/site-login/nope",
        "/no/such/page",
    ] {
        for logged_in in [false, true] {
            if logged_in {
                f.log_in();
            }
            let r = f.send(get_html(path, logged_in)).await;
            assert_eq!(r.status(), StatusCode::OK, "{path}");
            assert!(
                r.headers()
                    .get("content-type")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("text/html"),
                "{path}"
            );
            assert_security_headers(&r, path);
            assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
            let text = body(r).await;
            assert_eq!(text, shell_body, "{path}: 같은 틀");
            assert!(!text.contains("\"kind\""), "{path}: 데이터가 아니다");
        }
    }
    // HEAD도 틀이다(본문만 없다).
    let r = f
        .send(
            Request::head("/memory/x")
                .header("host", HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_security_headers(&r, "HEAD");
    // JSON으로 물으면 그때서야 처리기가 돈다: 로그인하지 않았으면 401, 로그인했으면 없는 기억 항목은 404.
    assert_eq!(
        f.send(get("/memory/x", false)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.send(get("/memory/x", true)).await.status(),
        StatusCode::NOT_FOUND
    );
}

/// 틀을 내줄 때 처리기는 돌지 않는다: 브라우저가 Google 동의에서 돌아오는 주소를 HTML로 열면 일회용 동의를 쓰지 않고,
/// 화면이 같은 주소를 JSON으로 물을 때 한 번만 쓴다.
#[tokio::test]
async fn shell_does_not_consume_the_google_callback() {
    let f = fixture().await;
    let pending = || crate::google_auth::PendingConsent {
        state: "y".into(),
        verifier: "v".into(),
        redirect_uri: "http://127.0.0.1:18781/".into(),
        started: Instant::now(),
    };
    f.state
        .consents
        .lock()
        .unwrap()
        .insert("y".into(), pending());

    let r = f.send(get_html("/?code=x&state=y", false)).await;
    assert_eq!(r.status(), StatusCode::OK);
    let text = body(r).await;
    assert_eq!(text, body(spa::shell()).await);
    assert!(!text.contains("google_returned"), "{text}");
    assert!(
        f.state.consents.lock().unwrap().contains_key("y"),
        "틀을 내줄 때 동의를 쓰지 않는다"
    );
    // 오류로 돌아온 경우도 같다.
    let r = f.send(get_html("/?error=access_denied", false)).await;
    assert_eq!(body(r).await, text);

    // JSON으로 물으면 처리기가 돈다: 일회용 동의를 꺼내 쓴다(클라이언트 JSON이 없어 연결은 실패한다).
    let r = f.send(get("/?code=x&state=y", false)).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json(r).await;
    let back = screen(&v);
    assert_eq!(back["type"], "google_returned");
    assert_eq!(back["ok"], false);
    assert!(f.state.consents.lock().unwrap().is_empty());
    // 두 번째는 이미 썼다.
    let v = json(f.send(get("/?code=x&state=y", false)).await).await;
    assert_eq!(screen(&v)["type"], "google_returned");
    assert!(
        screen(&v)["text"]
            .as_str()
            .unwrap()
            .contains("찾을 수 없습니다")
    );
}

/// 화면이 묻는 주소가 없으면(JSON) "없는 화면"으로 답한다. 없는 주소를 HTML로 열면 틀이다.
#[tokio::test]
async fn unknown_paths_answer_not_found_to_json() {
    let f = fixture().await;
    for logged_in in [false, true] {
        if logged_in {
            f.log_in();
        }
        let r = f.send(get("/no/such/page", logged_in)).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        assert_security_headers(&r, "없는 화면");
        let v = json(r).await;
        assert_eq!(screen(&v)["type"], "not_found");
    }
    // 쓰기 요청도 없는 주소면 같다.
    let r = f.send(post_json("/no/such/page", true, "csrf=x")).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    // HTML을 바라는 요청은 틀이다(화면이 주소를 보고 "없는 화면"을 그린다).
    let r = f.send(get_html("/no/such/page", false)).await;
    assert_eq!(r.status(), StatusCode::OK);
}

/// 사이트 로그인 입력 링크(결정 0006): 로그인 세션 없이 열리고, 한 번만 쓰이고, 캐시·리퍼러를 남기지 않는다.
#[tokio::test]
async fn site_login_link_works_once_without_a_session() {
    use crate::browser::fake::{FakeBrowser, FakeNotifier, FakeSecrets};
    use crate::browser::logins::{LoginsConfig, SiteLogins};
    let f = fixture().await;
    let notes = Arc::new(FakeNotifier::default());
    let browser = Arc::new(FakeBrowser::default());
    let logins = Arc::new(SiteLogins::new(
        browser.clone(),
        Arc::new(FakeSecrets::default()),
        f.state.store.clone(),
        notes.clone(),
        None,
        crate::browser::sites::builtin(),
        LoginsConfig {
            form_base: format!("http://{HOST}"),
            input_ttl: chrono::Duration::minutes(10),
            approval_ttl: chrono::Duration::minutes(30),
        },
    ));
    f.state.set_logins(logins.clone());

    // 브라우저가 붙은 상태로 모든 화면이 그려져야 한다(저장소 잠금 교착 회귀 시험, 2026-11-17).
    f.log_in();
    for page in [
        "/",
        "/connections",
        "/activity",
        "/memory",
        "/character",
        "/system",
        "/work",
        "/automation",
        "/browser",
        "/settings",
        "/settings/notify",
        "/settings/work",
    ] {
        let status = render_within(f.state.clone(), page, 5)
            .unwrap_or_else(|| panic!("{page}: 화면이 끝나지 않는다(저장소 잠금 교착)"));
        assert_eq!(status, StatusCode::OK, "{page}");
    }

    let r = f.send(get("/site-login/nope", false)).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    assert_eq!(screen(&json(r).await)["type"], "site_login_invalid");
    logins.request("quasarzone", "시험", None).await.unwrap();
    let link = notes.0.lock().unwrap()[0].clone();
    let path = link.rsplit(HOST).next().unwrap().trim().to_owned();
    assert!(path.starts_with("/site-login/"), "{path}");

    let r = f.send(get(&path, false)).await;
    assert_eq!(r.status(), StatusCode::OK, "로그인 세션 없이 열린다");
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
    assert_eq!(r.headers().get("referrer-policy").unwrap(), "no-referrer");
    let v = json(r).await;
    let form = screen(&v);
    assert_eq!(form["type"], "site_login_form");
    assert_eq!(form["site"], "퀘이사존");
    assert_eq!(form["token"], path.trim_start_matches("/site-login/"));
    assert!(form["error"].is_null());

    // 제출은 바로 결과 화면으로 보내고(POST → 303 → GET), 로그인은 요청과 떨어져 돈다(결정 0038).
    let r = f
        .send(post(
            &path,
            false,
            "username=me&password=p%40ss+word&save=1",
        ))
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers().get("location").unwrap(), path.as_str());
    // 화면은 "하는 중"이면 몇 초마다 다시 묻는다.
    let mut result = Value::Null;
    for _ in 0..200 {
        let r = f.send(get(&path, false)).await;
        assert_eq!(r.status(), StatusCode::OK);
        result = json(r).await;
        if screen(&result)["type"] != "site_login_busy" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(screen(&result)["type"], "site_login_done", "{result}");
    assert_eq!(screen(&result)["site"], "퀘이사존");
    assert!(
        screen(&result)["note"]
            .as_str()
            .unwrap()
            .contains("저장된 로그인"),
        "저장을 골랐다는 안내: {result}"
    );
    assert_eq!(*browser.passwords.lock().unwrap(), ["p@ss word"]);

    // 끝난 링크로 또 넣어도 다시 로그인하지 않고 결과만 보인다.
    let again = f.send(post(&path, false, "username=me&password=x")).await;
    assert_eq!(again.status(), StatusCode::SEE_OTHER);
    let v = json(f.send(get(&path, false)).await).await;
    assert_eq!(screen(&v)["type"], "site_login_done");
    assert_eq!(browser.passwords.lock().unwrap().len(), 1);
}

/// 기억 화면(결정 0007): 목록·항목 화면, 지우기(변경 기록에 남음), "기억해 줘"는 작업 큐로.
#[tokio::test]
async fn memory_pages_delete_and_tell() {
    let mut f = fixture().await;
    f.log_in();
    f.state
        .store()
        .write_memory(
            &lucia_store::MemoryWrite {
                slug: "devices",
                section: lucia_store::MemorySection::Topic,
                title: "기기",
                summary: "폴드 7",
                body: "- 갤럭시 Z 폴드 7",
            },
            None,
            lucia_store::MemoryAuthor::Agent,
            None,
            "처음",
            Utc::now(),
        )
        .unwrap();
    let v = page_json(&f, "/memory").await;
    assert_eq!(v["extra"]["view"], "list");
    let listed = v["snapshot"]["memories"].as_array().unwrap();
    assert!(
        listed
            .iter()
            .any(|m| m["slug"] == "devices" && m["title"] == "기기")
    );
    let v = page_json(&f, "/memory/devices").await;
    assert_eq!(v["extra"]["view"], "entry");
    assert_eq!(v["extra"]["entry"]["slug"], "devices");
    assert!(
        v["extra"]["entry"]["body"]
            .as_str()
            .unwrap()
            .contains("갤럭시 Z 폴드 7")
    );

    // 예전 개정으로 되돌리기(결정 0027): Lucia가 바꾼 것을 사용자 변경으로 되돌린다. 지금 개정에는 단추가 없다.
    let first = f.state.store().memory_history("devices", 1).unwrap()[0].id;
    f.state
        .store()
        .write_memory(
            &lucia_store::MemoryWrite {
                slug: "devices",
                section: lucia_store::MemorySection::Topic,
                title: "기기",
                summary: "폴드 8(잘못 앎)",
                body: "- 갤럭시 Z 폴드 8",
            },
            Some(first),
            lucia_store::MemoryAuthor::Agent,
            None,
            "잘못 고침",
            Utc::now(),
        )
        .unwrap();
    // 변경 기록은 새것부터이고, 항목의 지금 개정(`entry.revision`)이 그 맨 앞이다(화면은 지금 개정에는 되돌리기를 두지 않는다).
    let latest = f.state.store().memory_history("devices", 1).unwrap()[0].id;
    let v = page_json(&f, "/memory/devices").await;
    let ids: Vec<i64> = v["extra"]["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids.first(), Some(&latest));
    assert!(ids.contains(&first));
    assert_ne!(first, latest);
    assert_eq!(v["extra"]["entry"]["revision"], latest);
    let r = f
        .send(post(
            &format!("/memory/devices/rev/{first}/restore"),
            true,
            &format!("csrf={CSRF}"),
        ))
        .await;
    assert_eq!(location(&r), "/memory/devices");
    {
        let store = f.state.store();
        assert_eq!(store.memory("devices").unwrap().unwrap().summary, "폴드 7");
        let h = store.memory_history("devices", 1).unwrap();
        assert_eq!(h[0].author, lucia_store::MemoryAuthor::User);
        assert_eq!(store.recent_activity(1).unwrap()[0].kind, "memory_restore");
    }
    // 다른 항목의 개정 번호로는 되돌리지 않는다.
    let r = f
        .send(post(
            &format!("/memory/other/rev/{first}/restore"),
            true,
            &format!("csrf={CSRF}"),
        ))
        .await;
    assert_eq!(location(&r), "/memory/other");
    assert!(f.state.store().memory("other").unwrap().is_none());
    for bad in ["/memory/nope", "/memory/Bad%20Name"] {
        let r = f.send(get(bad, true)).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{bad}");
        let v = json(r).await;
        assert_eq!(v["kind"], "page", "{bad}");
        assert_eq!(v["extra"]["view"], "missing", "{bad}");
    }

    let r = f
        .send(post(
            "/memory/tell",
            true,
            &format!("csrf={CSRF}&text=%EB%82%B4+%EA%B0%95%EC%95%84%EC%A7%80%EB%8A%94+Beans"),
        ))
        .await;
    assert_eq!(location(&r), "/memory");
    let job = f.jobs.try_recv().unwrap();
    let crate::worker::Job::Routine(due) = job else {
        panic!("루틴 작업이어야 한다")
    };
    assert_eq!(due.action, crate::worker::actions::REMEMBER);
    assert_eq!(due.payload["text"], "내 강아지는 Beans");
    let r = f
        .send(post("/memory/tell", true, &format!("csrf={CSRF}&text=+")))
        .await;
    assert_eq!(location(&r), "/memory");
    assert!(f.jobs.try_recv().is_err(), "빈 요청은 넣지 않는다");

    let r = f
        .send(post(
            "/memory/devices/delete",
            true,
            &format!("csrf={CSRF}"),
        ))
        .await;
    assert_eq!(location(&r), "/memory");
    let store = f.state.store();
    assert!(store.memory("devices").unwrap().is_none());
    let history = store.memory_history("devices", 5).unwrap();
    assert_eq!(history[0].action, lucia_store::MemoryAction::Delete);
    assert_eq!(history[0].author, lucia_store::MemoryAuthor::User);
    assert_eq!(store.recent_activity(1).unwrap()[0].kind, "memory_delete");
}

/// 사이트 위임(결정 0028·0029): 관리 페이지에서 만들고(로그인 체크), 공용 접미사는 받지 않고, 거두고, 출구 검사 모드를 바꾼다.
#[tokio::test]
async fn site_grants_from_the_browser_page() {
    let f = fixture().await;
    f.log_in();
    let r = f
        .send(post(
            "/grants/add",
            true,
            &form(&[
                ("csrf", CSRF),
                ("host", "LMS.Example.ac.kr"),
                ("purpose", "과제"),
                ("login", "1"),
            ]),
        ))
        .await;
    assert_eq!(location(&r), "/browser");
    let grants = f.state.store().site_grants().unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].host, "lms.example.ac.kr");
    assert!(grants[0].login && !grants[0].actions);
    assert_eq!(grants[0].origin, "admin");
    f.send(post(
        "/grants/add",
        true,
        &form(&[("csrf", CSRF), ("host", "ac.kr")]),
    ))
    .await;
    assert_eq!(
        f.state.store().site_grants().unwrap().len(),
        1,
        "공용 접미사는 받지 않는다"
    );
    let v = page_json(&f, "/browser").await;
    let listed = v["snapshot"]["site_grants"].as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["host"], "lms.example.ac.kr");
    assert_eq!(
        listed[0]["login"], true,
        "저장된 로그인으로 묻지 않고 쓰는 위임"
    );
    assert_eq!(listed[0]["actions"], false);
    assert_eq!(listed[0]["origin"], "admin");
    assert!(listed[0]["revoked_at"].is_null());
    let gid = grants[0].id.clone();
    f.send(post(
        &format!("/grants/{gid}/revoke"),
        true,
        &format!("csrf={CSRF}"),
    ))
    .await;
    assert!(
        f.state
            .store()
            .site_grant(&gid)
            .unwrap()
            .unwrap()
            .revoked_at
            .is_some()
    );
    let v = page_json(&f, "/browser").await;
    assert!(!v["snapshot"]["site_grants"][0]["revoked_at"].is_null());
    f.send(post(
        "/egress/mode",
        true,
        &format!("csrf={CSRF}&mode=block"),
    ))
    .await;
    assert!(crate::leak::blocking(&f.state.store()));
    assert_eq!(
        page_json(&f, "/browser").await["snapshot"]["egress_block"],
        true
    );
    f.send(post(
        "/egress/mode",
        true,
        &format!("csrf={CSRF}&mode=weird"),
    ))
    .await;
    assert!(
        crate::leak::blocking(&f.state.store()),
        "모르는 값은 바꾸지 않는다"
    );
}

/// 목표(결정 0032): 할 일 화면에 보이고, 포기하면 하위 항목과 점검이 끝난다.
#[tokio::test]
async fn goals_show_on_the_work_page_and_can_be_abandoned() {
    let f = fixture().await;
    f.log_in();
    let gid = {
        let store = f.state.store();
        crate::goals::create(
            &store,
            &crate::goals::GoalCreate {
                title: "원서 접수 챙기기".into(),
                purpose: "기한 안에".into(),
                success: "접수 완료 메일".into(),
                deadline: None,
                interests: None,
                review_every_hours: None,
            },
            None,
            Utc::now(),
            chrono_tz::Asia::Seoul,
        );
        store.goals(false).unwrap()[0].id.clone()
    };
    let v = page_json(&f, "/work").await;
    let goals = v["snapshot"]["goals"].as_array().unwrap();
    assert_eq!(goals.len(), 1);
    assert_eq!(goals[0]["goal"]["id"], gid.as_str());
    assert_eq!(goals[0]["goal"]["title"], "원서 접수 챙기기");
    assert_eq!(goals[0]["goal"]["status"], "Active");
    f.send(post(
        &format!("/goals/{gid}/abandon"),
        true,
        &format!("csrf={CSRF}"),
    ))
    .await;
    let v = page_json(&f, "/work").await;
    assert_eq!(v["snapshot"]["goals"][0]["goal"]["status"], "Abandoned");
    let store = f.state.store();
    assert_eq!(
        store.goal(&gid).unwrap().unwrap().status,
        lucia_store::GoalStatus::Abandoned
    );
    assert!(
        !store
            .schedule(&crate::goals::schedule_id(&gid))
            .unwrap()
            .unwrap()
            .enabled
    );
}

/// 이벤트 확인·신호 처리 설정(결정 0031): 범위 밖은 저장하지 않고, 관심사는 줄로 다듬는다.
#[tokio::test]
async fn signal_settings_save_and_reject() {
    let f = fixture().await;
    f.log_in();
    let body = form(&[
        ("csrf", CSRF),
        ("enabled", "1"),
        ("weekly", "1"),
        ("poll_minutes", "3"),
        ("interests", " example.ac.kr \n\n# 주석\n과제"),
        ("classify_daily", "20"),
        ("wake_daily", "5"),
    ]);
    f.send(post("/signals/settings", true, &body)).await;
    {
        let store = f.state.store();
        assert_eq!(crate::events::poll_minutes(&store), 3);
        assert_eq!(
            crate::signals::interests(&store),
            vec!["example.ac.kr", "과제"]
        );
        assert!(crate::signals::enabled(&store));
        assert!(crate::reports::enabled(&store, crate::reports::WEEKLY_KEY));
        assert!(
            !crate::reports::enabled(&store, crate::reports::EVENING_KEY),
            "빠진 체크는 끔"
        );
    }
    let bad = form(&[
        ("csrf", CSRF),
        ("poll_minutes", "0"),
        ("classify_daily", "20"),
        ("wake_daily", "5"),
    ]);
    f.send(post("/signals/settings", true, &bad)).await;
    let store = f.state.store();
    assert_eq!(
        crate::events::poll_minutes(&store),
        3,
        "범위 밖은 저장하지 않는다"
    );
    assert!(
        crate::signals::enabled(&store),
        "실패한 저장은 아무것도 바꾸지 않는다"
    );
}

fn form(pairs: &[(&str, &str)]) -> String {
    let mut f = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        f.append_pair(k, v);
    }
    f.finish()
}

/// 캐릭터 화면(결정 0009): 파일을 보여 주고 고치고, 낡은 화면은 막고, 기록에서 되돌린다.
#[tokio::test]
async fn character_page_edits_the_file_with_history() {
    let f = fixture().await;
    f.log_in();
    let v = page_json(&f, "/character").await;
    assert_eq!(v["extra"]["view"], "edit");
    assert!(v["extra"]["content"].is_null());
    assert!(
        v["extra"]["error"]
            .as_str()
            .unwrap()
            .contains("character_file이 없다"),
        "경로를 모르면 읽지 못했다고 알린다"
    );
    assert!(
        v["extra"]["fingerprint"].is_null(),
        "읽지 못하면 지문도 없다"
    );

    let dir = f._dir.0.join("character");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("character.md");
    let original = "# 비서\n\n## 캐릭터보다 우선하는 것\n\n- <규칙>\n";
    std::fs::write(&path, original).unwrap();
    f.state.set_character_path(path.clone());

    let v = page_json(&f, "/character").await;
    assert!(v["extra"]["error"].is_null());
    assert_eq!(v["extra"]["content"], original, "내용은 글 그대로 실린다");
    assert_eq!(v["extra"]["path"], path.display().to_string());
    // 저장 폼이 `base`로 돌려보낼 지문(그새 파일이 바뀌었는지 본다)도 화면 데이터에 실린다.
    let base = crate::character::fingerprint(original);
    assert_eq!(v["extra"]["fingerprint"], base.as_str());

    let edited = "# 루시아\r\n\r\n## 캐릭터보다 우선하는 것\r\n\r\n- <규칙>\r\n- 반말을 쓴다\r\n";
    let r = f
        .send(post(
            "/character",
            true,
            &form(&[
                ("csrf", CSRF),
                ("content", edited),
                ("base", &base),
                ("note", "이름을 정했다"),
            ]),
        ))
        .await;
    assert_eq!(location(&r), "/character");
    let now = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        now,
        "# 루시아\n\n## 캐릭터보다 우선하는 것\n\n- <규칙>\n- 반말을 쓴다\n"
    );
    let v = page_json(&f, "/character").await;
    assert!(flash_text(&v).contains("저장했습니다"), "알림");
    assert_eq!(v["flash"]["tone"], "ok");
    let notes: Vec<&str> = v["extra"]["history"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|h| h["note"].as_str())
        .collect();
    assert!(notes.contains(&"이름을 정했다"), "기록: {notes:?}");

    // 낡은 화면(예전 지문)으로는 덮어쓰지 않는다.
    f.send(post(
        "/character",
        true,
        &form(&[("csrf", CSRF), ("content", "# 덮어쓰기\n"), ("base", &base)]),
    ))
    .await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), now);
    let v = page_json(&f, "/character").await;
    assert!(flash_text(&v).contains("바뀌었다"), "{v}");
    assert_eq!(v["flash"]["tone"], "bad");

    let revisions = f.state.store().character_revisions(10).unwrap();
    assert_eq!(revisions.len(), 2, "고치기 전과 후");
    let first = revisions.last().unwrap().id;
    // 예전 개정 화면: 그 내용과, 지금 파일과 같은 내용인지(같으면 되돌릴 것이 없다).
    let v = page_json(&f, &format!("/character/rev/{first}")).await;
    assert_eq!(v["extra"]["view"], "revision");
    assert_eq!(v["extra"]["revision"]["id"], first);
    assert_eq!(v["extra"]["revision"]["content"], original);
    assert_eq!(v["extra"]["is_current"], false);
    let newest = revisions[0].id;
    let v = page_json(&f, &format!("/character/rev/{newest}")).await;
    assert_eq!(
        v["extra"]["is_current"], true,
        "가장 새 개정은 지금 파일과 같다"
    );
    let r = f.send(get("/character/rev/999", true)).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    assert_eq!(json(r).await["extra"]["view"], "revision_missing");
    let r = f
        .send(post(
            &format!("/character/rev/{first}/restore"),
            true,
            &format!("csrf={CSRF}"),
        ))
        .await;
    assert_eq!(location(&r), "/character");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert_eq!(
        f.state.store().recent_activity(1).unwrap()[0].kind,
        "character_edit"
    );

    // 메시지 간격(결정 0026): 고른 값을 저장하고, 모르는 값은 바꾸지 않는다.
    let r = f
        .send(post(
            "/character/pace",
            true,
            &format!("csrf={CSRF}&pace=fast"),
        ))
        .await;
    assert_eq!(location(&r), "/character");
    let stored = || f.state.store().setting(crate::segments::PACE_KEY).unwrap();
    assert_eq!(stored().as_deref(), Some("fast"));
    f.send(post(
        "/character/pace",
        true,
        &format!("csrf={CSRF}&pace=warp"),
    ))
    .await;
    assert_eq!(stored().as_deref(), Some("fast"));
    let v = page_json(&f, "/character").await;
    assert_eq!(v["extra"]["pace"], "fast");
    assert!(
        v["extra"]["paces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["key"] == "fast")
    );
    f.send(post(
        "/character/pace",
        false,
        &format!("csrf={CSRF}&pace=off"),
    ))
    .await;
    assert_eq!(
        stored().as_deref(),
        Some("fast"),
        "로그인하지 않으면 바꾸지 않는다"
    );
}

/// 자동 실행 한도 저장(결정 0010): 금액·시간대 검사, 비우면 끔, 후속 조치 취소.
#[tokio::test]
async fn limits_and_followup_cancel_from_the_dashboard() {
    let f = fixture().await;
    f.log_in();
    let r = f
        .send(post(
            "/autonomy/limits",
            true,
            &format!("csrf={CSRF}&daily=2.5&weekly=&quiet=23%3A00-07%3A00"),
        ))
        .await;
    assert_eq!(location(&r), "/settings");
    let v = page_json(&f, "/settings").await;
    assert_eq!(v["snapshot"]["daily_cap"], 2.5);
    assert!(v["snapshot"]["weekly_cap"].is_null());
    assert_eq!(v["snapshot"]["quiet_hours"], "23:00-07:00");
    {
        let store = f.state.store();
        assert_eq!(
            crate::autonomy::cap(&store, crate::autonomy::keys::DAILY_USD),
            Some(2.5)
        );
        assert_eq!(
            crate::autonomy::cap(&store, crate::autonomy::keys::WEEKLY_USD),
            None
        );
        assert_eq!(
            crate::autonomy::fixed_quiet_hours(&store)
                .map(|q| q.display())
                .as_deref(),
            Some("23:00-07:00")
        );
    }
    for bad in ["daily=-1", "daily=abc", "quiet=25%3A00-07%3A00"] {
        f.send(post(
            "/autonomy/limits",
            true,
            &format!("csrf={CSRF}&{bad}"),
        ))
        .await;
        assert_eq!(
            crate::autonomy::cap(&f.state.store(), crate::autonomy::keys::DAILY_USD),
            Some(2.5),
            "{bad}: 잘못된 값이면 아무것도 바꾸지 않는다"
        );
    }
    f.send(post(
        "/autonomy/limits",
        true,
        &format!("csrf={CSRF}&daily=&quiet="),
    ))
    .await;
    assert_eq!(
        crate::autonomy::cap(&f.state.store(), crate::autonomy::keys::DAILY_USD),
        None
    );
    assert!(crate::autonomy::quiet_hours(&f.state.store()).is_none());

    let tz = chrono_tz::Asia::Seoul;
    let id = {
        let store = f.state.store();
        let req = crate::autonomy::FollowupRequest {
            when: (Utc::now() + chrono::Duration::days(1)).to_rfc3339(),
            task: "확인".into(),
            detail: String::new(),
            conversation: None,
            run_id: None,
            origin: "chat".into(),
        };
        let due = crate::autonomy::check_followup(&store, &req, tz, Utc::now()).unwrap();
        crate::autonomy::create_followup(&store, &req, due, tz, Utc::now()).unwrap()
    };
    let r = f
        .send(post(
            &format!("/followups/{id}/cancel"),
            true,
            &format!("csrf={CSRF}"),
        ))
        .await;
    assert_eq!(location(&r), "/");
    assert!(f.state.store().open_followups().unwrap().is_empty());
    let sched = f.state.store().schedule(&id).unwrap().unwrap();
    assert!(
        !sched.enabled && sched.next_due_at.is_none(),
        "취소하면 발사도 멈춘다"
    );

    // 결정 0013: 브리핑이 등록한 후속 조치 하나에 사용자가 브라우저를 허용한다.
    let id = {
        let store = f.state.store();
        let req = crate::autonomy::FollowupRequest {
            when: (Utc::now() + chrono::Duration::days(1)).to_rfc3339(),
            task: "과제 제출 여부 확인".into(),
            detail: String::new(),
            conversation: None,
            run_id: Some("briefing-1".into()),
            origin: "background".into(),
        };
        let due = crate::autonomy::check_followup(&store, &req, tz, Utc::now()).unwrap();
        crate::autonomy::create_followup(&store, &req, due, tz, Utc::now()).unwrap()
    };
    // 화면은 출처로 "읽기만"인지 "브라우저 허용함"인지 가른다(`followup_uses_browser`의 규칙).
    let v = page_json(&f, "/work").await;
    let open = v["snapshot"]["followups_open"].as_array().unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["id"], id.as_str());
    assert_eq!(open[0]["task"], "과제 제출 여부 확인");
    let origin = open[0]["origin"].as_str().unwrap();
    assert!(
        !crate::autonomy::followup_uses_browser(origin),
        "브리핑에서 시작한 후속 조치는 읽기만 한다: {origin}"
    );
    let r = f
        .send(post(
            &format!("/followups/{id}/allow-browser"),
            true,
            &format!("csrf={CSRF}"),
        ))
        .await;
    assert_eq!(location(&r), "/");
    let origin = f.state.store().followup(&id).unwrap().unwrap().origin;
    assert!(crate::autonomy::followup_uses_browser(&origin), "{origin}");
    let v = page_json(&f, "/work").await;
    assert_eq!(
        v["snapshot"]["followups_open"][0]["origin"],
        origin.as_str(),
        "화면에도 바뀐 출처가 실린다"
    );
}

/// 접속 API 토큰(결정 0021): 발급하면 한 번만 보이고 해시만 남는다. 취소하면 듣지 않는다.
#[tokio::test]
async fn app_tokens_are_issued_once_and_revoked() {
    let f = fixture().await;
    f.log_in();
    let r = f
        .send(post(
            "/clients/new",
            true,
            &format!("csrf={CSRF}&name=%ED%9C%B4%EB%8C%80%ED%8F%B0"),
        ))
        .await;
    assert_eq!(location(&r), "/connections");
    let v = page_json(&f, "/connections").await;
    let notice = flash_text(&v);
    let token = notice
        .split("lucia_")
        .nth(1)
        .map(|t| format!("lucia_{}", &t[..64]))
        .expect("발급한 토큰이 안내에 보인다");
    let listed = v["snapshot"]["clients"].as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0][0]["name"], "휴대폰");
    assert_eq!(listed[0][0]["role"], "app");
    assert!(listed[0][0]["revoked_at"].is_null());
    let client = {
        let store = f.state.store();
        let c = store
            .client_by_token_hash(&crate::gateway::token_hash(&token))
            .unwrap()
            .expect("해시로 찾는다");
        assert_eq!(c.role, "app");
        c
    };
    // 다시 열면 토큰은 보이지 않는다(안내는 한 번).
    let again = f.send(get("/connections", true)).await;
    let again = body(again).await;
    assert!(
        !again.contains(&token) && !again.contains(&crate::gateway::token_hash(&token)),
        "토큰도 그 해시도 화면 데이터에 없다"
    );
    let again: Value = serde_json::from_str(&again).unwrap();
    assert!(again["flash"].is_null());
    assert_eq!(again["snapshot"]["clients"][0][0]["id"], client.id.as_str());
    // 이름 검사.
    f.send(post("/clients/new", true, &format!("csrf={CSRF}&name=")))
        .await;
    assert_eq!(f.state.store().clients().unwrap().len(), 1);

    f.send(post(
        &format!("/clients/{}/revoke", client.id),
        true,
        &format!("csrf={CSRF}"),
    ))
    .await;
    assert!(
        f.state
            .store()
            .client_by_token_hash(&crate::gateway::token_hash(&token))
            .unwrap()
            .is_none()
    );
    let v = page_json(&f, "/connections").await;
    assert!(
        !v["snapshot"]["clients"][0][0]["revoked_at"].is_null(),
        "취소한 클라이언트는 취소한 때가 실린다"
    );
}

/// 앱 토큰의 범위(결정 0063): 발급할 때와 나중에 범위 화면에서 `scope_browser_handover` 상자로 켜고 끈다. 상자를 비우면
/// 그 범위만 빠진다. 화면 데이터에는 범위 이름과 클라이언트마다의 켜짐 표시가 실린다.
#[tokio::test]
async fn browser_handover_scope_is_set_at_issue_and_changed_later() {
    let f = fixture().await;
    f.log_in();
    let scopes_of = |name: &str| -> Vec<String> {
        f.state
            .store()
            .clients()
            .unwrap()
            .into_iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("{name} 클라이언트가 없다"))
            .scopes
    };
    let mark_of = |v: &Value, name: &str| -> (bool, Vec<String>) {
        let c = v["snapshot"]["clients"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pair| &pair[0])
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("{name}이 화면 데이터에 없다"));
        (
            c["ui"]["has_browser_handover"].as_bool().unwrap(),
            c["ui"]["scope_labels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap().to_owned())
                .collect(),
        )
    };

    // 발급할 때: 상자를 켜면 그 범위만, 켜지 않으면 범위 없음.
    let r = f
        .send(post(
            "/clients/new",
            true,
            &format!("csrf={CSRF}&name=handover&scope_browser_handover=1"),
        ))
        .await;
    assert_eq!(location(&r), "/connections");
    f.send(post(
        "/clients/new",
        true,
        &format!("csrf={CSRF}&name=plain"),
    ))
    .await;
    f.send(post(
        "/clients/new",
        true,
        &format!(
            "csrf={CSRF}&name=all&scope_security_mode=1&scope_app_tools=1&scope_browser_handover=1"
        ),
    ))
    .await;
    assert_eq!(
        scopes_of("handover"),
        [crate::gateway::scopes::BROWSER_HANDOVER]
    );
    assert!(scopes_of("plain").is_empty());
    assert_eq!(
        scopes_of("all"),
        [
            crate::gateway::scopes::SECURITY_MODE,
            crate::gateway::scopes::APP_TOOLS,
            crate::gateway::scopes::BROWSER_HANDOVER,
        ]
    );

    // 화면 데이터: 범위 표와 켜짐 표시, 사람이 읽는 이름.
    let v = page_json(&f, "/connections").await;
    assert_eq!(
        v["consts"]["gateway"]["scope_browser_handover"],
        "browser_handover"
    );
    assert!(
        v["consts"]["gateway"]["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["key"] == "browser_handover" && s["label"] == "브라우저 화면 넘겨받기")
    );
    let (on, labels) = mark_of(&v, "handover");
    assert!(on);
    assert_eq!(labels, ["브라우저 화면 넘겨받기"]);
    assert!(!mark_of(&v, "plain").0);
    assert!(mark_of(&v, "all").0);

    // 나중에 범위 화면에서: 켜고, 다른 범위와 함께, 비우면 빠진다.
    let id_of = |name: &str| {
        f.state
            .store()
            .clients()
            .unwrap()
            .into_iter()
            .find(|c| c.name == name)
            .unwrap()
            .id
    };
    let plain = id_of("plain");
    let r = f
        .send(post(
            &format!("/clients/{plain}/scopes"),
            true,
            &format!("csrf={CSRF}&scope_browser_handover=1"),
        ))
        .await;
    assert_eq!(location(&r), "/connections");
    assert_eq!(
        scopes_of("plain"),
        [crate::gateway::scopes::BROWSER_HANDOVER]
    );
    let all = id_of("all");
    f.send(post(
        &format!("/clients/{all}/scopes"),
        true,
        &format!("csrf={CSRF}&scope_app_tools=1"),
    ))
    .await;
    assert_eq!(
        scopes_of("all"),
        [crate::gateway::scopes::APP_TOOLS],
        "상자를 비우면 그 범위만 빠진다"
    );
    let v = page_json(&f, "/connections").await;
    assert!(mark_of(&v, "plain").0);
    assert!(!mark_of(&v, "all").0);
    // 켠 범위가 토큰의 범위로 쓰인다(게이트웨이가 그 클라이언트를 이 범위로 본다).
    assert!(
        f.state
            .store()
            .active_client(&plain)
            .unwrap()
            .unwrap()
            .scopes
            .iter()
            .any(|s| s == crate::gateway::scopes::BROWSER_HANDOVER)
    );
    // 취소한 클라이언트의 범위는 바꾸지 못한다.
    f.send(post(
        &format!("/clients/{plain}/revoke"),
        true,
        &format!("csrf={CSRF}"),
    ))
    .await;
    f.send(post(
        &format!("/clients/{plain}/scopes"),
        true,
        &format!("csrf={CSRF}"),
    ))
    .await;
    assert_eq!(
        scopes_of("plain"),
        [crate::gateway::scopes::BROWSER_HANDOVER],
        "취소한 것은 그대로"
    );
}

/// 앱 토큰의 "Discord DM의 파일 받기" 범위(결정 0066·0067): 발급할 때와 나중에 범위 화면에서 `scope_owner_files` 상자로 켜고 끈다.
/// 다른 앱 대화의 파일은 범위 없이 받으므로 이 범위는 Discord DM의 파일에만 필요하다(결정 0067).
/// 다른 범위와 섞여도 서로 간섭하지 않고, 화면 데이터에 이름표·상수·켜짐 표시가 실린다.
#[tokio::test]
async fn owner_files_scope_is_set_at_issue_and_changed_later() {
    let f = fixture().await;
    f.log_in();
    let owner_files = crate::gateway::scopes::OWNER_FILES;
    let client_of = |name: &str| {
        f.state
            .store()
            .clients()
            .unwrap()
            .into_iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("{name} 클라이언트가 없다"))
    };
    let mark_of = |v: &Value, name: &str| -> (bool, Vec<String>) {
        let c = v["snapshot"]["clients"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pair| &pair[0])
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("{name}이 화면 데이터에 없다"));
        (
            c["ui"]["has_owner_files"].as_bool().unwrap(),
            c["ui"]["scope_labels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap().to_owned())
                .collect(),
        )
    };

    // 발급할 때: 켜면 그 범위만, 켜지 않으면 범위 없음, 다른 범위와 함께면 발급 폼의 차례대로.
    let r = f
        .send(post(
            "/clients/new",
            true,
            &format!("csrf={CSRF}&name=files&scope_owner_files=1"),
        ))
        .await;
    assert_eq!(location(&r), "/connections");
    f.send(post(
        "/clients/new",
        true,
        &format!("csrf={CSRF}&name=plain"),
    ))
    .await;
    f.send(post(
        "/clients/new",
        true,
        &format!("csrf={CSRF}&name=both&scope_app_tools=1&scope_owner_files=1"),
    ))
    .await;
    assert_eq!(client_of("files").scopes, [owner_files]);
    assert!(client_of("plain").scopes.is_empty());
    assert_eq!(
        client_of("both").scopes,
        [crate::gateway::scopes::APP_TOOLS, owner_files]
    );

    // 화면 데이터: 상수, 범위 표, 켜짐 표시, 사람이 읽는 이름.
    let v = page_json(&f, "/connections").await;
    assert_eq!(v["consts"]["gateway"]["scope_owner_files"], "owner_files");
    assert!(
        v["consts"]["gateway"]["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["key"] == "owner_files" && s["label"] == "Discord DM의 파일 받기")
    );
    let (on, labels) = mark_of(&v, "files");
    assert!(on);
    assert_eq!(labels, ["Discord DM의 파일 받기"]);
    assert!(!mark_of(&v, "plain").0);
    assert!(mark_of(&v, "both").0);

    // 나중에 범위 화면에서: 켜고, 비우면 그 범위만 빠진다.
    let plain = client_of("plain").id;
    let r = f
        .send(post(
            &format!("/clients/{plain}/scopes"),
            true,
            &format!("csrf={CSRF}&scope_owner_files=1"),
        ))
        .await;
    assert_eq!(location(&r), "/connections");
    assert_eq!(client_of("plain").scopes, [owner_files]);
    let both = client_of("both").id;
    f.send(post(
        &format!("/clients/{both}/scopes"),
        true,
        &format!("csrf={CSRF}&scope_app_tools=1"),
    ))
    .await;
    assert_eq!(
        client_of("both").scopes,
        [crate::gateway::scopes::APP_TOOLS],
        "상자를 비우면 그 범위만 빠진다"
    );
    let v = page_json(&f, "/connections").await;
    assert!(mark_of(&v, "plain").0);
    assert!(!mark_of(&v, "both").0);
}

/// 알림 받을 곳은 "앱 모두"(`*`)와 Discord DM 둘뿐이다(결정 0066·0067): 앱으로 받으면 설정에는 늘 `*`가 저장되고,
/// 취소하지 않은 앱마다 "알림 받는 곳" 표시(`ui.is_notify_target`)가 켜진다. 취소한 앱·기기는 켜지지 않는다.
/// 예전 화면이 보내는 앱 하나의 ID도 `*`로 저장하고(없거나 취소한 앱은 거절), 예전에 저장된 앱 하나의 ID도 `*`로 읽는다.
/// 비우면 Discord DM(설정이 지워진다). 취소하지 않은 앱이 없으면 앱으로 둘 수 없다. 초기 설정의 요약은 앱 모두를 적는다.
#[tokio::test]
async fn notify_target_is_all_apps_or_discord_dm() {
    let f = fixture_with("\n[gateway]\nbind = [\"127.0.0.1:18790\"]\n").await;
    f.log_in();
    for name in ["first", "second", "gone"] {
        f.send(post(
            "/clients/new",
            true,
            &format!("csrf={CSRF}&name={name}"),
        ))
        .await;
    }
    let id_of = |name: &str| {
        f.state
            .store()
            .clients()
            .unwrap()
            .into_iter()
            .find(|c| c.name == name)
            .unwrap()
            .id
    };
    let (first, second, gone) = (id_of("first"), id_of("second"), id_of("gone"));
    f.state
        .store()
        .create_client(
            "cl-device",
            "sensor",
            crate::gateway::Role::Device.as_str(),
            "hash",
            Utc::now(),
        )
        .unwrap();
    f.send(post(
        &format!("/clients/{gone}/revoke"),
        true,
        &format!("csrf={CSRF}"),
    ))
    .await;
    let setting = || {
        f.state
            .store()
            .setting(crate::gateway::keys::NOTIFY_CLIENT)
            .unwrap()
    };
    // 이름별 "알림 받는 곳" 표시.
    let targets = |v: &Value| -> Vec<(String, bool)> {
        v["snapshot"]["clients"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pair| {
                let c = &pair[0];
                (
                    c["name"].as_str().unwrap().to_owned(),
                    c["ui"]["is_notify_target"].as_bool().unwrap(),
                )
            })
            .collect()
    };
    // 목록의 순서에 기대지 않게 이름순으로 돌려준다.
    let marked = |v: &Value| -> Vec<String> {
        let mut names: Vec<String> = targets(v)
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(n, _)| n)
            .collect();
        names.sort();
        names
    };

    // 처음: 정한 곳이 없다. 상수는 화면 데이터에 있다.
    let v = page_json(&f, "/connections").await;
    assert_eq!(v["consts"]["gateway"]["all_apps"], "*");
    assert!(v["snapshot"]["app"]["notify_client"].is_null());
    assert!(marked(&v).is_empty());

    // 앱 모두: 설정은 `*`, 안내, 활동 기록, 취소하지 않은 앱 둘에만 표시.
    let r = f
        .send(post("/app/notify", true, &format!("csrf={CSRF}&client=*")))
        .await;
    assert_eq!(location(&r), "/connections");
    assert_eq!(setting().as_deref(), Some("*"));
    assert_eq!(
        crate::gateway::notify_client(&f.state.store()).as_deref(),
        Some("*")
    );
    assert!(
        f.state
            .store()
            .recent_activity(5)
            .unwrap()
            .iter()
            .any(|a| a.kind == "client_admin" && a.summary == "알림 받을 곳: 앱 모두")
    );
    let v = page_json(&f, "/connections").await;
    assert_eq!(
        flash_text(&v),
        "알림·브리핑·승인 카드를 앱에 둡니다. 모든 앱이 같은 대화를 받고, 사람이 있는 기기에서 울리며, 보지 않으면 휴대폰(푸시)으로 넘깁니다."
    );
    assert_eq!(v["flash"]["tone"], "ok");
    assert_eq!(v["snapshot"]["app"]["notify_client"], "*");
    assert_eq!(marked(&v), ["first", "second"]);
    let all = targets(&v);
    assert!(
        all.contains(&("gone".to_owned(), false)),
        "취소한 앱: {all:?}"
    );
    assert!(all.contains(&("sensor".to_owned(), false)), "기기: {all:?}");
    // 초기 설정의 앱 단계는 `*`라는 이름의 클라이언트를 찾지 않고 앱 모두라고 적는다.
    let v = page_json(&f, "/start").await;
    let app = v["snapshot"]["setup"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "app")
        .unwrap();
    assert!(
        app["summary"]
            .as_str()
            .unwrap()
            .contains("알림 받는 곳 앱 모두"),
        "{app}"
    );

    // 예전 화면이 앱 하나의 ID를 보내도 앱 모두로 저장한다(앱 하나만 받는 것은 없다). 표시도 앱 모두와 같다.
    f.send(post(
        "/app/notify",
        true,
        &format!("csrf={CSRF}&client={second}"),
    ))
    .await;
    assert_eq!(setting().as_deref(), Some("*"));
    let v = page_json(&f, "/connections").await;
    assert_eq!(v["snapshot"]["app"]["notify_client"], "*");
    assert_eq!(marked(&v), ["first", "second"]);
    assert_eq!(v["flash"]["tone"], "ok");

    // 예전에 저장된 앱 하나의 ID(이 화면이 만든 것이 아니다)도 앱 모두로 읽는다.
    f.state
        .store()
        .set_setting(crate::gateway::keys::NOTIFY_CLIENT, &second, Utc::now())
        .unwrap();
    assert_eq!(
        crate::gateway::notify_client(&f.state.store()).as_deref(),
        Some("*")
    );
    let v = page_json(&f, "/connections").await;
    assert_eq!(v["snapshot"]["app"]["notify_client"], "*");
    assert_eq!(marked(&v), ["first", "second"]);

    // 앱 모두로 다시, 이어서 비우면 Discord DM(설정이 지워진다).
    f.send(post("/app/notify", true, &format!("csrf={CSRF}&client=*")))
        .await;
    assert_eq!(setting().as_deref(), Some("*"));
    f.send(post("/app/notify", true, &format!("csrf={CSRF}&client=")))
        .await;
    assert_eq!(setting(), None);
    assert_eq!(
        flash_text(&page_json(&f, "/connections").await),
        "알림을 Discord DM으로 받습니다."
    );
    let v = page_json(&f, "/connections").await;
    assert!(v["snapshot"]["app"]["notify_client"].is_null());
    assert!(marked(&v).is_empty());

    // 취소한 앱의 ID와 없는 ID, 앱이 아닌 기기의 ID는 거절하고 설정을 건드리지 않는다.
    for bad in [gone.as_str(), "no-such-client", "cl-device"] {
        f.send(post(
            "/app/notify",
            true,
            &format!("csrf={CSRF}&client={bad}"),
        ))
        .await;
        assert_eq!(setting(), None, "{bad}");
        let v = page_json(&f, "/connections").await;
        assert_eq!(flash_text(&v), "없거나 취소한 앱입니다.", "{bad}");
        assert_eq!(v["flash"]["tone"], "bad", "{bad}");
    }

    // 취소하지 않은 앱이 없으면 앱 모두로 둘 수 없다. 앱 모두로 둔 뒤 앱을 다 취소하면 표시는 사라지고 DM으로 간다.
    f.send(post("/app/notify", true, &format!("csrf={CSRF}&client=*")))
        .await;
    for id in [&first, &second] {
        f.send(post(
            &format!("/clients/{id}/revoke"),
            true,
            &format!("csrf={CSRF}"),
        ))
        .await;
    }
    let v = page_json(&f, "/connections").await;
    assert!(v["snapshot"]["app"]["notify_client"].is_null());
    assert!(marked(&v).is_empty());
    f.send(post("/app/notify", true, &format!("csrf={CSRF}&client=")))
        .await;
    f.send(post("/app/notify", true, &format!("csrf={CSRF}&client=*")))
        .await;
    assert_eq!(setting(), None, "앱이 없으면 저장하지 않는다");
    assert_eq!(
        flash_text(&page_json(&f, "/connections").await),
        "취소하지 않은 앱이 없습니다. 앱 토큰을 먼저 발급하세요."
    );
}

/// 반복 루틴(결정 0025): 시스템 화면에 보이고(코드는 글 그대로 실린다), 멈추기·다시 켜기·지금 실행·지우기가 된다.
#[tokio::test]
async fn routines_are_listed_and_managed_from_the_automation_page() {
    let mut f = fixture().await;
    f.log_in();
    let trigger = lucia_store::Trigger::Cron("0 8 * * 1-5".into());
    {
        let store = f.state.store();
        store
            .create_routine(
                "routine-r1",
                &lucia_store::NewRoutine {
                    name: "평일 아침 일정",
                    trigger: &trigger,
                    task: "오늘 일정을 알린다",
                    script: Some("print('<b>')"),
                    tools: &["calendar_events".to_owned()],
                    input: &serde_json::json!({}),
                    status: lucia_store::RoutineStatus::Active,
                    conversation: None,
                    until: None,
                },
                Utc::now(),
            )
            .unwrap()
            .unwrap();
        let r = store.routine("routine-r1").unwrap().unwrap();
        crate::routines::arm(&store, &r, Utc::now(), chrono_tz::Asia::Seoul).unwrap();
    }
    let v = page_json(&f, "/automation").await;
    let listed = v["snapshot"]["routines"].as_array().unwrap();
    assert_eq!(listed.len(), 1);
    let r = &listed[0]["routine"];
    assert_eq!(r["id"], "routine-r1");
    assert_eq!(r["name"], "평일 아침 일정");
    assert_eq!(r["trigger"]["Cron"], "0 8 * * 1-5");
    assert_eq!(r["script"], "print('<b>')", "코드는 글 그대로 실린다");
    assert_eq!(r["status"], "Active");
    assert!(!listed[0]["next"].is_null(), "다음 실행이 걸려 있다");

    let act = |verb: &str| {
        post(
            &format!("/routines/routine-r1/{verb}"),
            true,
            &format!("csrf={CSRF}"),
        )
    };
    f.send(act("pause")).await;
    assert_eq!(
        page_json(&f, "/automation").await["snapshot"]["routines"][0]["routine"]["status"],
        "Paused"
    );
    {
        let store = f.state.store();
        assert_eq!(
            store.routine("routine-r1").unwrap().unwrap().status,
            lucia_store::RoutineStatus::Paused
        );
        assert!(!store.schedule("routine-r1").unwrap().unwrap().enabled);
    }
    f.send(act("resume")).await;
    assert!(
        f.state
            .store()
            .schedule("routine-r1")
            .unwrap()
            .unwrap()
            .enabled
    );
    let r = f.send(act("run")).await;
    assert_eq!(location(&r), "/automation");
    match f.jobs.try_recv().unwrap() {
        crate::worker::Job::Routine(due) => {
            assert_eq!(due.action, crate::routines::ACTION);
            assert_eq!(due.payload["id"], "routine-r1");
        }
        _ => panic!("루틴 작업이어야 한다"),
    }
    f.send(act("remove")).await;
    assert!(f.state.store().routine("routine-r1").unwrap().is_none());

    // 하루 깨우기 상한은 "자동 실행 한도"에서 정한다(잘못된 값은 아무것도 바꾸지 않는다).
    assert_eq!(
        crate::routines::daily_wakes_max(&f.state.store()),
        crate::routines::DAILY_WAKES_MAX
    );
    f.send(post(
        "/autonomy/limits",
        true,
        &format!("csrf={CSRF}&routine_wakes=12"),
    ))
    .await;
    assert_eq!(crate::routines::daily_wakes_max(&f.state.store()), 12);
    f.send(post(
        "/autonomy/limits",
        true,
        &format!("csrf={CSRF}&routine_wakes=500"),
    ))
    .await;
    assert_eq!(crate::routines::daily_wakes_max(&f.state.store()), 12);
}

/// 권한 모드(결정 0040): 관리 페이지에서 바꾸면 활동 기록이 남고, 모르는 값은 거절하며, 개요에 지금 모드가 보인다.
#[tokio::test]
async fn security_mode_is_switched_from_the_dashboard() {
    let f = fixture().await;
    f.log_in();
    let v = page_json(&f, "/").await;
    assert_eq!(
        v["snapshot"]["security_mode"], "Standard",
        "개요에 지금 모드가 실린다"
    );
    let r = f
        .send(post(
            "/security/mode",
            true,
            &format!("csrf={CSRF}&mode=nonsense"),
        ))
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        crate::security::mode(&f.state.store()),
        crate::security::Mode::Standard
    );
    f.send(post(
        "/security/mode",
        true,
        &format!("csrf={CSRF}&mode=strict"),
    ))
    .await;
    assert_eq!(
        crate::security::mode(&f.state.store()),
        crate::security::Mode::Strict
    );
    let v = page_json(&f, "/").await;
    assert_eq!(v["snapshot"]["security_mode"], "Strict");
    assert!(
        f.state
            .store()
            .recent_activity(5)
            .unwrap()
            .iter()
            .any(|a| a.kind == "security_mode" && a.summary.contains("관리 페이지"))
    );
}

// ---------- 외부 노출과 2단계 인증(결정 0043) ----------

fn outside(method: &str, path: &str, cookie: Option<&str>, form: &str) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("host", PUBLIC_HOST)
        .header("content-type", "application/x-www-form-urlencoded");
    // GET은 화면이 데이터를 묻는 요청이다(JSON). POST는 처리기의 답(리다이렉트)을 그대로 본다.
    if method == "GET" {
        b = b.header("accept", JSON);
    }
    if let Some(c) = cookie {
        b = b.header("cookie", format!("{COOKIE}={c}"));
    }
    b.body(Body::from(form.to_owned())).unwrap()
}

async fn body_text(r: Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(r.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn outside_requests_need_the_public_host_and_a_second_factor() {
    // 공개 주소가 없으면 프록시를 거친 요청은 받지 않는다(프록시도 믿지 않으면 그냥 집 안 주소다).
    let f = fixture().await;
    let r = f
        .send_outside(outside("GET", "/login", None, ""), "8.8.8.8")
        .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    let f = fixture_with(PUBLIC).await;
    let hash = hash_password("correct horse battery").unwrap();
    f.state
        .store()
        .set_admin_password_hash(&hash, Utc::now())
        .unwrap();
    // 프록시가 상대 주소를 주지 않았거나, 밖에서 LAN 주소 Host로 오면 거절.
    let mut req = outside("GET", "/login", None, "");
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([172, 18, 0, 11], 50000))));
    assert_eq!(
        router(f.state.clone()).oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let mut lan_host = get("/login", false);
    lan_host
        .headers_mut()
        .insert("x-forwarded-for", "8.8.8.8".parse().unwrap());
    let mut lan_host = lan_host;
    lan_host
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([172, 18, 0, 11], 50000))));
    assert_eq!(
        router(f.state.clone())
            .oneshot(lan_host)
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    // 밖의 로그인 화면: 인증 코드 칸, HSTS. 2단계 인증이 없으면 로그인할 수 없다고 알린다.
    let r = f
        .send_outside(outside("GET", "/login", None, ""), "8.8.8.8")
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers().contains_key("strict-transport-security"));
    // 밖에서는 로그인 화면에 인증 코드 칸이 뜨고(`external`), 2단계 인증이 없으면 그렇게 알린다.
    let v: Value = serde_json::from_str(&body_text(r).await).unwrap();
    assert_eq!(screen(&v)["type"], "login");
    assert_eq!(screen(&v)["external"], true);
    assert!(
        screen(&v)["error"]
            .as_str()
            .unwrap()
            .contains("2단계 인증이 필요합니다")
    );
    let r = f
        .send_outside(
            outside("POST", "/login", None, "password=correct+horse+battery"),
            "8.8.8.8",
        )
        .await;
    assert_eq!(
        r.status(),
        StatusCode::FORBIDDEN,
        "비밀번호가 맞아도 2단계 인증 없이는 안 된다"
    );
    // 집 안은 그대로(코드 칸 없음, HSTS 없음).
    let r = f.send(get("/login", false)).await;
    assert!(!r.headers().contains_key("strict-transport-security"));
    let v: Value = serde_json::from_str(&body_text(r).await).unwrap();
    assert_eq!(screen(&v)["type"], "login");
    assert_eq!(screen(&v)["external"], false);
    assert!(screen(&v)["error"].is_null());
    // 초기 설정 링크는 밖에서 열리지 않는다.
    let r = f
        .send_outside(outside("GET", "/setup?token=x", None, ""), "8.8.8.8")
        .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    // 집 안 세션 쿠키를 밖에서 쓰지 못한다.
    f.log_in();
    let r = f
        .send_outside(outside("GET", "/", Some(SID), ""), "8.8.8.8")
        .await;
    // 화면이 물은 것이라 다른 로그아웃된 경우와 같은 꼴(401 + 로그인으로)로 답한다.
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert!(
        r.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    // 밖에서는 2단계 인증을 켜지 못한다.
    let r = f
        .send_outside(
            outside("POST", "/totp/start", Some(SID), &format!("csrf={CSRF}")),
            "8.8.8.8",
        )
        .await;
    assert_eq!(location(&r), "/login");
    // 밖의 실패가 한도를 넘으면 외부 로그인을 막는다(집 안은 그대로).
    for _ in 0..EXTERNAL_FAILURES_PER_HOUR {
        f.state.note_external_failure();
    }
    let r = f
        .send_outside(
            outside("POST", "/login", None, "password=x&code=123456"),
            "9.9.9.9",
        )
        .await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn second_factor_is_set_up_at_home_and_used_outside() {
    if !crate::credentials::user_credentials_work() {
        eprintln!("systemd-creds --user를 쓸 수 없음(systemd 256 미만이거나 없음), 건너뜀");
        return;
    }
    let f = fixture_with(PUBLIC).await;
    let hash = hash_password("correct horse battery").unwrap();
    f.state
        .store()
        .set_admin_password_hash(&hash, Utc::now())
        .unwrap();
    f.log_in();
    let r = f
        .send(post(
            "/totp/start",
            true,
            &format!("csrf={CSRF}&back=/settings"),
        ))
        .await;
    assert_eq!(location(&r), "/settings");
    let secret = f
        .state
        .totp_pending
        .lock()
        .unwrap()
        .get(SID)
        .unwrap()
        .0
        .clone();
    let v = page_json(&f, "/settings").await;
    let pending = &v["snapshot"]["access"]["totp_pending"];
    assert_eq!(pending[0], secret.as_str(), "설정 중에만 키가 실린다");
    assert!(pending[1].as_str().unwrap().starts_with("otpauth://totp/"));
    assert_eq!(v["snapshot"]["access"]["totp_enabled"], false);
    let raw = crate::totp::base32_decode(&secret).unwrap();
    let now = Utc::now().timestamp();
    let code = |step: i64| format!("{:06}", crate::totp::code_at(&raw, step));
    // 틀린 코드는 켜지 않는다.
    let wrong = format!(
        "{:06}",
        (crate::totp::code_at(&raw, crate::totp::step_of(now)) + 1) % 1_000_000
    );
    f.send(post(
        "/totp/confirm",
        true,
        &format!("csrf={CSRF}&code={wrong}"),
    ))
    .await;
    assert!(!f.state.creds.exists(crate::totp::CREDENTIAL));
    f.send(post(
        "/totp/confirm",
        true,
        &format!("csrf={CSRF}&code={}", code(crate::totp::step_of(now))),
    ))
    .await;
    assert!(f.state.creds.exists(crate::totp::CREDENTIAL), "켜졌다");
    let page = body_text(f.send(get("/settings", true)).await).await;
    assert!(!page.contains(&secret), "다시 보이지 않는다");
    let v: Value = serde_json::from_str(&page).unwrap();
    assert!(v["snapshot"]["access"]["totp_pending"].is_null());
    assert_eq!(v["snapshot"]["access"]["totp_enabled"], true);
    // 밖에서 로그인: 비밀번호만으로는 안 되고, 쓴 코드는 다시 쓰지 못한다.
    let r = f
        .send_outside(
            outside("POST", "/login", None, "password=correct+horse+battery"),
            "8.8.8.8",
        )
        .await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let used = code(crate::totp::step_of(now));
    let r = f
        .send_outside(
            outside(
                "POST",
                "/login",
                None,
                &format!("password=correct+horse+battery&code={used}"),
            ),
            "8.8.8.9",
        )
        .await;
    assert_eq!(
        r.status(),
        StatusCode::UNAUTHORIZED,
        "같은 칸의 코드는 한 번만"
    );
    let next = code(crate::totp::step_of(now) + 1);
    let r = f
        .send_outside(
            outside(
                "POST",
                "/login",
                None,
                &format!("password=correct+horse+battery&code={next}"),
            ),
            "8.8.8.10",
        )
        .await;
    assert_eq!(location(&r), "/start", "새 설치의 첫 화면(결정 0064)");
    let cookie = r.headers()["set-cookie"].to_str().unwrap().to_owned();
    assert!(cookie.contains("; Secure"), "{cookie}");
    let sid = cookie
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_owned();
    assert!(f.state.sessions.lock().unwrap()[&sid].external);
    assert!(
        f.state
            .store()
            .recent_activity(5)
            .unwrap()
            .iter()
            .any(|a| a.kind == "admin_login"),
        "밖의 로그인은 기록에 남는다"
    );
    // 밖에서 로그인한 세션은 집 안에서 쓰지 못하고, 2단계 인증을 끄지도 못한다.
    let mut home = get("/", false);
    home.headers_mut()
        .insert("cookie", format!("{COOKIE}={sid}").parse().unwrap());
    let r = f.send(home).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert!(
        r.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let csrf = f.state.sessions.lock().unwrap()[&sid].csrf.clone();
    f.send_outside(
        outside(
            "POST",
            "/totp/disable",
            Some(&sid),
            &format!("csrf={csrf}&code={next}"),
        ),
        "8.8.8.10",
    )
    .await;
    assert!(f.state.creds.exists(crate::totp::CREDENTIAL));
    // 집 안에서 끄면 밖의 세션도 끝난다.
    *f.state.totp_last.lock().unwrap() = Some(crate::totp::step_of(now));
    f.send(post(
        "/totp/disable",
        true,
        &format!("csrf={CSRF}&code={next}"),
    ))
    .await;
    assert!(!f.state.creds.exists(crate::totp::CREDENTIAL));
    assert!(!f.state.sessions.lock().unwrap().contains_key(&sid));
}

/// multipart 본문(글 칸들과 파일 하나).
fn multipart(fields: &[(&str, &str)], file: Option<&[u8]>) -> Request<Body> {
    let boundary = "XEXPRBOUNDARY";
    let mut out: Vec<u8> = Vec::new();
    for (name, value) in fields {
        out.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    if let Some(bytes) = file {
        out.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n")
                .as_bytes(),
        );
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Request::post("/expressions/add")
        .header("host", HOST)
        .header("cookie", format!("{COOKIE}={SID}"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(out))
        .unwrap()
}

/// 결정 0045: 이모지·스티커를 관리 페이지에서 올리고, 고치고, 지운다. 그림은 검사만 한다.
#[tokio::test]
async fn expressions_are_uploaded_edited_and_removed_from_the_page() {
    use crate::expressions::testing::png;
    let f = fixture().await;
    f.log_in();
    // 서비스가 붙기 전에는 화면이 그렇게 말하고, 올리기는 까닭과 함께 거절된다.
    let v = page_json(&f, "/expressions").await;
    assert!(
        v["snapshot"]["expressions"].is_null(),
        "서비스가 붙기 전에는 이 데몬이 이모지·스티커를 다루지 않는다"
    );
    let (itx, _irx) = mpsc::channel(1);
    let (ptx, _prx) = mpsc::channel(1);
    let gw = crate::gateway::Gateway::new(f.state.store.clone(), itx, ptx);
    let x = crate::expressions::Expressions::new(
        f.state.store.clone(),
        f._dir.0.join("data/expressions"),
    );
    gw.set_expressions(x.clone());
    f.state.set_gateway(gw);
    // 틀린 CSRF는 받지 않는다.
    let r = f
        .send(multipart(
            &[("csrf", "bad"), ("kind", "emoji"), ("name", "lucia_smile")],
            Some(&png(128)),
        ))
        .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert!(x.catalog().is_empty());
    // 올리기: 목록에 나오고 미리보기 그림이 열린다.
    let r = f
        .send(multipart(
            &[
                ("csrf", CSRF),
                ("kind", "emoji"),
                ("name", "lucia_smile"),
                ("label", "루시아 <웃음>"),
                ("description", "기분 좋게 웃는 얼굴"),
                ("tags", "웃음, 좋아"),
                ("category", ""),
            ],
            Some(&png(128)),
        ))
        .await;
    assert_eq!(location(&r), "/expressions");
    let item = x.catalog().emoji[0].clone();
    assert_eq!(item.tags, ["웃음", "좋아"]);
    assert_eq!(item.origin, "admin");
    let v = page_json(&f, "/expressions").await;
    assert!(flash_text(&v).contains("올렸습니다") && flash_text(&v).contains("lucia_smile"));
    assert_eq!(v["flash"]["tone"], "ok");
    let listed = &v["snapshot"]["expressions"]["emoji"];
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["name"], "lucia_smile");
    assert_eq!(
        listed[0]["label"], "루시아 <웃음>",
        "보이는 이름은 글 그대로 실린다"
    );
    assert_eq!(listed[0]["origin"], "admin");
    assert_eq!(
        listed[0]["file"]["id"],
        item.file.id.as_str(),
        "미리보기 그림의 주소를 만들 값"
    );
    let r = f
        .send(get_html(
            &format!("/expressions/file/{}", item.file.id),
            true,
        ))
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers().get("content-type").unwrap(), "image/png");
    assert_eq!(
        r.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    let r = f
        .send(get_html(
            &format!("/expressions/file/{}", item.file.id),
            false,
        ))
        .await;
    assert_ne!(
        r.status(),
        StatusCode::OK,
        "로그인 없이는 그림을 주지 않는다"
    );
    // 규격에 맞지 않는 그림은 까닭과 함께 거절한다.
    f.send(multipart(
        &[("csrf", CSRF), ("kind", "sticker"), ("name", "축하")],
        Some(&png(128)),
    ))
    .await;
    let v = page_json(&f, "/expressions").await;
    assert!(
        flash_text(&v).contains("올리지 않았습니다") && flash_text(&v).contains("160~640px"),
        "{v}"
    );
    assert_eq!(v["flash"]["tone"], "bad");
    f.send(multipart(
        &[("csrf", CSRF), ("kind", "emoji"), ("name", "nofile")],
        None,
    ))
    .await;
    let v = page_json(&f, "/expressions").await;
    assert!(flash_text(&v).contains("파일이 없습니다"), "{v}");
    // 분류 만들기 → 고치기(분류, 설명) → 분류 지우기.
    let r = f
        .send(post(
            "/expressions/categories/save",
            true,
            &form(&[("csrf", CSRF), ("id", "lucia"), ("label", "루시아")]),
        ))
        .await;
    assert_eq!(location(&r), "/expressions");
    assert_eq!(x.catalog().categories[0].label, "루시아");
    f.send(post(
        &format!("/expressions/{}/update", item.id),
        true,
        &form(&[
            ("csrf", CSRF),
            ("label", "미소"),
            ("description", "살짝 웃는 얼굴"),
            ("tags", "미소"),
            ("category", "lucia"),
        ]),
    ))
    .await;
    let edited = x.catalog().emoji[0].clone();
    assert_eq!(
        (
            edited.label.as_str(),
            edited.category.as_str(),
            edited.name.as_str()
        ),
        ("미소", "lucia", "lucia_smile")
    );
    f.send(post(
        "/expressions/categories/save",
        true,
        &form(&[
            ("csrf", CSRF),
            ("id", "lucia"),
            ("label", "루시아 묶음"),
            ("sort", "7"),
            ("icon", &item.id),
        ]),
    ))
    .await;
    let cat = x.catalog().categories[0].clone();
    assert_eq!(
        (cat.label.as_str(), cat.sort, cat.icon.as_deref()),
        ("루시아 묶음", 7, Some(item.id.as_str()))
    );
    f.send(post(
        "/expressions/categories/lucia/delete",
        true,
        &form(&[("csrf", CSRF)]),
    ))
    .await;
    assert!(x.catalog().categories.is_empty());
    assert_eq!(x.catalog().emoji[0].category, "");
    // 지우기: 목록에서 빠지고 활동 기록에 남는다. 그림은 보관 기간 동안 열린다.
    let r = f
        .send(post(
            &format!("/expressions/{}/delete", item.id),
            true,
            &form(&[("csrf", CSRF)]),
        ))
        .await;
    assert_eq!(location(&r), "/expressions");
    assert!(x.catalog().is_empty());
    assert!(x.file(&item.file.id).is_some());
    let kinds: Vec<String> = f
        .state
        .store()
        .recent_activity(10)
        .unwrap()
        .into_iter()
        .map(|a| a.kind)
        .collect();
    for kind in ["expression_add", "expression_edit", "expression_remove"] {
        assert!(kinds.iter().any(|k| k == kind), "{kind}: {kinds:?}");
    }
}

/// 결정 0046: OpenAI API 키와 이미지 하루 예산을 "연결" 화면에서 다룬다. 키 값은 다시 보이지 않는다.
#[tokio::test]
async fn image_key_and_budget_are_managed_from_the_connections_page() {
    if !crate::credentials::user_credentials_work() {
        eprintln!("systemd-creds --user를 쓸 수 없음(systemd 256 미만이거나 없음), 건너뜀");
        return;
    }
    let f = fixture().await;
    f.log_in();
    let v = page_json(&f, "/connections").await;
    assert_eq!(
        v["snapshot"]["images"]["key_set"], false,
        "키가 없으면 입력 칸이 나온다"
    );
    // 키답지 않은 값은 저장하지 않는다.
    for bad in ["short", "has space in the middle of it 1234"] {
        f.send(post(
            "/images/key",
            true,
            &form(&[("csrf", CSRF), ("key", bad)]),
        ))
        .await;
        assert!(!f.state.creds.exists(crate::imagegen::CREDENTIAL), "{bad}");
        let v = page_json(&f, "/connections").await;
        assert!(flash_text(&v).contains("키를 저장하지 않았습니다"), "{v}");
        assert_eq!(v["flash"]["tone"], "bad");
        assert_eq!(v["snapshot"]["images"]["key_set"], false);
    }
    // 저장하면 값은 화면에 나오지 않고, 지우는 단추가 생긴다.
    let key = "sk-test-0123456789abcdefghijklmnop";
    let r = f
        .send(post(
            "/images/key",
            true,
            &form(&[("csrf", CSRF), ("key", key)]),
        ))
        .await;
    assert_eq!(location(&r), "/connections");
    assert!(f.state.creds.exists(crate::imagegen::CREDENTIAL));
    assert_eq!(
        f.state
            .creds
            .read(crate::imagegen::CREDENTIAL)
            .await
            .unwrap()
            .as_deref(),
        Some(key)
    );
    let page = body(f.send(get("/connections", true)).await).await;
    assert!(!page.contains(key), "키 값은 화면 데이터에 없다");
    let v: Value = serde_json::from_str(&page).unwrap();
    assert!(
        flash_text(&v).contains("OpenAI API 키를 저장했습니다"),
        "{v}"
    );
    assert_eq!(v["flash"]["tone"], "ok");
    assert_eq!(
        v["snapshot"]["images"]["key_set"], true,
        "지우는 단추가 생긴다"
    );
    // 예산: 숫자만, 비우면 기본값.
    let budget = |v: &'static str| {
        let f = &f;
        async move {
            f.send(post(
                "/images/budget",
                true,
                &form(&[("csrf", CSRF), ("daily", v)]),
            ))
            .await;
            crate::imagegen::daily_cap(&f.state.store())
        }
    };
    assert_eq!(budget("2.5").await, 2.5);
    assert_eq!(budget("abc").await, 2.5, "잘못된 값은 바꾸지 않는다");
    assert_eq!(budget("-1").await, 2.5);
    assert_eq!(budget("0").await, 0.0);
    assert_eq!(budget("").await, crate::imagegen::DEFAULT_DAILY_USD);
    // 최근 요청과 오늘 쓴 돈이 보인다(프롬프트는 글 그대로 실린다).
    {
        let store = f.state.store();
        store
            .add_generated_image(&lucia_store::GeneratedImage {
                id: "img_0000000000aa".into(),
                run_id: None,
                model: "gpt-image-2.5-flare".into(),
                quality: "medium".into(),
                size: "1024x1024".into(),
                background: "opaque".into(),
                prompt: "<b>고양이</b>".into(),
                status: "running".into(),
                file_id: None,
                error: None,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
                created_at: Utc::now(),
                finished_at: None,
                inputs: 0,
            })
            .unwrap();
        store
            .finish_generated_image(
                "img_0000000000aa",
                Some("f_1"),
                None,
                10,
                1000,
                0.03,
                Utc::now(),
            )
            .unwrap();
    }
    let v = page_json(&f, "/connections").await;
    let images = &v["snapshot"]["images"];
    assert_eq!(images["recent"].as_array().unwrap().len(), 1);
    assert_eq!(
        images["recent"][0]["prompt"], "<b>고양이</b>",
        "글 그대로 실린다"
    );
    assert_eq!(images["recent"][0]["model"], "gpt-image-2.5-flare");
    assert_eq!(images["recent"][0]["cost_usd"], 0.03);
    assert_eq!(images["today_usd"], 0.03);
    assert_eq!(
        images["daily_cap"],
        crate::imagegen::DEFAULT_DAILY_USD,
        "마지막으로 비워 저장한 예산은 기본값"
    );
    // 지우면 다시 입력 칸이 나온다.
    f.send(post("/images/key/remove", true, &form(&[("csrf", CSRF)])))
        .await;
    assert!(!f.state.creds.exists(crate::imagegen::CREDENTIAL));
    let v = page_json(&f, "/connections").await;
    assert_eq!(
        v["snapshot"]["images"]["key_set"], false,
        "지우면 다시 입력 칸이 나온다"
    );
}

/// 결정 0052: 보관한 파일은 설정의 "작업과 보관"에 나오고, 내려받거나 보관함에서 뺄 수 있다.
#[tokio::test]
async fn kept_files_are_listed_downloaded_and_released() {
    let f = fixture().await;
    f.log_in();
    let files = crate::files::FileStore::new(
        f.state.config.data_dir.join("files"),
        reqwest::Client::new(),
        f.state.store.clone(),
    );
    let made = files
        .save_made(
            "보고서 <최종>.txt",
            "내용입니다".as_bytes(),
            None,
            Utc::now(),
        )
        .unwrap();
    files
        .keep(&made.id, "분기 보고서", "<b>3분기</b>", Utc::now())
        .unwrap();
    let v = page_json(&f, "/settings/work").await;
    let kept = v["snapshot"]["kept_files"].as_array().unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0]["title"], "분기 보고서");
    assert_eq!(kept[0]["note"], "<b>3분기</b>", "설명은 글 그대로 실린다");
    assert_eq!(
        kept[0]["file"]["id"],
        made.id.as_str(),
        "내려받기 주소를 만들 값"
    );
    // 내려받기: 로그인해야 하고, 브라우저가 열지 않게 첨부로 준다(파일 주소는 틀로 바뀌지 않는다).
    let r = f
        .send(get_html(&format!("/files/{}/download", made.id), false))
        .await;
    assert_ne!(r.status(), StatusCode::OK);
    let r = f
        .send(get_html(&format!("/files/{}/download", made.id), true))
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        r.headers().get("content-type").unwrap(),
        "application/octet-stream"
    );
    assert!(
        r.headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );
    assert_eq!(body(r).await, "내용입니다");
    assert_eq!(
        f.send(get_html("/files/f_000000000000/download", true))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    // 빼기.
    let r = f
        .send(post(
            &format!("/files/{}/unkeep", made.id),
            true,
            &form(&[("csrf", CSRF), ("back", "/settings/work")]),
        ))
        .await;
    assert_eq!(location(&r), "/settings/work");
    assert!(files.kept().is_empty());
    let v = page_json(&f, "/settings/work").await;
    assert!(flash_text(&v).contains("보관함에서 뺐습니다"), "{v}");
    assert!(v["snapshot"]["kept_files"].as_array().unwrap().is_empty());
}

// ---------- 결정 0064: 초기 설정, 이름, 프로필 그림 ----------

/// 프로필 그림을 정하는 곳을 붙인다(가짜 Discord 봇은 연결되지 않은 채로 시작한다). 그림은 임시 디렉터리에 둔다.
fn attach_profile(
    f: &Fixture,
) -> (
    crate::profile::ProfileAccess,
    Arc<crate::profile::tests::FakeHost>,
) {
    let host = Arc::new(crate::profile::tests::FakeHost::default());
    let dir = f._dir.0.join("profile-data");
    let access = crate::profile::ProfileAccess {
        profile: crate::profile::Profile::new(f.state.store.clone(), dir.join("avatars")),
        discord: Some(host.clone()),
        files: Arc::new(crate::files::FileStore::new(
            dir.join("files"),
            reqwest::Client::new(),
            f.state.store.clone(),
        )),
    };
    f.state.set_profile(access.clone());
    (access, host)
}

/// `/profile/avatar`로 보내는 multipart(글 칸들과 파일 하나).
fn avatar_upload(fields: &[(&str, &str)], file: Option<&[u8]>, logged_in: bool) -> Request<Body> {
    let boundary = "XAVATARBOUNDARY";
    let mut out: Vec<u8> = Vec::new();
    for (name, value) in fields {
        out.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    if let Some(bytes) = file {
        out.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n")
                .as_bytes(),
        );
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let mut b = Request::post("/profile/avatar")
        .header("host", HOST)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        );
    if logged_in {
        b = b.header("cookie", format!("{COOKIE}={SID}"));
    }
    b.body(Body::from(out)).unwrap()
}

/// 이름은 다듬어 저장하고, 규칙을 어기면 바꾸지 않고, 비우면 기본으로 돌아간다. 화면 데이터와 로그인 없는 화면에 실린다.
#[tokio::test]
async fn the_name_is_set_validated_reset_and_shown_on_every_screen() {
    let f = fixture().await;
    f.log_in();
    // 정하기 전에는 기본 이름이고, 이름을 직접 정한 것이 아니다.
    let v = page_json(&f, "/start").await;
    assert_eq!(v["name"], "Lucia");
    assert_eq!(v["snapshot"]["profile"]["name"], "Lucia");
    assert_eq!(v["snapshot"]["profile"]["name_set"], false);
    assert_eq!(v["consts"]["profile"]["default_name"], "Lucia");
    assert_eq!(v["consts"]["profile"]["name_max_chars"], 24);

    // 온 화면으로 돌아간다(정해 둔 목록 안에서만). 받은 이름은 다듬는다.
    let r = f
        .send(post(
            "/profile/name",
            true,
            &form(&[("csrf", CSRF), ("name", "  루시아 "), ("back", "/system")]),
        ))
        .await;
    assert_eq!(location(&r), "/system");
    let v = page_json(&f, "/connections").await;
    assert_eq!(v["name"], "루시아");
    assert_eq!(v["snapshot"]["profile"]["name"], "루시아");
    assert_eq!(v["snapshot"]["profile"]["name_set"], true);
    assert!(
        flash_text(&v).contains("“루시아”") && v["flash"]["tone"] == "ok",
        "{v}"
    );
    assert_eq!(
        crate::profile::display_name(&f.state.store()),
        "루시아",
        "저장소에 다듬은 이름이 있다"
    );
    assert!(
        f.state
            .store()
            .recent_activity(5)
            .unwrap()
            .iter()
            .any(|a| a.kind == "profile_name" && a.summary.contains("루시아")),
        "활동 기록에 남는다"
    );
    // 돌아갈 곳이 없으면 초기 설정, 목록 밖이면 홈(열린 리다이렉트 없음).
    for (back, to) in [
        (None, "/start"),
        (Some("https://evil.example/"), "/"),
        (Some("//evil.example"), "/"),
    ] {
        let mut pairs = vec![("csrf", CSRF), ("name", "달빛")];
        if let Some(back) = back {
            pairs.push(("back", back));
        }
        let r = f.send(post("/profile/name", true, &form(&pairs))).await;
        assert_eq!(location(&r), to, "{back:?}");
    }

    // 규칙을 어기면 바꾸지 않고 까닭을 알린다.
    for bad in ["한\n줄", &"가".repeat(25), "탭\t이름"] {
        f.send(post(
            "/profile/name",
            true,
            &form(&[("csrf", CSRF), ("name", bad)]),
        ))
        .await;
        let v = page_json(&f, "/start").await;
        assert!(flash_text(&v).contains("이름을 저장하지 않았습니다"), "{v}");
        assert_eq!(v["flash"]["tone"], "bad");
        assert_eq!(v["name"], "달빛", "바뀌지 않는다: {bad:?}");
    }
    // 24자는 된다.
    let longest = "나".repeat(24);
    f.send(post(
        "/profile/name",
        true,
        &form(&[("csrf", CSRF), ("name", &longest)]),
    ))
    .await;
    assert_eq!(page_json(&f, "/").await["name"], longest.as_str());

    // 로그인 없는 화면에도 이름이 실린다(처리기가 이름을 모르는 화면까지: 없는 주소, 틀린 링크).
    for path in [
        "/login",
        "/setup?token=nope",
        "/no/such/page",
        "/site-login/x",
    ] {
        let r = f.send(get(path, false)).await;
        let v = json(r).await;
        assert_eq!(v["kind"], "standalone", "{path}: {v}");
        assert_eq!(v["name"], longest.as_str(), "{path}");
    }
    set_password(&f);
    let v = json(f.send(get("/login", false)).await).await;
    assert_eq!(screen(&v)["type"], "login");
    assert_eq!(v["name"], longest.as_str());
    // 로그인 실패 화면(401)에도.
    let r = f.send(post_json("/login", false, "password=wrong")).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json(r).await["name"], longest.as_str());

    // 비우면 기본 이름으로 돌아가고 정한 이름은 지워진다.
    f.send(post(
        "/profile/name",
        true,
        &form(&[("csrf", CSRF), ("name", "   ")]),
    ))
    .await;
    let v = page_json(&f, "/start").await;
    assert_eq!(v["name"], "Lucia");
    assert_eq!(v["snapshot"]["profile"]["name_set"], false);
    assert!(flash_text(&v).contains("기본값"), "{v}");
    assert_eq!(
        f.state.store().setting(crate::profile::NAME_KEY).unwrap(),
        None
    );
    assert_eq!(
        json(f.send(get("/no/such/page", false)).await).await["name"],
        "Lucia"
    );
}

/// 프로필이 붙어 있으면 이름을 바꿀 때 연결된 앱에 알릴 수 있다(앱에 가는 프레임은 게이트웨이 시험이 본다).
#[tokio::test]
async fn the_name_change_reaches_the_attached_profile() {
    let f = fixture().await;
    f.log_in();
    let (access, _host) = attach_profile(&f);
    let mut names = access.profile.subscribe_name();
    f.send(post(
        "/profile/name",
        true,
        &form(&[("csrf", CSRF), ("name", "루시아")]),
    ))
    .await;
    assert!(names.has_changed().unwrap());
    assert_eq!(*names.borrow_and_update(), "루시아");
    assert_eq!(access.profile.view()["name"], "루시아");
    // 규칙을 어기면 알리지 않는다.
    f.send(post(
        "/profile/name",
        true,
        &form(&[("csrf", CSRF), ("name", &"가".repeat(30))]),
    ))
    .await;
    assert!(!names.has_changed().unwrap());
    assert_eq!(access.profile.name(), "루시아");
}

/// 소유자가 관리 페이지에서 올린 그림: 규칙은 Lucia의 도구와 같고, `owner`로 남고, 앱에 알리고, 연결된 Discord에도
/// 같은 길로 적용된다. 규칙을 어기면 어긴 것을 모두 알린다(Lucia에게 하라는 말 없이).
#[tokio::test]
async fn the_owner_uploads_and_removes_the_profile_image() {
    use crate::profile::tests::jpeg;
    let f = fixture().await;
    f.log_in();
    let good = jpeg(512, 512);

    // 로그인과 CSRF를 확인한다(multipart도).
    let r = f
        .send(avatar_upload(&[("csrf", CSRF)], Some(&good), false))
        .await;
    assert_eq!(location(&r), "/login");
    let r = f
        .send(avatar_upload(&[("csrf", "wrong")], Some(&good), true))
        .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // 프로필이 붙기 전: 까닭과 함께 거절한다.
    let r = f
        .send(avatar_upload(&[("csrf", CSRF)], Some(&good), true))
        .await;
    assert_eq!(location(&r), "/start");
    let v = page_json(&f, "/start").await;
    assert!(
        flash_text(&v).contains("프로필 그림을 다루지 않습니다"),
        "{v}"
    );
    f.send(post(
        "/profile/avatar/remove",
        true,
        &form(&[("csrf", CSRF)]),
    ))
    .await;
    assert!(flash_text(&page_json(&f, "/start").await).contains("다루지 않습니다"));

    let (access, host) = attach_profile(&f);
    let changes = access.profile.subscribe();
    // 파일이 없으면 알린다.
    f.send(avatar_upload(&[("csrf", CSRF)], None, true)).await;
    assert!(flash_text(&page_json(&f, "/start").await).contains("파일이 없습니다"));

    // 규칙에 맞는 그림: owner가 정하고, 앱에 알리고(구독), Discord는 연결되어 있지 않아 건드리지 않는다.
    let r = f
        .send(avatar_upload(
            &[("csrf", CSRF), ("back", "/start")],
            Some(&good),
            true,
        ))
        .await;
    assert_eq!(location(&r), "/start");
    let avatar = access.profile.avatar().expect("그림이 정해졌다");
    assert_eq!(
        (avatar.by.as_str(), avatar.width, avatar.mime.as_str()),
        ("owner", 512, "image/jpeg")
    );
    assert!(
        changes.has_changed().unwrap(),
        "앱에 알리는 구독이 깨어난다"
    );
    let v = page_json(&f, "/start").await;
    let said = flash_text(&v);
    assert!(
        said.contains("프로필 그림을 바꿨습니다")
            && said.contains("연결된 앱에 바로 알렸습니다")
            && said.contains("Discord는 연결되어 있지 않아"),
        "{said}"
    );
    assert_eq!(v["flash"]["tone"], "ok");
    assert!(host.sets.lock().unwrap().is_empty());
    let shown = &v["snapshot"]["profile"]["avatar"];
    assert_eq!(shown["id"], avatar.id.as_str());
    assert_eq!(shown["by"], "owner");
    assert_eq!(shown["ui"]["by_label"], "내가 정함");
    assert!(
        shown["ui"]["facts"]
            .as_str()
            .is_some_and(|t| t.starts_with("512×512 · JPEG")),
        "{shown}"
    );
    assert_eq!(
        v["consts"]["profile"]["avatar_rules"],
        crate::profile::rules_text()
    );
    assert!(
        f.state
            .store()
            .recent_activity(5)
            .unwrap()
            .iter()
            .any(|a| a.kind == "profile_image" && a.summary.contains("관리 페이지")),
        "활동 기록에 남는다"
    );

    // 그림 받기: 로그인한 사람만. 그림 주소는 틀로 바뀌지 않고, id가 해시라 오래 캐시한다.
    let path = format!("/profile/avatar/file/{}", avatar.id);
    assert_eq!(
        f.send(get_html(&path, false)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let r = f.send(get_html(&path, true)).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()["content-type"], "image/jpeg");
    assert_eq!(r.headers()["x-content-type-options"], "nosniff");
    assert!(
        r.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    assert_eq!(bytes.to_vec(), good);
    for missing in [
        "/profile/avatar/file/a_000000000000000000000000",
        "/profile/avatar/file/not-an-id",
    ] {
        assert_eq!(
            f.send(get_html(missing, true)).await.status(),
            StatusCode::NOT_FOUND,
            "{missing}"
        );
    }

    // 규칙을 어기면 바꾸지 않고, 어긴 것을 모두 알린다. Lucia에게 하라는 말은 없다.
    let mut big = jpeg(512, 512);
    big.resize(crate::profile::MAX_BYTES as usize + 1, 0);
    // 2 MiB(axum의 기본 요청 상한)를 넘어도 규칙의 3 MiB 안내가 나온다. 7 MiB까지는 받아서 알린다.
    let mut huge = jpeg(512, 512);
    huge.resize(7 * 1024 * 1024, 0);
    for (what, file, expect) in [
        (
            "정사각형이 아니고 큼",
            jpeg(4000, 3000),
            vec!["정사각형이 아니다(4000×3000)", "너무 크다"],
        ),
        ("너무 작음", jpeg(64, 64), vec!["너무 작다"]),
        ("3 MiB를 넘음", big, vec!["파일이 크다"]),
        ("7 MiB", huge, vec!["파일이 크다"]),
        (
            "그림이 아님",
            b"hello world".to_vec(),
            vec!["그림 파일이 아니"],
        ),
    ] {
        f.send(avatar_upload(&[("csrf", CSRF)], Some(&file), true))
            .await;
        let v = page_json(&f, "/start").await;
        let said = flash_text(&v);
        assert_eq!(v["flash"]["tone"], "bad", "{what}: {said}");
        assert!(said.contains("올리지 않았습니다"), "{what}: {said}");
        for part in expect {
            assert!(said.contains(part), "{what}: {said}");
        }
        assert!(
            !said.contains("pillow") && !said.contains("code_run"),
            "{what}: 소유자에게 Lucia의 지시를 보이지 않는다: {said}"
        );
        assert_eq!(access.profile.avatar().unwrap().id, avatar.id, "{what}");
    }
    // 요청 크기 상한(9 MiB)을 넘으면 받지 못했다고 알리고 그림은 그대로다.
    let mut too_much = jpeg(512, 512);
    too_much.resize(10 * 1024 * 1024, 0);
    let r = f
        .send(avatar_upload(&[("csrf", CSRF)], Some(&too_much), true))
        .await;
    assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(body(r).await.contains("그림을 받지 못했습니다"));
    assert_eq!(access.profile.avatar().unwrap().id, avatar.id);

    // Discord가 연결되어 있으면 Lucia의 도구와 같은 길로 봇 아바타도 바꾼다(처음이라 원래 아바타를 보관한다).
    *host.online.lock().unwrap() = true;
    *host.avatar.lock().unwrap() = Some(crate::expressions::testing::png(256));
    let second = crate::expressions::testing::png(256 + 128);
    f.send(avatar_upload(&[("csrf", CSRF)], Some(&second), true))
        .await;
    let v = page_json(&f, "/start").await;
    let said = flash_text(&v);
    assert!(
        said.contains("Discord 봇의 아바타도 바꿨다") && said.contains("보관함"),
        "{said}"
    );
    assert_eq!(host.sets.lock().unwrap().len(), 1);
    let current = access.profile.avatar().unwrap();
    assert_ne!(current.id, avatar.id);
    assert_eq!(current.by, "owner");
    assert_eq!(
        access.profile.discord_state().applied.as_deref(),
        Some(current.id.as_str())
    );

    // 지우기: 앱은 기본 모습으로, Discord는 원래 아바타로 돌아간다. 한 번 더 지워도 된다.
    let r = f
        .send(post(
            "/profile/avatar/remove",
            true,
            &form(&[("csrf", CSRF), ("back", "/start")]),
        ))
        .await;
    assert_eq!(location(&r), "/start");
    assert_eq!(access.profile.avatar(), None);
    let v = page_json(&f, "/start").await;
    let said = flash_text(&v);
    assert!(
        said.contains("프로필 그림을 지웠습니다") && said.contains("원래 것으로 되돌렸다"),
        "{said}"
    );
    assert!(v["snapshot"]["profile"]["avatar"].is_null());
    assert_eq!(host.sets.lock().unwrap().len(), 2);
    f.send(post(
        "/profile/avatar/remove",
        true,
        &form(&[("csrf", CSRF)]),
    ))
    .await;
    assert!(flash_text(&page_json(&f, "/start").await).contains("정한 프로필 그림이 없었습니다"));
}

/// 초기 설정의 상태: 필수는 대화할 곳 하나(Discord 토큰과 소유자, 또는 알림 받을 곳으로 고른 앱)뿐이다.
#[tokio::test]
async fn setup_is_complete_once_there_is_a_place_to_talk() {
    if !crate::credentials::user_credentials_work() {
        eprintln!("systemd-creds --user를 쓸 수 없음(systemd 256 미만이거나 없음), 건너뜀");
        return;
    }
    let f = fixture_with("\n[gateway]\nbind = [\"127.0.0.1:18790\"]\n").await;
    f.log_in();
    let steps = |v: &Value| -> Vec<(String, String)> {
        v["snapshot"]["setup"]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["key"].as_str().unwrap().to_owned(),
                    s["state"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    fn states(v: &Value) -> Vec<&str> {
        v["snapshot"]["setup"]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["state"].as_str().unwrap())
            .collect()
    }

    // 막 설치: 끝나지 않았고, 대화할 곳을 만드는 두 길이 해야 할 일이다. 나머지는 선택.
    let v = page_json(&f, "/start").await;
    let setup = &v["snapshot"]["setup"];
    assert_eq!(setup["complete"], false);
    assert_eq!(setup["dismissed"], false);
    assert_eq!(
        steps(&v)
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>(),
        ["profile", "discord", "google", "app", "images", "env"]
    );
    // 점검은 시험이 미리 채운 한 항목이 정상이다.
    assert_eq!(
        states(&v),
        ["optional", "todo", "optional", "todo", "optional", "done"]
    );
    for step in setup["steps"].as_array().unwrap() {
        assert!(
            step["title"].as_str().is_some_and(|t| !t.is_empty())
                && step["summary"].as_str().is_some_and(|t| !t.is_empty())
                && step["icon"].as_str().is_some_and(|t| !t.is_empty()),
            "{step}"
        );
    }
    assert_eq!(
        setup["steps"][1]["summary"],
        "봇 토큰이 없습니다 · 소유자 없음"
    );
    assert!(
        setup["env"].as_array().unwrap().len() >= 6
            && setup["env"][0]["key"] == "config"
            && setup["env"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| { e["key"] == "sandbox" && e["command"] == "deploy/create-sandbox.sh" }),
        "{setup}"
    );
    assert!(setup["config_path"].is_null(), "경로를 모르면 null");

    // 토큰만으로는 끝나지 않는다(소유자도 있어야 메시지를 받는다).
    f.state
        .creds
        .write(
            crate::credentials::names::DISCORD_BOT_TOKEN,
            "token-for-test",
        )
        .await
        .unwrap();
    let v = page_json(&f, "/start").await;
    assert_eq!(v["snapshot"]["setup"]["complete"], false);
    assert_eq!(states(&v)[1], "todo");
    assert!(
        v["snapshot"]["setup"]["steps"][1]["summary"]
            .as_str()
            .unwrap()
            .contains("소유자 없음")
    );
    assert!(
        !serde_json::to_string(&v)
            .unwrap()
            .contains("token-for-test"),
        "토큰 값은 화면 데이터에 없다"
    );

    // 토큰과 소유자가 있으면 끝: Discord는 끝남, 앱은 선택으로 바뀐다.
    f.state
        .store()
        .set_setting(
            lucia_store::keys::OWNER_DISCORD_USER_ID,
            "123456789012345678",
            Utc::now(),
        )
        .unwrap();
    let v = page_json(&f, "/start").await;
    assert_eq!(v["snapshot"]["setup"]["complete"], true);
    assert_eq!(
        states(&v),
        [
            "optional", "done", "optional", "optional", "optional", "done"
        ]
    );
    f.state
        .store()
        .delete_setting(lucia_store::keys::OWNER_DISCORD_USER_ID)
        .unwrap();
    f.state
        .creds
        .remove(crate::credentials::names::DISCORD_BOT_TOKEN)
        .await
        .unwrap();
    assert_eq!(
        page_json(&f, "/start").await["snapshot"]["setup"]["complete"],
        false
    );

    // 알림 받을 곳으로 고른 앱(앱 모두, 결정 0067)도 대화할 곳이다(Discord 없이).
    f.send(post(
        "/clients/new",
        true,
        &form(&[("csrf", CSRF), ("name", "휴대폰")]),
    ))
    .await;
    let v = page_json(&f, "/connections").await;
    let cid = v["snapshot"]["clients"][0][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // 앱 토큰을 만들기만 해서는 끝나지 않는다. 알림 받을 곳으로 골라야 한다.
    assert_eq!(
        page_json(&f, "/start").await["snapshot"]["setup"]["complete"],
        false
    );
    f.send(post(
        "/app/notify",
        true,
        &form(&[("csrf", CSRF), ("client", crate::gateway::ALL_APPS)]),
    ))
    .await;
    let v = page_json(&f, "/start").await;
    assert_eq!(v["snapshot"]["setup"]["complete"], true);
    assert_eq!(
        states(&v),
        [
            "optional", "optional", "optional", "done", "optional", "done"
        ]
    );
    assert_eq!(
        v["snapshot"]["setup"]["steps"][3]["summary"],
        "앱 토큰 1개 · 알림 받는 곳 앱 모두 · 푸시 꺼짐"
    );
    // 앱의 토큰을 취소하면 다시 끝나지 않은 것이 된다.
    f.send(post(
        &format!("/clients/{cid}/revoke"),
        true,
        &form(&[("csrf", CSRF)]),
    ))
    .await;
    assert_eq!(
        page_json(&f, "/start").await["snapshot"]["setup"]["complete"],
        false
    );
}

/// 접속 API가 꺼져 있으면 앱 단계는 설정 파일의 키를 알려 주는 읽기 전용 상태다.
#[tokio::test]
async fn setup_says_so_when_the_gateway_is_off() {
    let f = fixture().await;
    f.log_in();
    let v = page_json(&f, "/start").await;
    assert_eq!(v["snapshot"]["gateway_binds"].as_array().unwrap().len(), 0);
    let app = &v["snapshot"]["setup"]["steps"][3];
    assert_eq!(app["key"], "app");
    assert!(
        app["summary"].as_str().unwrap().contains("[gateway] bind"),
        "{app}"
    );
    let gateway = v["snapshot"]["setup"]["env"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "gateway")
        .unwrap();
    assert_eq!(gateway["tone"], "muted");
    assert!(
        gateway["detail"]
            .as_str()
            .unwrap()
            .contains("[gateway] bind")
    );
}

/// 안내 끄기와 다시 켜기. 끄기는 홈으로, 다시 켜기는 초기 설정으로 돌아간다.
#[tokio::test]
async fn setup_notice_is_dismissed_and_reopened() {
    let f = fixture().await;
    f.log_in();
    assert_eq!(
        page_json(&f, "/").await["snapshot"]["setup"]["dismissed"],
        false
    );
    let r = f
        .send(post("/start/dismiss", true, &form(&[("csrf", CSRF)])))
        .await;
    assert_eq!(location(&r), "/");
    let v = page_json(&f, "/").await;
    assert_eq!(v["snapshot"]["setup"]["dismissed"], true);
    assert_eq!(
        v["snapshot"]["setup"]["complete"], false,
        "끄는 것은 끝낸 것이 아니다"
    );
    assert!(flash_text(&v).contains("안내를 껐습니다"), "{v}");
    assert_eq!(
        f.state
            .store()
            .setting(setup::DISMISSED_KEY)
            .unwrap()
            .as_deref(),
        Some("1")
    );
    let r = f
        .send(post("/start/reopen", true, &form(&[("csrf", CSRF)])))
        .await;
    assert_eq!(location(&r), "/start");
    let v = page_json(&f, "/start").await;
    assert_eq!(v["snapshot"]["setup"]["dismissed"], false);
    assert!(flash_text(&v).contains("다시 보입니다"), "{v}");
    assert_eq!(f.state.store().setting(setup::DISMISSED_KEY).unwrap(), None);
}

/// 로그인한 뒤 첫 화면: 대화할 곳이 없고 안내를 끄지 않았을 때만 초기 설정이다. 이미 설정한 설치는 전과 같이 홈이다.
#[tokio::test]
async fn login_lands_on_the_setup_screen_only_while_it_is_unfinished() {
    if !crate::credentials::user_credentials_work() {
        eprintln!("systemd-creds --user를 쓸 수 없음(systemd 256 미만이거나 없음), 건너뜀");
        return;
    }
    let f = fixture().await;
    set_password(&f);
    let login = || async {
        let r = f
            .send(post_json("/login", false, "password=correct+horse+battery"))
            .await;
        assert_eq!(r.status(), StatusCode::OK);
        let pair = cookie_pair(&r);
        let landed = json(r).await["redirect"].as_str().unwrap().to_owned();
        // 이미 로그인한 채 로그인 주소를 열어도 같은 곳으로 보낸다.
        let mut req = get("/login", false);
        req.headers_mut().insert("cookie", pair.parse().unwrap());
        let again = json(f.send(req).await).await["redirect"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(landed, again);
        landed
    };
    // 새 설치.
    assert_eq!(login().await, "/start");
    // 안내를 껐다.
    f.state
        .store()
        .set_setting(setup::DISMISSED_KEY, "1", Utc::now())
        .unwrap();
    assert_eq!(login().await, "/");
    f.state
        .store()
        .delete_setting(setup::DISMISSED_KEY)
        .unwrap();
    // 토큰만 있다: 아직 끝나지 않았다.
    f.state
        .creds
        .write(
            crate::credentials::names::DISCORD_BOT_TOKEN,
            "token-for-test",
        )
        .await
        .unwrap();
    assert_eq!(login().await, "/start");
    // 토큰과 소유자: 설정한 설치다.
    f.state
        .store()
        .set_setting(
            lucia_store::keys::OWNER_DISCORD_USER_ID,
            "123456789012345678",
            Utc::now(),
        )
        .unwrap();
    assert_eq!(login().await, "/");
    // Discord 없이 알림 받을 곳으로 고른 앱.
    f.state
        .store()
        .delete_setting(lucia_store::keys::OWNER_DISCORD_USER_ID)
        .unwrap();
    assert_eq!(login().await, "/start");
    {
        let store = f.state.store();
        store
            .create_client(
                "cl-landing",
                "휴대폰",
                crate::gateway::Role::App.as_str(),
                "hash",
                Utc::now(),
            )
            .unwrap();
        store
            .set_setting(
                crate::gateway::keys::NOTIFY_CLIENT,
                "cl-landing",
                Utc::now(),
            )
            .unwrap();
    }
    assert_eq!(login().await, "/");
}
