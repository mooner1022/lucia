//! 가짜 Google 서버로 비서 도구 호출을 시험한다(네트워크 없음).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::TimeZone;

use super::*;

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

#[derive(Default)]
struct Fake {
    sent: Mutex<Vec<Value>>,
    send_status: Mutex<u16>,
}

fn b64(s: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(s)
}

async fn list(Query(q): Query<Vec<(String, String)>>) -> Json<Value> {
    let q: HashMap<_, _> = q.into_iter().collect();
    assert_eq!(q.get("q").map(String::as_str), Some("from:prof"));
    assert_eq!(
        q.get("maxResults").map(String::as_str),
        Some("20"),
        "상한 20"
    );
    Json(
        json!({"messages": [{"id": "m1", "threadId": "t1"}, {"id": "otp", "threadId": "t2"}, {"id": "gone", "threadId": "t3"}]}),
    )
}

async fn message(Path(id): Path<String>, Query(q): Query<Vec<(String, String)>>) -> Response {
    let format = q
        .iter()
        .find(|(k, _)| k == "format")
        .map(|(_, v)| v.as_str());
    match (id.as_str(), format) {
        ("gone", _) => StatusCode::NOT_FOUND.into_response(),
        ("m1", Some("METADATA")) => {
            let wanted: Vec<&str> = q
                .iter()
                .filter(|(k, _)| k == "metadataHeaders")
                .map(|(_, v)| v.as_str())
                .collect();
            assert_eq!(wanted, ["From", "To", "Subject", "Date"]);
            Json(json!({
                "id": "m1", "threadId": "t1", "labelIds": ["UNREAD", "INBOX"],
                "snippet": "과제 마감이 금요일로 바뀌었습니다",
                "payload": {"headers": [
                    {"name": "From", "value": "교수 <prof@example.edu>"},
                    {"name": "To", "value": "me@example.com"},
                    {"name": "Subject", "value": "과제\r\n 공지"},
                    {"name": "Date", "value": "Mon, 16 Nov 2026 09:00:00 +0900"}
                ]}
            }))
            .into_response()
        }
        ("otp", Some("METADATA")) => Json(json!({
            "id": "otp", "threadId": "t2", "labelIds": ["INBOX"],
            "snippet": "Your code is 123456",
            "payload": {"headers": [{"name": "Subject", "value": "Your verification code"}]}
        }))
        .into_response(),
        ("m1", Some("FULL")) => Json(json!({
            "id": "m1", "threadId": "t1",
            "payload": {
                "mimeType": "multipart/mixed",
                "headers": [
                    {"name": "From", "value": "교수 <prof@example.edu>"},
                    {"name": "Subject", "value": "과제 공지"},
                    {"name": "Message-ID", "value": "<abc@example.edu>"},
                    {"name": "References", "value": "<root@example.edu>"}
                ],
                "parts": [
                    {"mimeType": "multipart/alternative", "parts": [
                        {"mimeType": "text/plain", "headers": [{"name": "Content-Type", "value": "text/plain; charset=\"euc-kr\""}],
                         "body": {"data": b64(&encoding_rs::EUC_KR.encode("마감은 금요일 18시입니다.").0)}},
                        {"mimeType": "text/html", "body": {"data": b64("<p>무시될 HTML</p>".as_bytes())}}
                    ]},
                    {"mimeType": "application/pdf", "filename": "공지.pdf", "body": {"attachmentId": "a1", "size": 1234}}
                ]
            }
        }))
        .into_response(),
        ("html", Some("FULL")) => Json(json!({
            "id": "html", "threadId": "t9",
            "payload": {"mimeType": "text/html", "headers": [{"name": "Subject", "value": "소식"}],
                "body": {"data": b64("<html><head><style>p{}</style></head><body><p>안녕&amp;하세요</p><script>x()</script><div>둘째</div></body></html>".as_bytes())}}
        }))
        .into_response(),
        other => panic!("예상하지 못한 요청: {other:?}"),
    }
}

async fn send(
    State(fake): State<std::sync::Arc<Fake>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth != "Bearer t2" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    fake.sent.lock().unwrap().push(body);
    match *fake.send_status.lock().unwrap() {
        200 => {
            Json(json!({"id": "sent-1", "threadId": "t1", "labelIds": ["SENT"]})).into_response()
        }
        code => (
            StatusCode::from_u16(code).unwrap(),
            Json(json!({"error": {"message": "Invalid To header"}})),
        )
            .into_response(),
    }
}

async fn events(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    assert_eq!(q.get("singleEvents").map(String::as_str), Some("true"));
    assert_eq!(q.get("orderBy").map(String::as_str), Some("startTime"));
    assert!(q["timeMin"].ends_with("+00:00") && q["timeMax"].ends_with("+00:00"));
    assert_eq!(q.get("q").map(String::as_str), Some("시험"));
    Json(json!({"items": [
        {"id": "e1", "summary": "기말시험", "start": {"dateTime": "2026-12-09T09:00:00+09:00"}, "end": {"dateTime": "2026-12-09T11:00:00+09:00"}, "location": "공학관", "status": "confirmed", "htmlLink": "https://calendar.google.com/x"},
        {"id": "e2", "start": {"date": "2026-12-10"}, "end": {"date": "2026-12-11"}, "description": "<b>준비물</b> 계산기"}
    ]}))
}

/// 캘린더 쓰기(결정 0033): 초대 메일은 보내지 않는다(sendUpdates=none).
async fn event_create(
    Query(q): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    assert_eq!(q.get("sendUpdates").map(String::as_str), Some("none"));
    let mut out = body.clone();
    out["id"] = json!("new-1");
    Json(out)
}

async fn event_get(Path(id): Path<String>) -> Response {
    match id.as_str() {
        "e1" => Json(json!({"id": "e1", "summary": "기말시험", "organizer": {"self": true}}))
            .into_response(),
        "readonly" => StatusCode::FORBIDDEN.into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn event_patch(
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(q.get("sendUpdates").map(String::as_str), Some("none"));
    if id == "readonly" {
        return StatusCode::FORBIDDEN.into_response();
    }
    Json(json!({"id": id, "summary": body["summary"]})).into_response()
}

async fn event_delete(Path(_id): Path<String>) -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn send_as() -> Json<Value> {
    Json(json!({"sendAs": [
        {"sendAsEmail": "me@gmail.example", "displayName": "", "isPrimary": true, "isDefault": false},
        {"sendAsEmail": "work@example.org", "displayName": "업무", "isDefault": true, "verificationStatus": "accepted", "treatAsAlias": true},
        {"sendAsEmail": "club@example.org", "displayName": "Club \"A\"", "replyToAddress": "club-reply@example.org", "verificationStatus": "accepted"},
        {"sendAsEmail": "new@example.org", "verificationStatus": "pending"}
    ]}))
}

async fn serve() -> (GoogleAssist<FakeTokens>, std::sync::Arc<Fake>) {
    let fake = std::sync::Arc::new(Fake::default());
    *fake.send_status.lock().unwrap() = 200;
    let app = Router::new()
        .route("/gmail/users/me/messages", get(list))
        .route("/gmail/users/me/messages/{id}", get(message))
        .route("/gmail/users/me/messages/send", post(send))
        .route("/gmail/users/me/settings/sendAs", get(send_as))
        .route(
            "/cal/calendars/primary/events",
            get(events).post(event_create),
        )
        .route(
            "/cal/calendars/primary/events/{id}",
            get(event_get).patch(event_patch).delete(event_delete),
        )
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let _ = rustls::crypto::ring::default_provider().install_default();
    let assist = GoogleAssist::new(
        reqwest::Client::new(),
        std::sync::Arc::new(FakeTokens::default()),
        &format!("http://{addr}/gmail"),
        &format!("http://{addr}/cal"),
    );
    (assist, fake)
}

fn mail() -> OutgoingMail {
    OutgoingMail {
        from: "me@example.com".into(),
        from_name: None,
        reply_to: None,
        to: vec!["prof@example.edu".into()],
        cc: vec![],
        subject: "Re: 과제 공지".into(),
        body: "확인했습니다.\n감사합니다.".into(),
        in_reply_to: Some("<abc@example.edu>".into()),
        references: Some("<root@example.edu> <abc@example.edu>".into()),
        thread_id: Some("t1".into()),
    }
}

#[tokio::test]
async fn search_hides_sensitive_and_skips_missing() {
    let (assist, _) = serve().await;
    let found = assist.search_mail("from:prof", 50).await.unwrap();
    assert_eq!(found.len(), 2, "지워진 메일은 건너뛴다");
    assert_eq!(found[0].subject, "과제 공지", "헤더 줄바꿈을 접는다");
    assert!(found[0].unread);
    assert_eq!(
        found[0].snippet.as_deref(),
        Some("과제 마감이 금요일로 바뀌었습니다")
    );
    assert!(
        found[1].sensitive && found[1].snippet.is_none(),
        "인증 코드 미리보기를 숨긴다"
    );
}

#[tokio::test]
async fn read_prefers_plain_text_and_decodes_charset() {
    let (assist, _) = serve().await;
    let m = assist.read_mail("m1", 10).await.unwrap().unwrap();
    assert_eq!(
        m.body.as_deref(),
        Some("마감은 금요일 18"),
        "EUC-KR을 풀고 길이에서 자른다"
    );
    assert!(m.body_truncated);
    assert_eq!(m.message_id.as_deref(), Some("<abc@example.edu>"));
    assert_eq!(
        m.attachments,
        vec!["공지.pdf (application/pdf, 1234 bytes)"]
    );

    let h = assist.read_mail("html", 1000).await.unwrap().unwrap();
    assert_eq!(h.body.as_deref(), Some("안녕&하세요\n\n둘째"));

    assert!(
        assist.read_mail("../etc", 10).await.unwrap().is_none(),
        "ID 형식 검사"
    );
}

#[tokio::test]
async fn events_are_listed_in_order() {
    let (assist, _) = serve().await;
    let from = Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap();
    let to = Utc.with_ymd_and_hms(2026, 12, 31, 0, 0, 0).unwrap();
    let list = assist
        .list_events(from, to, Some("시험"), 10)
        .await
        .unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].summary, "기말시험");
    assert!(!list[0].all_day && list[1].all_day);
    assert_eq!(list[1].summary, "(제목 없음)");
    assert_eq!(list[1].description.as_deref(), Some("준비물 계산기"));
}

#[tokio::test]
async fn send_builds_a_threaded_reply_and_retries_only_on_401() {
    let (assist, fake) = serve().await;
    assert_eq!(assist.send_mail(&mail()).await.unwrap(), "sent-1");
    let sent = fake.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "401 뒤 한 번만 보냈다");
    assert_eq!(sent[0]["threadId"], "t1");
    let raw = String::from_utf8(
        BASE64URL_ANY
            .decode(sent[0]["raw"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(raw.contains("To: prof@example.edu\r\n"));
    assert!(raw.contains("In-Reply-To: <abc@example.edu>\r\n"));
    assert!(raw.contains("Subject: =?UTF-8?B?"));

    *fake.send_status.lock().unwrap() = 400;
    let err = assist.send_mail(&mail()).await.unwrap_err();
    assert!(
        matches!(err, SendError::NotSent(ref m) if m.contains("Invalid To header")),
        "{err}"
    );
    assert_eq!(
        fake.sent.lock().unwrap().len(),
        2,
        "거부는 다시 보내지 않는다"
    );
}

#[test]
fn validation_blocks_header_injection() {
    let mut m = mail();
    m.subject = "안녕\r\nBcc: x@evil.example".into();
    assert!(validate(&m).unwrap_err().contains("줄바꿈"));
    let mut m = mail();
    m.to = vec!["교수 <prof@example.edu>".into()];
    assert!(validate(&m).is_err(), "표시 이름은 받지 않는다");
    let mut m = mail();
    m.to = vec!["a@b.c\r\nBcc: x@y.z".into()];
    assert!(validate(&m).is_err());
    let mut m = mail();
    m.in_reply_to = Some("<a@b>\r\nX: y".into());
    assert!(validate(&m).is_err());
    let mut m = mail();
    m.body = "가".repeat(MAX_SEND_BODY_CHARS + 1);
    assert!(validate(&m).is_err());
    let mut m = mail();
    m.to.clear();
    assert!(validate(&m).is_err());
    let mut m = mail();
    m.to = vec!["prof@exаmple.edu".into()]; // 키릴 문자 а
    assert!(
        validate(&m).is_err(),
        "비슷하게 생긴 비ASCII 도메인은 받지 않는다"
    );
    assert!(validate(&mail()).is_ok());
}

#[test]
fn mime_is_crlf_and_round_trips_body() {
    let date = Utc.with_ymd_and_hms(2026, 11, 16, 9, 0, 0).unwrap();
    let mime = build_mime(&mail(), date).unwrap();
    assert!(!mime.replace("\r\n", "").contains('\n'), "줄 끝은 CRLF만");
    let (head, body) = mime.split_once("\r\n\r\n").unwrap();
    assert!(head.contains("Content-Type: text/plain; charset=\"UTF-8\""));
    assert!(head.lines().all(|l| l.len() <= 998));
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(body.replace("\r\n", ""))
        .unwrap();
    assert_eq!(
        String::from_utf8(decoded).unwrap(),
        "확인했습니다.\r\n감사합니다."
    );
    // 제목: encoded-word를 풀면 원문.
    let subject_line = head
        .split("\r\n")
        .skip_while(|l| !l.starts_with("Subject:"))
        .take_while(|l| l.starts_with("Subject:") || l.starts_with(' '))
        .collect::<Vec<_>>()
        .join("");
    let words: String = subject_line
        .trim_start_matches("Subject: ")
        .split_whitespace()
        .map(|w| {
            let b = w.trim_start_matches("=?UTF-8?B?").trim_end_matches("?=");
            String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b).unwrap()).unwrap()
        })
        .collect();
    assert_eq!(words, "Re: 과제 공지");
}

#[test]
fn sensitive_mail_patterns() {
    assert!(looks_sensitive(&["[네이버] 인증번호 안내"]));
    assert!(looks_sensitive(&["Reset your password"]));
    assert!(looks_sensitive(&["", "Use this magic link to sign in"]));
    assert!(looks_sensitive(&["Your OTP is 1234"]));
    assert!(!looks_sensitive(&["과제 공지", "마감은 금요일"]));
    // 흔한 메일을 숨기지 않는다.
    assert!(!looks_sensitive(&[
        "Reduce your carbon footprint",
        "Topology notes"
    ]));
    assert!(!looks_sensitive(&["일회용 컵 줄이기 캠페인"]));
    assert!(!looks_sensitive(&[
        "비밀번호가 변경되었습니다",
        "Turn on 2-Step Verification"
    ]));
    assert!(!looks_sensitive(&["One-time offer: 50% off"]));
    // 본문 뒤쪽(바닥글)의 낱말은 보지 않는다.
    let long = format!("{}verification code", "가".repeat(2000));
    assert!(!looks_sensitive(&[&long]));
}

#[tokio::test]
async fn aliases_are_listed_and_chosen() {
    let (assist, _) = serve().await;
    let list = assist.send_as_list().await.unwrap();
    assert_eq!(list.len(), 4);
    assert!(list[0].is_primary && list[0].usable && list[0].display_name.is_none());
    assert!(list[1].is_default && list[1].usable);
    assert_eq!(list[2].reply_to.as_deref(), Some("club-reply@example.org"));
    assert!(!list[3].usable, "확인 전 별칭은 쓰지 않는다");

    // 지정: 대소문자 무시, 쓸 수 없거나 없는 주소는 거부(쓸 수 있는 목록을 알려 준다).
    assert_eq!(
        choose_sender(&list, Some("Club@Example.org"), &[])
            .unwrap()
            .email,
        "club@example.org"
    );
    let err = choose_sender(&list, Some("new@example.org"), &[]).unwrap_err();
    assert!(
        err.contains("work@example.org") && !err.contains("new@example.org, "),
        "{err}"
    );
    assert!(choose_sender(&list, Some("evil@example.com"), &[]).is_err());
    // 답장: 원본이 온 별칭.
    let to = vec![
        "Someone <x@y.com>, 클럽 <club@example.org>".to_owned(),
        String::new(),
    ];
    assert_eq!(
        choose_sender(&list, None, &to).unwrap().email,
        "club@example.org"
    );
    // 새 메일: Gmail 기본 주소.
    assert_eq!(
        choose_sender(&list, None, &[]).unwrap().email,
        "work@example.org"
    );
}

#[test]
fn from_header_carries_the_alias_name() {
    let date = Utc.with_ymd_and_hms(2026, 11, 16, 9, 0, 0).unwrap();
    let mut m = mail();
    m.from_name = Some("Club \"A\"".into());
    m.reply_to = Some("club-reply@example.org".into());
    let mime = build_mime(&m, date).unwrap();
    assert!(
        mime.contains("From: \"Club \\\"A\\\"\" <me@example.com>\r\n"),
        "{mime}"
    );
    assert!(mime.contains("Reply-To: club-reply@example.org\r\n"));

    m.from_name = Some("업무".into());
    let mime = build_mime(&m, date).unwrap();
    let from = mime.lines().find(|l| l.starts_with("From:")).unwrap();
    assert!(
        from.starts_with("From: =?UTF-8?B?") && from.ends_with("?= <me@example.com>"),
        "{from}"
    );

    m.from_name = Some("이름\r\nBcc: x@y.z".into());
    assert!(validate(&m).is_err(), "이름에 줄바꿈 금지");
    m.from_name = None;
    m.reply_to = Some("not an address".into());
    assert!(validate(&m).is_err());
}

#[tokio::test]
async fn calendar_writes_send_no_invites_and_map_errors() {
    let (assist, _) = serve().await;
    let created = assist
        .create_event(&json!({"summary": "과제 마감", "start": {"date": "2026-12-05"}, "end": {"date": "2026-12-06"}}))
        .await
        .unwrap();
    assert_eq!(created["id"], "new-1");
    assert_eq!(
        assist.get_event("e1").await.unwrap().unwrap()["summary"],
        "기말시험"
    );
    assert!(assist.get_event("none").await.unwrap().is_none());
    let patched = assist
        .patch_event("e1", &json!({"summary": "기말시험(변경)"}))
        .await
        .unwrap();
    assert_eq!(patched["summary"], "기말시험(변경)");
    assert_eq!(
        assist
            .patch_event("readonly", &json!({"summary": "x"}))
            .await,
        Err(CalendarError::Scope),
        "403은 권한 없음(다시 연결 안내)"
    );
    assert!(assist.delete_event("e1").await.is_ok());
}
