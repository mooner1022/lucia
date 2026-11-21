//! 비서 도구용 Google 호출(결정 0003): 메일 검색·읽기·발송, 일정 조회.
//!
//! 공식 문서로 확인한 형태(2026-11-16):
//! - `messages.list?q=…&maxResults=…`는 id·threadId만 준다. 상세는 `messages.get`(`format=METADATA`+`metadataHeaders`,
//!   또는 `format=FULL`의 `payload` 트리: `mimeType`, `filename`, `headers`, `body.data`(base64url), `parts`).
//! - `messages.send`: `POST …/users/me/messages/send`, 본문 `{"raw": base64url(RFC 2822), "threadId"?}`. 스레드에
//!   넣으려면 threadId, RFC 2822에 맞는 `In-Reply-To`·`References`, 같은 제목이 필요하다. `gmail.send` 범위로 된다.
//!   base64url 패딩은 문서에 없다(보낼 때는 패딩 없이, 받을 때는 둘 다 받는다).
//! - Calendar `events.list`: `timeMin`·`timeMax`는 시간대가 있는 RFC3339, `orderBy=startTime`은 `singleEvents=true`일 때만.
//!   종일 일정은 `start.date`, 시각 일정은 `start.dateTime`.
//!
//! 일회용 코드·비밀번호 재설정·로그인 링크로 보이는 메일은 본문과 미리보기를 돌려주지 않는다.

use std::sync::Arc;

use base64::Engine as _;
use base64::alphabet::URL_SAFE;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::{Failure, TokenProvider, failure_to_error, get_json};
use crate::SourceError;

/// 메일 본문으로 돌려줄 최대 문자 수.
pub const MAX_BODY_CHARS: usize = 12_000;
/// 보낼 메일 본문의 최대 문자 수(승인 카드에 전부 보여야 하므로).
pub const MAX_SEND_BODY_CHARS: usize = 8_000;
/// 받는 사람(To+Cc) 최대 수.
pub const MAX_RECIPIENTS: usize = 20;

const BASE64URL_ANY: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// 검색 결과 한 건.
#[derive(Debug, Clone, PartialEq)]
pub struct MailSummary {
    pub id: String,
    pub thread_id: String,
    pub date: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    /// 보안 메일이면 `None`.
    pub snippet: Option<String>,
    pub unread: bool,
    pub sensitive: bool,
}

/// 읽은 메일 한 통.
#[derive(Debug, Clone, PartialEq)]
pub struct MailMessage {
    pub id: String,
    pub thread_id: String,
    pub date: String,
    pub from: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    /// `Message-ID` 헤더(답장의 `In-Reply-To`로 쓴다).
    pub message_id: Option<String>,
    pub references: Option<String>,
    /// 보안 메일이면 `None`.
    pub body: Option<String>,
    pub body_truncated: bool,
    pub attachments: Vec<String>,
    pub sensitive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalendarEvent {
    pub id: String,
    pub start: String,
    pub end: String,
    pub all_day: bool,
    pub summary: String,
    pub location: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub link: Option<String>,
}

/// 보낼 메일. [`build_mime`]이 검증한다.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutgoingMail {
    /// 보내는 주소: 연결된 계정이나 그 계정의 "다른 주소로 보내기"(send-as) 별칭.
    pub from: String,
    /// 보내는 이름(별칭에 정한 이름).
    #[serde(default)]
    pub from_name: Option<String>,
    /// 답장 받을 주소(별칭에 정해 둔 경우).
    #[serde(default)]
    pub reply_to: Option<String>,
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    pub subject: String,
    pub body: String,
    /// 답장이면 원본의 `Message-ID`.
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
}

/// 주소 하나(표시 이름 없이 `local@domain`만, ASCII만). 비ASCII를 받지 않는 것은 승인 카드에서 비슷하게 생긴
/// 도메인(예: 키릴 문자 `а`)을 사람이 구별할 수 없기 때문이다.
pub fn valid_address(addr: &str) -> bool {
    if !addr.is_ascii() {
        return false;
    }
    let bad = |c: char| c.is_whitespace() || c.is_control() || "<>,;:\"()[]\\".contains(c);
    let Some((local, domain)) = addr.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && addr.len() <= 254
        && !addr.chars().any(bad)
        && !domain.contains('@')
}

/// 메시지 ID 같은 헤더 값: `<…>` 토큰들만, 줄바꿈 없이.
fn valid_id_list(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 4000
        && v.split_whitespace()
            .all(|t| t.starts_with('<') && t.ends_with('>') && t.len() > 2 && t.is_ascii())
        && !v.contains(['\r', '\n'])
}

/// 보내기 전에 검사한다. 사람에게 보여 줄 거부 이유를 돌려준다.
pub fn validate(mail: &OutgoingMail) -> Result<(), String> {
    if mail.to.is_empty() {
        return Err("받는 사람이 없다".into());
    }
    if mail.to.len() + mail.cc.len() > MAX_RECIPIENTS {
        return Err(format!("받는 사람은 {MAX_RECIPIENTS}명까지다"));
    }
    if let Some(name) = &mail.from_name
        && (name.contains(['\r', '\n']) || name.chars().count() > 100)
    {
        return Err("보내는 이름에 줄바꿈을 넣을 수 없다(100자까지)".into());
    }
    for addr in mail
        .to
        .iter()
        .chain(&mail.cc)
        .chain([&mail.from])
        .chain(mail.reply_to.as_ref())
    {
        if !valid_address(addr) {
            return Err(format!(
                "주소 형식이 아니다: {addr:?}(표시 이름 없이 user@example.com 형식만)"
            ));
        }
    }
    if mail.subject.contains(['\r', '\n']) {
        return Err("제목에 줄바꿈을 넣을 수 없다".into());
    }
    if mail.subject.chars().count() > 250 {
        return Err("제목이 너무 길다(250자까지)".into());
    }
    if mail.body.trim().is_empty() {
        return Err("본문이 비었다".into());
    }
    if mail.body.chars().count() > MAX_SEND_BODY_CHARS {
        return Err(format!("본문이 너무 길다({MAX_SEND_BODY_CHARS}자까지)"));
    }
    for v in [&mail.in_reply_to, &mail.references].into_iter().flatten() {
        if !valid_id_list(v) {
            return Err("답장 헤더 형식이 잘못됐다".into());
        }
    }
    if let Some(t) = &mail.thread_id
        && !(t.chars().all(|c| c.is_ascii_alphanumeric()) && !t.is_empty() && t.len() <= 64)
    {
        return Err("threadId 형식이 잘못됐다".into());
    }
    Ok(())
}

/// RFC 2047 encoded-word로 제목을 싼다(ASCII만이면 그대로).
fn encode_subject(subject: &str) -> String {
    if subject
        .chars()
        .all(|c| c.is_ascii() && !c.is_ascii_control())
        && !subject.contains("=?")
    {
        return subject.to_owned();
    }
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in subject.chars() {
        if chunk.len() + c.len_utf8() > 45 {
            words.push(std::mem::take(&mut chunk));
        }
        chunk.push(c);
    }
    if !chunk.is_empty() {
        words.push(chunk);
    }
    words
        .iter()
        .map(|w| {
            format!(
                "=?UTF-8?B?{}?=",
                base64::engine::general_purpose::STANDARD.encode(w.as_bytes())
            )
        })
        .collect::<Vec<_>>()
        .join("\r\n ")
}

/// `이름 <주소>`. 이름이 ASCII면 따옴표로, 아니면 RFC 2047로 싼다.
fn display_address(name: Option<&str>, email: &str) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        None => email.to_owned(),
        Some(n) if n.chars().all(|c| c.is_ascii() && !c.is_ascii_control()) => {
            format!(
                "\"{}\" <{email}>",
                n.replace('\\', "\\\\").replace('"', "\\\"")
            )
        }
        Some(n) => format!("{} <{email}>", encode_subject(n)),
    }
}

/// 검증한 메일을 RFC 2822 텍스트(CRLF)로 만든다. 본문은 UTF-8 base64.
pub fn build_mime(mail: &OutgoingMail, date: DateTime<Utc>) -> Result<String, String> {
    validate(mail)?;
    let mut out = String::new();
    let mut header = |name: &str, value: &str| {
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push_str("\r\n");
    };
    header(
        "From",
        &display_address(mail.from_name.as_deref(), &mail.from),
    );
    if let Some(r) = &mail.reply_to {
        header("Reply-To", r);
    }
    header("To", &mail.to.join(", "));
    if !mail.cc.is_empty() {
        header("Cc", &mail.cc.join(", "));
    }
    header("Subject", &encode_subject(&mail.subject));
    header("Date", &date.to_rfc2822());
    if let Some(v) = &mail.in_reply_to {
        header("In-Reply-To", v);
    }
    if let Some(v) = &mail.references {
        header("References", v);
    }
    header("MIME-Version", "1.0");
    header("Content-Type", "text/plain; charset=\"UTF-8\"");
    header("Content-Transfer-Encoding", "base64");
    out.push_str("\r\n");
    let body = mail.body.replace("\r\n", "\n").replace('\n', "\r\n");
    let encoded = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
    for line in encoded.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(line).expect("base64는 ASCII"));
        out.push_str("\r\n");
    }
    Ok(out)
}

/// 일회용 코드, 비밀번호 재설정, 로그인 링크를 담은 메일로 보이는가. 제목과 본문 앞부분만 본다.
/// 영문 낱말은 낱말 경계에서만 맞춘다("footprint"가 "otp"로 걸리지 않게). 보안 알림 일반(2단계 인증 안내,
/// 비밀번호가 바뀌었다는 통지)은 비밀을 담지 않으므로 숨기지 않는다.
pub fn looks_sensitive(texts: &[&str]) -> bool {
    const KOREAN: &[&str] = &[
        "인증번호",
        "인증 번호",
        "인증코드",
        "인증 코드",
        "확인 코드",
        "확인코드",
        "보안 코드",
        "보안코드",
        "일회용 비밀번호",
        "일회용 코드",
        "일회용 인증",
        "비밀번호 재설정",
        "비밀번호를 재설정",
        "로그인 링크",
        "로그인 코드",
    ];
    const ENGLISH: &[&str] = &[
        "otp",
        "verification code",
        "security code",
        "one-time code",
        "one-time password",
        "one-time passcode",
        "one time code",
        "one time password",
        "password reset",
        "reset your password",
        "magic link",
        "sign-in link",
        "sign in link",
        "login link",
        "log-in link",
        "login code",
        "sign-in code",
        "sign in code",
    ];
    let word_at = |hay: &str, needle: &str| {
        hay.match_indices(needle).any(|(i, _)| {
            let before = hay[..i].chars().next_back();
            let after = hay[i + needle.len()..].chars().next();
            !before.is_some_and(|c| c.is_ascii_alphanumeric())
                && !after.is_some_and(|c| c.is_ascii_alphanumeric())
        })
    };
    texts.iter().any(|t| {
        let lower: String = t.chars().take(1500).collect::<String>().to_lowercase();
        KOREAN.iter().any(|p| lower.contains(p)) || ENGLISH.iter().any(|p| word_at(&lower, p))
    })
}

fn header(headers: Option<&Value>, name: &str) -> String {
    headers
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
        .to_owned()
}

/// 헤더 값을 표시용으로 줄인다(줄바꿈 없앰, 길이 제한).
fn tidy(v: &str, max: usize) -> String {
    v.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

/// 본문 바이트를 문자열로. UTF-8이면 그대로, 아니면 선언된 문자셋(EUC-KR 등)으로 푼다.
fn decode_text(bytes: &[u8], content_type: &str) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_owned();
    }
    let charset = content_type
        .split(';')
        .filter_map(|p| p.trim().split_once('='))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("charset"))
        .map(|(_, v)| v.trim().trim_matches('"').to_owned());
    match charset
        .as_deref()
        .and_then(|c| encoding_rs::Encoding::for_label(c.as_bytes()))
    {
        Some(enc) => enc.decode(bytes).0.into_owned(),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// 아주 단순한 HTML → 텍스트(태그 제거, 블록 끝은 줄바꿈, 흔한 엔티티).
pub fn html_to_text(html: &str) -> String {
    // 바이트 위치를 원문과 맞추려고 ASCII만 소문자로 바꾼다.
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len() / 2);
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < html.len() {
        if bytes[i] == b'<' {
            let rest = &lower[i..];
            for skip in ["<script", "<style", "<head"] {
                if rest.starts_with(skip) {
                    let close = format!("</{}", &skip[1..]);
                    if let Some(end) = rest.find(&close) {
                        i += end;
                    }
                    break;
                }
            }
            let rest = &lower[i..];
            let Some(end) = rest.find('>') else { break };
            let tag = &rest[1..end];
            let name = tag
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("");
            if matches!(
                name,
                "br" | "p"
                    | "div"
                    | "li"
                    | "tr"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "table"
                    | "ul"
                    | "ol"
            ) {
                out.push('\n');
            }
            i += end + 1;
            continue;
        }
        let next = html[i..].find('<').map_or(html.len(), |n| i + n);
        out.push_str(&html[i..next]);
        i = next;
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let mut lines: Vec<&str> = Vec::new();
    let mut blank = 0;
    for line in decoded.lines() {
        let line = line.trim();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_owned()
}

/// payload 트리에서 본문(텍스트 우선, 없으면 HTML)과 첨부 이름을 꺼낸다.
fn extract(payload: &Value) -> (Option<String>, Vec<String>) {
    let mut plain = None;
    let mut html = None;
    let mut attachments = Vec::new();
    let mut stack = vec![payload];
    while let Some(part) = stack.pop() {
        if let Some(children) = part.get("parts").and_then(Value::as_array) {
            stack.extend(children.iter().rev());
        }
        let mime = part
            .get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let filename = part.get("filename").and_then(Value::as_str).unwrap_or("");
        if !filename.is_empty() {
            let size = part
                .pointer("/body/size")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            attachments.push(format!("{} ({mime}, {size} bytes)", tidy(filename, 200)));
            continue;
        }
        let Some(data) = part.pointer("/body/data").and_then(Value::as_str) else {
            continue;
        };
        let Ok(bytes) = BASE64URL_ANY.decode(data) else {
            continue;
        };
        let content_type = header(part.get("headers"), "Content-Type");
        if mime == "text/plain" && plain.is_none() {
            plain = Some(decode_text(&bytes, &content_type));
        } else if mime == "text/html" && html.is_none() {
            html = Some(html_to_text(&decode_text(&bytes, &content_type)));
        }
    }
    (plain.or(html), attachments)
}

/// "다른 주소로 보내기"(send-as) 주소 하나. 기본 주소(계정 주소)도 들어 있다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendAs {
    pub email: String,
    pub display_name: Option<String>,
    pub reply_to: Option<String>,
    pub is_primary: bool,
    pub is_default: bool,
    /// 지금 보낼 수 있는가: 계정 주소이거나 확인을 마친(`accepted`) 별칭.
    pub usable: bool,
}

/// 보낼 주소를 고른다. `requested`가 있으면 쓸 수 있는 주소 중 그것만, 없으면 답장의 원본이 온 주소
/// (`original_recipients` 중 별칭과 맞는 첫 주소), 그것도 없으면 Gmail 기본 주소, 그다음 계정 주소.
pub fn choose_sender<'a>(
    aliases: &'a [SendAs],
    requested: Option<&str>,
    original_recipients: &[String],
) -> Result<&'a SendAs, String> {
    let usable: Vec<&SendAs> = aliases.iter().filter(|a| a.usable).collect();
    let find = |addr: &str| {
        let addr = addr.trim().trim_start_matches('<').trim_end_matches('>');
        usable
            .iter()
            .copied()
            .find(|a| a.email.eq_ignore_ascii_case(addr))
    };
    if let Some(req) = requested.map(str::trim).filter(|r| !r.is_empty()) {
        return find(req).ok_or_else(|| {
            format!(
                "보낼 수 있는 주소가 아니다: {req}. 쓸 수 있는 주소: {}",
                usable
                    .iter()
                    .map(|a| a.email.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
    }
    for recipient in original_recipients {
        // "이름 <주소>, 주소2" 꼴에서 주소만.
        for part in recipient.split(',') {
            let addr = part
                .rsplit_once('<')
                .map(|(_, rest)| rest.trim_end_matches('>'))
                .unwrap_or(part);
            if let Some(a) = find(addr) {
                return Ok(a);
            }
        }
    }
    usable
        .iter()
        .copied()
        .find(|a| a.is_default)
        .or_else(|| usable.iter().copied().find(|a| a.is_primary))
        .ok_or_else(|| "보낼 수 있는 주소가 없다".into())
}

pub struct GoogleAssist<T> {
    http: reqwest::Client,
    tokens: Arc<T>,
    gmail_base: String,
    calendar_base: String,
}

impl<T: TokenProvider> GoogleAssist<T> {
    pub fn new(
        http: reqwest::Client,
        tokens: Arc<T>,
        gmail_base: &str,
        calendar_base: &str,
    ) -> Self {
        Self {
            http,
            tokens,
            gmail_base: gmail_base.trim_end_matches('/').to_owned(),
            calendar_base: calendar_base.trim_end_matches('/').to_owned(),
        }
    }

    /// Gmail 검색(검색창과 같은 문법). 최근 순.
    pub async fn search_mail(
        &self,
        query: &str,
        max: usize,
    ) -> Result<Vec<MailSummary>, SourceError> {
        let url = format!("{}/users/me/messages", self.gmail_base);
        let q = [
            ("q", query.to_owned()),
            ("maxResults", max.clamp(1, 20).to_string()),
        ];
        let body = match get_json(&self.http, self.tokens.as_ref(), &url, &q).await? {
            Ok(body) => body,
            Err(f) => return Err(failure_to_error("Gmail 검색", f)),
        };
        let ids: Vec<String> = body
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect();
        let mut out = Vec::new();
        for id in ids {
            let url = format!("{}/users/me/messages/{id}", self.gmail_base);
            let q = [
                ("format", "METADATA".to_owned()),
                ("metadataHeaders", "From".to_owned()),
                ("metadataHeaders", "To".to_owned()),
                ("metadataHeaders", "Subject".to_owned()),
                ("metadataHeaders", "Date".to_owned()),
            ];
            let m = match get_json(&self.http, self.tokens.as_ref(), &url, &q).await? {
                Ok(m) => m,
                Err(Failure::Status(404)) => continue,
                Err(f) => return Err(failure_to_error("Gmail 메시지", f)),
            };
            let headers = m.pointer("/payload/headers");
            let subject = tidy(&header(headers, "Subject"), 300);
            let snippet = m
                .get("snippet")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let sensitive = looks_sensitive(&[&subject, &snippet]);
            let labels: Vec<&str> = m
                .get("labelIds")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            out.push(MailSummary {
                id: id.clone(),
                thread_id: m
                    .get("threadId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                date: tidy(&header(headers, "Date"), 80),
                from: tidy(&header(headers, "From"), 300),
                to: tidy(&header(headers, "To"), 500),
                subject,
                snippet: (!sensitive).then(|| tidy(&snippet, 300)),
                unread: labels.contains(&"UNREAD"),
                sensitive,
            });
        }
        Ok(out)
    }

    /// 보낼 수 있는 주소들(계정 주소와 "다른 주소로 보내기" 별칭). `gmail.readonly`로 읽힌다.
    pub async fn send_as_list(&self) -> Result<Vec<SendAs>, SourceError> {
        let url = format!("{}/users/me/settings/sendAs", self.gmail_base);
        let body = match get_json(&self.http, self.tokens.as_ref(), &url, &[]).await? {
            Ok(body) => body,
            Err(f) => return Err(failure_to_error("Gmail 보내는 주소 목록", f)),
        };
        let text = |a: &Value, key: &str| {
            a.get(key)
                .and_then(Value::as_str)
                .map(|v| tidy(v, 300))
                .filter(|v| !v.is_empty())
        };
        Ok(body
            .get("sendAs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| {
                let email = text(a, "sendAsEmail")?;
                let is_primary = a.get("isPrimary").and_then(Value::as_bool) == Some(true);
                let accepted =
                    a.get("verificationStatus").and_then(Value::as_str) == Some("accepted");
                Some(SendAs {
                    usable: (is_primary || accepted) && valid_address(&email),
                    display_name: text(a, "displayName"),
                    reply_to: text(a, "replyToAddress").filter(|r| valid_address(r)),
                    is_default: a.get("isDefault").and_then(Value::as_bool) == Some(true),
                    is_primary,
                    email,
                })
            })
            .collect())
    }

    /// 메일 한 통을 읽는다. 없으면 `None`.
    pub async fn read_mail(
        &self,
        id: &str,
        max_body_chars: usize,
    ) -> Result<Option<MailMessage>, SourceError> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Ok(None);
        }
        let url = format!("{}/users/me/messages/{id}", self.gmail_base);
        let q = [("format", "FULL".to_owned())];
        let m = match get_json(&self.http, self.tokens.as_ref(), &url, &q).await? {
            Ok(m) => m,
            Err(Failure::Status(404 | 400)) => return Ok(None),
            Err(f) => return Err(failure_to_error("Gmail 메시지", f)),
        };
        let payload = m.get("payload").cloned().unwrap_or(Value::Null);
        let headers = payload.get("headers");
        let subject = tidy(&header(headers, "Subject"), 300);
        let (text, attachments) = extract(&payload);
        let text = text.unwrap_or_default();
        let sensitive = looks_sensitive(&[&subject, &text]);
        let truncated = text.chars().count() > max_body_chars;
        let body = (!sensitive).then(|| text.chars().take(max_body_chars).collect::<String>());
        let opt = |name: &str| Some(header(headers, name)).filter(|v| !v.is_empty());
        Ok(Some(MailMessage {
            id: id.to_owned(),
            thread_id: m
                .get("threadId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            date: tidy(&header(headers, "Date"), 80),
            from: tidy(&header(headers, "From"), 300),
            to: tidy(&header(headers, "To"), 1000),
            cc: tidy(&header(headers, "Cc"), 1000),
            subject,
            message_id: opt("Message-ID").map(|v| tidy(&v, 1000)),
            references: opt("References").map(|v| tidy(&v, 3000)),
            body,
            body_truncated: truncated && !sensitive,
            attachments,
            sensitive,
        }))
    }

    /// 기본 캘린더의 일정(반복 일정은 하나씩 펼쳐서, 시작 순).
    pub async fn list_events(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        query: Option<&str>,
        max: usize,
    ) -> Result<Vec<CalendarEvent>, SourceError> {
        let url = format!("{}/calendars/primary/events", self.calendar_base);
        let mut q = vec![
            ("timeMin", from.to_rfc3339()),
            ("timeMax", to.to_rfc3339()),
            ("singleEvents", "true".to_owned()),
            ("orderBy", "startTime".to_owned()),
            ("maxResults", max.clamp(1, 100).to_string()),
        ];
        if let Some(text) = query.filter(|t| !t.trim().is_empty()) {
            q.push(("q", text.to_owned()));
        }
        let body = match get_json(&self.http, self.tokens.as_ref(), &url, &q).await? {
            Ok(body) => body,
            Err(f) => return Err(failure_to_error("캘린더", f)),
        };
        let when = |e: &Value, key: &str| -> (String, bool) {
            match (
                e.pointer(&format!("/{key}/dateTime"))
                    .and_then(Value::as_str),
                e.pointer(&format!("/{key}/date")).and_then(Value::as_str),
            ) {
                (Some(t), _) => (t.to_owned(), false),
                (None, Some(d)) => (d.to_owned(), true),
                _ => (String::new(), false),
            }
        };
        let text = |e: &Value, key: &str, max: usize| {
            e.get(key)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(|v| v.chars().take(max).collect::<String>())
        };
        Ok(body
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|e| {
                let (start, all_day) = when(e, "start");
                let (end, _) = when(e, "end");
                CalendarEvent {
                    id: text(e, "id", 200).unwrap_or_default(),
                    start,
                    end,
                    all_day,
                    summary: text(e, "summary", 300).unwrap_or_else(|| "(제목 없음)".into()),
                    location: text(e, "location", 300),
                    description: text(e, "description", 1000).map(|d| html_to_text(&d)),
                    status: text(e, "status", 20),
                    link: text(e, "htmlLink", 500),
                }
            })
            .collect())
    }

    /// 일정 하나(원본 JSON). 없으면 `None`.
    pub async fn get_event(&self, id: &str) -> Result<Option<Value>, CalendarError> {
        let url = format!(
            "{}/calendars/primary/events/{}",
            self.calendar_base,
            urlencode(id)
        );
        match self.calendar_call(reqwest::Method::GET, &url, None).await {
            Ok(v) => Ok(Some(v)),
            Err(CalendarError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// 일정을 만든다(초대 메일은 보내지 않는다: `sendUpdates=none`). 만든 일정의 원본 JSON.
    pub async fn create_event(&self, body: &Value) -> Result<Value, CalendarError> {
        let url = format!(
            "{}/calendars/primary/events?sendUpdates=none",
            self.calendar_base
        );
        self.calendar_call(reqwest::Method::POST, &url, Some(body))
            .await
    }

    /// 일정의 칸을 고친다(주어진 칸만, 초대 메일 없음).
    pub async fn patch_event(&self, id: &str, body: &Value) -> Result<Value, CalendarError> {
        let url = format!(
            "{}/calendars/primary/events/{}?sendUpdates=none",
            self.calendar_base,
            urlencode(id)
        );
        self.calendar_call(reqwest::Method::PATCH, &url, Some(body))
            .await
    }

    /// 일정을 지운다(초대 메일 없음).
    pub async fn delete_event(&self, id: &str) -> Result<(), CalendarError> {
        let url = format!(
            "{}/calendars/primary/events/{}?sendUpdates=none",
            self.calendar_base,
            urlencode(id)
        );
        self.calendar_call(reqwest::Method::DELETE, &url, None)
            .await
            .map(|_| ())
    }

    /// 캘린더 쓰기 요청 하나. 401은 토큰을 새로 받아 한 번 더 한다. 403은 동의 범위가 없을 때가 많다.
    async fn calendar_call(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<Value, CalendarError> {
        for attempt in 0..2 {
            let token = self
                .tokens
                .access_token()
                .await
                .map_err(|e| CalendarError::Failed(e.to_string()))?;
            let mut req = self.http.request(method.clone(), url).bearer_auth(token);
            if let Some(b) = body {
                req = req.json(b);
            }
            let response = req
                .send()
                .await
                .map_err(|e| CalendarError::Failed(e.without_url().to_string()))?;
            let status = response.status().as_u16();
            if status == 401 && attempt == 0 {
                self.tokens.invalidate();
                continue;
            }
            let text = response.text().await.unwrap_or_default();
            return match status {
                200..=299 => Ok(serde_json::from_str(&text).unwrap_or(Value::Null)),
                403 => Err(CalendarError::Scope),
                404 | 410 => Err(CalendarError::NotFound),
                _ => {
                    let reason = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|v| {
                            v.pointer("/error/message")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .unwrap_or_default();
                    Err(CalendarError::Failed(format!(
                        "캘린더가 거부했다(HTTP {status}) {}",
                        tidy(&reason, 300)
                    )))
                }
            };
        }
        Err(CalendarError::Scope)
    }

    /// 메일을 보낸다. 성공하면 Gmail 메시지 ID. 401(토큰 만료)만 한 번 다시 하고, 그 밖의 실패는 다시 하지 않는다
    /// (네트워크 오류는 보냈는지 모르는 상태라 다시 보내면 두 번 갈 수 있다).
    pub async fn send_mail(&self, mail: &OutgoingMail) -> Result<String, SendError> {
        let mime = build_mime(mail, Utc::now()).map_err(SendError::Invalid)?;
        let mut request = json!({ "raw": URL_SAFE_NO_PAD.encode(mime.as_bytes()) });
        if let Some(thread) = &mail.thread_id {
            request["threadId"] = Value::String(thread.clone());
        }
        let url = format!("{}/users/me/messages/send", self.gmail_base);
        for attempt in 0..2 {
            let token = self
                .tokens
                .access_token()
                .await
                .map_err(|e| SendError::NotSent(e.to_string()))?;
            let response = self
                .http
                .post(&url)
                .bearer_auth(token)
                .json(&request)
                .send()
                .await;
            let response = match response {
                Ok(r) => r,
                Err(err) if err.is_connect() => {
                    return Err(SendError::NotSent(format!(
                        "연결하지 못했다: {}",
                        err.without_url()
                    )));
                }
                Err(err) => {
                    return Err(SendError::Unknown(err.without_url().to_string()));
                }
            };
            let status = response.status().as_u16();
            if status == 401 && attempt == 0 {
                self.tokens.invalidate();
                continue;
            }
            let body = response.text().await.unwrap_or_default();
            if !(200..300).contains(&status) {
                let reason = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| {
                        v.pointer("/error/message")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_default();
                return Err(SendError::NotSent(format!(
                    "Gmail이 거부했다(HTTP {status}) {}",
                    tidy(&reason, 300)
                )));
            }
            return serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_owned))
                .ok_or_else(|| SendError::Unknown("응답에 메시지 ID가 없다".into()));
        }
        Err(SendError::NotSent(
            "Gmail이 인증을 거부했다(다시 연결 필요)".into(),
        ))
    }
}

/// 캘린더 쓰기 실패(결정 0033).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CalendarError {
    /// 동의 범위(캘린더 쓰기)가 없거나 그 일정을 고칠 권한이 없다.
    #[error(
        "캘린더 쓰기 권한이 없다(관리 페이지에서 Google을 다시 연결해 캘린더 쓰기를 허용해야 한다)"
    )]
    Scope,
    #[error("그 일정이 없다")]
    NotFound,
    #[error("{0}")]
    Failed(String),
}

fn urlencode(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '@' | '.'))
        .collect()
}

/// 보내기 실패. 보내지 않았음이 확실한 것과 결과를 모르는 것을 나눈다.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SendError {
    #[error("보낼 수 없는 내용: {0}")]
    Invalid(String),
    #[error("보내지 못했다: {0}")]
    NotSent(String),
    #[error("보냈는지 알 수 없다: {0}")]
    Unknown(String),
}

#[cfg(test)]
mod tests;
