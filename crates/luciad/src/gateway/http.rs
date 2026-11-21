//! 접속 API의 HTTP 부분(결정 0043): WebSocket에 싣기 어려운 것(파일)과 연결 없이 하는 것(기록 읽기, 푸시 토큰
//! 등록 — 백그라운드에서 토큰이 바뀔 때). 모두 `Authorization: Bearer <토큰>`이고, `app` 역할만 받는다.
//! 출처 확인(집 안·믿는 프록시)과 인증 실패 막기는 WebSocket과 같다.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Gateway, Role, conversation, methods};

pub(super) fn routes() -> Router<Arc<Gateway>> {
    Router::new()
        .route("/v1/me", get(me))
        .route("/v1/history", get(history))
        .route(
            "/v1/files",
            axum::routing::post(upload).layer(DefaultBodyLimit::max(
                crate::files::MAX_DOWNLOAD as usize + 1024,
            )),
        )
        .route("/v1/files/{id}", get(download))
        .route("/v1/push", axum::routing::put(push_put).delete(push_delete))
        .route("/v1/expressions", get(expressions))
        .route("/v1/expressions/files/{id}", get(expression_file))
        .route("/v1/profile", get(profile))
        .route("/v1/profile/avatar/{id}", get(profile_avatar))
        .route("/v1/browser/handover/frame/{tab}", get(handover_frame))
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

/// 출처와 토큰을 확인하고 클라이언트를 돌려준다(거절이면 바로 돌려줄 응답).
#[allow(clippy::result_large_err)]
fn authorize(
    gw: &Gateway,
    peer: SocketAddr,
    headers: &HeaderMap,
) -> Result<lucia_store::Client, Response> {
    let ip = gw
        .admit(peer, headers)
        .map_err(|s| error(s, "forbidden", "받지 않는 출처"))?;
    if gw.blocked(ip) {
        return Err(error(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_attempts",
            "인증 실패가 많아 잠시 막았다",
        ));
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or("");
    let Some(client) = (!token.is_empty())
        .then(|| gw.authenticate(token))
        .flatten()
    else {
        gw.failed(ip);
        tracing::warn!(%ip, "접속 API(HTTP) 인증 실패");
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "토큰이 틀렸거나 취소됐다",
        ));
    };
    if Role::parse(&client.role) != Some(Role::App) {
        return Err(error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "앱 토큰만 받는다",
        ));
    }
    gw.touch(&client.id);
    Ok(client)
}

async fn me(
    State(gw): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let client = match authorize(&gw, peer, &headers) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let last = gw
        .store
        .lock()
        .expect("store lock")
        .outbox_last(&client.id)
        .unwrap_or(0);
    let mut body = json!({
        "v": super::PROTOCOL_VERSION,
        "client": client.id,
        "name": client.name,
        "conversation": conversation(&client.id).0,
        "scopes": client.scopes,
        "methods": gw.method_names(&client),
        "seq": last,
        "acked": client.acked_seq,
        "online": gw.online(&client.id),
        "push": {"available": gw.push_ready(), "registered": client.push_token.is_some()},
    });
    if let Some(x) = gw.expressions() {
        body["expressions"] = x.catalog().summary();
    }
    if let Some(p) = gw.profile() {
        body["profile"] = p.view();
    }
    Json(body).into_response()
}

#[derive(Deserialize)]
struct HistoryQuery {
    before: Option<i64>,
    limit: Option<u64>,
    scope: Option<String>,
}

async fn history(
    State(gw): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<HistoryQuery>,
) -> Response {
    let client = match authorize(&gw, peer, &headers) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let params = json!({"before": q.before, "limit": q.limit, "scope": q.scope});
    match methods::history(&gw, &client, &params) {
        Ok(v) => Json(v).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, &e.code, &e.message),
    }
}

#[derive(Deserialize)]
struct UploadQuery {
    name: Option<String>,
}

/// 파일 올리기: 본문이 파일 그대로다(`?name=<파일 이름>`). 돌려준 ID를 `message.files`에 넣는다.
async fn upload(
    State(gw): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(q): Query<UploadQuery>,
    req: axum::extract::Request,
) -> Response {
    // 토큰을 먼저 본다: 본문(최대 30 MB)은 인증한 뒤에야 읽는다.
    let client = match authorize(&gw, peer, req.headers()) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let limit = crate::files::MAX_DOWNLOAD as usize;
    let body = match axum::body::to_bytes(req.into_body(), limit).await {
        Ok(b) => b,
        Err(_) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                "파일이 너무 크거나 끊겼다",
            );
        }
    };
    let Some(files) = gw.files.get() else {
        return error(
            StatusCode::NOT_IMPLEMENTED,
            "unsupported",
            "이 데몬은 파일을 받지 않는다",
        );
    };
    let name = q.name.unwrap_or_else(|| "file".into());
    match files.save(
        "in",
        &name,
        &body,
        Some(&conversation(&client.id).0),
        chrono::Utc::now(),
    ) {
        Ok(rec) => (
            StatusCode::CREATED,
            Json(json!({
                "id": rec.id,
                "name": rec.name,
                "size": rec.size,
                "mime": rec.mime,
                "kind": rec.kind,
            })),
        )
            .into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, "bad_file", &e),
    }
}

/// 파일 받기: 이 앱 대화로 오간 파일만.
async fn download(
    State(gw): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let client = match authorize(&gw, peer, &headers) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let Some(files) = gw.files.get() else {
        return error(StatusCode::NOT_FOUND, "not_found", "없는 파일");
    };
    let conv = conversation(&client.id).0;
    let Some(rec) = files
        .get(&id)
        .filter(|f| f.conversation.as_deref() == Some(conv.as_str()))
    else {
        return error(StatusCode::NOT_FOUND, "not_found", "없는 파일");
    };
    let bytes = match files.read(&rec) {
        Ok(b) => b,
        Err(_) => return error(StatusCode::GONE, "gone", "보관 기간이 지나 지웠다"),
    };
    let mut response = bytes.into_response();
    let h = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&rec.mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    let encoded =
        percent_encoding::utf8_percent_encode(&rec.name, percent_encoding::NON_ALPHANUMERIC);
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename*=UTF-8''{encoded}")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

async fn push_put(
    State(gw): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let client = match authorize(&gw, peer, &headers) {
        Ok(c) => c,
        Err(r) => return r,
    };
    match methods::register_push(
        &gw,
        &client.id,
        v["token"].as_str().unwrap_or(""),
        v["platform"].as_str().unwrap_or(""),
    ) {
        Ok(v) => Json(v).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, &e.code, &e.message),
    }
}

async fn push_delete(
    State(gw): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let client = match authorize(&gw, peer, &headers) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let _ = gw.store.lock().expect("store lock").set_client_push(
        &client.id,
        None,
        None,
        chrono::Utc::now(),
    );
    Json(json!({"registered": false})).into_response()
}

/// 이모지·스티커 목록(결정 0045). `If-None-Match`의 판이 그대로면 304.
async fn expressions(
    State(gw): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = authorize(&gw, peer, &headers) {
        return r;
    }
    let Some(x) = gw.expressions() else {
        return error(
            StatusCode::NOT_IMPLEMENTED,
            "unsupported",
            "이 데몬은 이모지·스티커를 다루지 않는다",
        );
    };
    let catalog = x.catalog();
    let etag = format!("\"{}\"", catalog.version);
    let same = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().trim_start_matches("W/") == etag)
        });
    let mut response = if same {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        Json(catalog.json()).into_response()
    };
    let h = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    response
}

/// Lucia의 프로필(결정 0060): 지금 프로필 그림의 정보. 그림이 없으면 `avatar`가 null이다.
async fn profile(
    State(gw): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = authorize(&gw, peer, &headers) {
        return r;
    }
    let Some(p) = gw.profile() else {
        return error(
            StatusCode::NOT_IMPLEMENTED,
            "unsupported",
            "이 데몬은 프로필 이미지를 다루지 않는다",
        );
    };
    let mut response = Json(p.view()).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    response
}

/// 프로필 그림. id가 내용 해시라 바뀌지 않는다(앱이 영구 캐시한다). 그림이 바뀌면 id가 바뀐다.
async fn profile_avatar(
    State(gw): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = authorize(&gw, peer, &headers) {
        return r;
    }
    let Some((mime, bytes)) = gw.profile().and_then(|p| p.file(&id)) else {
        return error(StatusCode::NOT_FOUND, "not_found", "없는 그림");
    };
    let mut response = bytes.into_response();
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    if let Ok(v) = HeaderValue::from_str(&format!("\"{id}\"")) {
        h.insert(header::ETAG, v);
    }
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    response
}

/// 넘겨받은 브라우저 화면 한 장(JPEG, 결정 0063). 범위 `browser_handover`가 있는 토큰만. 저장하지 않게 한다.
/// 그림의 픽셀 좌표가 `browser.handover.input`의 누르는 좌표와 같다.
async fn handover_frame(
    State(gw): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(tab): Path<String>,
) -> Response {
    let client = match authorize(&gw, peer, &headers) {
        Ok(c) => c,
        Err(r) => return r,
    };
    if !client
        .scopes
        .iter()
        .any(|s| s == super::scopes::BROWSER_HANDOVER)
    {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "이 토큰에는 그 권한이 없다",
        );
    }
    let Some(browser) = gw.handover() else {
        return error(StatusCode::NOT_FOUND, "unavailable", "브라우저가 꺼져 있다");
    };
    if browser.left().await.is_none() {
        return error(StatusCode::CONFLICT, "not_active", "넘겨받은 상태가 아니다");
    }
    let tab = tab.trim_end_matches(".jpg");
    match browser.frame(tab).await {
        Ok(frame) => {
            let mut response = frame.jpeg.into_response();
            let h = response.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            h.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            response
        }
        Err(_) => error(StatusCode::NOT_FOUND, "not_found", "없는 탭"),
    }
}

/// 이모지·스티커 그림. 파일 ID가 내용 해시라 바뀌지 않는다(앱이 영구 캐시한다).
async fn expression_file(
    State(gw): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = authorize(&gw, peer, &headers) {
        return r;
    }
    let Some((info, bytes)) = gw.expressions().and_then(|x| x.file(&id)) else {
        return error(StatusCode::NOT_FOUND, "not_found", "없는 그림");
    };
    let mut response = bytes.into_response();
    let h = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&info.mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&format!("\"{}\"", info.id)) {
        h.insert(header::ETAG, v);
    }
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    response
}
