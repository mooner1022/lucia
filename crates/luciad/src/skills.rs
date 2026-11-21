//! 스킬(결정 0018, 명세 §성공한 작업의 도구화): 성공한 작업의 절차를 다음에 더 빨리 하도록 남긴 메모.
//!
//! - 넣는 길: Claude Code의 기본 스킬 로딩이 아니라, 데몬이 맥락에 색인(`<skills>`)을 넣고 Lucia가
//!   `skill_read`로 본문을 읽는다. 상주 샌드박스의 훅은 데몬 MCP 도구와 이름을 정한 내장 도구만 통과시키고,
//!   새 마운트는 샌드박스를 만들 때만 붙는다(`lucia`는 다시 만들지 않는다).
//! - 파일: 본문의 원본은 저장소 개정이고, `<data_dir>/skills/<이름>/SKILL.md`(표준 앞머리)로도 쓰고 git으로
//!   남긴다(명세 "모든 개정은 git으로"). 프로젝트 레포가 아니다(그 레포는 VM에 마운트된다).
//! - 권한: 스킬은 권한을 넓히지 못한다. 실행의 범위([`crate::tools::Policy`])는 그대로다.
//!   외부 쓰기나 사이트 열기가 들어 있는 스킬은 사람이 승인한 뒤에만 쓴다. 그 판단은 모델이 밝힌 것이 아니라
//!   원래 작업이 실제로 한 일(도구 흔적)과 본문에 적힌 주소로 한다.
//! - 승격: 시험 중인 스킬은 그것을 읽은 작업을 사람이 [완료]로 확인할 때만 성공으로 센다(스스로 보고한 성공은
//!   세지 않는다). 두 번이면 활성.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use lucia_store::RunTool;
use serde_json::json;

/// 시험 → 활성에 필요한 확인된 성공 수 [제안].
pub const PROMOTE_AT: i64 = 2;
/// 이만큼 안 쓰면 오래됨, 그 뒤 이만큼 더 안 쓰면 보관 [제안].
pub const STALE_DAYS: i64 = 60;
pub const ARCHIVE_DAYS: i64 = 60;
/// 회고할 만한 작업: 도구를 이만큼 넘게 불렀거나, 턴이 이만큼 넘었거나, 사람이 말을 더했다 [제안].
pub const RETRO_MIN_TOOLS: u64 = 3;
pub const RETRO_MIN_TURNS: u64 = 8;
/// 이보다 큰 작업 세션은 회고하지 않는다(다시 읽는 비용) [제안].
pub const RETRO_MAX_CONTEXT: u64 = 150_000;

/// 원래 작업이 실제로 한 일: 읽기를 넘는 도구와 연 호스트.
pub fn footprint_from_trace(trace: &[RunTool]) -> serde_json::Value {
    let mut tools = BTreeSet::new();
    let mut hosts = BTreeSet::new();
    for t in trace {
        let grade = crate::tools::spec(&t.name).map(|s| s.grade);
        if t.name.starts_with("browser_")
            || matches!(
                grade,
                Some(lucia_core::Grade::ExternalWrite | lucia_core::Grade::Forbidden)
            )
        {
            tools.insert(t.name.clone());
        }
        if t.name == "browser_open"
            && let Ok(u) = url::Url::parse(&t.arg)
            && let Some(h) = u.host_str()
        {
            hosts.insert(h.to_ascii_lowercase());
        }
    }
    json!({ "tools": tools, "hosts": hosts })
}

/// 본문에 적힌 주소의 호스트와 이름이 나온 도구(사람이 채팅에서 만들 때처럼 흔적이 없을 때).
pub fn footprint_from_body(body: &str) -> serde_json::Value {
    let mut tools = BTreeSet::new();
    for t in crate::tools::TOOLS {
        if (t.name.starts_with("browser_") || t.grade >= lucia_core::Grade::ExternalWrite)
            && body.contains(t.name)
        {
            tools.insert(t.name.to_owned());
        }
    }
    let mut hosts = BTreeSet::new();
    for word in body.split(|c: char| c.is_whitespace() || "()<>[]\"'`".contains(c)) {
        if (word.starts_with("http://") || word.starts_with("https://"))
            && let Ok(u) = url::Url::parse(word)
            && let Some(h) = u.host_str()
        {
            hosts.insert(h.to_ascii_lowercase());
        }
    }
    json!({ "tools": tools, "hosts": hosts })
}

/// 스크립트(결정 0025)가 할 수 있는 일: 선언한 도구 중 브라우저·외부 쓰기, 사용자 자료를 읽으면서 바깥으로
/// 가져가는(fetch) 조합, 코드에 적힌 주소의 호스트.
pub fn footprint_from_script(code: &str, tools: &[String]) -> serde_json::Value {
    let mut out = BTreeSet::new();
    let private = tools.iter().any(|t| private_read(t));
    for t in tools {
        let spec = crate::tools::spec(t);
        if t.starts_with("browser_")
            || spec.is_some_and(|s| s.grade >= lucia_core::Grade::ExternalWrite)
            || (t == "fetch" && private)
        {
            out.insert(t.clone());
        }
    }
    let hosts = footprint_from_body(code)["hosts"].clone();
    json!({ "tools": out, "hosts": hosts })
}

/// 사용자 자료를 읽는 도구(기억, 메일·일정, Discord 기록).
pub fn private_read(name: &str) -> bool {
    use crate::tools::Group;
    // 로그인 세션으로 가져온 페이지(결정 0036)도 사용자 자료다.
    name == "fetch_session"
        || crate::tools::spec(name).is_some_and(|s| {
            s.grade == lucia_core::Grade::Read
                && matches!(s.group, Group::Memory | Group::Google | Group::Discord)
        })
}

/// 스크립트가 선언할 수 있는 도구인가: 표에 있는 도구(스스로를 부르는 것 빼고)나 `fetch`.
pub fn script_tool_ok(name: &str) -> bool {
    name == "fetch"
        || name == "fetch_session"
        || (crate::tools::spec(name).is_some() && !crate::script::never_from_script(name))
}

/// 두 흔적을 합친다.
pub fn merge(a: &serde_json::Value, b: &serde_json::Value) -> serde_json::Value {
    let set = |v: &serde_json::Value, k: &str| -> BTreeSet<String> {
        v[k].as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect()
    };
    let tools: BTreeSet<String> = set(a, "tools").union(&set(b, "tools")).cloned().collect();
    let hosts: BTreeSet<String> = set(a, "hosts").union(&set(b, "hosts")).cloned().collect();
    json!({ "tools": tools, "hosts": hosts })
}

/// 사람의 승인이 필요한가: 외부 쓰기·브라우저 도구를 썼거나 사이트를 열었다(명세 "새 도메인, 새 도구,
/// 외부 쓰기가 필요한 스킬은 사용자 승인 뒤에만 활성화") [제안: 사이트는 모두 새 도메인으로 본다].
pub fn needs_approval(footprint: &serde_json::Value) -> bool {
    let any = |k: &str| footprint[k].as_array().is_some_and(|a| !a.is_empty());
    any("tools") || any("hosts")
}

/// 흔적을 사람이 읽을 한 줄로.
pub fn footprint_line(footprint: &serde_json::Value) -> String {
    let list = |k: &str| -> String {
        footprint[k]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    let (tools, hosts) = (list("tools"), list("hosts"));
    match (tools.is_empty(), hosts.is_empty()) {
        (true, true) => "읽기만(검색·메일·일정·기억)".to_owned(),
        (false, true) => format!("도구: {tools}"),
        (true, false) => format!("사이트: {hosts}"),
        (false, false) => format!("도구: {tools} · 사이트: {hosts}"),
    }
}

/// 스킬 도구가 쓰는 것(MCP).
#[derive(Clone)]
pub struct SkillAccess {
    pub store: std::sync::Arc<std::sync::Mutex<lucia_store::Store>>,
    pub files: SkillFiles,
    /// 승인 데스크에 카드를 올리는 통로(새 스킬 알림, 승인).
    pub posts: Option<tokio::sync::mpsc::UnboundedSender<String>>,
}

/// 스킬 검토 카드(승인 종류 `skill_review`)를 만든다. `mode`: `notice`(읽기만: [고정]/[폐기], 답 없으면 그대로)
/// 또는 `approve`(외부 쓰기·사이트: [승인]/[폐기], 답 없으면 보관).
pub fn request_review(
    store: &lucia_store::Store,
    skill: &lucia_store::Skill,
    body: &str,
    mode: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let excerpt: String = body.chars().take(1500).collect();
    store
        .create_approval(
            &lucia_store::NewApproval {
                id: id.clone(),
                kind: crate::approvals::KIND_SKILL_REVIEW.into(),
                payload: json!({
                    "slug": skill.slug,
                    "title": skill.title,
                    "description": skill.description,
                    "revision": skill.revision,
                    "source": skill.source,
                    "footprint": footprint_line(&skill.footprint),
                    "body": excerpt,
                    "mode": mode,
                }),
                summary: format!("스킬 {}: {}", skill.slug, skill.title),
                run_id: None,
                expires_at: now + chrono::Duration::days(7),
            },
            now,
        )
        .map_err(|e| e.to_string())?;
    Ok(id)
}

/// 스킬 파일 사본과 git 기록(`<data_dir>/skills`). 실패해도 저장소의 개정이 원본이므로 알림만 한다.
#[derive(Debug, Clone)]
pub struct SkillFiles {
    pub dir: PathBuf,
}

impl SkillFiles {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            dir: data_dir.join("skills"),
        }
    }

    fn git(&self, args: &[&str]) -> Result<(), String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.dir)
            .args(["-c", "user.name=Lucia", "-c", "user.email=lucia@localhost"])
            .args(args)
            .output()
            .map_err(|e| format!("git: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
        }
    }

    /// `SKILL.md`(와 스크립트가 있으면 `run.py`)를 쓰고 그 개정을 커밋한다.
    pub fn write(
        &self,
        slug: &str,
        title: &str,
        description: &str,
        body: &str,
        script: Option<&str>,
        message: &str,
    ) -> Result<(), String> {
        // 이름은 저장소가 검사했다(영문 소문자·숫자·하이픈). 그래도 경로를 벗어나지 않게 한 번 더 본다.
        if slug.is_empty()
            || !slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err("스킬 이름이 경로로 쓸 수 없다".into());
        }
        std::fs::create_dir_all(self.dir.join(slug)).map_err(|e| e.to_string())?;
        if !self.dir.join(".git").exists() {
            self.git(&["init", "-q"])?;
        }
        let one = |s: &str| s.replace(['\n', '\r'], " ");
        let text = format!(
            "---\nname: {slug}\ndescription: {}\n---\n\n# {}\n\n{}\n",
            one(description),
            one(title),
            body.trim_end()
        );
        std::fs::write(self.dir.join(slug).join("SKILL.md"), text).map_err(|e| e.to_string())?;
        let run = self.dir.join(slug).join("run.py");
        match script {
            Some(code) => std::fs::write(&run, code).map_err(|e| e.to_string())?,
            None if run.exists() => std::fs::remove_file(&run).map_err(|e| e.to_string())?,
            None => {}
        }
        self.git(&["add", "-A"])?;
        self.git(&["commit", "-q", "--allow-empty", "-m", &one(message)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, arg: &str) -> RunTool {
        RunTool {
            run_id: "task-1-r1".into(),
            name: name.into(),
            arg: arg.into(),
        }
    }

    #[test]
    fn footprint_comes_from_what_the_task_did() {
        let read_only =
            footprint_from_trace(&[tool("WebSearch", "노트북"), tool("memory_read", "x")]);
        assert!(!needs_approval(&read_only));
        assert_eq!(footprint_line(&read_only), "읽기만(검색·메일·일정·기억)");
        let web = footprint_from_trace(&[
            tool("browser_open", "https://shop.example.com/info/?pcode=1"),
            tool("browser_snapshot", ""),
        ]);
        assert!(needs_approval(&web));
        assert_eq!(web["hosts"], json!(["shop.example.com"]));
        let mail = footprint_from_trace(&[tool("mail_send", "")]);
        assert!(needs_approval(&mail), "외부 쓰기");
        let body =
            footprint_from_body("1. https://www.example.com/a 를 연다\n2. browser_click로 더보기");
        assert_eq!(body["hosts"], json!(["www.example.com"]));
        assert_eq!(body["tools"], json!(["browser_click"]));
        assert_eq!(
            merge(&read_only, &body)["hosts"],
            json!(["www.example.com"])
        );
        // 스크립트(결정 0025): 사용자 자료를 읽고 바깥으로 가져가면 승인, 읽기만이나 가져오기만은 아님.
        let tools = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let reads = footprint_from_script("x = 1", &tools(&["mail_search", "memory_read"]));
        assert!(!needs_approval(&reads));
        let leak = footprint_from_script("x = 1", &tools(&["mail_search", "fetch"]));
        assert_eq!(leak["tools"], json!(["fetch"]));
        let public = footprint_from_script("x = 1", &tools(&["fetch"]));
        assert!(!needs_approval(&public));
        let site = footprint_from_script(
            "page = lucia.fetch('https://notice.example.ac.kr/list')",
            &tools(&["fetch"]),
        );
        assert_eq!(site["hosts"], json!(["notice.example.ac.kr"]));
        assert!(script_tool_ok("mail_search") && script_tool_ok("fetch"));
        assert!(!script_tool_ok("routine_create") && !script_tool_ok("nope"));
    }

    #[test]
    fn files_are_written_and_versioned_with_git() {
        let dir =
            std::env::temp_dir().join(format!("lucia-skills-{}", uuid::Uuid::new_v4().simple()));
        let files = SkillFiles { dir: dir.clone() };
        files
            .write(
                "price-check",
                "쇼핑몰 최저가",
                "제품 최저가 확인",
                "1. 검색",
                None,
                "price-check r1: 처음",
            )
            .unwrap();
        files
            .write(
                "price-check",
                "쇼핑몰 최저가",
                "제품 최저가 확인",
                "1. 검색\n2. 표",
                Some("print(1)"),
                "price-check r2: 고침",
            )
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("price-check/run.py")).unwrap(),
            "print(1)"
        );
        let text = std::fs::read_to_string(dir.join("price-check/SKILL.md")).unwrap();
        assert!(text.starts_with("---\nname: price-check\ndescription: 제품 최저가 확인\n---"));
        let log = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&log.stdout).lines().count(), 2);
        assert!(files.write("../x", "t", "d", "b", None, "m").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
