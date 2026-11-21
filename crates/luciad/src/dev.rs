//! 개발 위임(결정 0020, 명세 6단계 "레포 대상 개발 작업, 비용 상한 강제", §샌드박스 수명 "임시 샌드박스").
//!
//! - 레포는 데몬이 호스트에서 `<data_dir>/devwork/<작업>`에 클론하고(호스트의 git 인증을 쓴다), 작업 가지
//!   `lucia/<작업>`을 만든다.
//! - 작업마다 임시 샌드박스(`lucia-<작업>`)를 `sbx create --clone`으로 만든다. 에이전트는 컨테이너 안의 클론에서
//!   일하고(호스트 클론은 읽기 전용으로 붙는다), 그 커밋은 호스트에서 `sandbox-<이름>` 원격으로 보인다.
//! - 샌드박스 안에서는 코드 도구(Bash, 편집)를 쓴다. 경계는 VM과 네트워크 정책이다(명세 §격리 경계). 상주
//!   샌드박스의 훅은 건드리지 않는다. 네트워크는 데몬 MCP 포트와 관리 페이지에서 정한 주소만 연다.
//! - push는 데몬이 호스트에서, 사람의 승인 카드를 거쳐, `lucia/<작업>` 가지로만, 강제 없이 한다.
//! - 작업이 끝나면 샌드박스와 호스트 클론을 지운다(결과만 남긴다).

use std::path::{Path, PathBuf};
use std::process::Command;

/// 관리 페이지 설정 키.
pub mod keys {
    /// 개발 작업 하나의 비용 상한(달러, API 환산).
    pub const COST_CAP_USD: &str = "dev.cost_cap_usd";
    /// 임시 샌드박스에서 열 주소(줄마다 하나, 예: index.crates.io:443).
    pub const ALLOW_HOSTS: &str = "dev.allow_hosts";
    /// 맡길 수 있는 로컬 레포의 상위 경로(줄마다 하나). 비우면 https 주소만.
    pub const LOCAL_ROOTS: &str = "dev.local_roots";
}

/// 비용 상한 기본값 [제안].
pub const DEFAULT_COST_CAP_USD: f64 = 3.0;
const SANDBOX_PREFIX: &str = "lucia-";
/// 상주 샌드박스와 같이, 킷이 여는 계정 쪽·업데이트 주소는 막는다.
const DENY: [&str; 4] = [
    "mcp-proxy.anthropic.com",
    "bridge.claudeusercontent.com",
    "claude.com",
    "downloads.claude.ai",
];

/// 작업 가지 이름.
pub fn branch(task_id: &str) -> String {
    format!("lucia/{task_id}")
}

pub fn sandbox_name(task_id: &str) -> String {
    format!("{SANDBOX_PREFIX}{task_id}")
}

/// 맡길 레포를 검사한다: https 주소(공개 호스트) 또는 허용한 상위 경로 아래의 로컬 git 레포.
pub fn check_repo(repo: &str, local_roots: &[PathBuf]) -> Result<String, String> {
    let repo = repo.trim();
    if repo.starts_with("http://") {
        return Err("https 주소만 받는다".into());
    }
    if repo.starts_with("https://") {
        let egress = crate::browser::egress::Egress::new(vec![]);
        let url = egress.check_url(repo)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err("주소에 인증 정보를 넣지 않는다(호스트의 git 인증을 쓴다)".into());
        }
        return Ok(url.to_string());
    }
    let path = std::fs::canonicalize(repo)
        .map_err(|_| "https 주소이거나 있는 로컬 경로여야 한다".to_owned())?;
    if !local_roots
        .iter()
        .any(|root| std::fs::canonicalize(root).is_ok_and(|r| path.starts_with(&r)))
    {
        return Err("관리 페이지에서 허용한 로컬 경로 아래가 아니다".into());
    }
    // 작업 트리가 있는 레포(.git) 또는 bare 레포(HEAD·objects).
    let bare = path.join("HEAD").is_file() && path.join("objects").is_dir();
    if !(path.join(".git").exists() || bare) {
        return Err("git 레포가 아니다".into());
    }
    Ok(path.display().to_string())
}

/// 임시 샌드박스와 호스트 클론을 다룬다.
#[derive(Debug, Clone)]
pub struct DevSandboxes {
    pub sbx: PathBuf,
    pub data_dir: PathBuf,
    pub mcp_port: u16,
}

/// 샌드박스를 준비한 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub sandbox: String,
    /// 샌드박스 안 작업 디렉터리(호스트 클론과 같은 경로).
    pub workdir: String,
    pub base: String,
}

/// push 전에 사람에게 보일 것.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub origin: String,
    pub branch: String,
    pub commits: Vec<String>,
    pub stat: String,
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(crate::mask::global()
            .mask(err.trim())
            .chars()
            .take(400)
            .collect())
    }
}

impl DevSandboxes {
    pub fn workdir(&self, task_id: &str) -> PathBuf {
        self.data_dir.join("devwork").join(task_id)
    }

    fn git(&self, dir: &Path, args: &[&str]) -> Result<String, String> {
        run(Command::new("git").arg("-C").arg(dir).args(args))
    }

    fn sbx(&self, args: &[&str]) -> Result<String, String> {
        run(Command::new(&self.sbx).args(args))
    }

    /// 레포를 클론하고 임시 샌드박스를 만든다. 실패하면 만든 것을 지운다.
    pub fn prepare(
        &self,
        task_id: &str,
        repo: &str,
        allow_hosts: &[String],
    ) -> Result<Prepared, String> {
        let dir = self.workdir(task_id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir_all(dir.parent().expect("부모")).map_err(|e| e.to_string())?;
        run(Command::new("git")
            .args(["clone", "--quiet", "--no-hardlinks", repo])
            .arg(&dir))
        .map_err(|e| format!("클론 실패: {e}"))?;
        let base = self.git(&dir, &["rev-parse", "HEAD"])?;
        let name = sandbox_name(task_id);
        let dir_text = dir.display().to_string();
        let mut args = vec![
            "create",
            "--name",
            &name,
            "--clone",
            "--no-progress",
            "--memory",
            "4g",
            "--skills",
            "off",
            "--static-mcp",
            "none",
        ];
        for host in DENY {
            args.extend(["--deny-network", host]);
        }
        args.extend(["claude", &dir_text]);
        if let Err(e) = self.sbx(&args) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("임시 샌드박스를 만들지 못했다: {e}"));
        }
        let finish = || -> Result<(), String> {
            // 네트워크 규칙은 반드시 이 샌드박스에만 건다(빼면 모든 샌드박스에 적용된다).
            self.sbx(&[
                "policy",
                "allow",
                "network",
                &format!("localhost:{}", self.mcp_port),
                "--sandbox",
                &name,
            ])?;
            for host in allow_hosts {
                self.sbx(&["policy", "allow", "network", host, "--sandbox", &name])?;
            }
            // 컨테이너 안 클론에서 작업 가지를 연다.
            self.sbx(&[
                "exec",
                &name,
                "git",
                "-C",
                &dir_text,
                "checkout",
                "-q",
                "-B",
                &branch(task_id),
            ])?;
            Ok(())
        };
        if let Err(e) = finish() {
            self.cleanup(task_id, Some(&name));
            return Err(format!("임시 샌드박스 준비 실패: {e}"));
        }
        Ok(Prepared {
            sandbox: name,
            workdir: dir_text,
            base,
        })
    }

    /// 임시 샌드박스와 호스트 클론을 지운다. 이름이 이 모듈이 만든 것(`lucia-task-…`)일 때만 지운다.
    pub fn cleanup(&self, task_id: &str, sandbox: Option<&str>) {
        if let Some(name) = sandbox
            && name == sandbox_name(task_id)
            && name.starts_with("lucia-task-")
            && let Err(e) = self.sbx(&["rm", "--force", name])
        {
            tracing::warn!(err = %e, name, "임시 샌드박스를 지우지 못했다");
        }
        let dir = self.workdir(task_id);
        if dir.starts_with(self.data_dir.join("devwork")) && dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// 샌드박스의 작업 가지를 호스트로 가져와, push할 커밋과 바뀐 파일 요약을 만든다.
    pub fn pending(&self, task_id: &str, sandbox: &str, base: &str) -> Result<Pending, String> {
        let dir = self.workdir(task_id);
        let remote = format!("sandbox-{sandbox}");
        let br = branch(task_id);
        self.git(
            &dir,
            &[
                "fetch",
                "--quiet",
                &remote,
                &format!("{br}:refs/lucia/{br}"),
            ],
        )
        .map_err(|e| format!("샌드박스의 커밋을 가져오지 못했다: {e}"))?;
        let range = format!("{base}..refs/lucia/{br}");
        let log = self.git(
            &dir,
            &["log", "--oneline", "--no-decorate", "-n", "20", &range],
        )?;
        let commits: Vec<String> = log.lines().map(str::to_owned).collect();
        let stat = self.git(&dir, &["diff", "--stat", &range])?;
        let origin = self.git(&dir, &["remote", "get-url", "origin"])?;
        Ok(Pending {
            origin,
            branch: br,
            commits,
            stat,
        })
    }

    /// 승인된 push: `lucia/<작업>` 가지로만, 강제 없이.
    pub fn push(&self, task_id: &str) -> Result<String, String> {
        let dir = self.workdir(task_id);
        let br = branch(task_id);
        self.git(
            &dir,
            &[
                "push",
                "--quiet",
                "origin",
                &format!("refs/lucia/{br}:refs/heads/{br}"),
            ],
        )?;
        let origin = self.git(&dir, &["remote", "get-url", "origin"])?;
        Ok(format!("{origin} 의 {br} 가지로 보냈습니다."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repos_must_be_public_https_or_under_an_allowed_root() {
        assert!(check_repo("https://github.com/rust-lang/rust", &[]).is_ok());
        assert!(
            check_repo("https://user:pw@github.com/x/y", &[]).is_err(),
            "주소에 인증 정보 금지"
        );
        assert!(
            check_repo("https://192.168.0.10/x.git", &[]).is_err(),
            "사설 주소"
        );
        let root =
            std::env::temp_dir().join(format!("lucia-dev-root-{}", uuid::Uuid::new_v4().simple()));
        let repo = root.join("proj");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert!(
            check_repo(repo.to_str().unwrap(), &[]).is_err(),
            "허용한 경로가 없으면 로컬은 안 된다"
        );
        assert!(check_repo(repo.to_str().unwrap(), std::slice::from_ref(&root)).is_ok());
        assert!(check_repo("/etc", std::slice::from_ref(&root)).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn names_are_derived_from_the_task() {
        assert_eq!(branch("task-ab12cd34"), "lucia/task-ab12cd34");
        assert_eq!(sandbox_name("task-ab12cd34"), "lucia-task-ab12cd34");
    }
}
