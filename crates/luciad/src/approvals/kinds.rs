//! 승인 종류(결정 0013). 종류마다 카드 모양, 실행, 닫힘 알림, 재시작 정리를 한곳에 둔다.
//!
//! 데스크([`super::ApprovalDesk`])는 종류를 모르고 등록된 [`ApprovalKind`]에 맡긴다. 새 종류(작업 완료 확인,
//! 스킬 고정, git push, 나중의 외부 기기 동작)는 이 트레이트를 구현해 등록한다.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use lucia_core::{Card, CardTone, ConversationId};
use lucia_precheck::google::assist::SendError;
use lucia_store::{Approval, Store};

use super::{
    BrowserActionExec, CardState, KIND_BROWSER_ACTION, KIND_HELD_WRITE, KIND_MAIL_SEND,
    KIND_SITE_LOGIN, KIND_TASK_DONE, Mailer, SiteLoginExec, browser_card, held_card, login_card,
    mail_card, mail_of, task_card,
};
use crate::browser::BoxFut;

/// 실행에 필요한 데몬 상태.
pub struct KindCtx {
    pub store: Arc<Mutex<Store>>,
    pub tz: chrono_tz::Tz,
}

impl KindCtx {
    pub fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().expect("store lock")
    }
}

/// 승인된 요청을 실행한 결과. 데스크가 저장소·활동 기록·에피소드에 남기고 카드를 닫는다.
pub struct Executed {
    pub ok: bool,
    /// 활동 기록 종류(예: `mail_sent`).
    pub activity: &'static str,
    /// 기록할 글(마스킹 전).
    pub record: String,
    /// 카드에 보일 결과 문구(마스킹 전).
    pub status: String,
    pub tone: CardTone,
}

impl Executed {
    /// 흔한 모양: 성공이면 ✅, 실패면 ❌.
    pub fn simple(
        result: Result<String, String>,
        ok_kind: &'static str,
        fail_kind: &'static str,
    ) -> Self {
        match result {
            Ok(note) => Self {
                ok: true,
                activity: ok_kind,
                status: format!("✅ {note}"),
                record: note,
                tone: CardTone::Success,
            },
            Err(why) => Self {
                ok: false,
                activity: fail_kind,
                status: format!("❌ {why}"),
                record: why,
                tone: CardTone::Danger,
            },
        }
    }
}

/// 승인 종류 하나.
pub trait ApprovalKind: Send + Sync {
    fn kind(&self) -> &'static str;
    /// 알림 문구에 쓰는 이름(예: "메일 발송").
    fn label(&self) -> &'static str;
    /// 거절·만료로 닫을 때의 "하지 않았다" 문구.
    fn not_done(&self) -> &'static str;
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card;
    /// 승인된 요청을 한 번 실행한다(다시 시도하지 않는다). `input`은 값을 받는 버튼의 값(결정 0017).
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
        input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed>;
    /// 승인 버튼이 값을 받아야 하는가(값 없이 온 승인은 무시한다).
    fn needs_input(&self) -> bool {
        false
    }
    /// 승인 버튼이 여러 칸을 받아야 하는가(결정 0063). 칸 없이 온 승인은 무시한다.
    fn needs_form(&self) -> bool {
        false
    }
    /// 여러 칸을 받은 승인을 실행한다. 값은 기록·카드 문구에 넣지 않는다.
    fn execute_form<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
        _fields: &'a std::collections::BTreeMap<String, lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        self.execute(approval, ctx, None)
    }
    /// 거절·만료로 닫혔다(기다리는 쪽에 알린다). `approval.status`로 거절과 만료를 가른다.
    fn closed(&self, _approval: &Approval, _why: &str, _ctx: &KindCtx) {}
    /// 실행 도중 데몬이 멈췄을 때: (활동 기록 종류, 기록 글, 카드 문구).
    fn interrupted(&self) -> (&'static str, &'static str, &'static str);
    /// [계속 허용] 단추(결정 0028). 그 단추를 보이는 종류만 다르게 한다. 기본은 보통 승인과 같다.
    fn execute_always<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
    ) -> BoxFut<'a, Executed> {
        self.execute(approval, ctx, None)
    }
    /// 승인 뒤 이어 하기를 걸지(카드에 이어 하기 정보가 있을 때, 결정 0030).
    fn resume_wanted(&self, _approval: &Approval) -> bool {
        true
    }
}

/// 메일 발송(결정 0003).
pub struct MailKind<M> {
    pub mailer: M,
}

impl<M: Mailer> ApprovalKind for MailKind<M> {
    fn kind(&self) -> &'static str {
        KIND_MAIL_SEND
    }
    fn label(&self) -> &'static str {
        "메일 발송"
    }
    fn not_done(&self) -> &'static str {
        "보내지 않았습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        mail_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        _ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let mail = match mail_of(approval) {
                Ok(m) => m,
                Err(why) => {
                    return Executed {
                        ok: false,
                        activity: "send_failed",
                        status: format!("❌ 보내지 못했습니다: {why}"),
                        record: why,
                        tone: CardTone::Danger,
                    };
                }
            };
            let result = self.mailer.send(&mail).await;
            tracing::info!(id = %approval.id, sent = result.is_ok(), "승인된 발송 결과");
            match result {
                Ok(message_id) => Executed {
                    ok: true,
                    activity: "mail_sent",
                    record: format!("보냄(Gmail {message_id})"),
                    status: "✅ 보냈습니다.".into(),
                    tone: CardTone::Success,
                },
                Err(SendError::Unknown(why)) => Executed {
                    ok: false,
                    activity: "send_failed",
                    record: format!("결과 모름: {why}"),
                    status: format!(
                        "⚠️ 보냈는지 알 수 없습니다({why}). Gmail 보낸편지함을 확인하세요. 다시 보내지 않습니다."
                    ),
                    tone: CardTone::Warning,
                },
                Err(err) => Executed {
                    ok: false,
                    activity: "send_failed",
                    record: err.to_string(),
                    status: format!("❌ {err}. 다시 보내지 않습니다."),
                    tone: CardTone::Danger,
                },
            }
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "send_failed",
            "보내는 도중 데몬이 멈춤(결과 모름)",
            "⚠️ 보내는 도중 데몬이 멈춰 결과를 모릅니다. Gmail 보낸편지함을 확인하세요.",
        )
    }
}

/// 저장된 로그인으로 다시 로그인(결정 0006). 브라우저가 꺼져 있으면 실행하지 않는다.
pub struct SiteLoginKind {
    pub exec: Option<Arc<dyn SiteLoginExec>>,
}

impl ApprovalKind for SiteLoginKind {
    fn kind(&self) -> &'static str {
        KIND_SITE_LOGIN
    }
    fn label(&self) -> &'static str {
        "사이트 로그인"
    }
    fn not_done(&self) -> &'static str {
        "로그인하지 않았습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        login_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        _ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let site = approval.payload["site"].as_str().unwrap_or("").to_owned();
            let result = match &self.exec {
                Some(exec) => exec.login_saved(&site).await,
                None => Err("브라우저가 꺼져 있다".to_owned()),
            };
            tracing::info!(id = %approval.id, ok = result.is_ok(), "승인된 재로그인 결과");
            use crate::browser::logins::SavedLogin;
            match result {
                Ok(SavedLogin::LoggedIn(note)) => Executed {
                    ok: true,
                    activity: "login_ok",
                    record: "로그인함".into(),
                    status: format!("✅ {note}"),
                    tone: CardTone::Success,
                },
                // 아직 로그인하지 않았다: 인증 코드를 기다린다(결정 0017). "로그인함"으로 남기지 않는다.
                Ok(SavedLogin::NeedsCode(note)) => Executed {
                    ok: true,
                    activity: "login_code",
                    record: "비밀번호는 받아들였고 인증 코드를 기다림(아직 로그인 안 됨)".into(),
                    status: format!("🔢 {note}"),
                    tone: CardTone::Warning,
                },
                Err(why) => Executed {
                    ok: false,
                    activity: "login_failed",
                    record: format!("로그인 실패: {why}"),
                    status: format!("❌ 로그인하지 못했습니다: {why}"),
                    tone: CardTone::Danger,
                },
            }
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "login_failed",
            "로그인 도중 데몬이 멈춤",
            "⚠️ 로그인 도중 데몬이 멈췄습니다. 필요하면 다시 요청해 주세요.",
        )
    }
}

/// 브라우저 동작(결정 0008): 같은 주소·같은 요소인지 다시 확인한 뒤 한 번 실행한다.
pub struct BrowserActionKind {
    pub exec: Option<Arc<dyn BrowserActionExec>>,
}

impl ApprovalKind for BrowserActionKind {
    fn kind(&self) -> &'static str {
        KIND_BROWSER_ACTION
    }
    fn label(&self) -> &'static str {
        "브라우저 동작"
    }
    fn not_done(&self) -> &'static str {
        "실행하지 않았습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        browser_card(approval, conversation, tz, state)
    }
    fn resume_wanted(&self, approval: &Approval) -> bool {
        // 기다리던 도구 호출이 결과를 받아 갔으면 그 실행이 이어 한다.
        !self
            .exec
            .as_ref()
            .is_some_and(|e| e.delivered_live(&approval.id))
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        _ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let result = match &self.exec {
                Some(exec) => exec.run_approved(&approval.id, &approval.payload).await,
                None => Err("브라우저가 꺼져 있다".to_owned()),
            };
            tracing::info!(id = %approval.id, ok = result.is_ok(), "승인된 브라우저 동작 결과");
            match result {
                Ok(note) => Executed {
                    ok: true,
                    activity: "browser_act",
                    status: format!("✅ {note}"),
                    record: note,
                    tone: CardTone::Success,
                },
                Err(why) => Executed {
                    ok: false,
                    activity: "browser_failed",
                    record: format!("실행 안 함: {why}"),
                    status: format!("❌ 실행하지 않았습니다: {why}"),
                    tone: CardTone::Danger,
                },
            }
        })
    }
    fn closed(&self, approval: &Approval, why: &str, _ctx: &KindCtx) {
        if let Some(exec) = &self.exec {
            exec.closed(&approval.id, why);
        }
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "browser_failed",
            "실행 도중 데몬이 멈춤(결과 모름)",
            "⚠️ 실행 도중 데몬이 멈춰 결과를 모릅니다. 필요하면 다시 요청해 주세요.",
        )
    }
}

const WRITE_INTERRUPTED: (&str, &str, &str) = (
    "write_failed",
    "처리 도중 데몬이 멈춤",
    "⚠️ 처리 도중 데몬이 멈춰 반영됐는지 모릅니다. 관리 페이지 기억 화면에서 확인해 주세요.",
);

/// 인젝션 검사에 걸려 보류한 쓰기(결정 0010). 승인 = 적용, 거절 = 버리기.
pub struct HeldWriteKind;

impl ApprovalKind for HeldWriteKind {
    fn kind(&self) -> &'static str {
        KIND_HELD_WRITE
    }
    fn label(&self) -> &'static str {
        "보류한 쓰기"
    }
    fn not_done(&self) -> &'static str {
        "버렸습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        held_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            Executed::simple(
                apply_held(ctx, approval, Utc::now()),
                "write_applied",
                "write_failed",
            )
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        WRITE_INTERRUPTED
    }
}

/// 보류한 쓰기를 적용한다(사용자가 [적용]을 눌렀다). 기억은 보류할 때의 개정 번호를 다시 확인한다.
fn apply_held(ctx: &KindCtx, approval: &Approval, now: DateTime<Utc>) -> Result<String, String> {
    let p = &approval.payload;
    let t = |k: &str| p[k].as_str().unwrap_or("").to_owned();
    let store = ctx.store();
    if t("type") == "memory" {
        let section =
            lucia_store::MemorySection::parse(&t("section")).ok_or("저장된 갈래가 이상하다")?;
        let revision = p["revision"].as_i64().unwrap_or(0);
        let (name, title, summary, body) = (t("name"), t("title"), t("summary"), t("body"));
        store
            .write_memory(
                &lucia_store::MemoryWrite {
                    slug: &name,
                    section,
                    title: &title,
                    summary: &summary,
                    body: &body,
                },
                (revision != 0).then_some(revision),
                lucia_store::MemoryAuthor::Agent,
                approval.run_id.as_deref(),
                &format!("{}(보류 뒤 사용자가 적용)", t("reason")),
                now,
            )
            .map_err(|e| format!("적용하지 못했습니다: {e}"))?;
        Ok(format!("기억 [[{name}]]에 적용했습니다."))
    } else {
        let req = crate::autonomy::FollowupRequest {
            when: t("when"),
            task: t("task"),
            detail: t("detail"),
            conversation: None,
            run_id: approval.run_id.clone(),
            origin: t("origin"),
        };
        let due = match crate::autonomy::check_followup(&store, &req, ctx.tz, now) {
            Ok(due) => due,
            // 사람이 내용을 보고 적용했다.
            Err(crate::autonomy::FollowupRefusal::Suspicious(_)) => {
                crate::autonomy::parse_when(&req.when, ctx.tz)?
            }
            Err(crate::autonomy::FollowupRefusal::Invalid(why)) => {
                return Err(format!("등록하지 못했습니다: {why}"));
            }
        };
        let id = crate::autonomy::create_followup(&store, &req, due, ctx.tz, now)?;
        Ok(format!(
            "후속 조치로 등록했습니다({id}, {}).",
            due.with_timezone(&ctx.tz).format("%m-%d %H:%M")
        ))
    }
}

/// 작업 완료 확인(결정 0014). 완료는 사람만 정한다.
pub struct TaskDoneKind;

impl ApprovalKind for TaskDoneKind {
    fn kind(&self) -> &'static str {
        KIND_TASK_DONE
    }
    fn label(&self) -> &'static str {
        "작업 완료 확인"
    }
    fn not_done(&self) -> &'static str {
        "완료로 두지 않았습니다. 더 할 일을 말해 주세요"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        task_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let id = approval.payload["task"].as_str().unwrap_or("").to_owned();
            let now = Utc::now();
            let store = ctx.store();
            let result =
                match store.move_task(&id, lucia_core::TaskStatus::Done, "사용자가 완료 확인", now)
                {
                    Ok(true) => {
                        let _ = store.add_activity(
                            "task_done",
                            &format!("작업 완료 확인({id})"),
                            None,
                            now,
                        );
                        // 이 작업이 읽은 스킬마다 성공을 센다(사람의 확인만 센다, 결정 0018).
                        let promoted = store
                            .count_skill_successes(&id, crate::skills::PROMOTE_AT, now)
                            .unwrap_or_default();
                        if !promoted.is_empty() {
                            let _ = store.add_activity(
                                "skill_success",
                                &format!("스킬 성공({id}): {}", promoted.join(", ")),
                                None,
                                now,
                            );
                        }
                        Ok("완료로 표시했습니다.".to_owned())
                    }
                    Ok(false) => Err("이미 상태가 바뀐 작업이라 그대로 둡니다".to_owned()),
                    Err(e) => Err(e.to_string()),
                };
            Executed::simple(result, "task_done", "task_failed")
        })
    }
    fn closed(&self, approval: &Approval, why: &str, ctx: &KindCtx) {
        let id = approval.payload["task"].as_str().unwrap_or("");
        let (to, note) = match approval.status {
            lucia_store::ApprovalStatus::Expired => (
                lucia_core::TaskStatus::Expired,
                "완료 확인 카드가 만료됨".to_owned(),
            ),
            // 사용자의 [아직], 또는 카드를 올리지 못해 닫힘: 사람의 말을 기다린다.
            _ => (
                lucia_core::TaskStatus::WaitingInput,
                format!("완료 확인 카드 닫힘({why}): 더 할 일을 기다림"),
            ),
        };
        // 카드를 올린 뒤 작업이 이미 다시 돌고 있으면(사람이 말을 더해 이어 하는 중) 그대로 둔다.
        let store = ctx.store();
        if store
            .task(id)
            .ok()
            .flatten()
            .is_some_and(|t| t.status == lucia_core::TaskStatus::Reported)
        {
            let _ = store.move_task(id, to, &note, Utc::now());
        }
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "task_failed",
            "확인 처리 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다. 관리 페이지에서 작업 상태를 확인해 주세요.",
        )
    }
}

/// 캘린더 쓰기(결정 0033). 카드에 고정한 연산·일정·본문 그대로 한다.
pub struct CalendarKind {
    pub exec: Option<Arc<dyn crate::calendar::CalendarExec>>,
}

impl ApprovalKind for CalendarKind {
    fn kind(&self) -> &'static str {
        crate::calendar::KIND
    }
    fn label(&self) -> &'static str {
        "캘린더"
    }
    fn not_done(&self) -> &'static str {
        "캘린더를 바꾸지 않았습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        super::calendar_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let Some(exec) = &self.exec else {
                return Executed::simple(
                    Err("이 데몬에는 Google 캘린더가 연결되지 않았다".into()),
                    "calendar_write",
                    "calendar_failed",
                );
            };
            let p = &approval.payload;
            let op = p["op"].as_str().unwrap_or("");
            let event = p["event"].as_str();
            let result = exec.apply(op, event, &p["body"]).await;
            let result = result.map(|after| {
                let id = after
                    .as_ref()
                    .and_then(|a| a.get("id").and_then(serde_json::Value::as_str))
                    .or(event)
                    .unwrap_or("")
                    .to_owned();
                let before = p.get("before").filter(|b| !b.is_null());
                crate::calendar::record(
                    &ctx.store(),
                    op,
                    &id,
                    before,
                    after.as_ref(),
                    approval.run_id.as_deref(),
                    Utc::now(),
                    ctx.tz,
                );
                match op {
                    "create" => "일정을 추가했습니다.".to_owned(),
                    "update" => "일정을 고쳤습니다.".to_owned(),
                    _ => "일정을 지웠습니다.".to_owned(),
                }
            });
            Executed::simple(result, "calendar_approved", "calendar_failed")
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "calendar_failed",
            "처리 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다. 캘린더에서 바뀌었는지 확인해 주세요.",
        )
    }
}

/// 목표 완료 확인(결정 0032). 끝났는지는 사람이 정한다.
pub struct GoalDoneKind;

impl ApprovalKind for GoalDoneKind {
    fn kind(&self) -> &'static str {
        super::KIND_GOAL_DONE
    }
    fn label(&self) -> &'static str {
        "목표 완료 확인"
    }
    fn not_done(&self) -> &'static str {
        "목표를 계속 진행합니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        super::goal_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let id = approval.payload["goal"].as_str().unwrap_or("").to_owned();
            let store = ctx.store();
            let result = crate::goals::close(
                &store,
                &id,
                lucia_store::GoalStatus::Done,
                "사용자가 완료 확인",
                Utc::now(),
            );
            Executed::simple(result, "goal_done", "goal_failed")
        })
    }
    fn closed(&self, approval: &Approval, why: &str, ctx: &KindCtx) {
        if approval.status == lucia_store::ApprovalStatus::Denied {
            let id = approval.payload["goal"].as_str().unwrap_or("");
            let _ = ctx.store().add_goal_log(
                id,
                "status",
                &format!("사용자가 아직이라고 함({why})"),
                Utc::now(),
            );
        }
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "goal_failed",
            "확인 처리 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다. 관리 페이지에서 목표 상태를 확인해 주세요.",
        )
    }
}

/// 인증 코드 입력(결정 0017). [코드 입력]이 Discord 입력창으로 값을 받는다.
pub struct LoginCodeKind {
    pub exec: Option<Arc<dyn SiteLoginExec>>,
}

impl ApprovalKind for LoginCodeKind {
    fn kind(&self) -> &'static str {
        super::KIND_LOGIN_CODE
    }
    fn label(&self) -> &'static str {
        "인증 코드"
    }
    fn not_done(&self) -> &'static str {
        "로그인하지 않았습니다"
    }
    fn needs_input(&self) -> bool {
        true
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        super::code_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        _ctx: &'a KindCtx,
        input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let site = approval.payload["site"].as_str().unwrap_or("").to_owned();
            let result = match (&self.exec, input) {
                (Some(exec), Some(code)) => exec.submit_code(&site, code.expose()).await,
                (None, _) => Err("브라우저가 꺼져 있다".to_owned()),
                (_, None) => Err("코드가 없다".to_owned()),
            };
            // 기록에는 코드를 넣지 않는다(결과 문구만).
            Executed::simple(result, "login_ok", "login_failed")
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "login_failed",
            "인증 코드 처리 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다. 필요하면 다시 로그인을 요청해 주세요.",
        )
    }
}

/// 로그인 정보 입력(결정 0063). 자체 앱의 카드에서 아이디·비밀번호·저장 여부를 받는다.
/// 받은 값은 로그인 모듈로 곧장 넘기고, 승인 기록·활동 기록·카드 문구에는 넣지 않는다.
pub struct LoginInputKind {
    pub exec: Option<Arc<dyn SiteLoginExec>>,
}

impl ApprovalKind for LoginInputKind {
    fn kind(&self) -> &'static str {
        super::KIND_LOGIN_INPUT
    }
    fn label(&self) -> &'static str {
        "로그인 입력"
    }
    fn not_done(&self) -> &'static str {
        "로그인하지 않았습니다"
    }
    fn needs_form(&self) -> bool {
        true
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        super::login_input_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        _approval: &'a Approval,
        _ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async {
            Executed::simple(
                Err("아이디와 비밀번호가 오지 않았다".to_owned()),
                "login_input",
                "login_failed",
            )
        })
    }
    fn execute_form<'a>(
        &'a self,
        approval: &'a Approval,
        _ctx: &'a KindCtx,
        fields: &'a std::collections::BTreeMap<String, lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let token = approval.payload["token"].as_str().unwrap_or("");
            let text = |k: &str| fields.get(k).map_or("", |v| v.expose()).to_owned();
            let save = fields.get("save").is_some_and(|v| v.expose() == "true");
            let result = match &self.exec {
                Some(exec) => {
                    exec.clone()
                        .submit_input(token, text("username"), text("password"), save)
                }
                None => Err("브라우저가 꺼져 있다".to_owned()),
            };
            match result {
                // 아직 로그인한 것이 아니다: 받았다는 것만 남긴다(결과는 로그인 모듈이 알린다).
                Ok(note) => Executed {
                    ok: true,
                    activity: "login_input",
                    record: "로그인 정보를 받아 로그인을 시작함".into(),
                    status: format!("⏳ {note}"),
                    tone: CardTone::Neutral,
                },
                Err(why) => Executed {
                    ok: false,
                    activity: "login_failed",
                    record: format!("로그인 입력을 쓰지 못함: {why}"),
                    status: format!("❌ {why}"),
                    tone: CardTone::Danger,
                },
            }
        })
    }
    fn closed(&self, approval: &Approval, _why: &str, _ctx: &KindCtx) {
        // 사용자가 취소했을 때만 요청(링크 포함)을 닫는다. 만료는 그대로 둔다: 요청은 제 시간에 스스로 닫히고,
        // 링크에서 다시 넣는 중이면 그 시간은 이 카드보다 길다.
        if approval.status == lucia_store::ApprovalStatus::Denied
            && let (Some(exec), Some(token)) = (&self.exec, approval.payload["token"].as_str())
        {
            exec.cancel_input(token);
        }
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "login_failed",
            "로그인 입력 처리 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다. 필요하면 다시 로그인을 요청해 주세요.",
        )
    }
}

/// 스킬 검토(결정 0018).
pub struct SkillReviewKind;

impl ApprovalKind for SkillReviewKind {
    fn kind(&self) -> &'static str {
        super::KIND_SKILL_REVIEW
    }
    fn label(&self) -> &'static str {
        "스킬 검토"
    }
    fn not_done(&self) -> &'static str {
        "그 스킬을 쓰지 않습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        super::skill_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let p = &approval.payload;
            let slug = p["slug"].as_str().unwrap_or("").to_owned();
            let now = Utc::now();
            let store = ctx.store();
            let result = match store.skill(&slug) {
                Ok(Some(skill)) if p["mode"] == "notice" => store
                    .set_skill_pinned(&skill.slug, true, now)
                    .map(|_| "고정했습니다(자동 수정·보관에서 뺍니다).".to_owned())
                    .map_err(|e| e.to_string()),
                Ok(Some(skill)) => {
                    // 사람이 대화에서 만든 것은 바로 활성, 회고가 만든 것은 시험부터.
                    let to = if skill.source == "chat" {
                        lucia_store::SkillStatus::Active
                    } else {
                        lucia_store::SkillStatus::Trial
                    };
                    store
                        .set_skill_status(&skill.slug, to, now)
                        .map(|_| format!("승인했습니다({}).", to.label()))
                        .map_err(|e| e.to_string())
                }
                Ok(None) => Err("스킬이 없습니다".to_owned()),
                Err(e) => Err(e.to_string()),
            };
            Executed::simple(result, "skill_review", "skill_review")
        })
    }
    fn closed(&self, approval: &Approval, _why: &str, ctx: &KindCtx) {
        let p = &approval.payload;
        let slug = p["slug"].as_str().unwrap_or("");
        let expired = approval.status == lucia_store::ApprovalStatus::Expired;
        // 카드를 올리지 못해 닫힌 것은 사람이 폐기한 것이 아니다: 그대로 둔다(관리 페이지에서 승인할 수 있다).
        if approval.card_ref.is_none() {
            return;
        }
        // 읽기만 하는 새 스킬은 답이 없으면 그대로(시험 중), 폐기를 누르면 보관.
        if p["mode"] == "notice" && expired {
            return;
        }
        let _ = ctx
            .store()
            .set_skill_status(slug, lucia_store::SkillStatus::Archived, Utc::now());
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "skill_review",
            "스킬 검토 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다. 관리 페이지 스킬 화면에서 확인해 주세요.",
        )
    }
}

/// 감시 승인(결정 0019).
pub struct WatchReviewKind;

impl ApprovalKind for WatchReviewKind {
    fn kind(&self) -> &'static str {
        super::KIND_WATCH_REVIEW
    }
    fn label(&self) -> &'static str {
        "페이지 감시"
    }
    fn not_done(&self) -> &'static str {
        "감시하지 않습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        super::watch_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let id = approval.payload["id"].as_str().unwrap_or("").to_owned();
            // 로그인 세션·브라우저로 읽는 감시(결정 0036): 그 사이트의 읽기 위임이 없으면 함께 만든다(카드에 적었다).
            let fetcher = approval.payload["fetcher"].as_str().unwrap_or("plain");
            if fetcher != lucia_store::watch_fetcher::PLAIN {
                let host = crate::watch::host_of(approval.payload["url"].as_str().unwrap_or(""));
                let store = ctx.store();
                if !host.is_empty()
                    && crate::delegation::grant_for(&store, &host, Utc::now()).is_none()
                {
                    let gid = format!("grant-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
                    let _ = store.grant_site(
                        &lucia_store::NewSiteGrant {
                            id: &gid,
                            host: &host,
                            login: false,
                            actions: false,
                            purpose: approval.payload["what"].as_str().unwrap_or("페이지 감시"),
                            origin: "watch",
                            expires_at: None,
                        },
                        Utc::now(),
                    );
                }
            }
            let result = ctx
                .store()
                .set_watch_status(&id, lucia_store::WatchStatus::Active, Utc::now())
                .map_err(|e| e.to_string())
                .and_then(|ok| {
                    if ok {
                        Ok("감시를 시작했습니다. 첫 확인은 기준선만 잡습니다.".to_owned())
                    } else {
                        Err("감시가 없습니다".to_owned())
                    }
                });
            Executed::simple(result, "watch_start", "watch_failed")
        })
    }
    fn closed(&self, approval: &Approval, _why: &str, ctx: &KindCtx) {
        let id = approval.payload["id"].as_str().unwrap_or("");
        let store = ctx.store();
        if store
            .watch(id)
            .ok()
            .flatten()
            .is_some_and(|w| w.status == lucia_store::WatchStatus::Pending)
        {
            let _ = store.set_watch_status(id, lucia_store::WatchStatus::Removed, Utc::now());
        }
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "watch_failed",
            "감시 승인 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다. 관리 페이지에서 감시 상태를 확인해 주세요.",
        )
    }
}

/// 루틴 승인(결정 0025).
pub struct RoutineReviewKind;

impl ApprovalKind for RoutineReviewKind {
    fn kind(&self) -> &'static str {
        super::KIND_ROUTINE_REVIEW
    }
    fn label(&self) -> &'static str {
        "반복 루틴"
    }
    fn not_done(&self) -> &'static str {
        "켜지 않습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        super::routine_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let id = approval.payload["id"].as_str().unwrap_or("").to_owned();
            let now = Utc::now();
            let result = (|| {
                let store = ctx.store();
                let r = store
                    .routine(&id)
                    .map_err(|e| e.to_string())?
                    .filter(|r| r.status == lucia_store::RoutineStatus::Review)
                    .ok_or("승인을 기다리는 루틴이 없습니다")?;
                store
                    .set_routine_status(&id, lucia_store::RoutineStatus::Active, now)
                    .map_err(|e| e.to_string())?;
                let mut r = r;
                r.status = lucia_store::RoutineStatus::Active;
                let next =
                    crate::routines::arm(&store, &r, now, ctx.tz).map_err(|e| e.to_string())?;
                Ok(match next {
                    Some(at) => format!(
                        "루틴을 켰습니다. 다음 실행: {}",
                        at.with_timezone(&ctx.tz).format("%m-%d %H:%M")
                    ),
                    None => "루틴을 켰습니다(새 변화가 있을 때 돕니다).".to_owned(),
                })
            })();
            Executed::simple(result, "routine_on", "routine_failed")
        })
    }
    fn closed(&self, approval: &Approval, _why: &str, ctx: &KindCtx) {
        let id = approval.payload["id"].as_str().unwrap_or("");
        let mut store = ctx.store();
        if store
            .routine(id)
            .ok()
            .flatten()
            .is_some_and(|r| r.status == lucia_store::RoutineStatus::Review)
        {
            let _ = store.delete_routine(id);
        }
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "routine_failed",
            "루틴 승인 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다. 관리 페이지에서 루틴 상태를 확인해 주세요.",
        )
    }
}

/// git push 승인(결정 0020).
pub struct GitPushKind {
    pub dev: Option<Arc<crate::dev::DevSandboxes>>,
}

impl ApprovalKind for GitPushKind {
    fn kind(&self) -> &'static str {
        super::KIND_GIT_PUSH
    }
    fn label(&self) -> &'static str {
        "git push"
    }
    fn not_done(&self) -> &'static str {
        "push하지 않았습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        super::push_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        _ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let task = approval.payload["task"].as_str().unwrap_or("").to_owned();
            let result = match &self.dev {
                Some(dev) => {
                    let dev = dev.clone();
                    tokio::task::spawn_blocking(move || dev.push(&task))
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                }
                None => Err("개발 작업이 꺼져 있다".to_owned()),
            };
            Executed::simple(result, "git_push", "git_push_failed")
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "git_push_failed",
            "push 도중 데몬이 멈춤(결과 모름)",
            "⚠️ push 도중 데몬이 멈춰 결과를 모릅니다. 원격 저장소에서 가지를 확인해 주세요.",
        )
    }
}

/// Discord 글 올리기·서버 관리(결정 0024).
pub struct DiscordActionKind {
    pub exec: Option<Arc<dyn super::DiscordActionExec>>,
}

impl ApprovalKind for DiscordActionKind {
    fn kind(&self) -> &'static str {
        crate::discord_tools::KIND_DISCORD_ACTION
    }
    fn label(&self) -> &'static str {
        "Discord 동작"
    }
    fn not_done(&self) -> &'static str {
        "하지 않았습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        super::discord_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        _ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let result = match &self.exec {
                Some(exec) => exec.run_approved(&approval.payload).await,
                None => Err("이 데몬에는 Discord가 연결되어 있지 않다".to_owned()),
            };
            Executed::simple(result, "discord_action", "discord_action_failed")
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "discord_action_failed",
            "Discord 동작 도중 데몬이 멈춤(결과 모름)",
            "⚠️ 실행 도중 데몬이 멈춰 결과를 모릅니다. Discord에서 확인해 주세요.",
        )
    }
}

/// 등록되지 않은 종류(예: 그 기능이 꺼진 채 남은 요청). 실행하지 않는다.
pub struct UnknownKind;

impl ApprovalKind for UnknownKind {
    fn kind(&self) -> &'static str {
        "unknown"
    }
    fn label(&self) -> &'static str {
        "승인 요청"
    }
    fn not_done(&self) -> &'static str {
        "실행하지 않았습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        _tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        let (headline, tone, buttons) = match state {
            CardState::Open => (
                "승인 요청".to_owned(),
                CardTone::Pending,
                vec![lucia_core::CardButton {
                    action: super::action(&approval.id, false),
                    label: "닫기".into(),
                    primary: false,
                    prompt: None,
                    form: None,
                }],
            ),
            CardState::Closed { status, tone } => (status.to_owned(), tone, vec![]),
        };
        Card {
            conversation,
            headline,
            title: "처리할 수 없는 승인 요청".into(),
            fields: vec![],
            body: super::escape_markdown(&approval.summary),
            footer: Some(format!("종류 {} · 요청 {}", approval.kind, approval.id)),
            tone,
            buttons,
        }
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        _ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        let kind = approval.kind.clone();
        Box::pin(async move {
            Executed::simple(
                Err(format!("이 데몬은 {kind} 요청을 처리하지 않는다")),
                "approval_failed",
                "approval_failed",
            )
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "approval_failed",
            "처리 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다.",
        )
    }
}
