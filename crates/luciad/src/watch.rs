//! 감시(결정 0019·0036, 명세 §도구화 사다리 3단계 "주기적으로 반복되는 확인 작업은 데몬의 결정적 모듈로 승격해,
//! 이후에는 모델 없이 돈다"): 사람이 승인한 페이지를 데몬이 모델 없이 주기적으로 확인한다.
//!
//! - 두 방식: 한 구간이 바뀌었는가(region, 0019), 글 목록의 새 글(items, 0036, 규칙은 [`crate::board`]).
//! - 가져오기는 브라우저 프록시와 같은 검사를 쓴다: 차단 목록, 이름을 한 번 풀어 모든 답을 검사하고 그 IPv4
//!   주소로만 연결, 80·443만. 넘겨주기(redirect)는 따라가지 않고 매번 다시 검사한다(3번까지). 크기·시간 상한.
//! - 확인은 작업 큐 밖 전용 타이머가 한다([`due`]: 간격, 흔들림, 실패하면 물러서기. 같은 사이트 간격은 엔진).
//! - 변화는 사전 체크의 "바뀐 것"(소스 `watch:<id>`)으로 남아 브리핑과 `on_change` 루틴이 받는다. 첫 확인은 기준선만.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use lucia_store::Watch;
use sha2::{Digest as _, Sha256};

use crate::browser::BoxFut;
use crate::browser::egress::Egress;
use crate::browser::proxy::Proxy;

/// 소스 이름 앞머리.
pub const SOURCE_PREFIX: &str = "watch:";
/// 이만큼 이어 바뀌면 멈춘다(잡음) [제안].
pub const PAUSE_AFTER: i64 = 3;
/// 구간 감시 간격 하한(분) [제안].
pub const INTERVAL_MIN: i64 = 10;
/// 게시판 감시 간격 하한·기본(분) [제안].
pub const ITEMS_INTERVAL_MIN: i64 = 3;
pub const ITEMS_INTERVAL_DEFAULT: i64 = 10;
pub const INTERVAL_MAX: i64 = 24 * 60;
/// 같은 사이트는 이만큼 사이를 두고 가져온다(분) [제안].
pub const HOST_GAP_MINUTES: i64 = 3;
/// 실패하면 간격을 두 배씩 늘리되 이만큼까지(분, 간격이 더 길면 간격).
pub const BACKOFF_MAX_MINUTES: i64 = 60;
/// 이어 실패하면 한 번 알린다.
pub const FAIL_NOTICE_AT: i64 = 5;
/// 관리 페이지 설정: 감시 하나가 하루에 바로 보내는 새 글 알림 수.
pub const NOW_DAILY_KEY: &str = "watch.now_daily";
pub const NOW_DAILY_DEFAULT: i64 = 20;
pub const NOW_DAILY_RANGE: std::ops::RangeInclusive<i64> = 0..=200;
const BODY_MAX: usize = 2 * 1024 * 1024;
const REDIRECTS_MAX: usize = 3;
const TIMEOUT: Duration = Duration::from_secs(20);
/// 파일 받기(결정 0057): 내려받기 주소는 저장소로 여러 번 넘겨주는 일이 흔하다.
const FILE_REDIRECTS_MAX: usize = 8;
/// 파일 하나를 받는 시간 상한(연결은 [`TIMEOUT`]).
const FILE_TIMEOUT: Duration = Duration::from_secs(600);

/// 가져오기 하나의 상한과 받는 종류.
#[derive(Debug, Clone, Copy)]
struct Limits {
    /// 글(text·xml·json)이 아니어도 받는다.
    any_type: bool,
    body_max: u64,
    redirects: usize,
    timeout: Duration,
}

impl Limits {
    const PAGE: Limits = Limits {
        any_type: false,
        body_max: BODY_MAX as u64,
        redirects: REDIRECTS_MAX,
        timeout: TIMEOUT,
    };

    fn file(body_max: u64) -> Limits {
        Limits {
            any_type: true,
            body_max,
            redirects: FILE_REDIRECTS_MAX,
            timeout: FILE_TIMEOUT,
        }
    }
}

/// 페이지를 받는 방식(결정 0036). 저장 값은 `lucia_store::watch_fetcher`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// 쿠키 없이 HTTP.
    Plain,
    /// 로그인된 브라우저의 쿠키를 빌려 HTTP(2단계).
    Session,
    /// 로그인된 브라우저의 감시 탭에서 읽기(3단계).
    Browser,
}

impl Via {
    pub fn parse(s: &str) -> Self {
        match s {
            lucia_store::watch_fetcher::SESSION => Self::Session,
            lucia_store::watch_fetcher::BROWSER => Self::Browser,
            _ => Self::Plain,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => lucia_store::watch_fetcher::PLAIN,
            Self::Session => lucia_store::watch_fetcher::SESSION,
            Self::Browser => lucia_store::watch_fetcher::BROWSER,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "로그인 없이 HTTP",
            Self::Session => "로그인 세션으로 HTTP",
            Self::Browser => "브라우저로 읽기",
        }
    }
}

/// 가져오기 실패의 종류.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// 로그인이 필요하다(로그인 페이지로 넘겨줌, 401).
    Login(String),
    /// 사이트가 막았다(403, 봇 검사). 브라우저로는 될 수 있다.
    Blocked(String),
    /// 브라우저가 다른 일로 바쁘다(이번 회차는 건너뛴다. 실패로 세지 않는다).
    Busy,
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Login(w) => write!(f, "로그인이 필요하다: {w}"),
            Self::Blocked(w) => write!(f, "사이트가 막았다: {w}"),
            Self::Busy => write!(f, "브라우저가 다른 일로 바쁘다"),
            Self::Other(w) => f.write_str(w),
        }
    }
}

/// 페이지 글을 가져온다(시험은 가짜).
pub trait Fetch: Send + Sync {
    /// 쿠키 없이 가져온다(스크립트의 `lucia.fetch`, 공개 감시).
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<String, String>>;

    /// 방식대로 가져온다. 기본은 쿠키 없는 것만 된다.
    fn fetch_via<'a>(&'a self, url: &'a str, via: Via) -> BoxFut<'a, Result<String, FetchError>> {
        Box::pin(async move {
            match via {
                Via::Plain => self.fetch(url).await.map_err(FetchError::Other),
                _ => Err(FetchError::Other(format!(
                    "{}은(는) 이 데몬에서 쓸 수 없다",
                    via.label()
                ))),
            }
        })
    }

    /// 파일로 받는다(결정 0057): 바이트 그대로, `max` 바이트까지. 방식은 쿠키 없이 또는 로그인 세션.
    fn download<'a>(
        &'a self,
        _url: &'a str,
        _via: Via,
        _max: u64,
    ) -> BoxFut<'a, Result<FetchedFile, FetchError>> {
        Box::pin(async {
            Err(FetchError::Other(
                "이 데몬에서는 파일을 받을 수 없다".into(),
            ))
        })
    }

    /// 로그인이 풀렸을 때: 브라우저에서 세션을 다시 받고, 안 되면 위임된 로그인을 한다. 됐으면 `Ok`(다시 해 본다).
    fn refresh<'a>(&'a self, _url: &'a str) -> BoxFut<'a, Result<(), FetchError>> {
        Box::pin(async { Err(FetchError::Login("세션을 다시 받을 수 없다".into())) })
    }

    /// 브라우저 방식 감시가 있는 동안 브라우저를 계속 켜 둔다.
    fn keep_browser(&self, _on: bool) {}

    /// 브라우저가 지금 떠 있는가(로그인으로 멈춘 감시를 띄우지 않고 다시 해 볼 때).
    fn browser_running(&self) -> bool {
        false
    }
}

/// 받은 응답(바이트 그대로). 페이지 글은 [`decode_body`]로 푼다.
#[derive(Debug, Clone, Default)]
pub struct FetchedFile {
    pub bytes: Vec<u8>,
    /// 응답의 `Content-Type`(소문자).
    pub content_type: String,
    /// 응답의 `Content-Disposition`이 알려 준 이름.
    pub filename: Option<String>,
    /// 넘겨주기 뒤 도착한 주소.
    pub url: String,
    /// 응답이 준 쿠키(`이름=값`).
    pub set_cookies: Vec<(String, String)>,
}

/// `Content-Disposition` 머리에서 파일 이름을 읽는다. `filename*=UTF-8''…`(RFC 5987)을 먼저 보고, 없으면
/// `filename=`. 국내 사이트는 따옴표 안에 퍼센트 인코딩이나 EUC-KR 바이트를 그대로 싣기도 한다.
pub fn disposition_filename(raw: &[u8]) -> Option<String> {
    let text = match std::str::from_utf8(raw) {
        Ok(t) => t.to_owned(),
        Err(_) => encoding_rs::EUC_KR.decode(raw).0.into_owned(),
    };
    // 소문자로 바꿔도 바이트 자리는 그대로다(ASCII만 바뀐다).
    let lower = text.to_ascii_lowercase();
    let named = |n: String| Some(n.trim().to_owned()).filter(|n| !n.is_empty());
    if let Some(at) = lower.find("filename*=") {
        let v = text[at + "filename*=".len()..]
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches('"');
        let mut parts = v.splitn(3, '\'');
        if let (Some(charset), Some(_), Some(enc)) = (parts.next(), parts.next(), parts.next()) {
            let bytes: Vec<u8> = percent_encoding::percent_decode_str(enc).collect();
            let encoding = encoding_rs::Encoding::for_label(charset.trim().as_bytes())
                .unwrap_or(encoding_rs::UTF_8);
            if let Some(name) = named(encoding.decode(&bytes).0.into_owned()) {
                return Some(name);
            }
        }
    }
    let at = lower.find("filename=")?;
    let rest = text[at + "filename=".len()..].trim_start();
    let v = match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next().unwrap_or(""),
        None => rest.split(';').next().unwrap_or("").trim(),
    };
    let decoded = percent_encoding::percent_decode_str(v)
        .decode_utf8()
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| v.to_owned());
    named(decoded)
}

/// 실제 가져오기: 프록시의 검사를 거친 주소로만 연결한다.
pub struct SafeFetch {
    pub egress: Arc<Egress>,
    pub proxy: Arc<Proxy>,
}

/// 로그인 페이지로 보이는 주소(넘겨준 곳).
pub fn looks_like_login_url(url: &url::Url) -> bool {
    let p = url.path().to_ascii_lowercase();
    ["login", "signin", "sign_in", "sign-in", "logon"]
        .iter()
        .any(|k| p.contains(k))
}

/// 로그인 폼이 보이는 페이지(비밀번호 칸).
pub fn looks_logged_out(html: &str) -> bool {
    let l = html.to_ascii_lowercase();
    l.contains("type=\"password\"") || l.contains("type='password'") || l.contains("type=password")
}

impl SafeFetch {
    async fn get(&self, raw: &str) -> Result<String, String> {
        self.request(raw, None, |_| None)
            .await
            .map_err(|e| e.to_string())
    }

    /// 검사한 주소로 GET. `cookie`는 넘겨주기마다 그 주소에 붙일 쿠키 머리(없으면 붙이지 않는다).
    /// 쿠키를 붙인 요청이 위임 밖으로 넘겨지는 것은 `cookie`가 `None`을 돌려주는 것으로 막는다(부르는 쪽).
    pub async fn request(
        &self,
        raw: &str,
        user_agent: Option<&str>,
        cookie: impl Fn(&url::Url) -> Option<String>,
    ) -> Result<String, FetchError> {
        let got = self.request_page(raw, user_agent, cookie).await?;
        Ok(decode_body(&got.bytes, &got.content_type))
    }

    /// [`Self::request`]와 같되 글을 풀지 않고 바이트와 종류를 그대로 준다.
    pub async fn request_page(
        &self,
        raw: &str,
        user_agent: Option<&str>,
        cookie: impl Fn(&url::Url) -> Option<String>,
    ) -> Result<FetchedFile, FetchError> {
        self.request_raw(raw, user_agent, cookie, Limits::PAGE)
            .await
    }

    /// 파일로 받는다(결정 0057): 같은 검사를 거치고, 종류를 가리지 않으며, `max` 바이트까지.
    pub async fn request_file(
        &self,
        raw: &str,
        user_agent: Option<&str>,
        cookie: impl Fn(&url::Url) -> Option<String>,
        max: u64,
    ) -> Result<FetchedFile, FetchError> {
        self.request_raw(raw, user_agent, cookie, Limits::file(max))
            .await
    }

    async fn request_raw(
        &self,
        raw: &str,
        user_agent: Option<&str>,
        cookie: impl Fn(&url::Url) -> Option<String>,
        limits: Limits,
    ) -> Result<FetchedFile, FetchError> {
        let other = FetchError::Other;
        let mut url = self.egress.check_url(raw).map_err(other)?;
        let mut set_cookies = Vec::new();
        for _ in 0..=limits.redirects {
            let host = url
                .host_str()
                .ok_or_else(|| other("호스트가 없다".into()))?
                .to_owned();
            let port = url
                .port_or_known_default()
                .ok_or_else(|| other("포트가 없다".into()))?;
            let addrs: Vec<SocketAddr> = self
                .proxy
                .checked_addrs(&host, port)
                .await
                .map_err(other)?
                .into_iter()
                .map(SocketAddr::V4)
                .collect();
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(TIMEOUT)
                .timeout(limits.timeout)
                .user_agent(user_agent.unwrap_or("Mozilla/5.0 (compatible; lucia-watch/0.1)"))
                .resolve_to_addrs(&host, &addrs)
                .build()
                .map_err(|e| other(e.to_string()))?;
            let mut req = client.get(url.clone());
            if let Some(c) = cookie(&url) {
                req = req.header(reqwest::header::COOKIE, c);
            }
            let mut resp = req.send().await.map_err(|e| other(e.to_string()))?;
            for v in resp.headers().get_all(reqwest::header::SET_COOKIE) {
                if let Some((name, value)) = v
                    .to_str()
                    .ok()
                    .and_then(|v| v.split(';').next())
                    .and_then(|kv| kv.split_once('='))
                {
                    set_cookies.push((name.trim().to_owned(), value.trim().to_owned()));
                }
            }
            if resp.status().is_redirection() {
                let next = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| other("넘겨줄 곳이 없는 넘겨주기".into()))?;
                let joined = url.join(next).map_err(|e| other(e.to_string()))?;
                if looks_like_login_url(&joined) {
                    return Err(FetchError::Login(format!(
                        "로그인 페이지로 넘겨줌({})",
                        joined.path()
                    )));
                }
                url = self.egress.check_url(joined.as_str()).map_err(other)?;
                continue;
            }
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(FetchError::Login("HTTP 401".into()));
            }
            if status == reqwest::StatusCode::FORBIDDEN {
                return Err(FetchError::Blocked(format!("HTTP {status}")));
            }
            // 속도 제한은 막힘이 아니다: 물러서기만 한다(브라우저로 바꾸자고 하지 않는다).
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(other("HTTP 429(속도 제한)".into()));
            }
            if !status.is_success() {
                return Err(other(format!("HTTP {status}")));
            }
            let kind = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !(limits.any_type
                || kind.is_empty()
                || kind.contains("text/")
                || kind.contains("xml")
                || kind.contains("json"))
            {
                return Err(other(format!(
                    "글이 아니다({kind}). 파일은 file_download 도구나 스크립트의 lucia.download(url)로 받는다"
                )));
            }
            let too_big = || {
                other(if limits.any_type {
                    format!(
                        "파일이 너무 크다({}까지)",
                        crate::files::human_size(limits.body_max)
                    )
                } else {
                    "페이지가 너무 크다(2 MB)".into()
                })
            };
            if resp.content_length().is_some_and(|n| n > limits.body_max) {
                return Err(too_big());
            }
            let filename = resp
                .headers()
                .get(reqwest::header::CONTENT_DISPOSITION)
                .and_then(|v| disposition_filename(v.as_bytes()));
            let mut body = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(|e| other(e.to_string()))? {
                body.extend_from_slice(&chunk);
                if body.len() as u64 > limits.body_max {
                    return Err(too_big());
                }
            }
            return Ok(FetchedFile {
                bytes: body,
                content_type: kind,
                filename,
                url: url.to_string(),
                set_cookies,
            });
        }
        Err(other("넘겨주기가 너무 많다".into()))
    }
}

impl Fetch for SafeFetch {
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<String, String>> {
        Box::pin(self.get(url))
    }

    fn download<'a>(
        &'a self,
        url: &'a str,
        via: Via,
        max: u64,
    ) -> BoxFut<'a, Result<FetchedFile, FetchError>> {
        Box::pin(async move {
            match via {
                Via::Plain => self.request_file(url, None, |_| None, max).await,
                _ => Err(FetchError::Other(format!(
                    "{}은(는) 이 데몬에서 쓸 수 없다",
                    via.label()
                ))),
            }
        })
    }
}

/// 응답 글을 문자 집합대로 푼다: 헤더의 `charset`, 없으면 문서 앞쪽의 `<meta charset>`, 그래도 없으면 UTF-8.
/// EUC-KR 게시판의 제목이 깨지면 키워드가 하나도 맞지 않으므로 맞춰 푼다(결정 0036).
pub fn decode_body(body: &[u8], content_type: &str) -> String {
    fn label_after(text: &str) -> Option<String> {
        let at = text.find("charset=")? + "charset=".len();
        let rest = text[at..].trim_start_matches(['"', '\'', ' ']);
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(rest.len());
        Some(rest[..end].to_owned()).filter(|l| !l.is_empty())
    }
    let head = String::from_utf8_lossy(&body[..body.len().min(4096)]).to_ascii_lowercase();
    let label = label_after(content_type).or_else(|| label_after(&head));
    let encoding = label
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(body);
    text.into_owned()
}

/// HTML에서 보이는 글만 뽑고(스크립트·스타일 빼기), 표지 글 사이 구간을 자른다.
pub fn extract(html: &str, start: &str, end: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut text = String::with_capacity(html.len() / 2);
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // <script>·<style>·<noscript>는 닫는 태그까지 통째로 뺀다.
            let mut skipped = false;
            for tag in ["script", "style", "noscript"] {
                if lower[i + 1..].starts_with(tag) {
                    let close = format!("</{tag}");
                    i = lower[i..]
                        .find(&close)
                        .map_or(bytes.len(), |p| i + p + close.len());
                    i = lower[i..].find('>').map_or(bytes.len(), |p| i + p + 1);
                    skipped = true;
                    break;
                }
            }
            if !skipped {
                i = lower[i..].find('>').map_or(bytes.len(), |p| i + p + 1);
            }
            text.push(' ');
            continue;
        }
        let next = html[i..].find('<').map_or(html.len(), |p| i + p);
        text.push_str(&html[i..next]);
        i = next;
    }
    let decoded = text
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let flat = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    let from = if start.trim().is_empty() {
        0
    } else {
        match flat.find(start.trim()) {
            Some(p) => p,
            None => return String::new(),
        }
    };
    let rest = &flat[from..];
    let to = if end.trim().is_empty() {
        rest.len()
    } else {
        rest[start.trim().len().min(rest.len())..]
            .find(end.trim())
            .map_or(rest.len(), |p| p + start.trim().len().min(rest.len()))
    };
    rest[..to].trim().to_owned()
}

pub fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(12)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 방식에 맞는 간격 하한.
pub fn interval_min(items: bool) -> i64 {
    if items {
        ITEMS_INTERVAL_MIN
    } else {
        INTERVAL_MIN
    }
}

/// 감시 하나의 소스 이름.
pub fn source_name(id: &str) -> String {
    format!("{SOURCE_PREFIX}{id}")
}

/// 지금 확인할 때인가: 간격(실패하면 두 배씩, [`BACKOFF_MAX_MINUTES`]까지)에 감시마다 다른 ±10% 흔들림.
/// 타이머가 1분마다 보므로 30초 일찍도 받는다(몇 초 차이로 한 바퀴 건너뛰지 않게).
pub fn due(w: &Watch, checked: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    let Some(checked) = checked else {
        return true;
    };
    let base = w.interval_minutes.max(1);
    let step = if w.failures > 0 {
        let backed = base.saturating_mul(1 << w.failures.min(5));
        backed.min(BACKOFF_MAX_MINUTES.max(base))
    } else {
        base
    };
    let jitter = (w.id.bytes().map(i64::from).sum::<i64>() % 21) - 10;
    let secs = step * 60 * (100 + jitter) / 100;
    now - checked >= chrono::Duration::seconds(secs - 30)
}

/// 바로 알림 하루 상한(관리 페이지).
pub fn now_daily(store: &lucia_store::Store) -> i64 {
    store
        .setting(NOW_DAILY_KEY)
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| NOW_DAILY_RANGE.contains(n))
        .unwrap_or(NOW_DAILY_DEFAULT)
}

/// 주소의 호스트(같은 사이트 간격용).
pub fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default()
}

/// 감시 방식 바꾸기 카드(결정 0036): HTTP가 막힌 감시를 브라우저로. 승인 = 바꿈, 거절·만료 = 그대로.
pub struct WatchSwitchKind;

impl crate::approvals::ApprovalKind for WatchSwitchKind {
    fn kind(&self) -> &'static str {
        crate::engine::watches::KIND_WATCH_SWITCH
    }
    fn label(&self) -> &'static str {
        "감시 방식"
    }
    fn not_done(&self) -> &'static str {
        "그대로 둡니다"
    }
    fn card(
        &self,
        approval: &lucia_store::Approval,
        conversation: lucia_core::ConversationId,
        tz: chrono_tz::Tz,
        state: crate::approvals::CardState<'_>,
    ) -> lucia_core::Card {
        use crate::approvals::{clip_raw, escape_markdown as e};
        let p = &approval.payload;
        let t = |k: &str| p[k].as_str().unwrap_or("").to_owned();
        let (headline, tone, buttons, footer) = crate::approvals::open_or_closed(
            approval,
            tz,
            state,
            format!(
                "🔁 **브라우저로 읽을까요?** {}",
                e(&clip_raw(&t("what"), 60))
            ),
            "브라우저로",
            "그대로",
            "그대로 둡니다",
        );
        lucia_core::Card {
            conversation,
            headline,
            title: "감시 방식".to_owned(),
            fields: vec![
                lucia_core::CardField {
                    name: "주소".to_owned(),
                    value: e(&t("url")),
                    inline: false,
                },
                lucia_core::CardField {
                    name: "지금".to_owned(),
                    value: Via::parse(&t("from")).label().to_owned(),
                    inline: true,
                },
            ],
            body: "HTTP로 가져오면 사이트가 막는데(봇 검사 등), 로그인된 브라우저로는 글이 보입니다. 브라우저로 바꾸면 이 감시가 켜져 있는 동안 브라우저(메모리 약 2 GB)를 계속 켜 둡니다. 바꾸지 않으면 HTTP로 계속 해 보고 실패는 관리 페이지에 보입니다.".to_owned(),
            footer: Some(footer),
            tone,
            buttons,
        }
    }
    fn execute<'a>(
        &'a self,
        approval: &'a lucia_store::Approval,
        ctx: &'a crate::approvals::KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, crate::approvals::Executed> {
        Box::pin(async move {
            let id = approval.payload["id"].as_str().unwrap_or("");
            let result = ctx
                .store()
                .update_watch(
                    id,
                    &lucia_store::WatchEdit {
                        fetcher: Some(lucia_store::watch_fetcher::BROWSER),
                        ..lucia_store::WatchEdit::default()
                    },
                    chrono::Utc::now(),
                )
                .map_err(|e| e.to_string())
                .and_then(|ok| {
                    if ok {
                        Ok("브라우저로 읽게 바꿨습니다(본 글 기록은 그대로).".to_owned())
                    } else {
                        Err("감시가 없습니다".to_owned())
                    }
                });
            crate::approvals::Executed::simple(result, "watch_switch", "watch_failed")
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "watch_failed",
            "감시 방식 바꾸기 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다. 관리 페이지에서 감시를 확인해 주세요.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 결정 0057: 응답이 알려 준 파일 이름을 읽는다(국내 사이트의 여러 모양).
    #[test]
    fn disposition_filenames_are_decoded() {
        let name = |h: &[u8]| disposition_filename(h);
        assert_eq!(
            name(b"attachment; filename=\"report.pdf\"").as_deref(),
            Some("report.pdf")
        );
        assert_eq!(
            name(b"attachment; filename=data.zip; size=3").as_deref(),
            Some("data.zip")
        );
        // RFC 5987이 먼저다.
        assert_eq!(
            name(b"attachment; filename=\"x.zip\"; filename*=UTF-8''%EC%9E%90%EB%A3%8C%20%EB%AA%A8%EC%9D%8C.zip")
                .as_deref(),
            Some("자료 모음.zip")
        );
        // 따옴표 안의 퍼센트 인코딩, UTF-8 바이트 그대로, EUC-KR 바이트 그대로.
        assert_eq!(
            name(b"attachment; filename=\"%EA%B3%BC%EC%A0%9C.hwp\"").as_deref(),
            Some("과제.hwp")
        );
        assert_eq!(
            name("attachment; filename=\"과제 5.pdf\"".as_bytes()).as_deref(),
            Some("과제 5.pdf")
        );
        let (euckr, _, _) = encoding_rs::EUC_KR.encode("attachment; filename=\"안내문.pdf\"");
        assert_eq!(name(&euckr).as_deref(), Some("안내문.pdf"));
        assert_eq!(name(b"inline"), None);
        assert_eq!(name(b"attachment; filename=\"\""), None);
    }

    #[test]
    fn extract_keeps_visible_text_between_markers() {
        let html = "<html><head><style>p{}</style><script>var a='공지사항 가짜';</script></head>\
                    <body><nav>메뉴</nav><h2>공지사항</h2><ul><li>2026-11-17 휴강 &amp; 보강</li></ul>\
                    <footer>저작권</footer></body></html>";
        assert_eq!(
            extract(html, "공지사항", "저작권"),
            "공지사항 2026-11-17 휴강 & 보강"
        );
        assert_eq!(
            extract(html, "", ""),
            "메뉴 공지사항 2026-11-17 휴강 & 보강 저작권"
        );
        assert_eq!(extract(html, "없는 표지", ""), "");
    }

    #[test]
    fn bodies_are_decoded_by_charset() {
        let (euc, _, _) = encoding_rs::EUC_KR.encode("중고 장터");
        assert_eq!(decode_body(&euc, "text/html; charset=EUC-KR"), "중고 장터");
        let mut page = b"<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=euc-kr\"></head>".to_vec();
        page.extend_from_slice(&euc);
        assert!(decode_body(&page, "text/html").ends_with("중고 장터"));
        assert_eq!(decode_body("가나".as_bytes(), ""), "가나");
    }

    #[test]
    fn due_respects_interval_backoff_and_jitter() {
        let now = chrono::Utc::now();
        let mut w = Watch {
            id: "w1".into(),
            url: "https://example.com/n".into(),
            what: "학과 공지".into(),
            start_text: String::new(),
            end_text: String::new(),
            interval_minutes: 10,
            status: lucia_store::WatchStatus::Active,
            changes_in_row: 0,
            run_id: None,
            created_at: now,
            updated_at: now,
            mode: "items".into(),
            item_pattern: String::new(),
            include_words: String::new(),
            exclude_words: String::new(),
            notify: "now".into(),
            fetcher: "plain".into(),
            failures: 0,
            paused_reason: String::new(),
            sent_day: String::new(),
            sent_count: 0,
        };
        let min = chrono::Duration::minutes;
        assert!(due(&w, None, now), "처음은 바로");
        assert!(!due(&w, Some(now - min(8)), now));
        assert!(due(&w, Some(now - min(12)), now));
        w.failures = 2;
        assert!(!due(&w, Some(now - min(12)), now), "실패하면 물러선다");
        assert!(due(&w, Some(now - min(45)), now));
        w.failures = 9;
        assert!(due(&w, Some(now - min(70)), now), "물러서기는 60분까지");
        assert_eq!(host_of("https://Cafe.Example.com/a"), "cafe.example.com");
    }

    /// 실제 네트워크(수동): `cargo test -p luciad real_fetch -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn real_fetch_goes_through_the_checks() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let egress = Arc::new(Egress::new(vec![]));
        let f = SafeFetch {
            proxy: Proxy::system(egress.clone()),
            egress,
        };
        let page = f.get("https://www.rust-lang.org/").await.unwrap();
        assert!(extract(&page, "", "").contains("Rust"));
        assert!(f.get("http://192.168.0.1/").await.is_err());
        assert!(f.get("http://127.0.0.1:8780/").await.is_err());
        // 넘겨주기를 따라가며 매번 검사한다(http → https).
        assert!(f.get("http://rust-lang.org/").await.is_ok());
        // 게시판 감시(결정 0036): 실제 공개 게시판에서 글 링크를 찾는다.
        let list = "https://news.ycombinator.com/newest";
        let page = f.get(list).await.unwrap();
        let base = url::Url::parse(list).unwrap();
        let pattern = crate::board::LinkPattern::parse("/item?id=*", &base).unwrap();
        let items = crate::board::items(&page, &base, &pattern);
        assert!(items.len() >= 20, "{}", items.len());
        assert!(
            items
                .iter()
                .all(|i| !i.key.is_empty() && !i.title.is_empty())
        );
        for i in items.iter().take(3) {
            eprintln!("{} {} {}", i.key, i.title, i.url);
        }
    }

    /// 실제 네트워크(수동, 결정 0057): `cargo test -p luciad real_file -- --ignored --nocapture`.
    /// 저장소로 넘겨주는 압축 파일과 PDF를 받고, 글 가져오기는 여전히 파일을 받지 않는다.
    #[tokio::test]
    #[ignore]
    async fn real_file_download_follows_redirects_and_keeps_the_checks() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let egress = Arc::new(Egress::new(vec![]));
        let f = SafeFetch {
            proxy: Proxy::system(egress.clone()),
            egress,
        };
        let max = 20 * 1024 * 1024;
        // GitHub의 압축 파일 주소는 codeload로 넘겨준다.
        let zip = f
            .request_file(
                "https://github.com/octocat/Hello-World/archive/refs/heads/master.zip",
                None,
                |_| None,
                max,
            )
            .await
            .unwrap();
        eprintln!(
            "zip: {} bytes, {}, {:?}, {}",
            zip.bytes.len(),
            zip.content_type,
            zip.filename,
            zip.url
        );
        assert!(zip.bytes.starts_with(b"PK\x03\x04"));
        assert!(zip.url.contains("codeload.github.com"));
        assert_eq!(zip.filename.as_deref(), Some("Hello-World-master.zip"));
        let pdf = f
            .request_file(
                "https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf",
                None,
                |_| None,
                max,
            )
            .await
            .unwrap();
        eprintln!("pdf: {} bytes, {}", pdf.bytes.len(), pdf.content_type);
        assert!(pdf.bytes.starts_with(b"%PDF-"));
        // 상한을 넘으면 받지 않는다. 내부 주소는 파일로도 받지 않는다.
        let small = f
            .request_file(
                "https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf",
                None,
                |_| None,
                1024,
            )
            .await;
        assert!(
            matches!(&small, Err(FetchError::Other(w)) if w.contains("너무 크다")),
            "{small:?}"
        );
        assert!(
            f.request_file("http://192.168.0.1/a.zip", None, |_| None, max)
                .await
                .is_err()
        );
        assert!(
            f.request_file("http://127.0.0.1:8780/a.zip", None, |_| None, max)
                .await
                .is_err()
        );
        // 글 가져오기는 파일을 받지 않고, 무엇으로 받을지 알려 준다.
        let as_page = f
            .get("https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf")
            .await
            .unwrap_err();
        assert!(
            as_page.contains("글이 아니다") && as_page.contains("lucia.download"),
            "{as_page}"
        );
    }
}
