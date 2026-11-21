//! 가짜 Google 서버로 Calendar·Gmail 소스를 시험한다(네트워크 없음).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use super::*;

/// 처음에는 "t1", 버린 뒤에는 "t2"를 준다.
#[derive(Default)]
struct FakeTokens {
    invalidated: AtomicUsize,
}

impl TokenProvider for FakeTokens {
    async fn access_token(&self) -> Result<String, SourceError> {
        Ok(if self.invalidated.load(Ordering::SeqCst) == 0 {
            "t1".into()
        } else {
            "t2".into()
        })
    }

    fn invalidate(&self) {
        self.invalidated.fetch_add(1, Ordering::SeqCst);
    }
}

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

async fn events(headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    // 이 경로는 t1을 거부한다(만료된 액세스 토큰 흉내): 클라이언트가 한 번 새 토큰으로 다시 해야 한다.
    if bearer(&headers) != "Bearer t2" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match (q.get("syncToken").map(String::as_str), q.get("pageToken").map(String::as_str)) {
        (None, _) => {
            assert!(q.contains_key("timeMin"), "첫 동기화에만 timeMin");
            Json(json!({"items": [{"id": "old", "summary": "지난 일정", "updated": "u0"}], "nextSyncToken": "s1"}))
                .into_response()
        }
        (Some("expired"), _) => StatusCode::GONE.into_response(),
        (Some("s1"), None) => {
            assert!(!q.contains_key("timeMin"), "syncToken과 timeMin을 같이 쓰지 않는다");
            Json(json!({
                "items": [
                    {"id": "e1", "status": "confirmed", "summary": "기말시험", "start": {"dateTime": "2026-12-09T09:00:00+09:00"}, "updated": "u1"},
                    {"id": "e2", "status": "cancelled", "updated": "u2"}
                ],
                "nextPageToken": "p2"
            }))
            .into_response()
        }
        (Some("s1"), Some("p2")) => Json(json!({
            "items": [{"id": "e3", "summary": "종일 행사", "start": {"date": "2026-12-01"}, "updated": "u3"}],
            "nextSyncToken": "s2"
        }))
        .into_response(),
        other => panic!("예상하지 못한 요청: {other:?}"),
    }
}

async fn profile() -> Json<Value> {
    Json(json!({"emailAddress": "me@example.com", "historyId": "100"}))
}

async fn history(Query(q): Query<HashMap<String, String>>) -> Response {
    assert_eq!(
        q.get("historyTypes").map(String::as_str),
        Some("messageAdded")
    );
    assert_eq!(q.get("labelId").map(String::as_str), Some("INBOX"));
    match q.get("startHistoryId").map(String::as_str) {
        Some("1") => StatusCode::NOT_FOUND.into_response(),
        Some("100") => Json(json!({
            "history": [
                {"id": "101", "messagesAdded": [{"message": {"id": "m1"}}]},
                {"id": "102", "messagesAdded": [{"message": {"id": "m2"}}, {"message": {"id": "m1"}}]}
            ],
            "historyId": "105"
        }))
        .into_response(),
        Some("105") => Json(json!({"historyId": "105"})).into_response(),
        other => panic!("예상하지 못한 startHistoryId: {other:?}"),
    }
}

async fn message(Path(id): Path<String>, Query(q): Query<Vec<(String, String)>>) -> Response {
    assert!(q.contains(&("format".into(), "METADATA".into())));
    assert!(q.contains(&("metadataHeaders".into(), "Subject".into())));
    match id.as_str() {
        "m1" => Json(json!({"payload": {"headers": [
            {"name": "From", "value": "학과 사무실 <office@univ.example>"},
            {"name": "Subject", "value": "기말시험 일정 변경 안내"}
        ]}}))
        .into_response(),
        // 그사이 지워진 메일.
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

static SERVER: Mutex<Option<String>> = Mutex::new(None);

async fn server() -> String {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let app = Router::new()
        .route("/calendar/calendars/primary/events", get(events))
        .route("/gmail/users/me/profile", get(profile))
        .route("/gmail/users/me/history", get(history))
        .route("/gmail/users/me/messages/{id}", get(message));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = format!("http://{addr}");
    *SERVER.lock().unwrap() = Some(base.clone());
    base
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

#[tokio::test]
async fn calendar_baseline_then_paged_changes_and_410() {
    let base = server().await;
    let tokens = Arc::new(FakeTokens::default());
    let mut cal = GoogleCalendar::new(client(), tokens.clone(), &format!("{base}/calendar"));

    let first = cal.check(None).await.unwrap();
    assert_eq!(first.cursor.as_deref(), Some("s1"));
    assert!(first.changes.is_empty(), "첫 동기화는 기준선만");
    assert_eq!(
        tokens.invalidated.load(Ordering::SeqCst),
        1,
        "401 뒤 한 번 새 토큰으로"
    );

    let next = cal.check(Some("s1")).await.unwrap();
    assert_eq!(
        next.cursor.as_deref(),
        Some("s2"),
        "두 페이지를 다 읽은 뒤의 토큰"
    );
    let summaries: Vec<&str> = next.changes.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(
        summaries,
        [
            "일정 추가·변경: 기말시험 (시작 2026-12-09T09:00:00+09:00)",
            "일정 취소: (제목 없음)",
            "일정 추가·변경: 종일 행사 (시작 2026-12-01)",
        ]
    );
    assert_eq!(next.changes[0].key, "e1@u1", "버전(updated)을 포함한 키");

    assert!(matches!(
        cal.check(Some("expired")).await,
        Err(SourceError::CursorExpired(_))
    ));
}

#[tokio::test]
async fn gmail_baseline_new_mail_dedupe_and_404() {
    let base = server().await;
    let tokens = Arc::new(FakeTokens::default());
    let mut gmail = Gmail::new(client(), tokens, &format!("{base}/gmail"));

    assert_eq!(gmail.account_email().await.unwrap(), "me@example.com");
    let first = gmail.check(None).await.unwrap();
    assert_eq!(first.cursor.as_deref(), Some("100"));
    assert!(first.changes.is_empty());

    let next = gmail.check(Some("100")).await.unwrap();
    assert_eq!(next.cursor.as_deref(), Some("105"));
    assert_eq!(next.changes.len(), 1, "m1은 한 번만, 지워진 m2는 건너뛴다");
    assert_eq!(
        next.changes[0].summary,
        "새 메일: 학과 사무실 <office@univ.example> — 기말시험 일정 변경 안내"
    );
    assert_eq!(next.changes[0].key, "msg:m1");

    let quiet = gmail.check(Some("105")).await.unwrap();
    assert!(quiet.changes.is_empty());
    assert_eq!(quiet.cursor.as_deref(), Some("105"));

    assert!(matches!(
        gmail.check(Some("1")).await,
        Err(SourceError::CursorExpired(_))
    ));
}
