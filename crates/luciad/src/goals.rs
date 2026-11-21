//! 목표(결정 0032): 후속 조치·반복 루틴·감시를 하나로 묶어 진행을 추적하고, 목표 점검이 다음 할 일을 정한다.
//!
//! - **만들기**: 대화에서 `goal_create`(사람 필요). 카드는 없다(사람이 대화에서 맡긴 일이다). 필요한 사이트 위임은
//!   지금처럼 `site_grant_propose` 카드로 받는다.
//! - **점검**: 점검 주기마다, 그리고 하위 항목·신호가 오면 곧 깨어난다(엔진 `run_goal_review`, 사람 없음).
//!   목표는 실행 맥락(`runctx`)에서 정한다. 모델이 준 목표 ID를 믿지 않는다.
//! - **넓어지지 않게**: 점검이 만든 하위 루틴은 깨울 때 점검과 같은 범위(브리핑 등급)로 돈다. 점검은 사이트 위임을
//!   만들거나 넓히지 못한다. 한 목표의 점검은 하루 [`REVIEWS_PER_DAY`]번까지.
//! - **닫기**: 하위 후속 조치 취소, 하위 루틴 끝내기, 하위 감시 지우기. 사이트 위임은 전역이라 그대로 둔다.

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use lucia_store::{Goal, GoalStatus, NewGoal, Schedule, ScheduleKind, Store};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// 점검 스케줄의 동작 이름. 페이로드 `{"id": "goal-…"}`.
pub const REVIEW_ACTION: &str = "goal_review";
/// 한 목표의 하루 점검 상한 [제안](신호 → 점검 → 새 하위 항목 → 신호 고리를 막는다).
pub const REVIEWS_PER_DAY: i64 = 6;
/// 점검 주기 기본값(시간) [제안].
pub const REVIEW_EVERY_DEFAULT: i64 = 24;
pub const REVIEW_EVERY_RANGE: std::ops::RangeInclusive<i64> = 1..=168;
/// 하위 항목·신호가 오면 이만큼 뒤에 점검한다(여러 신호를 한 번에 본다).
pub const SOON_MINUTES: i64 = 10;
/// 칸 길이.
pub const TITLE_MAX: usize = 80;
pub const TEXT_MAX: usize = 1000;
pub const NOTE_MAX: usize = 300;

pub fn schedule_id(goal: &str) -> String {
    format!("goal-review-{goal}")
}

/// 점검을 건다(그 시각에 한 번).
pub fn arm(store: &Store, goal: &str, at: DateTime<Utc>) -> lucia_store::Result<()> {
    store.upsert_schedule(&Schedule {
        id: schedule_id(goal),
        kind: ScheduleKind::Once,
        spec: at.to_rfc3339(),
        timezone: "UTC".into(),
        action: REVIEW_ACTION.into(),
        payload: serde_json::json!({ "id": goal }),
        // 놓친 점검은 늦게라도 한다(다음 주기를 다시 건다).
        max_delay_secs: 7 * 24 * 3600,
        next_due_at: Some(at),
        last_fired_at: None,
        enabled: true,
    })
}

pub fn disarm(store: &Store, goal: &str) -> lucia_store::Result<()> {
    crate::routines::disarm(store, &schedule_id(goal))
}

/// 열린 목표 중 걸린 점검이 없는 것을 다시 건다(재시작, 발사 뒤).
pub fn rearm_all(store: &Store, now: DateTime<Utc>) -> usize {
    let mut n = 0;
    for g in store.goals(false).unwrap_or_default() {
        let armed = store
            .schedule(&schedule_id(&g.id))
            .ok()
            .flatten()
            .is_some_and(|s| s.enabled && s.next_due_at.is_some());
        if !armed {
            let at = g
                .next_review_at
                .unwrap_or(now + Duration::hours(g.review_every_hours));
            if arm(store, &g.id, at).is_ok() {
                n += 1;
            }
        }
    }
    n
}

/// 현지 자정(하루 상한을 셀 때).
pub fn midnight(now: DateTime<Utc>, tz: Tz) -> DateTime<Utc> {
    now.with_timezone(&tz)
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|d| chrono::TimeZone::from_local_datetime(&tz, &d).earliest())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or(now - Duration::hours(24))
}

/// 하위 항목이나 신호가 왔다: 곧 점검한다(오늘 상한이 남았을 때). 까닭은 진행 기록에 남겨 점검이 본다.
pub fn poke(store: &Store, goal: &str, why: &str, now: DateTime<Utc>, tz: Tz) -> bool {
    let Ok(Some(g)) = store.goal(goal) else {
        return false;
    };
    if !g.status.open() {
        return false;
    }
    let _ = store.add_goal_log(goal, "signal", &clip(why, NOTE_MAX), now);
    if store
        .goal_reviews_since(goal, midnight(now, tz))
        .unwrap_or(0)
        >= REVIEWS_PER_DAY
    {
        return false;
    }
    let soon = now + Duration::minutes(SOON_MINUTES);
    if g.next_review_at.is_some_and(|t| t <= soon) {
        return true;
    }
    let _ = store.update_goal(goal, None, None, None, None, None, Some(soon), now);
    arm(store, goal, soon).is_ok()
}

/// 목표를 닫는다: 하위 후속 조치 취소, 하위 루틴 끝내기, 하위 감시 지우기. 사이트 위임은 그대로 둔다.
pub fn close(
    store: &Store,
    goal: &str,
    status: GoalStatus,
    why: &str,
    now: DateTime<Utc>,
) -> Result<String, String> {
    match store.set_goal_status(goal, status, now) {
        Ok(true) => {}
        Ok(false) => return Err("열린 목표가 아니다".into()),
        Err(e) => return Err(e.to_string()),
    }
    let _ = disarm(store, goal);
    let mut closed = Vec::new();
    for c in store.goal_children(goal).unwrap_or_default() {
        let done = match c.kind.as_str() {
            "followup" => store
                .close_followup(
                    &c.child,
                    lucia_store::FollowupStatus::Cancelled,
                    &format!("목표가 {}", status.label()),
                    now,
                )
                .unwrap_or(false),
            "routine" => {
                let open = store
                    .routine(&c.child)
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.status != lucia_store::RoutineStatus::Done);
                if open {
                    let _ =
                        store.set_routine_status(&c.child, lucia_store::RoutineStatus::Done, now);
                    let _ = crate::routines::disarm(store, &c.child);
                }
                open
            }
            "watch" => {
                let open = store
                    .watch(&c.child)
                    .ok()
                    .flatten()
                    .is_some_and(|w| w.status != lucia_store::WatchStatus::Removed);
                if open {
                    let _ =
                        store.set_watch_status(&c.child, lucia_store::WatchStatus::Removed, now);
                }
                open
            }
            _ => false,
        };
        if done {
            closed.push(c.child);
        }
    }
    let why = clip(why, NOTE_MAX);
    let _ = store.add_goal_log(goal, "status", &format!("{}: {why}", status.label()), now);
    let _ = store.add_activity(
        "goal_closed",
        &crate::mask::global().mask(&format!("목표 {} {goal}: {why}", status.label())),
        None,
        now,
    );
    Ok(if closed.is_empty() {
        format!("목표를 {}(으)로 닫았다.", status.label())
    } else {
        format!(
            "목표를 {}(으)로 닫고 하위 항목 {}개를 끝냈다({}). 사이트 위임은 그대로다(관리 페이지에서 거둔다).",
            status.label(),
            closed.len(),
            closed.join(", ")
        )
    })
}

pub fn clip(text: &str, max: usize) -> String {
    text.trim().chars().take(max).collect()
}

fn local(t: DateTime<Utc>, tz: Tz) -> String {
    t.with_timezone(&tz).format("%m-%d %H:%M").to_string()
}

/// 관심사(신호 1단계): (목표 ID, 찾을 글).
pub fn interests(store: &Store) -> Vec<(String, String)> {
    store
        .goals(false)
        .unwrap_or_default()
        .into_iter()
        .flat_map(|g| {
            crate::signals::parse_interests(&g.interests)
                .into_iter()
                .map(move |i| (g.id.clone(), i))
        })
        .collect()
}

/// 실행 맥락에 붙는 목표 목록(채팅·브리핑·점검). 없으면 빈 글.
pub fn context_block(store: &Store, now: DateTime<Utc>, tz: Tz) -> String {
    let goals = store.goals(false).unwrap_or_default();
    if goals.is_empty() {
        return String::new();
    }
    let mut out = String::from("<goals>\n맡은 목표(goal_update로 진행을 남긴다):\n");
    for g in &goals {
        let last = store
            .goal_log(&g.id, 1)
            .ok()
            .and_then(|l| l.into_iter().next())
            .map(|l| format!(" · 최근: {}", l.text))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "- {} · {} · {}{}{}",
            g.id,
            crate::context::escape(&g.title),
            g.status.label(),
            g.deadline
                .map(|d| format!(" · 마감 {}", local(d, tz)))
                .unwrap_or_default(),
            crate::context::escape(&last)
        );
    }
    let _ = now;
    out.push_str("</goals>\n");
    out
}

/// 점검 실행에 넣는 목표 한 개의 자세한 모습.
pub fn review_block(store: &Store, g: &Goal, now: DateTime<Utc>, tz: Tz) -> String {
    let e = crate::context::escape;
    let mut out = String::new();
    let _ = writeln!(out, "<goal id=\"{}\">", g.id);
    let _ = writeln!(out, "제목: {}", e(&g.title));
    let _ = writeln!(out, "목적: {}", e(&g.purpose));
    let _ = writeln!(out, "끝났다고 볼 조건: {}", e(&g.success));
    if let Some(d) = g.deadline {
        let left = d - now;
        let _ = writeln!(
            out,
            "마감: {} (남은 {}일 {}시간)",
            local(d, tz),
            left.num_days(),
            left.num_hours() % 24
        );
    }
    let _ = writeln!(out, "상태: {}", g.status.label());
    if !g.interests.trim().is_empty() {
        let _ = writeln!(out, "관심사: {}", e(&g.interests.replace('\n', ", ")));
    }
    let _ = writeln!(out, "점검 주기: {}시간", g.review_every_hours);
    out.push_str("하위 항목:\n");
    let children = store.goal_children(&g.id).unwrap_or_default();
    if children.is_empty() {
        out.push_str("- (없음)\n");
    }
    for c in &children {
        let by = if c.by_review {
            "점검이 만듦"
        } else {
            "대화에서 만듦"
        };
        let line = match c.kind.as_str() {
            "followup" => store.followup(&c.child).ok().flatten().map(|f| {
                format!(
                    "후속 조치 {} · {} · {} · {}",
                    f.id,
                    f.status.as_str(),
                    local(f.due_at, tz),
                    e(&clip(&f.task, 120))
                )
            }),
            "routine" => store.routine(&c.child).ok().flatten().map(|r| {
                let last = store
                    .routine_runs(&r.id, 1)
                    .ok()
                    .and_then(|v| v.into_iter().next())
                    .map(|x| format!(" · 지난 실행: {}", e(&clip(&x.text, 120))))
                    .unwrap_or_default();
                format!(
                    "루틴 {} · {} · {} · {}{last}",
                    r.id,
                    r.status.label(),
                    crate::routines::trigger_label(&r.trigger),
                    e(&r.name)
                )
            }),
            "watch" => store
                .watch(&c.child)
                .ok()
                .flatten()
                .map(|w| format!("감시 {} · {} · {}", w.id, w.status.label(), e(&w.what))),
            _ => None,
        };
        if let Some(line) = line {
            let _ = writeln!(out, "- {line} ({by})");
        }
    }
    out.push_str("진행 기록(최근 것부터):\n");
    for l in store.goal_log(&g.id, 12).unwrap_or_default() {
        let _ = writeln!(out, "- {} [{}] {}", local(l.at, tz), l.kind, e(&l.text));
    }
    out.push_str("</goal>\n");
    out
}

// ---------- 도구 ----------

/// `goal_create` 인자.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GoalCreate {
    /// 사람이 읽는 한 줄(예: "12월 원서 접수 챙기기").
    pub title: String,
    /// 무엇을 위해, 무엇을 하는지.
    pub purpose: String,
    /// 끝났다고 볼 조건(예: "접수 완료 메일을 받음").
    pub success: String,
    /// 마감(선택, 현지 시각 `YYYY-MM-DD HH:MM`).
    #[serde(default)]
    pub deadline: Option<String>,
    /// 관심 발신자·키워드(선택). 새 메일·일정이 이것과 맞으면 이 목표를 곧 점검한다.
    #[serde(default)]
    pub interests: Option<Vec<String>>,
    /// 점검 주기(시간, 1~168, 기본 24).
    #[serde(default)]
    pub review_every_hours: Option<i64>,
}

/// `goal_update` 인자. 바꿀 칸만 준다.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GoalUpdate {
    /// 목표 ID(goals 블록이나 goal_list의 id).
    pub id: String,
    /// 진행 기록 한 줄(무엇이 바뀌었고 다음은 무엇인지).
    #[serde(default)]
    pub note: Option<String>,
    /// 상태: active(진행 중) | waiting(사람 차례) | done(끝남, 사람 없는 실행이면 사용자에게 확인 카드) | abandoned(포기, 대화에서만).
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub success: Option<String>,
    /// 마감. 빈 글이면 지운다.
    #[serde(default)]
    pub deadline: Option<String>,
    #[serde(default)]
    pub interests: Option<Vec<String>>,
    #[serde(default)]
    pub review_every_hours: Option<i64>,
    /// 다음 점검 시각(현지 시각 `YYYY-MM-DD HH:MM`, 30일 안).
    #[serde(default)]
    pub next_review: Option<String>,
    /// 하위 항목으로 이을 것(대화에서만): ["followup-…", "routine-…", "watch-…"].
    #[serde(default)]
    pub link: Option<Vec<String>>,
}

/// 부른 실행.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    pub run: String,
    pub human: bool,
    /// 이 실행이 맡은 목표(실행 맥락에서, 모델이 준 값이 아니다).
    pub goal: Option<String>,
    /// 이 실행이 목표 점검인가.
    pub review: bool,
}

impl Caller {
    pub fn of(run: &str, human: bool) -> Self {
        let ctx = crate::runctx::get(run);
        Self {
            run: run.to_owned(),
            human,
            goal: ctx.as_ref().and_then(|c| c.goal.clone()),
            review: ctx.is_some_and(|c| c.goal_review),
        }
    }
}

fn interests_text(list: &[String]) -> String {
    crate::signals::parse_interests(&list.join("\n")).join("\n")
}

fn kind_of(child: &str) -> Option<&'static str> {
    if child.starts_with("followup-") {
        Some("followup")
    } else if child.starts_with("routine-") {
        Some("routine")
    } else if child.starts_with("watch-") {
        Some("watch")
    } else {
        None
    }
}

pub fn create(
    store: &Store,
    args: &GoalCreate,
    conversation: Option<&str>,
    now: DateTime<Utc>,
    tz: Tz,
) -> String {
    let title = clip(&args.title, TITLE_MAX);
    let purpose = clip(&args.purpose, TEXT_MAX);
    let success = clip(&args.success, TEXT_MAX);
    if title.is_empty() || success.is_empty() {
        return "만들지 않았다: 제목과 끝났다고 볼 조건이 필요하다.".into();
    }
    for t in [&title, &purpose, &success] {
        if let Some(why) = crate::autonomy::injection(t) {
            return format!("만들지 않았다: {why}가 들어 있다.");
        }
    }
    let deadline = match args
        .deadline
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        Some(d) => match crate::autonomy::parse_when(d, tz) {
            Ok(t) => Some(t),
            Err(e) => return format!("만들지 않았다: 마감: {e}"),
        },
        None => None,
    };
    let every = args.review_every_hours.unwrap_or(REVIEW_EVERY_DEFAULT);
    if !REVIEW_EVERY_RANGE.contains(&every) {
        return "만들지 않았다: 점검 주기는 1~168시간이다.".into();
    }
    let interests = interests_text(args.interests.as_deref().unwrap_or_default());
    let id = format!("goal-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let first = now + Duration::hours(every);
    let created = store.create_goal(
        &id,
        &NewGoal {
            title: &title,
            purpose: &purpose,
            success: &success,
            deadline,
            interests: &interests,
            review_every_hours: every,
            next_review_at: first,
            conversation,
        },
        now,
    );
    match created {
        Ok(Ok(())) => {}
        Ok(Err(why)) => return format!("만들지 않았다: {why}"),
        Err(e) => return format!("만들지 못했다: {e}"),
    }
    let _ = arm(store, &id, first);
    let _ = store.add_activity(
        "goal_create",
        &crate::mask::global().mask(&format!("목표 만듦 {id} \"{title}\"")),
        None,
        now,
    );
    format!(
        "목표 {id}을(를) 만들었다. 첫 점검 {}. 이 목표에 쓸 후속 조치·루틴·감시를 만든 뒤 goal_update(id, link=[…])로 이어 둔다. 사람 없이 열어야 할 로그인 사이트가 있으면 site_grant_propose로 위임을 제안한다.",
        local(first, tz)
    )
}

pub fn list(store: &Store, tz: Tz) -> String {
    let goals = store.goals(true).unwrap_or_default();
    if goals.is_empty() {
        return "목표가 없다.".into();
    }
    goals
        .iter()
        .take(20)
        .map(|g| {
            format!(
                "- {} · {} · {}{} · 다음 점검 {}",
                g.id,
                g.status.label(),
                crate::context::escape(&g.title),
                g.deadline
                    .map(|d| format!(" · 마감 {}", local(d, tz)))
                    .unwrap_or_default(),
                g.next_review_at
                    .filter(|_| g.status.open())
                    .map(|t| local(t, tz))
                    .unwrap_or_else(|| "-".into())
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `goal_update`. 카드를 올렸으면 그 승인 ID도 돌려준다(부른 쪽이 승인 데스크에 넘긴다).
pub fn update(
    store: &Store,
    caller: &Caller,
    args: &GoalUpdate,
    now: DateTime<Utc>,
    tz: Tz,
) -> (String, Option<String>) {
    let id = args.id.trim();
    let Ok(Some(g)) = store.goal(id) else {
        return (format!("목표가 없다: {id}"), None);
    };
    // 사람 없는 실행은 자기가 맡은 목표만. 점검이 아닌 하위 실행(루틴 깨우기, 후속 조치)은 진행 기록만 남긴다.
    if !caller.human {
        if caller.goal.as_deref() != Some(id) {
            return (
                "사람 없는 실행에서는 이 실행이 맡은 목표만 고칠 수 있다.".into(),
                None,
            );
        }
        let other = args.status.is_some()
            || args.title.is_some()
            || args.success.is_some()
            || args.deadline.is_some()
            || args.interests.is_some()
            || args.review_every_hours.is_some()
            || args.next_review.is_some();
        if !caller.review && other {
            return (
                "이 실행에서는 진행 기록(note)만 남길 수 있다. 목표를 고치는 것은 목표 점검이 한다.".into(),
                None,
            );
        }
        if args.link.is_some() {
            return (
                "사람 없는 실행에서는 하위 항목을 따로 잇지 않는다(점검이 만든 것은 저절로 이어진다).".into(),
                None,
            );
        }
        if args.title.is_some() || args.success.is_some() {
            return (
                "제목과 끝났다고 볼 조건은 사용자와의 대화에서만 바꾼다.".into(),
                None,
            );
        }
    }
    if !g.status.open() {
        return (format!("이미 닫힌 목표다({}).", g.status.label()), None);
    }
    let mut done = Vec::new();
    for t in [&args.note, &args.title, &args.success]
        .into_iter()
        .flatten()
    {
        if let Some(why) = crate::autonomy::injection(t) {
            return (format!("반영하지 않았다: {why}가 들어 있다."), None);
        }
    }
    // 칸 고치기.
    let deadline = match args.deadline.as_deref().map(str::trim) {
        None => None,
        Some("") => Some(None),
        Some(d) => match crate::autonomy::parse_when(d, tz) {
            Ok(t) => Some(Some(t)),
            Err(e) => return (format!("반영하지 않았다: 마감: {e}"), None),
        },
    };
    let next = match args
        .next_review
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(d) => match crate::autonomy::parse_when(d, tz) {
            Ok(t) if t > now && t <= now + Duration::days(30) => Some(t),
            Ok(_) => {
                return (
                    "반영하지 않았다: 다음 점검은 지금부터 30일 안이다.".into(),
                    None,
                );
            }
            Err(e) => return (format!("반영하지 않았다: 다음 점검: {e}"), None),
        },
    };
    if let Some(h) = args.review_every_hours
        && !REVIEW_EVERY_RANGE.contains(&h)
    {
        return ("반영하지 않았다: 점검 주기는 1~168시간이다.".into(), None);
    }
    let title = args.title.as_deref().map(|t| clip(t, TITLE_MAX));
    let success = args.success.as_deref().map(|t| clip(t, TEXT_MAX));
    let interests = args.interests.as_deref().map(interests_text);
    let fields = title.is_some()
        || success.is_some()
        || deadline.is_some()
        || interests.is_some()
        || args.review_every_hours.is_some()
        || next.is_some();
    if fields {
        let _ = store.update_goal(
            id,
            title.as_deref(),
            success.as_deref(),
            deadline,
            interests.as_deref(),
            args.review_every_hours,
            next,
            now,
        );
        if let Some(t) = next {
            let _ = arm(store, id, t);
            done.push(format!("다음 점검 {}", local(t, tz)));
        } else {
            done.push("목표를 고쳤다".into());
        }
    }
    if let Some(note) = args
        .note
        .as_deref()
        .map(|n| clip(n, NOTE_MAX))
        .filter(|n| !n.is_empty())
    {
        let _ = store.add_goal_log(id, "note", &note, now);
        done.push("진행 기록을 남겼다".into());
    }
    if let Some(links) = &args.link {
        for child in links {
            let child = child.trim();
            let Some(kind) = kind_of(child) else {
                done.push(format!("{child}: 이을 수 없는 항목"));
                continue;
            };
            let exists = match kind {
                "followup" => store.followup(child).ok().flatten().is_some(),
                "routine" => store.routine(child).ok().flatten().is_some(),
                _ => store.watch(child).ok().flatten().is_some(),
            };
            if !exists {
                done.push(format!("{child}: 없는 항목"));
                continue;
            }
            match store.link_goal_child(id, kind, child, false, now) {
                Ok(true) => {
                    let _ = store.add_goal_log(id, "child", &format!("{child} 이음"), now);
                    done.push(format!("{child} 이음"));
                }
                _ => done.push(format!("{child}: 이미 다른 목표에 이어져 있다")),
            }
        }
    }
    let mut card = None;
    if let Some(status) = args.status.as_deref().map(str::trim) {
        match GoalStatus::parse(status) {
            Some(s @ (GoalStatus::Active | GoalStatus::Waiting)) => {
                let _ = store.set_goal_status(id, s, now);
                let _ = store.add_goal_log(id, "status", s.label(), now);
                done.push(format!("상태 {}", s.label()));
            }
            Some(s @ GoalStatus::Done) if caller.human => {
                match close(store, id, s, "사용자와 확인", now) {
                    Ok(t) => done.push(t),
                    Err(e) => done.push(format!("닫지 못했다: {e}")),
                }
            }
            Some(GoalStatus::Done) => {
                // 끝났는지는 사람이 정한다(작업 완료 확인과 같다).
                let why = args.note.clone().unwrap_or_default();
                match request_done(store, &g, &why, &caller.run, now) {
                    Ok(aid) => {
                        card = Some(aid);
                        done.push("끝났는지 사용자에게 [완료]/[아직] 카드로 묻는다".into());
                    }
                    Err(e) => done.push(format!("확인 카드를 올리지 못했다: {e}")),
                }
            }
            Some(s @ GoalStatus::Abandoned) if caller.human => {
                match close(
                    store,
                    id,
                    s,
                    args.note.as_deref().unwrap_or("사용자가 포기"),
                    now,
                ) {
                    Ok(t) => done.push(t),
                    Err(e) => done.push(format!("닫지 못했다: {e}")),
                }
            }
            Some(GoalStatus::Abandoned) => {
                done.push("포기는 사용자와의 대화에서만 정한다(사용자에게 묻는다)".into());
            }
            None => done.push(format!("모르는 상태: {status}")),
        }
    }
    if done.is_empty() {
        return ("바꿀 것이 없다.".into(), None);
    }
    let _ = store.add_activity(
        "goal_update",
        &crate::mask::global().mask(&format!("목표 {id}: {}", done.join(", "))),
        Some(&caller.run),
        now,
    );
    (format!("{id}: {}.", done.join(", ")), card)
}

/// 끝났는지 묻는 카드(결정 0032). 승인 = 완료(하위 항목 닫기), 거절 = 계속.
pub fn request_done(
    store: &Store,
    g: &Goal,
    why: &str,
    run: &str,
    now: DateTime<Utc>,
) -> Result<String, String> {
    // 이미 열린 확인 카드가 있으면 또 올리지 않는다.
    let open = store
        .recent_approvals(200)
        .unwrap_or_default()
        .into_iter()
        .any(|a| {
            a.kind == crate::approvals::KIND_GOAL_DONE
                && a.status == lucia_store::ApprovalStatus::Pending
                && a.payload["goal"].as_str() == Some(g.id.as_str())
        });
    if open {
        return Err("이미 사용자에게 확인 카드를 올려 두었다(답을 기다린다)".into());
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    store
        .create_approval(
            &lucia_store::NewApproval {
                id: id.clone(),
                kind: crate::approvals::KIND_GOAL_DONE.into(),
                payload: serde_json::json!({
                    "goal": g.id,
                    "title": g.title,
                    "success": g.success,
                    "why": clip(why, TEXT_MAX),
                    "conversation": g.conversation,
                }),
                summary: format!("목표 확인: {}", g.title),
                run_id: Some(run.to_owned()),
                expires_at: now + Duration::days(7),
            },
            now,
        )
        .map_err(|e| format!("저장하지 못했다: {e}"))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz() -> Tz {
        chrono_tz::Asia::Seoul
    }

    fn make(store: &Store, now: DateTime<Utc>) -> String {
        let out = create(
            store,
            &GoalCreate {
                title: "12월 원서 접수".into(),
                purpose: "원서를 기한 안에 낸다".into(),
                success: "접수 완료 메일".into(),
                deadline: Some("2026-12-05 18:00".into()),
                interests: Some(vec!["입학처".into(), " ".into()]),
                review_every_hours: None,
            },
            Some("dm"),
            now,
            tz(),
        );
        assert!(out.starts_with("목표 goal-"), "{out}");
        store.goals(false).unwrap()[0].id.clone()
    }

    #[test]
    fn create_arms_a_review_and_lists() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        let id = make(&store, now);
        let s = store.schedule(&schedule_id(&id)).unwrap().unwrap();
        assert_eq!(s.action, REVIEW_ACTION);
        assert!(s.next_due_at.unwrap() > now + Duration::hours(23));
        assert_eq!(interests(&store), vec![(id.clone(), "입학처".to_owned())]);
        assert!(list(&store, tz()).contains("12월 원서 접수"));
        assert!(context_block(&store, now, tz()).contains(&id));
    }

    #[test]
    fn unattended_callers_only_touch_their_own_goal() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        let id = make(&store, now);
        let other = Caller {
            run: "followup-x".into(),
            human: false,
            goal: Some("goal-zzz".into()),
            review: true,
        };
        let args = GoalUpdate {
            id: id.clone(),
            note: Some("확인함".into()),
            ..GoalUpdate::default()
        };
        assert!(
            update(&store, &other, &args, now, tz())
                .0
                .contains("맡은 목표만")
        );

        let child = Caller {
            run: "routine-run".into(),
            human: false,
            goal: Some(id.clone()),
            review: false,
        };
        assert!(
            update(&store, &child, &args, now, tz())
                .0
                .contains("진행 기록")
        );
        let widen = GoalUpdate {
            id: id.clone(),
            review_every_hours: Some(1),
            ..GoalUpdate::default()
        };
        assert!(
            update(&store, &child, &widen, now, tz()).0.contains("note"),
            "하위 실행은 기록만"
        );

        let review = Caller {
            run: "goal-run".into(),
            human: false,
            goal: Some(id.clone()),
            review: true,
        };
        let give_up = GoalUpdate {
            id: id.clone(),
            status: Some("abandoned".into()),
            ..GoalUpdate::default()
        };
        assert!(
            update(&store, &review, &give_up, now, tz())
                .0
                .contains("대화에서만")
        );
        assert!(store.goal(&id).unwrap().unwrap().status.open());
        let link = GoalUpdate {
            id: id.clone(),
            link: Some(vec!["routine-1".into()]),
            ..GoalUpdate::default()
        };
        assert!(
            update(&store, &review, &link, now, tz())
                .0
                .contains("잇지 않는다")
        );
        let done = GoalUpdate {
            id: id.clone(),
            status: Some("done".into()),
            note: Some("접수 완료 메일 확인".into()),
            ..GoalUpdate::default()
        };
        let (text, card) = update(&store, &review, &done, now, tz());
        assert!(card.is_some(), "{text}");
        assert!(
            store.goal(&id).unwrap().unwrap().status.open(),
            "끝은 사람이 정한다"
        );
    }

    #[test]
    fn closing_ends_children_but_not_site_grants() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        let id = make(&store, now);
        let f = crate::autonomy::create_followup(
            &store,
            &crate::autonomy::FollowupRequest {
                when: String::new(),
                task: "원서 확인".into(),
                detail: String::new(),
                origin: "chat".into(),
                conversation: None,
                run_id: None,
            },
            now + Duration::days(1),
            tz(),
            now,
        )
        .unwrap();
        let human = Caller {
            run: "chat-1".into(),
            human: true,
            ..Caller::default()
        };
        let link = GoalUpdate {
            id: id.clone(),
            link: Some(vec![f.clone(), "nothing-1".into()]),
            ..GoalUpdate::default()
        };
        let (text, _) = update(&store, &human, &link, now, tz());
        assert!(text.contains(&format!("{f} 이음")), "{text}");
        let give_up = GoalUpdate {
            id: id.clone(),
            status: Some("abandoned".into()),
            ..GoalUpdate::default()
        };
        let (text, _) = update(&store, &human, &give_up, now, tz());
        assert!(text.contains("하위 항목 1개"), "{text}");
        assert_eq!(
            store.followup(&f).unwrap().unwrap().status,
            lucia_store::FollowupStatus::Cancelled
        );
        assert!(!store.schedule(&schedule_id(&id)).unwrap().unwrap().enabled);
    }

    #[test]
    fn pokes_are_capped_per_day() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        let id = make(&store, now);
        assert!(poke(&store, &id, "입학처 메일", now, tz()));
        let next = store.goal(&id).unwrap().unwrap().next_review_at.unwrap();
        assert!(next <= now + Duration::minutes(SOON_MINUTES));
        for _ in 0..REVIEWS_PER_DAY {
            store.add_goal_log(&id, "review", "점검", now).unwrap();
        }
        assert!(!poke(&store, &id, "또 메일", now, tz()), "하루 상한");
    }
}
