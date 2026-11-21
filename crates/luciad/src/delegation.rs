//! 사이트 위임과 사람 없는 실행의 출구(결정 0028·0029·0030).
//!
//! - **누가 부르나**: MCP가 브라우저·코드 도구를 부르기 전에 [`CALLER`]에 실행 ID와 "사람 있음"을 싣는다.
//!   브라우저 도구 안에서 이것을 보고 판단한다. 없으면 데몬 자신의 동작(승인된 동작 실행, 로그인)이다.
//! - **사람 없는 실행**은 사이트 위임이 덮는 호스트로만 이동한다. 밖이면 카드(이번만 / 계속 허용 / 거절)를 올리고
//!   돌아온다. 위임 전에 만든 후속 조치(`delegation.since` 전)는 예전처럼 어디든 연다.
//! - **위임이 덮는 사이트**에서는 다시 묻지 않는다: 로그인을 허용했으면 저장된 로그인으로 다시 로그인하고,
//!   동작을 허용했으면 승인이 필요한 동작도 한다(금지 동작은 그대로 못 한다).
//! - **사람 없는 실행이 올린 카드**는 12시간 살고, 방해 금지 시간에는 끝난 뒤에 올린다. 승인되면 같은 사슬로
//!   이어 하기 후속 조치를 건다(결정 0030).

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use lucia_store::{NewApproval, SiteGrant, Store};
use serde_json::{Value, json};

/// 사이트 위임 제안 카드(대화에서). 승인 = 위임(로그인·동작 포함 여부는 제안대로), 거절·만료 = 하지 않음.
pub const KIND_SITE_GRANT: &str = "site_grant";
/// 사람 없는 실행이 위임 밖 사이트로 가려 할 때. [이번만] = 12시간 그 호스트, [계속 허용] = 읽기 위임.
pub const KIND_SITE_EGRESS: &str = "site_egress";

/// 이 시각 전에 만든 후속 조치는 예전처럼 어디든 연다(마이그레이션 0017이 배포 때 적는다).
pub const SINCE_KEY: &str = "delegation.since";

/// 사람 없는 실행이 올린 카드의 수명 [제안].
pub const UNATTENDED_CARD_HOURS: i64 = 12;
/// [이번만]으로 여는 시간.
pub const ONCE_HOURS: i64 = 12;
/// 하루에 올리는 출구 카드 수 [제안](브리핑이 메일 속 주소로 카드를 쏟지 않게).
pub const EGRESS_CARDS_PER_DAY: usize = 10;

/// 지금 도구를 부른 실행.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub run: String,
    pub human: bool,
}

tokio::task_local! {
    /// MCP 도구 호출 하나 동안의 호출자.
    pub static CALLER: Caller;
}

/// 지금 호출자. 데몬 자신의 동작이면 `None`.
pub fn caller() -> Option<Caller> {
    CALLER.try_with(Clone::clone).ok()
}

/// 이 호출이 위임 범위로 묶이는가: 사람 없는 실행이고, 위임 전에 만든 후속 조치가 아니다.
pub fn bounded(c: &Caller) -> bool {
    !c.human && !crate::runctx::get(&c.run).is_some_and(|x| x.legacy)
}

/// 주소의 호스트(소문자).
pub fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok().and_then(|u| {
        u.host_str()
            .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
    })
}

/// 이 호스트를 덮는 위임(가장 좁은 것). 쓸 수 있는 것만.
pub fn grant_for(store: &Store, host: &str, now: DateTime<Utc>) -> Option<SiteGrant> {
    store
        .site_grants_for(host, now)
        .ok()
        .and_then(|l| l.into_iter().next())
}

/// 사람 없는 실행 카드의 만료와 방해 금지 시간 미루기(결정 0030).
pub fn unattended_card(store: &Store, tz: chrono_tz::Tz, now: DateTime<Utc>) -> DateTime<Utc> {
    let start = crate::autonomy::quiet_hours(store)
        .filter(|q| q.contains(now, tz))
        .and_then(|q| q.ends_after(now, tz))
        .unwrap_or(now);
    start + Duration::hours(UNATTENDED_CARD_HOURS)
}

/// 방해 금지 시간이라 카드를 미뤄야 하는가(카드 내용에 `defer_quiet`가 있을 때).
pub fn defer_now(store: &Store, payload: &Value, tz: chrono_tz::Tz, now: DateTime<Utc>) -> bool {
    payload["defer_quiet"].as_bool() == Some(true)
        && crate::autonomy::quiet_hours(store).is_some_and(|q| q.contains(now, tz))
}

/// 승인 뒤 이어 하기 정보(사람 없는 실행이면). 카드 내용에 넣는다.
pub fn resume_of(run: &str) -> Value {
    crate::runctx::get(run)
        .and_then(|c| c.resume)
        .map(|r| json!(r))
        .unwrap_or(Value::Null)
}

/// 사람 없는 실행이 위임 밖 호스트로 가려 할 때 카드를 올린다. 모델에게 돌려줄 말.
#[allow(clippy::too_many_arguments)]
pub fn request_egress(
    store: &Arc<Mutex<Store>>,
    posts: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
    tz: chrono_tz::Tz,
    host: &str,
    url: &str,
    why: &str,
    run: &str,
) -> String {
    let now = Utc::now();
    let Some(posts) = posts else {
        return format!(
            "{host}는 사용자가 위임한 사이트가 아니라 사람 없는 실행에서는 열지 않는다(승인 경로가 없다). 사용자에게 보고한다."
        );
    };
    let id = uuid::Uuid::new_v4().simple().to_string();
    {
        let s = store.lock().expect("store lock");
        let recent = s.recent_approvals(200).unwrap_or_default();
        let pending_same = recent.iter().any(|a| {
            a.kind == KIND_SITE_EGRESS
                && a.status == lucia_store::ApprovalStatus::Pending
                && a.payload["host"].as_str() == Some(host)
        });
        if pending_same {
            return format!(
                "{host}는 사용자가 위임한 사이트가 아니라 열지 않았다. 이 사이트를 열지 묻는 카드가 이미 사용자에게 가 있다. 같은 요청을 다시 하지 말고, 카드를 기다린다고 보고한다."
            );
        }
        let today = recent
            .iter()
            .filter(|a| a.kind == KIND_SITE_EGRESS && a.requested_at > now - Duration::hours(24))
            .count();
        if today >= EGRESS_CARDS_PER_DAY {
            return format!(
                "{host}는 사용자가 위임한 사이트가 아니라 열지 않았다. 오늘 사이트를 묻는 카드가 이미 많아 더 올리지 않았다. 열지 못했다고 보고한다."
            );
        }
        let created = s.create_approval(
            &NewApproval {
                id: id.clone(),
                kind: KIND_SITE_EGRESS.into(),
                payload: json!({
                    "host": host,
                    "url": url,
                    "reason": why,
                    "run_kind": run_kind(run),
                    "defer_quiet": true,
                    "resume": resume_of(run),
                }),
                summary: format!("사이트 열기 → {host}"),
                run_id: Some(run.to_owned()),
                expires_at: unattended_card(&s, tz, now),
            },
            now,
        );
        if let Err(e) = created {
            return format!(
                "{host}는 사용자가 위임한 사이트가 아니라 열지 않았고, 묻는 카드도 만들지 못했다: {e}"
            );
        }
        let _ = s.add_activity(
            "egress_blocked",
            &format!("사람 없는 실행이 위임 밖 사이트로 가려 함: {host} — 카드로 물음"),
            Some(run),
            now,
        );
    }
    let _ = posts.send(id);
    format!(
        "{host}는 사용자가 위임한 사이트가 아니라 사람 없는 실행에서는 열지 않았다. 열어도 되는지 사용자에게 카드를 올렸다. \
         승인되면 데몬이 이 일을 이어서 다시 맡긴다. 같은 주소를 다시 열지 말고, 카드를 올렸다고 보고한다."
    )
}

/// 실행 ID 앞부분으로 본 실행 종류(카드에 보인다).
pub fn run_kind(run: &str) -> &'static str {
    match run.split('-').next().unwrap_or("") {
        "briefing" => "아침 브리핑",
        "followup" => "후속 조치",
        "routine" => "반복 루틴",
        "task" => "맡긴 작업",
        "chat" => "대화",
        _ => "사람 없는 실행",
    }
}

/// 승인 뒤 같은 사슬로 다시 맡긴다(결정 0030). 이어 하기 정보가 없거나 깊이를 넘으면 하지 않는다.
pub fn resume_after(
    store: &Store,
    payload: &Value,
    done: &str,
    tz: chrono_tz::Tz,
    now: DateTime<Utc>,
) -> Option<String> {
    let r: crate::runctx::Resume = serde_json::from_value(payload["resume"].clone()).ok()?;
    if r.depth >= crate::runctx::RESUME_DEPTH_MAX {
        let _ = store.add_activity(
            "resume_skipped",
            "이어 하기 깊이 상한이라 다시 맡기지 않음",
            None,
            now,
        );
        return None;
    }
    let depth = r.depth + 1;
    let req = crate::autonomy::FollowupRequest {
        when: String::new(),
        task: clip(
            &format!("[이어 하기] {} — 사용자가 승인한 일: {done}", r.task),
            crate::autonomy::TASK_MAX,
        ),
        detail: format!("{}{depth}", crate::runctx::RESUME_DETAIL),
        conversation: r.conversation.clone(),
        run_id: None,
        origin: r.origin.clone(),
    };
    crate::autonomy::create_followup(store, &req, now + Duration::minutes(1), tz, now).ok()
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        s.chars().take(max.saturating_sub(1)).chain(['…']).collect()
    }
}

/// 맥락에 붙이는 위임 목록(모델이 어디를 사람 없이 열 수 있는지 안다).
pub fn context_block(store: &Store, now: DateTime<Utc>) -> String {
    let grants: Vec<SiteGrant> = store
        .site_grants()
        .unwrap_or_default()
        .into_iter()
        .filter(|g| g.active(now) && g.origin != "once")
        .collect();
    if grants.is_empty() {
        return "<site_grants>(없음: 사람 없는 실행이 브라우저로 사이트를 열려 하면 데몬이 사용자에게 카드로 묻는다)</site_grants>\n".into();
    }
    let lines: Vec<String> = grants
        .iter()
        .map(|g| {
            let mut can = vec!["읽기"];
            if g.login {
                can.push("저장된 로그인으로 다시 로그인");
            }
            if g.actions {
                can.push("승인이 필요한 동작");
            }
            format!(
                "- {} (하위 도메인 포함): {} — {}",
                g.host,
                can.join(", "),
                crate::context::escape(&g.purpose)
            )
        })
        .collect();
    format!(
        "<site_grants>\n사용자가 위임한 사이트(사람 없는 실행도 다시 묻지 않고 쓴다):\n{}\n</site_grants>\n",
        lines.join("\n")
    )
}

// ---------- 승인 종류 ----------

use crate::approvals::{ApprovalKind, CardState, Executed, KindCtx, clip_raw, escape_markdown};
use crate::browser::BoxFut;
use lucia_core::{Card, CardButton, CardField, CardTone, ConversationId};
use lucia_store::{Approval, NewSiteGrant};

fn bool_of(p: &Value, k: &str) -> bool {
    p[k].as_bool().unwrap_or(false)
}

/// 위임 범위를 말로(카드·관리 페이지).
pub fn scope_text(login: bool, actions: bool) -> String {
    let mut can = vec!["사람 없이 열고 읽기"];
    if login {
        can.push("저장된 로그인으로 묻지 않고 다시 로그인");
    }
    if actions {
        can.push("승인이 필요한 브라우저 동작(폼 제출 등)도 묻지 않고 하기");
    }
    format!(
        "{}. 결제·주문·송금·탈퇴·비밀번호 바꾸기·로그아웃·파일 올리기는 그래도 하지 않습니다.",
        can.join(", ")
    )
}

fn grant(
    ctx: &KindCtx,
    host: &str,
    login: bool,
    actions: bool,
    purpose: &str,
    origin: &str,
    expires: Option<DateTime<Utc>>,
) -> Result<String, String> {
    let now = Utc::now();
    let id = format!("grant-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    ctx.store()
        .grant_site(
            &NewSiteGrant {
                id: &id,
                host,
                login,
                actions,
                purpose,
                origin,
                expires_at: expires,
            },
            now,
        )
        .map_err(|e| e.to_string())
}

fn executed(result: Result<String, String>, ok_text: String) -> Executed {
    match result {
        Ok(_) => Executed {
            ok: true,
            activity: "site_granted",
            status: format!("✅ {ok_text}"),
            record: ok_text,
            tone: CardTone::Success,
        },
        Err(why) => Executed {
            ok: false,
            activity: "site_grant_failed",
            status: format!("⚠️ 하지 못했습니다: {why}"),
            record: why,
            tone: CardTone::Danger,
        },
    }
}

/// 대화에서 제안한 사이트 위임(결정 0028).
pub struct SiteGrantKind;

impl ApprovalKind for SiteGrantKind {
    fn kind(&self) -> &'static str {
        KIND_SITE_GRANT
    }
    fn label(&self) -> &'static str {
        "사이트 위임"
    }
    fn not_done(&self) -> &'static str {
        "위임하지 않았습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        let p = &approval.payload;
        let host = p["host"].as_str().unwrap_or("");
        let e = escape_markdown;
        let (headline, tone, buttons, footer) = crate::approvals::open_or_closed(
            approval,
            tz,
            state,
            format!("🌐 **이 사이트를 맡겨도 될까요?** {}", e(host)),
            "위임하기",
            "하지 않기",
            "위임하지 않고 닫습니다",
        );
        Card {
            conversation,
            headline,
            title: "사이트 위임".into(),
            fields: vec![
                CardField {
                    name: "사이트".into(),
                    value: format!("{} (하위 도메인 포함)", e(host)),
                    inline: true,
                },
                CardField {
                    name: "쓰는 곳".into(),
                    value: e(&clip_raw(p["purpose"].as_str().unwrap_or(""), 200)),
                    inline: false,
                },
            ],
            body: format!(
                "위임하면 Lucia가 사람 없이 도는 일(후속 조치·루틴·브리핑·작업)에서도 이 사이트를 다시 묻지 않고 씁니다: {}\n\
                 관리 페이지 브라우저 화면 \"사이트 위임\"에서 언제든 거둘 수 있습니다.",
                scope_text(bool_of(p, "login"), bool_of(p, "actions"))
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
            let p = &approval.payload;
            let host = p["host"].as_str().unwrap_or("");
            let (login, actions) = (bool_of(p, "login"), bool_of(p, "actions"));
            executed(
                grant(
                    ctx,
                    host,
                    login,
                    actions,
                    p["purpose"].as_str().unwrap_or(""),
                    "chat",
                    None,
                ),
                format!("{host}를 위임했습니다: {}", scope_text(login, actions)),
            )
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "site_grant_failed",
            "위임하는 도중 데몬이 멈췄다",
            "⚠️ 위임하는 도중 멈췄습니다. 관리 페이지에서 확인해 주세요.",
        )
    }
}

/// 사람 없는 실행이 위임 밖 사이트로 가려 할 때(결정 0029).
pub struct SiteEgressKind;

impl ApprovalKind for SiteEgressKind {
    fn kind(&self) -> &'static str {
        KIND_SITE_EGRESS
    }
    fn label(&self) -> &'static str {
        "사이트 열기"
    }
    fn not_done(&self) -> &'static str {
        "열지 않았습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> Card {
        let p = &approval.payload;
        let host = p["host"].as_str().unwrap_or("");
        let e = escape_markdown;
        let (headline, tone, mut buttons, footer) = crate::approvals::open_or_closed(
            approval,
            tz,
            state,
            format!(
                "🌐 **{}이(가) 이 사이트를 열려고 합니다** {}",
                p["run_kind"].as_str().unwrap_or("사람 없는 실행"),
                e(host)
            ),
            "이번만 열기",
            "거절",
            "열지 않고 닫습니다",
        );
        if !buttons.is_empty() {
            buttons.insert(
                1,
                CardButton {
                    action: crate::approvals::action_always(&approval.id),
                    label: "이 사이트 계속 허용".into(),
                    primary: false,
                    prompt: None,
                    form: None,
                },
            );
        }
        let mut fields = vec![
            CardField {
                name: "사이트".into(),
                value: e(host),
                inline: true,
            },
            CardField {
                name: "실행".into(),
                value: e(p["run_kind"].as_str().unwrap_or("")),
                inline: true,
            },
            CardField {
                name: "열려던 주소".into(),
                value: e(&clip_raw(p["url"].as_str().unwrap_or(""), 300)),
                inline: false,
            },
        ];
        let reason = p["reason"].as_str().unwrap_or("");
        if !reason.is_empty() {
            fields.push(CardField {
                name: "까닭".into(),
                value: e(reason),
                inline: false,
            });
        }
        Card {
            conversation,
            headline,
            title: "위임 밖 사이트".into(),
            fields,
            body: format!(
                "사용자가 위임하지 않은 사이트라 사람 없이 도는 실행은 열지 않았습니다. 주소에 개인 정보가 실려 있지 않은지 보고 정해 주세요.\n\
                 • 이번만 열기: {ONCE_HOURS}시간 동안 이 사이트를 열고, 하던 일을 이어서 다시 맡깁니다.\n\
                 • 이 사이트 계속 허용: 앞으로 사람 없이 이 사이트를 읽습니다(로그인·폼 제출은 포함하지 않습니다. 관리 페이지에서 넓힐 수 있습니다). 하던 일도 이어서 맡깁니다.\n\
                 • 거절: 열지 않습니다."
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
            let host = approval.payload["host"].as_str().unwrap_or("");
            executed(
                grant(
                    ctx,
                    host,
                    false,
                    false,
                    "이번만(사람 없는 실행의 카드)",
                    "once",
                    Some(Utc::now() + Duration::hours(ONCE_HOURS)),
                ),
                format!(
                    "{ONCE_HOURS}시간 동안 {host}를 열도록 했습니다. 하던 일을 이어서 맡깁니다."
                ),
            )
        })
    }
    fn execute_always<'a>(
        &'a self,
        approval: &'a Approval,
        ctx: &'a KindCtx,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            let host = approval.payload["host"].as_str().unwrap_or("");
            executed(
                grant(
                    ctx,
                    host,
                    false,
                    false,
                    &format!(
                        "{}에서 열려던 곳(카드로 허용)",
                        approval.payload["run_kind"]
                            .as_str()
                            .unwrap_or("사람 없는 실행")
                    ),
                    "egress",
                    None,
                ),
                format!(
                    "{host}를 앞으로 사람 없이 읽도록 위임했습니다. 하던 일을 이어서 맡깁니다."
                ),
            )
        })
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "site_grant_failed",
            "위임하는 도중 데몬이 멈췄다",
            "⚠️ 처리하는 도중 멈췄습니다. 관리 페이지에서 확인해 주세요.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unattended_cards_live_long_and_wait_out_quiet_hours() {
        let store = Store::open_in_memory().unwrap();
        let tz = chrono_tz::Asia::Seoul;
        let now = chrono::TimeZone::with_ymd_and_hms(&tz, 2026, 11, 18, 2, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(unattended_card(&store, tz, now), now + Duration::hours(12));
        store
            .set_setting(crate::autonomy::keys::QUIET_HOURS, "23:00-07:00", now)
            .unwrap();
        let seven = chrono::TimeZone::with_ymd_and_hms(&tz, 2026, 11, 18, 7, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            unattended_card(&store, tz, now),
            seven + Duration::hours(12)
        );
        let payload = json!({"defer_quiet": true});
        assert!(defer_now(&store, &payload, tz, now));
        assert!(
            !defer_now(&store, &json!({}), tz, now),
            "표시가 없으면 미루지 않는다"
        );
        assert!(!defer_now(
            &store,
            &payload,
            tz,
            seven + Duration::minutes(1)
        ));
    }

    #[test]
    fn resume_keeps_the_chain_and_stops_at_the_depth_cap() {
        let store = Store::open_in_memory().unwrap();
        let tz = chrono_tz::Asia::Seoul;
        let now = Utc::now();
        let payload = json!({"resume": {"task": "LMS 과제 확인", "origin": "background", "conversation": null, "depth": 0}});
        let id = resume_after(&store, &payload, "사이트 열기 허용", tz, now).unwrap();
        let f = store.followup(&id).unwrap().unwrap();
        assert_eq!(f.origin, "background", "브리핑 사슬은 그대로 브리핑 사슬");
        assert!(f.task.contains("LMS 과제 확인") && f.task.contains("사이트 열기 허용"));
        assert_eq!(crate::runctx::depth_of(&f.detail), 1);
        let deep = json!({"resume": {"task": "x", "origin": "followup", "conversation": null, "depth": crate::runctx::RESUME_DEPTH_MAX}});
        assert!(resume_after(&store, &deep, "y", tz, now).is_none());
        assert!(resume_after(&store, &json!({}), "y", tz, now).is_none());
    }
}
