//! 브라우저 화면 넘겨받기를 앱에서(결정 0063). 로그인 도중 사이트가 사람 확인(캡차 등)을 띄우면 사용자가 그
//! 화면을 직접 보고 푼다(결정 0017). 관리 페이지에만 있던 길을 접속 API로도 연다.
//!
//! 토큰을 가진 사람이 **로그인된 브라우저를 보고 조작할 수 있게** 되므로 범위 `browser_handover`가 있는
//! 클라이언트만 쓴다(기본 꺼짐, 관리 페이지 "연결"에서 켠다). 넣은 글과 누른 자리는 기록하지 않는다.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::methods::{AppMethod, CallContext, CallError, CallFuture};
use super::{Gateway, scopes};
use crate::browser::{BoxFut, Frame, HandInput, Tab};

/// 넘겨받을 수 있는 브라우저(실제는 [`crate::browser::tools::BrowserTools`]).
pub trait Handover: Send + Sync {
    /// 넘겨받는 중이면 남은 시간(최대 시간이 지난 것은 먼저 끝낸다).
    fn left(&self) -> BoxFut<'_, Option<Duration>>;
    fn begin(&self) -> BoxFut<'_, Result<(), String>>;
    fn end<'a>(&'a self, why: &'a str) -> BoxFut<'a, Result<(), String>>;
    fn tabs(&self) -> BoxFut<'_, Result<Vec<Tab>, String>>;
    fn frame<'a>(&'a self, tab: &'a str) -> BoxFut<'a, Result<Frame, String>>;
    fn input<'a>(&'a self, tab: &'a str, input: &'a HandInput) -> BoxFut<'a, Result<(), String>>;
}

impl Handover for crate::browser::tools::BrowserTools {
    fn left(&self) -> BoxFut<'_, Option<Duration>> {
        Box::pin(async move {
            self.expire_handover().await;
            self.handover_left()
        })
    }
    fn begin(&self) -> BoxFut<'_, Result<(), String>> {
        Box::pin(self.begin_handover())
    }
    fn end<'a>(&'a self, why: &'a str) -> BoxFut<'a, Result<(), String>> {
        Box::pin(self.end_handover(why))
    }
    fn tabs(&self) -> BoxFut<'_, Result<Vec<Tab>, String>> {
        Box::pin(self.handover_tabs())
    }
    fn frame<'a>(&'a self, tab: &'a str) -> BoxFut<'a, Result<Frame, String>> {
        Box::pin(self.handover_frame(tab))
    }
    fn input<'a>(&'a self, tab: &'a str, input: &'a HandInput) -> BoxFut<'a, Result<(), String>> {
        Box::pin(self.handover_input(tab, input))
    }
}

/// 한 번에 넣는 글의 상한(문자 수, 관리 페이지와 같다).
const TEXT_MAX: usize = 2000;
/// 한 번에 굴리는 양의 상한(CSS 픽셀).
const SCROLL_MAX: i32 = 3000;

pub(super) fn register(gw: &Gateway) {
    gw.register(Arc::new(State));
    gw.register(Arc::new(Start));
    gw.register(Arc::new(Input));
    gw.register(Arc::new(End));
}

fn browser(gw: &Gateway) -> Result<&Arc<dyn Handover>, CallError> {
    gw.handover()
        .ok_or_else(|| CallError::new("unavailable", "브라우저가 꺼져 있다"))
}

/// 지금 상태: 넘겨받는 중인지, 남은 시간, 탭 목록(주소·제목은 가림 필터를 거친다).
async fn state(h: &Arc<dyn Handover>) -> Value {
    let left = h.left().await;
    let tabs = match left {
        Some(_) => h.tabs().await.unwrap_or_default(),
        None => Vec::new(),
    };
    let mask = crate::mask::global();
    json!({
        "active": left.is_some(),
        "left_seconds": left.map(|d| d.as_secs()),
        "max_seconds": crate::browser::tools::HANDOVER_MAX.as_secs(),
        "tabs": tabs.iter().map(|t| json!({
            "id": t.id,
            "url": mask.mask(&t.url),
            "title": mask.mask(&t.title),
        })).collect::<Vec<_>>(),
    })
}

/// `browser.handover.state`: 상태만 본다(시작하지 않는다).
struct State;

impl AppMethod for State {
    fn name(&self) -> &'static str {
        "browser.handover.state"
    }
    fn scope(&self) -> Option<&'static str> {
        Some(scopes::BROWSER_HANDOVER)
    }
    fn call<'a>(&'a self, ctx: CallContext<'a>, _params: Value) -> CallFuture<'a> {
        Box::pin(async move { Ok(state(browser(ctx.gateway)?).await) })
    }
}

/// `browser.handover.start`: 넘겨받는다. 이미 넘겨받는 중이면 그대로 둔다(시간을 다시 재지 않는다).
struct Start;

impl AppMethod for Start {
    fn name(&self) -> &'static str {
        "browser.handover.start"
    }
    fn scope(&self) -> Option<&'static str> {
        Some(scopes::BROWSER_HANDOVER)
    }
    fn call<'a>(&'a self, ctx: CallContext<'a>, _params: Value) -> CallFuture<'a> {
        Box::pin(async move {
            let h = browser(ctx.gateway)?;
            if h.left().await.is_none() {
                h.begin()
                    .await
                    .map_err(|e| CallError::owned("failed", &crate::mask::global().mask(&e)))?;
                tracing::info!(client = %ctx.client.id, "앱: 브라우저 화면 넘겨받기 시작");
            }
            Ok(state(h).await)
        })
    }
}

/// `browser.handover.input {tab, kind, x?, y?, text?, key?, dy?}`: 넘겨받은 화면에 입력을 보낸다.
/// 넣은 글과 누른 자리는 기록하지 않는다.
struct Input;

impl AppMethod for Input {
    fn name(&self) -> &'static str {
        "browser.handover.input"
    }
    fn scope(&self) -> Option<&'static str> {
        Some(scopes::BROWSER_HANDOVER)
    }
    fn call<'a>(&'a self, ctx: CallContext<'a>, params: Value) -> CallFuture<'a> {
        Box::pin(async move {
            let h = browser(ctx.gateway)?;
            let bad = |why: &str| CallError::new("bad_params", why);
            let tab = params["tab"]
                .as_str()
                .filter(|t| !t.is_empty())
                .ok_or_else(|| bad("tab이 없다"))?;
            let point = |k: &str| params[k].as_u64().and_then(|v| u32::try_from(v).ok());
            let input = match params["kind"].as_str().unwrap_or("") {
                "click" => match (point("x"), point("y")) {
                    (Some(x), Some(y)) => HandInput::Click { x, y },
                    _ => return Err(bad("click에는 x와 y가 있어야 한다")),
                },
                "text" => match params["text"].as_str() {
                    Some(t) if !t.is_empty() && t.chars().count() <= TEXT_MAX => {
                        HandInput::Text(t.to_owned())
                    }
                    _ => return Err(bad("text는 1~2000자다")),
                },
                "key" => match params["key"].as_str() {
                    Some(k) if !k.is_empty() && k.len() <= 20 => HandInput::Key(k.to_owned()),
                    _ => return Err(bad("key가 없다")),
                },
                "scroll" => match params["dy"].as_i64() {
                    Some(dy) => HandInput::Scroll(
                        dy.clamp(i64::from(-SCROLL_MAX), i64::from(SCROLL_MAX)) as i32,
                    ),
                    None => return Err(bad("scroll에는 dy가 있어야 한다")),
                },
                _ => return Err(bad("kind는 click, text, key, scroll 가운데 하나다")),
            };
            if h.left().await.is_none() {
                return Err(CallError::new("not_active", "넘겨받은 상태가 아니다"));
            }
            h.input(tab, &input)
                .await
                .map_err(|e| CallError::owned("failed", &crate::mask::global().mask(&e)))?;
            Ok(json!({"sent": true}))
        })
    }
}

/// `browser.handover.end`: 넘겨받기를 끝낸다(Lucia가 다시 브라우저를 쓴다). 하는 중이 아니어도 성공이다.
struct End;

impl AppMethod for End {
    fn name(&self) -> &'static str {
        "browser.handover.end"
    }
    fn scope(&self) -> Option<&'static str> {
        Some(scopes::BROWSER_HANDOVER)
    }
    fn call<'a>(&'a self, ctx: CallContext<'a>, _params: Value) -> CallFuture<'a> {
        Box::pin(async move {
            let h = browser(ctx.gateway)?;
            let was = h.left().await.is_some();
            // 탭 정리에 실패해도 넘겨받기는 끝난 것이다(관리 페이지와 같다).
            let warning = h.end("사용자가 앱에서 끝냄").await.err();
            if was {
                tracing::info!(client = %ctx.client.id, "앱: 브라우저 화면 넘겨받기 끝");
            }
            Ok(json!({
                "active": false,
                "warning": warning.map(|e| crate::mask::global().mask(&e)),
            }))
        })
    }
}
