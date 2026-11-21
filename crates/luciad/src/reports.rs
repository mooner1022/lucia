//! 정기 보고(결정 0035): 주간 자기 보고, 저녁 미리보기, 제안 카드.
//!
//! - **주간 자기 보고**(일요일 19:00 [제안]): 데몬이 한 주의 숫자(목표 진행, 사람 없이 한 외부 쓰기, 막힘, 만료된
//!   카드, 실패·멈춘 루틴, 신호, 모델별 비용)를 모으고, Lucia가 정리한다. 제안은 실행하지 않고 카드로만 올린다.
//!   [진행]을 누르면 카드에 보인 문장을 사용자가 말한 것처럼 대화에 넣는다(보이는 것 = 실행하는 것).
//! - **저녁 미리보기**(21:00 [제안]): 내일 일정, 마감, 목표의 다음 단계. 아침 브리핑은 그것을 되풀이하지 않는다.

use std::fmt::Write as _;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use lucia_core::{Card, CardField, ConversationId, Inbound};
use lucia_store::{Approval, ApprovalStatus, Store};
use serde_json::json;

use crate::approvals::{ApprovalKind, CardState, Executed, KindCtx, clip_raw, escape_markdown};
use crate::browser::BoxFut;

pub const WEEKLY_ID: &str = "weekly-review";
pub const WEEKLY_CRON: &str = "0 19 * * 0";
pub const WEEKLY_ACTION: &str = "weekly_review";
pub const EVENING_ID: &str = "evening-preview";
pub const EVENING_CRON: &str = "0 21 * * *";
pub const EVENING_ACTION: &str = "evening_preview";
/// 관리 페이지에서 끈다(`off`).
pub const WEEKLY_KEY: &str = "reports.weekly";
pub const EVENING_KEY: &str = "reports.evening";
/// 지난 저녁 미리보기(아침 브리핑이 본다): `{"at": …, "text": …}`.
pub const EVENING_LAST_KEY: &str = "reports.evening_last";
/// 아침 브리핑이 저녁 미리보기를 이만큼 안이면 본다(시간).
pub const EVENING_FRESH_HOURS: i64 = 14;

pub const SUGGESTION_KIND: &str = "suggestion";
pub const SUGGESTIONS_MAX: usize = 5;
pub const SUGGESTION_DAYS: i64 = 7;

pub fn enabled(store: &Store, key: &str) -> bool {
    store.setting(key).ok().flatten().as_deref() != Some("off")
}

fn local(t: DateTime<Utc>, tz: Tz) -> String {
    t.with_timezone(&tz).format("%m-%d %H:%M").to_string()
}

/// 한 주의 숫자(모델 입력). 데몬 기록에서 모은다.
pub fn weekly_stats(store: &Store, now: DateTime<Utc>, tz: Tz) -> String {
    let since = now - Duration::days(7);
    let e = crate::context::escape;
    let mut out = String::from("<weekly_stats>\n");
    let _ = writeln!(out, "기간: {} ~ {}", local(since, tz), local(now, tz));

    out.push_str("## 목표\n");
    let goals = store.goals(true).unwrap_or_default();
    let goals: Vec<_> = goals
        .iter()
        .filter(|g| g.status.open() || g.updated_at >= since)
        .collect();
    if goals.is_empty() {
        out.push_str("- (없음)\n");
    }
    for g in goals {
        let reviews = store.goal_reviews_since(&g.id, since).unwrap_or(0);
        let last = store
            .goal_log(&g.id, 1)
            .ok()
            .and_then(|l| l.into_iter().next())
            .map(|l| l.text)
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "- {} · {} · 이번 주 점검 {reviews}번 · 최근: {}",
            e(&g.title),
            g.status.label(),
            e(&last)
        );
    }

    let activity: Vec<_> = store
        .recent_activity(2000)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.at >= since)
        .collect();
    let count = |kind: &str| activity.iter().filter(|a| a.kind == kind).count();
    out.push_str("## 한 일(데몬 기록)\n");
    let _ = writeln!(
        out,
        "- 승인 카드로 한 외부 쓰기: 메일 {}건, 기타 승인 {}건",
        count("mail_sent"),
        activity
            .iter()
            .filter(|a| a.kind.ends_with("_approved") || a.kind == "approval_approved")
            .count()
    );
    let _ = writeln!(
        out,
        "- 묻지 않고 한 일: 위임으로 한 브라우저 동작 {}건, 위임으로 한 로그인 {}건, 캘린더 쓰기 {}건",
        count("browser_delegated"),
        count("login_delegated"),
        count("calendar_write")
    );
    let _ = writeln!(
        out,
        "- 후속 조치 {}건 함, 목표 진행 기록 {}건, 마감 처리 {}건",
        count("followup_done"),
        count("goal_update"),
        count("nudge_done")
    );
    let signals = store.signal_counts_since(since).unwrap_or_default();
    if !signals.is_empty() {
        let _ = writeln!(
            out,
            "- 새 메일·일정 신호: {}",
            signals
                .iter()
                .map(|(v, n)| format!("{v} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    out.push_str("## 못 한 일\n");
    let blocked: Vec<_> = activity.iter().filter(|a| a.kind == "blocked").collect();
    for a in blocked.iter().take(15) {
        let _ = writeln!(out, "- 막힘 {}: {}", local(a.at, tz), e(&a.summary));
    }
    let _ = writeln!(
        out,
        "- 위임 밖 사이트로 막힌 시도 {}건, 출구 검사에 걸린 것 {}건, 마감 지남 {}건",
        count("egress_blocked"),
        count("egress_flag"),
        count("nudge_expired")
    );
    let expired: Vec<Approval> = store
        .recent_approvals(300)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| {
            a.status == ApprovalStatus::Expired
                && a.expires_at >= since
                && !crate::attention::quiet_expiry(&a.kind)
        })
        .collect();
    for a in expired.iter().take(10) {
        let _ = writeln!(out, "- 만료된 카드: {}", e(&a.summary));
    }
    for r in store.routines().unwrap_or_default() {
        if r.failures > 0 || r.status == lucia_store::RoutineStatus::Paused {
            let _ = writeln!(
                out,
                "- 루틴 {} \u{201c}{}\u{201d}: {} · 이어 실패 {}번",
                r.id,
                e(&r.name),
                r.status.label(),
                r.failures
            );
        }
    }
    if blocked.is_empty() && expired.is_empty() {
        out.push_str("- 막힘·만료 카드 없음\n");
    }

    out.push_str("## 비용(API 환산)\n");
    let total = store.spend_between(since, now).ok();
    if let Some(t) = &total {
        let _ = writeln!(
            out,
            "- 합계 ${:.2} · 실행 {} · 모델 실행 {}",
            t.cost_usd, t.runs, t.model_runs
        );
    }
    for (purpose, model, runs, cost) in store
        .spend_by_purpose(since, now)
        .unwrap_or_default()
        .iter()
        .take(10)
    {
        let _ = writeln!(out, "- {purpose} ({model}): {runs}번 ${cost:.2}");
    }
    out.push_str("</weekly_stats>\n");
    out
}

/// 보고에서 "제안:" 줄을 떼어 낸다(본문, 제안들).
pub fn split_suggestions(text: &str) -> (String, Vec<String>) {
    let mut body = Vec::new();
    let mut found = Vec::new();
    for line in text.lines() {
        let t = line.trim().trim_start_matches(['-', '*', '•', ' ']);
        if let Some(rest) = t.strip_prefix("제안:").or_else(|| t.strip_prefix("제안 :")) {
            let s: String = rest.trim().chars().take(300).collect();
            if !s.is_empty() && found.len() < SUGGESTIONS_MAX {
                found.push(s);
            }
            continue;
        }
        body.push(line);
    }
    (body.join("\n").trim().to_owned(), found)
}

pub fn request_suggestion(
    store: &Store,
    text: &str,
    run: Option<&str>,
    now: DateTime<Utc>,
) -> Option<String> {
    if crate::autonomy::injection(text).is_some() {
        return None;
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    store
        .create_approval(
            &lucia_store::NewApproval {
                id: id.clone(),
                kind: SUGGESTION_KIND.into(),
                payload: json!({ "text": text }),
                summary: format!("제안: {}", text.chars().take(100).collect::<String>()),
                run_id: run.map(str::to_owned),
                expires_at: now + Duration::days(SUGGESTION_DAYS),
            },
            now,
        )
        .ok()?;
    Some(id)
}

/// 제안을 사용자의 말로 대화에 넣는다(데몬이 정한 소유자·알림 대화로).
pub trait SuggestionExec: Send + Sync + 'static {
    fn ask<'a>(&'a self, text: &'a str) -> BoxFut<'a, Result<(), String>>;
}

/// 데몬의 대화 입구로 넣는다(Discord 소유자의 말과 같은 길, 소유자 확인을 그대로 거친다).
pub struct InboundSuggestions {
    pub inbound: tokio::sync::mpsc::Sender<Inbound>,
    pub store: std::sync::Arc<std::sync::Mutex<Store>>,
    pub notify: Option<ConversationId>,
}

impl SuggestionExec for InboundSuggestions {
    fn ask<'a>(&'a self, text: &'a str) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            let (owner, conversation) = {
                let store = self
                    .store
                    .lock()
                    .map_err(|_| "저장소를 쓸 수 없다".to_owned())?;
                (
                    store
                        .setting(lucia_store::keys::OWNER_DISCORD_USER_ID)
                        .ok()
                        .flatten(),
                    // Discord 소유자의 말로 넣으므로 Discord 알림 대화(앱이 아니라).
                    store
                        .setting(lucia_store::keys::NOTIFY_CONVERSATION)
                        .ok()
                        .flatten()
                        .map(ConversationId)
                        .or_else(|| self.notify.clone()),
                )
            };
            let (Some(owner), Some(conversation)) = (owner, conversation) else {
                return Err("소유자나 알림 대화가 정해지지 않았다".into());
            };
            self.inbound
                .send(Inbound {
                    platform: lucia_channel::discord::PLATFORM.into(),
                    sender: owner,
                    conversation,
                    body: text.to_owned(),
                    attachments: Vec::new(),
                    received_at: Utc::now(),
                    reply_to: None,
                    venue: None,
                })
                .await
                .map_err(|_| "데몬이 멈추는 중".to_owned())
        })
    }
}

/// 제안 카드: [진행] = 보인 문장을 사용자의 말로 대화에 넣는다, [넘기기] = 아무것도 하지 않는다.
pub struct SuggestionKind {
    pub exec: Option<std::sync::Arc<dyn SuggestionExec>>,
}

impl ApprovalKind for SuggestionKind {
    fn kind(&self) -> &'static str {
        SUGGESTION_KIND
    }
    fn label(&self) -> &'static str {
        "제안"
    }
    fn not_done(&self) -> &'static str {
        "넘겼습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: Tz,
        state: CardState<'_>,
    ) -> Card {
        let e = escape_markdown;
        let text = approval.payload["text"].as_str().unwrap_or("");
        let (headline, tone, buttons, footer) = crate::approvals::open_or_closed(
            approval,
            tz,
            state,
            "💡 **주간 보고의 제안**".to_owned(),
            "진행",
            "넘기기",
            "넘겼습니다",
        );
        Card {
            conversation,
            headline,
            title: "제안".into(),
            fields: vec![CardField {
                name: "Lucia에게 보낼 말".into(),
                value: e(&clip_raw(text, 400)),
                inline: false,
            }],
            body: "[진행]을 누르면 위 문장을 내가 말한 것처럼 Lucia에게 보냅니다(그 뒤는 평소 대화와 같습니다).".into(),
            footer: Some(footer),
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
        Box::pin(async move {
            let text = approval.payload["text"].as_str().unwrap_or("").to_owned();
            let result = match &self.exec {
                Some(exec) => exec
                    .ask(&text)
                    .await
                    .map(|_| "Lucia에게 보냈습니다.".to_owned()),
                None => Err("이 데몬에서는 대화로 넘길 수 없다".into()),
            };
            Executed::simple(result, "suggestion_taken", "suggestion_failed")
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "suggestion_failed",
            "처리 도중 데몬이 멈춤",
            "⚠️ 처리 도중 데몬이 멈췄습니다.",
        )
    }
}

/// 저녁 미리보기를 남긴다(아침 브리핑이 본다).
pub fn remember_evening(store: &Store, text: &str, now: DateTime<Utc>) {
    let clipped: String = text.chars().take(1500).collect();
    let _ = store.set_setting(
        EVENING_LAST_KEY,
        &json!({ "at": now, "text": crate::mask::global().mask(&clipped) }).to_string(),
        now,
    );
}

/// 아침 브리핑에 붙일 지난 저녁 미리보기(14시간 안이면).
pub fn evening_block(store: &Store, now: DateTime<Utc>) -> String {
    let Some(v) = store
        .setting(EVENING_LAST_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return String::new();
    };
    let at = v["at"]
        .as_str()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Utc));
    match at {
        Some(at) if now - at < Duration::hours(EVENING_FRESH_HOURS) => format!(
            "<evening_preview>\n어젯밤 보낸 \"내일 챙길 것\"(되풀이하지 말고, 밤사이 바뀐 것 위주로):\n{}\n</evening_preview>\n",
            crate::context::escape(v["text"].as_str().unwrap_or(""))
        ),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_are_split_out_of_the_report() {
        let (body, s) = split_suggestions(
            "이번 주 요약입니다.\n- 제안: 과제 메일 루틴을 하루 한 번으로 줄여 줘\n제안: 장학 공지 감시를 만들어 줘\n끝.",
        );
        assert_eq!(body, "이번 주 요약입니다.\n끝.");
        assert_eq!(
            s,
            vec![
                "과제 메일 루틴을 하루 한 번으로 줄여 줘",
                "장학 공지 감시를 만들어 줘"
            ]
        );
    }

    #[test]
    fn evening_preview_is_seen_by_the_next_morning_only() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        remember_evening(&store, "내일 9시 면담", now);
        assert!(evening_block(&store, now + Duration::hours(10)).contains("내일 9시 면담"));
        assert!(evening_block(&store, now + Duration::hours(20)).is_empty());
    }

    #[test]
    fn weekly_stats_gather_blocks_and_costs() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        store
            .add_activity(
                "blocked",
                "🚧 못 한 일: LMS 확인 · 까닭: 위임 없음",
                None,
                now,
            )
            .unwrap();
        let s = weekly_stats(&store, now + Duration::minutes(1), chrono_tz::Asia::Seoul);
        assert!(s.contains("LMS 확인"));
        assert!(s.contains("## 비용"));
    }
}
