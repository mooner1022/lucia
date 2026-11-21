//! 알림이 반응에서 배우기(결정 0034).
//!
//! - **반응 단추**: "지금" 알림(신호 3단계) 뒤에 작은 카드를 올린다: [유용해요] / [브리핑에만] / [필요 없어요].
//!   누른 반응은 보낸 사람별 규칙(`notice_rules`)이 되어 신호 1단계(거르기)와 2단계(분류 입력)에 쓰인다.
//!   개선안은 "나" 기억 항목에 쌓자고 했지만, 키(보낸 사람)와 값이 분명한 규칙이라 전용 표에 두고 관리 페이지에서
//!   보고 지운다. 사람이 누른 것이라 출처는 사람이다.
//! - **단계적 독촉**: 마감이 있는 일(`nudge_create`)을 추적해, 처리되지 않은 채 다가오면 단계를 올린다:
//!   3일 전 소식 → 하루 전 단독 알림 → 3시간 전 [처리했어요]/[아직] 카드 [제안]. 마감이 지나면 닫는다.
//!   처리했다는 것은 사용자가 카드를 누르거나 대화에서 말해 Lucia가 `nudge_done`으로 표시한다.

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use lucia_core::{Card, CardButton, CardField, ConversationId};
use lucia_store::{Approval, Nudge, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::approvals::{ApprovalKind, CardState, Executed, KindCtx, clip_raw, escape_markdown};
use crate::browser::BoxFut;

pub const FEEDBACK_KIND: &str = "notice_feedback";
pub const NUDGE_KIND: &str = "nudge_check";
/// 반응 카드 수명(시간). 누르지 않으면 조용히 닫힌다.
pub const FEEDBACK_HOURS: i64 = 12;

/// 누르지 않아도 되는 카드(반응, 마감 확인, 제안): "곧 만료" 소식과 주간 보고의 만료 카드에 넣지 않는다.
pub fn quiet_expiry(kind: &str) -> bool {
    matches!(
        kind,
        FEEDBACK_KIND | NUDGE_KIND | crate::reports::SUGGESTION_KIND
    )
}

/// 규칙 값.
pub mod rule {
    pub const USEFUL: &str = "useful";
    pub const BRIEFING: &str = "briefing";
    pub const MUTE: &str = "mute";

    pub fn label(r: &str) -> &'static str {
        match r {
            USEFUL => "유용해요",
            BRIEFING => "브리핑에만",
            MUTE => "필요 없어요",
            _ => "?",
        }
    }
}

/// 독촉 단계 시각 [제안].
pub const DIGEST_BEFORE_HOURS: i64 = 72;
pub const ALERT_BEFORE_HOURS: i64 = 24;
pub const CARD_BEFORE_HOURS: i64 = 3;

/// 신호의 규칙 키: 메일은 보낸 사람 주소(소문자), 일정은 `calendar`, 기기는 그 출처.
pub fn sender_key(source: &str, payload: &Value) -> Option<(String, String)> {
    if source == lucia_precheck::google::GMAIL_SOURCE {
        let from = payload.get("from").and_then(Value::as_str)?.trim();
        let addr = match (from.rfind('<'), from.rfind('>')) {
            (Some(a), Some(b)) if b > a => &from[a + 1..b],
            _ => from,
        }
        .trim()
        .to_lowercase();
        if !addr.contains('@') {
            return None;
        }
        let name = from
            .split('<')
            .next()
            .map(|n| n.trim().trim_matches('"'))
            .filter(|n| !n.is_empty())
            .unwrap_or(&addr)
            .chars()
            .take(60)
            .collect();
        Some((addr, name))
    } else if source == lucia_precheck::google::CALENDAR_SOURCE {
        Some(("calendar".into(), "일정 변화".into()))
    } else if source.starts_with(crate::watch::SOURCE_PREFIX) {
        // 게시판 감시(결정 0036): 감시 하나가 보낸 곳 하나.
        let label = payload
            .get("what")
            .and_then(Value::as_str)
            .unwrap_or(source)
            .chars()
            .take(60)
            .collect();
        Some((source.to_owned(), label))
    } else {
        Some((
            source.to_owned(),
            source.trim_start_matches("event:").to_owned(),
        ))
    }
}

/// 2단계 분류 입력에 넣는 규칙(관심이 있는 것만, 50개까지).
pub fn rules_text(store: &Store) -> String {
    store
        .notice_rules()
        .unwrap_or_default()
        .iter()
        .take(50)
        .map(|r| {
            format!(
                "- {} <{}>: {}",
                crate::context::escape(&r.label),
                crate::context::escape(&r.key),
                match r.rule.as_str() {
                    rule::USEFUL => "사용자가 유용하다고 했다(급하면 now)",
                    rule::BRIEFING => "브리핑에만(now로 하지 않는다)",
                    _ => "필요 없다고 했다(drop)",
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 반응 카드를 저장한다(이번에 알린 신호의 보낸 사람들).
pub fn request_feedback(
    store: &Store,
    signals: &[lucia_store::Signal],
    run: Option<&str>,
    now: DateTime<Utc>,
) -> Option<String> {
    let mut senders: Vec<(String, String)> = Vec::new();
    for s in signals {
        if let Some(k) = sender_key(&s.source, &s.payload)
            && !senders.iter().any(|(key, _)| *key == k.0)
        {
            senders.push(k);
        }
    }
    if senders.is_empty() {
        return None;
    }
    let what: Vec<String> = signals
        .iter()
        .map(|s| {
            s.summary
                .trim_start_matches("새 메일: ")
                .chars()
                .take(80)
                .collect()
        })
        .collect();
    let id = uuid::Uuid::new_v4().simple().to_string();
    store
        .create_approval(
            &lucia_store::NewApproval {
                id: id.clone(),
                kind: FEEDBACK_KIND.into(),
                payload: json!({
                    "senders": senders.iter().map(|(k, l)| json!({"key": k, "label": l})).collect::<Vec<_>>(),
                    "what": what,
                }),
                summary: format!(
                    "알림 반응: {}",
                    senders.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join(", ")
                ),
                run_id: run.map(str::to_owned),
                expires_at: now + Duration::hours(FEEDBACK_HOURS),
            },
            now,
        )
        .ok()?;
    Some(id)
}

fn set_rules(ctx: &KindCtx, approval: &Approval, r: &str) -> Result<String, String> {
    let store = ctx.store();
    let now = Utc::now();
    let mut names = Vec::new();
    for s in approval.payload["senders"].as_array().into_iter().flatten() {
        let (Some(key), Some(label)) = (s["key"].as_str(), s["label"].as_str()) else {
            continue;
        };
        store
            .set_notice_rule(key, r, label, now)
            .map_err(|e| e.to_string())?;
        names.push(label.to_owned());
    }
    let _ = store.add_activity(
        "notice_rule",
        &crate::mask::global().mask(&format!(
            "알림 규칙 {}: {}",
            rule::label(r),
            names.join(", ")
        )),
        None,
        now,
    );
    Ok(match r {
        rule::USEFUL => "이런 알림은 앞으로도 바로 알려 드릴게요.".to_owned(),
        rule::BRIEFING => "이 보낸 사람의 알림은 앞으로 아침 브리핑에서만 다룹니다.".to_owned(),
        _ => "이 보낸 사람의 알림은 앞으로 보내지 않습니다(관리 페이지에서 되돌립니다).".to_owned(),
    })
}

/// 반응 카드.
pub struct FeedbackKind;

impl ApprovalKind for FeedbackKind {
    fn kind(&self) -> &'static str {
        FEEDBACK_KIND
    }
    fn label(&self) -> &'static str {
        "알림 반응"
    }
    fn not_done(&self) -> &'static str {
        "이 보낸 사람의 알림은 앞으로 보내지 않습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: Tz,
        state: CardState<'_>,
    ) -> Card {
        let e = escape_markdown;
        let (headline, tone, mut buttons, footer) = crate::approvals::open_or_closed(
            approval,
            tz,
            state,
            "💬 **방금 알림, 이런 건 어떻게 받을까요?**".to_owned(),
            rule::label(rule::USEFUL),
            rule::label(rule::MUTE),
            "그대로 둡니다",
        );
        if !buttons.is_empty() {
            buttons.insert(
                1,
                CardButton {
                    action: crate::approvals::action_always(&approval.id),
                    label: rule::label(rule::BRIEFING).into(),
                    primary: false,
                    prompt: None,
                    form: None,
                },
            );
        }
        let senders: Vec<String> = approval.payload["senders"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s["label"].as_str().map(str::to_owned))
            .collect();
        let what: Vec<String> = approval.payload["what"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(|t| format!("• {}", e(&clip_raw(t, 100)))))
            .collect();
        Card {
            conversation,
            headline,
            title: "알림 반응".into(),
            fields: vec![CardField {
                name: "보낸 곳".into(),
                value: e(&senders.join(", ")),
                inline: false,
            }],
            body: format!(
                "{}\n\n누르지 않아도 됩니다. 누르면 이 보낸 곳의 알림을 앞으로 그렇게 다룹니다(관리 페이지에서 바꿉니다).",
                what.join("\n")
            ),
            footer: Some(footer),
            tone,
            buttons,
        }
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            Executed::simple(
                set_rules(ctx, approval, rule::USEFUL),
                "notice_rule",
                "notice_rule",
            )
        })
    }
    fn execute_always<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            Executed::simple(
                set_rules(ctx, approval, rule::BRIEFING),
                "notice_rule",
                "notice_rule",
            )
        })
    }
    fn closed(&self, approval: &Approval, _why: &str, ctx: &KindCtx) {
        if approval.status == lucia_store::ApprovalStatus::Denied {
            let _ = set_rules(ctx, approval, rule::MUTE);
        }
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "notice_rule",
            "처리 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다.",
        )
    }
}

// ---------- 단계적 독촉 ----------

/// `nudge_create` 인자.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NudgeCreate {
    /// 할 일 한 줄(예: "장학금 신청서 제출").
    pub title: String,
    /// 마감(현지 시각 `YYYY-MM-DD HH:MM`).
    pub due: String,
    /// 어디서 왔는지(출처 메일 제목 등).
    #[serde(default)]
    pub note: Option<String>,
}

/// `nudge_done` 인자.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NudgeDone {
    pub id: String,
}

pub fn create(
    store: &Store,
    args: &NudgeCreate,
    run: Option<&str>,
    goal: Option<&str>,
    now: DateTime<Utc>,
    tz: Tz,
) -> String {
    let title: String = args.title.trim().chars().take(120).collect();
    if title.is_empty() {
        return "만들지 않았다: 할 일이 필요하다.".into();
    }
    let note: String = args
        .note
        .as_deref()
        .unwrap_or("")
        .trim()
        .chars()
        .take(300)
        .collect();
    for t in [&title, &note] {
        if let Some(why) = crate::autonomy::injection(t) {
            return format!("만들지 않았다: {why}가 들어 있다.");
        }
    }
    let due = match crate::autonomy::parse_when(&args.due, tz) {
        Ok(t) if t > now => t,
        Ok(_) => return "만들지 않았다: 마감이 지났다.".into(),
        Err(e) => return format!("만들지 않았다: {e}"),
    };
    if store
        .open_nudges()
        .unwrap_or_default()
        .iter()
        .any(|n| n.title == title && n.due_at == due)
    {
        return "이미 챙기는 마감이다.".into();
    }
    let id = format!("nudge-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    match store.create_nudge(&id, &title, due, &note, goal, run, now) {
        Ok(Ok(())) => {
            let _ = store.add_activity(
                "nudge_create",
                &crate::mask::global().mask(&format!("마감 챙기기 {id}: {title}")),
                run,
                now,
            );
            format!(
                "마감을 챙긴다({id}, {}). 처리되지 않으면 3일 전 소식, 하루 전 알림, 3시간 전 확인 카드로 다시 알린다. 사용자가 처리했다고 하면 nudge_done.",
                due.with_timezone(&tz).format("%m-%d %H:%M")
            )
        }
        Ok(Err(why)) => format!("만들지 않았다: {why}"),
        Err(e) => format!("만들지 못했다: {e}"),
    }
}

pub fn done(store: &Store, id: &str, run: Option<&str>, now: DateTime<Utc>) -> String {
    match store.close_nudge(id.trim(), "done", now) {
        Ok(true) => {
            let _ = store.add_activity(
                "nudge_done",
                &format!("마감 처리함 {}", id.trim()),
                run,
                now,
            );
            format!("{} 처리함으로 표시했다.", id.trim())
        }
        Ok(false) => format!("열린 마감이 아니다: {}", id.trim()),
        Err(e) => format!("표시하지 못했다: {e}"),
    }
}

/// 실행 맥락에 붙는 열린 마감. 없으면 빈 글.
pub fn context_block(store: &Store, tz: Tz) -> String {
    let list = store.open_nudges().unwrap_or_default();
    if list.is_empty() {
        return String::new();
    }
    let mut out = String::from("<deadlines>\n챙기는 마감(사용자가 처리했다고 하면 nudge_done):\n");
    for n in list.iter().take(20) {
        out.push_str(&format!(
            "- {} · {} · 마감 {}\n",
            n.id,
            crate::context::escape(&n.title),
            n.due_at.with_timezone(&tz).format("%m-%d %H:%M")
        ));
    }
    out.push_str("</deadlines>\n");
    out
}

/// 지금 할 독촉 한 단계.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Digest(String),
    Alert(String),
    Card,
    Expire,
}

/// 이 독촉에 지금 할 단계(없으면 `None`). 늦게 만든 것은 낮은 단계를 건너뛴다.
pub fn next_step(n: &Nudge, now: DateTime<Utc>, tz: Tz) -> Option<(i64, Step)> {
    let left = n.due_at - now;
    let when = n.due_at.with_timezone(&tz).format("%m-%d %H:%M");
    if left <= Duration::zero() {
        return Some((4, Step::Expire));
    }
    if n.stage < 3 && left <= Duration::hours(CARD_BEFORE_HOURS) {
        return Some((3, Step::Card));
    }
    if n.stage < 2 && left <= Duration::hours(ALERT_BEFORE_HOURS) {
        return Some((
            2,
            Step::Alert(format!("⏰ 마감이 다가옵니다: {} ({when}까지)", n.title)),
        ));
    }
    if n.stage < 1 && left <= Duration::hours(DIGEST_BEFORE_HOURS) {
        return Some((
            1,
            Step::Digest(format!(
                "⏳ {}일 안 마감: {} ({when})",
                left.num_days().max(1),
                n.title
            )),
        ));
    }
    None
}

pub fn request_nudge_card(store: &Store, n: &Nudge, now: DateTime<Utc>) -> Option<String> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    store
        .create_approval(
            &lucia_store::NewApproval {
                id: id.clone(),
                kind: NUDGE_KIND.into(),
                payload: json!({ "nudge": n.id, "title": n.title, "due": n.due_at, "note": n.note }),
                summary: format!("마감 확인: {}", n.title),
                run_id: None,
                expires_at: n.due_at.max(now + Duration::hours(1)),
            },
            now,
        )
        .ok()?;
    Some(id)
}

/// 마감 확인 카드: [처리했어요] = 닫기, [아직] = 그대로(더 올리지 않는다).
pub struct NudgeKind;

impl ApprovalKind for NudgeKind {
    fn kind(&self) -> &'static str {
        NUDGE_KIND
    }
    fn label(&self) -> &'static str {
        "마감 확인"
    }
    fn not_done(&self) -> &'static str {
        "아직이라고 표시했습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: Tz,
        state: CardState<'_>,
    ) -> Card {
        let e = escape_markdown;
        let p = &approval.payload;
        let due = p["due"]
            .as_str()
            .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
            .map(|d| d.with_timezone(&tz).format("%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let (headline, tone, buttons, footer) = crate::approvals::open_or_closed(
            approval,
            tz,
            state,
            format!(
                "⏰ **곧 마감입니다** {}",
                e(&clip_raw(p["title"].as_str().unwrap_or(""), 100))
            ),
            "처리했어요",
            "아직",
            "마감이 지났습니다",
        );
        Card {
            conversation,
            headline,
            title: "마감 확인".into(),
            fields: vec![CardField {
                name: "마감".into(),
                value: e(&due),
                inline: true,
            }],
            body: e(&clip_raw(p["note"].as_str().unwrap_or(""), 300)),
            footer: Some(footer),
            tone,
            buttons,
        }
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let id = approval.payload["nudge"].as_str().unwrap_or("");
            let text = done(&ctx.store(), id, None, Utc::now());
            Executed::simple(Ok(text), "nudge_done", "nudge_done")
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "nudge_done",
            "처리 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_keys_use_addresses() {
        let p = json!({"from": "\"김 교수\" <Prof@Example.ac.kr>"});
        assert_eq!(
            sender_key("gmail", &p),
            Some(("prof@example.ac.kr".into(), "김 교수".into()))
        );
        assert_eq!(sender_key("gmail", &json!({"from": "알 수 없음"})), None);
        assert_eq!(
            sender_key("google-calendar", &json!({})).unwrap().0,
            "calendar"
        );
    }

    #[test]
    fn nudges_climb_and_skip_missed_stages() {
        let tz = chrono_tz::Asia::Seoul;
        let now = Utc::now();
        let mut n = Nudge {
            id: "n1".into(),
            title: "장학금 신청".into(),
            due_at: now + Duration::hours(60),
            note: String::new(),
            goal: None,
            stage: 0,
            status: "open".into(),
            created_at: now,
        };
        assert!(matches!(next_step(&n, now, tz), Some((1, Step::Digest(_)))));
        n.stage = 1;
        assert_eq!(next_step(&n, now, tz), None);
        n.due_at = now + Duration::hours(20);
        assert!(matches!(next_step(&n, now, tz), Some((2, Step::Alert(_)))));
        n.stage = 0;
        n.due_at = now + Duration::hours(2);
        assert_eq!(
            next_step(&n, now, tz),
            Some((3, Step::Card)),
            "늦게 만든 것은 건너뛴다"
        );
        n.stage = 3;
        assert_eq!(next_step(&n, now, tz), None);
        n.due_at = now - Duration::minutes(1);
        assert_eq!(next_step(&n, now, tz), Some((4, Step::Expire)));
    }
}

#[cfg(test)]
mod kind_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn feedback_buttons_write_rules_per_sender() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let now = Utc::now();
        let signal = lucia_store::Signal {
            id: 1,
            source: "gmail".into(),
            key: "msg:1".into(),
            summary: "새 메일: 뉴스 — 이번 주".into(),
            payload: json!({"from": "Weekly <news@example.com>"}),
            interest: None,
            verdict: Some("now".into()),
            reason: None,
            received_at: now,
            decided_at: None,
            woke_at: None,
        };
        let id = request_feedback(&store.lock().unwrap(), &[signal], None, now).unwrap();
        let approval = store.lock().unwrap().approval(&id).unwrap().unwrap();
        let ctx = KindCtx {
            store: store.clone(),
            tz: chrono_tz::Asia::Seoul,
        };
        let card = FeedbackKind.card(
            &approval,
            ConversationId("dm".into()),
            chrono_tz::Asia::Seoul,
            CardState::Open,
        );
        assert_eq!(card.buttons.len(), 3, "유용해요 / 브리핑에만 / 필요 없어요");
        set_rules(&ctx, &approval, rule::BRIEFING).unwrap();
        assert_eq!(
            store
                .lock()
                .unwrap()
                .notice_rule("news@example.com")
                .unwrap()
                .as_deref(),
            Some(rule::BRIEFING)
        );
    }
}
