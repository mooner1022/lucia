//! `luciad store-backup`·`store-status`를 실제 바이너리로 부른다. 출력 세 줄은 배포 스크립트가 `grep`으로
//! 읽는 약속이라 글자 그대로 본다. 모델도 네트워크도 쓰지 않고, 설정과 운영 데이터에 닿지 않는다(HOME을
//! 임시 디렉터리로 바꿔 부른다).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{TimeZone, Utc};
use lucia_core::{Initiator, TaskStatus};
use lucia_store::{NewTask, Store};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lucia-store-cli-{}-{}-{}",
            std::process::id(),
            name,
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn luciad(home: &TempDir, args: &[&Path], sub: &str) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_luciad"));
    cmd.arg(sub);
    for pair in args.chunks(2) {
        match pair {
            [flag, value] => {
                cmd.arg(flag).arg(value);
            }
            _ => unreachable!(),
        }
    }
    cmd.env("HOME", &home.0)
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn store_with_work(path: &Path) -> Store {
    let at = Utc.with_ymd_and_hms(2026, 11, 21, 9, 0, 0).unwrap();
    let store = Store::open(path).unwrap();
    for id in ["t1", "t2"] {
        store
            .create_task(
                &NewTask {
                    id,
                    title: "작업",
                    brief: "내용",
                    initiator: Initiator::Human,
                    source: "chat",
                    run_id: None,
                },
                at,
            )
            .unwrap();
    }
    assert!(
        store
            .move_task("t1", TaskStatus::Running, "시작", at)
            .unwrap()
    );
    store.inbox_add("a", "{}", at).unwrap();
    store.inbox_add("b", "{}", at).unwrap();
    store.inbox_add("c", "{}", at).unwrap();
    store.set_setting("runtime.running", "{}", at).unwrap();
    store
}

#[test]
fn store_status_prints_exactly_the_three_lines() {
    let dir = TempDir::new("status");
    let db = dir.path("lucia.sqlite");
    // 데몬이 열어 둔 저장소처럼, 연 채로 읽는다.
    let _store = store_with_work(&db);

    let out = luciad(&dir, &[Path::new("--db"), &db], "store-status");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "running_tasks=1\ninbox=3\nrun_in_flight=1\n",
        "{}",
        stderr(&out)
    );
}

#[test]
fn store_status_of_a_missing_file_fails_and_creates_nothing() {
    let dir = TempDir::new("status-missing");
    let db = dir.path("nope.sqlite");

    let out = luciad(&dir, &[Path::new("--db"), &db], "store-status");
    assert!(!out.status.success());
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("저장소 상태를 읽지 못했다"),
        "{}",
        stderr(&out)
    );
    assert!(!db.exists());
}

#[test]
fn store_backup_makes_a_private_copy_and_reports_it() {
    let dir = TempDir::new("backup");
    let db = dir.path("lucia.sqlite");
    let _store = store_with_work(&db);
    let copy = dir.path("backup.sqlite");

    let out = luciad(
        &dir,
        &[Path::new("--db"), &db, Path::new("--out"), &copy],
        "store-backup",
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let line = stdout(&out);
    let prefix = format!("백업: {} (", copy.display());
    assert!(
        line.starts_with(&prefix) && line.ends_with(")\n") && line.lines().count() == 1,
        "{line:?}"
    );
    assert_eq!(
        std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
        0o600
    );

    // 사본으로도 같은 상태가 읽힌다.
    let out = luciad(&dir, &[Path::new("--db"), &copy], "store-status");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "running_tasks=1\ninbox=3\nrun_in_flight=1\n");
}

#[test]
fn store_backup_refuses_to_overwrite_and_fails_on_a_missing_db() {
    let dir = TempDir::new("backup-fail");
    let db = dir.path("lucia.sqlite");
    let _store = store_with_work(&db);
    let copy = dir.path("backup.sqlite");
    std::fs::write(&copy, b"keep me").unwrap();

    let out = luciad(
        &dir,
        &[Path::new("--db"), &db, Path::new("--out"), &copy],
        "store-backup",
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("저장소 백업 실패"),
        "{}",
        stderr(&out)
    );
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    assert_eq!(std::fs::read(&copy).unwrap(), b"keep me");

    let missing = dir.path("nope.sqlite");
    let copy2 = dir.path("backup2.sqlite");
    let out = luciad(
        &dir,
        &[Path::new("--db"), &missing, Path::new("--out"), &copy2],
        "store-backup",
    );
    assert!(!out.status.success());
    assert!(!missing.exists() && !copy2.exists());
}
