//! 모델이 쓰는 브라우저 도구의 판단과 흐름(결정 0008).
//!
//! - **등급**(명세 §권한 등급): 동작 직전에 요소를 다시 읽어 그 모습으로 정한다([`classify`]).
//!   - 그냥 한다(읽기): 링크 이동, 펼치기·탭, 체크·선택·글 넣기(제출 없이), GET 폼(검색), 스크롤, 뒤로.
//!   - Discord 승인: POST 폼 제출, 무엇을 할지 모르는 스크립트 버튼, Enter로 무엇이 될지 모르는 칸.
//!   - 하지 않는다: 결제·주문·탈퇴·비밀번호 바꾸기, 로그아웃, 비밀번호 칸, 로그인 폼(→ `browser_login`), 파일 올리기.
//! - **승인**: 요청을 카드로 올리고 도구 호출 안에서 잠깐(35초) 기다린다. 그 안에 누르면 데몬이 실행하고 결과
//!   화면을 바로 돌려준다. 늦게 누르면 데몬이 그때 실행하고 결과는 카드와 다음 대화의 기록에 남는다. 실행 직전에
//!   같은 주소의 같은 요소(카드에 보인 값 그대로)인지 다시 확인한다. 기다리는 동안 저장소·브라우저 잠금을 쥐지 않는다.
//! - **참조**: 화면을 읽을 때마다 요소 참조(`e12`)를 새로 단다. 동작은 마지막으로 보여 준 화면의 참조로만 받고,
//!   그 요소가 그새 다른 것으로 바뀌었으면 하지 않는다.
//! - **좌표**(결정 0023): iframe 안이나 목록에 없는 곳은 화면 좌표로 누르고(`browser_click_at`), 초점이 있는 칸에
//!   글을 넣고(`browser_type_text`), 정해진 키를 보낸다(`browser_key`). 등급은 모델의 말이 아니라 데몬이 그 점(또는
//!   초점)에서 직접 읽은 요소로 같은 규칙에 따라 정하고, 확인하지 못하면 승인을 받는다.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use lucia_store::{NewApproval, Store};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use super::egress::Egress;
use super::logins::{Requested, SiteLogins};
use super::proxy::Proxy;
use super::{
    Act, BoxFut, Browser, BrowserError, Element, Expect, Focus, Hit, PageView, Point, Scroll, Shot,
};
use crate::approvals::{BrowserActionExec, KIND_BROWSER_ACTION};

/// 모델에게 넘길 본문 글 상한(문자). 누구나 쓸 수 있는 외부 글이라 크기를 묶는다.
pub const TEXT_MAX: usize = 15_000;
/// 모델에게 넘길 요소 줄 상한.
pub const ELEMENT_LINES_MAX: usize = 150;
/// 도구가 브라우저를 기다리는 최대 시간(MCP HTTP 요청이 60초쯤에 끊기기 전에 답한다).
pub const TOOL_WAIT: Duration = Duration::from_secs(50);

/// 동작의 등급.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Free,
    /// 가벼운 확인: 무엇을 하는지 데몬이 알아보지 못한 동작. 권한 모드가 해제이거나 사이트 위임이 동작을 허용했으면
    /// 묻지 않는다.
    Approval(String),
    /// 되돌릴 수 없이 바깥에 남는 쓰기(글 등록, 폼 제출): 권한 모드와 상관없이 승인 카드를 거친다(결정 0059).
    /// 사이트 위임이 동작을 허용했으면 묻지 않는다(사용자가 그 사이트에 미리 허락한 것이다).
    Publish(String),
    /// 돈과 계정(결제·주문·송금·탈퇴·비밀번호 바꾸기): 사람이 대화 중이면 늘 승인 카드를 거치고(위임도 건너뛰지
    /// 못한다), 사람 없는 실행은 하지 않는다. 예전의 금지 등급이다(결정 0059).
    Critical(String),
    /// 사람이 대화 중일 때만 그냥 하는 동작(로그아웃, 비밀번호 칸과 로그인 폼). 사람 없는 실행은 하지 않는다.
    HumanOnly(String),
    /// 할 수 없는 동작(비활성·숨은 요소, 글을 넣을 수 없는 곳 등). 정책이 아니라 되지 않는 것이다.
    Forbidden(String),
}

/// 돈·계정에 닿는 말(예전 이름은 금지어다. 지금은 승인 카드로 간다).
const FORBIDDEN_WORDS: &[&str] = &[
    "결제",
    "구매하기",
    "바로구매",
    "바로 구매",
    "주문하기",
    "충전하기",
    "송금",
    "이체하기",
    "회원탈퇴",
    "회원 탈퇴",
    "탈퇴하기",
    "계정 삭제",
    "계정삭제",
    "비밀번호 변경",
    "비밀번호변경",
    "비밀번호 재설정",
    "checkout",
    "payment",
    "pay now",
    "place order",
    "buy now",
    "delete account",
    "close account",
    "deactivate account",
    "change password",
    "reset password",
];
const PAYMENT_PATHS: &[&str] = &["checkout", "payment", "/pay/", "/order/", "billing"];
const LOGOUT_WORDS: &[&str] = &[
    "로그아웃",
    "logout",
    "log out",
    "log-out",
    "sign out",
    "signout",
    "sign-out",
];
/// 이 말이 든 스크립트 단추는 바깥에 남는 쓰기로 본다(글 올리기, 보내기, 지우기).
const WRITE_WORDS: &[&str] = &[
    "등록",
    "게시",
    "올리기",
    "글쓰기",
    "작성",
    "보내기",
    "전송",
    "제출",
    "삭제",
    "댓글",
    "답글",
    "공유",
    "저장",
    "submit",
    "post",
    "publish",
    "send",
    "delete",
    "remove",
    "reply",
    "comment",
    "share",
    "save",
    "tweet",
];
/// 이 이름의 스크립트 버튼은 화면만 바꾼다고 본다(POST 폼 안이 아닐 때).
const SAFE_BUTTONS: &[&str] = &[
    "더보기",
    "더 보기",
    "펼치기",
    "접기",
    "닫기",
    "다음",
    "이전",
    "다음 페이지",
    "이전 페이지",
    "more",
    "show more",
    "load more",
    "see more",
    "read more",
    "close",
    "next",
    "previous",
    "prev",
    "expand",
    "collapse",
    "×",
    "✕",
    "x",
];

fn lower(s: &str) -> String {
    s.to_lowercase()
}

fn has_any(text: &str, words: &[&str]) -> bool {
    let t = lower(text);
    words.iter().any(|w| t.contains(w))
}

fn strip_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

fn is_logout(el: &Element) -> bool {
    has_any(&el.label, LOGOUT_WORDS)
        || el
            .href
            .as_deref()
            .is_some_and(|h| has_any(h, &["logout", "signout", "sign-out", "log-out"]))
}

fn payment_page(page_url: &str) -> bool {
    url::Url::parse(page_url)
        .map(|u| has_any(u.path(), PAYMENT_PATHS))
        .unwrap_or(false)
}

/// 폼 제출(제출 단추 누르기, Enter).
fn form_gate(el: &Element, page_url: &str) -> Gate {
    let Some(form) = &el.form else {
        return Gate::Approval("무엇을 보낼지 모르는 제출".into());
    };
    if has_any(&el.label, FORBIDDEN_WORDS)
        || has_any(&form.action, FORBIDDEN_WORDS)
        || payment_page(page_url)
        || has_any(&form.action, PAYMENT_PATHS)
    {
        return Gate::Critical("결제·주문·계정 설정 제출".into());
    }
    if form.password {
        return Gate::HumanOnly("로그인 폼 제출".into());
    }
    match form.method.as_str() {
        "get" | "dialog" => Gate::Free,
        _ => Gate::Publish(format!(
            "{} 폼 제출(외부에 남는 쓰기일 수 있다)",
            form.method.to_uppercase()
        )),
    }
}

/// 스크립트 버튼(무엇을 할지 페이지 코드가 정한다).
fn button_gate(el: &Element, page_url: &str) -> Gate {
    if has_any(&el.label, FORBIDDEN_WORDS) || payment_page(page_url) {
        return Gate::Critical("결제·주문·계정 설정 단추".into());
    }
    if el.expanded.is_some() || el.popup || el.kind == "tab" {
        return Gate::Free;
    }
    let post_form = el.form.as_ref().is_some_and(|f| f.method == "post");
    if !post_form && SAFE_BUTTONS.contains(&lower(el.label.trim()).as_str()) {
        return Gate::Free;
    }
    if post_form {
        return Gate::Publish("POST 폼 안의 단추(외부에 남는 쓰기일 수 있다)".into());
    }
    if has_any(&el.label, WRITE_WORDS) {
        return Gate::Publish("글을 올리거나 보내거나 지우는 단추로 보인다".into());
    }
    Gate::Approval("무엇을 할지 모르는 스크립트 단추".into())
}

/// 동작의 등급을 정한다(요소는 동작 직전에 다시 읽은 것).
pub fn classify(act: &Act, el: &Element, page_url: &str) -> Gate {
    let forbid = |m: &str| Gate::Forbidden(m.to_owned());
    match act {
        Act::Scroll { .. } | Act::Back => Gate::Free,
        // 좌표·초점 동작은 [`classify_hit`]·[`classify_focus`]가 정한다(여기에 오면 확인하지 못한 것이다).
        Act::ClickAt { .. } | Act::TypeText { .. } | Act::Key { .. } => {
            Gate::Approval("대상을 확인하지 못한 동작".into())
        }
        Act::Click { .. } => {
            if el.disabled {
                return forbid("비활성 요소다");
            }
            if is_logout(el) {
                return Gate::HumanOnly("로그아웃".into());
            }
            match el.kind.as_str() {
                "password" => Gate::HumanOnly("비밀번호 칸".into()),
                "file" => forbid(
                    "파일 칸은 누르지 않는다(파일 고르기 창은 다룰 수 없다). 파일을 올리려면 browser_upload를 쓴다",
                ),
                "hidden" => forbid("숨은 요소다"),
                "link" => {
                    let href = el.href.as_deref().unwrap_or("");
                    let navigates = (href.starts_with("https://") || href.starts_with("http://"))
                        && strip_fragment(href) != strip_fragment(page_url);
                    if navigates {
                        Gate::Free
                    } else {
                        button_gate(el, page_url)
                    }
                }
                "summary" | "tab" | "checkbox" | "radio" | "select" | "text" | "textarea"
                | "editable" | "reset" => Gate::Free,
                "submit" => form_gate(el, page_url),
                _ => button_gate(el, page_url),
            }
        }
        Act::Type { submit, .. } => {
            if el.disabled {
                return forbid("비활성 요소다");
            }
            if !matches!(
                el.kind.as_str(),
                "text" | "textarea" | "select" | "editable" | "password"
            ) {
                return forbid("글을 넣을 수 있는 요소가 아니다");
            }
            // 비밀번호 칸과 로그인 폼: 사람이 대화 중일 때만 넣는다(결정 0059). 제출도 같다.
            if el.kind == "password" || el.form.as_ref().is_some_and(|f| f.password) {
                return match (submit, form_gate(el, page_url)) {
                    (true, Gate::Critical(why)) => Gate::Critical(why),
                    _ => Gate::HumanOnly("비밀번호 칸·로그인 폼 입력".into()),
                };
            }
            if !submit {
                return Gate::Free;
            }
            if el.kind == "textarea" {
                return forbid(
                    "여러 줄 칸은 Enter로 제출하지 않는다. 제출 단추를 browser_click으로 누른다",
                );
            }
            match &el.form {
                Some(_) => form_gate(el, page_url),
                None if el.search => Gate::Free,
                None => Gate::Approval("Enter로 무엇이 실행될지 모른다".into()),
            }
        }
    }
}

/// 엄격 모드(결정 0040): 그냥 하던 누르기·글 넣기·Enter·Space를 묻는다. 링크 이동·펼치기·탭(화면 안 이동)과
/// Enter·Space 말고의 키(Tab, 화살표, 쪽 넘기기, Esc)는 그대로 둔다.
pub fn strict_gate(act: &Act, el: &Element, gate: Gate) -> Gate {
    if gate != Gate::Free {
        return gate;
    }
    let ask = || Gate::Approval("권한 모드가 엄격이라 누르기·입력은 묻는다".into());
    match act {
        Act::Click { .. } | Act::ClickAt { .. } => {
            if matches!(el.kind.as_str(), "link" | "summary" | "tab") {
                Gate::Free
            } else {
                ask()
            }
        }
        Act::Type { .. } | Act::TypeText { .. } => ask(),
        Act::Key { key } if matches!(key.as_str(), "enter" | "space") => ask(),
        _ => Gate::Free,
    }
}

/// 좌표 클릭의 등급(결정 0023): 데몬이 그 점에서 읽은 요소로, 참조 클릭과 같은 규칙.
///
/// - 확인하지 못한 곳(다른 사이트 iframe 안을 읽지 못함 등)은 승인을 받는다.
/// - 누를 수 있는 요소가 아닌 곳(글, 빈 곳)은 그냥 누른다. 단 손가락 커서가 뜨는 곳은 스크립트가 받는 곳으로 보고
///   스크립트 단추와 같이 다룬다([제안]).
pub fn classify_hit(hit: &Hit) -> Gate {
    let Some(el) = &hit.element else {
        return Gate::Approval(format!(
            "누르는 곳을 확인하지 못했다({})",
            hit.why.as_deref().unwrap_or("모름")
        ));
    };
    let page = el.frame.as_deref().unwrap_or(&hit.url);
    if el.kind == "area" {
        if el.disabled {
            return Gate::Free;
        }
        return if hit.pointer {
            button_gate(el, page)
        } else {
            Gate::Free
        };
    }
    classify(&Act::Click { r: el.r.clone() }, el, page)
}

/// 초점 동작의 등급(결정 0023): 초점이 있는 요소로.
pub fn classify_focus(act: &Act, page_url: &str, focus: &Focus) -> Gate {
    let el = match focus {
        Focus::On { element } => element,
        Focus::Unknown { why } => {
            return match act {
                Act::Key { key } if !matches!(key.as_str(), "enter" | "space") => Gate::Free,
                // 글자를 넣는 것만으로는 아무것도 보내지 않는다(제출은 Enter·단추에서 묻는다).
                Act::TypeText { submit: false, .. } => Gate::Free,
                _ => Gate::Approval(format!("초점이 있는 곳을 확인하지 못했다({why})")),
            };
        }
        Focus::Nothing => {
            return match act {
                Act::TypeText { .. } => Gate::Forbidden(
                    "초점이 있는 입력칸이 없다. 먼저 입력칸을 browser_click_at으로 누른다".into(),
                ),
                _ => Gate::Free,
            };
        }
    };
    let page = el.frame.as_deref().unwrap_or(page_url);
    match act {
        Act::TypeText { text, submit } => {
            if el.kind == "select" {
                return Gate::Forbidden(
                    "선택 상자에는 글을 넣지 않는다. 목록의 참조로 browser_type을 쓰거나 browser_key(up·down)로 고른다"
                        .into(),
                );
            }
            classify(
                &Act::Type {
                    r: el.r.clone(),
                    text: text.clone(),
                    submit: *submit,
                },
                el,
                page,
            )
        }
        Act::Key { key } => match key.as_str() {
            "enter" => match el.kind.as_str() {
                "text" | "password" => classify(
                    &Act::Type {
                        r: el.r.clone(),
                        text: String::new(),
                        submit: true,
                    },
                    el,
                    page,
                ),
                // 여러 줄 칸·편집 칸의 Enter는 줄바꿈으로 본다(글을 올리는 것은 등록 단추에서 묻는다, 사용자 지시
                // 2026-11-18). Enter로 보내는 채팅 입력칸은 묻지 않고 보낼 수 있다(받아들인 위험).
                "textarea" | "editable" => Gate::Free,
                // 폼 안의 체크·선택·선택 상자에서 Enter는 폼을 제출할 수 있다(암묵적 제출): 폼 판단을 따른다.
                "checkbox" | "radio" | "select" => match &el.form {
                    Some(_) => form_gate(el, page),
                    None => Gate::Free,
                },
                _ => classify(&Act::Click { r: el.r.clone() }, el, page),
            },
            "space" => match el.kind.as_str() {
                "text" | "textarea" | "editable" | "select" | "password" => Gate::Free,
                _ => classify(&Act::Click { r: el.r.clone() }, el, page),
            },
            _ => Gate::Free,
        },
        _ => classify(act, el, page),
    }
}

pub fn kind_ko(kind: &str) -> &'static str {
    match kind {
        "link" => "링크",
        "submit" => "제출 단추",
        "reset" => "초기화 단추",
        "text" => "입력칸",
        "textarea" => "여러 줄 칸",
        "select" => "선택 상자",
        "checkbox" => "체크",
        "radio" => "선택 단추",
        "password" => "비밀번호 칸",
        "file" => "파일 칸",
        "tab" => "탭",
        "summary" => "펼치기",
        "editable" => "편집 칸",
        "area" => "화면의 그 자리",
        "unknown" => "확인하지 못한 곳",
        _ => "단추",
    }
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// 동작 이름(카드·활동 기록).
pub fn act_ko(act: &Act) -> String {
    match act {
        Act::Click { .. } => "클릭".into(),
        Act::ClickAt { at } => format!("화면 ({}, {}) 클릭", at.x, at.y),
        Act::TypeText { submit: true, .. } => "초점 칸에 입력하고 Enter".into(),
        Act::TypeText { .. } => "초점 칸에 입력".into(),
        Act::Key { key } => format!("키 {key}"),
        Act::Type { submit: true, .. } => "입력하고 Enter로 제출".into(),
        Act::Type { .. } => "입력".into(),
        Act::Scroll { to, at } => format!(
            "스크롤({}{})",
            match to {
                Scroll::Down => "아래",
                Scroll::Up => "위",
                Scroll::Top => "맨 위",
                Scroll::Bottom => "맨 아래",
            },
            at.map(|p| format!(", ({}, {}) 자리", p.x, p.y))
                .unwrap_or_default()
        ),
        Act::Back => "뒤로".into(),
    }
}

fn element_line(e: &Element, page_url: &str) -> String {
    format!("[{}] {}", e.r, element_desc(e, page_url))
}

/// 요소 설명(참조나 좌표 뒤에 붙는다).
fn element_desc(e: &Element, page_url: &str) -> String {
    let mut s = kind_ko(&e.kind).to_owned();
    if !e.label.is_empty() {
        s.push_str(&format!(" “{}”", clip(&e.label, 80)));
    }
    if let Some(h) = &e.href {
        s.push_str(&format!(" → {}", clip(h, 160)));
    }
    if let Some(v) = e.value.as_deref().filter(|v| !v.is_empty()) {
        s.push_str(&format!(" 값=“{}”", clip(v, 80)));
    }
    if let Some(c) = e.checked {
        s.push_str(if c { " (켜짐)" } else { " (꺼짐)" });
    }
    if let Some(x) = e.expanded {
        s.push_str(if x { " (펼쳐짐)" } else { " (접힘)" });
    }
    if !e.options.is_empty() {
        let shown: Vec<&str> = e.options.iter().take(8).map(String::as_str).collect();
        s.push_str(&format!(
            " 선택지: {}{}",
            shown.join(" | "),
            if e.options.len() > 8 { " | …" } else { "" }
        ));
    }
    if e.kind == "submit"
        && let Some(f) = &e.form
    {
        s.push_str(&format!(
            " [{} → {}]",
            f.method.to_uppercase(),
            clip(&f.action, 100)
        ));
    }
    if e.search {
        s.push_str(" (검색 칸)");
    }
    if e.disabled {
        s.push_str(" (비활성)");
    }
    if !e.in_view {
        s.push_str(" (화면 밖)");
    }
    let probe = match e.kind.as_str() {
        "text" | "textarea" | "select" | "editable" => None,
        _ => Some(Act::Click { r: e.r.clone() }),
    };
    if let Some(act) = probe {
        match classify(&act, e, page_url) {
            Gate::Free | Gate::HumanOnly(_) => {}
            Gate::Approval(_) => s.push_str(" ⚑확인 필요"),
            Gate::Publish(_) | Gate::Critical(_) => s.push_str(" ⚑승인 필요"),
            Gate::Forbidden(_) => s.push_str(" ⛔못 함"),
        }
    }
    s
}

/// 화면을 모델에게 줄 글로. `compact`면 본문 없이 화면 안의 요소만(스크롤 뒤).
pub fn format_view(view: &PageView, compact: bool) -> String {
    let mut out = format!("주소: {}\n제목: {}\n", view.url, view.title);
    let (y, h, total) = view.scroll;
    if total > h {
        out.push_str(&format!(
            "스크롤: {}~{}px / 전체 {}px\n",
            y,
            (y + h).min(total),
            total
        ));
    }
    if !view.dialogs.is_empty() {
        out.push_str(&format!("대화상자: {}\n", view.dialogs.join(" / ")));
    }
    if !view.popups.is_empty() {
        out.push_str(&format!(
            "새 창이 열려 닫았다(필요하면 browser_open으로 연다): {}\n",
            view.popups.join(", ")
        ));
    }
    if view.login_form {
        out.push_str("이 화면에 로그인 칸이 있다. 로그인이 필요하면 사용자 의도를 확인하고 browser_login(이 주소)을 쓴다. 비밀번호를 묻거나 넣지 않는다.\n");
    }
    if !compact {
        out.push('\n');
        let total_chars = view.text.chars().count();
        if total_chars > TEXT_MAX {
            out.extend(view.text.chars().take(TEXT_MAX));
            out.push_str(&format!(
                "\n…(이하 생략, 전체 {total_chars}자. 더 보려면 링크나 스크롤)"
            ));
        } else {
            out.push_str(&view.text);
        }
        out.push('\n');
    }
    if view.viewport.0 > 0 {
        out.push_str(&format!(
            "화면: {}×{}(좌표는 지금 보이는 화면 기준, 스크린샷 픽셀과 같다)\n",
            view.viewport.0, view.viewport.1
        ));
    }
    let lines: Vec<&Element> = view
        .elements
        .iter()
        .filter(|e| !compact || e.in_view)
        .take(ELEMENT_LINES_MAX)
        .collect();
    if !lines.is_empty() {
        let total = view.element_total.max(view.elements.len());
        out.push_str(&format!(
            "\n조작할 수 있는 요소({}{}개 중 {}개, [참조]로 browser_click·browser_type):\n",
            if compact { "화면 안, 전체 " } else { "" },
            total,
            lines.len()
        ));
        for e in lines {
            out.push_str(&element_line(e, &view.url));
            out.push('\n');
        }
    }
    // 글 예산은 본문과 iframe이 함께 쓴다(큰 iframe부터 남은 만큼): 본문이 껍데기이고 내용이 iframe에 있는
    // 페이지(블로그 등)도, 광고 iframe이 많은 페이지도 맞춘다.
    let mut budget = TEXT_MAX
        .saturating_sub(view.text.chars().count())
        .max(FRAME_TEXT_MIN * 2);
    let mut lines_left = FRAME_ELEMENTS_TOTAL;
    for (i, f) in view.frames.iter().take(FRAMES_MAX).enumerate() {
        let (x, y, w, h) = f.rect;
        out.push_str(&format!(
            "\n── iframe {}: {} (화면 {x},{y}부터 {w}×{h}) ──\n",
            i + 1,
            clip(&f.url, 160)
        ));
        if !compact && !f.text.trim().is_empty() && budget > 0 {
            let take = f.text.chars().count().min(budget).min(FRAME_TEXT_MAX);
            out.push_str(&clip(&f.text, take));
            out.push('\n');
            budget = budget.saturating_sub(take);
        }
        if !f.elements.is_empty() && lines_left > 0 {
            out.push_str("iframe 안 요소(참조가 없다. 좌표로 browser_click_at, 글은 누른 뒤 browser_type_text):\n");
            let n = f.elements.len().min(FRAME_ELEMENTS_MAX).min(lines_left);
            lines_left -= n;
            for (p, e) in f.elements.iter().take(n) {
                out.push_str(&format!("({}, {}) {}\n", p.x, p.y, element_desc(e, &f.url)));
            }
        }
    }
    out
}

/// 화면에 보일 iframe 수, 프레임 하나의 글·요소 상한.
pub const FRAMES_MAX: usize = 6;
pub const FRAME_TEXT_MAX: usize = 12_000;
/// 본문이 길어도 iframe 글에 남겨 두는 최소 예산(두 개 몫).
pub const FRAME_TEXT_MIN: usize = 1500;
pub const FRAME_ELEMENTS_MAX: usize = 40;
/// 모든 iframe의 요소 줄 합계 상한.
pub const FRAME_ELEMENTS_TOTAL: usize = 60;

/// 마지막으로 보여 준 화면의 요소들.
struct Seen {
    url: String,
    elements: HashMap<String, Element>,
}

#[derive(Debug, Clone)]
pub struct ToolsConfig {
    /// 도구 호출 안에서 승인을 기다리는 시간.
    pub approval_wait: Duration,
    /// 승인 요청이 살아 있는 시간(사람이 대화 중일 때). 사람 없는 실행은 결정 0030의 시간을 쓴다.
    pub approval_ttl: chrono::Duration,
    /// 사용자 시간대(방해 금지 시간).
    pub tz: chrono_tz::Tz,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            approval_wait: Duration::from_secs(35),
            approval_ttl: chrono::Duration::minutes(5),
            tz: chrono_tz::UTC,
        }
    }
}

type Waiter = oneshot::Sender<Result<String, String>>;

pub struct BrowserTools {
    logins: Arc<SiteLogins>,
    egress: Arc<Egress>,
    proxy: Option<Arc<Proxy>>,
    store: Arc<Mutex<Store>>,
    posts: Option<mpsc::UnboundedSender<String>>,
    cfg: ToolsConfig,
    seen: Mutex<Option<Seen>>,
    waiters: Mutex<HashMap<String, Waiter>>,
    /// 승인 결과를 기다리던 도구 호출이 받아 간 요청(결정 0030: 그러면 이어 하기를 걸지 않는다).
    live: Mutex<std::collections::HashSet<String>>,
    /// 지금 브라우저를 쓰는 쪽(상주 대화 또는 작업 하나)과 마지막으로 쓴 때(결정 0014).
    lease: Mutex<Option<(String, std::time::Instant)>>,
    /// 사람이 화면을 넘겨받은 때(결정 0017). 넘겨받는 동안은 아무 실행도 브라우저를 쓰지 않는다.
    handover: Mutex<Option<std::time::Instant>>,
    /// 파일 받기(결정 0057). 가져오기가 브라우저보다 뒤에 만들어져 나중에 채운다.
    downloads: std::sync::OnceLock<Arc<crate::downloads::Downloads>>,
    /// 바깥 동작의 승인(결정 0059): 스크립트 실행과 파일 올리기가 쓴다.
    actions: std::sync::OnceLock<Arc<crate::actions::ActionGate>>,
}

/// 페이지에서 돌릴 스크립트의 길이 상한(문자).
const SCRIPT_MAX: usize = 20_000;
/// 스크립트가 돌려준 값을 모델에게 넘기는 상한(문자).
const SCRIPT_VALUE_MAX: usize = 20_000;

/// 스크립트가 바깥에 쓰거나, 무엇을 누르거나, 다른 곳으로 옮기는 모양인가. 글자로만 가려내므로 놓치는 것이 있다
/// (그래서 지침은 그런 스크립트를 돌리기 전에 승인을 받으라고 한다).
fn script_acts(code: &str) -> bool {
    let c = code.to_lowercase();
    [
        "fetch(",
        "xmlhttprequest",
        "sendbeacon",
        "websocket(",
        "eventsource(",
        ".submit(",
        "requestsubmit(",
        ".click(",
        "dispatchevent(",
        "location.href",
        "location.assign",
        "location.replace",
        "location =",
        "location=",
        "window.open",
        "document.cookie",
        "import(",
    ]
    .iter()
    .any(|w| c.contains(w))
}

/// 브라우저가 시작하려던 내려받기를 데몬이 대신 받을 때 그 도구 호출 안에서 기다리는 시간(여러 개면 합쳐서).
/// 넘으면 받기는 이어지고 "받는 중"이라고 알린다.
const REPLAY_WAIT: Duration = Duration::from_secs(8);
/// 도구 호출 하나가 브라우저 동작과 대신 받기를 합쳐 쓸 수 있는 시간(MCP 요청이 60초쯤에 끊기기 전에 답한다).
const CALL_BUDGET: Duration = Duration::from_secs(54);
/// 한 동작에서 대신 받는 내려받기 수.
const REPLAY_MAX: usize = 3;
/// 파일 주소를 열면 브라우저는 "중단됨"으로 답하고 내려받기 사건은 조금 뒤에 온다.
const DOWNLOAD_EVENT_WAIT: Duration = Duration::from_millis(1200);

/// 넘겨받기 최대 시간 [제안]. 지나면 다음 확인 때 끝낸다.
pub const HANDOVER_MAX: Duration = Duration::from_secs(15 * 60);

/// 다른 쪽이 이만큼 쓰지 않으면 브라우저를 넘겨받는다 [제안].
pub const LEASE_IDLE: Duration = Duration::from_secs(180);

/// 도구 결과: 페이지에서 온 글(외부 데이터로 감싼다) 또는 데몬의 말.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Page(String),
    Note(String),
    /// 페이지 글과 데몬의 말을 함께(브라우저가 시작하려던 내려받기를 데몬이 받았을 때, 결정 0057).
    PageWithNote {
        page: String,
        note: String,
    },
}

impl Reply {
    /// 모델에게 줄 글(페이지 글은 외부 데이터 표시로 감싼다). 비밀은 가린다.
    pub fn render(self) -> String {
        match self {
            Self::Page(t) => crate::mcp::external("browser", &crate::mask::global().mask(&t)),
            Self::Note(t) => crate::mask::global().mask(&t),
            Self::PageWithNote { page, note } => format!(
                "{}\n{}",
                crate::mask::global().mask(&note),
                crate::mcp::external("browser", &crate::mask::global().mask(&page))
            ),
        }
    }
}

impl BrowserTools {
    pub fn new(
        logins: Arc<SiteLogins>,
        egress: Arc<Egress>,
        proxy: Option<Arc<Proxy>>,
        store: Arc<Mutex<Store>>,
        posts: Option<mpsc::UnboundedSender<String>>,
        cfg: ToolsConfig,
    ) -> Self {
        Self {
            logins,
            egress,
            proxy,
            store,
            posts,
            cfg,
            seen: Mutex::default(),
            waiters: Mutex::default(),
            live: Mutex::default(),
            lease: Mutex::default(),
            handover: Mutex::default(),
            downloads: std::sync::OnceLock::new(),
            actions: std::sync::OnceLock::new(),
        }
    }

    /// 바깥 동작의 승인을 잇는다(데몬이 뜰 때 한 번).
    pub fn set_actions(&self, actions: Arc<crate::actions::ActionGate>) {
        let _ = self.actions.set(actions);
    }

    /// 승인 카드를 올리고 답을 기다린다. 승인이 아니면 모델에게 줄 말.
    async fn ask_action(&self, run: Option<&str>, what: &str, detail: &str) -> Result<(), String> {
        let Some(gate) = self.actions.get() else {
            return Err("승인을 받을 수 없는 실행이라 하지 않았다.".into());
        };
        let id = gate
            .request(run.unwrap_or_default(), what, detail, "browser")
            .map_err(|why| format!("하지 않았다: {why}"))?;
        match gate.wait(&id, self.cfg.approval_wait).await {
            crate::actions::Status::Approved => Ok(()),
            other => Err(format!(
                "하지 않았다. {} 승인된 뒤 같은 요청을 그대로 다시 부르면 통과한다.",
                crate::actions::explain(&other)
            )),
        }
    }

    fn seen_url(&self) -> Option<String> {
        self.seen
            .lock()
            .expect("seen lock")
            .as_ref()
            .map(|s| s.url.clone())
    }

    /// `browser_eval`(결정 0059): 지금 페이지에서 스크립트를 돈다. 사람이 대화 중일 때만. 바깥에 쓰거나 무엇을 누르는
    /// 모양이면, 결제·계정 페이지면, 권한 모드가 엄격이면 승인 카드를 거친다.
    pub async fn eval(&self, code: &str, run: Option<&str>) -> Reply {
        let started = std::time::Instant::now();
        if !crate::delegation::caller().is_some_and(|c| c.human) {
            return Reply::Note(
                "페이지에서 스크립트를 돌리는 것은 사람이 대화 중일 때만 한다.".into(),
            );
        }
        let code = code.trim();
        if code.is_empty() || code.chars().count() > SCRIPT_MAX {
            return Reply::Note(format!(
                "스크립트가 비었거나 너무 길다({SCRIPT_MAX}자까지)."
            ));
        }
        let Some(url) = self.seen_url() else {
            return Reply::Note("먼저 browser_open으로 페이지를 연다.".into());
        };
        let host = crate::delegation::host_of(&url).unwrap_or_default();
        let strict = crate::security::strict(&self.store.lock().expect("store lock"));
        let money = payment_page(&url);
        if money || strict || script_acts(code) {
            let what = format!(
                "브라우저에서 스크립트 실행 @ {host}{}",
                if money {
                    " (결제·주문 페이지)"
                } else {
                    ""
                }
            );
            if let Err(note) = self.ask_action(run, &what, code).await {
                self.activity(
                    "browser_refused",
                    &format!("스크립트 실행 @ {}", clip(&url, 120)),
                    run,
                );
                return Reply::Note(note);
            }
        }
        self.activity(
            "browser_act",
            &format!(
                "스크립트 실행({}자) @ {}",
                code.chars().count(),
                clip(&url, 120)
            ),
            run,
        );
        let since = Utc::now();
        let reply = match tokio::time::timeout(TOOL_WAIT, self.browser().eval(code)).await {
            Ok(Ok((value, view))) => {
                if let Some(blocked) = self.arrival_check(&view.url) {
                    return blocked;
                }
                let value: String = if value.chars().count() > SCRIPT_VALUE_MAX {
                    format!(
                        "{}…(이하 생략)",
                        value.chars().take(SCRIPT_VALUE_MAX).collect::<String>()
                    )
                } else {
                    value
                };
                match self.page(&view, true, since) {
                    Reply::Page(page) => {
                        Reply::Page(format!("[스크립트가 돌려준 값]\n{value}\n\n{page}"))
                    }
                    other => other,
                }
            }
            Ok(Err(e)) => Reply::Note(self.explain(&e, None, since)),
            Err(_) => Reply::Note(
                "스크립트가 제때 끝나지 않았다. browser_snapshot으로 지금 화면을 확인한다.".into(),
            ),
        };
        self.with_downloads(reply, run, started).await
    }

    /// `browser_upload`(결정 0059): 파일 고르기 칸에 파일을 넣는다. 사람이 대화 중일 때만, 늘 승인 카드를 거친다
    /// (사용자의 파일이 바깥 사이트로 간다).
    pub async fn upload(&self, r: &str, name: &str, bytes: &[u8], run: Option<&str>) -> Reply {
        if !crate::delegation::caller().is_some_and(|c| c.human) {
            return Reply::Note("파일 올리기는 사람이 대화 중일 때만 한다.".into());
        }
        let known = self
            .seen
            .lock()
            .expect("seen lock")
            .as_ref()
            .and_then(|s| s.elements.get(r).cloned().map(|e| (s.url.clone(), e)));
        let Some((url, el)) = known else {
            return Reply::Note(format!(
                "모르는 요소 참조다: {r}. browser_snapshot으로 화면을 다시 보고, 파일 칸의 참조를 쓴다."
            ));
        };
        if el.kind != "file" {
            return Reply::Note(format!(
                "{r}는 파일 칸이 아니다({}). 파일 칸의 참조를 쓴다.",
                kind_ko(&el.kind)
            ));
        }
        let host = crate::delegation::host_of(&url).unwrap_or_default();
        let summary = format!(
            "파일 올리기 {} ({}) → {} “{}”",
            clip(name, 60),
            crate::files::human_size(bytes.len() as u64),
            clip(&url, 120),
            clip(&el.label, 40)
        );
        if let Err(note) = self
            .ask_action(
                run,
                &format!("파일 올리기: {} → {host}", clip(name, 60)),
                &summary,
            )
            .await
        {
            self.activity("browser_refused", &summary, run);
            return Reply::Note(note);
        }
        self.activity("browser_act", &summary, run);
        let since = Utc::now();
        match tokio::time::timeout(TOOL_WAIT, self.browser().upload(r, name, bytes)).await {
            Ok(Ok(view)) => match self.page(&view, false, since) {
                Reply::Page(page) => Reply::PageWithNote {
                    page,
                    note: "파일 칸에 파일을 넣었다. 아직 보낸 것은 아니다: 올리려면 그 폼의 제출 단추를 누른다(그때 다시 승인을 받는다)."
                        .into(),
                },
                other => other,
            },
            Ok(Err(e)) => Reply::Note(self.explain(&e, None, since)),
            Err(_) => Reply::Note("브라우저가 제때 답하지 않았다.".into()),
        }
    }

    /// 파일 받기를 잇는다(데몬이 뜰 때 한 번).
    pub fn set_downloads(&self, downloads: Arc<crate::downloads::Downloads>) {
        let _ = self.downloads.set(downloads);
    }

    /// 넘겨받는 중이면 남은 시간. 최대 시간이 지났으면 `None`(끝내기는 [`Self::expire_handover`]).
    pub fn handover_left(&self) -> Option<Duration> {
        let started = (*self.handover.lock().expect("handover lock"))?;
        HANDOVER_MAX.checked_sub(started.elapsed())
    }

    /// 넘겨받기 시작(관리 페이지, 로그인한 사람만). 실행 중인 쪽이 있어도 사람이 먼저다.
    pub async fn begin_handover(&self) -> Result<(), String> {
        self.browser()
            .set_handover(true)
            .await
            .map_err(|e| e.to_string())?;
        *self.handover.lock().expect("handover lock") = Some(std::time::Instant::now());
        *self.lease.lock().expect("lease lock") = None;
        self.logins.set_handover(true);
        let url = self
            .browser()
            .handover_tabs()
            .await
            .ok()
            .and_then(|t| t.first().map(|t| t.url.clone()))
            .unwrap_or_default();
        self.activity(
            "handover_start",
            &crate::mask::global().mask(&format!("화면 넘겨받기 시작: {url}")),
            None,
        );
        Ok(())
    }

    /// 넘겨받기 끝. 넘겨받는 동안 열린 탭은 닫고, 마지막 주소들만 기록한다(입력은 기록하지 않는다).
    pub async fn end_handover(&self, why: &str) -> Result<(), String> {
        let was = self.handover.lock().expect("handover lock").take();
        if was.is_none() {
            return Ok(());
        }
        let urls: Vec<String> = self
            .browser()
            .handover_tabs()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.url)
            .collect();
        self.logins.set_handover(false);
        let result = self.browser().set_handover(false).await;
        self.activity(
            "handover_end",
            &crate::mask::global().mask(&format!("화면 넘겨받기 끝({why}): {}", urls.join(", "))),
            None,
        );
        result.map_err(|e| e.to_string())
    }

    /// 최대 시간이 지난 넘겨받기를 끝낸다.
    pub async fn expire_handover(&self) {
        let started = *self.handover.lock().expect("handover lock");
        if started.is_some_and(|s| s.elapsed() >= HANDOVER_MAX) {
            let _ = self.end_handover("시간이 다 됨").await;
        }
    }

    pub async fn handover_tabs(&self) -> Result<Vec<super::Tab>, String> {
        self.require_handover()?;
        self.browser()
            .handover_tabs()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn handover_frame(&self, tab: &str) -> Result<super::Frame, String> {
        self.require_handover()?;
        self.browser()
            .handover_frame(tab)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn handover_input(&self, tab: &str, input: &super::HandInput) -> Result<(), String> {
        self.require_handover()?;
        self.browser()
            .handover_input(tab, input)
            .await
            .map_err(|e| e.to_string())
    }

    fn require_handover(&self) -> Result<(), String> {
        match self.handover_left() {
            Some(_) => Ok(()),
            None => Err("넘겨받은 상태가 아니다(시간이 다 됐으면 다시 시작한다)".into()),
        }
    }

    /// 브라우저 하나를 상주 대화와 작업들이 함께 쓰므로, 한쪽이 쓰는 동안(마지막 사용 뒤 3분까지) 다른 쪽은
    /// 기다리게 한다(명세 "자원 종류별로 동시 실행 수를 제한", 결정 0014). 페이지와 요소 참조가 섞이지 않게 한다.
    pub fn claim(&self, who: &str) -> Result<(), String> {
        // 사람이 화면을 넘겨받았으면 아무 실행도 쓰지 않는다(결정 0017).
        if self.handover.lock().expect("handover lock").is_some() {
            return Err(
                "사람이 브라우저 화면을 넘겨받아 쓰고 있다. 몇 분 뒤 다시 한다.".to_owned(),
            );
        }
        // 인증 코드를 기다리는 동안은 아무도 쓰지 않는다(페이지를 그 단계에 붙잡아 둔다, 결정 0017).
        if self.logins.code_pending() {
            return Err(
                "브라우저가 사이트 로그인의 인증 코드를 기다리고 있다. 몇 분 뒤 다시 한다."
                    .to_owned(),
            );
        }
        let mut lease = self.lease.lock().expect("lease lock");
        let now = std::time::Instant::now();
        match lease.as_ref() {
            Some((owner, last)) if owner != who && now.duration_since(*last) < LEASE_IDLE => Err(
                "브라우저를 다른 작업이 쓰고 있다. 몇 분 뒤 다시 하거나, 브라우저 없이 할 수 있는 만큼 하고 그렇다고 보고한다."
                    .to_owned(),
            ),
            _ => {
                *lease = Some((who.to_owned(), now));
                Ok(())
            }
        }
    }

    pub fn egress(&self) -> &Arc<Egress> {
        &self.egress
    }

    pub fn proxy(&self) -> Option<&Arc<Proxy>> {
        self.proxy.as_ref()
    }

    fn browser(&self) -> &Arc<dyn Browser> {
        self.logins.browser()
    }

    fn activity(&self, kind: &str, text: &str, run: Option<&str>) {
        let _ = self.store.lock().expect("store lock").add_activity(
            kind,
            &crate::mask::global().mask(text),
            run,
            Utc::now(),
        );
    }

    fn remember(&self, view: &PageView) {
        *self.seen.lock().expect("seen lock") = Some(Seen {
            url: view.url.clone(),
            elements: view
                .elements
                .iter()
                .map(|e| (e.r.clone(), e.clone()))
                .collect(),
        });
    }

    /// 실패에 프록시가 막은 까닭을 덧붙인다.
    fn explain(
        &self,
        err: &BrowserError,
        url: Option<&str>,
        since: chrono::DateTime<Utc>,
    ) -> String {
        let mut msg = err.to_string();
        if let Some(proxy) = &self.proxy {
            let host = url
                .and_then(|u| url::Url::parse(u).ok())
                .and_then(|u| u.host_str().map(str::to_owned));
            let reason = host
                .as_deref()
                .and_then(|h| proxy.block_reason(h, since))
                .or_else(|| {
                    proxy
                        .blocked_since(since)
                        .first()
                        .and_then(|d| d.blocked.clone())
                });
            if let Some(r) = reason {
                msg.push_str(&format!(" (데몬 프록시가 막음: {r})"));
            }
        }
        msg
    }

    /// 화면을 보여 줄 때 이번에 막은 연결을 알린다(하위 요청 포함).
    fn blocked_note(&self, since: chrono::DateTime<Utc>) -> String {
        let Some(proxy) = &self.proxy else {
            return String::new();
        };
        let blocked = proxy.blocked_since(since);
        if blocked.is_empty() {
            return String::new();
        }
        let mut hosts: Vec<String> = Vec::new();
        for d in &blocked {
            if !hosts.contains(&d.host) {
                hosts.push(d.host.clone());
            }
        }
        format!(
            "막은 연결 {}개(차단 목록·내부 주소): {}\n",
            blocked.len(),
            hosts.into_iter().take(5).collect::<Vec<_>>().join(", ")
        )
    }

    fn page(&self, view: &PageView, compact: bool, since: chrono::DateTime<Utc>) -> Reply {
        self.remember(view);
        let mut text = format_view(view, compact);
        let note = self.blocked_note(since);
        if !note.is_empty() {
            text.push_str(&note);
        }
        Reply::Page(text)
    }

    /// 사람 없는 실행이고 위임 범위로 묶이면 그 호출자(결정 0029).
    fn bounded(&self) -> Option<crate::delegation::Caller> {
        crate::delegation::caller().filter(crate::delegation::bounded)
    }

    /// 이 호스트를 덮는 위임(쓴 것으로 적는다).
    fn grant(&self, host: &str) -> Option<lucia_store::SiteGrant> {
        let store = self.store.lock().expect("store lock");
        let g = crate::delegation::grant_for(&store, host, Utc::now())?;
        let _ = store.touch_site_grant(&g.id, Utc::now());
        Some(g)
    }

    /// 사람 없는 실행이 위임 밖으로 가려 하면 카드를 올리고 모델에게 줄 말을 돌려준다.
    fn egress_check(&self, url: &str, why: &str) -> Option<String> {
        let c = self.bounded()?;
        let host = crate::delegation::host_of(url)?;
        if self.grant(&host).is_some() {
            return None;
        }
        Some(crate::delegation::request_egress(
            &self.store,
            self.posts.as_ref(),
            self.cfg.tz,
            &host,
            url,
            why,
            &c.run,
        ))
    }

    /// 나가는 글(주소의 경로·쿼리, 넣는 글)에 읽은 비공개 자료나 개인정보가 실렸는지 본다(결정 0029).
    /// 늘 기록하고(도메인과 까닭만, 글은 남기지 않는다), 관리 페이지에서 "막기"로 정했을 때만 까닭을 돌려준다.
    fn leak_gate(
        &self,
        outgoing: &str,
        url: &str,
        what: &str,
        run: Option<&str>,
    ) -> Option<String> {
        let why = crate::leak::inspect(outgoing)?;
        let host = crate::delegation::host_of(url).unwrap_or_default();
        let store = self.store.lock().expect("store lock");
        let block = crate::leak::blocking(&store);
        let _ = store.add_activity(
            "egress_flag",
            &format!(
                "{what} → {host}: {why}{}",
                if block { " (막음)" } else { " (기록만)" }
            ),
            run,
            Utc::now(),
        );
        block.then_some(why)
    }

    /// 동작 뒤 도착한 곳이 위임 밖이면(넘겨주기 등) 화면을 돌려주지 않는다.
    fn arrival_check(&self, url: &str) -> Option<Reply> {
        self.egress_check(url, "넘어간 곳(넘겨주기·스크립트 이동)")
            .map(|note| {
                Reply::Note(format!(
                    "이동한 곳이 위임 밖이라 화면을 읽지 않았다. {note}"
                ))
            })
    }

    /// 주소의 파일을 받는다(결정 0057). 나갈 수 있는 곳은 `browser_open`과 같다: 주소 검사, 사람 없는 실행은
    /// 위임한 사이트만(결정 0029), 나가는 글 검사. `session`이면 로그인된 브라우저의 쿠키를 빌린다.
    async fn fetch_file(
        &self,
        raw: &str,
        session: bool,
        name: Option<&str>,
        run: Option<&str>,
        wait: Duration,
    ) -> Result<crate::downloads::Outcome, String> {
        let Some(downloads) = self.downloads.get() else {
            return Err("이 데몬에서는 파일을 받을 수 없다.".into());
        };
        let url = self
            .egress
            .check_url(raw)
            .map_err(|why| format!("받을 수 없는 주소다: {why}"))?;
        if let Some(note) = self.egress_check(url.as_str(), "Lucia가 받으려던 파일") {
            return Err(note);
        }
        if let Some(why) = self.leak_gate(
            &crate::leak::carried(url.as_str()),
            url.as_str(),
            "파일 받기",
            run,
        ) {
            return Err(format!(
                "주소에 사용자 자료로 보이는 글({why})이 실려 있어 받지 않았다. 사용자 정보는 사용자가 그 사이트에 보내라고 한 것만 싣는다. 필요하면 사용자에게 확인을 받는다."
            ));
        }
        let via = if session {
            crate::watch::Via::Session
        } else {
            crate::watch::Via::Plain
        };
        Ok(downloads.get(url.as_str(), via, name, run, wait).await)
    }

    /// `file_download`.
    pub async fn download(
        &self,
        raw: &str,
        session: bool,
        name: Option<&str>,
        run: Option<&str>,
    ) -> Reply {
        Reply::Note(
            match self
                .fetch_file(raw, session, name, run, crate::downloads::TOOL_WAIT)
                .await
            {
                Ok(outcome) => crate::downloads::render(&outcome),
                Err(why) => why,
            },
        )
    }

    /// 브라우저가 시작하려던 내려받기를 데몬이 대신 받고(브라우저 안에는 저장하지 않는다) 알릴 말을 만든다.
    /// 브라우저가 보냈을 요청과 같게 로그인 세션으로 받는다.
    async fn replay_downloads(
        &self,
        run: Option<&str>,
        started: std::time::Instant,
    ) -> Option<String> {
        let seen = self.browser().take_downloads();
        if seen.is_empty() {
            return None;
        }
        // 이 호출에 남은 시간 안에서만 기다린다(받기 자체는 데몬 작업으로 이어진다).
        let deadline = std::time::Instant::now()
            + REPLAY_WAIT.min(CALL_BUDGET.saturating_sub(started.elapsed()));
        let mut out = String::from(
            "브라우저가 파일을 내려받으려 해서 데몬이 대신 받았다(브라우저 안에는 저장하지 않는다).\n",
        );
        for d in seen.iter().take(REPLAY_MAX) {
            if !d.url.starts_with("http://") && !d.url.starts_with("https://") {
                out.push_str("- 받지 못했다: 페이지가 스크립트로 만든 파일(blob·data 주소)은 데몬이 받을 수 없다.\n");
                continue;
            }
            let wait = deadline.saturating_duration_since(std::time::Instant::now());
            let line = match self.fetch_file(&d.url, true, None, run, wait).await {
                Ok(crate::downloads::Outcome::Pending(_)) => format!(
                    "아직 받는 중이다: {} — file_download에 이 주소와 session=true를 주면 이어서 기다린다.",
                    d.url
                ),
                Ok(outcome) => crate::downloads::render(&outcome),
                Err(why) => format!("받지 못했다: {why}"),
            };
            out.push_str(&format!("- {line}\n"));
        }
        Some(out)
    }

    /// 동작의 답에 그 동작이 일으킨 내려받기를 덧붙인다.
    async fn with_downloads(
        &self,
        reply: Reply,
        run: Option<&str>,
        started: std::time::Instant,
    ) -> Reply {
        let Some(note) = self.replay_downloads(run, started).await else {
            return reply;
        };
        match reply {
            Reply::Page(page) => Reply::PageWithNote { page, note },
            Reply::Note(text) => Reply::Note(format!("{text}\n{note}")),
            Reply::PageWithNote { page, note: first } => Reply::PageWithNote {
                page,
                note: format!("{first}\n{note}"),
            },
        }
    }

    /// 화면이 비어 있으면(글도 요소도 없다) 그 주소가 페이지가 아니라 파일(브라우저가 뷰어로 연 PDF·그림)인지
    /// 받아 본다. 파일이면 알릴 말. 페이지면(HTML) 아무것도 남기지 않는다.
    async fn file_behind_blank(
        &self,
        view: &PageView,
        run: Option<&str>,
        started: std::time::Instant,
    ) -> Option<String> {
        let blank =
            view.text.trim().is_empty() && view.elements.is_empty() && view.frames.is_empty();
        if !blank || self.downloads.get().is_none() {
            return None;
        }
        let wait = REPLAY_WAIT.min(CALL_BUDGET.saturating_sub(started.elapsed()));
        match self.fetch_file(&view.url, true, None, run, wait).await {
            Ok(crate::downloads::Outcome::Done(got)) => Some(format!(
                "이 주소는 페이지가 아니라 파일이다(브라우저 화면에는 읽을 글이 없다). 데몬이 받아 두었다.\n{}",
                crate::downloads::describe(&got)
            )),
            _ => None,
        }
    }

    /// `browser_open`.
    pub async fn open(&self, raw: &str, run: Option<&str>) -> Reply {
        let started = std::time::Instant::now();
        let reply = self.open_page(raw, run, started).await;
        self.with_downloads(reply, run, started).await
    }

    async fn open_page(&self, raw: &str, run: Option<&str>, started: std::time::Instant) -> Reply {
        let url = match self.egress.check_url(raw) {
            Ok(u) => u,
            Err(why) => return Reply::Note(format!("열 수 없는 주소다: {why}")),
        };
        if let Some(note) = self.egress_check(url.as_str(), "Lucia가 열려던 주소") {
            return Reply::Note(note);
        }
        if let Some(why) = self.leak_gate(
            &crate::leak::carried(url.as_str()),
            url.as_str(),
            "주소 열기",
            run,
        ) {
            return Reply::Note(format!(
                "주소에 사용자 자료로 보이는 글({why})이 실려 있어 열지 않았다. 사용자 정보는 사용자가 그 사이트에 보내라고 한 것만 싣는다. 필요하면 사용자에게 확인을 받는다."
            ));
        }
        let site = self.logins.builtin_for(&url).cloned();
        self.logins.note_open(url.as_str(), run);
        let since = Utc::now();
        let opened =
            tokio::time::timeout(TOOL_WAIT, self.browser().open(url.as_str(), site.as_ref())).await;
        let view = match opened {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                // 파일 주소면 브라우저는 열지 않고 내려받기를 시작하려 한다(결정 0057): 그 사건을 기다렸다가
                // 데몬이 받는다. 사건이 없으면 실패를 그대로 알린다.
                if e.to_string().contains("ERR_ABORTED") {
                    tokio::time::sleep(DOWNLOAD_EVENT_WAIT).await;
                    if let Some(note) = self.replay_downloads(run, started).await {
                        return Reply::Note(format!(
                            "이 주소는 페이지가 아니라 파일이라 브라우저 화면으로는 열리지 않는다.\n{note}"
                        ));
                    }
                }
                return Reply::Note(self.explain(&e, Some(url.as_str()), since));
            }
            Err(_) => {
                return Reply::Note(
                    "브라우저가 아직 페이지를 열지 못했다(처음 열 때는 시간이 걸린다). 잠시 뒤 같은 주소를 다시 열어라.".into(),
                );
            }
        };
        if view.login_redirect
            && let Some(site) = &site
        {
            return Reply::Note(
                self.request_login(site, "Lucia가 페이지를 열다가 로그인이 필요했습니다.", run)
                    .await,
            );
        }
        if let Some(blocked) = self.arrival_check(&view.url) {
            return blocked;
        }
        if let Some(note) = self.file_behind_blank(&view, run, started).await {
            return match self.page(&view, false, since) {
                Reply::Page(page) => Reply::PageWithNote { page, note },
                other => other,
            };
        }
        self.page(&view, false, since)
    }

    /// `browser_snapshot`. 앞선 동작 뒤 늦게 시작된 내려받기도 여기서 받는다.
    pub async fn snapshot(&self, run: Option<&str>) -> Reply {
        let started = std::time::Instant::now();
        let reply = self.snapshot_page().await;
        self.with_downloads(reply, run, started).await
    }

    async fn snapshot_page(&self) -> Reply {
        let since = Utc::now();
        match tokio::time::timeout(TOOL_WAIT, self.browser().view()).await {
            Ok(Ok(v)) => self.page(&v, false, since),
            Ok(Err(e)) => Reply::Note(e.to_string()),
            Err(_) => Reply::Note("화면을 읽지 못했다(시간 초과).".into()),
        }
    }

    /// `browser_screenshot`.
    pub async fn screenshot(&self, full_page: bool) -> Result<Shot, String> {
        match tokio::time::timeout(TOOL_WAIT, self.browser().screenshot(full_page)).await {
            Ok(Ok(s)) => Ok(s),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("스크린샷을 찍지 못했다(시간 초과).".into()),
        }
    }

    /// `browser_click`·`browser_type`.
    pub async fn act(&self, act: Act, run: Option<&str>) -> Reply {
        let started = std::time::Instant::now();
        let reply = self.act_on(act, run).await;
        self.with_downloads(reply, run, started).await
    }

    async fn act_on(&self, act: Act, run: Option<&str>) -> Reply {
        let Some(r) = act.target().map(str::to_owned) else {
            return self.simple(act).await;
        };
        let known = self
            .seen
            .lock()
            .expect("seen lock")
            .as_ref()
            .and_then(|s| s.elements.get(&r).cloned().map(|e| (s.url.clone(), e)));
        let Some((seen_url, seen_el)) = known else {
            return Reply::Note(format!(
                "모르는 요소 참조다: {r}. browser_snapshot(또는 browser_open)으로 화면을 다시 보고, 그 목록의 참조를 쓴다."
            ));
        };
        let (url, el) = match self.browser().inspect(&r).await {
            Ok(x) => x,
            Err(e) => return Reply::Note(e.to_string()),
        };
        if strip_fragment(&url) != strip_fragment(&seen_url) || !el.same_target(&seen_el) {
            return Reply::Note(format!(
                "화면이 바뀌어 {r}가 마지막으로 본 요소와 다르다. browser_snapshot으로 다시 본다."
            ));
        }
        let summary = format!(
            "{} {} “{}” @ {}",
            act_ko(&act),
            kind_ko(&el.kind),
            clip(&el.label, 60),
            clip(&url, 120)
        );
        let gate = classify(&act, &el, &url);
        self.gated(act, url, el, gate, summary, run).await
    }

    /// `browser_click_at`(결정 0023): 데몬이 그 점의 요소를 읽어 등급을 정한다.
    pub async fn click_at(&self, at: Point, run: Option<&str>) -> Reply {
        let started = std::time::Instant::now();
        let reply = self.click_at_on(at, run).await;
        self.with_downloads(reply, run, started).await
    }

    async fn click_at_on(&self, at: Point, run: Option<&str>) -> Reply {
        let hit = match tokio::time::timeout(TOOL_WAIT, self.browser().hit(at)).await {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => return Reply::Note(e.to_string()),
            Err(_) => return Reply::Note("화면을 읽지 못했다(시간 초과).".into()),
        };
        let (w, h) = hit.viewport;
        if at.x < 0 || at.y < 0 || (w > 0 && at.x >= w) || (h > 0 && at.y >= h) {
            return Reply::Note(format!(
                "화면 밖 좌표다(화면 {w}×{h}). 지금 보이는 화면의 스크린샷 좌표를 쓴다(전체 페이지 스크린샷의 좌표가 아니다)."
            ));
        }
        let el = hit.target();
        let summary = format!(
            "{} {} “{}”{} @ {}",
            act_ko(&Act::ClickAt { at }),
            kind_ko(&el.kind),
            clip(&el.label, 60),
            el.frame
                .as_deref()
                .map(|f| format!(" (iframe {})", clip(f, 80)))
                .unwrap_or_default(),
            clip(&hit.url, 120)
        );
        let gate = classify_hit(&hit);
        self.gated(Act::ClickAt { at }, hit.url, el, gate, summary, run)
            .await
    }

    /// `browser_type_text`·`browser_key`(결정 0023): 초점이 있는 요소로 등급을 정한다.
    pub async fn at_focus(&self, act: Act, run: Option<&str>) -> Reply {
        let started = std::time::Instant::now();
        let reply = self.at_focus_on(act, run).await;
        self.with_downloads(reply, run, started).await
    }

    async fn at_focus_on(&self, act: Act, run: Option<&str>) -> Reply {
        let (url, focus) = match tokio::time::timeout(TOOL_WAIT, self.browser().focused()).await {
            Ok(Ok(f)) => f,
            Ok(Err(e)) => return Reply::Note(e.to_string()),
            Err(_) => return Reply::Note("화면을 읽지 못했다(시간 초과).".into()),
        };
        let el = focus.target();
        let summary = format!(
            "{} {} “{}”{} @ {}",
            act_ko(&act),
            kind_ko(&el.kind),
            clip(&el.label, 60),
            el.frame
                .as_deref()
                .map(|f| format!(" (iframe {})", clip(f, 80)))
                .unwrap_or_default(),
            clip(&url, 120)
        );
        let gate = classify_focus(&act, &url, &focus);
        self.gated(act, url, el, gate, summary, run).await
    }

    async fn gated(
        &self,
        act: Act,
        url: String,
        element: Element,
        gate: Gate,
        summary: String,
        run: Option<&str>,
    ) -> Reply {
        // 사람 없는 실행(결정 0029): 누를 링크가 위임 밖이면 묻는다.
        if self.bounded().is_some()
            && !matches!(gate, Gate::Forbidden(_))
            && let Some(href) = element.href.as_deref()
            && let Some(note) = self.egress_check(href, "누르려던 링크의 주소")
        {
            return Reply::Note(note);
        }
        // 권한 모드(결정 0040): 엄격이면 그냥 하던 입력도 묻고 위임의 "동작"을 쓰지 않는다. 해제이고 사람이 대화
        // 중이면 승인을 그냥으로 바꾼다(금지는 그대로). 판정 함수(`classify*`)는 그대로 두고 여기서만 고친다.
        let mode = crate::security::mode(&self.store.lock().expect("store lock"));
        let human = crate::delegation::caller().is_some_and(|c| c.human);
        // 결정 0059: 부른 쪽에 맞게 등급을 푼다.
        // - 사람 전용 동작(로그아웃, 비밀번호 칸·로그인 폼)은 사람이 대화 중이면 그냥, 아니면 하지 않는다.
        // - 돈·계정은 사람이 대화 중이면 늘 승인 카드(`critical`: 위임도 해제 모드도 건너뛰지 못한다), 아니면 하지 않는다.
        // - 바깥에 남는 쓰기는 늘 승인 카드(`must_ask`: 해제 모드가 건너뛰지 못한다. 위임의 동작 허용은 건너뛴다).
        let mut must_ask = false;
        let mut critical = false;
        let gate = match gate {
            Gate::HumanOnly(_) if human => Gate::Free,
            Gate::HumanOnly(what) => Gate::Forbidden(format!("{what}: 사람이 대화 중일 때만 한다")),
            Gate::Critical(why) if human => {
                must_ask = true;
                critical = true;
                Gate::Approval(format!("{why}(돈·계정에 닿는 동작)"))
            }
            Gate::Critical(what) => Gate::Forbidden(format!(
                "{what}: 사람 없는 실행에서는 하지 않는다(사용자가 대화에서 승인해야 한다)"
            )),
            Gate::Publish(why) => {
                must_ask = true;
                Gate::Approval(why)
            }
            other => other,
        };
        // 결제·주문 페이지(또는 그런 주소의 iframe)에서는 가벼운 확인도 건너뛰지 않는다: 확인하지 못한 자리를 누르는
        // 것이 결제 단추일 수 있다. 사람 없는 실행은 그런 페이지에서 확인이 필요한 동작을 하지 않는다.
        let on_money_page = payment_page(element.frame.as_deref().unwrap_or(&url));
        let gate = match gate {
            Gate::Approval(why) if on_money_page && !critical => {
                if human {
                    must_ask = true;
                    critical = true;
                    Gate::Approval(format!("{why}(결제·주문 페이지)"))
                } else {
                    Gate::Forbidden(format!(
                        "{why}: 결제·주문 페이지에서는 사람 없는 실행이 하지 않는다"
                    ))
                }
            }
            other => other,
        };
        let gate = match mode {
            crate::security::Mode::Strict => strict_gate(&act, &element, gate),
            _ => gate,
        };
        // 위임이 동작을 허용한 사이트면 누가 불렀든 다시 묻지 않는다(결정 0028: 한 번 허용하면 그 뒤는 묻지 않는다).
        // 돈·계정은 위임으로 건너뛰지 못한다.
        let gate = match gate {
            Gate::Approval(why) if mode != crate::security::Mode::Strict && !critical => {
                let host = crate::delegation::host_of(&url).unwrap_or_default();
                match self.grant(&host) {
                    Some(g) if g.actions => {
                        self.activity(
                            "browser_delegated",
                            &format!("{summary}: 위임({})으로 승인 없이 함 — {why}", g.host),
                            run,
                        );
                        Gate::Free
                    }
                    _ => Gate::Approval(why),
                }
            }
            other => other,
        };
        // 나가는 글 검사(결정 0029): 넣는 글과 누를 링크의 주소. "막기"면 승인으로 돌린다(금지는 그대로).
        // 위임의 "동작 허용"보다 뒤에 둔다: 사용자 자료가 실린 것은 위임이 있어도 묻는다.
        let outgoing = match &act {
            Act::Type { text, .. } | Act::TypeText { text, .. } => Some(("글 넣기", text.clone())),
            _ => element
                .href
                .as_deref()
                .map(|h| ("링크 누르기", crate::leak::carried(h))),
        };
        let gate = match (gate, outgoing) {
            (Gate::Forbidden(w), _) => Gate::Forbidden(w),
            (g, Some((what, text))) => match self.leak_gate(&text, &url, what, run) {
                Some(why) => Gate::Approval(format!("사용자 자료로 보이는 글: {why}")),
                None => g,
            },
            (g, None) => g,
        };
        // 해제로 건너뛴 동작은 사람이 승인한 동작처럼 한다(확인 대화상자를 받아들인다). 바깥에 남는 쓰기와
        // 돈·계정은 해제에서도 묻는다(결정 0059).
        let mut granted = false;
        let gate = match gate {
            Gate::Approval(why) if mode == crate::security::Mode::Open && human && !must_ask => {
                self.activity(
                    "browser_open_mode",
                    &format!("{summary}: 권한 모드 해제라 묻지 않고 함 — {why}"),
                    run,
                );
                granted = true;
                Gate::Free
            }
            other => other,
        };
        match gate {
            Gate::Forbidden(why) => {
                self.activity("browser_refused", &format!("{summary}: {why}"), run);
                Reply::Note(format!("하지 않았다: {why}"))
            }
            Gate::Free => {
                self.activity("browser_act", &summary, run);
                let expect = Expect {
                    url,
                    element,
                    approved: granted,
                };
                self.run_act(&act, &expect).await
            }
            Gate::Approval(why) => {
                // 승인 카드는 동작(넣을 글 포함)을 저장소에 남긴다. 비밀번호는 남기지 않는다.
                if matches!(act, Act::Type { .. } | Act::TypeText { .. })
                    && (element.kind == "password"
                        || element.form.as_ref().is_some_and(|f| f.password))
                {
                    self.activity("browser_refused", &format!("{summary}: {why}"), run);
                    return Reply::Note(format!(
                        "하지 않았다: {why}. 비밀번호 칸의 입력은 승인 카드로 미루지 않는다(값을 남기지 않으려고). 로그인은 browser_login을 쓴다."
                    ));
                }
                let expect = Expect {
                    url,
                    element,
                    approved: true,
                };
                self.with_approval(act, expect, why, summary, run).await
            }
            // 위에서 모두 풀었다.
            Gate::Publish(why) | Gate::Critical(why) | Gate::HumanOnly(why) => {
                Reply::Note(format!("하지 않았다: {why}"))
            }
        }
    }

    async fn simple(&self, act: Act) -> Reply {
        let since = Utc::now();
        match tokio::time::timeout(TOOL_WAIT, self.browser().act(&act, None)).await {
            Ok(Ok(v)) => {
                if let Some(blocked) = self.arrival_check(&v.url) {
                    return blocked;
                }
                self.page(&v, matches!(act, Act::Scroll { .. }), since)
            }
            Ok(Err(e)) => Reply::Note(self.explain(&e, None, since)),
            Err(_) => Reply::Note("브라우저가 제때 답하지 않았다.".into()),
        }
    }

    async fn run_act(&self, act: &Act, expect: &Expect) -> Reply {
        let since = Utc::now();
        match tokio::time::timeout(TOOL_WAIT, self.browser().act(act, Some(expect))).await {
            Ok(Ok(v)) => {
                if let Some(blocked) = self.arrival_check(&v.url) {
                    return blocked;
                }
                self.page(&v, false, since)
            }
            Ok(Err(e)) => Reply::Note(self.explain(&e, None, since)),
            Err(_) => Reply::Note(
                "브라우저가 제때 답하지 않았다. browser_snapshot으로 지금 화면을 확인한다.".into(),
            ),
        }
    }

    async fn with_approval(
        &self,
        act: Act,
        expect: Expect,
        why: String,
        summary: String,
        run: Option<&str>,
    ) -> Reply {
        let Some(posts) = &self.posts else {
            return Reply::Note("승인이 필요한 동작인데 승인 경로가 없어 하지 않았다.".into());
        };
        let host = url::Url::parse(&expect.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        let id = Uuid::new_v4().simple().to_string();
        let now = Utc::now();
        let typed = match &act {
            Act::Type { text, .. } | Act::TypeText { text, .. } => Some(text.as_str()),
            _ => None,
        };
        let risks =
            crate::risk::assess_browser(&self.store.lock().expect("store lock"), &host, typed);
        // 사람 없는 실행이 올리는 카드는 길게 살고, 방해 금지 시간에는 미루고, 승인되면 이어 맡긴다(결정 0030).
        let unattended = crate::delegation::caller().filter(|c| !c.human);
        // 만료는 잠금을 잡기 전에 정한다(std 잠금은 다시 들어갈 수 없다: 같은 식 안에서 두 번 잡으면 멈춘다).
        let expires_at = match &unattended {
            Some(_) => crate::delegation::unattended_card(
                &self.store.lock().expect("store lock"),
                self.cfg.tz,
                now,
            ),
            None => now + self.cfg.approval_ttl,
        };
        let created = self.store.lock().expect("store lock").create_approval(
            &NewApproval {
                id: id.clone(),
                kind: KIND_BROWSER_ACTION.into(),
                payload: json!({
                    "act": act,
                    "expect": expect,
                    "host": host,
                    "reason": why,
                    "risks": risks,
                    "defer_quiet": unattended.is_some(),
                    "resume": unattended.as_ref().map(|c| crate::delegation::resume_of(&c.run)),
                }),
                summary: crate::mask::global().mask(&format!("브라우저 {summary}")),
                run_id: run.map(str::to_owned),
                expires_at,
            },
            now,
        );
        if let Err(e) = created {
            return Reply::Note(format!("승인 요청을 만들지 못했다: {e}"));
        }
        self.activity(
            "browser_approval",
            &format!("{summary} 승인 요청({why})"),
            run,
        );
        let (tx, rx) = oneshot::channel();
        self.waiters
            .lock()
            .expect("waiters lock")
            .insert(id.clone(), tx);
        if posts.send(id.clone()).is_err() {
            self.waiters.lock().expect("waiters lock").remove(&id);
            return Reply::Note("승인 데스크가 멈춰 하지 않았다.".into());
        }
        let short: String = id.chars().take(8).collect();
        match tokio::time::timeout(self.cfg.approval_wait, rx).await {
            Ok(Ok(Ok(page))) => Reply::Page(format!("[사용자가 승인해 실행했다]\n{page}")),
            Ok(Ok(Err(why))) => Reply::Note(format!("하지 않았다: {why}")),
            Ok(Err(_)) => Reply::Note("승인 요청이 처리되지 않았다.".into()),
            Err(_) => {
                self.waiters.lock().expect("waiters lock").remove(&id);
                Reply::Note(format!(
                    "승인이 필요한 동작이다({why}). Discord에 승인 카드를 올렸다(요청 {short}). 아직 답이 없다. \
                     사용자가 [실행]을 누르면 데몬이 그때 실행하고 결과를 카드와 기록에 남긴다. 같은 동작을 다시 요청하지 말고, \
                     사용자에게 카드를 확인해 달라고 보고하라."
                ))
            }
        }
    }

    /// `browser_login`: 내장 사이트 ID 또는 로그인 페이지 주소.
    pub async fn login(&self, site_or_url: &str, run: Option<&str>) -> Reply {
        let arg = site_or_url.trim();
        let mut remember = None;
        let site = if arg.contains("://") {
            let url = match self.egress.check_url(arg) {
                Ok(u) => u,
                Err(why) => return Reply::Note(format!("열 수 없는 주소다: {why}")),
            };
            match self.logins.profile_for(&url) {
                Ok(s) => {
                    // 저장된 로그인 주소를 바꾸는 것은 위임 확인을 지난 뒤에(아래).
                    remember = Some(s.clone());
                    s
                }
                Err(why) => return Reply::Note(format!("로그인할 수 없다: {why}")),
            }
        } else {
            match self.logins.site(arg) {
                Some(s) => s,
                None => {
                    return Reply::Note(format!(
                        "모르는 사이트다: {arg}. 내장 사이트 ID({}) 또는 로그인 페이지의 https 주소를 넣는다.",
                        self.logins
                            .sites()
                            .iter()
                            .map(|s| s.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
        };
        // 사람 없는 실행(결정 0011·0028): 사람이 로그인을 위임한 사이트만. 아무 주소로나 비밀번호 입력 링크를
        // 보내 피싱 통로가 되지 않게 한다.
        if crate::delegation::caller().is_some_and(|c| !c.human) {
            let host = site
                .hosts
                .first()
                .map(|h| h.trim_start_matches("*.").to_owned())
                .unwrap_or_default();
            let delegated = crate::delegation::grant_for(
                &self.store.lock().expect("store lock"),
                &host,
                Utc::now(),
            )
            .is_some_and(|g| g.login);
            if !delegated {
                return Reply::Note(format!(
                    "사람 없는 실행에서는 사용자가 로그인을 위임한 사이트만 로그인한다. {} 로그인이 필요하다고 사용자에게 알린다.",
                    site.display()
                ));
            }
        }
        if let Some(s) = &remember {
            self.logins.remember_login_url(s);
        }
        Reply::Note(
            self.request_login(&site, "Lucia가 로그인을 요청했습니다.", run)
                .await,
        )
    }

    async fn request_login(
        &self,
        site: &super::SiteProfile,
        why: &str,
        run: Option<&str>,
    ) -> String {
        let name = site.display();
        // 사람 없는 실행(결정 0028·0030): 위임이 로그인을 허용한 사이트면 저장된 로그인으로 묻지 않고 다시 로그인한다.
        // 아니면 카드를 길게 살리고 방해 금지 시간에는 미루며, 승인되면 이어 맡긴다.
        let unattended = crate::delegation::caller().filter(|c| !c.human);
        {
            // 로그인을 위임한 사이트면 누가 불렀든(대화 중이어도) 저장된 로그인으로 묻지 않고 다시 로그인한다.
            let host = site
                .hosts
                .first()
                .map(|h| h.trim_start_matches("*.").to_owned())
                .unwrap_or_default();
            // 권한 모드(결정 0040): 엄격이면 위임의 "로그인"을 쓰지 않고, 해제이고 사람이 대화 중이면 저장된 로그인을
            // 묻지 않고 쓴다.
            let mode = crate::security::mode(&self.store.lock().expect("store lock"));
            let human = crate::delegation::caller().is_some_and(|c| c.human);
            let delegated =
                mode != crate::security::Mode::Strict && self.grant(&host).is_some_and(|g| g.login);
            let open = mode == crate::security::Mode::Open && human;
            if (delegated || open) && self.logins.has_saved(&site.id) {
                self.activity(
                    "login_delegated",
                    &format!("{name}: 위임(또는 권한 모드 해제)으로 저장된 로그인을 묻지 않고 씀"),
                    run,
                );
                return match self
                    .logins
                    .login_delegated(
                        &site.id,
                        if delegated {
                            "로그인 위임(브라우저 도구)"
                        } else {
                            "권한 모드 해제(대화 중)"
                        },
                    )
                    .await
                {
                    Ok(super::logins::SavedLogin::LoggedIn(_)) => format!(
                        "{name}에 저장된 로그인으로 다시 로그인했다(사용자가 위임한 사이트). 열려던 주소를 다시 열어 이어 한다."
                    ),
                    Ok(super::logins::SavedLogin::NeedsCode(note)) => format!(
                        "{name} 로그인에 인증 코드가 필요하다. {note} 사용자가 코드를 넣으면 다시 해야 하므로, 그렇다고 보고한다."
                    ),
                    Err(e) => format!("{name}에 저장된 로그인으로 다시 로그인하지 못했다: {e}"),
                };
            }
        }
        let extra = unattended.map(|c| {
            (
                crate::delegation::unattended_card(
                    &self.store.lock().expect("store lock"),
                    self.cfg.tz,
                    Utc::now(),
                ),
                crate::delegation::resume_of(&c.run),
            )
        });
        let waiting = if extra.is_some() {
            "사용자가 카드에서 [로그인]을 누르면 데몬이 이 일을 이어서 다시 맡긴다. 그렇다고 보고하라."
        } else {
            "사용자에게 카드에서 [로그인]을 누른 뒤 다시 요청해 달라고 보고하라."
        };
        match self.logins.request_with(&site.id, why, run, extra).await {
            Ok(Requested::InputLink) => format!(
                "{name}에 로그인이 필요하다. 데몬이 사용자에게 로그인 입력 링크를 알림으로 보냈다. 사용자에게 링크에서 로그인한 뒤 다시 요청해 달라고 보고하라. 비밀번호를 묻지 않는다."
            ),
            Ok(Requested::InputCard) => format!(
                "{name}에 로그인이 필요하다. 데몬이 사용자의 앱에 로그인 입력 카드를 올렸다(아이디·비밀번호는 카드에서 데몬이 직접 받는다). 사용자에게 카드에서 로그인한 뒤 다시 요청해 달라고 보고하라. 비밀번호를 묻지 않는다."
            ),
            Ok(Requested::ApprovalCard) => format!(
                "{name}에 로그인이 필요하다. 저장된 로그인으로 다시 로그인할지 승인 카드를 올렸다. {waiting}"
            ),
            Ok(Requested::AlreadyPending) => format!(
                "{name} 로그인 요청(입력 링크나 승인 카드)이 이미 사용자에게 가 있다. 그것을 처리해 달라고 보고하라."
            ),
            Err(e) => format!("{name}에 로그인이 필요하지만 요청하지 못했다: {e}"),
        }
    }

    fn notify_waiter(&self, id: &str, outcome: Result<String, String>) {
        let tx = self.waiters.lock().expect("waiters lock").remove(id);
        if let Some(tx) = tx
            && tx.send(outcome).is_ok()
        {
            self.live.lock().expect("live lock").insert(id.to_owned());
        }
    }

    async fn run_approved_inner(&self, id: &str, payload: &Value) -> Result<String, String> {
        // 사람이 화면을 넘겨받아 쓰는 중이면 늦게 누른 승인도 실행하지 않는다(그 페이지는 사람이 다룬다).
        if self.handover.lock().expect("handover lock").is_some() {
            let why = "사람이 브라우저 화면을 넘겨받아 쓰는 중이라 실행하지 않았다".to_owned();
            self.notify_waiter(id, Err(why.clone()));
            return Err(why);
        }
        let act: Act = serde_json::from_value(payload["act"].clone())
            .map_err(|e| format!("저장된 동작: {e}"))?;
        let mut expect: Expect = serde_json::from_value(payload["expect"].clone())
            .map_err(|e| format!("저장된 확인 값: {e}"))?;
        expect.approved = true;
        let since = Utc::now();
        let result = tokio::time::timeout(TOOL_WAIT, self.browser().act(&act, Some(&expect))).await;
        match result {
            Ok(Ok(view)) => {
                let note = format!(
                    "실행했습니다 → {} ({})",
                    clip(&view.title, 60),
                    clip(&view.url, 120)
                );
                let page = match self.page(&view, false, since) {
                    Reply::Page(t) | Reply::Note(t) => t,
                    Reply::PageWithNote { page, .. } => page,
                };
                self.notify_waiter(id, Ok(page));
                Ok(note)
            }
            Ok(Err(e)) => {
                let why = self.explain(&e, None, since);
                self.notify_waiter(id, Err(why.clone()));
                Err(why)
            }
            Err(_) => {
                let why = "브라우저가 제때 답하지 않았다(실행됐는지 모른다)".to_owned();
                self.notify_waiter(id, Err(why.clone()));
                Err(why)
            }
        }
    }
}

impl BrowserActionExec for BrowserTools {
    fn delivered_live(&self, id: &str) -> bool {
        self.live.lock().expect("live lock").remove(id)
    }
    fn run_approved<'a>(
        &'a self,
        id: &'a str,
        payload: &'a Value,
    ) -> BoxFut<'a, Result<String, String>> {
        Box::pin(self.run_approved_inner(id, payload))
    }

    fn closed(&self, id: &str, why: &str) {
        self.notify_waiter(id, Err(why.to_owned()));
    }
}

#[cfg(test)]
mod tests;
