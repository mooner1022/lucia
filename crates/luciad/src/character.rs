//! 캐릭터 파일(결정 0009): 관리 페이지에서 사용자가 고친다.
//!
//! - 파일은 레포의 `character/character.md`다. 상주 샌드박스가 그 디렉터리를 읽기 전용으로 마운트하고, 실행마다
//!   `--append-system-prompt-file`로 넘긴다. 다만 Claude Code는 대화의 첫 요청에서 만든 시스템 프롬프트를 기록해
//!   두고, 이어 쓰는 동안(`--resume`)은 압축될 때까지 그 기록을 그대로 보낸다(`--system-prompt-snapshot`의 기본값
//!   on, `claude --help`). 그래서 고친 캐릭터는 상주 대화의 다음 압축이나 새 세션부터 적용된다(명세 §캐릭터
//!   "캐릭터 변경은 세션 교대 때 적용된다").
//! - 호스트 쪽 경로는 설정에 따로 적지 않고 `sbx ls --json`의 마운트에서 찾는다(VM 안 경로 `sandbox.character_file`을
//!   품은 마운트).
//! - Lucia(모델)와 학습 루프는 이 파일을 고칠 수 없다(보호 영역). 고치는 길은 로그인한 관리 페이지뿐이다.
//! - 저장할 때: 줄바꿈을 맞추고, 크기를 묶고, 비밀처럼 보이는 글을 막고, 화면을 연 뒤 파일이 바뀌었으면
//!   덮어쓰지 않는다. 같은 디렉터리의 임시 파일에 쓰고 이름을 바꿔(원자적으로) 넣는다. 고치기 전·후 내용을
//!   저장소에 모두 남겨 되돌릴 수 있게 한다.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use lucia_store::Store;
use sha2::{Digest as _, Sha256};

/// 파일 크기 상한(바이트). 매 실행 시스템 프롬프트에 통째로 들어가므로 묶는다.
pub const MAX_BYTES: usize = 16 * 1024;
/// 지우면 경고하는 절(판단·정확성·승인 규칙이 캐릭터보다 우선한다는 절).
pub const SAFETY_HEADING: &str = "## 캐릭터보다 우선하는 것";
/// 바꾼 까닭 상한(자).
pub const NOTE_MAX: usize = 200;

/// `sbx ls --json` 결과에서 샌드박스의 마운트를 보고, VM 안 파일의 호스트 경로를 찾는다.
/// 마운트는 `호스트:VM[:ro]` 꼴이다.
pub fn host_path_from_ls_json(json: &str, sandbox: &str, vm_file: &str) -> Option<PathBuf> {
    let list: serde_json::Value = serde_json::from_str(json).ok()?;
    let entry = list
        .as_array()?
        .iter()
        .find(|s| s["name"].as_str() == Some(sandbox))?;
    let vm_file = Path::new(vm_file);
    entry["mounts"]
        .as_array()?
        .iter()
        .filter_map(|m| m.as_str())
        .filter_map(|m| {
            let mut parts = m.split(':');
            let host = parts.next()?;
            let vm = parts.next().unwrap_or(host);
            let rest = vm_file.strip_prefix(vm).ok()?;
            Some((vm.len(), Path::new(host).join(rest)))
        })
        // 가장 깊은 마운트가 이긴다.
        .max_by_key(|(len, _)| *len)
        .map(|(_, p)| p)
}

/// 실제 sbx로 호스트 경로를 찾는다.
pub async fn resolve(sbx: &Path, sandbox: &str, vm_file: &str) -> Result<PathBuf, String> {
    let out = tokio::process::Command::new(sbx)
        .args(["ls", "--json"])
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("sbx ls 실행: {e}"))?;
    if !out.status.success() {
        return Err(format!("sbx ls 실패({})", out.status));
    }
    host_path_from_ls_json(&String::from_utf8_lossy(&out.stdout), sandbox, vm_file).ok_or_else(
        || {
            format!(
                "샌드박스 {sandbox}의 마운트에서 {vm_file}을 찾지 못했다(deploy/create-sandbox.sh)"
            )
        },
    )
}

/// 내용의 짧은 지문(화면을 연 뒤 파일이 바뀌었는지 본다).
pub fn fingerprint(content: &str) -> String {
    Sha256::digest(content.as_bytes())
        .iter()
        .take(12)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 저장할 글로 다듬는다: CRLF → LF, 줄 끝 공백 제거, 끝에 줄바꿈 하나. 경고도 돌려준다.
pub fn normalize(text: &str) -> Result<(String, Vec<String>), String> {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.trim_end_matches('\r').trim_end())
        .collect();
    let mut out = lines.join("\n").trim_matches('\n').to_owned();
    if out.trim().is_empty() {
        return Err("비어 있다. 캐릭터 파일은 비울 수 없다".into());
    }
    out.push('\n');
    if out.len() > MAX_BYTES {
        return Err(format!(
            "너무 길다({}바이트). {}바이트까지 쓸 수 있다(매 실행 통째로 모델에 들어간다)",
            out.len(),
            MAX_BYTES
        ));
    }
    if out.contains('\0') {
        return Err("널 문자가 들어 있다".into());
    }
    if crate::mask::global().mask(&out) != out {
        return Err(
            "비밀(토큰·비밀번호)로 보이는 글이 들어 있다. 캐릭터 파일은 매 실행 모델에 들어가므로 넣지 않는다".into(),
        );
    }
    let mut warnings = Vec::new();
    if !out.contains(SAFETY_HEADING) {
        warnings.push(format!(
            "‘{}’ 절이 없습니다. 판단·정확성·승인 규칙이 말투보다 우선한다는 절이라 남겨 두기를 권합니다.",
            SAFETY_HEADING.trim_start_matches("## ")
        ));
    }
    Ok((out, warnings))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    /// 새 개정 번호. 바뀐 것이 없으면 `None`.
    pub revision: Option<i64>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CharacterFile {
    pub path: PathBuf,
}

impl CharacterFile {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn read(&self) -> Result<String, String> {
        std::fs::read_to_string(&self.path).map_err(|e| format!("{}: {e}", self.path.display()))
    }

    /// 같은 디렉터리의 임시 파일에 쓰고 이름을 바꾼다(VM은 디렉터리를 마운트하므로 바뀐 파일을 바로 본다).
    fn write_atomic(&self, content: &str) -> Result<(), String> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| "캐릭터 파일의 디렉터리가 없다".to_owned())?;
        let tmp = dir.join(".character.md.lucia-tmp");
        let mode = std::fs::metadata(&self.path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o644);
        let result = (|| -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
            std::fs::rename(&tmp, &self.path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result.map_err(|e| format!("쓰기 실패: {e}"))
    }

    /// 저장한다. `base`는 화면을 열 때의 지문이다(그 뒤 파일이 바뀌었으면 덮어쓰지 않는다).
    pub fn save(
        &self,
        store: &Mutex<Store>,
        text: &str,
        base: &str,
        note: &str,
        now: DateTime<Utc>,
    ) -> Result<Saved, String> {
        let (content, warnings) = normalize(text)?;
        let current = self.read()?;
        if fingerprint(&current) != base {
            return Err(
                "화면을 연 뒤 파일이 바뀌었다(다른 곳에서 고쳤다). 화면을 새로 열어 다시 고친다"
                    .into(),
            );
        }
        if content == current {
            return Ok(Saved {
                revision: None,
                warnings,
            });
        }
        let note: String = match note.trim() {
            "" => "관리 페이지에서 고침".to_owned(),
            n => n.chars().take(NOTE_MAX).collect(),
        };
        // 고치기 전 내용이 기록에 없으면(처음이거나 바깥에서 고쳤으면) 먼저 남긴다.
        {
            let store = store.lock().expect("store lock");
            let latest = store
                .character_revisions(1)
                .map_err(|e| format!("기록 읽기: {e}"))?;
            if latest.first().map(|r| r.content.as_str()) != Some(current.as_str()) {
                store
                    .add_character_revision(&current, "고치기 전 파일", now)
                    .map_err(|e| format!("기록 저장: {e}"))?;
            }
        }
        self.write_atomic(&content)?;
        let store = store.lock().expect("store lock");
        let revision = store
            .add_character_revision(&content, &note, now)
            .map_err(|e| format!("파일은 바꿨지만 기록을 남기지 못했다: {e}"))?;
        let _ = store.add_activity(
            "character_edit",
            &format!("캐릭터 파일을 고쳤다(개정 {revision}): {note}"),
            None,
            now,
        );
        Ok(Saved {
            revision: Some(revision),
            warnings,
        })
    }

    /// 예전 개정으로 되돌린다(새 개정으로 남는다).
    pub fn restore(
        &self,
        store: &Mutex<Store>,
        id: i64,
        now: DateTime<Utc>,
    ) -> Result<Saved, String> {
        let old = store
            .lock()
            .expect("store lock")
            .character_revision(id)
            .map_err(|e| format!("기록 읽기: {e}"))?
            .ok_or_else(|| format!("개정 {id}이 없다"))?;
        let base = fingerprint(&self.read()?);
        self.save(
            store,
            &old.content,
            &base,
            &format!("개정 {id}으로 되돌림"),
            now,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_path_comes_from_the_deepest_mount() {
        let json = r#"[
          {"name": "lucia-poc", "mounts": ["/repo/poc/p2:/opt/lucia/p2:ro"]},
          {"name": "lucia", "mounts": [
            "/home/u/.local/share/lucia/workspace:/home/u/.local/share/lucia/workspace",
            "/home/u/lucia/plugin:/opt/lucia/plugin:ro",
            "/home/u/lucia/character:/opt/lucia/character:ro",
            "/elsewhere:/opt/lucia:ro"
          ]},
          {"name": "lucia-browser"}
        ]"#;
        assert_eq!(
            host_path_from_ls_json(json, "lucia", "/opt/lucia/character/character.md"),
            Some(PathBuf::from("/home/u/lucia/character/character.md"))
        );
        assert_eq!(
            host_path_from_ls_json(json, "lucia", "/opt/lucia/characterx/c.md"),
            Some(PathBuf::from("/elsewhere/characterx/c.md")),
            "경로 조각 단위로 비교한다"
        );
        assert!(host_path_from_ls_json(json, "lucia-browser", "/opt/lucia/x").is_none());
        assert!(host_path_from_ls_json(json, "nope", "/opt/lucia/x").is_none());
        assert!(host_path_from_ls_json("not json", "lucia", "/x").is_none());
    }

    #[test]
    fn normalize_cleans_line_endings_and_checks_limits() {
        let (out, warnings) = normalize("# 비서\r\n\r\n말투  \r\n\r\n\r\n").unwrap();
        assert_eq!(out, "# 비서\n\n말투\n");
        assert_eq!(warnings.len(), 1, "안전 절이 없으면 경고");
        let (_, warnings) = normalize(&format!("# 비서\n{SAFETY_HEADING}\n- 규칙\n")).unwrap();
        assert!(warnings.is_empty());
        assert!(normalize(" \r\n ").is_err());
        assert!(normalize(&"가".repeat(MAX_BYTES)).is_err());
        crate::mask::global().register("character-secret-xyz-123");
        assert!(normalize("토큰 character-secret-xyz-123").is_err());
    }

    fn temp_file(content: &str) -> (PathBuf, CharacterFile) {
        let dir = std::env::temp_dir().join(format!("lucia-character-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("character.md");
        std::fs::write(&path, content).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        (dir, CharacterFile::new(path))
    }

    #[test]
    fn save_records_before_and_after_and_refuses_stale_edits() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let (dir, file) = temp_file("# 비서\n존댓말\n");
        let base = fingerprint(&file.read().unwrap());
        let saved = file
            .save(&store, "# 비서\r\n반말\r\n", &base, "말투 바꿈", Utc::now())
            .unwrap();
        assert!(saved.revision.is_some());
        assert_eq!(file.read().unwrap(), "# 비서\n반말\n");
        assert_eq!(
            std::fs::metadata(&file.path).unwrap().permissions().mode() & 0o777,
            0o640,
            "권한을 지킨다"
        );
        assert!(!dir.join(".character.md.lucia-tmp").exists());
        let history = store.lock().unwrap().character_revisions(10).unwrap();
        assert_eq!(history.len(), 2, "고치기 전 내용도 남긴다");
        assert_eq!(history[0].note, "말투 바꿈");
        assert_eq!(history[1].content, "# 비서\n존댓말\n");

        assert!(
            file.save(&store, "# 비서\n다른 글\n", &base, "", Utc::now())
                .unwrap_err()
                .contains("바뀌었다"),
            "낡은 화면으로는 덮어쓰지 않는다"
        );
        let base = fingerprint(&file.read().unwrap());
        let same = file
            .save(&store, "# 비서\n반말\n", &base, "", Utc::now())
            .unwrap();
        assert_eq!(same.revision, None, "바뀐 것이 없으면 기록하지 않는다");

        // 바깥에서 고친 뒤 저장: 바깥 내용도 기록에 남는다.
        std::fs::write(&file.path, "# 비서\n바깥에서 고침\n").unwrap();
        let base = fingerprint(&file.read().unwrap());
        file.save(&store, "# 비서\n다시\n", &base, "", Utc::now())
            .unwrap();
        let history = store.lock().unwrap().character_revisions(10).unwrap();
        assert_eq!(history[0].note, "관리 페이지에서 고침");
        assert_eq!(history[1].content, "# 비서\n바깥에서 고침\n");

        let first = history.last().unwrap().id;
        file.restore(&store, first, Utc::now()).unwrap();
        assert_eq!(file.read().unwrap(), "# 비서\n존댓말\n");
        assert!(
            store.lock().unwrap().character_revisions(1).unwrap()[0]
                .note
                .contains("되돌림")
        );
        assert!(file.restore(&store, 9999, Utc::now()).is_err());
        let activity = store.lock().unwrap().recent_activity(10).unwrap();
        assert!(activity.iter().any(|a| a.kind == "character_edit"));
        std::fs::remove_dir_all(dir).ok();
    }
}
