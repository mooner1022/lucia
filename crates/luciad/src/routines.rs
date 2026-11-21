//! 반복 루틴(결정 0025): 사람이 대화에서 만든 반복 실행. 정한 때(cron), 간격(몇 분마다, 시간대 안에서),
//! 사전 체크가 찾은 변화(새 메일, 감시한 페이지)마다 돈다.
//!
//! - **스크립트가 먼저**: 스크립트가 있으면 모델 없이 그것부터 돌린다. 알릴 것만 `lucia.notify`로 보내고,
//!   판단이 필요할 때만 `lucia.wake`로 Lucia를 깨운다. 할 일이 없으면 토큰을 쓰지 않고 끝난다.
//!   스크립트가 없으면 매번 Lucia를 깨운다(그래서 간격 하한이 더 길다).
//! - **거는 법**: 후속 조치처럼 루틴 ID와 같은 일회성 스케줄 하나를 두고, 실행마다 다음 것을 건다
//!   (설정의 cron 루틴 동기화가 끄지 않게). 잃은 스케줄은 시작할 때와 조용할 때 다시 건다.
//! - **범위**: 루틴 스크립트는 사람 없이 돈다. 그래서 만든 사람(채팅)의 범위가 아니라 발사할 때 정한 읽기 범위
//!   ([`script_policy`])와 만들 때 선언한 도구만 쓴다. 쓰기는 Lucia를 깨워서 하고, 그 실행은 승인 카드를 거친다.
//!   사용자 자료를 읽으면서 바깥으로 가져가는(fetch) 스크립트만 사람이 카드에서 승인한다.

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use lucia_store::{Routine, RoutineStatus, Schedule, ScheduleKind, Store, Trigger};
use serde_json::{Value, json};

use crate::autonomy::QuietHours;
use crate::tools::{Grade, Group, Policy};

/// 루틴 발사 스케줄의 동작. 페이로드 `{"id": "routine-…"}`.
pub const ACTION: &str = "routine";
/// 방해 금지 시간에 미룬 깨우기. 페이로드 `{"id", "note", "data"}`.
pub const WAKE_ACTION: &str = "routine_wake";
/// 간격 하한 [제안]: 스크립트만(모델 없음) / 매번 모델.
pub const MIN_SCRIPT_MINUTES: i64 = 5;
pub const MIN_WAKE_MINUTES: i64 = 60;
pub const MAX_EVERY_MINUTES: i64 = 7 * 24 * 60;
/// 루틴이 하루에 Lucia를 깨우는 횟수(모든 루틴 합)의 기본값 [제안]. 관리 페이지 "자동 실행 한도"에서 바꾼다.
pub const DAILY_WAKES_MAX: i64 = 24;
pub const DAILY_WAKES_RANGE: std::ops::RangeInclusive<i64> = 1..=200;

/// 하루 깨우기 상한(관리 페이지 값, 없으면 기본값).
pub fn daily_wakes_max(store: &Store) -> i64 {
    store
        .setting(crate::autonomy::keys::ROUTINE_WAKES)
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| DAILY_WAKES_RANGE.contains(v))
        .unwrap_or(DAILY_WAKES_MAX)
}
/// 이만큼 이어 실패하면 멈추고 알린다 [제안].
pub const PAUSE_AFTER_FAILURES: i64 = 3;
/// 늦은 발사는 이만큼까지 한다(넘으면 건너뛰고 다음 것을 건다).
pub const MAX_DELAY_SECS: u64 = 30 * 60;
pub const NAME_MAX: usize = 60;
pub const TASK_MAX: usize = 1000;
/// 루틴 하나의 알림(글자) [제안].
pub const NOTIFY_MAX: usize = 1500;

/// 사람 없이 도는 루틴 스크립트의 범위: 기억·메일·일정·Discord 읽기만.
pub fn script_policy() -> Policy {
    Policy::new(
        &[Group::Memory, Group::Google, Group::Discord],
        Grade::Read,
        false,
    )
    .for_script()
}

/// 받은 칸에서 언제를 고른다(셋 중 하나).
pub fn parse_trigger(
    cron: Option<&str>,
    every_minutes: Option<i64>,
    between: Option<&str>,
    on_change: Option<&str>,
) -> Result<Trigger, String> {
    let cron = cron.map(str::trim).filter(|c| !c.is_empty());
    let on_change = on_change.map(str::trim).filter(|c| !c.is_empty());
    let between = between.map(str::trim).filter(|c| !c.is_empty());
    match (cron, every_minutes, on_change) {
        (Some(c), None, None) if between.is_none() => Ok(Trigger::Cron(c.to_owned())),
        (None, Some(m), None) => Ok(Trigger::Every {
            minutes: m,
            window: between.map(str::to_owned),
        }),
        (None, None, Some(s)) if between.is_none() => Ok(Trigger::Change(s.to_owned())),
        (_, _, _) if between.is_some() && every_minutes.is_none() => {
            Err("between은 every_minutes와 함께 쓴다".into())
        }
        _ => Err("cron, every_minutes, on_change 중 하나만 정한다".into()),
    }
}

/// 언제가 맞는지: 형식, 간격 하한(스크립트가 없으면 더 길게), 아는 소스.
pub fn check_trigger(
    t: &Trigger,
    scripted: bool,
    tz: Tz,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let min = if scripted {
        MIN_SCRIPT_MINUTES
    } else {
        MIN_WAKE_MINUTES
    };
    match t {
        Trigger::Cron(c) => {
            if c.split_whitespace().count() != 5 {
                return Err("cron은 다섯 칸(분 시 일 월 요일)이다".into());
            }
            let mut at = now;
            let mut prev: Option<DateTime<Utc>> = None;
            for _ in 0..6 {
                let next = crate::scheduler::next_after(ScheduleKind::Cron, c, tz.name(), at)
                    .map_err(|e| format!("cron 식이 맞지 않다: {e:#}"))?
                    .ok_or("cron 식의 다음 시각이 없다")?;
                if let Some(p) = prev
                    && (next - p) < Duration::minutes(min)
                {
                    return Err(format!(
                        "너무 자주 돈다: {}분 이상 간격이어야 한다(스크립트가 {})",
                        min,
                        if scripted {
                            "있을 때"
                        } else {
                            "없으면 매번 Lucia를 깨우므로"
                        }
                    ));
                }
                prev = Some(next);
                at = next;
            }
            Ok(())
        }
        Trigger::Every { minutes, window } => {
            if *minutes < min || *minutes > MAX_EVERY_MINUTES {
                return Err(format!(
                    "every_minutes는 {min}~{MAX_EVERY_MINUTES}분(스크립트가 {})",
                    if scripted {
                        "있을 때"
                    } else {
                        "없으면 매번 Lucia를 깨우므로 60분부터"
                    }
                ));
            }
            if let Some(w) = window {
                QuietHours::parse(w).map_err(|e| format!("between: {e}"))?;
            }
            Ok(())
        }
        Trigger::Change(s) => {
            let ok = matches!(s.as_str(), "gmail" | "calendar" | "watch")
                || s.strip_prefix(crate::watch::SOURCE_PREFIX)
                    .is_some_and(|id| !id.is_empty());
            if ok {
                Ok(())
            } else {
                Err("on_change는 gmail, calendar, watch(모든 감시), watch:<id> 중 하나다".into())
            }
        }
    }
}

/// 사람이 읽을 언제.
pub fn trigger_label(t: &Trigger) -> String {
    match t {
        Trigger::Cron(c) => format!("cron {c}"),
        Trigger::Every { minutes, window } => {
            let every = if minutes % 60 == 0 {
                format!("{}시간마다", minutes / 60)
            } else {
                format!("{minutes}분마다")
            };
            match window {
                Some(w) => format!("{every}({w})"),
                None => every,
            }
        }
        Trigger::Change(s) => format!("새 변화마다({s}, 사전 체크 때)"),
    }
}

/// 다음 실행 시각. 켜져 있지 않거나, 변화로 도는 루틴이거나, 기한을 넘으면 `None`.
pub fn next_due(r: &Routine, now: DateTime<Utc>, tz: Tz) -> Option<DateTime<Utc>> {
    if r.status != RoutineStatus::Active {
        return None;
    }
    let next = match (r.next_at, &r.trigger) {
        (_, Trigger::Change(_)) => return None,
        (Some(t), _) if t > now => t,
        (_, Trigger::Cron(c)) => {
            crate::scheduler::next_after(ScheduleKind::Cron, c, tz.name(), now).ok()??
        }
        (_, Trigger::Every { minutes, window }) => {
            let step = Duration::minutes(*minutes);
            let t = match r.last_run_at {
                Some(last) => (last + step).max(now + Duration::minutes(1)),
                None => now + step,
            };
            match window.as_deref().and_then(|w| QuietHours::parse(w).ok()) {
                Some(w) if !w.contains(t, tz) => window_start_after(&w, t, tz)?,
                _ => t,
            }
        }
    };
    match r.until {
        Some(until) if next > until => None,
        _ => Some(next),
    }
}

/// 시간대(창) 밖인 `t` 다음에 창이 열리는 시각.
fn window_start_after(w: &QuietHours, t: DateTime<Utc>, tz: Tz) -> Option<DateTime<Utc>> {
    use chrono::TimeZone as _;
    let local = t.with_timezone(&tz);
    let mut day = local.date_naive();
    for _ in 0..3 {
        if let Some(start) = tz.from_local_datetime(&day.and_time(w.start)).earliest() {
            let start = start.with_timezone(&Utc);
            if start > t {
                return Some(start);
            }
        }
        day = day.succ_opt()?;
    }
    None
}

/// 다음 실행을 건다(없으면 스케줄을 끈다). 건 시각.
pub fn arm(
    store: &Store,
    r: &Routine,
    now: DateTime<Utc>,
    tz: Tz,
) -> lucia_store::Result<Option<DateTime<Utc>>> {
    let next = next_due(r, now, tz);
    match next {
        Some(at) => store.upsert_schedule(&Schedule {
            id: r.id.clone(),
            kind: ScheduleKind::Once,
            spec: at.to_rfc3339(),
            timezone: tz.name().to_owned(),
            action: ACTION.into(),
            payload: json!({ "id": r.id }),
            max_delay_secs: MAX_DELAY_SECS,
            next_due_at: Some(at),
            last_fired_at: None,
            enabled: true,
        })?,
        None => disarm(store, &r.id)?,
    }
    Ok(next)
}

/// 발사 스케줄을 끈다.
pub fn disarm(store: &Store, id: &str) -> lucia_store::Result<()> {
    if let Some(mut s) = store.schedule(id)? {
        s.enabled = false;
        s.next_due_at = None;
        store.upsert_schedule(&s)?;
    }
    Ok(())
}

/// 켜진 시간 루틴 중 걸린 스케줄이 없는 것을 다시 건다(재시작, 늦어 건너뛴 발사). 기한이 지난 것은 끝낸다.
/// (다시 건 수, 끝낸 루틴 이름)
pub fn rearm_all(
    store: &Store,
    now: DateTime<Utc>,
    tz: Tz,
) -> lucia_store::Result<(usize, Vec<String>)> {
    let mut rearmed = 0;
    let mut ended = Vec::new();
    for r in store.routines()? {
        if r.status != RoutineStatus::Active {
            continue;
        }
        if r.until.is_some_and(|u| u <= now) {
            store.set_routine_status(&r.id, RoutineStatus::Done, now)?;
            disarm(store, &r.id)?;
            ended.push(r.name.clone());
            continue;
        }
        if matches!(r.trigger, Trigger::Change(_)) {
            continue;
        }
        let armed = store
            .schedule(&r.id)?
            .is_some_and(|s| s.enabled && s.next_due_at.is_some());
        if !armed && arm(store, &r, now, tz)?.is_some() {
            rearmed += 1;
        }
    }
    Ok((rearmed, ended))
}

/// 스크립트가 선언할 수 있는 도구(루틴): 읽기 범위 안의 도구와 `fetch`.
pub fn check_tools(tools: &[String]) -> Result<(), String> {
    let policy = script_policy();
    for t in tools {
        if t == "fetch" || t == "fetch_session" {
            continue;
        }
        if !crate::skills::script_tool_ok(t) || !policy.allows(t) {
            return Err(format!(
                "루틴 스크립트는 {t}을(를) 쓸 수 없다: 읽기 도구(기억·메일·일정·Discord 읽기)와 fetch만 쓴다. 쓰기가 필요하면 lucia.wake로 Lucia를 깨워 맡긴다"
            ));
        }
    }
    if tools.len() > 30 {
        return Err("도구는 30개까지".into());
    }
    Ok(())
}

/// 사람이 카드에서 승인해야 하는가: 사용자 자료를 읽으면서 바깥으로 가져간다(fetch). 새 메일·일정 변화로 도는
/// 루틴은 변화 목록(`lucia.input["changes"]`)으로 사용자 자료를 받으므로 읽기로 친다.
pub fn needs_review(tools: &[String], trigger: &Trigger) -> bool {
    let private = tools.iter().any(|t| crate::skills::private_read(t))
        || matches!(trigger, Trigger::Change(s) if s == "gmail" || s == "calendar");
    tools.iter().any(|t| t == "fetch") && private
}

/// 깨울 때 맡길 메모: 사람이 만든 루틴의 할 일만(스크립트가 본 것과 자료는 [`wake_data`]가 외부 데이터로).
pub fn wake_memo(r: &Routine, tz: Tz, now: DateTime<Utc>) -> String {
    let mut memo = format!(
        "반복 루틴 \"{}\"({}, {})이 Lucia를 깨웠다.\n할 일: {}",
        r.name,
        r.id,
        trigger_label(&r.trigger),
        r.task
    );
    if let Some(last) = r.last_run_at {
        memo.push_str(&format!(
            "\n지난 실행: {}",
            last.with_timezone(&tz).format("%m-%d %H:%M")
        ));
    }
    memo.push_str(&format!(
        "\n지금: {}",
        now.with_timezone(&tz).format("%m-%d %H:%M")
    ));
    memo
}

/// 스크립트가 본 것(`lucia.wake`의 첫 인자)과 넘긴 자료. 외부 글에서 왔을 수 있으므로 외부 데이터로 감싼다.
pub fn wake_data(note: &str, data: &Value) -> Option<String> {
    let mut body = String::new();
    if !note.trim().is_empty() {
        body.push_str("스크립트가 본 것: ");
        body.push_str(note.trim());
        body.push('\n');
    }
    match data {
        Value::Null => {}
        Value::String(s) => body.push_str(s),
        v => body.push_str(&serde_json::to_string_pretty(v).unwrap_or_default()),
    }
    (!body.trim().is_empty())
        .then(|| crate::mcp::external("routine-script", &crate::script::clip(&body, 12_000)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn kst(d: u32, h: u32, m: u32) -> DateTime<Utc> {
        chrono_tz::Asia::Seoul
            .with_ymd_and_hms(2026, 11, d, h, m, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn routine(trigger: Trigger) -> Routine {
        Routine {
            id: "routine-1".into(),
            name: "과제 메일".into(),
            trigger,
            task: "요약".into(),
            script: Some("pass".into()),
            tools: vec![],
            input: json!({}),
            status: RoutineStatus::Active,
            conversation: None,
            until: None,
            next_at: None,
            created_at: kst(17, 9, 0),
            updated_at: kst(17, 9, 0),
            last_run_at: None,
            last_ok: None,
            last_text: None,
            runs: 0,
            wakes: 0,
            failures: 0,
        }
    }

    #[test]
    fn triggers_parse_and_respect_minimum_intervals() {
        let tz = chrono_tz::Asia::Seoul;
        let now = kst(17, 9, 0);
        let t = parse_trigger(Some("0 9 * * 1-5"), None, None, None).unwrap();
        assert!(check_trigger(&t, false, tz, now).is_ok());
        let often = parse_trigger(Some("*/10 * * * *"), None, None, None).unwrap();
        assert!(
            check_trigger(&often, true, tz, now).is_ok(),
            "스크립트는 5분부터"
        );
        assert!(
            check_trigger(&often, false, tz, now).is_err(),
            "모델만이면 60분부터"
        );
        assert!(check_trigger(&Trigger::Cron("0 9 * *".into()), true, tz, now).is_err());
        assert!(parse_trigger(Some("0 9 * * *"), Some(30), None, None).is_err());
        assert!(parse_trigger(None, None, Some("09:00-18:00"), Some("gmail")).is_err());
        let every = parse_trigger(None, Some(30), Some("09:00-21:00"), None).unwrap();
        assert!(check_trigger(&every, true, tz, now).is_ok());
        assert!(check_trigger(&every, false, tz, now).is_err());
        assert!(
            check_trigger(
                &Trigger::Every {
                    minutes: 30,
                    window: Some("9시-6시".into())
                },
                true,
                tz,
                now
            )
            .is_err()
        );
        for ok in ["gmail", "calendar", "watch", "watch:3"] {
            assert!(check_trigger(&Trigger::Change(ok.into()), true, tz, now).is_ok());
        }
        assert!(check_trigger(&Trigger::Change("rss".into()), true, tz, now).is_err());
        assert_eq!(trigger_label(&every), "30분마다(09:00-21:00)");
    }

    #[test]
    fn next_due_follows_trigger_window_override_and_until() {
        let tz = chrono_tz::Asia::Seoul;
        let mut r = routine(Trigger::Cron("30 7 * * *".into()));
        assert_eq!(next_due(&r, kst(17, 8, 0), tz), Some(kst(18, 7, 30)));

        r.trigger = Trigger::Every {
            minutes: 90,
            window: Some("09:00-21:00".into()),
        };
        assert_eq!(next_due(&r, kst(17, 9, 0), tz), Some(kst(17, 10, 30)));
        r.last_run_at = Some(kst(17, 20, 0));
        assert_eq!(
            next_due(&r, kst(17, 20, 0), tz),
            Some(kst(18, 9, 0)),
            "창 밖이면 다음 창이 열릴 때"
        );
        r.next_at = Some(kst(17, 20, 10));
        assert_eq!(
            next_due(&r, kst(17, 20, 0), tz),
            Some(kst(17, 20, 10)),
            "스크립트가 정한 다음 실행"
        );
        r.next_at = None;
        r.until = Some(kst(18, 8, 0));
        assert_eq!(next_due(&r, kst(17, 20, 0), tz), None, "기한 뒤면 끝");

        r.until = None;
        r.status = RoutineStatus::Paused;
        assert_eq!(next_due(&r, kst(17, 20, 0), tz), None);
        let change = routine(Trigger::Change("gmail".into()));
        assert_eq!(next_due(&change, kst(17, 20, 0), tz), None);
    }

    #[test]
    fn arming_and_rearming_use_a_once_schedule() {
        let tz = chrono_tz::Asia::Seoul;
        let store = Store::open_in_memory().unwrap();
        let trigger = Trigger::Cron("0 9 * * *".into());
        store
            .create_routine(
                "routine-1",
                &lucia_store::NewRoutine {
                    name: "아침",
                    trigger: &trigger,
                    task: "확인",
                    script: None,
                    tools: &[],
                    input: &json!({}),
                    status: RoutineStatus::Active,
                    conversation: None,
                    until: Some(kst(20, 0, 0)),
                },
                kst(17, 8, 0),
            )
            .unwrap()
            .unwrap();
        let (n, ended) = rearm_all(&store, kst(17, 8, 0), tz).unwrap();
        assert_eq!((n, ended.len()), (1, 0));
        let s = store.schedule("routine-1").unwrap().unwrap();
        assert_eq!(s.kind, ScheduleKind::Once);
        assert_eq!(s.next_due_at, Some(kst(17, 9, 0)));
        assert_eq!(s.action, ACTION);
        // 이미 걸려 있으면 그대로.
        assert_eq!(rearm_all(&store, kst(17, 8, 1), tz).unwrap().0, 0);
        // 기한이 지나면 끝난다.
        let (_, ended) = rearm_all(&store, kst(20, 1, 0), tz).unwrap();
        assert_eq!(ended, vec!["아침"]);
        assert!(!store.schedule("routine-1").unwrap().unwrap().enabled);
    }

    #[test]
    fn routine_scripts_read_only_and_review_only_for_leaks() {
        let tools = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(check_tools(&tools(&["mail_search", "calendar_events", "fetch"])).is_ok());
        assert!(check_tools(&tools(&["mail_send"])).is_err());
        assert!(
            check_tools(&tools(&["browser_open"])).is_err(),
            "사람 없이 브라우저 없음"
        );
        assert!(check_tools(&tools(&["memory_write"])).is_err());
        let cron = Trigger::Cron("0 9 * * *".into());
        assert!(needs_review(&tools(&["mail_read", "fetch"]), &cron));
        assert!(!needs_review(&tools(&["fetch"]), &cron));
        assert!(!needs_review(&tools(&["mail_read"]), &cron));
        assert!(
            needs_review(&tools(&["fetch"]), &Trigger::Change("gmail".into())),
            "새 메일 변화는 사용자 자료"
        );
        assert!(!needs_review(
            &tools(&["fetch"]),
            &Trigger::Change("watch:3".into())
        ));
        // 로그인 세션 가져오기(결정 0036)는 사용자 자료 읽기: 바깥 fetch와 함께면 승인.
        assert!(check_tools(&tools(&["fetch_session", "fetch"])).is_ok());
        assert!(!needs_review(&tools(&["fetch_session"]), &cron));
        assert!(needs_review(&tools(&["fetch_session", "fetch"]), &cron));
        let p = script_policy();
        assert!(p.allows("mail_search") && !p.allows("memory_write") && !p.allows("browser_open"));
    }
}
