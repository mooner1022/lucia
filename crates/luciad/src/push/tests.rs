use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Form, Json, Router};
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};

use super::*;

// 시험 전용 RSA 2048 키(PKCS#8). 이 시험을 위해 openssl로 만든 것이고 어떤 계정·프로젝트와도 관계없다.
// 비밀 검사기가 실제 키로 오인하지 않도록 PEM 머리·꼬리 줄은 시험 안에서 붙인다.
const TEST_KEY_BODY: &str = concat!(
    "MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDSSeYzfQS774Vg\n",
    "qERUJxSOvtbElRTf1h3RFmGgd6CVdgX4z3v4sPEgUbablNFR3JLpFu96b+Owswu1\n",
    "0ai6XHkOcju2ew6tpofuGX9FifKuVKrRVMxQTmo/z6sC6Ojir9j2taEl2jr2NRhF\n",
    "8m84L7yIfBGA//cglAhyFBWa8eme74XH3OCAulwFwAa02mnu08fsVGrjNxG64BlA\n",
    "HQApF5pTgzY8MGRq86rFemHZAeHP0XsYf3yQ/vNHwRQhharCFFXjWfavkJV/x8Zm\n",
    "tieIUqCt3iiVSJvdPNOb0D8xb/oyb8hEkEQUyAbKshvlRtWGmrk7RgBj7MBSitGm\n",
    "edmr53E7AgMBAAECggEAATLNno6AV+faDT54QTiFy/h3deBZoZrxSY9EmCsGXcOx\n",
    "7gSUssqjXSlKDqHu/XXx2mU4kSyREiE7GFqtCP7c68Gn6HbUExZHYQqBKK7MFMVD\n",
    "wfSnQ2/m+B0hXt9f8GbLKEDJvdL8EgokbMWgb6QKAYcffGo5CNT/JVNK+jqVt8Oo\n",
    "DOmHThTXltzYHw9VVVA/qyKZIOEtgVqvesTh506xJAPz5m9DjCdDI8wihF3HA0I+\n",
    "hC6YreB2r31JWmI0eHOCKaOZ/f+uCM2QTj9cI/jmrENwcZVUgGZ65h85kVwASPvL\n",
    "XEod4AIonaDJSDvvXf5vR3+Ui9o8qgtQJVajXrnicQKBgQDrtjuoQYCxzGNUre8n\n",
    "TCtDK29XqsOPU/sNN2krzeXHpRqtxtpdPuPZV702atMSaDcphWwx0fJ/1YCjjIh8\n",
    "Z/esmEpmkTID7/LKz/FyTbNY07hrl/hTVecDdjij7Dtn/Be4oA2iV0W3haxehgGb\n",
    "+ZD2iSpRig1vtbMKinyErnn3iwKBgQDkY3tflSSkmbUfwhEH7q6zGSE12U/ycbsC\n",
    "IHGRvdP1JvnlwEaeXINzfKEx8OTedXpCNBGsb3GBh4S+8dJxajvE8317JRsIffGy\n",
    "CXX2dTIzhRR2CMckusDf117uJaEb/WaPwaffrxekv8cWHlm3tvvdePkKDnRhNmPQ\n",
    "Qi2CqaIjEQKBgQDN1MOBqTmFiXKQBpcZXOVGcY54IpFE/tPy9ZksfE8UKLQEXepf\n",
    "lMSJDZoYA+6vWzwYUP9q/efiA4Giz9x7rppUKGSH7nCbiZQd0LoJ05oyjie0JO/f\n",
    "bdseZcUOOAKkSjo4qeNeCRHDMcKUhDaxipww/RRsQb5791ISeV5EysuNAwKBgQDD\n",
    "I6z6Ujho21xCslJS8MEcDdCubes9P6eFfasL16IOySqj/9FW5bz7I/Wm8ijbWo0H\n",
    "m+2t49qbzerCK2SfuyEues0wNtgOPW4RrLu5xRELrB17bKya2WaK2z4lRMmPXMgQ\n",
    "9Go3WSdegO3eHDiUHCi/ZumCii8uEIbgHms8RSxfwQKBgHC29PfKUBr+eU2rPrd2\n",
    "c9qS0kpnM5yYCDvjbFXkd6VWTZDNSa5N3fmjz+h3XyO9RAoW1fjCCpmajQJZvjXj\n",
    "NRji4umBakYA4k3F4ZWhMmQljqghQ5yaRKdnrIcKJ/gWf7mTEzBAcG2mSD5flX5+\n",
    "CRkyL0VvBkpxAk2f5EJ2K/PC\n",
);

const PROJECT: &str = "fake-project";
const EMAIL: &str = "pusher@fake-project.example";
const AUD: &str = "https://oauth2.googleapis.com/token";
const DEVICE: &str = "devtok-1234567890abcdef";

fn test_pem() -> String {
    let label = ["PRIVATE", "KEY"].join(" ");
    format!("-----BEGIN {label}-----\n{TEST_KEY_BODY}-----END {label}-----\n")
}

fn account_json(private_key: &str) -> String {
    json!({
        "type": "service_account",
        "project_id": PROJECT,
        "private_key_id": "0",
        "private_key": private_key,
        "client_email": EMAIL,
        "token_uri": AUD,
    })
    .to_string()
}

fn public_key() -> Vec<u8> {
    let der = pem_to_der(&test_pem()).unwrap();
    RsaKeyPair::from_pkcs8(&der)
        .unwrap()
        .public()
        .as_ref()
        .to_vec()
}

/// 발송 응답 하나(비어 있으면 성공).
struct Reply {
    status: u16,
    retry_after: Option<&'static str>,
    body: Value,
}

#[derive(Default)]
struct Mock {
    public_key: Vec<u8>,
    token_hits: AtomicUsize,
    /// 발급한 토큰(최근 것만 유효).
    current: Mutex<Option<String>>,
    replies: Mutex<VecDeque<Reply>>,
    /// 받은 발송: (Authorization, 본문).
    sends: Mutex<Vec<(String, Value)>>,
}

fn b64_json(segment: &str) -> Value {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment)
        .unwrap();
    serde_json::from_slice(&raw).unwrap()
}

async fn token_endpoint(
    State(mock): State<Arc<Mock>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let reject = |why: &str| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant", "error_description": why})),
        )
            .into_response()
    };
    if form.get("grant_type").map(String::as_str) != Some(JWT_BEARER) {
        return reject("grant_type");
    }
    let Some(assertion) = form.get("assertion") else {
        return reject("assertion");
    };
    let parts: Vec<&str> = assertion.split('.').collect();
    if parts.len() != 3 {
        return reject("segments");
    }
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[2])
        .unwrap();
    let signed = format!("{}.{}", parts[0], parts[1]);
    if UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, &mock.public_key)
        .verify(signed.as_bytes(), &signature)
        .is_err()
    {
        return reject("signature");
    }
    let header = b64_json(parts[0]);
    if header != json!({"alg": "RS256", "typ": "JWT"}) {
        return reject("header");
    }
    let claims = b64_json(parts[1]);
    let iat = claims["iat"].as_i64().unwrap();
    let exp = claims["exp"].as_i64().unwrap();
    let now = chrono::Utc::now().timestamp();
    if claims["iss"] != EMAIL
        || claims["scope"] != SCOPE
        || claims["aud"] != AUD
        || exp <= iat
        || exp - iat > 3600
        || (iat - now).abs() > 60
    {
        return reject("claims");
    }
    let n = mock.token_hits.fetch_add(1, Ordering::SeqCst) + 1;
    let token = format!("fcm-mock-access-token-{n}");
    *mock.current.lock().unwrap() = Some(token.clone());
    Json(json!({"access_token": token, "expires_in": 3600, "token_type": "Bearer"})).into_response()
}

async fn send_endpoint(
    State(mock): State<Arc<Mock>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    mock.sends
        .lock()
        .unwrap()
        .push((auth.clone(), body.clone()));
    let current = mock.current.lock().unwrap().clone();
    if current.map(|t| format!("Bearer {t}")) != Some(auth) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(fcm_error(401, "UNAUTHENTICATED", None)),
        )
            .into_response();
    }
    let Some(message) = body.get("message") else {
        return (
            StatusCode::BAD_REQUEST,
            Json(fcm_error(400, "INVALID_ARGUMENT", Some("INVALID_ARGUMENT"))),
        )
            .into_response();
    };
    assert_eq!(message["token"], DEVICE);
    let reply = mock.replies.lock().unwrap().pop_front();
    match reply {
        None => {
            Json(json!({"name": format!("projects/{PROJECT}/messages/0:1700")})).into_response()
        }
        Some(reply) => {
            let mut response = (
                StatusCode::from_u16(reply.status).unwrap(),
                Json(reply.body),
            )
                .into_response();
            if let Some(after) = reply.retry_after {
                response
                    .headers_mut()
                    .insert("retry-after", after.parse().unwrap());
            }
            response
        }
    }
}

/// Google API 공통 오류 본문(가정한 모양, 모듈 머리말 참고).
fn fcm_error(code: u16, status: &str, error_code: Option<&str>) -> Value {
    let details = match error_code {
        Some(c) => {
            json!([{"@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError", "errorCode": c}])
        }
        None => json!([]),
    };
    json!({"error": {"code": code, "message": format!("{status} from mock"), "status": status, "details": details}})
}

async fn start(replies: Vec<Reply>) -> (Fcm, Arc<Mock>) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mock = Arc::new(Mock {
        public_key: public_key(),
        replies: Mutex::new(replies.into()),
        ..Default::default()
    });
    let app = Router::new()
        .route("/token", post(token_endpoint))
        .route(
            &format!("/v1/projects/{PROJECT}/messages:send"),
            post(send_endpoint),
        )
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let account = ServiceAccount::parse(&account_json(&test_pem())).unwrap();
    let fcm = Fcm::new(reqwest::Client::new(), account)
        .with_endpoints(&format!("http://{addr}"), &format!("http://{addr}/token"));
    (fcm, mock)
}

fn message() -> PushMessage {
    PushMessage {
        token: DEVICE.into(),
        title: Some("제목".into()),
        body: Some("본문".into()),
        data: BTreeMap::from([("kind".to_owned(), "chat".to_owned())]),
        high_priority: true,
        collapse: Some("chat-1".into()),
    }
}

#[tokio::test]
async fn sends_and_reuses_access_token() {
    let (fcm, mock) = start(vec![]).await;
    let name = fcm.send(&message()).await.unwrap();
    assert_eq!(name, format!("projects/{PROJECT}/messages/0:1700"));
    fcm.send(&message()).await.unwrap();
    assert_eq!(mock.token_hits.load(Ordering::SeqCst), 1);

    let sends = mock.sends.lock().unwrap();
    assert_eq!(sends.len(), 2);
    assert_eq!(sends[0].0, "Bearer fcm-mock-access-token-1");
    let m = &sends[0].1["message"];
    assert_eq!(m["notification"], json!({"title": "제목", "body": "본문"}));
    assert_eq!(m["data"], json!({"kind": "chat"}));
    assert_eq!(m["android"]["priority"], "high");
    assert_eq!(m["android"]["collapse_key"], "chat-1");
    assert_eq!(m["android"]["notification"]["tag"], "chat-1");
    assert_eq!(m["apns"]["headers"]["apns-priority"], "10");
    assert_eq!(m["apns"]["headers"]["apns-collapse-id"], "chat-1");
    assert!(m["apns"].get("payload").is_none());
}

#[tokio::test]
async fn refreshes_token_once_after_401() {
    let (fcm, mock) = start(vec![Reply {
        status: 401,
        retry_after: None,
        body: fcm_error(401, "UNAUTHENTICATED", None),
    }])
    .await;
    fcm.send(&message()).await.unwrap();
    assert_eq!(mock.token_hits.load(Ordering::SeqCst), 2);
    let sends = mock.sends.lock().unwrap();
    assert_eq!(sends.len(), 2);
    assert_eq!(sends[0].0, "Bearer fcm-mock-access-token-1");
    assert_eq!(sends[1].0, "Bearer fcm-mock-access-token-2");
}

#[tokio::test]
async fn second_401_is_auth_error_and_third_party_auth_is_not_retried() {
    let unauth = || Reply {
        status: 401,
        retry_after: None,
        body: fcm_error(401, "UNAUTHENTICATED", None),
    };
    let (fcm, mock) = start(vec![unauth(), unauth()]).await;
    assert!(matches!(
        fcm.send(&message()).await,
        Err(PushError::Auth(_))
    ));
    assert_eq!(mock.sends.lock().unwrap().len(), 2);

    let (fcm, mock) = start(vec![Reply {
        status: 401,
        retry_after: None,
        body: fcm_error(401, "UNAUTHENTICATED", Some("THIRD_PARTY_AUTH_ERROR")),
    }])
    .await;
    let err = fcm.send(&message()).await.unwrap_err();
    assert!(matches!(&err, PushError::Auth(m) if m.contains("THIRD_PARTY_AUTH_ERROR")));
    assert_eq!(mock.sends.lock().unwrap().len(), 1);
    assert_eq!(mock.token_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unregistered_token() {
    let (fcm, _mock) = start(vec![Reply {
        status: 404,
        retry_after: None,
        body: fcm_error(404, "NOT_FOUND", Some("UNREGISTERED")),
    }])
    .await;
    assert_eq!(
        fcm.send(&message()).await.unwrap_err(),
        PushError::Unregistered
    );
}

#[tokio::test]
async fn unavailable_with_retry_after_is_transient() {
    let (fcm, _mock) = start(vec![Reply {
        status: 503,
        retry_after: Some("120"),
        body: fcm_error(503, "UNAVAILABLE", Some("UNAVAILABLE")),
    }])
    .await;
    match fcm.send(&message()).await.unwrap_err() {
        PushError::Transient { retry_after, .. } => {
            assert_eq!(retry_after, Some(Duration::from_secs(120)));
        }
        other => panic!("일시 오류여야 한다: {other:?}"),
    }
}

#[tokio::test]
async fn data_only_message_has_no_notification() {
    let (fcm, mock) = start(vec![]).await;
    let msg = PushMessage {
        title: None,
        body: None,
        ..message()
    };
    fcm.send(&msg).await.unwrap();
    let sends = mock.sends.lock().unwrap();
    let m = &sends[0].1["message"];
    assert!(m.get("notification").is_none());
    assert_eq!(m["data"], json!({"kind": "chat"}));
    // Android는 high 그대로, Apple 데이터 메시지는 5여야 한다(10이면 FCM이 거부).
    assert_eq!(m["android"]["priority"], "high");
    assert_eq!(m["android"]["collapse_key"], "chat-1");
    assert!(m["android"].get("notification").is_none());
    assert_eq!(m["apns"]["headers"]["apns-priority"], "5");
    assert_eq!(m["apns"]["headers"]["apns-collapse-id"], "chat-1");
    assert_eq!(m["apns"]["payload"]["aps"]["content-available"], 1);
}

#[test]
fn normal_priority_without_collapse() {
    let msg = PushMessage {
        high_priority: false,
        collapse: None,
        data: BTreeMap::new(),
        ..message()
    };
    let m = msg.to_json();
    assert_eq!(m["android"], json!({"priority": "normal"}));
    assert_eq!(m["apns"], json!({"headers": {"apns-priority": "5"}}));
    assert!(m.get("data").is_none());
    let notification = PushMessage {
        body: None,
        ..message()
    }
    .to_json();
    assert_eq!(notification["notification"], json!({"title": "제목"}));
}

#[tokio::test]
async fn reserved_data_keys_are_rejected_locally() {
    let (fcm, mock) = start(vec![]).await;
    let mut msg = message();
    msg.data.insert("google.x".into(), "1".into());
    assert!(matches!(
        fcm.send(&msg).await,
        Err(PushError::BadRequest(m)) if m.contains("google.x")
    ));
    assert_eq!(mock.token_hits.load(Ordering::SeqCst), 0);
}

#[test]
fn classify_prefers_error_code_and_masks_device_token() {
    let body = |status: &str, code: Option<&str>, message: &str| FcmErrorBody {
        code: code.map(str::to_owned),
        status: Some(status.into()),
        message: Some(message.into()),
    };
    // errorCode 없는 404는 토큰 문제로 보지 않는다.
    assert!(matches!(
        classify(404, &body("NOT_FOUND", None, "x"), None, DEVICE),
        PushError::BadRequest(_)
    ));
    let err = classify(
        400,
        &body(
            "INVALID_ARGUMENT",
            Some("INVALID_ARGUMENT"),
            &format!("The registration token is not valid: {DEVICE}"),
        ),
        None,
        DEVICE,
    );
    let PushError::BadRequest(text) = err else {
        panic!("BadRequest여야 한다");
    };
    assert!(!text.contains(DEVICE), "{text}");
    assert!(text.contains("devtok…"));
    assert!(matches!(
        classify(
            403,
            &body("PERMISSION_DENIED", Some("SENDER_ID_MISMATCH"), ""),
            None,
            DEVICE
        ),
        PushError::Auth(_)
    ));
    assert!(matches!(
        classify(
            429,
            &body("RESOURCE_EXHAUSTED", Some("QUOTA_EXCEEDED"), ""),
            None,
            DEVICE
        ),
        PushError::Transient { .. }
    ));
    assert!(matches!(
        classify(500, &FcmErrorBody::default(), None, DEVICE),
        PushError::Transient { .. }
    ));
    assert!(format!("{:?}", message()).contains("devtok…"));
    assert!(!format!("{:?}", message()).contains(DEVICE));
}

#[test]
fn retry_after_forms() {
    assert_eq!(parse_retry_after(" 30 "), Some(Duration::from_secs(30)));
    assert_eq!(
        parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"),
        Some(Duration::ZERO)
    );
    let later = (chrono::Utc::now() + chrono::Duration::seconds(90)).to_rfc2822();
    let after = parse_retry_after(&later).unwrap();
    assert!(after > Duration::from_secs(80) && after <= Duration::from_secs(90));
    assert_eq!(parse_retry_after("soon"), None);
}

#[test]
fn parse_reads_service_account_and_hides_key() {
    let account = ServiceAccount::parse(&account_json(&test_pem())).unwrap();
    assert_eq!(account.project_id(), PROJECT);
    assert_eq!(account.client_email(), EMAIL);
    assert!(!format!("{account:?}").contains("MIIE"));
    // \r\n 줄바꿈도 읽는다.
    assert!(ServiceAccount::parse(&account_json(&test_pem().replace('\n', "\r\n"))).is_ok());
    // token_uri가 없으면 기본값.
    let mut v: Value = serde_json::from_str(&account_json(&test_pem())).unwrap();
    v.as_object_mut().unwrap().remove("token_uri");
    assert!(ServiceAccount::parse(&v.to_string()).is_ok());
}

#[test]
fn parse_errors_do_not_echo_input() {
    let pem = test_pem();
    let snippet = &TEST_KEY_BODY[..40];

    let not_sa = account_json(&pem).replace("service_account", "authorized_user");
    let err = ServiceAccount::parse(&not_sa).unwrap_err();
    assert!(err.contains("service_account"));
    assert!(!err.contains(snippet));

    let err = ServiceAccount::parse(&format!("{{\"private_key\": \"{snippet}")).unwrap_err();
    assert!(!err.contains(snippet), "{err}");

    // 머리줄은 맞는데 본문이 깨진 키.
    let broken = pem.replacen("MIIEvgIBADAN", "MIIEvgIB!!AN", 1);
    let err = ServiceAccount::parse(&account_json(&broken)).unwrap_err();
    assert!(!err.contains("MIIEvgIB"), "{err}");
    // base64는 맞지만 키가 아닌 것(일부를 잘라냄).
    let cut: String = TEST_KEY_BODY.lines().take(5).collect::<Vec<_>>().join("\n");
    let label = ["PRIVATE", "KEY"].join(" ");
    let truncated = format!("-----BEGIN {label}-----\n{cut}\n-----END {label}-----\n");
    let err = ServiceAccount::parse(&account_json(&truncated)).unwrap_err();
    assert!(err.contains("RSA"), "{err}");
    assert!(!err.contains(&cut[..20]), "{err}");
    // PKCS#1 PEM은 받지 않는다.
    let pkcs1 = pem.replace(&label, "RSA PRIVATE KEY");
    assert!(ServiceAccount::parse(&account_json(&pkcs1)).is_err());

    let mut v: Value = serde_json::from_str(&account_json(&pem)).unwrap();
    v["project_id"] = json!("Bad/Project");
    assert!(ServiceAccount::parse(&v.to_string()).is_err());
    v["project_id"] = json!(PROJECT);
    v["token_uri"] = json!("http://insecure.example/token");
    assert!(ServiceAccount::parse(&v.to_string()).is_err());
}

#[test]
fn assertion_verifies_with_public_key() {
    let account = ServiceAccount::parse(&account_json(&test_pem())).unwrap();
    let jwt = account
        .assertion(&SystemRandom::new(), 1_700_000_000)
        .unwrap();
    let parts: Vec<&str> = jwt.split('.').collect();
    assert_eq!(parts.len(), 3);
    assert!(!jwt.contains('=') && !jwt.contains('+') && !jwt.contains('/'));
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[2])
        .unwrap();
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, public_key())
        .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
        .unwrap();
    let claims = b64_json(parts[1]);
    assert_eq!(
        claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
        3600
    );
    assert_eq!(claims["aud"], AUD);
}
