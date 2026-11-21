//! 작업(결정 0014): MCP 도구·엔진·관리 화면이 함께 쓰는 것.
//!
//! - 작업은 사람이 채팅에서 맡긴다(`task_start`). 데몬이 레코드를 만들고 작업 줄에 넣는다.
//! - 작업 줄은 채팅과 따로 한 번에 하나씩 돈다. 작업마다 따로 Claude 세션(작업 세션)을 쓴다.
//! - 진행은 작업 스레드(채널이 스레드를 지원하고 관리 페이지에 작업 채널을 정했을 때) 또는 소유자 DM으로 간다.
//! - 결과를 보고하면 "확인 기다림"이 되고, 완료는 사람이 카드에서 확인한다.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use lucia_core::{ConversationId, Outbound, TaskStatus};
use lucia_store::Store;
use tokio::sync::mpsc;

/// 작업 줄에 넣는 일.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskJob {
    pub id: String,
    /// 사람이 더한 말(이어 하기). 없으면 처음 시작이다.
    pub input: Option<String>,
}

/// 작업 진행 알림(작업 세션의 `task_update`). 작업 줄이 실행 중이어도 바로 보내도록 따로 처리한다.
#[derive(Debug, Clone)]
pub struct TaskNote {
    pub id: String,
    pub text: String,
}

/// 작업 도구가 쓰는 것.
#[derive(Clone)]
pub struct TaskAccess {
    pub store: Arc<Mutex<Store>>,
    pub jobs: mpsc::UnboundedSender<TaskJob>,
    pub notes: mpsc::UnboundedSender<TaskNote>,
    /// 개발 작업(결정 0020). 없으면 dev_start·dev_push가 안내만 한다.
    pub dev: Option<Arc<crate::dev::DevSandboxes>>,
    /// 승인 데스크에 카드를 올리는 통로(push 승인).
    pub posts: Option<mpsc::UnboundedSender<String>>,
}

/// 끝나지 않은 작업 수 상한(사람이 맡기므로 넉넉히, 잊힌 작업이 쌓이지 않게) [제안].
pub const OPEN_MAX: usize = 20;
/// 이만큼 움직임이 없는 "답 기다림·확인 기다림" 작업은 만료한다 [제안].
pub const STALE_DAYS: i64 = 7;
/// 완료 확인 카드의 유효 시간.
pub const CONFIRM_TTL_DAYS: i64 = 7;
pub const TITLE_MAX: usize = 80;
pub const BRIEF_MAX: usize = 4_000;
/// 관리 페이지에서 정하는 작업 채널(스레드를 열 부모 대화). 비우면 소유자 DM에서 한다.
pub const CHANNEL_KEY: &str = "tasks.channel";

pub fn new_task_id() -> String {
    format!("task-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

/// 작업 세션 실행의 ID(`task-xxxxxxxx-r…`).
pub fn new_run_id(task_id: &str) -> String {
    format!(
        "{task_id}-r{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

/// 실행 ID에서 작업 ID를 꺼낸다.
pub fn task_of_run(run_id: &str) -> Option<&str> {
    let rest = run_id.strip_prefix("task-")?;
    let end = rest.find('-').unwrap_or(rest.len());
    (end > 0).then(|| &run_id[..5 + end])
}

/// 진행 알림을 작업의 대화로 보낸다(작업 줄과 따로 돈다).
pub async fn run_notes<C: lucia_channel::ChannelAdapter>(
    store: Arc<Mutex<Store>>,
    channel: Arc<C>,
    fallback: Option<ConversationId>,
    mut notes: mpsc::UnboundedReceiver<TaskNote>,
) {
    while let Some(note) = notes.recv().await {
        let target = {
            let store = store.lock().expect("store lock");
            let task = store.task(&note.id).ok().flatten();
            task.and_then(|t| {
                let conv = t
                    .conversation
                    .clone()
                    .map(ConversationId)
                    .or_else(|| fallback.clone())?;
                Some((conv, t.thread, t.title))
            })
        };
        let Some((conversation, thread, title)) = target else {
            continue;
        };
        let body = if thread {
            note.text.clone()
        } else {
            format!("[작업: {title}] {}", note.text)
        };
        let _ = channel
            .send(Outbound::Note {
                conversation,
                body: crate::mask::global().mask(&body),
            })
            .await;
    }
}

/// 사람에게 보이는 한 줄(목록·맥락용).
pub fn line(t: &lucia_store::Task, tz: chrono_tz::Tz) -> String {
    format!(
        "{} · {} · {} · {}",
        t.id,
        t.status.label(),
        t.updated_at.with_timezone(&tz).format("%m-%d %H:%M"),
        t.title
    )
}

/// 사람이 말을 더할 수 있는 상태인가.
pub fn accepts_input(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Created
            | TaskStatus::Running
            | TaskStatus::WaitingInput
            | TaskStatus::WaitingApproval
            | TaskStatus::Reported
    )
}

/// 오래 멈춘 작업의 기준 시각.
pub fn stale_before(now: DateTime<Utc>) -> DateTime<Utc> {
    now - chrono::Duration::days(STALE_DAYS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_point_back_to_their_task() {
        let id = new_task_id();
        let run = new_run_id(&id);
        assert_eq!(task_of_run(&run), Some(id.as_str()));
        assert_eq!(task_of_run(&format!("{run}-report")), Some(id.as_str()));
        assert_eq!(task_of_run("chat-1"), None);
        assert_eq!(task_of_run("task-"), None);
    }
}
