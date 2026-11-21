//! 화면이 스스로 계산하지 않도록 데몬이 붙여 주는 값(결정 0061). 예전에는 서버에서 HTML을 그리는 화면 코드가 데몬의
//! 함수를 불러 이름·결·설명을 만들었다. 화면이 브라우저로 옮겨 갔으므로, 그 값은 JSON에 실어 보낸다.
//!
//! - **항목마다 `ui`**: 직렬화한 스냅숏의 항목(과 `extra`의 항목)에 `ui` 칸을 붙인다. 이름표, 결(tone), 불리언 같은 것.
//! - **스냅숏 전체의 `ui`**: 여러 칸을 합쳐야 나오는 값(`snapshot.ui`).
//! - **상수 표(`consts`)**: 데이터와 상관없는 이름표·한도·목록. 표의 `key`는 항목의 `ui`가 싣는 같은 이름의 `*_key`와
//!   맞춘다(열거형을 그대로 직렬화하면 `"Strict"`처럼 대문자로 시작해 표의 `as_str()` 값과 다르다).
//!
//! 무엇이 어디에 실리는지는 `web/admin/docs/derived.md`에 있다. 예전 화면 코드에만 있던 표(활동 아이콘,
//! 위임 출처 이름 …)는 이 파일로 옮겼고, 그 코드는 지웠다.

use lucia_store::{ApprovalStatus, FollowupStatus, MemoryAction, MemoryAuthor, WatchStatus};
use serde::Serialize;
use serde_json::{Value, json};

use super::snapshot::{self, ActivityFilter, GoogleView, Page, Snapshot, Tone};
use crate::gateway::Role;

fn set_ui(item: &mut Value, ui: Value) {
    if let Some(map) = item.as_object_mut() {
        map.insert("ui".into(), ui);
    }
}

/// `path`(JSON 포인터)의 배열 항목마다 `ui`를 붙인다. `typed`는 같은 순서의 원래 값이다.
fn each<T>(snap: &mut Value, path: &str, typed: &[T], ui: impl Fn(&T) -> Value) {
    if let Some(list) = snap.pointer_mut(path).and_then(Value::as_array_mut) {
        for (item, t) in list.iter_mut().zip(typed) {
            set_ui(item, ui(t));
        }
    }
}

/// `[[A, B], …]` 짝 배열의 첫 요소(A)에 `ui`를 붙인다.
fn each_first<A, B>(snap: &mut Value, path: &str, typed: &[(A, B)], ui: impl Fn(&A) -> Value) {
    if let Some(list) = snap.pointer_mut(path).and_then(Value::as_array_mut) {
        for (pair, (a, _)) in list.iter_mut().zip(typed) {
            if let Some(first) = pair.get_mut(0) {
                set_ui(first, ui(a));
            }
        }
    }
}

/// 값을 JSON으로 만들고 `ui`를 붙여 돌려준다(`extra`의 항목들이 쓴다).
pub(super) fn with_ui<T: Serialize>(item: &T, ui: Value) -> Value {
    let mut value = serde_json::to_value(item).unwrap_or(Value::Null);
    set_ui(&mut value, ui);
    value
}

/// 직렬화한 스냅숏의 항목마다 `ui`를 붙이고, 스냅숏 전체의 `ui`도 단다.
pub fn decorate(snap: &mut Value, s: &Snapshot) {
    decorate_status(snap, s);
    decorate_work(snap, s);
    decorate_automation(snap, s);
    decorate_lucia(snap, s);
    decorate_connections(snap, s);
    decorate_browser(snap, s);
    decorate_settings(snap, s);
    super::setup::decorate(snap, s);
    if let Some(map) = snap.as_object_mut() {
        map.insert("ui".into(), whole(s));
    }
}

/// 상태와 기록: 활동, 정기 루틴, 승인, 상주 세션, 백업, Discord, Google.
fn decorate_status(snap: &mut Value, s: &Snapshot) {
    each(snap, "/activity", &s.activity, |a| {
        let groups: Vec<&str> = ActivityFilter::ALL
            .iter()
            .filter(|f| f.matches(&a.kind))
            .map(|f| f.key())
            .collect();
        json!({
            "label": snapshot::activity_label(&a.kind),
            "tone": snapshot::activity_tone(&a.kind),
            "groups": groups,
            "icon": activity_icon(&a.kind),
            "detail": activity_detail(&a.kind, &a.summary),
        })
    });
    each(snap, "/schedules", &s.schedules, |sch| {
        json!({
            "label": snapshot::routine_label(&sch.id),
            "cron_text": snapshot::cron_text(&sch.spec),
        })
    });
    each(snap, "/approvals", &s.approvals, |a| {
        let (label, tone) = snapshot::approval_status(a.status);
        json!({
            "status": label,
            "tone": tone,
            "pending": a.status == ApprovalStatus::Pending,
        })
    });
    if let (Some(session), Some(row)) = (snap.get_mut("session"), &s.session) {
        // 상주 세션 ID는 앞 8자만 보인다(글자 단위로 자른다).
        let short_id: String = row.id.chars().take(8).collect();
        set_ui(session, json!({ "short_id": short_id }));
    }
    if let Some(backup) = snap.get_mut("backup") {
        set_ui(
            backup,
            json!({ "cron_text": snapshot::cron_text(&s.backup.cron) }),
        );
    }
    if let Some(discord) = snap.get_mut("discord") {
        set_ui(discord, discord_ui(s));
    }
    if let Some(google) = snap.get_mut("google") {
        set_ui(google, google_ui(&s.google));
    }
}

fn discord_ui(s: &Snapshot) -> Value {
    use crate::discord_ctl::DiscordState;
    let (label, tone) = snapshot::discord_status(&s.discord.state);
    let (state, reason) = match &s.discord.state {
        DiscordState::NotConfigured => ("not_configured", None),
        DiscordState::Connecting { .. } => ("connecting", None),
        DiscordState::Connected { .. } => ("connected", None),
        DiscordState::Stopped { reason } => ("stopped", Some(reason.as_str())),
    };
    json!({
        "status": label,
        "tone": tone,
        "bot": snapshot::discord_bot(&s.discord.state),
        "state": state,
        "stopped_reason": reason,
    })
}

fn google_ui(g: &GoogleView) -> Value {
    let (status, tone) = google_status(g);
    let scopes: Vec<Value> = g
        .scope_list()
        .into_iter()
        .map(|(label, granted)| json!({ "label": label, "granted": granted }))
        .collect();
    json!({
        "status": status,
        "tone": tone,
        "scopes": scopes,
        "has_gmail_send": g.has_scope(crate::google_auth::SCOPE_GMAIL_SEND),
        "has_calendar_write": g.has_scope(crate::google_auth::SCOPE_CALENDAR_WRITE),
    })
}

/// Google 연결 상태: (글, 톤). 홈의 타일과 연결 화면이 같은 말을 쓴다(개편 전 연결 화면의 `google_status`를 옮겼다).
pub(super) fn google_status(g: &GoogleView) -> (&'static str, Tone) {
    let source_error = g
        .sources
        .iter()
        .any(|src| src.state.as_ref().is_some_and(|st| st.last_error.is_some()));
    if g.connected && source_error {
        ("확인 오류", Tone::Bad)
    } else if g.connected && !g.has_scope(crate::google_auth::SCOPE_GMAIL_SEND) {
        ("읽기만", Tone::Warn)
    } else if g.connected {
        ("연결됨", Tone::Ok)
    } else if g.has_client {
        ("동의 전", Tone::Warn)
    } else {
        ("연결 안 됨", Tone::Muted)
    }
}

/// 활동 종류의 아이콘 이름(개편 전 화면 부품의 `activity_icon`을 옮겼다). 모르는 종류는 `activity`.
fn activity_icon(kind: &str) -> &'static str {
    match kind {
        "mail_search" => "search",
        "mail_read" | "mail_aliases" => "mail",
        "calendar_read" => "calendar",
        "mail_sent" | "approval_requested" => "send",
        "send_failed" | "approval_failed" => "x-circle",
        "approval_denied" | "approval_expired" => "shield",
        "memory_write" | "memory_delete" | "memory_tell" | "memory_restore" => "memory",
        "character_edit" => "chat",
        "followup_create" | "followup_cancel" | "followup_done" | "promise_check" => "clock",
        "write_held" | "write_applied" | "write_failed" => "shield",
        "web_search" => "search",
        "browser_open" | "browser_act" => "link",
        "browser_approval" | "browser_refused" | "browser_failed" => "shield",
        "action_requested" | "action_approved" | "action_denied" => "shield",
        "shell_net" => "link",
        "login_requested" | "login_ok" | "login_failed" | "login_input" => "shield",
        "file_create" | "file_sent" | "file_download" => "upload",
        "expression_add" | "expression_remove" | "expression_edit" => "smile",
        "image_generate" | "image_admin" => "spark",
        _ => "activity",
    }
}

/// 활동 요약이 종류 이름으로 시작하면 그 부분을 뺀다(제목과 겹치지 않게. 개편 전 화면 부품의 `activity_detail`을 옮겼다).
fn activity_detail<'a>(kind: &str, summary: &'a str) -> &'a str {
    let label = snapshot::activity_label(kind);
    match summary.strip_prefix(label) {
        Some(rest) if !rest.trim().is_empty() => rest.trim_start_matches([':', ' ']),
        _ => summary,
    }
}

/// 할 일: 목표, 챙기는 마감, 작업, 후속 조치.
fn decorate_work(snap: &mut Value, s: &Snapshot) {
    each(snap, "/goals", &s.goals, |v| {
        json!({
            "open": v.goal.status.open(),
            "status_label": v.goal.status.label(),
        })
    });
    each(
        snap,
        "/nudges",
        &s.nudges,
        |n| json!({ "stage_label": nudge_stage_label(n.stage) }),
    );
    for (path, list) in [
        ("/tasks_open", &s.tasks_open),
        ("/tasks_closed", &s.tasks_closed),
    ] {
        each(snap, path, list, |t| {
            let tone = if t.status == lucia_core::TaskStatus::Done {
                Tone::Ok
            } else {
                Tone::Warn
            };
            json!({ "status_label": t.status.label(), "tone": tone })
        });
    }
    for (path, list) in [
        ("/followups_open", &s.followups_open),
        ("/followups_closed", &s.followups_closed),
    ] {
        each(snap, path, list, |f| {
            let tone = match f.status {
                FollowupStatus::Failed => Tone::Bad,
                FollowupStatus::Done => Tone::Ok,
                _ => Tone::Muted,
            };
            json!({
                "status_label": f.status.label(),
                "tone": tone,
                "uses_browser": crate::autonomy::followup_uses_browser(&f.origin),
                "origin_label": followup_origin_label(&f.origin),
            })
        });
    }
}

/// 챙기는 마감이 어디까지 알렸나(개편 전 할 일 화면에서 옮겼다).
fn nudge_stage_label(stage: i64) -> &'static str {
    match stage {
        0 => "아직 알리지 않음",
        1 => "소식 보냄",
        2 => "알림 보냄",
        _ => "확인 카드 보냄",
    }
}

/// 후속 조치를 누가 어떻게 등록했나(결정 0011·0013: 채팅에서 시작한 것과 사용자가 허용한 것만 브라우저를 쓴다).
fn followup_origin_label(origin: &str) -> &'static str {
    if origin == crate::autonomy::ORIGIN_USER {
        "브라우저 허용함"
    } else if crate::autonomy::followup_uses_browser(origin) {
        "대화에서 등록(브라우저 사용)"
    } else {
        "브리핑에서 등록(위임 사이트 읽기만)"
    }
}

/// 자동화: 반복 루틴과 페이지 감시.
fn decorate_automation(snap: &mut Value, s: &Snapshot) {
    use lucia_store::RoutineStatus as R;
    each(snap, "/routines", &s.routines, |v| {
        let r = &v.routine;
        let tone = match r.status {
            R::Active => Tone::Ok,
            R::Review => Tone::Warn,
            _ => Tone::Muted,
        };
        let resume_label = match r.status {
            R::Paused => Some("다시 켜기"),
            R::Review => Some("켜기(승인)"),
            _ => None,
        };
        json!({
            "status_label": r.status.label(),
            "tone": tone,
            "trigger_label": crate::routines::trigger_label(&r.trigger),
            "can_run": r.status == R::Active,
            "can_pause": r.status == R::Active,
            "resume_label": resume_label,
        })
    });
    each_first(snap, "/watches", &s.watches, |w| {
        let tone = match w.status {
            WatchStatus::Active => Tone::Ok,
            WatchStatus::Pending => Tone::Info,
            _ => Tone::Warn,
        };
        json!({
            "status_label": w.status_label(),
            "tone": tone,
            "is_items": w.is_items(),
            "uses_browser": w.fetcher == lucia_store::watch_fetcher::BROWSER,
            "notify_label": notify_label(&w.notify),
            "via_label": crate::watch::Via::parse(&w.fetcher).label(),
            "interval_min": crate::watch::interval_min(w.is_items()),
            "can_pause": w.status == WatchStatus::Active,
            "can_resume": w.status == WatchStatus::Paused,
            "editable": w.status != WatchStatus::Pending,
        })
    });
}

/// 새 글 알림 방식의 이름(개편 전 자동화 화면에서 옮겼다).
fn notify_label(notify: &str) -> &'static str {
    use lucia_store::watch_notify as n;
    match notify {
        n::NOW => "바로 알림",
        n::CLASSIFY => "작은 모델이 골라 알림",
        _ => "묶음 소식만",
    }
}

/// Lucia 자신: 기억 항목(스킬과 캐릭터는 `extra`).
fn decorate_lucia(snap: &mut Value, s: &Snapshot) {
    each(snap, "/memories", &s.memories, memory_entry_ui);
}

/// 기억 항목의 갈래(`section_key`는 `consts.memory.sections[].key`와 같다).
pub(super) fn memory_entry_ui(e: &lucia_store::MemoryEntry) -> Value {
    json!({
        "section_key": e.section.as_str(),
        "section_label": e.section.label(),
    })
}

/// 기억 개정 하나: 쓴 이, 바꾼 실행의 출처, 지운 개정인가.
pub(super) fn memory_revision_ui(r: &lucia_store::MemoryRevision) -> Value {
    let author = match r.author {
        MemoryAuthor::Agent => "Lucia",
        MemoryAuthor::User => "사용자",
        MemoryAuthor::Daemon => "데몬",
    };
    json!({
        "author_label": author,
        "origin": run_origin(r.run_id.as_deref()),
        "deleted": r.action == MemoryAction::Delete,
    })
}

/// 바꾼 실행의 출처(결정 0028): 실행 ID 앞부분으로. 사람이 없을 때 바뀐 것을 가려 보게 한다(개편 전 기억 화면에서 옮겼다).
fn run_origin(run_id: Option<&str>) -> &'static str {
    let Some(run) = run_id else {
        return "관리 페이지·데몬";
    };
    match run.split('-').next().unwrap_or("") {
        "chat" => "대화 중",
        "remember" => "관리 페이지 \u{201c}기억해 줘\u{201d}",
        "briefing" => "아침 브리핑(사람 없음)",
        "followup" => "후속 조치(사람 없음)",
        "routine" => "반복 루틴(사람 없음)",
        "task" => "맡긴 작업(사람 없음)",
        "wrapup" => "하루 정리(사람 없음)",
        "retro" => "회고(사람 없음)",
        _ => "기타 실행",
    }
}

/// 스킬 하나: 상태 이름·결, 쓸 수 있나, 흔적 한 줄, 출처, 상태에 맞는 단추.
pub(super) fn skill_ui(s: &lucia_store::Skill) -> Value {
    use lucia_store::SkillStatus as S;
    let tone = match s.status {
        S::Active => Tone::Ok,
        S::Trial | S::Pending => Tone::Warn,
        _ => Tone::Muted,
    };
    let source = if s.source == "chat" {
        "사용자 요청"
    } else {
        "회고"
    };
    json!({
        "status_label": s.status.label(),
        "tone": tone,
        "usable": s.status.usable(),
        "footprint": crate::skills::footprint_line(&s.footprint),
        "source_label": source,
        "can_approve": s.status == S::Pending,
        "can_restore": matches!(s.status, S::Stale | S::Archived),
    })
}

/// 스킬 개정 하나: 쓴 이.
pub(super) fn skill_revision_ui(r: &lucia_store::SkillRevision) -> Value {
    let author = if r.author == "user" {
        "사용자"
    } else {
        "Lucia"
    };
    json!({ "author_label": author })
}

/// 연결: 접속 API 클라이언트와 이미지 생성 요청.
fn decorate_connections(snap: &mut Value, s: &Snapshot) {
    each_first(snap, "/clients", &s.clients, |c| {
        let scope_labels: Vec<String> = c.scopes.iter().map(|sc| scope_label(sc)).collect();
        json!({
            "role_label": Role::parse(&c.role).map(|r| r.label()).unwrap_or("?"),
            "is_app": c.role == Role::App.as_str(),
            "has_security_mode": c.scopes.iter().any(|x| x == crate::gateway::scopes::SECURITY_MODE),
            "has_app_tools": c.scopes.iter().any(|x| x == crate::gateway::scopes::APP_TOOLS),
            "has_browser_handover": c.scopes.iter().any(|x| x == crate::gateway::scopes::BROWSER_HANDOVER),
            "scope_labels": scope_labels,
            // 푸시 등록 토큰 자체는 내보내지 않는다. 있는지만 알린다("시험 푸시" 단추).
            "has_push_token": c.push_token.is_some(),
        })
    });
    each(snap, "/images/recent", &s.images.recent, |g| {
        let (tone, label) = match g.status.as_str() {
            "done" => (Tone::Ok, "만듦"),
            "running" => (Tone::Info, "만드는 중"),
            _ => (Tone::Bad, "실패"),
        };
        json!({ "label": label, "tone": tone })
    });
}

/// 접속 API 권한 범위의 이름. 모르는 범위는 이름 그대로.
fn scope_label(name: &str) -> String {
    crate::gateway::scopes::ALL
        .iter()
        .find(|(n, _)| *n == name)
        .map_or(name.to_owned(), |(_, l)| (*l).to_owned())
}

/// 브라우저: 사이트 위임. 브라우저 상태 자체에는 붙일 것이 없다.
fn decorate_browser(snap: &mut Value, s: &Snapshot) {
    each(snap, "/site_grants", &s.site_grants, |g| {
        json!({
            "active": g.active(s.now),
            "scope_text": crate::delegation::scope_text(g.login, g.actions),
            "origin_label": grant_origin_label(&g.origin),
        })
    });
}

/// 사이트 위임이 어디서 생겼나(개편 전 브라우저 화면에서 옮겼다).
fn grant_origin_label(origin: &str) -> &'static str {
    match origin {
        "chat" => "대화에서 위임",
        "egress" => "카드에서 계속 허용",
        "admin" => "관리 페이지에서 위임",
        "once" => "이번만",
        "watch" => "로그인 감시 승인",
        _ => "위임",
    }
}

/// 설정과 보관: 알림 규칙, 신호, 보관한 파일, 이모지·스티커.
fn decorate_settings(snap: &mut Value, s: &Snapshot) {
    each(
        snap,
        "/notice_rules",
        &s.notice_rules,
        |r| json!({ "rule_label": crate::attention::rule::label(&r.rule) }),
    );
    each(
        snap,
        "/signals/recent",
        &s.signals.recent,
        |x| json!({ "verdict_label": verdict_label(x.verdict.as_deref()) }),
    );
    each(snap, "/kept_files", &s.kept_files, |k| {
        json!({
            "size_text": crate::files::human_size(k.file.size),
            "kind_label": crate::files::Kind::parse(&k.file.kind).label(),
            "direction_label": crate::files::direction_label(&k.file.direction),
        })
    });
    if let Some(catalog) = &s.expressions {
        for (path, list) in [
            ("/expressions/emoji", &catalog.emoji),
            ("/expressions/stickers", &catalog.stickers),
        ] {
            each(snap, path, list, |e| {
                let origin = if e.origin == "agent" {
                    "Lucia가 등록"
                } else {
                    "관리 페이지에서 등록"
                };
                json!({
                    "size_text": crate::files::human_size(e.file.size),
                    "origin_label": origin,
                })
            });
        }
    }
}

/// 신호를 어떻게 나눴나(개편 전 활동 화면에서 옮겼다). 아직 안 나눴으면 "분류 대기".
fn verdict_label(verdict: Option<&str>) -> &'static str {
    use lucia_store::signal_verdict as v;
    match verdict {
        Some(v::NOW) => "지금 알림",
        Some(v::DIGEST) => "소식",
        Some(v::DROP) => "버림",
        Some(v::BRIEFING) => "브리핑에서 다룸",
        _ => "분류 대기",
    }
}

/// 스냅숏 전체에서 나오는 값(`snapshot.ui`).
fn whole(s: &Snapshot) -> Value {
    let kept_total: u64 = s.kept_files.iter().map(|k| k.file.size).sum();
    // 문자열의 `.len()`은 UTF-8 바이트 수다(브라우저의 글자 수와 다르다).
    let memory_bytes: usize = s
        .memories
        .iter()
        .map(|e| e.title.len() + e.summary.len() + e.body.len())
        .sum();
    let browser_watches = s
        .watches
        .iter()
        .filter(|(w, _)| {
            w.status == WatchStatus::Active && w.fetcher == lucia_store::watch_fetcher::BROWSER
        })
        .count();
    json!({
        "security_mode": s.security_mode.as_str(),
        "kept_files_total_text": crate::files::human_size(kept_total),
        "memory_bytes": memory_bytes,
        "browser_watches": browser_watches,
    })
}

/// 화면이 쓰는 상수와 이름표(데이터와 상관없다).
pub fn consts() -> Value {
    json!({
        "activity_filters": ActivityFilter::ALL
            .iter()
            .map(|f| json!({ "key": f.key(), "label": f.label() }))
            .collect::<Vec<_>>(),
        "pages": pages_table(),
        "settings_tabs": Page::SETTINGS,
        "security": security_table(),
        "watch": watch_table(),
        "routines": {
            "daily_wakes_max": crate::routines::DAILY_WAKES_MAX,
            "daily_wakes_range": [
                crate::routines::DAILY_WAKES_RANGE.start(),
                crate::routines::DAILY_WAKES_RANGE.end(),
            ],
        },
        "gateway": gateway_table(),
        "memory": {
            "sections": lucia_store::MemorySection::ALL
                .iter()
                .map(|m| json!({ "key": m.as_str(), "label": m.label() }))
                .collect::<Vec<_>>(),
        },
        "expressions": expressions_table(),
        "signals": {
            "poll_minutes_range": [
                crate::events::POLL_MINUTES_RANGE.start(),
                crate::events::POLL_MINUTES_RANGE.end(),
            ],
            "daily_range": [
                crate::signals::DAILY_RANGE.start(),
                crate::signals::DAILY_RANGE.end(),
            ],
        },
        // 위쪽 한도 3650은 처리기에 적힌 값이다(상수가 아니다).
        "wrapup": {
            "keep_days_range": [crate::engine::wrapup::KEEP_DAYS_MIN, 3650],
        },
        "images": { "default_daily_usd": crate::imagegen::DEFAULT_DAILY_USD },
        "dev": { "default_cost_cap_usd": crate::dev::DEFAULT_COST_CAP_USD },
        "browser": browser_table(),
        "profile": {
            "default_name": crate::profile::DEFAULT_NAME,
            "name_max_chars": crate::profile::NAME_MAX_CHARS,
            "avatar_rules": crate::profile::rules_text(),
        },
    })
}

/// 화면 목록: 경로와 이름들(`Page`의 메서드). `key`는 응답의 `page`와 같다.
fn pages_table() -> Vec<Value> {
    Page::ALL
        .iter()
        .map(|p| {
            json!({
                "key": p,
                "path": p.path(),
                "label": p.label(),
                "nav_label": p.nav_label(),
                "tab_label": p.tab_label(),
            })
        })
        .collect()
}

/// 권한 모드(결정 0040): 모드마다 이름, 아이콘, 한 줄 설명, 달라지는 것, 해제에서도 그대로 두는 것.
/// 권한 모드의 아이콘 이름(화면의 아이콘 목록 `web/admin/src/ui/icons.ts`의 이름). Discord에서 쓰는 이모지(`Mode::icon`)와 따로다:
/// 이모지는 기기마다 모양이 다르고 글꼴이 없으면 빈 네모로 나온다(결정 0062).
fn mode_icon(mode: crate::security::Mode) -> &'static str {
    use crate::security::Mode;
    match mode {
        Mode::Strict => "lock",
        Mode::Standard => "shield",
        Mode::Open => "unlock",
    }
}

fn security_table() -> Value {
    use crate::security::{Mode, OPEN_ITEMS, OPEN_KEEPS, STRICT_ITEMS};
    let modes: Vec<Value> = Mode::ALL
        .iter()
        .map(|m| {
            let (items, keeps): (&[&str], &[&str]) = match m {
                Mode::Strict => (STRICT_ITEMS, &[]),
                Mode::Standard => (&[], &[]),
                Mode::Open => (OPEN_ITEMS, OPEN_KEEPS),
            };
            json!({
                "key": m.as_str(),
                "label": m.label(),
                "icon": mode_icon(*m),
                "summary": m.summary(),
                "items": items,
                "keeps": keeps,
            })
        })
        .collect();
    json!({ "modes": modes })
}

/// 페이지 감시(결정 0019·0036)의 한도와 선택지.
fn watch_table() -> Value {
    use crate::watch::{self, Via};
    json!({
        "interval_max": watch::INTERVAL_MAX,
        "interval_min": {
            "items": watch::interval_min(true),
            "region": watch::interval_min(false),
        },
        "now_daily_range": [watch::NOW_DAILY_RANGE.start(), watch::NOW_DAILY_RANGE.end()],
        "host_gap_minutes": watch::HOST_GAP_MINUTES,
        "notify": lucia_store::watch_notify::ALL
            .iter()
            .map(|n| json!({ "key": n, "label": notify_label(n) }))
            .collect::<Vec<_>>(),
        "fetchers": lucia_store::watch_fetcher::ALL
            .iter()
            .map(|f| json!({ "key": f, "label": Via::parse(f).label() }))
            .collect::<Vec<_>>(),
        "browser_fetcher": lucia_store::watch_fetcher::BROWSER,
    })
}

/// 접속 API(결정 0021·0043)의 역할과 권한 범위.
fn gateway_table() -> Value {
    let roles: Vec<Value> = [Role::App, Role::Device]
        .iter()
        .map(|r| json!({ "key": r.as_str(), "label": r.label() }))
        .collect();
    json!({
        "roles": roles,
        "app_role": Role::App.as_str(),
        "scopes": crate::gateway::scopes::ALL
            .iter()
            .map(|(key, label)| json!({ "key": key, "label": label }))
            .collect::<Vec<_>>(),
        "scope_security_mode": crate::gateway::scopes::SECURITY_MODE,
        "scope_app_tools": crate::gateway::scopes::APP_TOOLS,
        "scope_browser_handover": crate::gateway::scopes::BROWSER_HANDOVER,
    })
}

/// 이모지·스티커(결정 0045)의 종류와 한도.
fn expressions_table() -> Value {
    use crate::expressions::{
        EMOJI_MAX_BYTES, KEEP_DAYS, Kind, MAX_CATEGORIES, MAX_EMOJI, MAX_STICKERS,
        STICKER_MAX_BYTES,
    };
    json!({
        "kinds": [
            { "key": Kind::Emoji.as_str(), "label": Kind::Emoji.label(), "max_count": MAX_EMOJI, "max_bytes": EMOJI_MAX_BYTES },
            { "key": Kind::Sticker.as_str(), "label": Kind::Sticker.label(), "max_count": MAX_STICKERS, "max_bytes": STICKER_MAX_BYTES },
        ],
        "max_categories": MAX_CATEGORIES,
        "keep_days": KEEP_DAYS,
    })
}

/// 브라우저: 화면 넘겨받기의 길이와 키 단추(결정 0017).
fn browser_table() -> Value {
    // 키 이름은 브라우저 드라이버가 받는 값, 글은 단추에 적는 말이다(개편 전 브라우저 화면에서 옮겼다).
    let keys: Vec<Value> = [
        ("enter", "Enter"),
        ("tab", "Tab"),
        ("backspace", "⌫"),
        ("escape", "Esc"),
        ("up", "↑"),
        ("down", "↓"),
    ]
    .iter()
    .map(|(key, label)| json!({ "key": key, "label": label }))
    .collect();
    json!({
        "handover_max_secs": crate::browser::tools::HANDOVER_MAX.as_secs(),
        "handover_keys": keys,
    })
}
