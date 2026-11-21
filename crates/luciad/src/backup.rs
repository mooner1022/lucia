//! 상주 세션 기록의 VM 밖 백업(명세 §샌드박스 수명: "세션 기록은 VM 밖에도 주기적으로 백업한다", PLAN §8).
//!
//! 하루 한 번 `sbx cp <샌드박스>:<VM 경로> <임시 디렉터리>`로 복사한 뒤 `<데이터>/session-backup/<날짜>`로
//! 옮긴다(복사가 끝난 것만 날짜 이름을 얻는다). 같은 날 다시 하면 바꾼다. 보관 기간이 지난 것은 지운다.
//! 세션 기록에는 메일 내용이 들어 있으므로 디렉터리는 0700으로 만든다(`sbx cp`는 0600·0700을 유지한다).
//!
//! 복사는 `sbx cp`가 한다(이 데몬은 VM이 준 이름으로 경로를 만들거나 묶음을 풀지 않는다). 복사된 것을 셀 때와 지울
//! 때는 심볼릭 링크를 따라가지 않는다. 받은 내용은 읽지 않고 그대로 둔다. VM 안은 Lucia가 쉘로 바꿀 수 있으므로
//! (결정 0059) 복사된 것 안의 링크나 파일을 호스트에서 따라가 열면 안 된다.

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, bail};
use chrono::{DateTime, NaiveDate, Utc};
use lucia_store::Store;

use crate::config::Config;

const COPY_TIMEOUT: Duration = Duration::from_secs(300);
/// 백업 하나의 크기 상한 [제안]. 상주 VM은 이제 Lucia가 쉘로 쓸 수 있는 곳이라(결정 0059), 세션 기록 자리에
/// 무엇이 얼마나 놓일지 호스트가 믿을 수 없다. 넘으면 남기지 않고 실패로 알린다(호스트 디스크를 지킨다).
const MAX_BACKUP_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub struct SessionBackup {
    pub sbx: PathBuf,
    pub sandbox: String,
    pub vm_path: String,
    pub dest_root: PathBuf,
    pub keep_days: u32,
    pub tz: chrono_tz::Tz,
    pub store: Arc<Mutex<Store>>,
}

fn mkdir_private(path: &Path) -> anyhow::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .with_context(|| format!("디렉터리 만들기: {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// 파일 수와 바이트 수.
fn tally(path: &Path) -> (usize, u64) {
    let mut files = 0;
    let mut bytes = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(entry.path()),
                Ok(t) if t.is_file() => {
                    files += 1;
                    bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
                _ => {}
            }
        }
    }
    (files, bytes)
}

impl SessionBackup {
    pub fn from_config(cfg: &Config, store: Arc<Mutex<Store>>) -> Option<Self> {
        if !cfg.backup.enabled {
            return None;
        }
        Some(Self {
            sbx: cfg.sandbox.sbx.clone(),
            sandbox: cfg.sandbox.name.clone(),
            vm_path: cfg.backup.vm_path.clone(),
            dest_root: cfg.data_dir.join("session-backup"),
            keep_days: cfg.backup.keep_days,
            tz: cfg.tz().ok()?,
            store,
        })
    }

    /// 백업 한 번. 요약(파일 수, 크기, 위치)을 돌려준다.
    pub async fn run(&self, now: DateTime<Utc>) -> anyhow::Result<String> {
        let today = now.with_timezone(&self.tz).date_naive();
        let day = today.format("%Y-%m-%d").to_string();
        mkdir_private(&self.dest_root)?;
        let incoming = self.dest_root.join(format!(".incoming-{day}"));
        if incoming.exists() {
            std::fs::remove_dir_all(&incoming)?;
        }
        mkdir_private(&incoming)?;

        let source = format!("{}:{}", self.sandbox, self.vm_path);
        let mut cmd = tokio::process::Command::new(&self.sbx);
        cmd.arg("cp")
            .arg(&source)
            .arg(&incoming)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let out = tokio::time::timeout(COPY_TIMEOUT, cmd.output())
            .await
            .context("sbx cp 시간 초과")?
            .context("sbx cp 실행")?;
        if !out.status.success() {
            let _ = std::fs::remove_dir_all(&incoming);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
            bail!(
                "sbx cp 실패({}): {}",
                out.status,
                tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
            );
        }
        let (files, bytes) = tally(&incoming);
        if files == 0 {
            let _ = std::fs::remove_dir_all(&incoming);
            bail!("복사된 세션 기록이 없다({source})");
        }
        if bytes > MAX_BACKUP_BYTES {
            let _ = std::fs::remove_dir_all(&incoming);
            bail!(
                "세션 기록이 너무 크다({} MB, 상한 {} MB). 백업을 남기지 않았다. 상주 VM의 {}를 확인한다",
                bytes / (1024 * 1024),
                MAX_BACKUP_BYTES / (1024 * 1024),
                self.vm_path
            );
        }
        let target = self.dest_root.join(&day);
        if target.exists() {
            std::fs::remove_dir_all(&target)?;
        }
        std::fs::rename(&incoming, &target)?;
        let pruned = self.prune(today)?;

        let summary = format!(
            "{files}개 파일, {} KB → {}{}",
            bytes.div_ceil(1024),
            target.display(),
            if pruned > 0 {
                format!(" (오래된 백업 {pruned}개 지움)")
            } else {
                String::new()
            }
        );
        if let Ok(store) = self.store.lock() {
            let local = now.with_timezone(&self.tz).format("%m-%d %H:%M");
            let _ = store.set_setting(
                lucia_store::keys::BACKUP_LAST,
                &format!("{local} · {files}개 파일, {} KB", bytes.div_ceil(1024)),
                now,
            );
        }
        Ok(summary)
    }

    /// 보관 기간이 지난 날짜 디렉터리를 지운다(이름이 날짜인 것만).
    fn prune(&self, today: NaiveDate) -> anyhow::Result<usize> {
        let oldest = today - chrono::Duration::days(i64::from(self.keep_days));
        let mut removed = 0;
        for entry in std::fs::read_dir(&self.dest_root)?.flatten() {
            let name = entry.file_name();
            let Some(date) = name
                .to_str()
                .and_then(|n| NaiveDate::parse_from_str(n, "%Y-%m-%d").ok())
            else {
                continue;
            };
            if date < oldest && entry.file_type().is_ok_and(|t| t.is_dir()) {
                std::fs::remove_dir_all(entry.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// `sbx cp SRC DST`를 흉내 낸다: FAKE_FROM 디렉터리를 DST 안에 복사한다. FAKE_FAIL이면 실패.
    fn fake_sbx(dir: &Path, from: &Path, fail: bool) -> PathBuf {
        let script = dir.join("sbx");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n[ \"$1\" = cp ] || exit 9\n{}cp -rp '{}' \"$3\"\n",
                if fail {
                    "echo 'sandbox not found' >&2; exit 1\n"
                } else {
                    ""
                },
                from.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn setup(name: &str, fail: bool) -> (SessionBackup, PathBuf) {
        let root = std::env::temp_dir().join(format!("lucia-backup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let vm = root.join("vm/projects/-work");
        std::fs::create_dir_all(&vm).unwrap();
        std::fs::write(vm.join("s1.jsonl"), "{}\n").unwrap();
        std::fs::write(vm.join("s2.jsonl"), "{}\n{}\n").unwrap();
        let sbx = fake_sbx(&root, &root.join("vm/projects"), fail);
        let backup = SessionBackup {
            sbx,
            sandbox: "lucia".into(),
            vm_path: "/home/agent/.claude/projects".into(),
            dest_root: root.join("data/session-backup"),
            keep_days: 3,
            tz: chrono_tz::Asia::Seoul,
            store: Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
        };
        (backup, root)
    }

    #[tokio::test]
    async fn copies_into_a_private_dated_directory_and_prunes_old_ones() {
        let (b, root) = setup("ok", false);
        let old = b.dest_root.join("2026-11-01");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(b.dest_root.join("keep-me")).unwrap();

        let now = Utc.with_ymd_and_hms(2026, 11, 16, 19, 0, 0).unwrap(); // KST 17일 04시
        let summary = b.run(now).await.unwrap();
        let target = b.dest_root.join("2026-11-17");
        assert!(target.join("projects/-work/s1.jsonl").exists(), "{summary}");
        assert!(summary.contains("2개 파일"), "{summary}");
        let mode = std::fs::metadata(&b.dest_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "백업 디렉터리는 소유자만");
        assert!(!old.exists(), "보관 기간이 지난 백업은 지운다");
        assert!(
            b.dest_root.join("keep-me").exists(),
            "날짜 이름이 아닌 것은 건드리지 않는다"
        );
        assert!(!b.dest_root.join(".incoming-2026-11-17").exists());

        // 같은 날 다시: 바꾼다(겹쳐 쌓이지 않는다).
        b.run(now).await.unwrap();
        assert!(!target.join("projects/projects").exists());
        assert!(
            b.store
                .lock()
                .unwrap()
                .setting(lucia_store::keys::BACKUP_LAST)
                .unwrap()
                .unwrap()
                .contains("2개 파일")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn failure_keeps_previous_backup_and_reports() {
        let (b, root) = setup("fail", true);
        let earlier = b.dest_root.join("2026-11-16");
        std::fs::create_dir_all(earlier.join("projects")).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 11, 16, 19, 0, 0).unwrap();
        let err = b.run(now).await.unwrap_err().to_string();
        assert!(err.contains("sandbox not found"), "{err}");
        assert!(earlier.exists(), "실패해도 전날 백업은 남는다");
        assert!(!b.dest_root.join(".incoming-2026-11-17").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
