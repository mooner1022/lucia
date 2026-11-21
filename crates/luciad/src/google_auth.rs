//! Google OAuth(Desktop app 클라이언트, 루프백 리디렉션, PKCE). 공식 문서로 확인한 흐름(2026-11-16):
//!
//! - 동의: `https://accounts.google.com/o/oauth2/v2/auth` + client_id, redirect_uri(`http://127.0.0.1:<포트>/`),
//!   response_type=code, scope, code_challenge(S256), state. 다시 동의할 때 새 refresh token을 받도록 prompt=consent.
//!   Desktop 앱은 refresh token이 늘 온다.
//! - 교환: `token_uri`에 code, client_id, client_secret(선택), code_verifier, grant_type, redirect_uri.
//! - 갱신: grant_type=refresh_token. `invalid_grant`면 다시 동의해야 한다(알림).
//! - 철회: `https://oauth2.googleapis.com/revoke`에 token.
//!
//! 클라이언트 JSON과 refresh token은 자격 증명 파일로만 둔다(관리 페이지가 쓴다). 액세스 토큰은 메모리에만.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use base64::Engine as _;
use lucia_precheck::SourceError;
use lucia_precheck::google::TokenProvider;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::credentials::{Credentials, names};

/// 읽기(캘린더·메일)와 메일 발송(결정 0003: 발송만 되는 가장 좁은 범위, 보내기는 사람 승인 뒤 데몬이 한다).
pub const SCOPES: &str = "https://www.googleapis.com/auth/calendar.readonly https://www.googleapis.com/auth/calendar.events https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/gmail.send";
/// 캘린더 일정 쓰기(결정 0033). 예전 연결에는 없다: 읽기는 그대로 되고, 쓰기는 다시 연결하라고 안내한다.
pub const SCOPE_CALENDAR_WRITE: &str = "https://www.googleapis.com/auth/calendar.events";
pub const SCOPE_GMAIL_SEND: &str = "https://www.googleapis.com/auth/gmail.send";
pub const SCOPE_GMAIL_READ: &str = "https://www.googleapis.com/auth/gmail.readonly";
pub const SCOPE_CALENDAR_READ: &str = "https://www.googleapis.com/auth/calendar.readonly";
const AUTH_URI: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const REVOKE_URI: &str = "https://oauth2.googleapis.com/revoke";
/// 동의를 시작하고 끝낼 때까지 기다리는 시간.
pub const CONSENT_TTL: Duration = Duration::from_secs(600);

/// Desktop app 클라이언트 JSON(`{"installed": {...}}`).
#[derive(Debug, Clone, Deserialize)]
pub struct OAuthClient {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default = "default_token_uri")]
    pub token_uri: String,
}

fn default_token_uri() -> String {
    "https://oauth2.googleapis.com/token".into()
}

impl OAuthClient {
    /// 내려받은 JSON을 읽는다. Desktop app 종류(`installed`)만 받는다.
    pub fn parse(json: &str) -> anyhow::Result<Self> {
        let value: serde_json::Value = serde_json::from_str(json.trim())
            .context("JSON이 아니다(내려받은 파일 내용 전체를 붙여 넣으세요)")?;
        let Some(installed) = value.get("installed") else {
            if value.get("web").is_some() {
                bail!(
                    "웹 애플리케이션 클라이언트다. Desktop app 종류로 만든 클라이언트가 필요하다"
                );
            }
            bail!("Desktop app 클라이언트 JSON이 아니다(`installed` 항목이 없다)");
        };
        let client: OAuthClient =
            serde_json::from_value(installed.clone()).context("클라이언트 JSON 형식")?;
        anyhow::ensure!(
            client.client_id.ends_with(".apps.googleusercontent.com"),
            "client_id 형식이 Google OAuth 클라이언트가 아니다"
        );
        anyhow::ensure!(
            client.token_uri.starts_with("https://"),
            "token_uri는 https여야 한다"
        );
        Ok(client)
    }
}

/// 진행 중인 동의 한 건.
#[derive(Debug, Clone)]
pub struct PendingConsent {
    pub state: String,
    pub verifier: String,
    pub redirect_uri: String,
    pub started: Instant,
}

fn random_token() -> String {
    // PKCE verifier 허용 문자(영숫자)만, 64자.
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// 동의 URL을 만든다.
pub fn consent_url(client: &OAuthClient, redirect_uri: &str) -> (String, PendingConsent) {
    let pending = PendingConsent {
        state: random_token(),
        verifier: random_token(),
        redirect_uri: redirect_uri.to_owned(),
        started: Instant::now(),
    };
    let mut url = url::Url::parse(AUTH_URI).expect("상수 URL");
    url.query_pairs_mut()
        .append_pair("client_id", &client.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPES)
        .append_pair("code_challenge", &challenge(&pending.verifier))
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &pending.state)
        .append_pair("prompt", "consent");
    (url.to_string(), pending)
}

/// 리디렉션된 URL(또는 그 쿼리)에서 code와 state를 꺼낸다. 사용자가 거부했으면 오류.
pub fn parse_callback(text: &str) -> anyhow::Result<(String, String)> {
    let text = text.trim();
    let query = match url::Url::parse(text) {
        Ok(u) => u.query().unwrap_or("").to_owned(),
        Err(_) => text.trim_start_matches('?').to_owned(),
    };
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }
    if let Some(error) = error {
        bail!("Google이 동의를 돌려주지 않았다: {error}");
    }
    match (code, state) {
        (Some(code), Some(state)) => Ok((code, state)),
        _ => bail!("주소에 code와 state가 없다(동의 뒤 주소창의 전체 주소를 붙여 넣으세요)"),
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    refresh_token: Option<String>,
    /// 실제로 받은 범위(공백으로 나눈 목록).
    #[serde(default)]
    scope: Option<String>,
}

pub struct GoogleAuth {
    creds: Arc<Credentials>,
    http: reqwest::Client,
    cache: Mutex<Option<(String, Instant)>>,
}

impl GoogleAuth {
    pub fn new(creds: Arc<Credentials>, http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            creds,
            http,
            cache: Mutex::new(None),
        })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// 클라이언트와 refresh token이 둘 다 있는가(값은 읽지 않는다).
    pub fn connected(&self) -> bool {
        self.creds.exists(names::GOOGLE_OAUTH_CLIENT)
            && self.creds.exists(names::GOOGLE_REFRESH_TOKEN)
    }

    pub fn has_client(&self) -> bool {
        self.creds.exists(names::GOOGLE_OAUTH_CLIENT)
    }

    pub async fn client(&self) -> anyhow::Result<OAuthClient> {
        let json = self
            .creds
            .read(names::GOOGLE_OAUTH_CLIENT)
            .await?
            .context("Google 클라이언트 JSON이 없다")?;
        OAuthClient::parse(&json)
    }

    /// 클라이언트 JSON을 확인하고 저장한다.
    pub async fn set_client(&self, json: &str) -> anyhow::Result<OAuthClient> {
        let client = OAuthClient::parse(json)?;
        if let Some(secret) = &client.client_secret {
            crate::mask::global().register(secret);
        }
        self.creds
            .write(names::GOOGLE_OAUTH_CLIENT, json.trim())
            .await?;
        Ok(client)
    }

    /// 동의로 받은 code를 토큰으로 바꾸고 refresh token을 저장한다. 실제로 받은 범위를 돌려준다.
    pub async fn complete(
        &self,
        pending: &PendingConsent,
        code: &str,
    ) -> anyhow::Result<Option<String>> {
        let client = self.client().await?;
        let mut form = vec![
            ("code", code.to_owned()),
            ("client_id", client.client_id.clone()),
            ("code_verifier", pending.verifier.clone()),
            ("grant_type", "authorization_code".to_owned()),
            ("redirect_uri", pending.redirect_uri.clone()),
        ];
        if let Some(secret) = &client.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        let response = self
            .http
            .post(&client.token_uri)
            .form(&form)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("토큰 교환 요청 실패: {}", e.without_url()))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!(
                "토큰 교환 실패(HTTP {}): {}",
                status.as_u16(),
                crate::mask::global().mask(&oauth_error(&body))
            );
        }
        let token: TokenResponse = serde_json::from_str(&body).context("토큰 응답 형식")?;
        crate::mask::global().register(&token.access_token);
        let refresh = token
            .refresh_token
            .context("refresh token이 오지 않았다(다시 연결해 보세요)")?;
        self.creds
            .write(names::GOOGLE_REFRESH_TOKEN, &refresh)
            .await?;
        *self.cache.lock().expect("cache lock") = Some((
            token.access_token,
            Instant::now() + Duration::from_secs(token.expires_in.unwrap_or(3600)),
        ));
        Ok(token.scope)
    }

    /// 연결을 끊는다: 토큰을 철회하고(실패해도 계속) 자격 증명을 지운다.
    pub async fn disconnect(&self) -> anyhow::Result<()> {
        if let Ok(Some(refresh)) = self.creds.read(names::GOOGLE_REFRESH_TOKEN).await {
            let result = self
                .http
                .post(REVOKE_URI)
                .form(&[("token", refresh)])
                .send()
                .await;
            if let Err(err) = result {
                tracing::warn!(err = %err.without_url(), "Google 토큰 철회 요청 실패(로컬에서는 지운다)");
            }
        }
        *self.cache.lock().expect("cache lock") = None;
        self.creds.remove(names::GOOGLE_REFRESH_TOKEN).await
    }

    async fn refresh(&self) -> Result<String, SourceError> {
        let client = self
            .client()
            .await
            .map_err(|e| SourceError::Auth(format!("{e:#}")))?;
        let refresh = self
            .creds
            .read(names::GOOGLE_REFRESH_TOKEN)
            .await
            .map_err(|e| SourceError::Transient(format!("{e:#}")))?
            .ok_or_else(|| SourceError::Auth("Google이 연결되지 않았다".into()))?;
        let mut form = vec![
            ("client_id", client.client_id.clone()),
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", refresh),
        ];
        if let Some(secret) = &client.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        let response = self
            .http
            .post(&client.token_uri)
            .form(&form)
            .send()
            .await
            .map_err(|e| {
                SourceError::Transient(format!("토큰 갱신 요청 실패: {}", e.without_url()))
            })?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let error = oauth_error(&body);
            return Err(
                if error.contains("invalid_grant") || error.contains("invalid_client") {
                    SourceError::Auth(format!(
                        "Google 동의가 만료됐거나 취소됐다(관리 페이지에서 다시 연결): {error}"
                    ))
                } else {
                    SourceError::Transient(format!(
                        "토큰 갱신 실패(HTTP {}): {error}",
                        status.as_u16()
                    ))
                },
            );
        }
        let token: TokenResponse = serde_json::from_str(&body)
            .map_err(|e| SourceError::Transient(format!("토큰 응답 형식: {e}")))?;
        crate::mask::global().register(&token.access_token);
        let expires = Instant::now() + Duration::from_secs(token.expires_in.unwrap_or(3600));
        *self.cache.lock().expect("cache lock") = Some((token.access_token.clone(), expires));
        Ok(token.access_token)
    }
}

/// OAuth 오류 응답에서 error·error_description만 꺼낸다(본문을 통째로 남기지 않는다).
fn oauth_error(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) => format!(
            "{} {}",
            v.get("error").and_then(|e| e.as_str()).unwrap_or("?"),
            v.get("error_description")
                .and_then(|e| e.as_str())
                .unwrap_or("")
        )
        .trim()
        .to_owned(),
        Err(_) => "알 수 없는 응답".into(),
    }
}

impl TokenProvider for GoogleAuth {
    async fn access_token(&self) -> Result<String, SourceError> {
        if let Some((token, expires)) = self.cache.lock().expect("cache lock").as_ref()
            && *expires > Instant::now() + Duration::from_secs(60)
        {
            return Ok(token.clone());
        }
        self.refresh().await
    }

    fn invalidate(&self) {
        *self.cache.lock().expect("cache lock") = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP: &str = r#"{"installed":{"client_id":"123-abc.apps.googleusercontent.com","project_id":"p","auth_uri":"https://accounts.google.com/o/oauth2/auth","token_uri":"https://oauth2.googleapis.com/token","client_secret":"GOCSPX-test","redirect_uris":["http://localhost"]}}"#;

    #[test]
    fn parses_desktop_client_and_rejects_others() {
        let c = OAuthClient::parse(DESKTOP).unwrap();
        assert_eq!(c.client_id, "123-abc.apps.googleusercontent.com");
        assert_eq!(c.token_uri, "https://oauth2.googleapis.com/token");
        assert!(
            OAuthClient::parse(&DESKTOP.replace("installed", "web"))
                .unwrap_err()
                .to_string()
                .contains("Desktop")
        );
        assert!(OAuthClient::parse("not json").is_err());
        assert!(
            OAuthClient::parse(&DESKTOP.replace(".apps.googleusercontent.com", ".example"))
                .is_err()
        );
    }

    #[test]
    fn consent_url_has_pkce_and_state() {
        let client = OAuthClient::parse(DESKTOP).unwrap();
        let (url, pending) = consent_url(&client, "http://127.0.0.1:8780/");
        let u = url::Url::parse(&url).unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], challenge(&pending.verifier));
        assert_eq!(q["state"], pending.state);
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:8780/");
        assert!(q["scope"].contains("calendar.readonly") && q["scope"].contains("gmail.readonly"));
        assert_eq!(pending.verifier.len(), 64);
        // RFC 7636 예시: verifier "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk" → challenge.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn parses_pasted_callback_urls() {
        let (code, state) =
            parse_callback("http://127.0.0.1:8780/?state=abc&code=4/0Ab-xyz&scope=x").unwrap();
        assert_eq!((code.as_str(), state.as_str()), ("4/0Ab-xyz", "abc"));
        assert!(
            parse_callback("http://127.0.0.1:8780/?error=access_denied&state=abc")
                .unwrap_err()
                .to_string()
                .contains("access_denied")
        );
        assert!(parse_callback("http://127.0.0.1:8780/").is_err());
        let (code, _) = parse_callback("?code=c1&state=s1").unwrap();
        assert_eq!(code, "c1");
    }

    #[test]
    fn oauth_error_keeps_only_error_fields() {
        let body = r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked.","refresh_token":"1//leak"}"#;
        let e = oauth_error(body);
        assert!(e.starts_with("invalid_grant"));
        assert!(!e.contains("1//leak"));
    }
}
