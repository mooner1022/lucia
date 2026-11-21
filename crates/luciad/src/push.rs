//! Firebase Cloud Messaging(FCM) HTTP v1 발송. 공식 문서로 확인한 것(2026-11-19):
//!
//! - 발송: `POST https://fcm.googleapis.com/v1/projects/{project_id}/messages:send`, 본문 `{"message": {...}}`.
//!   범위는 `https://www.googleapis.com/auth/firebase.messaging`(또는 cloud-platform). 성공 응답은 Message이고
//!   `name`이 `projects/*/messages/{message_id}` 형식이다.
//!   <https://firebase.google.com/docs/reference/fcm/rest/v1/projects.messages/send>
//! - 액세스 토큰(서비스 계정 JWT bearer): 헤더 `{"alg":"RS256","typ":"JWT"}`, 클레임 iss(서비스 계정 이메일),
//!   scope(공백 구분), aud(토큰 URI, `https://oauth2.googleapis.com/token`), iat, exp(iat 뒤 최대 1시간).
//!   각 부분은 base64url, 서명은 SHA256withRSA(RSASSA-PKCS1-v1_5). `token_uri`에 form으로
//!   `grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer`, `assertion=<JWT>`. 응답 `access_token`,
//!   `expires_in`(초), 그 시간 안에는 다시 써도 된다.
//!   <https://developers.google.com/identity/protocols/oauth2/service-account>
//! - 메시지: `token`(등록 토큰. 레퍼런스에는 `fid`를 쓰라는 deprecated 표시가 있으나 클라이언트 SDK가 주는 값이
//!   등록 토큰이라 이것을 쓴다), `notification{title,body}`, `data`(문자열→문자열, 예약어 `from`,
//!   `message_type`, `google.`·`gcm.notification.`로 시작하는 키 금지), `android{priority, collapse_key,
//!   notification{tag}}`(필드 이름은 snake_case, priority는 "normal"/"high"), `apns{headers, payload{aps}}`.
//!   <https://firebase.google.com/docs/reference/fcm/rest/v1/projects.messages>
//! - 우선순위: Apple 기기로 가는 데이터 메시지는 apns-priority가 반드시 5여야 하고, 높게 보내면 FCM이
//!   INVALID_ARGUMENT로 거부한다. 그래서 알림이 없는(데이터만) 메시지는 high여도 apns-priority 5로 보낸다.
//!   조용한 전달은 `aps.content-available`로 한다.
//!   <https://firebase.google.com/docs/cloud-messaging/customize-messages/setting-message-priority>
//!   <https://firebase.google.com/docs/cloud-messaging/ios/receive-messages>
//! - 접기: Android는 AndroidConfig의 collapse key, APNs는 `apns-collapse-id` 헤더. 알림 메시지는 늘 접히며
//!   collapse key를 무시하므로 알림 서랍에서 바꿔 끼우는 것은 `android.notification.tag`가 한다.
//!   <https://firebase.google.com/docs/cloud-messaging/customize-messages/collapsible-message-types>
//! - 오류(`error.details[].errorCode`): UNREGISTERED(404, 토큰을 지우고 다시 보내지 않는다),
//!   INVALID_ARGUMENT(400, 요청이 잘못됨. 본문이 확실히 맞을 때만 토큰 문제로 본다), SENDER_ID_MISMATCH(403),
//!   THIRD_PARTY_AUTH_ERROR(401, APNs 인증서·웹 푸시 키 문제), THIRD_PARTY_PERMISSION_DENIED(403),
//!   QUOTA_EXCEEDED(429), UNAVAILABLE(503, Retry-After를 따른다), INTERNAL(500). 재시도는 지수 백오프.
//!   <https://firebase.google.com/docs/reference/fcm/rest/v1/ErrorCode>
//!   <https://firebase.google.com/docs/cloud-messaging/manage-tokens>
//!
//! 확인하지 못한 것: 오류 본문 모양은 Google API 공통 형식(`{"error":{"code","message","status","details":[
//! {"@type":"...FcmError","errorCode":...}]}}`)으로 보고 읽는다(문서에 예시 본문이 없다). Apple 문서(헤더
//! 길이 제한 등)는 열리지 않아 apns-collapse-id 길이(64바이트로 알려짐)는 검사하지 않는다.
//!
//! 비밀: 서비스 계정 키는 이 모듈 밖으로 내보내지 않고, 액세스 토큰은 메모리에만 두며 마스킹 필터에 등록한다.
//! 기기 토큰은 오류 글·로그에 앞 6자만 남긴다.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// FCM 발송에 필요한 가장 좁은 범위.
pub const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const FCM_BASE: &str = "https://fcm.googleapis.com";
const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
/// JWT 수명(문서상 최대 1시간).
const ASSERTION_LIFETIME_SECS: i64 = 3600;
/// 남은 시간이 이보다 짧으면 액세스 토큰을 새로 받는다.
const REFRESH_MARGIN: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// 오류 글에 남기는 FCM 메시지 길이(문자).
const MAX_ERROR_CHARS: usize = 300;
/// Retry-After를 이보다 길게 따르지 않는다.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(24 * 3600);

const PEM_BEGIN: &str = "-----BEGIN PRIVATE KEY-----";
const PEM_END: &str = "-----END PRIVATE KEY-----";

/// Firebase 프로젝트의 서비스 계정(키 JSON에서 읽는다).
#[derive(Clone)]
pub struct ServiceAccount {
    project_id: String,
    client_email: String,
    token_uri: String,
    key: Arc<RsaKeyPair>,
}

impl fmt::Debug for ServiceAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceAccount")
            .field("project_id", &self.project_id)
            .field("client_email", &self.client_email)
            .field("token_uri", &self.token_uri)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct RawAccount {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    client_email: Option<String>,
    #[serde(default)]
    private_key: Option<String>,
    #[serde(default)]
    token_uri: Option<String>,
}

impl ServiceAccount {
    /// 서비스 계정 JSON(type == "service_account")을 읽는다. 오류 글에 키 내용을 넣지 않는다.
    pub fn parse(json: &str) -> Result<Self, String> {
        // serde·base64·ring 오류는 글에 넣지 않는다(입력 일부가 섞일 수 있다). 정해 둔 글만 쓴다.
        let raw: RawAccount = serde_json::from_str(json.trim()).map_err(|_| {
            "JSON이 아니다(Firebase 서비스 계정 키 파일 내용 전체를 붙여 넣으세요)".to_owned()
        })?;
        if raw.kind.as_deref() != Some("service_account") {
            return Err("서비스 계정 키 JSON이 아니다(type이 service_account가 아니다)".into());
        }
        let project_id = nonempty(raw.project_id).ok_or("project_id가 없다")?;
        if !project_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | ':'))
        {
            return Err("project_id 형식이 Google Cloud 프로젝트 ID가 아니다".into());
        }
        let client_email = nonempty(raw.client_email).ok_or("client_email이 없다")?;
        if !client_email.contains('@') {
            return Err("client_email 형식이 이메일이 아니다".into());
        }
        let token_uri = nonempty(raw.token_uri).unwrap_or_else(|| DEFAULT_TOKEN_URI.to_owned());
        if !token_uri.starts_with("https://") {
            return Err("token_uri는 https여야 한다".into());
        }
        let pem = nonempty(raw.private_key).ok_or("private_key가 없다")?;
        let der = pem_to_der(&pem)
            .ok_or("private_key를 읽을 수 없다(PKCS#8 PEM, BEGIN PRIVATE KEY 형식이어야 한다)")?;
        let key = RsaKeyPair::from_pkcs8(&der)
            .map_err(|_| "private_key가 쓸 수 있는 RSA 키가 아니다".to_owned())?;
        Ok(Self {
            project_id,
            client_email,
            token_uri,
            key: Arc::new(key),
        })
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    #[allow(dead_code)]
    pub fn client_email(&self) -> &str {
        &self.client_email
    }

    /// 토큰 요청에 쓸 서명된 JWT(`now`는 유닉스 초).
    fn assertion(&self, rng: &SystemRandom, now: i64) -> Result<String, ()> {
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = json!({"alg": "RS256", "typ": "JWT"});
        let claims = json!({
            "iss": self.client_email,
            "scope": SCOPE,
            "aud": self.token_uri,
            "iat": now,
            "exp": now + ASSERTION_LIFETIME_SECS,
        });
        let signing_input = format!(
            "{}.{}",
            b64.encode(header.to_string()),
            b64.encode(claims.to_string())
        );
        let mut signature = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                rng,
                signing_input.as_bytes(),
                &mut signature,
            )
            .map_err(|_| ())?;
        Ok(format!("{signing_input}.{}", b64.encode(signature)))
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// PKCS#8 PEM 본문을 직접 푼다(머리·꼬리 줄 사이의 base64, 줄바꿈·공백은 버린다).
fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    let start = pem.find(PEM_BEGIN)? + PEM_BEGIN.len();
    let end = start + pem[start..].find(PEM_END)?;
    let body: String = pem[start..end]
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    if body.is_empty() {
        return None;
    }
    base64::engine::general_purpose::STANDARD
        .decode(body.as_bytes())
        .ok()
}

/// 기기 토큰을 로그·오류 글에 남길 때의 모양(앞 6자).
pub fn mask_device_token(token: &str) -> String {
    let head: String = token.chars().take(6).collect();
    format!("{head}…")
}

/// 보낼 메시지 한 건.
pub struct PushMessage {
    /// FCM 등록 토큰(기기). 로그에 남기지 않는다.
    pub token: String,
    pub title: Option<String>,
    pub body: Option<String>,
    /// 앱에 넘길 값(문자열→문자열).
    pub data: BTreeMap<String, String>,
    pub high_priority: bool,
    /// 같은 값의 이전 알림을 바꿔 끼운다.
    pub collapse: Option<String>,
}

impl fmt::Debug for PushMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PushMessage")
            .field("token", &mask_device_token(&self.token))
            .field("title", &self.title)
            .field("body", &self.body)
            .field("data_keys", &self.data.keys().collect::<Vec<_>>())
            .field("high_priority", &self.high_priority)
            .field("collapse", &self.collapse)
            .finish()
    }
}

impl PushMessage {
    fn has_notification(&self) -> bool {
        self.title.is_some() || self.body.is_some()
    }

    /// FCM 예약어 키(문서: `from`, `message_type`, `google.`·`gcm.notification.`로 시작).
    fn reserved_data_key(&self) -> Option<&str> {
        self.data
            .keys()
            .find(|k| {
                matches!(k.as_str(), "from" | "message_type")
                    || k.starts_with("google.")
                    || k.starts_with("gcm.notification.")
            })
            .map(String::as_str)
    }

    /// `message` 객체(본문의 `{"message": ...}` 안쪽).
    fn to_json(&self) -> Value {
        let notify = self.has_notification();
        let mut message = Map::new();
        message.insert("token".into(), json!(self.token));
        if notify {
            let mut notification = Map::new();
            if let Some(title) = &self.title {
                notification.insert("title".into(), json!(title));
            }
            if let Some(body) = &self.body {
                notification.insert("body".into(), json!(body));
            }
            message.insert("notification".into(), Value::Object(notification));
        }
        if !self.data.is_empty() {
            message.insert("data".into(), json!(self.data));
        }

        let mut android = Map::new();
        android.insert(
            "priority".into(),
            json!(if self.high_priority { "high" } else { "normal" }),
        );
        if let Some(collapse) = &self.collapse {
            android.insert("collapse_key".into(), json!(collapse));
            if notify {
                android.insert("notification".into(), json!({"tag": collapse}));
            }
        }
        message.insert("android".into(), Value::Object(android));

        // Apple 기기로 가는 데이터 메시지는 우선순위 5여야 한다(10이면 INVALID_ARGUMENT).
        let apns_priority = if notify && self.high_priority {
            "10"
        } else {
            "5"
        };
        let mut headers = Map::new();
        headers.insert("apns-priority".into(), json!(apns_priority));
        if let Some(collapse) = &self.collapse {
            headers.insert("apns-collapse-id".into(), json!(collapse));
        }
        let mut apns = Map::new();
        apns.insert("headers".into(), Value::Object(headers));
        if !notify {
            // 알림 없이 앱을 깨우는 조용한 전달.
            apns.insert("payload".into(), json!({"aps": {"content-available": 1}}));
        }
        message.insert("apns".into(), Value::Object(apns));
        Value::Object(message)
    }
}

/// 발송 실패. 글에는 키·액세스 토큰·기기 토큰 전체가 들어가지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushError {
    /// 기기 토큰이 더는 유효하지 않다(UNREGISTERED). 토큰을 지운다.
    Unregistered,
    /// 요청이 잘못됐다(INVALID_ARGUMENT 등). 본문이 확실히 맞다면 토큰이 잘못된 것일 수 있다.
    BadRequest(String),
    /// 서비스 계정·권한·APNs 인증 문제. 설정을 고쳐야 한다.
    Auth(String),
    /// 잠시 뒤 다시 보내면 될 수 있다(429, 5xx, 네트워크).
    Transient {
        message: String,
        retry_after: Option<Duration>,
    },
}

impl fmt::Display for PushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PushError::Unregistered => write!(f, "기기 토큰이 등록 해제됐다(UNREGISTERED)"),
            PushError::BadRequest(m) => write!(f, "FCM 요청 거부: {m}"),
            PushError::Auth(m) => write!(f, "FCM 인증 실패: {m}"),
            PushError::Transient {
                message,
                retry_after,
            } => match retry_after {
                Some(after) => write!(f, "FCM 일시 오류({}초 뒤 다시): {message}", after.as_secs()),
                None => write!(f, "FCM 일시 오류: {message}"),
            },
        }
    }
}

impl std::error::Error for PushError {}

fn transient(message: String) -> PushError {
    PushError::Transient {
        message,
        retry_after: None,
    }
}

struct CachedToken {
    value: String,
    expires: Instant,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// FCM HTTP v1 클라이언트. 액세스 토큰을 메모리에 두고 다시 쓴다.
pub struct Fcm {
    http: reqwest::Client,
    account: ServiceAccount,
    fcm_base: String,
    token_url: String,
    rng: SystemRandom,
    /// 갱신 중에는 잠가 둬서 동시에 여러 번 발급받지 않는다.
    cache: tokio::sync::Mutex<Option<CachedToken>>,
}

impl Fcm {
    pub fn new(http: reqwest::Client, account: ServiceAccount) -> Self {
        let token_url = account.token_uri.clone();
        Self {
            http,
            account,
            fcm_base: FCM_BASE.to_owned(),
            token_url,
            rng: SystemRandom::new(),
            cache: tokio::sync::Mutex::new(None),
        }
    }

    /// 시험용: 발송·토큰 주소를 바꾼다. JWT의 aud는 계정의 token_uri 그대로 둔다.
    #[cfg(test)]
    pub fn with_endpoints(mut self, fcm_base: &str, token_url: &str) -> Self {
        self.fcm_base = fcm_base.trim_end_matches('/').to_owned();
        self.token_url = token_url.to_owned();
        self
    }

    pub fn account(&self) -> &ServiceAccount {
        &self.account
    }

    /// 한 건 보낸다. 성공하면 FCM이 붙인 메시지 이름(`projects/*/messages/*`)을 돌려준다.
    /// 다시 보내기(백오프)는 부르는 쪽이 `PushError::Transient`를 보고 정한다.
    pub async fn send(&self, msg: &PushMessage) -> Result<String, PushError> {
        if let Some(key) = msg.reserved_data_key() {
            return Err(PushError::BadRequest(format!(
                "data에 FCM 예약어 키를 쓸 수 없다: {key}"
            )));
        }
        let url = format!(
            "{}/v1/projects/{}/messages:send",
            self.fcm_base, self.account.project_id
        );
        let body = json!({ "message": msg.to_json() });
        let mut retried = false;
        loop {
            let token = self.access_token().await?;
            let response = self
                .http
                .post(&url)
                .bearer_auth(&token)
                .json(&body)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
                .map_err(|e| transient(format!("FCM 요청 실패: {}", e.without_url())))?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_retry_after);
            let text = response.text().await.unwrap_or_default();
            if (200..300).contains(&status) {
                let name = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_owned));
                return Ok(name.unwrap_or_else(|| {
                    // 보내졌으므로 실패로 돌리지 않는다(다시 보내면 겹친다).
                    tracing::warn!("FCM 성공 응답에 메시지 이름이 없다");
                    String::new()
                }));
            }
            let error = FcmErrorBody::parse(&text);
            if status == 401 && error.code.as_deref() != Some("THIRD_PARTY_AUTH_ERROR") && !retried
            {
                // 액세스 토큰이 만료·철회됐을 수 있다: 한 번만 새로 받아 다시 보낸다.
                tracing::info!("FCM이 액세스 토큰을 거부했다. 새로 받아 한 번 다시 보낸다");
                self.invalidate(&token).await;
                retried = true;
                continue;
            }
            return Err(classify(status, &error, retry_after, &msg.token));
        }
    }

    async fn access_token(&self) -> Result<String, PushError> {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref()
            && cached.expires > Instant::now() + REFRESH_MARGIN
        {
            return Ok(cached.value.clone());
        }
        let fresh = self.fetch_token().await?;
        let value = fresh.value.clone();
        *cache = Some(fresh);
        Ok(value)
    }

    /// 쓴 토큰이 아직 캐시에 있을 때만 지운다(그 사이 다른 발송이 새로 받았을 수 있다).
    async fn invalidate(&self, used: &str) {
        let mut cache = self.cache.lock().await;
        if cache.as_ref().is_some_and(|c| c.value == used) {
            *cache = None;
        }
    }

    async fn fetch_token(&self) -> Result<CachedToken, PushError> {
        let now = chrono::Utc::now().timestamp();
        let assertion = self
            .account
            .assertion(&self.rng, now)
            .map_err(|_| PushError::Auth("서비스 계정 JWT 서명 실패".into()))?;
        let response = self
            .http
            .post(&self.token_url)
            .form(&[
                ("grant_type", JWT_BEARER),
                ("assertion", assertion.as_str()),
            ])
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| transient(format!("토큰 요청 실패: {}", e.without_url())))?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);
        let body = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            let error = crate::mask::global().mask(&oauth_error(&body));
            return Err(if status == 429 || status >= 500 {
                PushError::Transient {
                    message: format!("토큰 발급 실패(HTTP {status}): {error}"),
                    retry_after,
                }
            } else {
                PushError::Auth(format!(
                    "서비스 계정으로 토큰을 받지 못했다(HTTP {status}): {error}"
                ))
            });
        }
        let token: TokenResponse = serde_json::from_str(&body)
            .map_err(|_| transient("토큰 응답 형식이 맞지 않다".into()))?;
        crate::mask::global().register(&token.access_token);
        Ok(CachedToken {
            value: token.access_token,
            expires: Instant::now()
                + Duration::from_secs(token.expires_in.unwrap_or(3600).min(3600)),
        })
    }
}

/// OAuth 오류 응답에서 error·error_description만 꺼낸다(본문을 통째로 남기지 않는다).
fn oauth_error(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => format!(
            "{} {}",
            v.get("error").and_then(Value::as_str).unwrap_or("?"),
            v.get("error_description")
                .and_then(Value::as_str)
                .unwrap_or("")
        )
        .trim()
        .to_owned(),
        Err(_) => "알 수 없는 응답".into(),
    }
}

/// FCM 오류 본문에서 쓰는 부분.
#[derive(Debug, Default)]
struct FcmErrorBody {
    /// `details[].errorCode`(FcmError).
    code: Option<String>,
    /// `error.status`(google.rpc 상태 이름).
    status: Option<String>,
    message: Option<String>,
}

impl FcmErrorBody {
    fn parse(body: &str) -> Self {
        let Ok(v) = serde_json::from_str::<Value>(body) else {
            return Self::default();
        };
        let Some(error) = v.get("error") else {
            return Self::default();
        };
        let code = error
            .get("details")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find_map(|d| d.get("errorCode").and_then(Value::as_str))
            .map(str::to_owned);
        Self {
            code,
            status: error
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_owned),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }
    }
}

/// FCM 오류를 나눈다. `details[].errorCode`가 먼저고, 없을 때만 HTTP 상태로 본다
/// (errorCode 없는 404는 프로젝트 ID가 틀린 경우일 수 있어 토큰을 지우지 않는다).
fn classify(
    status: u16,
    error: &FcmErrorBody,
    retry_after: Option<Duration>,
    device_token: &str,
) -> PushError {
    let label = error
        .code
        .as_deref()
        .or(error.status.as_deref())
        .unwrap_or("?");
    let mut message = error.message.clone().unwrap_or_default();
    if !device_token.is_empty() {
        message = message.replace(device_token, &mask_device_token(device_token));
    }
    let message = crate::mask::global().mask(&message);
    let message: String = message.chars().take(MAX_ERROR_CHARS).collect();
    let detail = format!("HTTP {status} {label}: {message}")
        .trim_end_matches([' ', ':'])
        .to_owned();
    let transient = |detail: String| PushError::Transient {
        message: detail,
        retry_after,
    };
    match error.code.as_deref() {
        Some("UNREGISTERED") => PushError::Unregistered,
        Some("INVALID_ARGUMENT") => PushError::BadRequest(detail),
        Some("SENDER_ID_MISMATCH" | "THIRD_PARTY_AUTH_ERROR" | "THIRD_PARTY_PERMISSION_DENIED") => {
            PushError::Auth(detail)
        }
        Some("QUOTA_EXCEEDED" | "UNAVAILABLE" | "INTERNAL") => transient(detail),
        _ => match status {
            401 | 403 => PushError::Auth(detail),
            429 | 500..=599 => transient(detail),
            _ => PushError::BadRequest(detail),
        },
    }
}

/// Retry-After: 초(정수) 또는 HTTP 날짜.
fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    let after = if let Ok(secs) = value.parse::<u64>() {
        Duration::from_secs(secs)
    } else {
        let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
        let secs = (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
            .num_seconds()
            .max(0);
        Duration::from_secs(u64::try_from(secs).ok()?)
    };
    Some(after.min(MAX_RETRY_AFTER))
}

#[cfg(test)]
mod tests;
