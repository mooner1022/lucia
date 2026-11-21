//! 교차 정합성 검사(결정 0033): 메일·일정·기억을 이어 보고 어긋남을 먼저 찾는다.
//!
//! - 코드(모델 없음): 앞으로 7일 일정의 겹침과 빠듯한 이동 시간, 날짜·마감 말이 든 새 메일 후보 뽑기.
//! - 후보가 있을 때만 Lucia(브리핑 등급 + 캘린더 쓰기)가 메일을 확인해 캘린더에 없는 마감·약속을 넣거나 묻는다.
//! - 같은 메일은 한 번만 본다(지난 검사 시각 뒤에 들어온 신호만). 같은 겹침은 7일에 한 번만 알린다.

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use lucia_precheck::google::assist::CalendarEvent;
use regex::Regex;
use std::sync::OnceLock;

/// 동작 이름과 내장 루틴 ID.
pub const ACTION: &str = "consistency_check";
pub const ROUTINE_ID: &str = "consistency-check";
/// 하루 두 번 [제안].
pub const CRON: &str = "30 12,18 * * *";
/// 지난 검사 시각(설정).
pub const SINCE_KEY: &str = "consistency.since";
/// 관리 페이지에서 끈다(`off`).
pub const ENABLED_KEY: &str = "consistency.enabled";
/// 겹침을 보는 기간(일) [제안].
pub const AHEAD_DAYS: i64 = 7;
/// 다른 장소로 옮길 때 이보다 짧으면 알린다(분) [제안].
pub const TRAVEL_MINUTES: i64 = 30;
/// 같은 알림을 다시 하기까지(시간).
pub const RENOTIFY_HOURS: i64 = 7 * 24;
/// 한 번에 넘기는 메일 후보 수.
pub const CANDIDATES_MAX: usize = 20;

pub fn enabled(store: &lucia_store::Store) -> bool {
    store.setting(ENABLED_KEY).ok().flatten().as_deref() != Some("off")
}

/// 날짜·마감 말이 들어 있는가(후보 뽑기, 넉넉하게).
pub fn date_hint(text: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?ix)
            \d{1,2}\s*월\s*\d{1,2}\s*일
            | \b\d{1,2}/\d{1,2}\b
            | \d{4}[-./]\s?\d{1,2}[-./]\s?\d{1,2}
            | \d{1,2}\s*:\s*\d{2}
            | \d{1,2}\s*시
            | 오늘 | 내일 | 모레 | 이번\s*주 | 다음\s*주 | 다음\s*달
            | [월화수목금토일]요일
            | 마감 | 기한 | 까지 | 일정 | 변경 | 연기 | 취소 | 면담 | 약속 | 회의 | 시험 | 제출
            | \bdeadline\b | \bdue\b | \buntil\b | \breschedul | \bcancel | \bmeeting\b
            | \b(mon|tues|wednes|thurs|fri|satur|sun)day\b",
        )
        .expect("date hint regex")
    });
    re.is_match(text)
}

fn start_of(e: &CalendarEvent) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&e.start)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn end_of(e: &CalendarEvent) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&e.end)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// 겹치는 일정과 빠듯한 이동 시간: (같은 알림을 거르는 키, 알림 글).
pub fn conflicts(events: &[CalendarEvent], tz: Tz) -> Vec<(String, String)> {
    let hm = |t: DateTime<Utc>| t.with_timezone(&tz).format("%m-%d %H:%M").to_string();
    let mut timed: Vec<(&CalendarEvent, DateTime<Utc>, DateTime<Utc>)> = events
        .iter()
        .filter(|e| !e.all_day && e.status.as_deref() != Some("cancelled"))
        .filter_map(|e| Some((e, start_of(e)?, end_of(e)?)))
        .collect();
    timed.sort_by_key(|(_, s, _)| *s);
    let mut out = Vec::new();
    for (i, (a, a_start, a_end)) in timed.iter().enumerate() {
        for (b, b_start, b_end) in timed.iter().skip(i + 1) {
            if b_start >= a_end {
                // 바로 다음 일정만 이동 시간을 본다.
                let gap = *b_start - *a_end;
                if gap < Duration::minutes(TRAVEL_MINUTES)
                    && let (Some(la), Some(lb)) = (&a.location, &b.location)
                    && !la.trim().is_empty()
                    && !lb.trim().is_empty()
                    && la.trim() != lb.trim()
                    && timed
                        .iter()
                        .filter(|(_, s, _)| s >= a_end && s < b_start)
                        .count()
                        == 0
                {
                    out.push((
                        format!("travel:{}:{}", a.id, b.id),
                        format!(
                            "🚶 이동 시간이 빠듯합니다: {}({}, {}에 끝남) → {}({}, {} 시작), {}분",
                            a.summary,
                            la.trim(),
                            hm(*a_end),
                            b.summary,
                            lb.trim(),
                            hm(*b_start),
                            gap.num_minutes()
                        ),
                    ));
                }
                break;
            }
            out.push((
                format!("overlap:{}:{}", a.id, b.id),
                format!(
                    "📅 일정이 겹칩니다: {}({}–{}) / {}({}–{})",
                    a.summary,
                    hm(*a_start),
                    hm(*a_end),
                    b.summary,
                    hm(*b_start),
                    hm(*b_end)
                ),
            ));
        }
    }
    out
}

/// 일정 읽기(엔진이 코드로 겹침을 볼 때). 시험에서는 가짜로 바꾼다.
pub trait CalendarRead: Send + Sync + 'static {
    fn upcoming<'a>(
        &'a self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> crate::browser::BoxFut<'a, Result<Vec<CalendarEvent>, String>>;
}

impl<T: lucia_precheck::google::TokenProvider> CalendarRead
    for lucia_precheck::google::assist::GoogleAssist<T>
{
    fn upcoming<'a>(
        &'a self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> crate::browser::BoxFut<'a, Result<Vec<CalendarEvent>, String>> {
        Box::pin(async move {
            self.list_events(from, to, None, 100)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, s: &str, e: &str, loc: Option<&str>) -> CalendarEvent {
        CalendarEvent {
            id: id.into(),
            start: s.into(),
            end: e.into(),
            all_day: false,
            summary: id.to_uppercase(),
            location: loc.map(str::to_owned),
            description: None,
            status: Some("confirmed".into()),
            link: None,
        }
    }

    #[test]
    fn hints_catch_dates_and_deadlines_only() {
        assert!(date_hint("과제 제출 12월 5일 18시까지"));
        assert!(date_hint("면담 시간 변경 안내"));
        assert!(date_hint("Assignment due Friday"));
        assert!(date_hint("2026-12-05 접수"));
        assert!(!date_hint("이번 학기 소식지입니다"));
        assert!(!date_hint("Weekly newsletter"));
    }

    #[test]
    fn overlaps_and_tight_moves_are_found() {
        let tz = chrono_tz::Asia::Seoul;
        let list = vec![
            ev(
                "a",
                "2026-11-20T10:00:00+09:00",
                "2026-11-20T11:00:00+09:00",
                Some("공학관"),
            ),
            ev(
                "b",
                "2026-11-20T10:30:00+09:00",
                "2026-11-20T11:30:00+09:00",
                None,
            ),
            ev(
                "c",
                "2026-11-20T11:40:00+09:00",
                "2026-11-20T12:30:00+09:00",
                Some("병원"),
            ),
            ev(
                "d",
                "2026-11-20T15:00:00+09:00",
                "2026-11-20T16:00:00+09:00",
                Some("병원"),
            ),
        ];
        let found = conflicts(&list, tz);
        let keys: Vec<&str> = found.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"overlap:a:b"), "{keys:?}");
        assert!(
            !keys.iter().any(|k| k.contains(":d")),
            "멀리 떨어진 일정은 아니다: {keys:?}"
        );
        // a(공학관, 11:00 끝) 다음 일정은 b라 이동 시간을 보지 않는다. b(장소 없음)→c도 보지 않는다.
        assert!(!keys.iter().any(|k| k.starts_with("travel")), "{keys:?}");
        let tight = vec![
            ev(
                "x",
                "2026-11-20T10:00:00+09:00",
                "2026-11-20T11:00:00+09:00",
                Some("공학관"),
            ),
            ev(
                "y",
                "2026-11-20T11:10:00+09:00",
                "2026-11-20T12:00:00+09:00",
                Some("시내 병원"),
            ),
        ];
        let found = conflicts(&tight, tz);
        assert_eq!(found.len(), 1);
        assert!(found[0].1.contains("10분"), "{}", found[0].1);
    }
}
