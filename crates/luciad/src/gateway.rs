//! 접속 API(결정 0021, 명세 7단계 "어댑터 인터페이스를 WebSocket이나 HTTP API로 노출"): 자체 앱이 Discord와
//! 같은 자리의 채널이 된다. 판 2(결정 0043)에서 전용 앱이 Discord를 대신할 수 있게 넓혔다.
//!
//! - **클라이언트**: 관리 페이지가 이름과 역할을 붙여 토큰을 발급하고, 해시만 남긴다. 언제든 취소한다.
//!   - 역할 `app`: 사람(소유자)이 쓰는 앱. 채팅과 승인 카드를 Discord와 똑같이 다룬다. 기본 밖의 권한(예: 권한 모드
//!     바꾸기)은 발급할 때 고르는 범위(`scopes`)로만 준다.
//!   - 역할 `device`: 외부 기기. 이벤트 프레임(`event`)만 받아 이벤트 입구로 넘긴다(결정 0031).
//! - **연결**: `GET /v1/ws`(WebSocket). 첫 프레임 `hello{token}`으로 인증한다. 집 LAN·VPN에서 온 연결과, 설정
//!   `[public] gateway_url`이 있으면 믿는 리버스 프록시를 거친 연결을 받는다. 실패가 많은 주소는 잠시 막는다.
//!   HTTP(`/v1/files`, `/v1/history`, `/v1/push` 등, [`http`])는 `Authorization: Bearer <토큰>`.
//! - **잃지 않게**: 앱으로 가는 글·카드는 번호(`seq`)를 붙여 상자(`app_outbox`)에 넣고 보낸다. 앱은 `ack`로 받은
//!   번호를 알리고, 다시 연결할 때 `hello.resume`으로 그 뒤의 것을 다시 받는다. 끊겨 있거나 뒤로 가 있으면 푸시로
//!   깨운다(FCM, [`PushJob`]). 푸시를 쓸 수 없으면 예전처럼 알림 대화(Discord DM)로 보낸다.
//! - **넓히기**: 앱이 부르는 메서드(`call`)는 등록표([`AppMethod`])로, 데몬이 앱에 부탁하는 일(`invoke`)은 앱이
//!   hello에서 알린 기능(`features`)으로 정한다. 앱에서만 되는 연동은 여기에 하나씩 더한다.
//! - **채널**: 앱의 대화는 `app:<클라이언트>`다. 엔진은 [`RouterChannel`]을 받아 대화 ID로 Discord와 앱을 가른다.
//!   관리 페이지에서 앱을 "알림 받을 곳"으로 정하면 브리핑·알림·승인 카드를 그 앱 대화로 보낸다([`NotifyTarget`]).
//!   앱이 끊겨 있고 푸시도 없으면 Discord DM으로 간다.

mod handover;
mod http;
mod methods;

// 앱에서만 되는 연동 모듈이 메서드를 구현할 때 쓴다(지금은 기본 메서드뿐).
pub use handover::Handover;
#[allow(unused_imports)]
pub use methods::{AppMethod, CallContext, CallError, CallFuture};

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::Utc;
use futures_util::{SinkExt as _, StreamExt as _};
use lucia_channel::{ChannelAdapter, ChannelError, check_files, split_message};
use lucia_core::{
    ButtonPress, Capabilities, Card, CardTone, ConversationId, Inbound, InboundAttachment,
    Outbound, SecretText,
};
use lucia_store::Store;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// 어댑터 이름(`Inbound::platform`).
pub const PLATFORM: &str = "app";
/// 프로토콜 판. 바뀌면 올린다. 클라이언트는 `welcome.v`를 보고 맞춘다.
pub const PROTOCOL_VERSION: u32 = 2;
/// 메시지 하나의 최대 길이(문자). 넘으면 나눠 보낸다.
pub const MAX_MESSAGE_CHARS: usize = 8000;
/// 클라이언트가 보내는 말 하나의 최대 길이(문자).
const MAX_INBOUND_CHARS: usize = 8000;
/// 프레임 하나의 최대 크기(받는 쪽). 파일은 HTTP로 올린다.
const MAX_FRAME_BYTES: usize = 64 * 1024;
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// 반쯤 죽은 연결(휴대폰이 잠들거나 Wi-Fi를 벗어남) 찾기: 이만큼마다 핑을 보내고, 이만큼 아무것도(퐁 포함)
/// 듣지 못하면 끊는다. 그 사이에 보낸 글은 상자에 남아 다시 연결할 때 받는다.
const PING_EVERY: Duration = Duration::from_secs(30);
const IDLE_LIMIT: Duration = Duration::from_secs(75);
/// 이 시간 안에 인증 실패가 이만큼이면 그 주소를 이 시간 동안 막는다.
const FAILURE_WINDOW: Duration = Duration::from_secs(600);
const MAX_FAILURES: u32 = 10;
/// 주소별 실패 기록 상한(인터넷 훑기로 끝없이 늘지 않게).
const MAX_FAILURE_ENTRIES: usize = 4096;
/// 발급하는 클라이언트 수 상한.
pub const MAX_CLIENTS: usize = 20;
/// 다시 받기로 한 번에 보내는 프레임 수 [제안]. 넘으면 `resumed.truncated`로 알리고 앱은 기록을 다시 읽는다.
const REPLAY_MAX: usize = 500;
/// 말 하나에 붙이는 파일 수.
const MAX_MESSAGE_FILES: usize = 10;
/// 앱·기기 한 연결이 한 시간에 보내는 이벤트 수 [제안]. 넘으면 거절한다(고장이 모델 호출로 번지지 않게).
const EVENTS_PER_HOUR: usize = 60;
/// 앱이 hello에서 알리는 기능 수와 이름 길이.
const MAX_FEATURES: usize = 32;
const MAX_FEATURE_CHARS: usize = 40;
/// 푸시로 깨우는 프레임 종류(진행 안내·카드 닫힘은 깨우지 않는다).
const PUSH_KINDS: &[&str] = &["text", "files", "card", "sticker"];

/// 설정 키(결정 0043).
pub mod keys {
    /// 알림·브리핑·승인 카드를 받을 앱 클라이언트. 비우면 Discord DM.
    pub const NOTIFY_CLIENT: &str = "app.notify_client";
    /// 푸시에 글 미리보기를 싣나(`"1"`). 기본은 싣지 않는다(푸시는 Google을 거친다).
    pub const PUSH_PREVIEW: &str = "app.push_preview";
}

/// 클라이언트 범위(결정 0043): 기본 밖의 권한. 발급할 때 관리 페이지에서 고른다.
pub mod scopes {
    /// 권한 모드 바꾸기(`/권한 엄격|기본|해제`, 결정 0040). 토큰이 새면 해제 모드로 바꿀 수 있어 기본은 없다.
    pub const SECURITY_MODE: &str = "security_mode";
    /// Lucia가 이 앱의 기능(위치, 휴대폰 알림 등)을 부르게 한다(결정 0053). 기본은 없다.
    pub const APP_TOOLS: &str = "app_tools";
    /// 브라우저 화면 넘겨받기를 앱에서(결정 0063): 로그인된 브라우저를 보고 조작한다. 기본은 없다.
    pub const BROWSER_HANDOVER: &str = "browser_handover";
    /// (이름, 관리 페이지 설명).
    pub const ALL: &[(&str, &str)] = &[
        (SECURITY_MODE, "권한 모드 바꾸기(/권한)"),
        (APP_TOOLS, "Lucia가 앱 기능 부르기"),
        (BROWSER_HANDOVER, "브라우저 화면 넘겨받기"),
    ];
}

/// 클라이언트의 역할.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// 소유자가 쓰는 앱.
    App,
    /// 외부 기기(이벤트만).
    Device,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Device => "device",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "app" => Some(Self::App),
            "device" => Some(Self::Device),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::App => "앱",
            Self::Device => "기기",
        }
    }
}

/// 앱 클라이언트의 대화 ID.
pub fn conversation(client_id: &str) -> ConversationId {
    ConversationId(format!("{PLATFORM}:{client_id}"))
}

/// 앱 대화 ID나 카드 참조(`app:<클라이언트>[:<카드>]`)의 클라이언트.
pub fn client_of(id: &str) -> Option<&str> {
    let rest = id.strip_prefix("app:")?;
    let client = rest.split(':').next().unwrap_or("");
    (!client.is_empty()).then_some(client)
}

/// 새 토큰(약 244비트). 앞머리로 무엇의 토큰인지 알아보게 한다.
pub fn new_token() -> String {
    format!(
        "lucia_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub fn token_hash(token: &str) -> String {
    crate::admin::sha256_hex(token.trim())
}

pub fn new_client_id() -> String {
    format!("cl-{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

/// 집 LAN·VPN에서 온 연결인가(루프백, 사설, 링크 로컬, CGNAT(100.64/10, Tailscale 등), IPv6 ULA).
pub fn local_peer(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            v.is_loopback()
                || v.is_private()
                || v.is_link_local()
                || (o[0] == 100 && (o[1] & 0xC0) == 64)
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return local_peer(IpAddr::V4(v4));
            }
            let s0 = v.segments()[0];
            v.is_loopback() || (s0 & 0xfe00) == 0xfc00 || (s0 & 0xffc0) == 0xfe80
        }
    }
}

/// 알림을 받을 앱 클라이언트(관리 페이지에서 정한 것, 취소되지 않은 app 역할만).
pub fn notify_client(store: &Store) -> Option<String> {
    let id = store.setting(keys::NOTIFY_CLIENT).ok().flatten()?;
    store
        .active_client(&id)
        .ok()
        .flatten()
        .filter(|c| c.role == Role::App.as_str())
        .map(|c| c.id)
}

/// 알림 받을 앱이 화면 넘겨받기를 쓸 수 있는가(결정 0063, 안내 글에 쓴다): 그 앱 토큰에 범위가 있다.
pub fn notify_app_can_handover(store: &Store) -> bool {
    notify_client(store)
        .and_then(|id| store.active_client(&id).ok().flatten())
        .is_some_and(|c| c.scopes.iter().any(|s| s == scopes::BROWSER_HANDOVER))
}

/// 소유자에게 알릴 곳을 읽는 곳(엔진의 브리핑·알림, 승인 카드, 로그인 안내 등)이 쓴다(결정 0043): 알림 받을 앱을
/// 정했으면 그 앱 대화, 아니면 관리 페이지의 알림 대화(Discord DM). 앱이 끊겨 있고 푸시도 없으면 [`RouterChannel`]이
/// Discord DM으로 돌린다. 소유자가 Discord DM에서 직접 한 대화의 답은 그 DM으로 간다(대화 ID로 가르지 않는다).
pub trait NotifyTarget {
    fn notify_conversation(&self) -> Option<String>;
}

impl NotifyTarget for Store {
    fn notify_conversation(&self) -> Option<String> {
        notify_client(self).map(|c| conversation(&c).0).or_else(|| {
            self.setting(lucia_store::keys::NOTIFY_CONVERSATION)
                .ok()
                .flatten()
        })
    }
}

/// 푸시로 깨울 일(결정 0043). 푸시 작업자([`crate::apppush`])가 받아 FCM으로 보낸다.
#[derive(Debug, Clone, PartialEq)]
pub struct PushJob {
    pub client_id: String,
    /// FCM 등록 토큰(로그에 남기지 않는다).
    pub token: String,
    /// 깨운 프레임 종류(`text`, `files`, `card`, `sticker`).
    pub kind: String,
    pub seq: i64,
    /// 미리보기 후보(설정이 켜졌을 때만 싣는다).
    pub preview: String,
    /// 카드처럼 바로 봐야 하는 것.
    pub high: bool,
}

/// 앱이 Lucia에게 내주는 기능 하나(결정 0053). 앱이 `hello.tools`로 알린다.
#[derive(Debug, Clone, PartialEq)]
pub struct AppTool {
    /// 메서드 이름(`기능.동작`, 예: `location.get`).
    pub name: String,
    pub description: String,
    /// 인자 설명(앱이 준 JSON. 스키마든 예시든 그대로 보여 준다).
    pub params: Value,
    /// 무엇을 바꾸는 기능이다(사람이 대화 중일 때만 부른다).
    pub write: bool,
}

const MAX_APP_TOOLS: usize = 32;
const APP_TOOL_DESCRIPTION_CHARS: usize = 200;
const APP_TOOL_PARAMS_BYTES: usize = 2000;
/// 앱 기능 하나를 기다리는 시간(MCP 요청이 60초쯤에 끊기기 전에 답한다).
pub const APP_CALL_TIMEOUT: Duration = Duration::from_secs(40);

/// `hello.tools`를 정리한다: 이름은 `기능.동작`(소문자·숫자·`_`·`-`), 설명은 한 줄 200자, 인자 설명은 2 KB까지.
fn clean_tools(list: Vec<Value>) -> Vec<AppTool> {
    let segment = |s: &str| {
        !s.is_empty()
            && s.len() <= MAX_FEATURE_CHARS
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    };
    let mut out: Vec<AppTool> = Vec::new();
    for v in list {
        let name = v["name"].as_str().unwrap_or("").trim().to_owned();
        let parts: Vec<&str> = name.split('.').collect();
        if !(2..=3).contains(&parts.len()) || !parts.iter().all(|p| segment(p)) {
            continue;
        }
        if out.iter().any(|t| t.name == name) {
            continue;
        }
        let description: String = v["description"]
            .as_str()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(APP_TOOL_DESCRIPTION_CHARS)
            .collect();
        let params = match &v["params"] {
            p if p.is_null() => Value::Null,
            p if p.to_string().len() <= APP_TOOL_PARAMS_BYTES => p.clone(),
            _ => Value::Null,
        };
        out.push(AppTool {
            name,
            description,
            params,
            write: v["effect"].as_str() == Some("write"),
        });
        if out.len() >= MAX_APP_TOOLS {
            break;
        }
    }
    out
}

/// 연결된 클라이언트 하나.
struct Conn {
    id: u64,
    tx: mpsc::UnboundedSender<Value>,
    /// 앱이 hello에서 알린 기능(데몬 → 앱 `invoke`에 쓴다).
    features: Vec<String>,
    /// 앱이 Lucia에게 내주는 기능(결정 0053).
    tools: Vec<AppTool>,
    /// 앱이 뒤로 갔다고 알렸다(연결이 살아 있어도 푸시로 깨운다).
    background: bool,
}

/// 데몬이 앱에 부탁하고 기다리는 답(호출 ID → (클라이언트, 답 통로)).
type Pending = HashMap<String, (String, oneshot::Sender<Result<Value, CallError>>)>;

/// 접속 API. 연결을 받고, 앱 대화로 가는 글을 그 연결로 보낸다.
pub struct Gateway {
    store: Arc<Mutex<Store>>,
    inbound: mpsc::Sender<Inbound>,
    presses: mpsc::Sender<ButtonPress>,
    conns: Mutex<HashMap<String, Conn>>,
    failures: Mutex<HashMap<IpAddr, (u32, Instant)>>,
    next_conn: AtomicU64,
    /// (핑 간격, 무응답 한도)
    heartbeat: (Duration, Duration),
    /// 기기·앱 이벤트를 넣는 이벤트 입구(결정 0031). 없으면 기기 연결을 받지 않는다.
    events: Mutex<Option<mpsc::UnboundedSender<crate::events::Poke>>>,
    /// 믿는 프록시와 외부 연결 허용(결정 0043).
    public: Mutex<(crate::public::Proxies, bool)>,
    /// 앱이 올리고 받는 파일(결정 0043).
    files: OnceLock<Arc<crate::files::FileStore>>,
    /// 앱의 명령(`/권한`)을 승인 데스크로(결정 0040·0043).
    commands: OnceLock<mpsc::Sender<lucia_channel::SlashCommand>>,
    /// 작업 큐와, 청한 세션 정리를 기다리는 시간(결정 0056).
    jobs: OnceLock<(mpsc::Sender<crate::worker::Job>, Duration)>,
    /// 앱이 청한 세션 정리가 큐에 있거나 도는 중이다.
    compacting: std::sync::atomic::AtomicBool,
    /// 푸시 작업자에게(결정 0043).
    push: OnceLock<mpsc::UnboundedSender<PushJob>>,
    /// 푸시 자격 증명이 있어 보낼 수 있다.
    push_ready: AtomicBool,
    /// 앱이 부르는 메서드 등록표.
    methods: Mutex<HashMap<&'static str, Arc<dyn AppMethod>>>,
    pending: Mutex<Pending>,
    /// 앱 이모지·스티커 목록(결정 0045). 없으면 그 기능이 없는 데몬으로 보인다.
    expressions: OnceLock<Arc<crate::expressions::Expressions>>,
    /// Lucia의 프로필 이미지(결정 0060).
    profile: OnceLock<Arc<crate::profile::Profile>>,
    /// 화면을 넘겨받을 브라우저(결정 0063). 브라우저를 켠 데몬에만 있다.
    handover: OnceLock<Arc<dyn Handover>>,
}

impl Gateway {
    pub fn new(
        store: Arc<Mutex<Store>>,
        inbound: mpsc::Sender<Inbound>,
        presses: mpsc::Sender<ButtonPress>,
    ) -> Arc<Self> {
        Self::build(store, inbound, presses, (PING_EVERY, IDLE_LIMIT))
    }

    fn build(
        store: Arc<Mutex<Store>>,
        inbound: mpsc::Sender<Inbound>,
        presses: mpsc::Sender<ButtonPress>,
        heartbeat: (Duration, Duration),
    ) -> Arc<Self> {
        let gw = Arc::new(Self {
            store,
            inbound,
            presses,
            conns: Mutex::new(HashMap::new()),
            events: Mutex::new(None),
            failures: Mutex::new(HashMap::new()),
            next_conn: AtomicU64::new(1),
            heartbeat,
            public: Mutex::new((crate::public::Proxies::default(), false)),
            files: OnceLock::new(),
            commands: OnceLock::new(),
            jobs: OnceLock::new(),
            compacting: std::sync::atomic::AtomicBool::new(false),
            push: OnceLock::new(),
            push_ready: AtomicBool::new(false),
            methods: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            expressions: OnceLock::new(),
            profile: OnceLock::new(),
            handover: OnceLock::new(),
        });
        methods::register_builtin(&gw);
        gw
    }

    /// 시험용: 핑 간격과 무응답 한도를 줄인다.
    #[cfg(test)]
    pub fn with_heartbeat(
        store: Arc<Mutex<Store>>,
        inbound: mpsc::Sender<Inbound>,
        presses: mpsc::Sender<ButtonPress>,
        ping_every: Duration,
        idle_limit: Duration,
    ) -> Arc<Self> {
        Self::build(store, inbound, presses, (ping_every, idle_limit))
    }

    /// 기기 이벤트를 받을 이벤트 입구를 잇는다(결정 0031).
    pub fn set_events(&self, events: mpsc::UnboundedSender<crate::events::Poke>) {
        *self.events.lock().expect("events lock") = Some(events);
    }

    fn events(&self) -> Option<mpsc::UnboundedSender<crate::events::Poke>> {
        self.events.lock().expect("events lock").clone()
    }

    /// 믿는 프록시와 외부 연결 허용(설정 `[public]`, 결정 0043).
    pub fn set_public(&self, proxies: crate::public::Proxies, open: bool) {
        *self.public.lock().expect("public lock") = (proxies, open);
    }

    pub fn set_files(&self, files: Arc<crate::files::FileStore>) {
        let _ = self.files.set(files);
    }

    /// 앱 이모지·스티커(결정 0045). 목록이 바뀌면 연결된 앱에 번호 없는 `expressions` 프레임을 보낸다.
    pub fn set_expressions(self: &Arc<Self>, expressions: Arc<crate::expressions::Expressions>) {
        let mut changes = expressions.subscribe();
        if self.expressions.set(expressions).is_err() {
            return;
        }
        let gw = Arc::downgrade(self);
        tokio::spawn(async move {
            // 처음 값은 연결할 때 `welcome`으로 간다.
            changes.borrow_and_update();
            while changes.changed().await.is_ok() {
                let version = changes.borrow_and_update().clone();
                let Some(gw) = gw.upgrade() else { return };
                gw.broadcast(json!({"type": "expressions", "version": version}));
            }
        });
    }

    pub fn expressions(&self) -> Option<&Arc<crate::expressions::Expressions>> {
        self.expressions.get()
    }

    /// Lucia의 프로필(결정 0060·0064): 그림이나 이름이 바뀌면 연결된 앱에 번호 없는 `profile` 프레임을 보낸다
    /// (꺼져 있던 앱은 다음 `welcome`에서 안다). 프레임은 늘 지금의 이름과 그림 전체를 싣는다.
    pub fn set_profile(self: &Arc<Self>, profile: Arc<crate::profile::Profile>) {
        let mut avatars = profile.subscribe();
        let mut names = profile.subscribe_name();
        if self.profile.set(profile).is_err() {
            return;
        }
        let gw = Arc::downgrade(self);
        tokio::spawn(async move {
            // 처음 값은 연결할 때 `welcome`으로 간다.
            avatars.borrow_and_update();
            names.borrow_and_update();
            loop {
                let closed = tokio::select! {
                    r = avatars.changed() => r.is_err(),
                    r = names.changed() => r.is_err(),
                };
                if closed {
                    return;
                }
                avatars.borrow_and_update();
                names.borrow_and_update();
                let Some(gw) = gw.upgrade() else { return };
                let Some(profile) = gw.profile() else { return };
                let mut frame = profile.view();
                frame["type"] = json!("profile");
                gw.broadcast(frame);
            }
        });
    }

    pub fn profile(&self) -> Option<&Arc<crate::profile::Profile>> {
        self.profile.get()
    }

    /// 화면 넘겨받기를 잇는다(브라우저를 켠 데몬이 뜰 때 한 번, 결정 0063). 그 메서드들은 이때 등록되므로
    /// 브라우저가 없는 데몬의 `welcome.methods`에는 보이지 않는다.
    pub fn set_handover(&self, browser: Arc<dyn Handover>) {
        if self.handover.set(browser).is_ok() {
            handover::register(self);
        }
    }

    pub fn handover(&self) -> Option<&Arc<dyn Handover>> {
        self.handover.get()
    }

    /// 연결된 모든 앱에 번호 없는 프레임을 보낸다(상자에 넣지 않는다). 기기 연결은 이 표에 없다.
    fn broadcast(&self, frame: Value) {
        let conns = self.conns.lock().expect("conns lock");
        for conn in conns.values() {
            let _ = conn.tx.send(frame.clone());
        }
    }

    pub fn set_commands(&self, commands: mpsc::Sender<lucia_channel::SlashCommand>) {
        let _ = self.commands.set(commands);
    }

    /// 작업 큐를 잇는다(결정 0056). `compact_wait`는 앱이 청한 세션 정리의 답을 기다리는 시간이다
    /// (앞선 실행 하나와 정리 하나가 각각 실행 시간 제한을 다 써도 들어오게 잡는다).
    pub fn set_jobs(&self, jobs: mpsc::Sender<crate::worker::Job>, compact_wait: Duration) {
        let _ = self.jobs.set((jobs, compact_wait));
    }

    /// 푸시 작업자를 잇는다. 자격 증명이 생기거나 사라지면 [`Self::set_push_ready`]로 알린다.
    pub fn set_push(&self, jobs: mpsc::UnboundedSender<PushJob>) {
        let _ = self.push.set(jobs);
    }

    pub fn set_push_ready(&self, ready: bool) {
        self.push_ready.store(ready, Ordering::SeqCst);
    }

    pub fn push_ready(&self) -> bool {
        self.push_ready.load(Ordering::SeqCst) && self.push.get().is_some()
    }

    /// 앱이 부를 메서드를 더한다(같은 이름이면 바꾼다). 앱에서만 되는 연동은 모듈마다 여기에 등록한다.
    pub fn register(&self, method: Arc<dyn AppMethod>) {
        self.methods
            .lock()
            .expect("methods lock")
            .insert(method.name(), method);
    }

    fn method(&self, name: &str) -> Option<Arc<dyn AppMethod>> {
        self.methods
            .lock()
            .expect("methods lock")
            .get(name)
            .cloned()
    }

    /// 이 클라이언트가 부를 수 있는 메서드 이름(범위를 거른다).
    fn method_names(&self, client: &lucia_store::Client) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self
            .methods
            .lock()
            .expect("methods lock")
            .values()
            .filter(|m| {
                m.scope()
                    .is_none_or(|s| client.scopes.iter().any(|c| c == s))
            })
            .map(|m| m.name())
            .collect();
        names.sort_unstable();
        names
    }

    /// 지금 연결되어 있는가.
    pub fn online(&self, client_id: &str) -> bool {
        self.conns
            .lock()
            .expect("conns lock")
            .get(client_id)
            .is_some_and(|c| !c.tx.is_closed())
    }

    /// 끊겨 있어도 푸시로 깨울 수 있는가(자격 증명 + 그 앱의 등록 토큰).
    pub fn can_push(&self, client_id: &str) -> bool {
        self.push_ready() && self.push_token(client_id).is_some()
    }

    fn push_token(&self, client_id: &str) -> Option<String> {
        self.store
            .lock()
            .expect("store lock")
            .active_client(client_id)
            .ok()
            .flatten()
            .and_then(|c| c.push_token)
    }

    /// 연결을 등록한다. 같은 클라이언트의 예전 연결은 닫힌다(보낼 통로를 버린다).
    #[cfg(test)]
    fn attach(&self, client_id: &str) -> (u64, mpsc::UnboundedReceiver<Value>) {
        self.attach_with(client_id, Vec::new(), Vec::new())
    }

    fn attach_with(
        &self,
        client_id: &str,
        features: Vec<String>,
        tools: Vec<AppTool>,
    ) -> (u64, mpsc::UnboundedReceiver<Value>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let id = self.next_conn.fetch_add(1, Ordering::SeqCst);
        self.conns.lock().expect("conns lock").insert(
            client_id.to_owned(),
            Conn {
                id,
                tx,
                features,
                tools,
                background: false,
            },
        );
        (id, rx)
    }

    /// 시험용: 실제 소켓 없이 연결을 등록한다.
    #[cfg(test)]
    pub fn attach_for_test(&self, client_id: &str) -> (u64, mpsc::UnboundedReceiver<Value>) {
        self.attach(client_id)
    }

    fn detach(&self, client_id: &str, conn_id: u64) {
        let mut conns = self.conns.lock().expect("conns lock");
        if conns.get(client_id).is_some_and(|c| c.id == conn_id) {
            conns.remove(client_id);
        }
        drop(conns);
        // 그 연결에 부탁하고 기다리던 일은 끊겼다고 답한다.
        let mut pending = self.pending.lock().expect("pending lock");
        let gone: Vec<String> = pending
            .iter()
            .filter(|(_, (c, _))| c == client_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            if let Some((_, tx)) = pending.remove(&id) {
                let _ = tx.send(Err(CallError::new("offline", "앱 연결이 끊겼다")));
            }
        }
    }

    /// 취소한 클라이언트의 연결을 끊는다.
    pub fn disconnect(&self, client_id: &str) {
        self.conns.lock().expect("conns lock").remove(client_id);
    }

    fn set_background(&self, client_id: &str, conn_id: u64, background: bool) {
        if let Some(c) = self
            .conns
            .lock()
            .expect("conns lock")
            .get_mut(client_id)
            .filter(|c| c.id == conn_id)
        {
            c.background = background;
        }
    }

    /// 지금 연결로만 보낸다(번호 없음: 답, 오류, 부탁).
    fn send_live(&self, client_id: &str, frame: Value) -> Result<(), ChannelError> {
        let conns = self.conns.lock().expect("conns lock");
        let conn = conns.get(client_id).ok_or(ChannelError::Closed)?;
        conn.tx.send(frame).map_err(|_| ChannelError::Closed)
    }

    /// 앱으로 갈 프레임을 상자에 넣고(번호를 매긴다) 연결돼 있으면 바로 보낸다. 끊겼거나 뒤로 가 있으면 푸시로
    /// 깨운다. 상자에 넣었거나 바로 보냈으면 `Ok`.
    fn deliver(
        &self,
        client_id: &str,
        kind: &str,
        mut frame: Value,
        preview: &str,
    ) -> Result<(), ChannelError> {
        let stored = self.store.lock().expect("store lock").outbox_push(
            client_id,
            kind,
            &frame.to_string(),
            Utc::now(),
        );
        let seq = match stored {
            Ok(seq) => {
                frame["seq"] = json!(seq);
                Some(seq)
            }
            Err(err) => {
                tracing::warn!(%err, client = client_id, "앱 프레임을 상자에 넣지 못했다");
                None
            }
        };
        let (sent, background) = {
            let conns = self.conns.lock().expect("conns lock");
            match conns.get(client_id) {
                Some(c) if !c.tx.is_closed() => (c.tx.send(frame).is_ok(), c.background),
                _ => (false, false),
            }
        };
        if let Some(seq) = seq
            && (!sent || background)
            && PUSH_KINDS.contains(&kind)
        {
            self.wake(client_id, kind, seq, preview);
        }
        if sent || seq.is_some() {
            Ok(())
        } else {
            Err(ChannelError::Closed)
        }
    }

    /// 푸시 작업자에게 깨우기를 맡긴다(토큰이 없거나 푸시를 쓸 수 없으면 넘어간다).
    fn wake(&self, client_id: &str, kind: &str, seq: i64, preview: &str) {
        let Some(tx) = self.push.get() else { return };
        if !self.push_ready.load(Ordering::SeqCst) {
            return;
        }
        let Some(token) = self.push_token(client_id) else {
            return;
        };
        let _ = tx.send(PushJob {
            client_id: client_id.to_owned(),
            token,
            kind: kind.to_owned(),
            seq,
            preview: preview.chars().take(200).collect(),
            high: kind == "card",
        });
    }

    /// Lucia가 부를 수 있는 앱 기능(결정 0053): 지금 연결된 앱 가운데, 사용자가 범위(`app_tools`)를 준 것만.
    /// (클라이언트 ID, 앱 이름, 기능).
    pub fn app_tools(&self) -> Vec<(String, String, AppTool)> {
        let offered: Vec<(String, Vec<AppTool>)> = {
            let conns = self.conns.lock().expect("conns lock");
            conns
                .iter()
                .filter(|(_, c)| !c.tx.is_closed() && !c.tools.is_empty())
                .map(|(id, c)| (id.clone(), c.tools.clone()))
                .collect()
        };
        let mut out = Vec::new();
        for (id, tools) in offered {
            let Some(client) = self.client(&id) else {
                continue;
            };
            if !client.scopes.iter().any(|s| s == scopes::APP_TOOLS) {
                continue;
            }
            out.extend(
                tools
                    .into_iter()
                    .map(|t| (id.clone(), client.name.clone(), t)),
            );
        }
        out.sort_by(|a, b| (&a.0, &a.2.name).cmp(&(&b.0, &b.2.name)));
        out
    }

    /// 앱 기능 하나를 부른다(결정 0053). 그 기능을 내준, 범위가 있는 연결된 앱에 부탁한다. `client`를 주면 그 앱만.
    /// 돌려주는 것은 (앱 이름, 결과).
    pub async fn call_app(
        &self,
        method: &str,
        params: Value,
        client: Option<&str>,
        human: bool,
    ) -> Result<(String, Value), CallError> {
        let offered = self.app_tools();
        let found = offered
            .iter()
            .find(|(id, _, t)| t.name == method && client.is_none_or(|c| c == id));
        let Some((id, name, tool)) = found else {
            return Err(CallError::new(
                "unsupported",
                "그 기능을 내주는 앱이 지금 연결되어 있지 않다(app_tools로 지금 쓸 수 있는 것을 본다)",
            ));
        };
        if tool.write && !human {
            return Err(CallError::new(
                "forbidden",
                "무엇을 바꾸는 앱 기능은 사용자가 대화 중일 때만 부른다",
            ));
        }
        let data = self.invoke(id, method, params, APP_CALL_TIMEOUT).await?;
        Ok((name.clone(), data))
    }

    /// 데몬이 앱에 일을 부탁하고 답을 기다린다(`invoke` → `result`). 앱이 hello에서 알린 기능(메서드 이름의
    /// 첫 마디, 예: `location.get` → `location`)만 부른다. 앱에서만 되는 연동(위치, 휴대폰 알림 등)이 쓴다.
    pub async fn invoke(
        &self,
        client_id: &str,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, CallError> {
        let feature = method.split('.').next().unwrap_or(method);
        let id = format!("d-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let (tx, rx) = oneshot::channel();
        {
            let conns = self.conns.lock().expect("conns lock");
            let conn = conns
                .get(client_id)
                .filter(|c| !c.tx.is_closed())
                .ok_or_else(|| CallError::new("offline", "앱이 연결되어 있지 않다"))?;
            if !conn.features.iter().any(|f| f == feature) {
                return Err(CallError::new(
                    "unsupported",
                    &format!("앱이 {feature} 기능을 알리지 않았다"),
                ));
            }
            self.pending
                .lock()
                .expect("pending lock")
                .insert(id.clone(), (client_id.to_owned(), tx));
            if conn
                .tx
                .send(json!({"type": "invoke", "id": id, "method": method, "params": params}))
                .is_err()
            {
                self.pending.lock().expect("pending lock").remove(&id);
                return Err(CallError::new("offline", "앱이 연결되어 있지 않다"));
            }
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CallError::new("offline", "앱 연결이 끊겼다")),
            Err(_) => {
                self.pending.lock().expect("pending lock").remove(&id);
                Err(CallError::new("timeout", "앱이 제때 답하지 않았다"))
            }
        }
    }

    /// 앱의 `result`: 그 앱에 부탁한 일에만 답으로 받는다.
    fn resolve(&self, client_id: &str, id: &str, result: Result<Value, CallError>) -> bool {
        let mut pending = self.pending.lock().expect("pending lock");
        match pending.get(id) {
            Some((c, _)) if c == client_id => {
                let (_, tx) = pending.remove(id).expect("방금 봤다");
                let _ = tx.send(result);
                true
            }
            _ => false,
        }
    }

    fn blocked(&self, ip: IpAddr) -> bool {
        let mut f = self.failures.lock().expect("failures lock");
        match f.get(&ip) {
            Some((_, since)) if since.elapsed() > FAILURE_WINDOW => {
                f.remove(&ip);
                false
            }
            Some((count, _)) => *count >= MAX_FAILURES,
            None => false,
        }
    }

    fn failed(&self, ip: IpAddr) {
        let mut f = self.failures.lock().expect("failures lock");
        if f.len() >= MAX_FAILURE_ENTRIES && !f.contains_key(&ip) {
            f.retain(|_, (_, since)| since.elapsed() <= FAILURE_WINDOW);
            if f.len() >= MAX_FAILURE_ENTRIES
                && let Some(oldest) = f.iter().min_by_key(|(_, (_, s))| *s).map(|(k, _)| *k)
            {
                f.remove(&oldest);
            }
        }
        let e = f.entry(ip).or_insert((0, Instant::now()));
        e.0 += 1;
    }

    /// 토큰으로 클라이언트를 찾는다.
    fn authenticate(&self, token: &str) -> Option<lucia_store::Client> {
        self.store
            .lock()
            .expect("store lock")
            .client_by_token_hash(&token_hash(token))
            .ok()
            .flatten()
    }

    fn client(&self, client_id: &str) -> Option<lucia_store::Client> {
        self.store
            .lock()
            .expect("store lock")
            .active_client(client_id)
            .ok()
            .flatten()
    }

    fn still_active(&self, client_id: &str) -> bool {
        self.client(client_id).is_some()
    }

    fn touch(&self, client_id: &str) {
        let _ = self
            .store
            .lock()
            .expect("store lock")
            .touch_client(client_id, Utc::now());
    }

    /// 요청의 출처를 보고 받을지 정한다(결정 0043). 받으면 실제 상대 주소.
    fn admit(&self, peer: SocketAddr, headers: &HeaderMap) -> Result<IpAddr, StatusCode> {
        let (proxies, open) = self.public.lock().expect("public lock").clone();
        match crate::public::resolve(peer.ip(), headers, &proxies) {
            Ok(crate::public::Origin::Local(ip)) if local_peer(ip) => Ok(ip),
            Ok(crate::public::Origin::Local(ip)) => {
                tracing::warn!(%ip, "집 밖 주소의 접속 API 연결, 거절");
                Err(StatusCode::FORBIDDEN)
            }
            Ok(crate::public::Origin::External(ip)) if open => Ok(ip),
            Ok(crate::public::Origin::External(ip)) => {
                tracing::warn!(%ip, "공개 주소를 정하지 않아 외부 연결을 거절");
                Err(StatusCode::FORBIDDEN)
            }
            Err(why) => {
                tracing::warn!(%peer, why, "접속 API: 프록시 요청을 거절했다");
                Err(StatusCode::BAD_REQUEST)
            }
        }
    }

    /// HTTP 라우터(`/v1/ws`, `/v1/health`, HTTP API).
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/v1/ws", get(ws_upgrade))
            .route("/v1/health", get(|| async { "ok" }))
            .merge(http::routes())
            .with_state(self.clone())
    }

    /// 설정한 주소마다 연다.
    pub async fn serve(
        self: &Arc<Self>,
        binds: &[SocketAddr],
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        for addr in binds {
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|e| anyhow::anyhow!("접속 API 바인드 {addr}: {e}"))?;
            let app = self
                .router()
                .into_make_service_with_connect_info::<SocketAddr>();
            let cancel = cancel.clone();
            let addr = *addr;
            tokio::spawn(async move {
                let result = axum::serve(listener, app)
                    .with_graceful_shutdown(async move { cancel.cancelled_owned().await })
                    .await;
                if let Err(err) = result {
                    tracing::error!(%err, %addr, "접속 API가 멈췄다");
                }
            });
            tracing::info!(%addr, "접속 API");
        }
        Ok(())
    }
}

async fn ws_upgrade(
    State(gw): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let ip = match gw.admit(peer, &headers) {
        Ok(ip) => ip,
        Err(status) => return status.into_response(),
    };
    ws.max_message_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| serve_socket(gw, socket, ip))
}

/// 클라이언트가 보내는 프레임.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    Hello {
        token: String,
        /// 이 번호 뒤의 프레임을 다시 받는다(앱이 마지막으로 처리한 `seq`).
        #[serde(default)]
        resume: Option<i64>,
        /// 앱이 할 수 있는 일(데몬 → 앱 `invoke`의 첫 마디).
        #[serde(default)]
        features: Vec<String>,
        /// Lucia에게 내주는 기능(결정 0053): `{name, description, params, effect}`.
        #[serde(default)]
        tools: Vec<Value>,
    },
    Message {
        #[serde(default)]
        text: String,
        /// 먼저 `POST /v1/files`로 올린 파일 ID.
        #[serde(default)]
        files: Vec<String>,
        /// 목록에 있는 스티커 ID 하나(결정 0045).
        #[serde(default)]
        sticker: Option<String>,
        /// 앱이 붙인 ID(`accepted`에 그대로 돌려준다).
        #[serde(default)]
        id: Option<String>,
    },
    Press {
        #[serde(rename = "ref")]
        message_ref: String,
        action: String,
        #[serde(default)]
        input: Option<String>,
        /// 여러 칸을 받는 버튼(`form`)의 값(결정 0063). 칸 이름 → 값.
        #[serde(default)]
        inputs: Option<HashMap<String, FormValue>>,
    },
    /// 여기까지 받았다(누적).
    Ack {
        seq: i64,
    },
    /// 앱이 앞(`foreground`)이나 뒤(`background`)로 갔다.
    Presence {
        state: String,
    },
    /// 앱 → 데몬 메서드 호출.
    Call {
        id: String,
        method: String,
        #[serde(default)]
        params: Value,
    },
    /// 데몬이 부탁한 일(`invoke`)의 답.
    Result {
        id: String,
        #[serde(default)]
        ok: bool,
        #[serde(default)]
        data: Value,
        #[serde(default)]
        error: Option<CallErrorFrame>,
    },
    /// 이벤트(기기, 앱의 연동). 사람의 말이 아니라 외부 데이터로 신호 처리에 들어간다(결정 0031).
    Event {
        #[serde(default)]
        key: Option<String>,
        text: String,
    },
    Ping,
}

#[derive(Debug, Deserialize)]
struct CallErrorFrame {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

fn error_frame(code: &str, message: &str) -> Value {
    json!({"type": "error", "code": code, "message": message})
}

/// hello의 기능 이름을 다듬는다(소문자·숫자·`_-`, 개수·길이 제한).
fn clean_features(list: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = list
        .into_iter()
        .map(|f| f.trim().to_ascii_lowercase())
        .filter(|f| {
            !f.is_empty()
                && f.len() <= MAX_FEATURE_CHARS
                && f.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
        .collect();
    out.sort();
    out.dedup();
    out.truncate(MAX_FEATURES);
    out
}

/// 한 시간 이벤트 상한(연결마다).
struct EventBudget(VecDeque<Instant>);

impl EventBudget {
    fn take(&mut self) -> bool {
        while self
            .0
            .front()
            .is_some_and(|t| t.elapsed() > Duration::from_secs(3600))
        {
            self.0.pop_front();
        }
        if self.0.len() >= EVENTS_PER_HOUR {
            return false;
        }
        self.0.push_back(Instant::now());
        true
    }
}

/// 이벤트 하나를 이벤트 입구로.
fn take_event(
    gw: &Gateway,
    budget: &mut EventBudget,
    source: &str,
    key: Option<&str>,
    text: &str,
) -> Value {
    let Some(ev) = crate::events::ExternalEvent::new(source, key, text, Utc::now()) else {
        return error_frame("empty", "빈 이벤트");
    };
    let Some(tx) = gw.events() else {
        return error_frame("unsupported", "이 데몬은 이벤트를 받지 않는다");
    };
    if !budget.take() {
        return error_frame("too_many_events", "한 시간 이벤트 상한을 넘었다");
    }
    if tx.send(crate::events::Poke::Event(ev)).is_ok() {
        json!({"type": "accepted"})
    } else {
        error_frame("closed", "데몬이 멈추는 중")
    }
}

async fn serve_socket(gw: Arc<Gateway>, socket: WebSocket, ip: IpAddr) {
    let (mut sink, mut stream) = socket.split();
    let send = |v: Value| Message::Text(v.to_string().into());

    // 1. 인증(첫 프레임).
    if gw.blocked(ip) {
        let _ = sink
            .send(send(error_frame(
                "too_many_attempts",
                "인증 실패가 많아 잠시 막았다",
            )))
            .await;
        return;
    }
    let first = tokio::time::timeout(HELLO_TIMEOUT, stream.next()).await;
    let (token, resume, features, tools) = match first {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<ClientFrame>(&text) {
            Ok(ClientFrame::Hello {
                token,
                resume,
                features,
                tools,
            }) => (token, resume, features, tools),
            _ => {
                let _ = sink
                    .send(send(error_frame("hello_required", "첫 프레임은 hello")))
                    .await;
                return;
            }
        },
        _ => return,
    };
    let Some(client) = gw.authenticate(&token) else {
        gw.failed(ip);
        tracing::warn!(%ip, "접속 API 인증 실패");
        let _ = sink
            .send(send(error_frame(
                "unauthorized",
                "토큰이 틀렸거나 취소됐다",
            )))
            .await;
        return;
    };
    match Role::parse(&client.role) {
        Some(Role::App) => {}
        // 기기(결정 0021의 자리, 결정 0031 밑작업): 이벤트 프레임만 받는다. 이벤트 입구가 없으면 받지 않는다.
        Some(Role::Device) if gw.events().is_some() => {
            serve_device(&gw, sink, stream, &client).await;
            return;
        }
        _ => {
            let _ = sink
                .send(send(error_frame(
                    "unsupported_role",
                    "이 역할의 연결은 아직 받지 않는다",
                )))
                .await;
            return;
        }
    }

    // 내주는 기능의 첫 마디는 기능 이름으로도 친다(따로 적지 않아도 부를 수 있게).
    let tools = clean_tools(tools);
    let features = clean_features(
        features
            .into_iter()
            .chain(
                tools
                    .iter()
                    .filter_map(|t| t.name.split('.').next().map(str::to_owned)),
            )
            .collect(),
    );
    let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
    let (conn_id, mut rx) = gw.attach_with(&client.id, features.clone(), tools);
    gw.touch(&client.id);
    tracing::info!(client = %client.id, name = %client.name, "앱 연결");
    let conversation = conversation(&client.id);
    let last_seq = gw
        .store
        .lock()
        .expect("store lock")
        .outbox_last(&client.id)
        .unwrap_or(0);
    let mut welcome = welcome_frame(&gw, &client, &conversation, last_seq, &features);
    // 받아들인 기능 이름(결정 0053). Lucia가 실제로 부르려면 이 앱에 `app_tools` 범위가 있어야 한다.
    welcome["tools"] = json!(tool_names);
    if sink.send(send(welcome)).await.is_err() {
        gw.detach(&client.id, conn_id);
        return;
    }
    // 다시 받기: 앱이 처리한 번호 뒤의 프레임. 그 사이 새로 온 프레임과 겹칠 수 있다(앱은 본 번호를 버린다).
    if let Some(from) = resume {
        let frames = gw
            .store
            .lock()
            .expect("store lock")
            .outbox_after(&client.id, from, REPLAY_MAX)
            .unwrap_or_default();
        let gap = frames.first().is_some_and(|f| f.seq > from + 1)
            || (frames.is_empty() && last_seq > from);
        let truncated = gap || frames.len() >= REPLAY_MAX;
        for f in &frames {
            let mut v: Value = serde_json::from_str(&f.frame).unwrap_or(Value::Null);
            if !v.is_object() {
                continue;
            }
            v["seq"] = json!(f.seq);
            v["replay"] = json!(true);
            if sink.send(send(v)).await.is_err() {
                gw.detach(&client.id, conn_id);
                return;
            }
        }
        let resumed = json!({
            "type": "resumed",
            "from": from,
            "to": frames.last().map_or(from, |f| f.seq),
            "last": last_seq,
            "truncated": truncated,
        });
        if sink.send(send(resumed)).await.is_err() {
            gw.detach(&client.id, conn_id);
            return;
        }
    }

    // 2. 주고받기. 핑을 보내고, 오래 아무것도 듣지 못하면 끊는다.
    let (ping_every, idle_limit) = gw.heartbeat;
    let mut ping = tokio::time::interval(ping_every);
    ping.tick().await;
    let mut last_heard = Instant::now();
    let mut budget = EventBudget(VecDeque::new());
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if last_heard.elapsed() > idle_limit {
                    tracing::info!(client = %client.id, "앱이 응답하지 않아 끊는다");
                    break;
                }
                if sink.send(Message::Ping(Default::default())).await.is_err() {
                    break;
                }
            }
            out = rx.recv() => {
                // 통로가 닫혔으면 새 연결이 이 연결을 대신했거나 취소됐다.
                let Some(frame) = out else { break };
                if sink.send(send(frame)).await.is_err() {
                    break;
                }
            }
            incoming = stream.next() => {
                last_heard = Instant::now();
                let text = match incoming {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    Some(Ok(Message::Binary(_))) => {
                        let _ = sink.send(send(error_frame("bad_frame", "글 프레임(JSON)만 받는다"))).await;
                        continue;
                    }
                    _ => break,
                };
                let Some(current) = gw.client(&client.id) else {
                    let _ = sink.send(send(error_frame("unauthorized", "토큰이 취소됐다"))).await;
                    break;
                };
                gw.touch(&client.id);
                let reply = handle_frame(&gw, &current, conn_id, &conversation, &mut budget, &text).await;
                if let Some(reply) = reply
                    && sink.send(send(reply)).await.is_err()
                {
                    break;
                }
            }
        }
    }
    gw.detach(&client.id, conn_id);
    tracing::info!(client = %client.id, "앱 연결 끊김");
}

fn welcome_frame(
    gw: &Gateway,
    client: &lucia_store::Client,
    conversation: &ConversationId,
    last_seq: i64,
    features: &[String],
) -> Value {
    let notify = notify_client(&gw.store.lock().expect("store lock"));
    let mut frame = json!({
        "type": "welcome",
        "v": PROTOCOL_VERSION,
        "client": client.id,
        "name": client.name,
        "role": client.role,
        "conversation": conversation.0,
        "seq": last_seq,
        "scopes": client.scopes,
        "methods": gw.method_names(client),
        "features": features,
        "notify_target": notify.as_deref() == Some(client.id.as_str()),
        "push": {
            "available": gw.push_ready(),
            "registered": client.push_token.is_some(),
        },
        "capabilities": {
            "max_message_chars": MAX_MESSAGE_CHARS,
            "buttons": true,
            "edit": true,
            "prompts": true,
            "forms": true,
            "attachments": gw.files.get().is_some(),
            "max_upload_bytes": crate::files::MAX_DOWNLOAD,
            "max_message_files": MAX_MESSAGE_FILES,
        },
    });
    // 이모지·스티커(결정 0045): 이 필드가 없으면 앱은 그 기능이 없는 데몬으로 본다.
    if let Some(x) = gw.expressions() {
        frame["expressions"] = x.catalog().summary();
    }
    // 프로필 이미지(결정 0060): 이 필드가 없으면 그 기능이 없는 데몬이고, `avatar`가 null이면 정한 그림이 없다.
    if let Some(p) = gw.profile() {
        frame["profile"] = p.view();
    }
    frame
}

/// 기기 연결: `event`와 `ping`만 받는다. 말 보내기·카드 누르기는 사람의 일이라 받지 않는다.
async fn serve_device(
    gw: &Gateway,
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    mut stream: futures_util::stream::SplitStream<WebSocket>,
    client: &lucia_store::Client,
) {
    let send = |v: Value| Message::Text(v.to_string().into());
    gw.touch(&client.id);
    tracing::info!(client = %client.id, name = %client.name, "기기 연결");
    let welcome = json!({
        "type": "welcome",
        "v": PROTOCOL_VERSION,
        "client": client.id,
        "name": client.name,
        "role": client.role,
        "capabilities": { "events": true, "max_event_chars": crate::events::EVENT_TEXT_MAX },
    });
    if sink.send(send(welcome)).await.is_err() {
        return;
    }
    let (ping_every, idle_limit) = gw.heartbeat;
    let mut ping = tokio::time::interval(ping_every);
    ping.tick().await;
    let mut last_heard = Instant::now();
    let mut budget = EventBudget(VecDeque::new());
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if last_heard.elapsed() > idle_limit
                    || sink.send(Message::Ping(Default::default())).await.is_err()
                {
                    break;
                }
            }
            incoming = stream.next() => {
                last_heard = Instant::now();
                let text = match incoming {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    Some(Ok(Message::Binary(_))) => {
                        let _ = sink.send(send(error_frame("bad_frame", "글 프레임(JSON)만 받는다"))).await;
                        continue;
                    }
                    _ => break,
                };
                if !gw.still_active(&client.id) {
                    let _ = sink.send(send(error_frame("unauthorized", "토큰이 취소됐다"))).await;
                    break;
                }
                gw.touch(&client.id);
                let reply = match serde_json::from_str::<ClientFrame>(&text) {
                    Ok(ClientFrame::Ping) => json!({"type": "pong"}),
                    Ok(ClientFrame::Event { key, text }) => {
                        take_event(gw, &mut budget, &client.name, key.as_deref(), &text)
                    }
                    Ok(_) => error_frame("forbidden", "기기는 이벤트만 보낸다"),
                    Err(_) => error_frame("bad_frame", "모르는 프레임"),
                };
                if sink.send(send(reply)).await.is_err() {
                    break;
                }
            }
        }
    }
    tracing::info!(client = %client.id, "기기 연결 끊김");
}

/// 프레임 하나를 처리한다. 바로 돌려줄 것이 있으면 돌려준다.
async fn handle_frame(
    gw: &Arc<Gateway>,
    client: &lucia_store::Client,
    conn_id: u64,
    conversation: &ConversationId,
    budget: &mut EventBudget,
    text: &str,
) -> Option<Value> {
    let frame = match serde_json::from_str::<ClientFrame>(text) {
        Ok(f) => f,
        Err(_) => return Some(error_frame("bad_frame", "모르는 프레임")),
    };
    let client_id = client.id.as_str();
    match frame {
        ClientFrame::Ping => Some(json!({"type": "pong"})),
        ClientFrame::Hello { .. } => Some(error_frame("bad_frame", "이미 인증했다")),
        ClientFrame::Ack { seq } => {
            let _ = gw
                .store
                .lock()
                .expect("store lock")
                .ack_client(client_id, seq);
            None
        }
        ClientFrame::Presence { state } => match state.as_str() {
            "foreground" | "background" => {
                gw.set_background(client_id, conn_id, state == "background");
                None
            }
            _ => Some(error_frame("bad_frame", "state는 foreground나 background")),
        },
        ClientFrame::Event { key, text } => {
            // 앱의 연동(휴대폰 알림 등)이 보내는 이벤트: 소유자의 앱이어도 글은 외부 데이터다.
            Some(take_event(gw, budget, &client.name, key.as_deref(), &text))
        }
        ClientFrame::Call { id, method, params } => {
            // 오래 걸릴 수 있어 떨어진 작업으로 돌리고, 답은 이 연결로 보낸다(그 사이 다른 프레임도 오간다).
            let gw = gw.clone();
            let client = client.clone();
            tokio::spawn(async move {
                let result = methods::dispatch(&gw, &client, &method, params).await;
                let frame = match result {
                    Ok(data) => json!({"type": "result", "id": id, "ok": true, "data": data}),
                    Err(e) => {
                        json!({"type": "result", "id": id, "ok": false, "error": e.to_json()})
                    }
                };
                let _ = gw.send_live(&client.id, frame);
            });
            None
        }
        ClientFrame::Result {
            id,
            ok,
            data,
            error,
        } => {
            let result = if ok {
                Ok(data)
            } else {
                let e = error.unwrap_or(CallErrorFrame {
                    code: "failed".into(),
                    message: String::new(),
                });
                Err(CallError::owned(&e.code, &e.message))
            };
            if gw.resolve(client_id, &id, result) {
                None
            } else {
                Some(error_frame("bad_ref", "기다리는 부탁이 아니다"))
            }
        }
        ClientFrame::Message {
            text,
            files,
            sticker,
            id,
        } => {
            let text = text.trim();
            let sticker = sticker
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty());
            if text.is_empty() && files.is_empty() && sticker.is_none() {
                return Some(error_frame("empty", "빈 말"));
            }
            if text.chars().count() > MAX_INBOUND_CHARS {
                return Some(error_frame("too_long", "말이 너무 길다"));
            }
            // 스티커는 그림 대신 이름과 설명 한 줄로 Lucia에게 넘긴다(기록에도 그 줄로 남는다).
            let mut body = text.to_owned();
            if let Some(sticker_id) = &sticker {
                let line = gw.expressions().and_then(|x| {
                    x.catalog()
                        .sticker(sticker_id)
                        .map(crate::expressions::sticker_line)
                });
                let Some(line) = line else {
                    return Some(error_frame("bad_sticker", "목록에 없는 스티커"));
                };
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(&line);
            }
            let attachments = match attachments(gw, conversation, &files) {
                Ok(a) => a,
                Err(e) => return Some(error_frame("bad_file", &e)),
            };
            let msg = Inbound {
                platform: PLATFORM.into(),
                sender: client_id.to_owned(),
                conversation: conversation.clone(),
                body,
                attachments,
                received_at: Utc::now(),
                reply_to: None,
                venue: None,
            };
            if gw.inbound.send(msg).await.is_err() {
                return Some(error_frame("closed", "데몬이 멈추는 중"));
            }
            Some(json!({"type": "accepted", "id": id}))
        }
        ClientFrame::Press {
            message_ref,
            action,
            input,
            inputs,
        } => {
            // 자기 카드의 버튼만 누른다.
            if client_of(&message_ref) != Some(client_id) {
                return Some(error_frame("bad_ref", "이 연결의 카드가 아니다"));
            }
            // 여러 칸의 값(결정 0063): 값을 다듬지 않는다(비밀번호의 앞뒤 공백도 값이다). 기록하지 않는다.
            let fields = match inputs.map(form_values) {
                None => None,
                Some(Ok(fields)) => Some(fields),
                Some(Err(why)) => return Some(error_frame("bad_frame", why)),
            };
            let press = ButtonPress {
                platform: PLATFORM.into(),
                sender: client_id.to_owned(),
                conversation: conversation.clone(),
                message_ref,
                action,
                input: input
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty())
                    .map(SecretText::new),
                fields,
                received_at: Utc::now(),
            };
            if gw.presses.send(press).await.is_err() {
                return Some(error_frame("closed", "데몬이 멈추는 중"));
            }
            None
        }
    }
}

/// `press.inputs`의 값 하나: 글 또는 켜고 끄는 값. `Debug`에 값이 찍히지 않는다.
#[derive(Deserialize)]
#[serde(untagged)]
enum FormValue {
    Text(String),
    Check(bool),
}

impl std::fmt::Debug for FormValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FormValue(…)")
    }
}

/// 한 번에 받는 칸 수와 칸 이름·값의 길이 상한(문자 수).
const FORM_FIELDS_MAX: usize = 8;
const FORM_KEY_MAX: usize = 40;
const FORM_VALUE_MAX: usize = 1000;

fn form_values(
    inputs: HashMap<String, FormValue>,
) -> Result<std::collections::BTreeMap<String, SecretText>, &'static str> {
    if inputs.len() > FORM_FIELDS_MAX {
        return Err("입력 칸이 너무 많다");
    }
    let mut out = std::collections::BTreeMap::new();
    for (key, value) in inputs {
        let value = match value {
            FormValue::Text(v) => v,
            FormValue::Check(v) => v.to_string(),
        };
        if key.is_empty()
            || key.chars().count() > FORM_KEY_MAX
            || value.chars().count() > FORM_VALUE_MAX
        {
            return Err("입력 칸의 이름이나 값이 너무 길다");
        }
        out.insert(key, SecretText::new(value));
    }
    Ok(out)
}

/// 말에 붙인 파일 ID를 첨부로. 이 앱 대화로 올린 파일만 받는다.
fn attachments(
    gw: &Gateway,
    conversation: &ConversationId,
    ids: &[String],
) -> Result<Vec<InboundAttachment>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    if ids.len() > MAX_MESSAGE_FILES {
        return Err(format!("파일은 {MAX_MESSAGE_FILES}개까지"));
    }
    let files = gw.files.get().ok_or("이 데몬은 파일을 받지 않는다")?;
    ids.iter()
        .map(|id| {
            let f = files
                .get(id)
                .filter(|f| {
                    f.direction == "in" && f.conversation.as_deref() == Some(&conversation.0)
                })
                .ok_or_else(|| format!("올린 파일이 아니다: {id}"))?;
            Ok(InboundAttachment {
                url: format!("{}{}", crate::files::APP_FILE_SCHEME, f.id),
                filename: f.name,
                content_type: Some(f.mime),
                size: f.size,
            })
        })
        .collect()
}

fn tone(t: CardTone) -> &'static str {
    match t {
        CardTone::Pending => "pending",
        CardTone::Success => "success",
        CardTone::Danger => "danger",
        CardTone::Warning => "warning",
        CardTone::Neutral => "neutral",
    }
}

/// 카드를 프레임 안의 JSON으로.
fn card_json(card: &Card) -> Value {
    json!({
        "headline": card.headline,
        "title": card.title,
        "fields": card.fields.iter().map(|f| json!({"name": f.name, "value": f.value, "inline": f.inline})).collect::<Vec<_>>(),
        "body": card.body,
        "footer": card.footer,
        "tone": tone(card.tone),
        "buttons": card.buttons.iter().map(|b| json!({
            "action": b.action,
            "label": b.label,
            "primary": b.primary,
            "prompt": b.prompt.as_ref().map(|p| json!({
                "title": p.title,
                "label": p.label,
                "placeholder": p.placeholder,
                "min_len": p.min_len,
                "max_len": p.max_len,
            })),
            "form": b.form.as_ref().map(|f| json!({
                "title": f.title,
                "fields": f.fields.iter().map(|x| json!({
                    "key": x.key,
                    "label": x.label,
                    "kind": x.kind.as_str(),
                    "min_len": x.min_len,
                    "max_len": x.max_len,
                })).collect::<Vec<_>>(),
            })),
        })).collect::<Vec<_>>(),
    })
}

impl ChannelAdapter for Gateway {
    fn platform(&self) -> &str {
        PLATFORM
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            max_message_chars: MAX_MESSAGE_CHARS,
            buttons: true,
            edit: true,
            threads: false,
            prompts: true,
            forms: true,
        }
    }

    async fn send(&self, message: Outbound) -> Result<usize, ChannelError> {
        let client = client_of(&message.conversation().0)
            .ok_or_else(|| ChannelError::Send("앱 대화가 아니다".into()))?
            .to_owned();
        match message {
            Outbound::Text { body, .. } | Outbound::Note { body, .. } if body.trim().is_empty() => {
                Err(ChannelError::Empty)
            }
            Outbound::Text { body, .. } => {
                let parts = split_message(&body, MAX_MESSAGE_CHARS);
                for p in &parts {
                    self.deliver(&client, "text", json!({"type": "text", "body": p}), p)?;
                }
                Ok(parts.len())
            }
            Outbound::Note { body, .. } => {
                let parts = split_message(&body, MAX_MESSAGE_CHARS);
                for p in &parts {
                    self.deliver(&client, "note", json!({"type": "note", "body": p}), p)?;
                }
                Ok(parts.len())
            }
            Outbound::Files {
                body,
                files,
                conversation,
            } => {
                if files.is_empty() {
                    return Err(ChannelError::Empty);
                }
                check_files(&files)?;
                let mut parts = split_message(&body, MAX_MESSAGE_CHARS);
                let last = parts.pop().unwrap_or_default();
                for p in &parts {
                    self.deliver(&client, "text", json!({"type": "text", "body": p}), p)?;
                }
                // 파일은 저장해 두고 ID로 알린다(앱은 `GET /v1/files/<ID>`로 받는다). 상자에 큰 덩어리를 넣지 않는다.
                let store = self
                    .files
                    .get()
                    .ok_or_else(|| ChannelError::Send("파일 저장소가 없다".into()))?;
                let mut listed = Vec::new();
                for f in &files {
                    let rec = store
                        .save("out", &f.name, &f.bytes, Some(&conversation.0), Utc::now())
                        .map_err(ChannelError::Send)?;
                    listed.push(json!({
                        "id": rec.id,
                        "name": rec.name,
                        "size": rec.size,
                        "mime": rec.mime,
                        "kind": rec.kind,
                    }));
                }
                let preview = if last.trim().is_empty() {
                    format!("파일 {}개", listed.len())
                } else {
                    last.clone()
                };
                self.deliver(
                    &client,
                    "files",
                    json!({"type": "files", "body": last, "files": listed}),
                    &preview,
                )?;
                Ok(parts.len() + 1)
            }
        }
    }

    async fn typing(&self, conversation: &ConversationId) -> Result<(), ChannelError> {
        let client = client_of(&conversation.0).ok_or(ChannelError::Closed)?;
        // 입력 중 표시는 상자에 넣지 않는다(지나간 것을 다시 보일 까닭이 없다).
        match self.send_live(client, json!({"type": "typing"})) {
            Ok(()) | Err(ChannelError::Closed) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn send_card(&self, card: Card) -> Result<String, ChannelError> {
        let client = client_of(&card.conversation.0)
            .ok_or(ChannelError::Closed)?
            .to_owned();
        let message_ref = format!(
            "{}:{}",
            card.conversation.0,
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        );
        let preview = if card.title.is_empty() {
            card.headline.clone()
        } else {
            card.title.clone()
        };
        self.deliver(
            &client,
            "card",
            json!({"type": "card", "ref": message_ref, "card": card_json(&card)}),
            &preview,
        )?;
        Ok(message_ref)
    }

    /// 닫힘도 상자에 넣는다(다시 받을 때 열린 카드가 닫힌 것으로 바뀐다).
    async fn close_card(&self, message_ref: &str, card: &Card) -> Result<(), ChannelError> {
        let client = client_of(message_ref).ok_or(ChannelError::Closed)?;
        match self.deliver(
            client,
            "card_closed",
            json!({"type": "card_closed", "ref": message_ref, "card": card_json(card)}),
            "",
        ) {
            Ok(()) | Err(ChannelError::Closed) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// 앱 스티커(결정 0045): 번호 붙는 `sticker` 프레임. 상자·다시 받기·푸시가 글과 똑같이 적용된다.
    async fn send_sticker(
        &self,
        conversation: &ConversationId,
        sticker_id: &str,
    ) -> Result<(), ChannelError> {
        let client = client_of(&conversation.0).ok_or(ChannelError::Closed)?;
        let catalog = self
            .expressions()
            .ok_or(ChannelError::Unsupported("스티커"))?
            .catalog();
        let sticker = catalog
            .sticker(sticker_id)
            .ok_or_else(|| ChannelError::Send("목록에 없는 스티커".into()))?;
        self.deliver(
            client,
            "sticker",
            json!({"type": "sticker", "sticker": {"id": sticker.id, "name": sticker.name}}),
            "스티커를 보냈어요",
        )
    }

    fn ready(&self) -> bool {
        true
    }
}

/// 엔진이 받는 채널: 대화 ID로 Discord(또는 다른 기본 채널)와 앱을 가른다.
pub struct RouterChannel<D> {
    primary: Arc<D>,
    gateway: Arc<Gateway>,
    store: Arc<Mutex<Store>>,
    /// 설정 파일의 기본 알림 대화(관리 페이지 값이 없을 때).
    default_notify: Option<ConversationId>,
}

/// 보낼 곳.
enum Route {
    /// 이 앱 대화로(연결돼 있거나 푸시로 깨울 수 있다).
    App(ConversationId),
    Primary,
    /// 앱이 연결되어 있지 않고 푸시도 없어 기본 채널의 알림 대화로.
    Fallback(ConversationId),
}

impl<D: ChannelAdapter> RouterChannel<D> {
    pub fn new(
        primary: Arc<D>,
        gateway: Arc<Gateway>,
        store: Arc<Mutex<Store>>,
        default_notify: Option<ConversationId>,
    ) -> Self {
        Self {
            primary,
            gateway,
            store,
            default_notify,
        }
    }

    /// 기본 채널의 알림 대화(Discord DM).
    fn fallback_target(&self) -> Option<ConversationId> {
        self.store
            .lock()
            .expect("store lock")
            .setting(lucia_store::keys::NOTIFY_CONVERSATION)
            .ok()
            .flatten()
            .map(ConversationId)
            .or_else(|| self.default_notify.clone())
            .filter(|c| client_of(&c.0).is_none())
    }

    fn route(&self, conversation: &ConversationId) -> Result<Route, ChannelError> {
        let Some(client) = client_of(&conversation.0) else {
            return Ok(Route::Primary);
        };
        if self.gateway.online(client) || self.gateway.can_push(client) {
            return Ok(Route::App(conversation.clone()));
        }
        tracing::info!(
            client,
            "앱이 연결되어 있지 않고 푸시도 없어 알림 대화로 보낸다"
        );
        self.fallback_target()
            .map(Route::Fallback)
            .ok_or(ChannelError::Closed)
    }
}

impl<D> RouterChannel<D> {
    /// 앱이 아닌 곳으로 가는 글에서 앱 이모지 표기(`:이름:`)를 지운다(결정 0045: 그곳에서는 글자 그대로 보인다).
    /// 글이 표기뿐이었으면 그대로 둔다(빈 메시지는 보낼 수 없다).
    fn plain(&self, message: Outbound) -> Outbound {
        let Some(x) = self.gateway.expressions() else {
            return message;
        };
        let clean = |body: String| {
            let stripped = x.strip_emoji(&body);
            if stripped.trim().is_empty() {
                body
            } else {
                stripped
            }
        };
        match message {
            Outbound::Text { conversation, body } => Outbound::Text {
                conversation,
                body: clean(body),
            },
            Outbound::Note { conversation, body } => Outbound::Note {
                conversation,
                body: clean(body),
            },
            Outbound::Files {
                conversation,
                body,
                files,
            } => Outbound::Files {
                conversation,
                body: if body.trim().is_empty() {
                    body
                } else {
                    clean(body)
                },
                files,
            },
        }
    }
}

fn retarget(message: Outbound, to: ConversationId) -> Outbound {
    match message {
        Outbound::Text { body, .. } => Outbound::Text {
            conversation: to,
            body,
        },
        Outbound::Note { body, .. } => Outbound::Note {
            conversation: to,
            body,
        },
        Outbound::Files { body, files, .. } => Outbound::Files {
            conversation: to,
            body,
            files,
        },
    }
}

impl<D: ChannelAdapter> ChannelAdapter for RouterChannel<D> {
    fn platform(&self) -> &str {
        self.primary.platform()
    }

    /// 기본 채널의 능력(알림·작업 스레드는 기본 채널로 간다). 앱은 그 이상을 다 받는다.
    fn capabilities(&self) -> Capabilities {
        self.primary.capabilities()
    }

    async fn send(&self, message: Outbound) -> Result<usize, ChannelError> {
        match self.route(message.conversation())? {
            Route::App(to) => self.gateway.send(retarget(message, to)).await,
            Route::Primary => self.primary.send(self.plain(message)).await,
            Route::Fallback(to) => self.primary.send(self.plain(retarget(message, to))).await,
        }
    }

    async fn typing(&self, conversation: &ConversationId) -> Result<(), ChannelError> {
        match self.route(conversation)? {
            Route::App(to) => self.gateway.typing(&to).await,
            Route::Primary => self.primary.typing(conversation).await,
            Route::Fallback(to) => self.primary.typing(&to).await,
        }
    }

    async fn send_card(&self, mut card: Card) -> Result<String, ChannelError> {
        match self.route(&card.conversation)? {
            Route::App(to) => {
                card.conversation = to;
                self.gateway.send_card(card).await
            }
            Route::Primary => self.primary.send_card(card).await,
            Route::Fallback(to) => {
                card.conversation = to;
                self.primary.send_card(card).await
            }
        }
    }

    async fn close_card(&self, message_ref: &str, card: &Card) -> Result<(), ChannelError> {
        if client_of(message_ref).is_some() {
            self.gateway.close_card(message_ref, card).await
        } else if client_of(&card.conversation.0).is_some() {
            // 앱이 끊겨 있어 기본 채널로 보냈던 카드: 닫을 때도 그 대화로.
            let mut card = card.clone();
            card.conversation = self.fallback_target().ok_or(ChannelError::Closed)?;
            self.primary.close_card(message_ref, &card).await
        } else {
            self.primary.close_card(message_ref, card).await
        }
    }

    async fn open_thread(
        &self,
        parent: &ConversationId,
        name: &str,
    ) -> Result<ConversationId, ChannelError> {
        if client_of(&parent.0).is_some() {
            return Err(ChannelError::Unsupported("스레드"));
        }
        self.primary.open_thread(parent, name).await
    }

    /// 스티커: 앱 대화는 앱 목록의 스티커(결정 0045), 그 밖은 기본 채널의 서버 스티커(결정 0044).
    /// 앱이 끊겨 알림 대화로 돌릴 때는 보내지 않는다(그곳에는 앱 스티커가 없다).
    async fn send_sticker(
        &self,
        conversation: &ConversationId,
        sticker_id: &str,
    ) -> Result<(), ChannelError> {
        match self.route(conversation)? {
            Route::App(to) => self.gateway.send_sticker(&to, sticker_id).await,
            Route::Primary => self.primary.send_sticker(conversation, sticker_id).await,
            Route::Fallback(_) => Err(ChannelError::Unsupported("스티커")),
        }
    }

    /// 알림을 보낼 곳이 있는가: 기본 채널, 또는 알림 받을 앱.
    fn ready(&self) -> bool {
        self.primary.ready() || notify_client(&self.store.lock().expect("store lock")).is_some()
    }
}

#[cfg(test)]
mod tests;
