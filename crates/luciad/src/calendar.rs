//! 캘린더 쓰기(결정 0033, 소유자 결정: "바로 추가, 삭제는 카드").
//!
//! - **참석자 없는 내 일정**의 만들기·고치기는 로컬 쓰기다: 묻지 않고 하고, 직전 상태를 `calendar_edits`에
//!   남기고, 사람 없는 실행이면 소식으로 알린다.
//! - **참석자가 있는 일정**(만들 때 참석자를 주거나, 고칠 일정에 이미 참석자가 있거나 내가 주최하지 않은 일정)과
//!   **지우기**는 승인 카드(`calendar_write`)다. 카드에 보인 그대로 데몬이 한다. 외부 쓰기라 외부 쓰기까지 열린
//!   실행(대화, 대화 사슬의 후속 조치, 작업)에서만 요청할 수 있다.
//! - 초대 메일은 보내지 않는다(`sendUpdates=none`). Lucia가 만든 일정에는 표지(`extendedProperties.private.lucia`)를
//!   달아, 증분 확인으로 돌아온 변화를 신호 처리가 버리게 한다(결정 0031).
//! - 판단은 모두 데몬이 한다(모델이 "참석자 없음"이라고 말해도 믿지 않고 일정을 다시 읽는다).

use chrono::{DateTime, Duration, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::browser::BoxFut;

/// 승인 카드 종류.
pub const KIND: &str = "calendar_write";
/// 카드 수명.
pub const CARD_HOURS: i64 = 24;
pub const TITLE_MAX: usize = 200;
pub const TEXT_MAX: usize = 2000;
pub const ATTENDEES_MAX: usize = 20;

/// `calendar_create` 인자.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CalendarCreate {
    /// 일정 제목.
    pub title: String,
    /// 시작(현지 시각 `YYYY-MM-DD HH:MM`, 하루 종일이면 `YYYY-MM-DD`).
    pub start: String,
    /// 끝(선택, 같은 꼴). 없으면 1시간(하루 종일이면 그날 하루).
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    /// 메모(출처 메일 제목 등).
    #[serde(default)]
    pub description: Option<String>,
    /// 참석자 메일 주소. 주면 사용자 승인 카드로 간다(초대 메일은 보내지 않는다).
    #[serde(default)]
    pub attendees: Option<Vec<String>>,
}

/// `calendar_update` 인자. 바꿀 칸만 준다.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CalendarUpdate {
    /// 일정 ID(calendar_events의 id).
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `calendar_delete` 인자.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CalendarDelete {
    pub id: String,
    /// 지우는 까닭 한 줄(카드에 보인다).
    pub reason: String,
}

/// 캘린더 쓰기를 쓸 수 있는가(동의 범위). 다시 연결하기 전에는 지침에서 빼고 사용자에게 알리게 한다.
pub fn can_write(store: &lucia_store::Store) -> bool {
    store
        .setting(lucia_store::keys::GOOGLE_SCOPES)
        .ok()
        .flatten()
        .is_some_and(|g| {
            g.split_whitespace()
                .any(|x| x == crate::google_auth::SCOPE_CALENDAR_WRITE)
        })
}

/// 맥락에 붙는 캘린더 쓰기 안내(`calendar_note`).
pub fn write_note(store: &lucia_store::Store) -> String {
    if can_write(store) {
        "<calendar_note>메일 속 마감·약속이 캘린더에 없으면 calendar_create로 넣는다(참석자 없는 내 일정은 바로 된다).</calendar_note>\n".into()
    } else {
        "<calendar_note>캘린더 쓰기 권한이 아직 없다(관리 페이지에서 Google을 다시 연결해야 한다). 캘린더를 고치지 말고, 넣을 만한 일정은 알림에 적는다.</calendar_note>\n".into()
    }
}

/// 시각 하나: 하루 종일(날짜)인가 시각인가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    Day(NaiveDate),
    At(DateTime<Utc>),
}

fn parse(text: &str, tz: Tz) -> Result<When, String> {
    let t = text.trim();
    if let Ok(d) = NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        return Ok(When::Day(d));
    }
    crate::autonomy::parse_when(t, tz).map(When::At)
}

fn when_json(w: When, tz: Tz) -> Value {
    match w {
        When::Day(d) => json!({ "date": d.format("%Y-%m-%d").to_string() }),
        When::At(t) => json!({
            "dateTime": t.with_timezone(&tz).to_rfc3339(),
            "timeZone": tz.name(),
        }),
    }
}

/// 시작·끝 칸을 만든다. 끝이 없으면 1시간(하루 종일이면 다음 날, 끝 날짜는 Google 규칙대로 하루 뒤).
fn span(start: &str, end: Option<&str>, tz: Tz) -> Result<(Value, Value), String> {
    let s = parse(start, tz)?;
    let e = match end.map(str::trim).filter(|e| !e.is_empty()) {
        Some(e) => parse(e, tz)?,
        None => match s {
            When::Day(d) => When::Day(d),
            When::At(t) => When::At(t + Duration::hours(1)),
        },
    };
    let e = match (s, e) {
        (When::Day(_), When::Day(d)) => When::Day(d + Duration::days(1)),
        (When::At(a), When::At(b)) if b > a => When::At(b),
        (When::At(_), When::At(_)) => return Err("끝이 시작보다 뒤여야 한다".into()),
        _ => return Err("시작과 끝은 둘 다 날짜이거나 둘 다 시각이어야 한다".into()),
    };
    if let (When::Day(a), When::Day(b)) = (s, e)
        && b <= a
    {
        return Err("끝 날짜가 시작보다 뒤여야 한다".into());
    }
    Ok((when_json(s, tz), when_json(e, tz)))
}

fn clip(text: &str, max: usize) -> String {
    text.trim().chars().take(max).collect()
}

/// 만들 일정의 본문. Lucia 표지를 단다.
pub fn create_body(args: &CalendarCreate, tz: Tz) -> Result<Value, String> {
    let title = clip(&args.title, TITLE_MAX);
    if title.is_empty() {
        return Err("제목이 필요하다".into());
    }
    let (start, end) = span(&args.start, args.end.as_deref(), tz)?;
    let mut body = json!({
        "summary": title,
        "start": start,
        "end": end,
        "extendedProperties": { "private": { lucia_precheck::google::LUCIA_EVENT_MARK: "1" } },
    });
    if let Some(l) = args
        .location
        .as_deref()
        .map(|l| clip(l, TITLE_MAX))
        .filter(|l| !l.is_empty())
    {
        body["location"] = json!(l);
    }
    if let Some(d) = args
        .description
        .as_deref()
        .map(|d| clip(d, TEXT_MAX))
        .filter(|d| !d.is_empty())
    {
        body["description"] = json!(d);
    }
    let attendees: Vec<String> = args
        .attendees
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|a| a.trim().to_owned())
        .filter(|a| !a.is_empty())
        .collect();
    if attendees.len() > ATTENDEES_MAX {
        return Err(format!("참석자는 {ATTENDEES_MAX}명까지다"));
    }
    if let Some(bad) = attendees
        .iter()
        .find(|a| !lucia_precheck::google::assist::valid_address(a))
    {
        return Err(format!("메일 주소가 아니다: {bad}"));
    }
    if !attendees.is_empty() {
        body["attendees"] = Value::Array(attendees.iter().map(|a| json!({ "email": a })).collect());
    }
    Ok(body)
}

/// 고칠 칸만 담은 본문.
pub fn patch_body(args: &CalendarUpdate, current: &Value, tz: Tz) -> Result<Value, String> {
    let mut body = json!({});
    if let Some(t) = args.title.as_deref() {
        let t = clip(t, TITLE_MAX);
        if t.is_empty() {
            return Err("빈 제목".into());
        }
        body["summary"] = json!(t);
    }
    if args.start.is_some() || args.end.is_some() {
        // 하나만 주면 나머지는 지금 값으로(길이를 지킨다).
        let cur = |k: &str| -> Option<String> {
            current
                .pointer(&format!("/{k}/dateTime"))
                .and_then(Value::as_str)
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string())
                .or_else(|| {
                    current
                        .pointer(&format!("/{k}/date"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
        };
        let start = args
            .start
            .clone()
            .or_else(|| cur("start"))
            .ok_or("지금 시작 시각을 모른다")?;
        let end = match (&args.start, &args.end) {
            (_, Some(e)) => Some(e.clone()),
            // 시작만 옮기면 길이를 지킨다.
            (Some(new_start), None) => {
                let old_s = cur("start").and_then(|s| parse(&s, tz).ok());
                let old_e = cur("end").and_then(|s| parse(&s, tz).ok());
                match (old_s, old_e, parse(new_start, tz)?) {
                    (Some(When::At(a)), Some(When::At(b)), When::At(n)) => Some(
                        (n + (b - a))
                            .with_timezone(&tz)
                            .format("%Y-%m-%d %H:%M")
                            .to_string(),
                    ),
                    _ => None,
                }
            }
            _ => None,
        };
        let (s, e) = span(&start, end.as_deref(), tz)?;
        body["start"] = s;
        body["end"] = e;
    }
    if let Some(l) = args.location.as_deref() {
        body["location"] = json!(clip(l, TITLE_MAX));
    }
    if let Some(d) = args.description.as_deref() {
        body["description"] = json!(clip(d, TEXT_MAX));
    }
    if body.as_object().is_some_and(|o| o.is_empty()) {
        return Err("바꿀 칸이 없다".into());
    }
    Ok(body)
}

/// 이 일정을 묻지 않고 고칠 수 있는가: 참석자가 없고 내가 주최한 일정. 아니면 그 까닭.
pub fn needs_card(event: &Value) -> Option<&'static str> {
    let attendees = event
        .get("attendees")
        .and_then(Value::as_array)
        .is_some_and(|a| {
            a.iter()
                .any(|x| x.get("self").and_then(Value::as_bool) != Some(true))
        });
    if attendees {
        return Some("참석자가 있는 일정");
    }
    let organizer_self = event
        .pointer("/organizer/self")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    (!organizer_self).then_some("내가 주최하지 않은 일정")
}

/// 사람이 읽을 일정 한 줄(제목, 시작).
pub fn line(event: &Value, tz: Tz) -> String {
    let title = event
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("(제목 없음)");
    let start = event
        .pointer("/start/dateTime")
        .and_then(Value::as_str)
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&tz).format("%m-%d %H:%M").to_string())
        .or_else(|| {
            event
                .pointer("/start/date")
                .and_then(Value::as_str)
                .map(|d| format!("{d} 하루 종일"))
        })
        .unwrap_or_default();
    format!("{} ({start})", clip(title, 80))
}

/// 카드 한 장을 저장한다(보이는 것 = 하는 것: 연산·일정 ID·본문을 고정).
#[allow(clippy::too_many_arguments)]
pub fn request_card(
    store: &lucia_store::Store,
    op: &str,
    event_id: Option<&str>,
    body: &Value,
    before: Option<&Value>,
    why: &str,
    run: Option<&str>,
    now: DateTime<Utc>,
    tz: Tz,
) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let shown = match op {
        "create" => line(body, tz),
        _ => before.map(|b| line(b, tz)).unwrap_or_default(),
    };
    let label = match op {
        "create" => "일정 추가",
        "update" => "일정 고치기",
        _ => "일정 지우기",
    };
    store
        .create_approval(
            &lucia_store::NewApproval {
                id: id.clone(),
                kind: KIND.into(),
                payload: json!({
                    "op": op,
                    "event": event_id,
                    "body": body,
                    "before": before,
                    "why": clip(why, 300),
                }),
                summary: format!("{label}: {shown}"),
                run_id: run.map(str::to_owned),
                expires_at: now + Duration::hours(CARD_HOURS),
            },
            now,
        )
        .map_err(|e| format!("저장하지 못했다: {e}"))?;
    Ok(id)
}

/// 직전 상태를 남기고 활동 기록에 적는다.
#[allow(clippy::too_many_arguments)]
pub fn record(
    store: &lucia_store::Store,
    op: &str,
    event_id: &str,
    before: Option<&Value>,
    after: Option<&Value>,
    run: Option<&str>,
    now: DateTime<Utc>,
    tz: Tz,
) {
    let _ = store.add_calendar_edit(op, event_id, before, after, run, now);
    let what = after.or(before).map(|e| line(e, tz)).unwrap_or_default();
    let label = match op {
        "create" => "일정 추가",
        "update" => "일정 고침",
        _ => "일정 지움",
    };
    let _ = store.add_activity(
        "calendar_write",
        &crate::mask::global().mask(&format!("{label}: {what}")),
        run,
        now,
    );
}

/// 카드가 승인되면 데몬이 한다.
pub trait CalendarExec: Send + Sync + 'static {
    fn apply<'a>(
        &'a self,
        op: &'a str,
        event: Option<&'a str>,
        body: &'a Value,
    ) -> BoxFut<'a, Result<Option<Value>, String>>;
}

impl<T: lucia_precheck::google::TokenProvider> CalendarExec
    for lucia_precheck::google::assist::GoogleAssist<T>
{
    fn apply<'a>(
        &'a self,
        op: &'a str,
        event: Option<&'a str>,
        body: &'a Value,
    ) -> BoxFut<'a, Result<Option<Value>, String>> {
        Box::pin(async move {
            match (op, event) {
                ("create", _) => self.create_event(body).await.map(Some),
                ("update", Some(id)) => self.patch_event(id, body).await.map(Some),
                ("delete", Some(id)) => self.delete_event(id).await.map(|_| None),
                _ => return Err("알 수 없는 캘린더 요청".to_owned()),
            }
            .map_err(|e| e.to_string())
        })
    }
}

// ---------- 도구 ----------

/// 도구 하나의 결과에 필요한 것(MCP 서버가 넘긴다).
pub struct Tool<'a> {
    pub assist: &'a lucia_precheck::google::assist::GoogleAssist<crate::google_auth::GoogleAuth>,
    pub store: &'a std::sync::Mutex<lucia_store::Store>,
    pub posts: &'a tokio::sync::mpsc::UnboundedSender<String>,
    pub grants: crate::mcp::RunGrants,
    pub run: Option<String>,
    pub tz: Tz,
}

impl Tool<'_> {
    /// 권한 모드가 엄격이면 참석자 없는 내 일정도 카드로 묻는다(결정 0040).
    fn strict(&self) -> bool {
        self.store.lock().is_ok_and(|s| crate::security::strict(&s))
    }

    /// 카드를 요청할 수 있는 실행인가(외부 쓰기까지 열린 실행).
    fn may_ask(&self) -> bool {
        self.grants
            .allows_grade(lucia_core::Grade::ExternalWrite, false)
    }

    fn card(
        &self,
        op: &str,
        event: Option<&str>,
        body: &Value,
        before: Option<&Value>,
        why: &str,
    ) -> String {
        if !self.may_ask() {
            return format!(
                "{why}이라 사용자 승인이 필요한데, 이 실행에서는 승인 카드를 올릴 수 없다. 사용자에게 알려 대화에서 하게 한다."
            );
        }
        let created = match self.store.lock() {
            Ok(store) => request_card(
                &store,
                op,
                event,
                body,
                before,
                why,
                self.run.as_deref(),
                Utc::now(),
                self.tz,
            ),
            Err(_) => return "저장소를 쓸 수 없다.".into(),
        };
        match created {
            Ok(id) => {
                // 권한 모드(결정 0040): 해제이고 사람이 대화 중이면 카드 없이 데스크가 바로 한다.
                let auto = self.store.lock().is_ok_and(|store| {
                    let auto = crate::security::auto_approve(&store, KIND, self.grants.human);
                    if auto {
                        crate::security::mark_auto(&store, &id);
                    }
                    auto
                });
                let _ = self.posts.send(id);
                crate::security::requested_note(
                    auto,
                    &format!(
                        "{why}이라 사용자에게 Discord 카드로 물었다. 승인되면 데몬이 한다(초대 메일은 보내지 않는다)."
                    ),
                )
            }
            Err(e) => format!("카드를 올리지 못했다: {e}"),
        }
    }

    /// 묻지 않고 한 쓰기를 남기고, 사람 없는 실행이면 소식으로 알린다.
    fn done(&self, op: &str, id: &str, before: Option<&Value>, after: Option<&Value>) {
        let now = Utc::now();
        let Ok(store) = self.store.lock() else {
            return;
        };
        record(
            &store,
            op,
            id,
            before,
            after,
            self.run.as_deref(),
            now,
            self.tz,
        );
        if !self.grants.human {
            let what = after
                .or(before)
                .map(|e| line(e, self.tz))
                .unwrap_or_default();
            let label = if op == "create" { "추가" } else { "고침" };
            let _ = store.add_notice(
                "calendar",
                None,
                &crate::mask::global().mask(&format!("📅 Lucia가 일정을 {label}했습니다: {what}")),
                now,
                now,
            );
        }
    }

    pub async fn create(&self, args: &CalendarCreate) -> String {
        let body = match create_body(args, self.tz) {
            Ok(b) => b,
            Err(e) => return format!("만들지 않았다: {e}"),
        };
        if body.get("attendees").is_some() {
            return self.card("create", None, &body, None, "참석자가 있는 일정");
        }
        if self.strict() {
            return self.card("create", None, &body, None, "권한 모드가 엄격");
        }
        match self.assist.create_event(&body).await {
            Ok(created) => {
                let id = created
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                self.done("create", &id, None, Some(&created));
                format!("일정을 추가했다: {} (id {id}).", line(&created, self.tz))
            }
            Err(e) => crate::mask::global().mask(&format!("추가하지 못했다: {e}")),
        }
    }

    pub async fn update(&self, args: &CalendarUpdate) -> String {
        let id = args.id.trim();
        let current = match self.assist.get_event(id).await {
            Ok(Some(e)) => e,
            Ok(None) => return format!("그 일정이 없다: {id}"),
            Err(e) => return crate::mask::global().mask(&format!("일정을 읽지 못했다: {e}")),
        };
        let body = match patch_body(args, &current, self.tz) {
            Ok(b) => b,
            Err(e) => return format!("고치지 않았다: {e}"),
        };
        if let Some(why) = needs_card(&current) {
            return self.card("update", Some(id), &body, Some(&current), why);
        }
        if self.strict() {
            return self.card(
                "update",
                Some(id),
                &body,
                Some(&current),
                "권한 모드가 엄격",
            );
        }
        match self.assist.patch_event(id, &body).await {
            Ok(after) => {
                self.done("update", id, Some(&current), Some(&after));
                format!("일정을 고쳤다: {}.", line(&after, self.tz))
            }
            Err(e) => crate::mask::global().mask(&format!("고치지 못했다: {e}")),
        }
    }

    pub async fn delete(&self, args: &CalendarDelete) -> String {
        let id = args.id.trim();
        let current = match self.assist.get_event(id).await {
            Ok(Some(e)) => e,
            Ok(None) => return format!("그 일정이 없다: {id}"),
            Err(e) => return crate::mask::global().mask(&format!("일정을 읽지 못했다: {e}")),
        };
        let why = format!("일정 지우기({})", clip(&args.reason, 200));
        self.card("delete", Some(id), &json!({}), Some(&current), &why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz() -> Tz {
        chrono_tz::Asia::Seoul
    }

    #[test]
    fn bodies_carry_the_mark_and_sane_spans() {
        let b = create_body(
            &CalendarCreate {
                title: "과제 마감".into(),
                start: "2026-12-05 18:00".into(),
                ..Default::default()
            },
            tz(),
        )
        .unwrap();
        assert_eq!(b["extendedProperties"]["private"]["lucia"], "1");
        assert_eq!(b["start"]["dateTime"], "2026-12-05T18:00:00+09:00");
        assert_eq!(b["end"]["dateTime"], "2026-12-05T19:00:00+09:00");
        assert!(b.get("attendees").is_none());
        let day = create_body(
            &CalendarCreate {
                title: "원서 접수".into(),
                start: "2026-12-05".into(),
                ..Default::default()
            },
            tz(),
        )
        .unwrap();
        assert_eq!(
            day["end"]["date"], "2026-12-06",
            "하루 종일은 끝 날짜가 다음 날"
        );
        assert!(
            create_body(
                &CalendarCreate {
                    title: "x".into(),
                    start: "2026-12-05 18:00".into(),
                    end: Some("2026-12-05 17:00".into()),
                    ..Default::default()
                },
                tz()
            )
            .is_err()
        );
        let with = create_body(
            &CalendarCreate {
                title: "회의".into(),
                start: "2026-12-05 10:00".into(),
                attendees: Some(vec!["a@example.com".into()]),
                ..Default::default()
            },
            tz(),
        )
        .unwrap();
        assert_eq!(with["attendees"][0]["email"], "a@example.com");
    }

    #[test]
    fn moving_the_start_keeps_the_length() {
        let current = json!({
            "start": {"dateTime": "2026-12-05T10:00:00+09:00"},
            "end": {"dateTime": "2026-12-05T11:30:00+09:00"},
        });
        let p = patch_body(
            &CalendarUpdate {
                id: "e1".into(),
                start: Some("2026-12-06 14:00".into()),
                ..Default::default()
            },
            &current,
            tz(),
        )
        .unwrap();
        assert_eq!(p["end"]["dateTime"], "2026-12-06T15:30:00+09:00");
        assert!(
            patch_body(
                &CalendarUpdate {
                    id: "e1".into(),
                    ..Default::default()
                },
                &current,
                tz()
            )
            .is_err()
        );
    }

    #[test]
    fn attendees_or_foreign_organizers_need_a_card() {
        assert_eq!(needs_card(&json!({"organizer": {"self": true}})), None);
        assert_eq!(
            needs_card(&json!({"attendees": [{"self": true}]})),
            None,
            "나만 있는 참석자 목록"
        );
        assert!(
            needs_card(&json!({"attendees": [{"self": true}, {"email": "b@x.com"}]})).is_some()
        );
        assert!(needs_card(&json!({"organizer": {"self": false}})).is_some());
    }
}
