//! 자격 증명 읽기와 쓰기(명세 §인증과 비밀 관리, `docs/PLAN.md` §4 자격 증명 파일 규칙).
//!
//! - `<credentials_dir>/<이름>.cred`를 `systemd-creds decrypt --user`로 푼다. 관리 페이지가 이 파일을 바꾸므로
//!   파일을 먼저 읽는다(systemd가 시작할 때 넘긴 사본은 그 뒤 바뀐 값을 모른다).
//! - 파일이 없을 때만 `$CREDENTIALS_DIRECTORY/<이름>`(systemd `LoadCredentialEncrypted=`)을 읽는다.
//! - 쓸 때는 `systemd-creds encrypt --user`로 바로 암호화한다(관리 페이지에서 받은 값).
//!
//! 읽은 값은 곧바로 마스킹 필터에 등록한다. 값을 로그에 남기지 않는다.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context as _, bail};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::mask::Masker;

/// 자격 증명 이름(`docs/PLAN.md` §4).
pub mod names {
    pub const DISCORD_BOT_TOKEN: &str = "discord-bot-token";
    #[allow(dead_code)] // Google 연결(다음 단계)에서 쓴다.
    pub const GOOGLE_OAUTH_CLIENT: &str = "google-oauth-client";
    #[allow(dead_code)] // Google 연결(다음 단계)에서 쓴다.
    pub const GOOGLE_REFRESH_TOKEN: &str = "google-refresh-token";
}

pub struct Credentials {
    dir: PathBuf,
    masker: Masker,
}

impl Credentials {
    pub fn new(dir: PathBuf, masker: Masker) -> Self {
        Self { dir, masker }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.cred"))
    }

    /// 있는가(값은 읽지 않는다). 상태 화면용.
    pub fn exists(&self, name: &str) -> bool {
        std::env::var_os("CREDENTIALS_DIRECTORY")
            .map(|d| Path::new(&d).join(name).exists())
            .unwrap_or(false)
            || self.file(name).exists()
    }

    /// 읽는다. 없으면 `Ok(None)`.
    pub async fn read(&self, name: &str) -> anyhow::Result<Option<String>> {
        validate_name(name)?;
        let file = self.file(name);
        let systemd_copy = std::env::var_os("CREDENTIALS_DIRECTORY")
            .map(|dir| Path::new(&dir).join(name))
            .filter(|p| p.exists());
        let value = if !file.exists() {
            match systemd_copy {
                Some(path) => tokio::fs::read_to_string(&path)
                    .await
                    .with_context(|| format!("자격 증명 {name}"))?,
                None => return Ok(None),
            }
        } else {
            let out = Command::new("systemd-creds")
                .args(["decrypt", "--user", &format!("--name={name}")])
                .arg(&file)
                .arg("-")
                .stdin(Stdio::null())
                .output()
                .await
                .context("systemd-creds 실행")?;
            if !out.status.success() {
                // stderr에 값이 섞일 일은 없지만 그래도 마스킹한다.
                bail!(
                    "자격 증명 {name}를 풀 수 없다: {}",
                    self.masker
                        .mask(String::from_utf8_lossy(&out.stderr).trim())
                );
            }
            String::from_utf8(out.stdout).context("자격 증명이 UTF-8이 아니다")?
        };
        let value = value.trim().to_owned();
        self.masker.register(&value);
        Ok(Some(value))
    }

    /// 암호화해 저장한다(관리 페이지에서 받은 값). 기존 값은 바꾼다.
    pub async fn write(&self, name: &str, value: &str) -> anyhow::Result<()> {
        validate_name(name)?;
        let value = value.trim();
        anyhow::ensure!(!value.is_empty(), "빈 값은 저장하지 않는다");
        self.masker.register(value);
        tokio::fs::create_dir_all(&self.dir).await?;
        set_private(&self.dir)?;
        let tmp = self.dir.join(format!(".{name}.cred.tmp"));
        let mut child = Command::new("systemd-creds")
            .args(["encrypt", "--user", &format!("--name={name}"), "-"])
            .arg(&tmp)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("systemd-creds 실행")?;
        let mut stdin = child.stdin.take().expect("stdin piped");
        stdin.write_all(value.as_bytes()).await?;
        drop(stdin);
        let out = child.wait_with_output().await?;
        if !out.status.success() {
            let _ = tokio::fs::remove_file(&tmp).await;
            bail!(
                "자격 증명 {name}를 암호화하지 못했다: {}",
                self.masker
                    .mask(String::from_utf8_lossy(&out.stderr).trim())
            );
        }
        set_mode(&tmp, 0o600)?;
        tokio::fs::rename(&tmp, self.file(name)).await?;
        Ok(())
    }

    pub async fn remove(&self, name: &str) -> anyhow::Result<()> {
        validate_name(name)?;
        match tokio::fs::remove_file(self.file(name)).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "자격 증명 이름이 잘못됐다: {name:?}"
    );
    Ok(())
}

fn set_private(dir: &Path) -> anyhow::Result<()> {
    set_mode(dir, 0o700)
}

fn set_mode(path: &Path, mode: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// 시험용: 이 기계에서 `systemd-creds encrypt --user`가 실제로 되는가. `--user`는 systemd 256부터 있어서,
/// 그보다 오래된 곳(예: Ubuntu 24.04의 255)이나 사용자 세션이 없는 곳에서는 실제 자격 증명을 쓰는 시험을 건너뛴다.
#[cfg(test)]
pub(crate) fn user_credentials_work() -> bool {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new("systemd-creds")
        .args(["encrypt", "--user", "--name=lucia-probe", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"probe");
    }
    child.wait().is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 실제 systemd-creds --user로 쓰고 읽는다(이 호스트에서 sudo 없이 동작함을 확인했다).
    #[tokio::test]
    async fn write_read_roundtrip_registers_mask() {
        if !crate::credentials::user_credentials_work() {
            eprintln!("systemd-creds --user를 쓸 수 없음(systemd 256 미만이거나 없음), 건너뜀");
            return;
        }
        let dir = std::env::temp_dir().join(format!("lucia-creds-{}", uuid::Uuid::new_v4()));
        let masker = Masker::default();
        let creds = Credentials::new(dir.clone(), masker.clone());
        assert!(creds.read("test-secret").await.unwrap().is_none());
        assert!(!creds.exists("test-secret"));

        creds
            .write("test-secret", "  value-for-roundtrip-123\n")
            .await
            .unwrap();
        assert!(creds.exists("test-secret"));
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("test-secret.cred"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "자격 증명 파일은 소유자만 읽는다");
        }
        let raw = std::fs::read(dir.join("test-secret.cred")).unwrap();
        assert!(
            !String::from_utf8_lossy(&raw).contains("value-for-roundtrip-123"),
            "파일에는 평문이 없다"
        );

        let fresh = Masker::default();
        let reader = Credentials::new(dir.clone(), fresh.clone());
        assert_eq!(
            reader.read("test-secret").await.unwrap().as_deref(),
            Some("value-for-roundtrip-123")
        );
        assert_eq!(
            fresh.mask("x value-for-roundtrip-123 y"),
            format!("x {} y", crate::mask::MASK)
        );

        reader.remove("test-secret").await.unwrap();
        assert!(reader.read("test-secret").await.unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn rejects_path_like_names() {
        let creds = Credentials::new(std::env::temp_dir(), Masker::default());
        assert!(creds.read("../etc/passwd").await.is_err());
        assert!(creds.write("a/b", "x").await.is_err());
    }
}
