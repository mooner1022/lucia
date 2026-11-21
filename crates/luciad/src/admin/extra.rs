//! 화면마다 다른 덧붙임(`extra`, 결정 0061). 스냅숏에 없는 것(기억 항목, 스킬, 캐릭터 파일, 넘겨받기 상태)을
//! 처리기가 여기 함수로 만든다. 화면 쪽은 `extra.view`로 그릴 것을 가른다. 시험 데이터도 같은 함수를 쓴다.

use std::time::Duration;

use serde_json::{Value, json};

use super::derive::{memory_entry_ui, memory_revision_ui, skill_revision_ui, skill_ui, with_ui};
use super::snapshot::ActivityFilter;

/// 활동: 고른 거르개.
pub fn activity(filter: ActivityFilter) -> Value {
    json!({ "filter": filter.key() })
}

/// 기억 목록. `query`는 찾는 말(없으면 빈 글).
pub fn memory_list(query: &str) -> Value {
    json!({ "view": "list", "query": query })
}

/// 기억 항목 하나와 변경 기록. 항목에 `ui.section_key`·`ui.section_label`, 개정마다 `ui.author_label`·`ui.origin`·`ui.deleted`.
pub fn memory_entry(
    entry: &lucia_store::MemoryEntry,
    history: &[lucia_store::MemoryRevision],
) -> Value {
    let history: Vec<Value> = history
        .iter()
        .map(|r| with_ui(r, memory_revision_ui(r)))
        .collect();
    json!({
        "view": "entry",
        "entry": with_ui(entry, memory_entry_ui(entry)),
        "history": history,
    })
}

pub fn memory_missing() -> Value {
    json!({ "view": "missing" })
}

/// 스킬 목록. 스킬마다 `ui`(상태 이름·결, 쓸 수 있나, 흔적 한 줄, 출처, 상태에 맞는 단추).
pub fn skills(list: &[lucia_store::Skill]) -> Value {
    let skills: Vec<Value> = list.iter().map(|s| with_ui(s, skill_ui(s))).collect();
    json!({ "view": "list", "skills": skills })
}

/// 스킬 하나와 개정. 스킬에 목록과 같은 `ui`, 개정마다 `ui.author_label`.
pub fn skill_detail(skill: &lucia_store::Skill, revisions: &[lucia_store::SkillRevision]) -> Value {
    let revisions: Vec<Value> = revisions
        .iter()
        .map(|r| with_ui(r, skill_revision_ui(r)))
        .collect();
    json!({
        "view": "detail",
        "skill": with_ui(skill, skill_ui(skill)),
        "revisions": revisions,
    })
}

/// 캐릭터: 파일(또는 읽지 못한 까닭), 변경 기록, 메시지 간격, 주 모델.
/// 저장 폼이 보낼 `fingerprint`(파일이 그새 바뀌었는지 보는 지문), 크기 상한 `max_bytes`, 지금 내용의 바이트 수
/// `content_bytes`(문자열의 `.len()`은 UTF-8 바이트라 브라우저의 글자 수와 다르다), 개정마다 `ui.bytes`를 싣는다.
pub fn character(
    loaded: &Result<(String, String), String>,
    history: &[lucia_store::CharacterRevision],
    pace: crate::segments::Pace,
    models: &crate::models::MainModel,
) -> Value {
    let (path, content, error) = match loaded {
        Ok((path, content)) => (Some(path), Some(content), None),
        Err(e) => (None, None, Some(e)),
    };
    let history: Vec<Value> = history
        .iter()
        .map(|r| with_ui(r, json!({ "bytes": r.content.len() })))
        .collect();
    json!({
        "view": "edit",
        "path": path,
        "content": content,
        "error": error,
        "fingerprint": content.map(|c| crate::character::fingerprint(c)),
        "content_bytes": content.map(|c| c.len()),
        "max_bytes": crate::character::MAX_BYTES,
        "history": history,
        "pace": pace.key(),
        "paces": crate::segments::Pace::ALL
            .iter()
            .map(|p| json!({ "key": p.key(), "label": p.label() }))
            .collect::<Vec<_>>(),
        "model": {
            "picked": models.picked().map(|c| c.key),
            "label": models.label(),
            "config_default": models.config_default(),
            "choices": crate::models::CHOICES,
            "change_note": crate::models::CHANGE_NOTE,
        },
    })
}

pub fn character_revision(revision: &lucia_store::CharacterRevision, is_current: bool) -> Value {
    json!({ "view": "revision", "revision": revision, "is_current": is_current })
}

pub fn character_revision_missing() -> Value {
    json!({ "view": "revision_missing" })
}

/// 브라우저: 저장된 로그인과 위임(스냅숏만 쓴다).
pub fn browser_sites() -> Value {
    json!({ "view": "sites" })
}

/// 화면 넘겨받기. `refresh`가 있으면 화면이 그 초마다 다시 묻는다.
pub fn handover(
    available: bool,
    left: Option<Duration>,
    tabs: &[crate::browser::Tab],
    tab: Option<&str>,
    auto: bool,
    refresh: Option<u32>,
) -> Value {
    json!({
        "view": "handover",
        "available": available,
        "left_secs": left.map(|d| d.as_secs()),
        "tabs": tabs,
        "tab": tab,
        "auto": auto,
        "refresh": refresh,
    })
}
