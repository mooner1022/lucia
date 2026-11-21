//! Google Calendar·Gmail 사전 체크(읽기 전용). 공식 문서로 확인한 동작(2026-11-16):
//!
//! - Calendar: `GET /calendars/primary/events`. 첫 동기화에서 받은 `nextSyncToken`으로 이후에는 바뀐 것만 받는다.
//!   `timeMin` 등은 syncToken과 함께 쓸 수 없어 첫 동기화에만 쓴다. 토큰이 무효면 **410** → 기준선을 다시 잡는다.
//!   지운 일정은 `status: "cancelled"`로 온다.
//! - Gmail: `GET /users/me/profile`의 `historyId`가 기준점. `GET /users/me/history?startHistoryId=…&historyTypes=messageAdded`
//!   로 새 메일만 받는다. 기준점이 너무 오래되면 **404** → 기준선을 다시 잡는다. 제목·보낸 사람·라벨·미리보기
//!   (`snippet`)만 `messages.get?format=METADATA&metadataHeaders=…`로 가져온다(본문은 가져오지 않는다).
//!
//! 액세스 토큰은 [`TokenProvider`]가 준다(데몬이 refresh token으로 갱신). 401이 오면 토큰을 버리고 한 번 다시 한다.

use std::future::Future;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Change, CheckOutput, Source, SourceError};

pub mod assist;

pub const CALENDAR_SOURCE: &str = "google-calendar";
pub const GMAIL_SOURCE: &str = "gmail";
pub const CALENDAR_BASE: &str = "https://www.googleapis.com/calendar/v3";
pub const GMAIL_BASE: &str = "https://gmail.googleapis.com/gmail/v1";

/// 한 번 확인에서 제목까지 읽는 새 메일 수. 나머지는 개수만 알린다.
const MAX_MAIL_DETAILS: usize = 20;
/// 신호 처리에 싣는 메일 미리보기 길이(글자).
const SNIPPET_CHARS: usize = 150;
/// 첫 동기화에서 지난 일정을 얼마나 거슬러 볼지(기준선을 잡을 뿐, 변화로 보고하지 않는다).
const BASELINE_DAYS: i64 = 30;

pub trait TokenProvider: Send + Sync + 'static {
    /// 유효한 액세스 토큰. 다시 동의해야 하면 `SourceError::Auth`.
    fn access_token(&self) -> impl Future<Output = Result<String, SourceError>> + Send;
    /// 401을 받았을 때: 가지고 있던 액세스 토큰을 버린다.
    fn invalidate(&self);
}

enum Failure {
    Status(u16),
    Network(String),
}

/// GET하고 JSON을 돌려준다. 401이면 토큰을 새로 받아 한 번 더 한다.
async fn get_json<T: TokenProvider>(
    http: &reqwest::Client,
    tokens: &T,
    url: &str,
    query: &[(&str, String)],
) -> Result<Result<Value, Failure>, SourceError> {
    for attempt in 0..2 {
        let token = tokens.access_token().await?;
        let response = http.get(url).bearer_auth(token).query(query).send().await;
        let response = match response {
            Ok(r) => r,
            Err(err) => return Ok(Err(Failure::Network(err.without_url().to_string()))),
        };
        let status = response.status().as_u16();
        if status == 401 && attempt == 0 {
            tokens.invalidate();
            continue;
        }
        if !(200..300).contains(&status) {
            return Ok(Err(Failure::Status(status)));
        }
        return Ok(response
            .json::<Value>()
            .await
            .map_err(|e| Failure::Network(e.without_url().to_string())));
    }
    Ok(Err(Failure::Status(401)))
}

fn failure_to_error(what: &str, failure: Failure) -> SourceError {
    match failure {
        Failure::Status(401 | 403) => {
            SourceError::Auth(format!("{what}: 권한이 없다(다시 연결 필요)"))
        }
        Failure::Status(code) => SourceError::Transient(format!("{what}: HTTP {code}")),
        Failure::Network(err) => SourceError::Transient(format!("{what}: {err}")),
    }
}

pub struct GoogleCalendar<T> {
    http: reqwest::Client,
    tokens: Arc<T>,
    base: String,
    now: fn() -> DateTime<Utc>,
}

impl<T: TokenProvider> GoogleCalendar<T> {
    pub fn new(http: reqwest::Client, tokens: Arc<T>, base: &str) -> Self {
        Self {
            http,
            tokens,
            base: base.trim_end_matches('/').to_owned(),
            now: Utc::now,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct EventTime {
    date_time: Option<String>,
    date: Option<String>,
}

#[derive(Deserialize)]
struct Event {
    id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    start: Option<EventTime>,
    #[serde(default)]
    updated: Option<String>,
    #[serde(default)]
    creator: Option<Person>,
    #[serde(default)]
    organizer: Option<Person>,
    #[serde(default, rename = "extendedProperties")]
    extended: Option<Value>,
}

#[derive(Deserialize, Default)]
struct Person {
    #[serde(default, rename = "self")]
    is_self: bool,
}

/// Lucia가 만든 일정에 다는 표지(`extendedProperties.private`의 키, 결정 0033). 증분 확인으로 돌아온 자기 변화를
/// 신호 처리가 버리게 한다.
pub const LUCIA_EVENT_MARK: &str = "lucia";

fn event_change(e: Event) -> Change {
    let title = e.summary.clone().unwrap_or_else(|| "(제목 없음)".into());
    let start = e
        .start
        .as_ref()
        .and_then(|s| s.date_time.clone().or_else(|| s.date.clone()))
        .unwrap_or_default();
    let cancelled = e.status.as_deref() == Some("cancelled");
    // 내가 만들었거나 내가 주최한 일정(사람이 직접 바꾼 것)인가. 지운 일정은 이 정보가 오지 않는다.
    let self_made = e.creator.as_ref().is_some_and(|p| p.is_self)
        || e.organizer.as_ref().is_some_and(|p| p.is_self);
    let lucia = e
        .extended
        .as_ref()
        .and_then(|x| x.pointer(&format!("/private/{LUCIA_EVENT_MARK}")))
        .is_some();
    let summary = if cancelled {
        format!("일정 취소: {title}")
    } else {
        format!("일정 추가·변경: {title} (시작 {start})")
    };
    Change {
        key: format!("{}@{}", e.id, e.updated.clone().unwrap_or_default()),
        summary,
        payload: json!({
            "id": e.id,
            "status": e.status,
            "summary": e.summary,
            "start": start,
            "updated": e.updated,
            "self_made": self_made,
            "lucia": lucia,
        }),
    }
}

impl<T: TokenProvider> Source for GoogleCalendar<T> {
    fn name(&self) -> &str {
        CALENDAR_SOURCE
    }

    async fn check(&mut self, cursor: Option<&str>) -> Result<CheckOutput, SourceError> {
        let url = format!("{}/calendars/primary/events", self.base);
        let mut changes = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut query = vec![("maxResults", "2500".to_owned())];
            match cursor {
                Some(sync) => query.push(("syncToken", sync.to_owned())),
                None => query.push((
                    "timeMin",
                    ((self.now)() - chrono::Duration::days(BASELINE_DAYS)).to_rfc3339(),
                )),
            }
            if let Some(p) = &page {
                query.push(("pageToken", p.clone()));
            }
            let body = match get_json(&self.http, self.tokens.as_ref(), &url, &query).await? {
                Ok(body) => body,
                Err(Failure::Status(410)) => {
                    return Err(SourceError::CursorExpired(
                        "캘린더 syncToken이 무효(410)".into(),
                    ));
                }
                Err(f) => return Err(failure_to_error("캘린더", f)),
            };
            if cursor.is_some() {
                let items: Vec<Event> = serde_json::from_value(
                    body.get("items").cloned().unwrap_or(Value::Array(vec![])),
                )
                .map_err(|e| SourceError::Transient(format!("캘린더 응답 형식: {e}")))?;
                changes.extend(items.into_iter().map(event_change));
            }
            if let Some(next) = body.get("nextPageToken").and_then(Value::as_str) {
                page = Some(next.to_owned());
                continue;
            }
            let sync = body
                .get("nextSyncToken")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    SourceError::Transient("캘린더 응답에 nextSyncToken이 없다".into())
                })?;
            return Ok(CheckOutput {
                cursor: Some(sync.to_owned()),
                changes,
            });
        }
    }
}

pub struct Gmail<T> {
    http: reqwest::Client,
    tokens: Arc<T>,
    base: String,
}

impl<T: TokenProvider> Gmail<T> {
    pub fn new(http: reqwest::Client, tokens: Arc<T>, base: &str) -> Self {
        Self {
            http,
            tokens,
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    /// 연결된 계정의 메일 주소(상태 화면용).
    pub async fn account_email(&self) -> Result<String, SourceError> {
        let url = format!("{}/users/me/profile", self.base);
        match get_json(&self.http, self.tokens.as_ref(), &url, &[]).await? {
            Ok(body) => body
                .get("emailAddress")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| SourceError::Transient("profile에 emailAddress가 없다".into())),
            Err(f) => Err(failure_to_error("Gmail profile", f)),
        }
    }

    /// 새 메일 한 통의 머리(보낸 사람, 제목, 짧은 미리보기, 라벨). 본문은 가져오지 않는다.
    /// 신호 처리(결정 0031) 1단계가 라벨(프로모션·소셜)로 거르고, 2단계가 미리보기까지 본다.
    async fn mail_head(&self, id: &str) -> Result<Option<MailHead>, SourceError> {
        let url = format!("{}/users/me/messages/{id}", self.base);
        let query = [
            ("format", "METADATA".to_owned()),
            ("metadataHeaders", "From".to_owned()),
            ("metadataHeaders", "Subject".to_owned()),
        ];
        let body = match get_json(&self.http, self.tokens.as_ref(), &url, &query).await? {
            Ok(body) => body,
            // 그사이 지워진 메일.
            Err(Failure::Status(404)) => return Ok(None),
            Err(f) => return Err(failure_to_error("Gmail 메시지", f)),
        };
        let header = |name: &str| -> String {
            body.pointer("/payload/headers")
                .and_then(Value::as_array)
                .and_then(|hs| {
                    hs.iter().find(|h| {
                        h.get("name")
                            .and_then(Value::as_str)
                            .is_some_and(|n| n.eq_ignore_ascii_case(name))
                    })
                })
                .and_then(|h| h.get("value").and_then(Value::as_str))
                .unwrap_or("")
                .chars()
                .take(200)
                .collect()
        };
        let from = header("From");
        let subject = header("Subject");
        let snippet: String = body
            .get("snippet")
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(SNIPPET_CHARS)
            .collect();
        // 보안 메일(일회용 코드, 재설정 링크)은 미리보기를 싣지 않는다(assist와 같은 판정).
        let snippet = (!snippet.is_empty() && !assist::looks_sensitive(&[&subject, &snippet]))
            .then_some(snippet);
        let labels = body
            .get("labelIds")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Some(MailHead {
            from,
            subject,
            snippet,
            labels,
        }))
    }
}

/// 새 메일의 머리.
struct MailHead {
    from: String,
    subject: String,
    snippet: Option<String>,
    labels: Vec<String>,
}

impl MailHead {
    fn summary(&self) -> String {
        format!(
            "새 메일: {} — {}",
            self.from,
            if self.subject.is_empty() {
                "(제목 없음)"
            } else {
                &self.subject
            }
        )
    }
}

impl<T: TokenProvider> Source for Gmail<T> {
    fn name(&self) -> &str {
        GMAIL_SOURCE
    }

    async fn check(&mut self, cursor: Option<&str>) -> Result<CheckOutput, SourceError> {
        let Some(start) = cursor else {
            let url = format!("{}/users/me/profile", self.base);
            let body = match get_json(&self.http, self.tokens.as_ref(), &url, &[]).await? {
                Ok(body) => body,
                Err(f) => return Err(failure_to_error("Gmail profile", f)),
            };
            let history = history_id(&body)
                .ok_or_else(|| SourceError::Transient("profile에 historyId가 없다".into()))?;
            return Ok(CheckOutput {
                cursor: Some(history),
                changes: Vec::new(),
            });
        };

        let url = format!("{}/users/me/history", self.base);
        let mut ids: Vec<String> = Vec::new();
        let mut latest = start.to_owned();
        let mut page: Option<String> = None;
        loop {
            let mut query = vec![
                ("startHistoryId", start.to_owned()),
                ("historyTypes", "messageAdded".to_owned()),
                ("labelId", "INBOX".to_owned()),
                ("maxResults", "500".to_owned()),
            ];
            if let Some(p) = &page {
                query.push(("pageToken", p.clone()));
            }
            let body = match get_json(&self.http, self.tokens.as_ref(), &url, &query).await? {
                Ok(body) => body,
                Err(Failure::Status(404)) => {
                    return Err(SourceError::CursorExpired(
                        "Gmail historyId가 너무 오래됐다(404)".into(),
                    ));
                }
                Err(f) => return Err(failure_to_error("Gmail history", f)),
            };
            for record in body
                .get("history")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                for added in record
                    .get("messagesAdded")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(id) = added.pointer("/message/id").and_then(Value::as_str)
                        && !ids.iter().any(|x| x == id)
                    {
                        ids.push(id.to_owned());
                    }
                }
            }
            if let Some(h) = history_id(&body) {
                latest = h;
            }
            match body.get("nextPageToken").and_then(Value::as_str) {
                Some(next) => page = Some(next.to_owned()),
                None => break,
            }
        }

        let mut changes = Vec::new();
        for id in ids.iter().take(MAX_MAIL_DETAILS) {
            if let Some(head) = self.mail_head(id).await? {
                changes.push(Change {
                    key: format!("msg:{id}"),
                    summary: head.summary(),
                    payload: json!({
                        "id": id,
                        "from": head.from,
                        "subject": head.subject,
                        "snippet": head.snippet,
                        "labels": head.labels,
                    }),
                });
            }
        }
        if ids.len() > MAX_MAIL_DETAILS {
            changes.push(Change {
                key: format!("more:{start}-{latest}"),
                summary: format!("새 메일 외 {}건", ids.len() - MAX_MAIL_DETAILS),
                payload: json!({ "count": ids.len() - MAX_MAIL_DETAILS }),
            });
        }
        Ok(CheckOutput {
            cursor: Some(latest),
            changes,
        })
    }
}

/// historyId는 문서상 문자열(uint64)이지만 숫자로 올 수도 있게 받는다.
fn history_id(body: &Value) -> Option<String> {
    match body.get("historyId")? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
