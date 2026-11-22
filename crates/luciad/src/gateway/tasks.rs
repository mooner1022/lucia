//! 맡은 일 목록(결정 0066, 앱 메서드 `tasks.list`와 번호 없는 `tasks` 프레임). 범위가 필요 없다(소유자의 일이다).
//!
//! 목표(`g-`)·작업(`t-`)·후속 조치(`f-`)를 한 목록에 모아 여섯 가지 상태로 보여 준다. 끝나지 않은 것과
//! 24시간 안에 끝난 것만, 최대 30개다. 목록이 바뀌었는지는 10초마다(앱이 연결되어 있을 때만) 지금 목록의
//! 지문을 이전 것과 견주어 가린다. 쓰는 곳마다 알림을 심지 않으므로 진행 줄이 늘어도 프레임은 10초에 한 번이다.
//!
//! 상태 대응(원천 → `state`):
//!
//! | 원천 | 값 | `state` |
//! |---|---|---|
//! | 목표 | 진행 중(active) | `running` |
//! | 목표 | 사람 차례(waiting) | `waiting` |
//! | 목표 | 완료(done) | `done` |
//! | 목표 | 포기(abandoned) | `cancelled` |
//! | 작업 | 대기(created)·하는 중(running) | `running` |
//! | 작업 | 승인 대기·답 기다림·확인 기다림(reported) | `waiting` (사람 차례) |
//! | 작업 | 완료·실패·취소 | `done`·`failed`·`cancelled` |
//! | 작업 | 만료(expired: 사람이 끝내 답하지 않았다) | `cancelled` |
//! | 후속 조치 | 열림, 시각이 아직 안 됨 | `scheduled` |
//! | 후속 조치 | 열림, 시각이 지남(발사 중) | `running` |
//! | 후속 조치 | 함·취소·실패 | `done`·`cancelled`·`failed` |

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Duration as Span, SecondsFormat, Utc};
use lucia_core::TaskStatus;
use lucia_store::{FollowupStatus, GoalStatus, Store, StoreError};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::Gateway;
use super::methods::{AppMethod, CallContext, CallError, CallFuture};

/// 목록이 바뀌었는지 보는 간격(= `tasks` 프레임을 보내는 가장 짧은 간격).
const POLL: Duration = Duration::from_secs(10);
/// 목록 크기 상한.
const MAX_ITEMS: usize = 30;
/// 끝난 일을 목록에 두는 시간.
const FINISHED_FOR: Span = Span::hours(24);
const TITLE_MAX: usize = 120;
const NOTE_MAX: usize = 200;
/// 끝난 것을 저장소에서 읽는 개수(24시간 안의 것만 남긴다).
const CLOSED_READ: usize = 50;

/// `tasks.list` 메서드를 등록하고 목록이 바뀌는지 보는 일을 띄운다. 한 번만 부른다([`Gateway::start_tasks`]).
pub(super) fn register(gw: &Arc<Gateway>, cancel: CancellationToken) {
    gw.register(Arc::new(TasksList));
    tokio::spawn(watch(Arc::downgrade(gw), cancel, POLL));
}

/// 앱이 하나라도 연결되어 있으면 `every`마다 목록을 만들어, 지난번 보낸 것과 다르면 `tasks` 프레임을 보낸다.
async fn watch(gw: std::sync::Weak<Gateway>, cancel: CancellationToken, every: Duration) {
    let mut last: Option<String> = None;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(every) => {}
        }
        let Some(gw) = gw.upgrade() else { return };
        if !gw.has_connection() {
            continue;
        }
        let list = {
            let store = gw.store.lock().expect("store lock");
            build(&store, Utc::now())
        };
        match list {
            Ok(items) => {
                let fingerprint = Value::Array(items).to_string();
                if last.as_deref() != Some(&fingerprint) {
                    last = Some(fingerprint);
                    gw.broadcast(json!({"type": "tasks"}));
                }
            }
            Err(err) => tracing::warn!(%err, "맡은 일 목록을 만들지 못했다"),
        }
    }
}

impl Gateway {
    /// 지금 연결된 앱이 있는가.
    fn has_connection(&self) -> bool {
        self.conns
            .lock()
            .expect("conns lock")
            .values()
            .any(|c| !c.tx.is_closed())
    }
}

/// `tasks.list`: 끝나지 않은 일과 24시간 안에 끝난 일(최대 30개).
struct TasksList;

impl AppMethod for TasksList {
    fn name(&self) -> &'static str {
        "tasks.list"
    }

    fn call<'a>(&'a self, ctx: CallContext<'a>, _params: Value) -> CallFuture<'a> {
        Box::pin(async move {
            let store = ctx.gateway.store.lock().expect("store lock");
            let tasks = build(&store, Utc::now())
                .map_err(|e| CallError::owned("failed", &e.to_string()))?;
            Ok(json!({"tasks": tasks}))
        })
    }
}

/// 목록의 한 줄(정렬하려고 시각과 끝남 여부를 함께 둔다).
struct Item {
    id: String,
    kind: &'static str,
    title: String,
    state: &'static str,
    note: Option<String>,
    updated_at: DateTime<Utc>,
}

impl Item {
    fn finished(&self) -> bool {
        matches!(self.state, "done" | "failed" | "cancelled")
    }

    fn view(&self) -> Value {
        json!({
            "id": self.id,
            "kind": self.kind,
            "title": self.title,
            "state": self.state,
            "note": self.note,
            "updated_at": self.updated_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        })
    }
}

/// 한 줄로 만들고(줄바꿈·연속 공백을 하나로), 가림 필터를 거쳐, `max`자로 줄인다. 비면 `None`.
fn line(text: &str, max: usize) -> Option<String> {
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.is_empty() {
        return None;
    }
    // 가린 뒤에 줄인다(줄이다가 비밀의 앞부분만 남기지 않게).
    let masked = crate::mask::global().mask(&one);
    Some(if masked.chars().count() > max {
        let mut cut: String = masked.chars().take(max - 1).collect();
        cut.push('…');
        cut
    } else {
        masked
    })
}

/// 쓸 수 있는 목록 전체(끝나지 않은 것 먼저, 그 안에서 최근 것부터, 최대 30개)를 JSON으로.
fn build(store: &Store, now: DateTime<Utc>) -> Result<Vec<Value>, StoreError> {
    let recent = |closed: Option<DateTime<Utc>>, fallback: DateTime<Utc>| {
        closed.unwrap_or(fallback) >= now - FINISHED_FOR
    };
    let mut items: Vec<Item> = Vec::new();

    for g in store.goals(true)? {
        if !g.status.open() && !recent(g.closed_at, g.updated_at) {
            continue;
        }
        let last = store.goal_log(&g.id, 1)?.into_iter().next();
        items.push(Item {
            id: format!("g-{}", g.id),
            kind: "goal",
            title: line(&g.title, TITLE_MAX).unwrap_or_default(),
            state: match g.status {
                GoalStatus::Active => "running",
                GoalStatus::Waiting => "waiting",
                GoalStatus::Done => "done",
                GoalStatus::Abandoned => "cancelled",
            },
            updated_at: last
                .as_ref()
                .map_or(g.updated_at, |l| l.at.max(g.updated_at)),
            note: last.and_then(|l| line(&l.text, NOTE_MAX)),
        });
    }

    let tasks = store
        .open_tasks()?
        .into_iter()
        .chain(store.closed_tasks(CLOSED_READ)?);
    for t in tasks {
        if t.status.is_final() && !recent(t.closed_at, t.updated_at) {
            continue;
        }
        // 확인을 기다리거나 끝난 작업은 결과가 마지막 말이고, 아니면 가장 최근 진행 기록이다.
        let result = (t.status == TaskStatus::Reported || t.status.is_final())
            .then(|| t.result.as_deref().and_then(|r| line(r, NOTE_MAX)))
            .flatten();
        let note = match result {
            Some(r) => Some(r),
            None => store
                .task_events(&t.id)?
                .iter()
                .rev()
                .find_map(|e| line(&e.note, NOTE_MAX)),
        };
        items.push(Item {
            id: format!("t-{}", t.id),
            kind: "task",
            title: line(&t.title, TITLE_MAX).unwrap_or_default(),
            state: match t.status {
                TaskStatus::Created | TaskStatus::Running => "running",
                TaskStatus::WaitingApproval | TaskStatus::WaitingInput | TaskStatus::Reported => {
                    "waiting"
                }
                TaskStatus::Done => "done",
                TaskStatus::Failed => "failed",
                TaskStatus::Cancelled | TaskStatus::Expired => "cancelled",
            },
            updated_at: t.updated_at,
            note,
        });
    }

    let followups = store
        .open_followups()?
        .into_iter()
        .chain(store.closed_followups(CLOSED_READ)?);
    for f in followups {
        if f.status != FollowupStatus::Open && !recent(f.closed_at, f.created_at) {
            continue;
        }
        items.push(Item {
            id: format!("f-{}", f.id),
            kind: "followup",
            title: line(&f.task, TITLE_MAX).unwrap_or_default(),
            state: match f.status {
                FollowupStatus::Open if f.due_at > now => "scheduled",
                FollowupStatus::Open => "running",
                FollowupStatus::Done => "done",
                FollowupStatus::Cancelled => "cancelled",
                FollowupStatus::Failed => "failed",
            },
            updated_at: f.closed_at.unwrap_or(f.created_at),
            note: f.result.as_deref().and_then(|r| line(r, NOTE_MAX)),
        });
    }

    items.sort_by(|a, b| {
        a.finished()
            .cmp(&b.finished())
            .then(b.updated_at.cmp(&a.updated_at))
            .then(a.id.cmp(&b.id))
    });
    items.truncate(MAX_ITEMS);
    Ok(items.iter().map(Item::view).collect())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use lucia_core::Initiator;
    use lucia_store::{NewFollowup, NewGoal, NewTask};

    use super::super::methods::dispatch;
    use super::*;

    fn gateway() -> (Arc<Gateway>, lucia_store::Client) {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        store
            .lock()
            .unwrap()
            .create_client("cl-1", "시험 앱", "app", "hash", Utc::now())
            .unwrap();
        let client = store
            .lock()
            .unwrap()
            .active_client("cl-1")
            .unwrap()
            .unwrap();
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let (ptx, _prx) = tokio::sync::mpsc::channel(8);
        (Gateway::new(store, itx, ptx), client)
    }

    fn goal(store: &Store, id: &str, title: &str, now: DateTime<Utc>) {
        store
            .create_goal(
                id,
                &NewGoal {
                    title,
                    purpose: "목적",
                    success: "성공",
                    deadline: None,
                    interests: "",
                    review_every_hours: 24,
                    next_review_at: now + Span::hours(24),
                    conversation: None,
                },
                now,
            )
            .unwrap()
            .unwrap();
    }

    fn task(store: &Store, id: &str, title: &str, now: DateTime<Utc>) {
        store
            .create_task(
                &NewTask {
                    id,
                    title,
                    brief: "본문",
                    initiator: Initiator::Human,
                    source: "chat",
                    run_id: None,
                },
                now,
            )
            .unwrap();
    }

    fn followup(store: &Store, id: &str, text: &str, due: DateTime<Utc>, now: DateTime<Utc>) {
        store
            .create_followup(
                &NewFollowup {
                    id,
                    due_at: due,
                    task: text,
                    detail: "",
                    conversation: None,
                    run_id: None,
                    origin: "chat",
                },
                now,
            )
            .unwrap();
    }

    fn by_id<'a>(list: &'a [Value], id: &str) -> &'a Value {
        list.iter()
            .find(|v| v["id"] == id)
            .unwrap_or_else(|| panic!("{id}가 없다: {list:?}"))
    }

    #[test]
    fn each_kind_maps_to_its_state_and_note() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        let ago = |m: i64| now - Span::minutes(m);

        goal(&store, "goal-1", "분기 보고서 초안", ago(90));
        store
            .add_goal_log(
                "goal-1",
                "progress",
                "자료 3개 가운데\n2개를 읽었어요",
                ago(10),
            )
            .unwrap();
        goal(&store, "goal-2", "사람 차례인 목표", ago(80));
        store
            .set_goal_status("goal-2", GoalStatus::Waiting, ago(70))
            .unwrap();
        goal(&store, "goal-3", "끝낸 목표", ago(60));
        store
            .set_goal_status("goal-3", GoalStatus::Done, ago(50))
            .unwrap();
        goal(&store, "goal-4", "포기한 목표", ago(60));
        store
            .set_goal_status("goal-4", GoalStatus::Abandoned, ago(45))
            .unwrap();

        task(&store, "task-1", "자료 정리", ago(100));
        store
            .move_task("task-1", TaskStatus::Running, "시작", ago(95))
            .unwrap();
        store
            .note_task("task-1", "두 번째 파일을 읽는 중", ago(5))
            .unwrap();
        task(&store, "task-2", "승인 기다리는 작업", ago(100));
        store
            .move_task("task-2", TaskStatus::Running, "시작", ago(99))
            .unwrap();
        store
            .move_task(
                "task-2",
                TaskStatus::WaitingApproval,
                "메일 승인 대기",
                ago(98),
            )
            .unwrap();
        task(&store, "task-3", "확인 기다리는 작업", ago(100));
        store
            .move_task("task-3", TaskStatus::Running, "시작", ago(99))
            .unwrap();
        store
            .move_task("task-3", TaskStatus::Reported, "보고함", ago(40))
            .unwrap();
        store
            .set_task_result("task-3", "표 두 개를 만들었다", ago(40))
            .unwrap();
        task(&store, "task-4", "끝낸 작업", ago(100));
        store
            .move_task("task-4", TaskStatus::Running, "시작", ago(99))
            .unwrap();
        store
            .move_task("task-4", TaskStatus::Failed, "VM 오류", ago(30))
            .unwrap();
        task(&store, "task-5", "취소한 작업", ago(100));
        store
            .move_task("task-5", TaskStatus::Cancelled, "사용자가 취소", ago(25))
            .unwrap();
        task(&store, "task-6", "대기 작업", ago(20));

        followup(
            &store,
            "fup-1",
            "치과 예약 확인",
            now + Span::hours(3),
            ago(120),
        );
        followup(&store, "fup-2", "이미 지난 약속", ago(1), ago(120));
        followup(&store, "fup-3", "한 약속", ago(30), ago(120));
        store
            .close_followup("fup-3", FollowupStatus::Done, "확인했어요", ago(20))
            .unwrap();
        followup(&store, "fup-4", "실패한 약속", ago(30), ago(120));
        store
            .close_followup("fup-4", FollowupStatus::Failed, "답이 없었다", ago(15))
            .unwrap();
        followup(
            &store,
            "fup-5",
            "취소한 약속",
            now + Span::hours(1),
            ago(120),
        );
        store
            .close_followup("fup-5", FollowupStatus::Cancelled, "필요 없음", ago(10))
            .unwrap();

        let list = build(&store, now).unwrap();
        let state = |id: &str| by_id(&list, id)["state"].as_str().unwrap().to_owned();
        let kind = |id: &str| by_id(&list, id)["kind"].as_str().unwrap().to_owned();
        // 목표
        assert_eq!(
            (state("g-goal-1"), kind("g-goal-1")),
            ("running".into(), "goal".into())
        );
        assert_eq!(state("g-goal-2"), "waiting");
        assert_eq!(state("g-goal-3"), "done");
        assert_eq!(state("g-goal-4"), "cancelled");
        // 작업
        assert_eq!(
            (state("t-task-1"), kind("t-task-1")),
            ("running".into(), "task".into())
        );
        assert_eq!(state("t-task-2"), "waiting");
        assert_eq!(state("t-task-3"), "waiting");
        assert_eq!(state("t-task-4"), "failed");
        assert_eq!(state("t-task-5"), "cancelled");
        assert_eq!(state("t-task-6"), "running");
        // 후속 조치
        assert_eq!(
            (state("f-fup-1"), kind("f-fup-1")),
            ("scheduled".into(), "followup".into())
        );
        assert_eq!(state("f-fup-2"), "running");
        assert_eq!(state("f-fup-3"), "done");
        assert_eq!(state("f-fup-4"), "failed");
        assert_eq!(state("f-fup-5"), "cancelled");

        // 한 줄: 목표는 마지막 진행 기록, 작업은 마지막 기록(확인 기다림·끝남은 결과), 후속 조치는 결과.
        assert_eq!(
            by_id(&list, "g-goal-1")["note"],
            "자료 3개 가운데 2개를 읽었어요"
        );
        assert_eq!(by_id(&list, "g-goal-1")["title"], "분기 보고서 초안");
        assert_eq!(by_id(&list, "t-task-1")["note"], "두 번째 파일을 읽는 중");
        assert_eq!(by_id(&list, "t-task-2")["note"], "메일 승인 대기");
        assert_eq!(by_id(&list, "t-task-3")["note"], "표 두 개를 만들었다");
        assert_eq!(by_id(&list, "t-task-4")["note"], "VM 오류");
        assert_eq!(by_id(&list, "f-fup-3")["note"], "확인했어요");
        assert!(by_id(&list, "f-fup-1")["note"].is_null());
        // 시각은 초 단위 RFC 3339(Z).
        let at = by_id(&list, "t-task-1")["updated_at"].as_str().unwrap();
        assert!(at.ends_with('Z') && !at.contains('.'), "{at}");

        // 정렬: 끝나지 않은 것이 앞, 그 안에서는 최근 것부터. 끝난 것은 그 뒤.
        let states: Vec<&str> = list.iter().map(|v| v["state"].as_str().unwrap()).collect();
        let first_finished = states
            .iter()
            .position(|s| matches!(*s, "done" | "failed" | "cancelled"))
            .unwrap();
        assert!(
            states[first_finished..]
                .iter()
                .all(|s| matches!(*s, "done" | "failed" | "cancelled"))
        );
        let times: Vec<&str> = list[..first_finished]
            .iter()
            .map(|v| v["updated_at"].as_str().unwrap())
            .collect();
        assert!(times.windows(2).all(|w| w[0] >= w[1]), "{times:?}");
        assert_eq!(list[0]["id"], "t-task-1", "가장 최근에 바뀐 끝나지 않은 일");
    }

    #[test]
    fn finished_items_older_than_a_day_and_extra_items_are_left_out() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        let ago = |h: i64| now - Span::hours(h);

        // 25시간 전에 끝난 것은 뺀다. 23시간 전은 둔다. 끝나지 않은 것은 오래되어도 둔다.
        for (id, closed_h) in [("task-old", 25), ("task-new", 23)] {
            task(&store, id, id, ago(60));
            store
                .move_task(id, TaskStatus::Running, "시작", ago(59))
                .unwrap();
            store
                .move_task(id, TaskStatus::Failed, "실패", ago(closed_h))
                .unwrap();
        }
        task(&store, "task-open", "오래된 열린 작업", ago(500));
        goal(&store, "goal-old", "오래전에 끝낸 목표", ago(100));
        store
            .set_goal_status("goal-old", GoalStatus::Done, ago(30))
            .unwrap();
        followup(&store, "fup-old", "오래전 약속", ago(40), ago(50));
        store
            .close_followup("fup-old", FollowupStatus::Done, "함", ago(26))
            .unwrap();
        followup(&store, "fup-new", "어제 약속", ago(3), ago(50));
        store
            .close_followup("fup-new", FollowupStatus::Done, "함", ago(2))
            .unwrap();

        let list = build(&store, now).unwrap();
        let ids: Vec<&str> = list.iter().map(|v| v["id"].as_str().unwrap()).collect();
        assert!(
            ids.contains(&"t-task-new") && ids.contains(&"t-task-open"),
            "{ids:?}"
        );
        assert!(ids.contains(&"f-fup-new"), "{ids:?}");
        assert!(!ids.contains(&"t-task-old"), "{ids:?}");
        assert!(!ids.contains(&"g-goal-old"), "{ids:?}");
        assert!(!ids.contains(&"f-fup-old"), "{ids:?}");

        // 30개를 넘으면 끝난 것부터 잘리고, 끝나지 않은 것이 먼저 남는다.
        for i in 0..40 {
            task(
                &store,
                &format!("task-x{i:02}"),
                "작업",
                now - Span::minutes(i),
            );
        }
        let list = build(&store, now).unwrap();
        assert_eq!(list.len(), MAX_ITEMS);
        assert!(
            list.iter().all(|v| v["state"] == "running"),
            "끝난 것은 밀려났다"
        );
    }

    #[test]
    fn titles_and_notes_are_one_line_masked_and_clipped() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        crate::mask::global().register("시험용-비밀-값-1234567890");
        task(
            &store,
            "task-1",
            &format!("{}\n둘째 줄", "제목".repeat(100)),
            now,
        );
        store
            .note_task(
                "task-1",
                "키는 시험용-비밀-값-1234567890 이고\n\n  줄이 바뀐다 "
                    .repeat(20)
                    .as_str(),
                now,
            )
            .unwrap();
        let list = build(&store, now).unwrap();
        let item = by_id(&list, "t-task-1");
        let title = item["title"].as_str().unwrap();
        let note = item["note"].as_str().unwrap();
        assert_eq!(title.chars().count(), TITLE_MAX);
        assert!(title.ends_with('…') && !title.contains('\n'));
        assert_eq!(note.chars().count(), NOTE_MAX);
        assert!(
            !note.contains('\n') && !note.contains("시험용-비밀-값"),
            "{note}"
        );
    }

    #[tokio::test]
    async fn the_method_is_registered_and_answers_with_the_list() {
        let (gw, client) = gateway();
        gw.start_tasks(CancellationToken::new());
        assert!(gw.method_names(&client).contains(&"tasks.list"));
        {
            let store = gw.store.lock().unwrap();
            task(&store, "task-1", "자료 정리", Utc::now());
        }
        let r = dispatch(&gw, &client, "tasks.list", Value::Null)
            .await
            .unwrap();
        let tasks = r["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["id"], "t-task-1");
        assert_eq!(tasks[0]["kind"], "task");
        assert_eq!(tasks[0]["state"], "running");
    }

    /// 앱이 연결되어 있을 때만 보고, 바뀐 것이 있을 때만 `tasks` 프레임을 한 번 보낸다.
    #[tokio::test]
    async fn a_tasks_frame_is_sent_only_when_the_list_changed_and_an_app_is_connected() {
        let (gw, _client) = gateway();
        let cancel = CancellationToken::new();
        tokio::spawn(watch(
            Arc::downgrade(&gw),
            cancel.clone(),
            Duration::from_millis(40),
        ));
        let store = gw.store.clone();
        let wait = |ms: u64| tokio::time::sleep(Duration::from_millis(ms));

        // 앱이 없으면 바뀌어도 보내지 않고(볼 곳이 없다), 연결되면 처음 한 번 알린다.
        {
            let s = store.lock().unwrap();
            task(&s, "task-1", "자료 정리", Utc::now());
        }
        wait(120).await;
        let (_, mut rx) = gw.attach_for_test("cl-1");
        wait(150).await;
        assert_eq!(rx.try_recv().unwrap(), json!({"type": "tasks"}));
        // 그대로면 더 보내지 않는다.
        wait(200).await;
        assert!(rx.try_recv().is_err());

        // 진행 줄이 바뀌면 한 번 보낸다.
        {
            let s = store.lock().unwrap();
            s.note_task("task-1", "두 번째 파일을 읽는 중", Utc::now())
                .unwrap();
        }
        wait(200).await;
        assert_eq!(rx.try_recv().unwrap(), json!({"type": "tasks"}));
        assert!(rx.try_recv().is_err(), "한 번만");

        // 끝나도(상태가 바뀌어도) 한 번.
        {
            let s = store.lock().unwrap();
            s.move_task("task-1", TaskStatus::Cancelled, "취소", Utc::now())
                .unwrap();
        }
        wait(200).await;
        assert_eq!(rx.try_recv().unwrap(), json!({"type": "tasks"}));

        cancel.cancel();
        wait(120).await;
        {
            let s = store.lock().unwrap();
            task(&s, "task-2", "또 하나", Utc::now());
        }
        wait(200).await;
        assert!(rx.try_recv().is_err(), "멈춘 뒤에는 보내지 않는다");
    }
}
