//! 앱이 부르는 메서드(`call`, 결정 0043). 앱에서만 되는 연동을 더할 때는 [`AppMethod`]를 구현해
//! [`Gateway::register`]로 등록한다(이 파일을 고치지 않아도 된다). 범위가 필요한 메서드는 [`AppMethod::scope`]를
//! 돌려주고, 그 범위가 없는 클라이언트에게는 보이지도 불리지도 않는다.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::{Gateway, conversation, scopes};

/// 메서드의 답.
pub type CallFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, CallError>> + Send + 'a>>;

/// 메서드 오류(앱에 `{code, message}`로 간다).
#[derive(Debug, Clone, PartialEq)]
pub struct CallError {
    pub code: String,
    pub message: String,
}

impl CallError {
    pub fn new(code: &str, message: &str) -> Self {
        Self::owned(code, message)
    }

    pub fn owned(code: &str, message: &str) -> Self {
        Self {
            code: code.chars().take(40).collect(),
            message: message.chars().take(500).collect(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"code": self.code, "message": self.message})
    }
}

/// 부르는 쪽.
pub struct CallContext<'a> {
    pub gateway: &'a Arc<Gateway>,
    pub client: &'a lucia_store::Client,
}

/// 앱이 부르는 메서드 하나.
pub trait AppMethod: Send + Sync {
    /// `history`, `location.report` 같은 이름(점으로 묶는다).
    fn name(&self) -> &'static str;
    /// 필요한 클라이언트 범위([`scopes`]). 없으면 모든 앱.
    fn scope(&self) -> Option<&'static str> {
        None
    }
    fn call<'a>(&'a self, ctx: CallContext<'a>, params: Value) -> CallFuture<'a>;
}

/// 메서드를 찾아 부른다(범위를 확인한다).
pub(super) async fn dispatch(
    gw: &Arc<Gateway>,
    client: &lucia_store::Client,
    name: &str,
    params: Value,
) -> Result<Value, CallError> {
    let method = gw
        .method(name)
        .ok_or_else(|| CallError::new("unknown_method", "모르는 메서드"))?;
    if let Some(scope) = method.scope()
        && !client.scopes.iter().any(|s| s == scope)
    {
        return Err(CallError::new("forbidden", "이 토큰에는 그 권한이 없다"));
    }
    method
        .call(
            CallContext {
                gateway: gw,
                client,
            },
            params,
        )
        .await
}

pub(super) fn register_builtin(gw: &Gateway) {
    gw.register(Arc::new(History));
    gw.register(Arc::new(Commands));
    gw.register(Arc::new(Command));
    gw.register(Arc::new(SessionCompact));
    gw.register(Arc::new(PushRegister));
    gw.register(Arc::new(PushUnregister));
}

// ---------- 대화 기록 ----------

/// 한 번에 돌려주는 기록 수 상한.
const HISTORY_MAX: usize = 100;

/// `history {before?, limit?, scope?: "app"|"owner"}`: 대화 기록(오래된 것부터). `app`은 모든 앱 대화(결정 0067),
/// `owner`는 Discord 알림 대화(DM)까지(같은 Lucia와 나눈 소유자의 대화).
struct History;

impl AppMethod for History {
    fn name(&self) -> &'static str {
        "history"
    }

    fn call<'a>(&'a self, ctx: CallContext<'a>, params: Value) -> CallFuture<'a> {
        Box::pin(async move { history(ctx.gateway, ctx.client, &params) })
    }
}

pub(super) fn history(
    gw: &Gateway,
    client: &lucia_store::Client,
    params: &Value,
) -> Result<Value, CallError> {
    let before = params["before"].as_i64();
    let limit = params["limit"]
        .as_u64()
        .map_or(50, |l| l as usize)
        .clamp(1, HISTORY_MAX);
    let owner = match params["scope"].as_str() {
        None | Some("app") => false,
        Some("owner") => true,
        Some(_) => return Err(CallError::new("bad_params", "scope는 app이나 owner")),
    };
    // 새 모양(결정 0066): 나뉘어 간 조각(`parts`)과 붙은 파일(`files`). 달라는 앱에만 준다.
    let shaped = match &params["shape"] {
        Value::Null => false,
        v if v.as_u64() == Some(1) => false,
        v if v.as_u64() == Some(2) => true,
        _ => return Err(CallError::new("bad_params", "shape는 1이나 2")),
    };
    let store = gw.store.lock().expect("store lock");
    let convs = history_conversations(&store, client, owner);
    // 하나 더 읽어 더 있는지 본다.
    let mut list = store
        .episodes_in_before(&convs, before, limit + 1)
        .map_err(|e| CallError::owned("failed", &e.to_string()))?;
    let more = list.len() > limit;
    if more {
        list.remove(0);
    }
    let shapes = if shaped {
        let ids: Vec<i64> = list.iter().map(|e| e.id).collect();
        store
            .episode_shapes(&ids)
            .map_err(|e| CallError::owned("failed", &e.to_string()))?
    } else {
        Default::default()
    };
    let items: Vec<Value> = list
        .iter()
        .map(|e| {
            let from = match e.origin {
                lucia_store::Origin::User => "user",
                lucia_store::Origin::Agent => "lucia",
                lucia_store::Origin::Daemon => "daemon",
            };
            let mut item = json!({
                "id": e.id,
                "at": e.at.to_rfc3339(),
                "from": from,
                "conversation": e.conversation,
                "body": e.body,
            });
            if let Some(shape) = shapes.get(&e.id) {
                if let Some(parts) = &shape.parts {
                    item["parts"] = json!(parts);
                }
                if !shape.files.is_empty() {
                    // 파일 설명 줄은 `files`와 두 번 보이지 않게 뺀다.
                    item["body"] = json!(without_attachment_line(&e.body));
                    item["files"] = shape
                        .files
                        .iter()
                        .map(|f| file_entry(gw, &store, client, f))
                        .collect();
                }
            }
            item
        })
        .collect();
    Ok(json!({"items": items, "more": more}))
}

/// 기록 항목의 파일 하나: 지금도 있으면 그대로, 지워졌으면 `"gone": true`. 이 토큰으로 받을 수 있는지(`reachable`).
fn file_entry(
    gw: &Gateway,
    store: &lucia_store::Store,
    client: &lucia_store::Client,
    f: &lucia_store::EpisodeFile,
) -> Value {
    let live = gw.files.get().and_then(|files| {
        // 저장소 잠금을 쥔 채로 FileStore::get을 부르면 같은 잠금을 다시 잡는다: 기록만 직접 본다.
        let rec = store.file(&f.id).ok().flatten()?;
        let expired = store
            .file_expiry(&f.id)
            .ok()
            .flatten()
            .is_some_and(|at| at <= chrono::Utc::now());
        (!expired && files.path(&rec).exists()).then_some(rec)
    });
    let mut v = json!({
        "id": f.id,
        "name": f.name,
        "size": f.size,
        "mime": f.mime,
        "kind": f.kind,
    });
    match live {
        Some(rec) => v["reachable"] = json!(can_download(store, client, &rec)),
        None => {
            v["gone"] = json!(true);
            v["reachable"] = json!(false);
        }
    }
    v
}

/// 본문 끝의 파일 설명 줄(`[첨부: …]`)을 뺀다(마지막 것 하나).
fn without_attachment_line(body: &str) -> String {
    let lines: Vec<&str> = body.split('\n').collect();
    let Some(at) = lines
        .iter()
        .rposition(|l| l.starts_with("[첨부: ") && l.ends_with(']'))
    else {
        return body.to_owned();
    };
    let mut kept: Vec<&str> = lines[..at].to_vec();
    kept.extend_from_slice(&lines[at + 1..]);
    kept.join("\n").trim_end().to_owned()
}

/// 기록에서 읽을 대화(결정 0067: 앱은 기기가 여럿이어도 한 대화). 앱 모두(`app:*`)와 모든 앱 대화(취소한 앱의 지난
/// 대화 포함). `owner`이면 Discord DM도.
pub(super) fn history_conversations(
    store: &lucia_store::Store,
    client: &lucia_store::Client,
    owner: bool,
) -> Vec<String> {
    let mut convs = vec![conversation(&client.id).0, conversation(super::ALL_APPS).0];
    for c in store.clients().unwrap_or_default() {
        if c.role == "app" && c.id != client.id {
            convs.push(conversation(&c.id).0);
        }
    }
    if owner
        && let Some(dm) = store
            .setting(lucia_store::keys::NOTIFY_CONVERSATION)
            .ok()
            .flatten()
    {
        convs.push(dm);
    }
    convs
}

/// 이 클라이언트가 이 파일을 받을 수 있는가(결정 0043·0066·0067). 앱 대화는 하나라 어느 앱 대화(앱 모두 포함)로
/// 오간 파일이든, 또는 그 대화로 보낸 파일. 다만 다른 앱이 짧게 둔 파일(`keep=short`)은 그 앱만. 범위
/// `owner_files`가 있으면 Discord 알림 대화의 파일도.
pub(super) fn can_download(
    store: &lucia_store::Store,
    client: &lucia_store::Client,
    rec: &lucia_store::FileRecord,
) -> bool {
    let owner = client.scopes.iter().any(|s| s == scopes::OWNER_FILES);
    let mine = conversation(&client.id).0;
    let short = store.file_expiry(&rec.id).ok().flatten().is_some();
    let dm = store
        .setting(lucia_store::keys::NOTIFY_CONVERSATION)
        .ok()
        .flatten();
    let reads = |conv: &str| {
        conv == mine
            || (super::client_of(conv).is_some() && !short)
            || (owner && dm.as_deref() == Some(conv))
    };
    if rec.conversation.as_deref().is_some_and(reads) {
        return true;
    }
    store
        .file_grants(&rec.id)
        .unwrap_or_default()
        .iter()
        .any(|g| reads(g))
}

// ---------- 명령 ----------

/// `commands`: 이 앱이 쓸 수 있는 명령(Discord 슬래시 명령과 같은 것).
struct Commands;

impl AppMethod for Commands {
    fn name(&self) -> &'static str {
        "commands"
    }

    fn call<'a>(&'a self, ctx: CallContext<'a>, _params: Value) -> CallFuture<'a> {
        Box::pin(async move {
            let can_change = ctx.client.scopes.iter().any(|s| s == scopes::SECURITY_MODE);
            let models: Vec<&str> = lucia_channel::MODEL_CHOICES
                .iter()
                .map(|(_, value)| *value)
                .collect();
            // 지금 값(`current`)은 `options` 가운데 하나다. 앱이 고르는 화면에 지금 값을 표시한다.
            let (mode, model) = {
                let store = ctx.gateway.store.lock().expect("store lock");
                (
                    crate::security::mode(&store).label(),
                    crate::models::stored(&store).map_or("default", |c| c.key),
                )
            };
            Ok(json!({"commands": [{
                "name": lucia_channel::COMMAND_SECURITY,
                "about": "권한 모드 보기·바꾸기",
                "options": ["엄격", "기본", "해제"],
                "current": mode,
                "can_change": can_change,
            }, {
                // 주 모델(결정 0055): 안전 장치가 아니라서 범위를 따로 두지 않는다. 바꾸면 알림 대화에 남는다.
                "name": lucia_channel::COMMAND_MODEL,
                "about": "주 모델 보기·바꾸기",
                "options": models,
                "current": model,
                "can_change": true,
            }]}))
        })
    }
}

/// 명령의 답을 기다리는 시간.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// `command {name, option?}`: Discord의 슬래시 명령과 같은 처리기(승인 데스크)로 간다. 지금 명령은 `권한` 하나다.
/// 모드를 바꾸는 것(`option`이 있을 때)은 `security_mode` 범위가 있는 토큰만(결정 0040의 예외, 결정 0043).
struct Command;

impl AppMethod for Command {
    fn name(&self) -> &'static str {
        "command"
    }

    fn call<'a>(&'a self, ctx: CallContext<'a>, params: Value) -> CallFuture<'a> {
        Box::pin(async move {
            let name = params["name"].as_str().unwrap_or("").trim().to_owned();
            let option = params["option"]
                .as_str()
                .map(|o| o.trim().to_owned())
                .filter(|o| !o.is_empty());
            if name != lucia_channel::COMMAND_SECURITY && name != lucia_channel::COMMAND_MODEL {
                return Err(CallError::new("unknown_command", "모르는 명령"));
            }
            if name == lucia_channel::COMMAND_SECURITY
                && option.is_some()
                && !ctx.client.scopes.iter().any(|s| s == scopes::SECURITY_MODE)
            {
                return Err(CallError::new(
                    "forbidden",
                    "이 토큰은 권한 모드를 바꿀 수 없다(관리 페이지에서 범위를 주어야 한다)",
                ));
            }
            let tx = ctx
                .gateway
                .commands
                .get()
                .ok_or_else(|| CallError::new("unsupported", "이 데몬은 명령을 받지 않는다"))?;
            let (reply, rx) = tokio::sync::oneshot::channel();
            tx.send(lucia_channel::SlashCommand {
                platform: super::PLATFORM.into(),
                sender: ctx.client.id.clone(),
                in_dm: true,
                name,
                option,
                reply,
            })
            .await
            .map_err(|_| CallError::new("closed", "데몬이 멈추는 중"))?;
            match tokio::time::timeout(COMMAND_TIMEOUT, rx).await {
                Ok(Ok(r)) => Ok(json!({"text": r.text})),
                Ok(Err(_)) => Err(CallError::new("failed", "명령이 답하지 않았다")),
                // 늦으면 처리기가 적용하지 않는다(답 통로가 닫힌 것을 본다, 결정 0040).
                Err(_) => Err(CallError::new("timeout", "명령이 제때 답하지 않았다")),
            }
        })
    }
}

// ---------- 세션 정리 ----------

/// `session.compact`(결정 0056): 상주 세션의 대화를 요약해 줄이고(모델 1회), 끝날 때까지 기다렸다가 결과를 답한다.
/// 작업 큐에서 차례대로 돌므로 도는 답이나 정기 작업이 있으면 그 뒤에 한다. 주 모델을 바꾸기 전에 부르면 새 모델이
/// 다시 읽을 대화가 줄어든다(결정 0055).
///
/// - 마지막 정리 뒤에 실행이 없으면 모델을 부르지 않고 `unchanged`로 답한다. 그래서 답을 받지 못한 앱이 다시 불러도
///   정리가 한 번 더 돌지 않는다.
/// - `min_tokens`를 주면 맥락이 그보다 작을 때 하지 않고 `small`로 답한다(정리해도 줄일 것이 없는 짧은 대화).
/// - 기다리는 시간을 넘기면 `timeout`이다. 아직 차례가 오지 않은 요청은 버려지고, 이미 도는 정리는 끝까지 간다.
/// - 한 번에 하나만 받는다(겹치면 `busy`).
struct SessionCompact;

/// 정리 요청 하나가 끝나면(답했든, 시간을 넘겼든, 연결이 끊겼든) 다음 요청을 받게 푼다.
struct CompactingGuard<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for CompactingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl AppMethod for SessionCompact {
    fn name(&self) -> &'static str {
        "session.compact"
    }

    fn call<'a>(&'a self, ctx: CallContext<'a>, params: Value) -> CallFuture<'a> {
        use crate::worker::CompactReply;
        Box::pin(async move {
            // 맥락이 이보다 작으면 하지 않는다(없으면 크기를 보지 않는다).
            let min_tokens = match &params["min_tokens"] {
                Value::Null => 0,
                v => v
                    .as_u64()
                    .ok_or_else(|| CallError::new("bad_params", "min_tokens는 0 이상의 정수"))?,
            };
            let (jobs, wait) = ctx.gateway.jobs.get().ok_or_else(|| {
                CallError::new("unsupported", "이 데몬은 세션 정리를 받지 않는다")
            })?;
            if ctx
                .gateway
                .compacting
                .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(CallError::new("busy", "이미 세션을 정리하는 중이다"));
            }
            let _guard = CompactingGuard(&ctx.gateway.compacting);
            let (reply, rx) = tokio::sync::oneshot::channel();
            jobs.send(crate::worker::Job::Compact { min_tokens, reply })
                .await
                .map_err(|_| CallError::new("closed", "데몬이 멈추는 중"))?;
            tracing::info!(client = %ctx.client.id, "앱이 세션 정리를 청했다");
            let done = match tokio::time::timeout(*wait, rx).await {
                Ok(Ok(done)) => done,
                Ok(Err(_)) => return Err(CallError::new("closed", "데몬이 멈추는 중")),
                // 답 통로를 닫는다: 아직 차례가 오지 않았으면 작업자가 건너뛴다.
                Err(_) => {
                    return Err(CallError::new(
                        "timeout",
                        "정리가 제때 끝나지 않았다(아직 시작하지 않았으면 하지 않는다)",
                    ));
                }
            };
            let clip = |why: &str| -> String {
                crate::mask::global().mask(why).chars().take(300).collect()
            };
            Ok(match done {
                CompactReply::Compacted { pre, post } => json!({
                    "outcome": "compacted",
                    "pre_tokens": pre,
                    "context_tokens": post,
                    "text": format!(
                        "대화를 정리했습니다({}→{} 토큰).",
                        crate::admin::num(pre),
                        crate::admin::num(post)
                    ),
                }),
                CompactReply::Unchanged { tokens } => json!({
                    "outcome": "unchanged",
                    "context_tokens": tokens,
                    "text": "마지막 정리 뒤에 새 대화가 없어 다시 정리하지 않았습니다.",
                }),
                CompactReply::Small { tokens } => json!({
                    "outcome": "small",
                    "context_tokens": tokens,
                    "text": "대화가 짧아 정리하지 않았습니다.",
                }),
                CompactReply::NoSession => json!({
                    "outcome": "none",
                    "context_tokens": 0,
                    "text": "정리할 대화가 없습니다.",
                }),
                CompactReply::Reset(why) => json!({
                    "outcome": "reset",
                    "context_tokens": 0,
                    "text": format!(
                        "정리하지 못해 새 세션으로 넘어갑니다(기억과 최근 대화로 이어 갑니다): {}",
                        clip(&why)
                    ),
                }),
                CompactReply::Error(why) => {
                    return Err(CallError::owned("failed", &clip(&why)));
                }
            })
        })
    }
}

// ---------- 푸시 토큰 ----------

/// `push.register {token, platform}`: FCM 등록 토큰을 이 클라이언트에 둔다(토큰이 바뀌면 다시 부른다).
struct PushRegister;

impl AppMethod for PushRegister {
    fn name(&self) -> &'static str {
        "push.register"
    }

    fn call<'a>(&'a self, ctx: CallContext<'a>, params: Value) -> CallFuture<'a> {
        Box::pin(async move {
            register_push(
                ctx.gateway,
                &ctx.client.id,
                params["token"].as_str().unwrap_or(""),
                params["platform"].as_str().unwrap_or(""),
            )
        })
    }
}

/// `push.unregister`: 푸시를 받지 않는다.
struct PushUnregister;

impl AppMethod for PushUnregister {
    fn name(&self) -> &'static str {
        "push.unregister"
    }

    fn call<'a>(&'a self, ctx: CallContext<'a>, _params: Value) -> CallFuture<'a> {
        Box::pin(async move {
            ctx.gateway
                .store
                .lock()
                .expect("store lock")
                .set_client_push(&ctx.client.id, None, None, chrono::Utc::now())
                .map_err(|e| CallError::owned("failed", &e.to_string()))?;
            Ok(json!({"registered": false}))
        })
    }
}

/// 등록 토큰은 FCM이 주는 글(영숫자와 `:_-`). 길이·문자를 확인한다.
pub(super) fn register_push(
    gw: &Gateway,
    client_id: &str,
    token: &str,
    platform: &str,
) -> Result<Value, CallError> {
    let token = token.trim();
    if token.is_empty()
        || token.len() > 4096
        || !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '.'))
    {
        return Err(CallError::new("bad_params", "푸시 토큰 형식이 아니다"));
    }
    if !matches!(platform, "android" | "ios" | "web") {
        return Err(CallError::new("bad_params", "platform은 android, ios, web"));
    }
    let store = gw.store.lock().expect("store lock");
    store
        .set_client_push(client_id, Some(token), Some(platform), chrono::Utc::now())
        .map_err(|e| CallError::owned("failed", &e.to_string()))?;
    let _ = store.add_activity(
        "client_push",
        &format!("앱 푸시 등록: {client_id} ({platform})"),
        None,
        chrono::Utc::now(),
    );
    Ok(json!({"registered": true, "available": gw.push_ready()}))
}
