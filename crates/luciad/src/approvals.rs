//! 승인 데스크(결정 0003): 요청을 카드로 올리고, 소유자의 버튼 누름을 받아 데몬이 실행한다.
//!
//! - 메일 발송: 에이전트는 `mail_send`(MCP)로 요청만 만든다. [`request_mail_send`]가 검증하고 내용을 고정해 저장한다.
//! - 사이트 재로그인(결정 0006): 저장된 로그인을 다시 쓸 때마다 승인을 받는다(dots 방식). 요청은
//!   `browser::logins`가 만들고, 승인되면 [`SiteLoginExec`]가 로그인한다.
//! - 브라우저 동작(결정 0008): POST 폼 제출·모르는 스크립트 단추. 요청은 `browser::tools`가 만들고 도구 호출이
//!   잠깐 기다린다. 승인되면 [`BrowserActionExec`]가 같은 주소·같은 요소인지 다시 확인한 뒤 실행한다.
//! - 종류(`kind`)마다 카드 모양과 실행이 다르다. 종류는 [`ApprovalKind`]로 등록한다(결정 0013, `kinds`).
//! - 데스크 작업 하나가 카드 올리기, 버튼 누름, 만료 정리를 차례로 처리한다(동시에 두 번 보내지 않게).
//! - 승인은 `pending`인 행만 바꾸는 조건부 갱신이다. 보내기 실패는 다시 시도하지 않는다.
//! - 결과는 카드에 적고, 에피소드로도 남겨 에이전트가 다음 대화에서 안다.

use crate::gateway::NotifyTarget as _;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use lucia_channel::ChannelAdapter;
use lucia_core::{ButtonPress, Card, CardButton, CardField, CardTone, ConversationId, Outbound};
use lucia_precheck::google::TokenProvider;
use lucia_precheck::google::assist::{GoogleAssist, OutgoingMail, SendError, validate};
use lucia_store::{
    Approval, ApprovalStatus, Decided, EpisodeKind, NewApproval, NewEpisode, Origin, Store,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::engine::{Access, access_allows};

mod kinds;
pub use kinds::{
    ApprovalKind, BrowserActionKind, DiscordActionKind, Executed, GitPushKind, HeldWriteKind,
    KindCtx, LoginCodeKind, MailKind, RoutineReviewKind, SiteLoginKind, SkillReviewKind,
    TaskDoneKind, UnknownKind, WatchReviewKind,
};

pub const KIND_MAIL_SEND: &str = "mail_send";
/// 저장된 로그인으로 다시 로그인(결정 0006).
pub const KIND_SITE_LOGIN: &str = "site_login";
/// 브라우저 동작(결정 0008).
pub const KIND_BROWSER_ACTION: &str = "browser_action";
/// 인젝션 검사에 걸려 보류한 쓰기(기억 항목·후속 조치, 결정 0010). 승인 = 적용, 거절 = 버리기.
pub const KIND_HELD_WRITE: &str = "held_write";
/// 스킬 검토(결정 0018). `notice`: 승인 = 고정, 거절 = 폐기, 만료 = 그대로. `approve`: 승인 = 쓰기 시작,
/// 거절·만료 = 보관.
pub const KIND_SKILL_REVIEW: &str = "skill_review";
/// git push 승인(결정 0020). 승인 = 데몬이 호스트에서 push, 거절·만료 = 하지 않는다.
pub const KIND_GIT_PUSH: &str = "git_push";
/// 감시 승인(결정 0019). 승인 = 감시 시작, 거절·만료 = 지움.
pub const KIND_WATCH_REVIEW: &str = "watch_review";
/// 루틴 승인(결정 0025): 사용자 자료를 읽고 바깥으로 가져가는 스크립트. 승인 = 켜기, 거절·만료 = 지움.
pub const KIND_ROUTINE_REVIEW: &str = "routine_review";
/// 인증 코드 입력(결정 0017). 승인 버튼이 값을 받는다(Discord 입력창). 거절·만료 = 로그인하지 않는다.
pub const KIND_LOGIN_CODE: &str = "login_code";
/// 로그인 정보 입력(결정 0063): 자체 앱의 카드에서 아이디·비밀번호를 받는다.
pub const KIND_LOGIN_INPUT: &str = "login_input";
/// 작업 완료 확인(결정 0014). 승인 = 완료, 거절 = 더 할 일이 있다(답 기다림), 만료 = 만료.
pub const KIND_TASK_DONE: &str = "task_done";
/// 캘린더 쓰기 카드(결정 0033): 참석자가 있는 일정의 만들기·고치기, 지우기.
pub fn calendar_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    let e = escape_markdown;
    let p = &approval.payload;
    let op = p["op"].as_str().unwrap_or("");
    let (head, yes) = match op {
        "create" => ("📅 **일정을 추가할까요?**", "추가"),
        "update" => ("📅 **일정을 고칠까요?**", "고치기"),
        _ => ("🗑️ **일정을 지울까요?**", "지우기"),
    };
    let (headline, tone, buttons, footer) = open_or_closed(
        approval,
        tz,
        state,
        format!("{head} {}", e(&clip_raw(&approval.summary, 120))),
        yes,
        "거절",
        "하지 않습니다",
    );
    let mut fields = Vec::new();
    if op != "delete" {
        let b = &p["body"];
        let pick = |k: &str| {
            b.get(k)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        if let Some(t) = pick("summary") {
            fields.push(CardField {
                name: "제목".into(),
                value: e(&clip_raw(&t, 200)),
                inline: false,
            });
        }
        if b.get("start").is_some() {
            fields.push(CardField {
                name: "시각".into(),
                value: e(&crate::calendar::line(b, tz)),
                inline: true,
            });
        }
        if let Some(l) = pick("location") {
            fields.push(CardField {
                name: "장소".into(),
                value: e(&clip_raw(&l, 200)),
                inline: true,
            });
        }
        if let Some(a) = b.get("attendees").and_then(serde_json::Value::as_array) {
            let list: Vec<String> = a
                .iter()
                .filter_map(|x| x.get("email").and_then(serde_json::Value::as_str))
                .map(str::to_owned)
                .collect();
            fields.push(CardField {
                name: "참석자".into(),
                value: e(&list.join(", ")),
                inline: false,
            });
        }
    }
    if let Some(before) = p.get("before").filter(|b| !b.is_null()) {
        fields.push(CardField {
            name: if op == "delete" {
                "지울 일정"
            } else {
                "지금 일정"
            }
            .into(),
            value: e(&crate::calendar::line(before, tz)),
            inline: false,
        });
    }
    Card {
        conversation,
        headline,
        title: "캘린더".to_owned(),
        fields,
        body: format!(
            "{}\n\n초대 메일은 보내지 않습니다. 누르면 보인 그대로 데몬이 합니다.",
            e(&clip_raw(p["why"].as_str().unwrap_or(""), 600))
        ),
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// 목표 완료 확인(결정 0032). 승인 = 완료(하위 항목 닫기), 거절 = 계속, 만료 = 그대로.
pub const KIND_GOAL_DONE: &str = "goal_done";

/// 작업 결과의 완료 확인 카드. `conversation`이 있으면(작업 스레드) 카드를 거기에 올린다.
pub fn request_task_done(
    store: &Store,
    task: &lucia_store::Task,
    result: &str,
    conversation: Option<&str>,
    run_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<String, String> {
    let id = Uuid::new_v4().simple().to_string();
    store
        .create_approval(
            &NewApproval {
                id: id.clone(),
                kind: KIND_TASK_DONE.into(),
                payload: serde_json::json!({
                    "task": task.id,
                    "title": task.title,
                    "result": result,
                    "conversation": conversation,
                }),
                summary: format!("작업 확인: {}", task.title),
                run_id: run_id.map(str::to_owned),
                expires_at: now + chrono::Duration::days(crate::tasks::CONFIRM_TTL_DAYS),
            },
            now,
        )
        .map_err(|e| format!("저장하지 못했다: {e}"))?;
    Ok(id)
}

/// 보류한 쓰기를 저장한다(3일 동안 기다린다).
pub fn request_held_write(
    store: &Store,
    payload: serde_json::Value,
    what: &str,
    run_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<String, String> {
    let id = Uuid::new_v4().simple().to_string();
    let why = payload["why"].as_str().unwrap_or("").to_owned();
    let summary = format!("보류: {what}({why})");
    store
        .create_approval(
            &NewApproval {
                id: id.clone(),
                kind: KIND_HELD_WRITE.into(),
                payload,
                summary: summary.clone(),
                run_id: run_id.map(str::to_owned),
                expires_at: now + chrono::Duration::days(3),
            },
            now,
        )
        .map_err(|e| format!("저장하지 못했다: {e}"))?;
    let _ = store.add_activity(
        "write_held",
        &crate::mask::global().mask(&summary),
        run_id,
        now,
    );
    Ok(id)
}

/// 승인된 브라우저 동작을 실행한다(`browser::tools::BrowserTools`). 결과 문구 또는 실패 이유.
/// 기다리는 도구 호출이 있으면 결과를 그쪽에도 넘긴다.
pub trait BrowserActionExec: Send + Sync {
    fn run_approved<'a>(
        &'a self,
        id: &'a str,
        payload: &'a serde_json::Value,
    ) -> crate::browser::BoxFut<'a, Result<String, String>>;
    /// 거절·만료로 닫혔다(기다리는 도구 호출에 알린다).
    fn closed(&self, id: &str, why: &str);
    /// 승인된 결과를 기다리던 도구 호출이 받아 갔는가(그러면 실행이 이어 하므로 다시 맡기지 않는다, 결정 0030).
    fn delivered_live(&self, _id: &str) -> bool {
        false
    }
}

/// 승인된 재로그인을 실행한다(`browser::logins::SiteLogins`). 결과 문구 또는 실패 이유.
pub trait SiteLoginExec: Send + Sync {
    fn login_saved<'a>(
        &'a self,
        site: &'a str,
    ) -> crate::browser::BoxFut<'a, Result<crate::browser::logins::SavedLogin, String>>;
    /// 인증 코드를 넣는다(결정 0017). 값은 어디에도 남기지 않는다.
    fn submit_code<'a>(
        &'a self,
        site: &'a str,
        code: &'a str,
    ) -> crate::browser::BoxFut<'a, Result<String, String>>;
    /// 앱 카드에서 받은 아이디·비밀번호로 로그인을 시작한다(결정 0063). 값은 어디에도 남기지 않는다.
    /// 로그인은 떨어진 작업으로 돌고 결과는 알림으로 간다. 받았으면 카드에 보일 문구.
    fn submit_input(
        self: Arc<Self>,
        token: &str,
        username: String,
        password: String,
        save: bool,
    ) -> Result<String, String>;
    /// 입력 카드가 취소·만료로 닫혔다: 아직 쓰지 않은 요청을 닫는다.
    fn cancel_input(&self, token: &str);
}
/// 승인된 Discord 글 올리기·서버 관리를 실행한다(`discord_tools::DiscordExec`, 결정 0024).
pub trait DiscordActionExec: Send + Sync {
    fn run_approved<'a>(
        &'a self,
        payload: &'a serde_json::Value,
    ) -> crate::browser::BoxFut<'a, Result<String, String>>;
}

const ACTION_PREFIX: &str = "approval:";
/// 만료 정리 주기.
const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// 버튼에 싣는 값.
pub fn action(id: &str, approve: bool) -> String {
    format!(
        "{ACTION_PREFIX}{id}:{}",
        if approve { "approve" } else { "deny" }
    )
}

/// "계속 허용" 버튼(결정 0028): 승인이면서 같은 일을 앞으로 묻지 않게 한다. 종류가 [`ApprovalKind::execute_always`]로 다룬다.
pub fn action_always(id: &str) -> String {
    format!("{ACTION_PREFIX}{id}:always")
}

/// (요청 ID, 승인인가, 계속 허용인가).
pub fn parse_action_verb(action: &str) -> Option<(&str, bool, bool)> {
    let rest = action.strip_prefix(ACTION_PREFIX)?;
    let (id, verb) = rest.rsplit_once(':')?;
    let (approve, always) = match verb {
        "approve" => (true, false),
        "always" => (true, true),
        "deny" => (false, false),
        _ => return None,
    };
    (!id.is_empty()).then_some((id, approve, always))
}

#[cfg(test)]
pub fn parse_action(action: &str) -> Option<(&str, bool)> {
    parse_action_verb(action).map(|(id, approve, _)| (id, approve))
}

/// 발송 요청을 검증하고 고정해 저장한다. 요청 ID를 돌려준다(에이전트에게 보여 줄 거부 이유는 `Err`).
pub fn request_mail_send(
    store: &Store,
    mail: &OutgoingMail,
    run_id: Option<&str>,
    ttl: chrono::Duration,
    now: DateTime<Utc>,
) -> Result<String, String> {
    validate(mail)?;
    let id = Uuid::new_v4().simple().to_string();
    let mut payload = serde_json::to_value(mail).map_err(|e| e.to_string())?;
    payload["risks"] = serde_json::json!(crate::risk::assess_mail(store, mail));
    let summary = format!(
        "메일 → {} · {}",
        mail.to.join(", "),
        mail.subject.chars().take(80).collect::<String>()
    );
    store
        .create_approval(
            &NewApproval {
                id: id.clone(),
                kind: KIND_MAIL_SEND.into(),
                payload,
                summary: summary.clone(),
                run_id: run_id.map(str::to_owned),
                expires_at: now + ttl,
            },
            now,
        )
        .map_err(|e| format!("저장하지 못했다: {e}"))?;
    let _ = store.add_activity("approval_requested", &summary, run_id, now);
    Ok(id)
}

fn mail_of(approval: &Approval) -> Result<OutgoingMail, String> {
    serde_json::from_value(approval.payload.clone()).map_err(|e| format!("저장된 내용: {e}"))
}

/// 메신저 마크다운을 끈다: 모델이 쓴 글이 스포일러(`||…||`), 가린 링크(`[글](주소)`), 멘션, 제목 등으로
/// 바뀌어 카드에서 일부가 가려지면 사람이 보는 것과 실제로 보내는 것이 달라진다. 문장부호 앞에 `\`를 붙이면
/// 그대로 보인다.
pub fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for c in text.chars() {
        if "\\*_~`|><#[]()-:@.!+".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 카드 상태: 열림(버튼을 단다) 또는 닫힘(결과 문구와 색).
#[derive(Debug, Clone, Copy)]
pub enum CardState<'a> {
    Open,
    Closed { status: &'a str, tone: CardTone },
}

/// 머리줄에 넣을 만큼만(이스케이프하기 전에 자른다: 자른 자리에서 이스케이프가 갈리지 않게).
pub(crate) fn clip_raw(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// 승인 카드(결정 0005 "승인 카드 모양"): 보낼 내용 전부를 구조대로 보인다.
///
/// - 모델이 쓴 글(주소, 이름, 제목, 본문)은 이스케이프해 칸 값·본문·머리줄에만 넣는다. 제목·칸 이름·꼬리말은
///   고정 문구와 데몬이 만든 값뿐이다(그곳의 이스케이프 동작은 확인하지 않았다).
/// - 닫을 때도 같은 제목·칸·본문으로 다시 만든다(어댑터가 같은 모양으로 고칠 수 있게).
pub fn mail_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    let e = escape_markdown;
    let short_id: String = approval.id.chars().take(8).collect();
    let when = |t: DateTime<Utc>| t.with_timezone(&tz).format("%m-%d %H:%M").to_string();
    let mail = mail_of(approval);
    let (headline, tone, buttons, footer) = match state {
        CardState::Open => (
            match &mail {
                Ok(m) => format!(
                    "📨 **메일을 보내도 될까요?** {}",
                    e(&clip_raw(&m.subject, 80))
                ),
                Err(_) => "📨 **메일 발송 승인 요청**".to_owned(),
            },
            CardTone::Pending,
            vec![
                CardButton {
                    action: action(&approval.id, true),
                    label: "보내기".into(),
                    primary: true,
                    prompt: None,
                    form: None,
                },
                CardButton {
                    action: action(&approval.id, false),
                    label: "거절".into(),
                    primary: false,
                    prompt: None,
                    form: None,
                },
            ],
            format!(
                "요청 {short_id} · {}까지 답이 없으면 보내지 않고 닫습니다",
                when(approval.expires_at)
            ),
        ),
        CardState::Closed { status, tone } => (
            status.to_owned(),
            tone,
            Vec::new(),
            match approval.decided_at {
                Some(t) => format!("요청 {short_id} · {} 처리", when(t)),
                None => format!("요청 {short_id}"),
            },
        ),
    };
    let title = "메일 발송 승인 요청".to_owned();
    let Ok(mail) = mail else {
        return Card {
            conversation,
            headline,
            title,
            fields: Vec::new(),
            body: "저장된 내용을 읽을 수 없습니다.".into(),
            footer: Some(footer),
            tone,
            buttons,
        };
    };
    let field = |name: &str, value: String, inline: bool| CardField {
        name: name.to_owned(),
        value,
        inline,
    };
    let list = |addrs: &[String]| addrs.iter().map(|a| e(a)).collect::<Vec<_>>().join("\n");
    let mut fields: Vec<CardField> = risk_field(approval).into_iter().collect();
    fields.extend([
        field(
            "보내는 주소",
            match &mail.from_name {
                Some(name) => format!("{} <{}>", e(name), e(&mail.from)),
                None => e(&mail.from),
            },
            true,
        ),
        field("받는 사람", list(&mail.to), true),
    ]);
    if !mail.cc.is_empty() {
        fields.push(field("참조", list(&mail.cc), true));
    }
    if let Some(r) = &mail.reply_to {
        fields.push(field("답장 받을 주소", e(r), true));
    }
    if mail.in_reply_to.is_some() {
        fields.push(field("종류", "받은 메일에 대한 답장".into(), true));
    }
    fields.push(field("제목", e(&mail.subject), false));
    Card {
        conversation,
        headline,
        title,
        fields,
        body: e(&mail.body),
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// 위험 줄 칸(결정 0028): 요청을 만들 때 데몬이 찾아 고정해 둔 것. 없으면 칸도 없다.
fn risk_field(approval: &Approval) -> Option<CardField> {
    crate::risk::line(&crate::risk::of_payload(&approval.payload)).map(|value| CardField {
        name: "확인할 점".to_owned(),
        value,
        inline: false,
    })
}

/// 재로그인 승인 카드(결정 0006). 값은 모두 데몬이 정한 것(사이트 이름, 저장된 아이디, 고정 이유)이고,
/// 그래도 칸 값은 이스케이프한다. 비밀번호는 카드에 없다.
pub fn login_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    let e = escape_markdown;
    let p = &approval.payload;
    let text = |k: &str| p[k].as_str().unwrap_or("").to_owned();
    let site_name = match text("site_name") {
        n if n.is_empty() => "사이트".to_owned(),
        n => n,
    };
    let short_id: String = approval.id.chars().take(8).collect();
    let when = |t: DateTime<Utc>| t.with_timezone(&tz).format("%m-%d %H:%M").to_string();
    let (headline, tone, buttons, footer) = match state {
        CardState::Open => (
            format!(
                "🔐 **{}에 다시 로그인할까요?** 저장된 로그인을 씁니다.",
                e(&site_name)
            ),
            CardTone::Pending,
            vec![
                CardButton {
                    action: action(&approval.id, true),
                    label: "로그인".into(),
                    primary: true,
                    prompt: None,
                    form: None,
                },
                CardButton {
                    action: action(&approval.id, false),
                    label: "거절".into(),
                    primary: false,
                    prompt: None,
                    form: None,
                },
            ],
            format!(
                "요청 {short_id} · {}까지 답이 없으면 로그인하지 않고 닫습니다",
                when(approval.expires_at)
            ),
        ),
        CardState::Closed { status, tone } => (
            status.to_owned(),
            tone,
            Vec::new(),
            match approval.decided_at {
                Some(t) => format!("요청 {short_id} · {} 처리", when(t)),
                None => format!("요청 {short_id}"),
            },
        ),
    };
    let field = |name: &str, value: String, inline: bool| CardField {
        name: name.to_owned(),
        value,
        inline,
    };
    let mut fields = vec![
        field(
            "사이트",
            match text("host") {
                h if h.is_empty() => e(&site_name),
                h => format!("{} · {}", e(&site_name), e(&h)),
            },
            true,
        ),
        field("아이디", e(&text("username")), true),
    ];
    let reason = text("reason");
    if !reason.is_empty() {
        fields.push(field("이유", e(&reason), false));
    }
    Card {
        conversation,
        headline,
        title: "사이트 로그인 승인 요청".to_owned(),
        fields,
        body: "승인하면 데몬이 저장된 아이디와 비밀번호로 한 번 로그인합니다. 비밀번호는 Lucia(모델)에게 가지 않습니다.".to_owned(),
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// 브라우저 동작 승인 카드(결정 0008): 사이트·페이지·누를 요소·보낼 폼 값을 그대로 보인다.
/// 값은 페이지에서 읽은 글이라 모두 이스케이프한다.
pub fn browser_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    use crate::browser::tools::{act_ko, kind_ko};
    use crate::browser::{Act, Expect};
    let e = escape_markdown;
    let p = &approval.payload;
    let act: Option<Act> = serde_json::from_value(p["act"].clone()).ok();
    let expect: Option<Expect> = serde_json::from_value(p["expect"].clone()).ok();
    let host = p["host"].as_str().unwrap_or("").to_owned();
    let short_id: String = approval.id.chars().take(8).collect();
    let when = |t: DateTime<Utc>| t.with_timezone(&tz).format("%m-%d %H:%M:%S").to_string();
    let label = expect
        .as_ref()
        .map(|x| clip_raw(&x.element.label, 60))
        .unwrap_or_default();
    let (headline, tone, buttons, footer) = match state {
        CardState::Open => (
            format!(
                "🖱️ **브라우저에서 이 동작을 할까요?** {} “{}”",
                e(&host),
                e(&label)
            ),
            CardTone::Pending,
            vec![
                CardButton {
                    action: action(&approval.id, true),
                    label: "실행".into(),
                    primary: true,
                    prompt: None,
                    form: None,
                },
                CardButton {
                    action: action(&approval.id, false),
                    label: "거절".into(),
                    primary: false,
                    prompt: None,
                    form: None,
                },
            ],
            format!(
                "요청 {short_id} · {}까지 답이 없으면 실행하지 않고 닫습니다",
                when(approval.expires_at)
            ),
        ),
        CardState::Closed { status, tone } => (
            status.to_owned(),
            tone,
            Vec::new(),
            match approval.decided_at {
                Some(t) => format!("요청 {short_id} · {} 처리", when(t)),
                None => format!("요청 {short_id}"),
            },
        ),
    };
    let field = |name: &str, value: String, inline: bool| CardField {
        name: name.to_owned(),
        value,
        inline,
    };
    let title = "브라우저 동작 승인 요청".to_owned();
    let (Some(act), Some(expect)) = (act, expect) else {
        return Card {
            conversation,
            headline,
            title,
            fields: Vec::new(),
            body: "저장된 내용을 읽을 수 없습니다.".into(),
            footer: Some(footer),
            tone,
            buttons,
        };
    };
    let el = &expect.element;
    let mut fields: Vec<CardField> = risk_field(approval).into_iter().collect();
    fields.extend([
        field("사이트", e(&host), true),
        field("동작", act_ko(&act), true),
        field(
            "대상",
            format!("{} “{}”", kind_ko(&el.kind), e(&clip_raw(&el.label, 100))),
            false,
        ),
        field("페이지", e(&clip_raw(&expect.url, 300)), false),
    ]);
    if let Some(frame) = &el.frame {
        fields.push(field("iframe", e(&clip_raw(frame, 300)), false));
    }
    if el.kind == "unknown" {
        fields.push(field(
            "확인",
            "데몬이 그 자리의 요소를 읽지 못했습니다. 관리 페이지의 화면 넘겨받기로 직접 볼 수 있습니다.".into(),
            false,
        ));
    }
    if let Some(href) = &el.href {
        fields.push(field("링크 주소", e(&clip_raw(href, 300)), false));
    }
    if let Some(form) = &el.form {
        fields.push(field(
            "보낼 곳",
            format!(
                "{} {}",
                form.method.to_uppercase(),
                e(&clip_raw(&form.action, 300))
            ),
            false,
        ));
    }
    if let Act::Type { text, .. } | Act::TypeText { text, .. } = &act {
        fields.push(field("넣을 글", e(&clip_raw(text, 900)), false));
    }
    let reason = p["reason"].as_str().unwrap_or("");
    if !reason.is_empty() {
        fields.push(field("승인이 필요한 까닭", e(reason), false));
    }
    let body = match &el.form {
        Some(form) if !form.fields.is_empty() || form.hidden > 0 => {
            let mut lines: Vec<String> = form
                .fields
                .iter()
                .map(|(k, v)| format!("• {}: {}", e(&clip_raw(k, 60)), e(&clip_raw(v, 300))))
                .collect();
            if form.hidden > 0 {
                lines.push(format!("• (숨은 칸 {}개는 사이트가 정한 값)", form.hidden));
            }
            format!("폼에 지금 든 값:\n{}", lines.join("\n"))
        }
        _ => "승인하면 데몬이 같은 페이지의 같은 요소인지 다시 확인한 뒤 한 번 실행합니다."
            .to_owned(),
    };
    Card {
        conversation,
        headline,
        title,
        fields,
        body,
        footer: Some(footer),
        tone,
        buttons,
    }
}

pub(crate) fn open_or_closed(
    approval: &Approval,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
    open_headline: String,
    yes: &str,
    no: &str,
    unanswered: &str,
) -> (String, CardTone, Vec<CardButton>, String) {
    let short_id: String = approval.id.chars().take(8).collect();
    let when = |t: DateTime<Utc>| t.with_timezone(&tz).format("%m-%d %H:%M").to_string();
    match state {
        CardState::Open => (
            open_headline,
            CardTone::Pending,
            vec![
                CardButton {
                    action: action(&approval.id, true),
                    label: yes.into(),
                    primary: true,
                    prompt: None,
                    form: None,
                },
                CardButton {
                    action: action(&approval.id, false),
                    label: no.into(),
                    primary: false,
                    prompt: None,
                    form: None,
                },
            ],
            format!(
                "요청 {short_id} · {}까지 답이 없으면 {unanswered}",
                when(approval.expires_at)
            ),
        ),
        CardState::Closed { status, tone } => (
            status.to_owned(),
            tone,
            Vec::new(),
            match approval.decided_at {
                Some(t) => format!("요청 {short_id} · {} 처리", when(t)),
                None => format!("요청 {short_id}"),
            },
        ),
    }
}

/// 보류한 쓰기 카드(결정 0010): 무엇이 왜 걸렸는지와 내용 전부.
pub fn held_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    let e = escape_markdown;
    let p = &approval.payload;
    let t = |k: &str| p[k].as_str().unwrap_or("").to_owned();
    let memory = t("type") == "memory";
    let (headline, tone, buttons, footer) = open_or_closed(
        approval,
        tz,
        state,
        format!(
            "⚠️ **지시문처럼 보이는 글이라 반영을 보류했습니다** ({})",
            e(&t("why"))
        ),
        "적용",
        "버리기",
        "버립니다",
    );
    let field = |name: &str, value: String| CardField {
        name: name.to_owned(),
        value,
        inline: true,
    };
    let (fields, body) = if memory {
        (
            vec![
                field("종류", "기억 항목".into()),
                field("항목", format!("\\[\\[{}\\]\\]", e(&t("name")))),
                field("제목", e(&clip_raw(&t("title"), 80))),
                field("까닭(Lucia)", e(&clip_raw(&t("reason"), 200))),
            ],
            format!(
                "요약: {}\n\n{}",
                e(&clip_raw(&t("summary"), 300)),
                e(&clip_raw(&t("body"), 3000))
            ),
        )
    } else {
        (
            vec![
                field("종류", "후속 조치".into()),
                field("시각", e(&t("when"))),
            ],
            format!(
                "{}\n\n{}",
                e(&clip_raw(&t("task"), 300)),
                e(&clip_raw(&t("detail"), 2000))
            ),
        )
    };
    Card {
        conversation,
        headline,
        title: "보류한 쓰기".to_owned(),
        fields,
        body: format!(
            "{body}\n\n외부 글(메일·웹)에서 온 지시가 섞였을 수 있습니다. 내용이 맞으면 [적용], 아니면 [버리기]."
        ),
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// 메일을 보내는 쪽(실제는 Gmail, 시험은 가짜).
pub trait Mailer: Send + Sync + 'static {
    fn send(&self, mail: &OutgoingMail) -> impl Future<Output = Result<String, SendError>> + Send;
}

impl<T: TokenProvider> Mailer for GoogleAssist<T> {
    async fn send(&self, mail: &OutgoingMail) -> Result<String, SendError> {
        self.send_mail(mail).await
    }
}

/// 인증 코드 카드(결정 0017): [코드 입력]은 값을 받는 버튼이다. 버튼을 못 쓰면 링크로.
pub fn code_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    let e = escape_markdown;
    let p = &approval.payload;
    let t = |k: &str| p[k].as_str().unwrap_or("").to_owned();
    let (headline, tone, mut buttons, footer) = open_or_closed(
        approval,
        tz,
        state,
        format!(
            "🔢 **인증 코드가 필요합니다** {}",
            e(&clip_raw(&t("site_name"), 60))
        ),
        "코드 입력",
        "취소",
        "로그인하지 않습니다",
    );
    if let Some(first) = buttons.first_mut() {
        first.prompt = Some(lucia_core::ButtonPrompt {
            title: "인증 코드 입력".into(),
            label: "인증 코드".into(),
            placeholder: Some("예: 123456".into()),
            min_len: 1,
            max_len: 12,
        });
    }
    Card {
        conversation,
        headline,
        title: "인증 코드 입력".to_owned(),
        fields: vec![
            CardField {
                name: "사이트".to_owned(),
                value: e(&t("origin")),
                inline: true,
            },
            CardField {
                name: "칸".to_owned(),
                value: e(&clip_raw(&t("hint"), 60)),
                inline: true,
            },
        ],
        body: format!(
            "사이트 주소가 맞는지 확인하고 [코드 입력]을 눌러 넣어 주세요. 코드는 Lucia(모델)에게 가지 않고 데몬이 브라우저에 바로 넣습니다.\n버튼이 안 되면: {}",
            t("link")
        ),
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// 로그인 입력 카드(결정 0063). [로그인 정보 입력]은 여러 칸을 받는 버튼이라 능력 `forms`가 있는 채널(자체 앱)만
/// 그린다. 그리지 못하는 곳(앱이 끊겨 Discord로 돌린 카드)을 위해 본문에 입력 링크를 함께 둔다.
pub fn login_input_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    use lucia_core::{ButtonForm, FormField, FormFieldKind};
    let e = escape_markdown;
    let p = &approval.payload;
    let t = |k: &str| p[k].as_str().unwrap_or("").to_owned();
    let (headline, tone, mut buttons, footer) = open_or_closed(
        approval,
        tz,
        state,
        format!(
            "🔐 **로그인이 필요합니다** {}",
            e(&clip_raw(&t("site_name"), 60))
        ),
        "로그인 정보 입력",
        "취소",
        "로그인하지 않습니다",
    );
    if let Some(first) = buttons.first_mut() {
        let field = |key: &str, label: &str, kind, min_len, max_len| FormField {
            key: key.into(),
            label: label.into(),
            kind,
            min_len,
            max_len,
        };
        first.form = Some(ButtonForm {
            title: "로그인 정보 입력".into(),
            fields: vec![
                field("username", "아이디", FormFieldKind::Text, 1, 100),
                field("password", "비밀번호", FormFieldKind::Secret, 1, 200),
                field(
                    "save",
                    "이 로그인 저장(다음부터는 승인만)",
                    FormFieldKind::Check,
                    0,
                    0,
                ),
            ],
        });
    }
    let mut fields = vec![CardField {
        name: "사이트".to_owned(),
        value: e(&t("origin")),
        inline: true,
    }];
    let reason = t("reason");
    if !reason.trim().is_empty() {
        fields.push(CardField {
            name: "까닭".to_owned(),
            value: e(&clip_raw(&reason, 200)),
            inline: false,
        });
    }
    let error = t("error");
    if !error.trim().is_empty() {
        fields.push(CardField {
            name: "앞선 시도".to_owned(),
            value: format!("⚠️ {}", e(&clip_raw(&error, 200))),
            inline: false,
        });
    }
    Card {
        conversation,
        headline,
        title: "로그인 정보 입력".to_owned(),
        fields,
        body: format!(
            "사이트 주소가 맞는지 확인하고 [로그인 정보 입력]을 눌러 아이디와 비밀번호를 넣어 주세요(실패하면 {}번까지). 넣은 값은 Lucia(모델)에게 가지 않고 데몬이 브라우저에 바로 넣습니다.\n버튼이 없거나 안 되면: {}",
            p["attempts"].as_u64().unwrap_or(1),
            t("link")
        ),
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// 스킬 검토 카드(결정 0018).
pub fn skill_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    let e = escape_markdown;
    let p = &approval.payload;
    let t = |k: &str| p[k].as_str().unwrap_or("").to_owned();
    let notice = t("mode") == "notice";
    let (headline, tone, buttons, footer) = open_or_closed(
        approval,
        tz,
        state,
        if notice {
            format!(
                "🧩 **새 스킬을 저장했습니다** {}",
                e(&clip_raw(&t("title"), 60))
            )
        } else {
            format!(
                "🧩 **스킬 승인이 필요합니다** {}",
                e(&clip_raw(&t("title"), 60))
            )
        },
        if notice { "고정" } else { "승인" },
        "폐기",
        if notice {
            "그대로 둡니다(시험 중)"
        } else {
            "쓰지 않습니다"
        },
    );
    Card {
        conversation,
        headline,
        title: if notice {
            "새 스킬"
        } else {
            "스킬 승인"
        }
        .to_owned(),
        fields: vec![
            CardField {
                name: "이름".to_owned(),
                value: e(&t("slug")),
                inline: true,
            },
            CardField {
                name: "쓰는 것".to_owned(),
                value: e(&clip_raw(&t("footprint"), 200)),
                inline: true,
            },
            CardField {
                name: "언제".to_owned(),
                value: e(&clip_raw(&t("description"), 200)),
                inline: false,
            },
        ],
        body: format!(
            "{}\n\n{}",
            e(&clip_raw(&t("body"), 1500)),
            if notice {
                "읽기만 하는 절차라 시험 중으로 저장했습니다. [고정]은 자동 수정·보관에서 빼고, [폐기]는 지웁니다."
            } else {
                "사이트를 열거나 외부에 쓰는 절차입니다. [승인]해야 Lucia가 씁니다. 스킬은 권한을 넓히지 않고, 외부 쓰기는 그때마다 승인 카드를 거칩니다."
            }
        ),
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// 감시 승인 카드(결정 0019): 정확한 주소(자르지 않음), 보는 구간, 간격을 보인다.
pub fn watch_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    let e = escape_markdown;
    let p = &approval.payload;
    let t = |k: &str| p[k].as_str().unwrap_or("").to_owned();
    let (headline, tone, buttons, footer) = open_or_closed(
        approval,
        tz,
        state,
        format!(
            "👀 **페이지 감시를 시작할까요?** {}",
            e(&clip_raw(&t("what"), 60))
        ),
        "감시 시작",
        "하지 않기",
        "감시하지 않습니다",
    );
    let marker = |k: &str| {
        let v = t(k);
        if v.is_empty() {
            "(없음)".to_owned()
        } else {
            e(&v)
        }
    };
    let interval = CardField {
        name: "간격".to_owned(),
        value: format!("{}분", p["interval_minutes"].as_i64().unwrap_or(0)),
        inline: true,
    };
    let field = |name: &str, value: String, inline: bool| CardField {
        name: name.to_owned(),
        value,
        inline,
    };
    let (fields, body) = if t("mode") == lucia_store::watch_mode::ITEMS {
        let words = |k: &str| {
            let v = t(k).replace('\n', ", ");
            if v.is_empty() {
                "(없음)".to_owned()
            } else {
                e(&v)
            }
        };
        let samples: Vec<String> = p["samples"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| format!("• {}", e(&clip_raw(s, 80))))
                    .collect()
            })
            .unwrap_or_default();
        let notify = match t("notify").as_str() {
            "classify" => "작은 모델이 골라 알림",
            "digest" => "묶음 소식만",
            _ => "바로 알림(모델 없음)",
        };
        (
            vec![
                field("무엇", e(&t("what")), false),
                field("주소", e(&t("url")), false),
                field("글 링크 모양", e(&t("item_pattern")), false),
                field("넣을 말", words("include"), true),
                field("뺄 말", words("exclude"), true),
                field("알림", notify.to_owned(), true),
                interval,
                field(
                    "읽는 방식",
                    {
                        let via = crate::watch::Via::parse(&t("fetcher"));
                        let why = t("chosen_why");
                        if why.is_empty() {
                            via.label().to_owned()
                        } else {
                            format!("{} ({})", via.label(), e(&clip_raw(&why, 200)))
                        }
                    },
                    false,
                ),
                if p["tested"] == false {
                    field(
                        "미리 보기",
                        "사람 없는 실행이 제안해 미리 가져와 보지 않았습니다(위임하지 않은 사이트). 승인하면 첫 확인에서 글을 찾습니다.".to_owned(),
                        false,
                    )
                } else {
                    field(
                        &format!(
                            "지금 목록에서 찾은 글 {}개(키워드에 맞는 것 {}개), 앞 글",
                            p["found"].as_u64().unwrap_or(0),
                            p["matching"].as_u64().unwrap_or(0)
                        ),
                        if samples.is_empty() {
                            "(없음)".to_owned()
                        } else {
                            samples.join("\n")
                        },
                        false,
                    )
                },
            ],
            {
                let via = crate::watch::Via::parse(&t("fetcher"));
                let mut body = String::from(match via {
                    crate::watch::Via::Plain => {
                        "승인하면 데몬이 모델 없이 이 주소를 정해진 간격으로 가져와(로그인 없이) 처음 보는 글을 찾고, 키워드에 맞는 새 글을 알립니다."
                    }
                    crate::watch::Via::Session => {
                        "승인하면 데몬이 로그인된 브라우저의 세션(쿠키)을 빌려 이 주소를 정해진 간격으로 읽고(모델 없이), 키워드에 맞는 새 글을 알립니다. 계정으로 자주 읽으면 사이트 규칙에 따라 계정이 제한될 수 있습니다."
                    }
                    crate::watch::Via::Browser => {
                        "승인하면 데몬이 로그인된 브라우저의 감시 탭에서 이 주소를 정해진 간격으로 읽고(모델 없이), 키워드에 맞는 새 글을 알립니다. 이 감시가 켜져 있는 동안 브라우저(메모리 약 2 GB)를 계속 켜 둡니다. 계정으로 자주 읽으면 사이트 규칙에 따라 계정이 제한될 수 있습니다."
                    }
                });
                if via != crate::watch::Via::Plain && p["granted"] == false {
                    body.push_str(" 승인하면 이 사이트의 읽기 위임도 함께 만듭니다(관리 페이지 \"사이트 위임\"에서 거둘 수 있고, 거두면 감시가 멈춥니다).");
                }
                body.push_str(" 첫 확인은 기준선이라 지금 있는 글은 알리지 않습니다. 앞 글이 그 게시판의 글이 맞는지, 주소가 맞는지 확인하세요(주소 안의 글도 그 사이트로 갑니다). 관리 페이지에서 언제든 멈추거나 고칠 수 있습니다.");
                body
            },
        )
    } else {
        (
            vec![
                field("무엇", e(&t("what")), false),
                field("주소", e(&t("url")), false),
                field("시작 표지", marker("start_text"), true),
                field("끝 표지", marker("end_text"), true),
                interval,
            ],
            "승인하면 데몬이 모델 없이 이 주소를 정해진 간격으로 가져와(로그인 없이), 표지 사이 글이 바뀌었을 때만 알리고 브리핑에 넣습니다. 주소가 맞는지 확인하세요(주소 안의 글도 그 사이트로 갑니다). 관리 페이지에서 언제든 멈출 수 있습니다.".to_owned(),
        )
    };
    Card {
        conversation,
        headline,
        title: "페이지 감시".to_owned(),
        fields,
        body,
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// 루틴 승인 카드(결정 0025): 이름, 언제, 할 일, 스크립트가 쓰는 도구와 코드(앞부분).
pub fn routine_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    let e = escape_markdown;
    let p = &approval.payload;
    let t = |k: &str| p[k].as_str().unwrap_or("").to_owned();
    let (headline, tone, buttons, footer) = open_or_closed(
        approval,
        tz,
        state,
        format!(
            "🔁 **이 반복 루틴을 켤까요?** {}",
            e(&clip_raw(&t("name"), 60))
        ),
        "켜기",
        "지우기",
        "켜지 않습니다",
    );
    let code = clip_raw(&t("script"), 1400).replace("```", "'''");
    Card {
        conversation,
        headline,
        title: "반복 루틴".to_owned(),
        fields: vec![
            CardField {
                name: "이름".to_owned(),
                value: e(&clip_raw(&t("name"), 100)),
                inline: true,
            },
            CardField {
                name: "언제".to_owned(),
                value: e(&clip_raw(&t("trigger"), 200)),
                inline: true,
            },
            CardField {
                name: "쓰는 도구".to_owned(),
                value: e(&clip_raw(&t("tools"), 400)),
                inline: false,
            },
            CardField {
                name: "Lucia를 깨울 때 할 일".to_owned(),
                value: e(&clip_raw(&t("task"), 900)),
                inline: false,
            },
        ],
        body: format!(
            "이 루틴의 스크립트는 사용자 자료(기억·메일·일정·Discord)를 읽으면서 공개 페이지도 가져옵니다(fetch). 가져오는 주소에 자료가 실려 나가지 않는지 코드를 확인하세요. 켜면 사람 없이 정해진 때마다 돕니다. 관리 페이지에서 언제든 멈출 수 있습니다.\n```python\n{code}\n```"
        ),
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// git push 카드(결정 0020): 보낼 곳, 가지, 커밋, 바뀐 파일 요약. 카드에 보인 커밋만 보낸다.
pub fn push_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    let e = escape_markdown;
    let p = &approval.payload;
    let t = |k: &str| p[k].as_str().unwrap_or("").to_owned();
    let (headline, tone, buttons, footer) = open_or_closed(
        approval,
        tz,
        state,
        format!(
            "⬆️ **git push 승인이 필요합니다** {}",
            e(&clip_raw(&t("title"), 60))
        ),
        "push",
        "하지 않기",
        "push하지 않습니다",
    );
    let commits: Vec<String> = p["commits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c.as_str().map(|c| format!("• {}", e(&clip_raw(c, 120)))))
        .collect();
    Card {
        conversation,
        headline,
        title: "git push".to_owned(),
        fields: vec![
            CardField {
                name: "보낼 곳".to_owned(),
                value: e(&t("origin")),
                inline: false,
            },
            CardField {
                name: "가지".to_owned(),
                value: e(&t("branch")),
                inline: true,
            },
            CardField {
                name: "작업".to_owned(),
                value: e(&t("task")),
                inline: true,
            },
        ],
        body: format!(
            "{}\n\n커밋 {}개:\n{}\n\n바뀐 파일:\n{}\n\n이 가지로만, 강제 없이 보냅니다. 카드에 보인 커밋까지만 보냅니다.",
            e(&clip_raw(&t("summary"), 400)),
            commits.len(),
            commits.join("\n"),
            e(&clip_raw(&t("stat"), 1200))
        ),
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// Discord 글 올리기·서버 관리 카드(결정 0024). 서버·대상 이름은 데몬이 요청 때 알아내 고정한 값이다
/// (모델이 쓴 글이 아니다. 그래도 사람이 정한 이름이라 이스케이프한다).
pub fn discord_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    let e = escape_markdown;
    let p = &approval.payload;
    let shown = |k: &str| p["shown"][k].as_str().unwrap_or("").to_owned();
    let post = p["op"].as_str() == Some("post");
    let manage: Option<lucia_channel::discord::server::Manage> =
        serde_json::from_value(p["manage"].clone()).ok();
    let label = if post {
        "글 올리기"
    } else {
        manage.as_ref().map(|m| m.label()).unwrap_or("서버 관리")
    };
    let (headline, tone, buttons, footer) = open_or_closed(
        approval,
        tz,
        state,
        format!(
            "🛡️ **Discord {label} 승인이 필요합니다** {}",
            e(&clip_raw(&shown("server"), 60))
        ),
        if post { "올리기" } else { "실행" },
        "하지 않기",
        "하지 않습니다",
    );
    let mut fields: Vec<CardField> = risk_field(approval).into_iter().collect();
    fields.push(CardField {
        name: "서버".to_owned(),
        value: e(&clip_raw(&shown("server"), 200)),
        inline: true,
    });
    for (key, name) in [("channel", "채널"), ("user", "대상"), ("role", "역할")] {
        let v = shown(key);
        if !v.is_empty() {
            fields.push(CardField {
                name: name.to_owned(),
                value: e(&clip_raw(&v, 200)),
                inline: true,
            });
        }
    }
    let mut body = Vec::new();
    if post {
        body.push(format!("올릴 글:\n{}", e(p["text"].as_str().unwrap_or(""))));
        if p["reply_to"].as_str().is_some() {
            body.push("(그 채널의 메시지에 답장으로 올립니다)".to_owned());
        }
        body.push("멘션은 알림 없이 글로만 보입니다.".to_owned());
    } else if let Some(m) = &manage {
        use lucia_channel::discord::server::Manage as M;
        match m {
            M::Kick { reason, .. } | M::Ban { reason, .. } | M::Timeout { reason, .. } => {
                if let Some(r) = reason {
                    body.push(format!("이유(감사 로그): {}", e(&clip_raw(r, 400))));
                }
            }
            _ => {}
        }
        match m {
            M::Ban {
                delete_message_seconds,
                ..
            } if *delete_message_seconds > 0 => body.push(format!(
                "최근 {}시간의 메시지도 지웁니다.",
                delete_message_seconds / 3600
            )),
            M::Timeout { minutes, .. } => body.push(format!("기간: {minutes}분")),
            M::Nick { nick, .. } => body.push(match nick {
                Some(n) => format!("새 별명: {}", e(n)),
                None => "별명을 지웁니다.".to_owned(),
            }),
            M::ChannelCreate {
                name, kind, topic, ..
            } => {
                body.push(format!("이름: {} ({kind:?})", e(name)));
                if let Some(t) = topic {
                    body.push(format!("주제: {}", e(&clip_raw(t, 400))));
                }
            }
            M::ChannelRename { name, .. } => body.push(format!("새 이름: {}", e(name))),
            M::ChannelTopic { topic, .. } => {
                body.push(format!("새 주제: {}", e(&clip_raw(topic, 600))))
            }
            M::Slowmode { seconds, .. } => body.push(if *seconds == 0 {
                "느린 모드를 끕니다.".to_owned()
            } else {
                format!("느린 모드: {seconds}초")
            }),
            _ => {}
        }
        let msg = shown("message");
        if !msg.is_empty() {
            body.push(format!("메시지:\n{}", e(&msg)));
        }
        if m.destructive() {
            body.push("⚠️ 되돌릴 수 없습니다.".to_owned());
        }
        body.push("실행할 때 봇의 관리자 권한을 다시 확인합니다.".to_owned());
    }
    Card {
        conversation,
        headline,
        title: format!("Discord {label}"),
        fields,
        body: body.join("\n\n"),
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// 작업 완료 확인 카드(결정 0014).
pub fn task_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    let e = escape_markdown;
    let p = &approval.payload;
    let t = |k: &str| p[k].as_str().unwrap_or("").to_owned();
    let (headline, tone, buttons, footer) = open_or_closed(
        approval,
        tz,
        state,
        format!(
            "📋 **작업 결과를 확인해 주세요** {}",
            e(&clip_raw(&t("title"), 80))
        ),
        "완료",
        "아직",
        "그대로 둡니다",
    );
    Card {
        conversation,
        headline,
        title: "작업 완료 확인".to_owned(),
        fields: vec![
            CardField {
                name: "작업".to_owned(),
                value: e(&clip_raw(&t("title"), 200)),
                inline: true,
            },
            CardField {
                name: "ID".to_owned(),
                value: e(&t("task")),
                inline: true,
            },
        ],
        body: format!(
            "{}\n\n[완료]를 누르면 끝난 작업으로 둡니다. [아직]이면 더 할 일을 말해 주세요.",
            e(&clip_raw(&t("result"), 1500))
        ),
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// 목표 완료 확인 카드(결정 0032).
pub fn goal_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> Card {
    let e = escape_markdown;
    let p = &approval.payload;
    let t = |k: &str| p[k].as_str().unwrap_or("").to_owned();
    let (headline, tone, buttons, footer) = open_or_closed(
        approval,
        tz,
        state,
        format!(
            "🎯 **목표를 끝내도 될까요?** {}",
            e(&clip_raw(&t("title"), 80))
        ),
        "완료",
        "아직",
        "그대로 둡니다",
    );
    Card {
        conversation,
        headline,
        title: "목표 완료 확인".to_owned(),
        fields: vec![
            CardField {
                name: "끝났다고 볼 조건".to_owned(),
                value: e(&clip_raw(&t("success"), 300)),
                inline: false,
            },
            CardField {
                name: "ID".to_owned(),
                value: e(&t("goal")),
                inline: true,
            },
        ],
        body: format!(
            "{}\n\n[완료]를 누르면 목표를 닫고 하위 후속 조치·루틴·감시를 끝냅니다. [아직]이면 계속 점검합니다.",
            e(&clip_raw(&t("why"), 1500))
        ),
        footer: Some(footer),
        tone,
        buttons,
    }
}

/// 승인 데스크. 종류별 처리는 등록된 [`ApprovalKind`]에 맡긴다(결정 0013).
pub struct ApprovalDesk<C> {
    pub store: Arc<Mutex<Store>>,
    pub channel: Arc<C>,
    pub access: Access,
    pub tz: chrono_tz::Tz,
    /// 카드를 올릴 대화(소유자 DM). 매번 저장소 설정을 먼저 본다.
    pub fallback_conversation: Option<ConversationId>,
    kinds: HashMap<&'static str, Arc<dyn ApprovalKind>>,
    ctx: KindCtx,
    /// 주 모델(결정 0055). `/모델` 명령이 보고 바꾼다.
    models: Option<crate::models::MainModel>,
}

impl<C: ChannelAdapter> ApprovalDesk<C> {
    /// 주 모델을 붙인다(결정 0055).
    pub fn with_models(mut self, models: crate::models::MainModel) -> Self {
        self.models = Some(models);
        self
    }

    pub fn new(
        store: Arc<Mutex<Store>>,
        channel: Arc<C>,
        access: Access,
        tz: chrono_tz::Tz,
        fallback_conversation: Option<ConversationId>,
    ) -> Self {
        Self {
            ctx: KindCtx {
                store: store.clone(),
                tz,
            },
            store,
            channel,
            access,
            tz,
            fallback_conversation,
            kinds: HashMap::new(),
            models: None,
        }
    }

    /// 승인 종류를 등록한다. 같은 종류를 다시 등록하면 바꾼다.
    pub fn register(mut self, kind: impl ApprovalKind + 'static) -> Self {
        self.set_kind(kind);
        self
    }

    pub fn set_kind(&mut self, kind: impl ApprovalKind + 'static) {
        self.kinds.insert(kind.kind(), Arc::new(kind));
    }

    /// Discord 글 올리기·서버 관리(결정 0024). Discord가 없는 실행(개발용 ask 등)에서는 실행하지 않는다.
    pub fn with_discord(self, exec: Option<Arc<dyn DiscordActionExec>>) -> Self {
        self.register(DiscordActionKind { exec })
    }

    /// 주간 보고의 제안 카드(결정 0035). 대화 입구가 없는 실행(개발용)에서는 넘기지 않는다.
    pub fn with_suggestions(self, exec: Option<Arc<dyn crate::reports::SuggestionExec>>) -> Self {
        self.register(crate::reports::SuggestionKind { exec })
    }

    /// 캘린더 쓰기 카드(결정 0033). Google이 없는 실행에서는 실행하지 않는다.
    pub fn with_calendar(self, exec: Option<Arc<dyn crate::calendar::CalendarExec>>) -> Self {
        self.register(crate::approvals::kinds::CalendarKind { exec })
    }

    /// 개발 작업의 push 승인(결정 0020). 개발 작업을 켤 때만 등록한다.
    pub fn with_dev(self, dev: Option<std::sync::Arc<crate::dev::DevSandboxes>>) -> Self {
        self.register(GitPushKind { dev })
    }

    /// 데몬의 기본 종류: 메일 발송, 사이트 로그인, 브라우저 동작, 보류한 쓰기, 기억 되돌리기.
    pub fn with_builtin_kinds<M: Mailer>(
        self,
        mailer: M,
        site_login: Option<Arc<dyn SiteLoginExec>>,
        browser_action: Option<Arc<dyn BrowserActionExec>>,
    ) -> Self {
        self.register(MailKind { mailer })
            .register(LoginCodeKind {
                exec: site_login.clone(),
            })
            .register(crate::approvals::kinds::LoginInputKind {
                exec: site_login.clone(),
            })
            .register(SiteLoginKind { exec: site_login })
            .register(BrowserActionKind {
                exec: browser_action,
            })
            .register(HeldWriteKind)
            .register(TaskDoneKind)
            .register(crate::approvals::kinds::GoalDoneKind)
            .register(crate::attention::FeedbackKind)
            .register(crate::attention::NudgeKind)
            .register(SkillReviewKind)
            .register(WatchReviewKind)
            .register(crate::watch::WatchSwitchKind)
            .register(RoutineReviewKind)
            .register(crate::delegation::SiteGrantKind)
            .register(crate::delegation::SiteEgressKind)
    }

    fn kind_of(&self, approval: &Approval) -> Arc<dyn ApprovalKind> {
        self.kinds
            .get(approval.kind.as_str())
            .cloned()
            .unwrap_or_else(|| Arc::new(UnknownKind))
    }

    /// 종류에 맞는 카드.
    pub fn card_for(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        state: CardState<'_>,
    ) -> Card {
        self.kind_of(approval)
            .card(approval, conversation, self.tz, state)
    }

    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().expect("store lock")
    }

    fn conversation(&self) -> Option<ConversationId> {
        self.store()
            .notify_conversation()
            .map(ConversationId)
            .or_else(|| self.fallback_conversation.clone())
    }

    fn record(&self, approval: &Approval, kind: &str, text: &str, now: DateTime<Utc>) {
        let store = self.store();
        let _ = store.add_activity(kind, text, approval.run_id.as_deref(), now);
        // 에이전트가 다음 대화에서 결과를 알게 한다.
        let _ = store.add_episode(&NewEpisode {
            at: now,
            kind: EpisodeKind::AgentReport,
            origin: Origin::Daemon,
            conversation: approval.conversation.clone(),
            session_id: None,
            run_id: approval.run_id.clone(),
            body: format!("[승인 결과] {} — {text}", approval.summary),
        });
    }

    /// 대기 요청의 카드를 올린다. 올리지 못하면 요청을 거절로 닫는다(카드 없이 남겨 두지 않는다).
    pub async fn post_card(&self, id: &str) {
        let now = Utc::now();
        let Ok(Some(approval)) = self.store().approval(id) else {
            tracing::warn!(id, "카드를 올릴 승인 요청이 없다");
            return;
        };
        if approval.status != ApprovalStatus::Pending {
            return;
        }
        // 권한 모드 해제(결정 0040): 만들 때 "카드 없이"로 정한 요청은 바로 하고, 한 일을 닫힌 카드로 남긴다.
        if approval.auto {
            self.run_auto(approval).await;
            return;
        }
        // 사람 없는 실행이 올린 카드는 방해 금지 시간이 끝난 뒤 올린다(결정 0030). 만료는 그 뒤로 잡혀 있다.
        if crate::delegation::defer_now(&self.store(), &approval.payload, self.tz, now) {
            tracing::info!(id, "방해 금지 시간이라 카드를 미룬다");
            return;
        }
        // 카드를 올릴 대화: 요청이 정했으면(작업 스레드) 거기, 아니면 소유자 DM.
        let target = approval
            .payload
            .get("conversation")
            .and_then(serde_json::Value::as_str)
            .map(|c| ConversationId(c.to_owned()))
            .or_else(|| self.conversation());
        let failure = match target {
            None => Some("카드를 보낼 대화(소유자)가 없다".to_owned()),
            Some(conversation) => {
                let card = self.card_for(&approval, conversation.clone(), CardState::Open);
                match self.channel.send_card(card).await {
                    Ok(card_ref) => {
                        tracing::info!(id, card = %card_ref, "승인 카드를 올렸다");
                        let _ = self
                            .store()
                            .set_approval_card(id, &conversation.0, &card_ref);
                        None
                    }
                    Err(err) => Some(format!("카드를 올리지 못했다: {err}")),
                }
            }
        };
        if let Some(why) = failure {
            tracing::warn!(id, %why, "승인 카드 실패");
            // 잠금을 먼저 풀고 기록한다(record도 저장소를 잠근다).
            let decided = self.store().decide_approval(id, false, now);
            if let Ok(Decided::Changed(a)) = decided {
                self.kind_of(&a).closed(&a, &why, &self.ctx);
                self.record(&a, "approval_failed", &format!("요청을 닫음: {why}"), now);
            }
            // 에이전트는 이미 "카드를 올렸다"고 답했을 수 있으므로, 글로라도 알린다.
            let text = format!(
                "{} 승인 카드를 올리지 못해 요청을 닫았습니다(실행하지 않음): {why}",
                self.kind_of(&approval).label()
            );
            let delivered = match self.conversation() {
                Some(conversation) => self
                    .channel
                    .send(Outbound::Text {
                        conversation,
                        body: format!("[알림] {text}"),
                    })
                    .await
                    .is_ok(),
                None => false,
            };
            let _ = self.store().add_alert(&text, delivered, now);
        }
    }

    /// 해제 모드의 요청: 승인으로 바꾸고 실행한 뒤, 무엇을 했는지 닫힌 카드로 DM에 남긴다(버튼 없음).
    async fn run_auto(&self, approval: Approval) {
        let now = Utc::now();
        let approval = match self.store().decide_approval(&approval.id, true, now) {
            Ok(Decided::Changed(a)) => *a,
            _ => return,
        };
        let kind = self.kind_of(&approval);
        let done = kind.execute(&approval, &self.ctx, None).await;
        let now = Utc::now();
        let mask = crate::mask::global();
        let (record, status) = (mask.mask(&done.record), mask.mask(&done.status));
        let _ = self
            .store()
            .finish_approval(&approval.id, done.ok, &record, now);
        self.record(
            &approval,
            done.activity,
            &format!("{record}(권한 모드 해제: 카드 없이 함)"),
            now,
        );
        let target = approval
            .payload
            .get("conversation")
            .and_then(serde_json::Value::as_str)
            .map(|c| ConversationId(c.to_owned()))
            .or_else(|| self.conversation());
        if let Some(conversation) = target {
            let status = format!("🔓 권한 모드가 해제라 묻지 않고 했습니다. {status}");
            let card = self.card_for(
                &approval,
                conversation,
                CardState::Closed {
                    status: &status,
                    tone: done.tone,
                },
            );
            if let Err(err) = self.channel.send_card(card).await {
                // 해제에서는 이 기록이 사용자가 볼 유일한 흔적이다: 카드가 안 되면 글로라도 남긴다.
                tracing::warn!(id = %approval.id, %err, "해제 모드 기록 카드를 보내지 못했다");
                if let Some(conversation) = self.conversation() {
                    let _ = self
                        .channel
                        .send(Outbound::Text {
                            conversation,
                            body: format!("{status} ({})", approval.summary),
                        })
                        .await;
                }
            }
        }
    }

    async fn close(&self, approval: &Approval, status: &str, tone: CardTone) {
        let (Some(conversation), Some(card_ref)) = (&approval.conversation, &approval.card_ref)
        else {
            return;
        };
        let card = self.card_for(
            approval,
            ConversationId(conversation.clone()),
            CardState::Closed { status, tone },
        );
        if let Err(err) = self.channel.close_card(card_ref, &card).await {
            tracing::warn!(id = %approval.id, %err, "승인 카드를 닫지 못했다");
        }
    }

    /// 버튼 누름 처리.
    pub async fn handle_press(&self, press: ButtonPress) {
        let Some((id, approve, always)) = parse_action_verb(&press.action) else {
            tracing::warn!(action = %press.action, "모르는 버튼");
            return;
        };
        let allowed = {
            let store = self.store();
            access_allows(&self.access, &store, &press.platform, &press.sender).unwrap_or(false)
        };
        if !allowed {
            tracing::warn!(platform = %press.platform, sender = %press.sender, "허용되지 않은 사람의 승인 버튼, 무시");
            return;
        }
        // 앱(결정 0021)의 누름은 그 클라이언트에게 실제로 보인 카드에만 듣는다(다른 카드의 승인 ID를 넣어 누르지 못하게).
        if press.platform == crate::gateway::PLATFORM {
            let shown = self.store().approval(id).ok().flatten().is_some_and(|a| {
                a.card_ref.as_deref() == Some(press.message_ref.as_str())
                    && crate::gateway::client_of(&press.message_ref) == Some(press.sender.as_str())
            });
            if !shown {
                tracing::warn!(id, card = %press.message_ref, "앱 누름의 카드 참조가 맞지 않는다, 무시");
                return;
            }
        }
        tracing::info!(id, approve, card = %press.message_ref, "승인 버튼 누름");
        // 값을 받는 종류(인증 코드, 로그인 입력)는 값 없이 온 승인을 무시한다(입력창을 여는 누름일 수 있다).
        if approve && let Ok(Some(a)) = self.store().approval(id) {
            let kind = self.kind_of(&a);
            if (kind.needs_input() && press.input.is_none())
                || (kind.needs_form() && press.fields.is_none())
            {
                tracing::info!(id, "값 없이 온 승인 누름(입력을 기다린다)");
                return;
            }
        }
        let now = Utc::now();
        let decided = self.store().decide_approval(id, approve, now);
        let approval = match decided {
            Ok(Decided::Changed(a)) => *a,
            Ok(Decided::AlreadyFinal(status)) => {
                // 만료로 방금 닫혔으면 카드에 보인다(두 번째 누름은 조용히 무시).
                let expired = if status == ApprovalStatus::Expired {
                    self.store().approval(id).ok().flatten()
                } else {
                    None
                };
                if let Some(a) = expired
                    && a.card_ref.as_deref() == Some(press.message_ref.as_str())
                {
                    let not_done = self.kind_of(&a).not_done();
                    self.close(
                        &a,
                        &format!("⌛ 만료된 요청이라 {not_done}."),
                        CardTone::Neutral,
                    )
                    .await;
                }
                return;
            }
            Ok(Decided::NotFound) => {
                tracing::warn!(id, "없는 승인 요청의 버튼");
                return;
            }
            Err(err) => {
                tracing::error!(%err, "승인 상태를 바꾸지 못했다");
                return;
            }
        };
        if approval.card_ref.as_deref() != Some(press.message_ref.as_str()) {
            tracing::warn!(id, "다른 메시지의 버튼으로 승인됨(카드 참조 불일치)");
        }
        let kind = self.kind_of(&approval);
        if !approve {
            tracing::info!(id, "승인 요청 거절");
            self.record(&approval, "approval_denied", "거절함", now);
            kind.closed(&approval, "사용자가 거절했다", &self.ctx);
            self.close(
                &approval,
                &format!("🚫 거절했습니다. {}.", kind.not_done()),
                CardTone::Neutral,
            )
            .await;
            return;
        }
        let done = if always {
            kind.execute_always(&approval, &self.ctx).await
        } else if let Some(fields) = press.fields.as_ref().filter(|_| kind.needs_form()) {
            kind.execute_form(&approval, &self.ctx, fields).await
        } else {
            kind.execute(&approval, &self.ctx, press.input.as_ref())
                .await
        };
        let now = Utc::now();
        let mask = crate::mask::global();
        let (record, status) = (mask.mask(&done.record), mask.mask(&done.status));
        let _ = self
            .store()
            .finish_approval(&approval.id, done.ok, &record, now);
        self.record(&approval, done.activity, &record, now);
        // 사람 없는 실행이 올린 카드면 같은 사슬로 이어 맡긴다(결정 0030). 실패해도 그 사실을 들고 다시 계획한다.
        if !approval.payload["resume"].is_null() && kind.resume_wanted(&approval) {
            let done_text = format!("{} → {}", approval.summary, record);
            let resumed = crate::delegation::resume_after(
                &self.store(),
                &approval.payload,
                &done_text,
                self.tz,
                now,
            );
            if let Some(f) = resumed {
                tracing::info!(id = %approval.id, followup = %f, "승인 뒤 이어 하기를 걸었다");
            }
        }
        self.close(&approval, &status, done.tone).await;
    }

    /// 슬래시 명령(결정 0040). 소유자만 받는다. `/권한`: 지금 모드를 보이거나 바꾼다. 바꾸면 활동 기록과 DM 알림을
    /// 남긴다(본인에게만 보이는 답과 따로, 기록으로).
    pub async fn handle_command(&self, cmd: lucia_channel::SlashCommand) {
        use lucia_channel::CommandReply;
        let allowed = {
            let store = self.store();
            access_allows(&self.access, &store, &cmd.platform, &cmd.sender).unwrap_or(false)
        };
        if !allowed {
            tracing::warn!(platform = %cmd.platform, sender = %cmd.sender, "소유자가 아닌 사람의 명령, 거절");
            let _ = cmd.reply.send(CommandReply::private(
                "Lucia의 소유자만 쓸 수 있는 명령입니다.",
            ));
            return;
        }
        if cmd.name == lucia_channel::COMMAND_MODEL {
            return self.model_command(cmd).await;
        }
        if cmd.name != lucia_channel::COMMAND_SECURITY {
            let _ = cmd.reply.send(CommandReply::private("모르는 명령입니다."));
            return;
        }
        let now = Utc::now();
        let reply = match cmd.option.as_deref().map(crate::security::Mode::parse) {
            None => CommandReply {
                text: crate::security::describe(crate::security::mode(&self.store())),
                private: !cmd.in_dm,
            },
            Some(None) => CommandReply::private("모드는 엄격, 기본, 해제 중 하나입니다."),
            // 답을 받을 쪽이 이미 떠났으면(어댑터가 시간 초과로 답함) 바꾸지 않는다: 사용자는 실패로 알고 있다.
            Some(Some(_)) if cmd.reply.is_closed() => {
                tracing::warn!("늦게 꺼낸 /권한 명령이라 적용하지 않았다");
                return;
            }
            Some(Some(mode)) => {
                let by = if cmd.platform == crate::gateway::PLATFORM {
                    "앱 /권한"
                } else {
                    "Discord /권한"
                };
                let changed = crate::security::set_mode(&self.store(), mode, by, now);
                match changed {
                    Ok(changed) => {
                        let head = if changed {
                            "권한 모드를 바꿨습니다."
                        } else {
                            "이미 그 모드입니다."
                        };
                        let text = format!("{head}\n{}", crate::security::describe(mode));
                        if changed {
                            self.note_command_change(&cmd, &text).await;
                        }
                        CommandReply {
                            text,
                            private: !cmd.in_dm,
                        }
                    }
                    Err(e) => CommandReply::private(&format!("바꾸지 못했습니다: {e}")),
                }
            }
        };
        let _ = cmd.reply.send(reply);
    }

    /// 명령으로 설정을 바꿨을 때 기록을 남긴다. 서버에서 불렀으면 본인에게만 답하므로 DM에 남기고, 앱에서 바꿨으면
    /// 앱 밖(알림 대화)에 남긴다(토큰이 새어 바뀐 것이면 알아챌 수 있게, 결정 0043). DM에서 부른 것은 답이 곧 기록이다.
    async fn note_command_change(&self, cmd: &lucia_channel::SlashCommand, text: &str) {
        let from_app = cmd.platform == crate::gateway::PLATFORM;
        if cmd.in_dm && !from_app {
            return;
        }
        let record_to = if from_app {
            self.store()
                .setting(lucia_store::keys::NOTIFY_CONVERSATION)
                .ok()
                .flatten()
                .map(ConversationId)
                .or_else(|| self.conversation())
        } else {
            self.conversation()
        };
        if let Some(conversation) = record_to {
            let _ = self
                .channel
                .send(Outbound::Text {
                    conversation,
                    body: text.to_owned(),
                })
                .await;
        }
    }

    /// `/모델`(결정 0055): 주 모델을 보이거나 바꾼다. 소유자 확인은 부른 쪽([`Self::handle_command`])이 했다.
    async fn model_command(&self, cmd: lucia_channel::SlashCommand) {
        use lucia_channel::CommandReply;
        let Some(models) = &self.models else {
            let _ = cmd.reply.send(CommandReply::private(
                "이 데몬에서는 주 모델을 바꿀 수 없습니다.",
            ));
            return;
        };
        let private = !cmd.in_dm;
        let Some(option) = cmd.option.as_deref().map(str::trim) else {
            let _ = cmd.reply.send(CommandReply {
                text: models.describe(),
                private,
            });
            return;
        };
        let to = if crate::models::DEFAULT_WORDS.contains(&option) {
            None
        } else {
            match crate::models::parse(option) {
                Some(choice) => Some(choice),
                None => {
                    let _ = cmd.reply.send(CommandReply::private(
                        "모델은 fable, opus, sonnet, haiku, 기본 중 하나입니다.",
                    ));
                    return;
                }
            }
        };
        // 답을 받을 쪽이 이미 떠났으면(어댑터가 시간 초과로 답함) 바꾸지 않는다: 사용자는 실패로 알고 있다.
        if cmd.reply.is_closed() {
            tracing::warn!("늦게 꺼낸 /모델 명령이라 적용하지 않았다");
            return;
        }
        let by = if cmd.platform == crate::gateway::PLATFORM {
            "앱 /모델"
        } else {
            "Discord /모델"
        };
        let changed = models.set(&self.store(), to, by, Utc::now());
        let reply = match changed {
            Ok(true) => {
                let text = format!(
                    "주 모델을 {}(으)로 바꿨습니다.\n{}",
                    models.label(),
                    crate::models::CHANGE_NOTE
                );
                self.note_command_change(&cmd, &text).await;
                CommandReply { text, private }
            }
            Ok(false) => CommandReply {
                text: format!("이미 {}입니다.", models.label()),
                private,
            },
            Err(e) => CommandReply::private(&format!("바꾸지 못했습니다: {e}")),
        };
        let _ = cmd.reply.send(reply);
    }

    /// 만료된 요청을 닫는다.
    pub async fn sweep(&self) {
        let now = Utc::now();
        let expired = self.store().expire_approvals(now).unwrap_or_default();
        for a in expired {
            let kind = self.kind_of(&a);
            kind.closed(&a, "답이 없어 만료됐다", &self.ctx);
            self.record(&a, "approval_expired", "답이 없어 만료(실행하지 않음)", now);
            self.close(
                &a,
                &format!("⌛ 답이 없어 만료됐습니다. {}.", kind.not_done()),
                CardTone::Neutral,
            )
            .await;
        }
        // 방해 금지 시간이라 미뤄 둔 카드를 끝난 뒤 올린다(결정 0030).
        let deferred: Vec<String> = self
            .store()
            .recent_approvals(100)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| {
                a.status == ApprovalStatus::Pending
                    && a.card_ref.is_none()
                    && a.payload["defer_quiet"].as_bool() == Some(true)
            })
            .map(|a| a.id)
            .collect();
        for id in deferred {
            self.post_card(&id).await;
        }
    }

    /// 시작할 때: 실행 도중 멈춘 요청을 닫고, 카드를 못 올린 대기 요청은 다시 올린다.
    pub async fn recover(&self) {
        let now = Utc::now();
        let interrupted = self
            .store()
            .fail_interrupted_approvals(now)
            .unwrap_or_default();
        for a in interrupted {
            let (activity, record, status) = self.kind_of(&a).interrupted();
            self.record(&a, activity, record, now);
            self.close(&a, status, CardTone::Warning).await;
        }
        self.sweep().await;
        let unposted: Vec<String> = self
            .store()
            .recent_approvals(50)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.status == ApprovalStatus::Pending && a.card_ref.is_none())
            .map(|a| a.id)
            .collect();
        for id in unposted {
            self.post_card(&id).await;
        }
    }
}

/// 데스크 작업: 카드 올리기, 버튼 누름, 만료 정리를 한 줄로 처리한다.
pub async fn run_desk<C: ChannelAdapter>(
    desk: Arc<ApprovalDesk<C>>,
    mut posts: mpsc::UnboundedReceiver<String>,
    mut presses: mpsc::Receiver<ButtonPress>,
    mut commands: Option<mpsc::Receiver<lucia_channel::SlashCommand>>,
    cancel: tokio_util::sync::CancellationToken,
) {
    desk.recover().await;
    let mut tick = tokio::time::interval(SWEEP_EVERY);
    loop {
        let command = async {
            match commands.as_mut() {
                Some(rx) => rx.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => return,
            Some(id) = posts.recv() => desk.post_card(&id).await,
            Some(press) = presses.recv() => desk.handle_press(press).await,
            // 명령은 줄 밖에서 바로 처리한다(승인된 동작을 실행하는 동안 기다리다 시간 초과로 답한 뒤 적용되지 않게).
            Some(cmd) = command => {
                let desk = desk.clone();
                tokio::spawn(async move { desk.handle_command(cmd).await });
            }
            _ = tick.tick() => desk.sweep().await,
        }
    }
}

#[cfg(test)]
mod tests;
