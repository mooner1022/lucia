//! 사람 없는 실행의 맥락(결정 0028·0030): 엔진이 실행을 띄우기 전에 적어 두고, MCP 도구가 실행 ID로 찾는다.
//!
//! - 무엇을 위해 돌았나: 발사한 후속 조치, 깨운 반복 루틴(자기 것만 줄일 수 있다, 결정 0028 I2).
//! - 이어 하기 정보: 승인 카드가 닫힌 뒤 같은 사슬로 다시 걸 후속 조치의 메모·출처·대화(결정 0030).
//! - 메모리에만 둔다. 카드에는 만들 때 이어 하기 정보를 복사해 넣으므로 재시작해도 이어 하기는 남는다.
//! - 코드 다리(`-code`·`-skill`·`-routine`)와 다시 청하기(`-report`·`-retry`)의 실행 ID는 부모 실행으로 찾는다.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// 기억해 두는 실행 수(오래된 것부터 버린다).
const KEEP: usize = 200;
/// 이어 하기 한 사슬에서 다시 맡길 수 있는 횟수 [제안].
pub const RESUME_DEPTH_MAX: u32 = 3;
/// 이어 하기 후속 조치의 맥락(detail)에 붙는 표지. 뒤에 깊이가 온다.
pub const RESUME_DETAIL: &str = "이어 하기 깊이 ";

/// 후속 조치 맥락에서 이어 하기 깊이(없으면 0).
pub fn depth_of(detail: &str) -> u32 {
    detail
        .strip_prefix(RESUME_DETAIL)
        .and_then(|d| d.trim().parse().ok())
        .unwrap_or(0)
}

/// 승인 뒤 다시 걸 후속 조치(결정 0030).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resume {
    /// 원래 할 일(Lucia가 남긴 메모 또는 루틴의 할 일).
    pub task: String,
    /// 사슬의 출처(`chat`·`followup`·`background`·`user`). 이어 하기도 같은 출처로 건다.
    pub origin: String,
    /// 결과를 보낼 대화.
    pub conversation: Option<String>,
    /// 이 사슬에서 이미 이어 한 횟수.
    pub depth: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunCtx {
    /// 이 실행이 발사한 후속 조치.
    pub followup: Option<String>,
    /// 이 실행을 깨운 반복 루틴.
    pub routine: Option<String>,
    /// 사이트 위임 전에 만든 후속 조치(결정 0029: 예전처럼 어디든 연다).
    pub legacy: bool,
    pub resume: Option<Resume>,
    /// 이 실행이 맡은 목표(결정 0032): 목표 점검, 또는 목표의 하위 항목(후속 조치·루틴)이 깨운 실행.
    pub goal: Option<String>,
    /// 목표 점검이다: 그 목표의 하위 루틴·후속 조치를 만들고 고칠 수 있다(범위는 넓어지지 않는다).
    pub goal_review: bool,
    /// 사용자가 말해서 돈 대화 실행이면 그 대화(결정 0066): 앱 기능의 `when: "here"`는 그 앱의 대화일 때만 부른다.
    pub chat: Option<String>,
}

/// 실행 ID → 맥락, 그리고 넣은 순서(오래된 것부터 버리려고).
type Table = (HashMap<String, RunCtx>, Vec<String>);

fn table() -> &'static Mutex<Table> {
    static T: OnceLock<Mutex<Table>> = OnceLock::new();
    T.get_or_init(Mutex::default)
}

/// 실행을 띄우기 전에 적는다.
pub fn register(run_id: &str, ctx: RunCtx) {
    let mut t = table().lock().expect("runctx lock");
    if t.0.insert(run_id.to_owned(), ctx).is_none() {
        t.1.push(run_id.to_owned());
    }
    while t.1.len() > KEEP {
        let old = t.1.remove(0);
        t.0.remove(&old);
    }
}

/// 부모 실행 ID(코드 다리·다시 청하기의 꼬리를 뗀다).
pub fn parent(run_id: &str) -> &str {
    let mut id = run_id;
    for tail in ["-code", "-skill", "-routine", "-report", "-retry"] {
        if let Some(p) = id.strip_suffix(tail) {
            id = p;
        }
    }
    id
}

pub fn get(run_id: &str) -> Option<RunCtx> {
    let t = table().lock().expect("runctx lock");
    t.0.get(run_id).or_else(|| t.0.get(parent(run_id))).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_find_their_parent() {
        register(
            "followup-abc",
            RunCtx {
                followup: Some("followup-1".into()),
                ..RunCtx::default()
            },
        );
        for id in [
            "followup-abc",
            "followup-abc-code",
            "followup-abc-retry",
            "followup-abc-report",
        ] {
            assert_eq!(
                get(id).and_then(|c| c.followup).as_deref(),
                Some("followup-1"),
                "{id}"
            );
        }
        assert!(get("followup-zzz").is_none());
    }
}
