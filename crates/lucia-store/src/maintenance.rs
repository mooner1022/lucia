//! 저장소 파일을 바깥에서 다루는 일: 일관된 사본 만들기와 상태 읽기. 배포 스크립트가 `sqlite3` 명령 없이
//! 쓰는 길이다(`luciad store-backup`, `luciad store-status`).
//!
//! 둘 다 데몬이 같은 파일을 쓰는 중이어도 된다(WAL이라 읽는 쪽이 쓰는 쪽을 막지 않는다). 원본은 읽기 전용으로만
//! 열고 마이그레이션하지 않는다(`Store::open`을 쓰지 않는다). 새 바이너리가 저장소를 바꾸기 전의 모습을 그대로
//! 남기려는 것이고, 파일이 없을 때 빈 저장소를 만들어 버리지 않으려는 것이다.

use std::ffi::OsString;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};

use crate::{Result, StoreError};

/// 하는 중인 정기 작업을 적어 두는 설정 키. 데몬의 `engine::recovery::RUNNING_KEY`와 같은 값이다
/// (luciad의 시험이 같은지 본다).
pub const RUNTIME_RUNNING_KEY: &str = "runtime.running";

/// 다른 연결이 잠시 쥔 자물쇠를 기다리는 시간.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// 복사 전체의 제한 시간. 넘으면 사본을 지우고 실패한다.
const COPY_DEADLINE: Duration = Duration::from_secs(120);
/// 자물쇠를 못 잡았을 때(Busy·Locked) 다시 해 보기 전에 쉬는 시간.
const RETRY_PAUSE: Duration = Duration::from_millis(100);
/// 한 번에 복사하는 쪽수. 음수는 전부다. 데몬이 도는 동안에는 한 번에 끝내야 한다. 나눠서 복사하면 단계 사이에
/// 다른 연결이 커밋할 때마다 처음부터 다시 시작해서(SQLite 백업 API의 규칙), 쓰기가 잦으면 끝나지 않을 수 있다.
/// WAL이라 한 번의 읽기 트랜잭션은 쓰는 쪽을 막지 않고, 시작한 시점의 일관된 모습을 준다.
const PAGES_PER_STEP: std::os::raw::c_int = -1;

fn file_err(what: &str, path: &Path, err: impl std::fmt::Display) -> StoreError {
    StoreError::File(format!("{what}: {}: {err}", path.display()))
}

/// 원본을 읽기 전용으로 연다. 없으면 만들지 않고 실패한다(CREATE 플래그를 주지 않는다).
fn open_read_only(path: &Path) -> Result<Connection> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => {
            return Err(StoreError::File(format!(
                "저장소가 파일이 아니다: {}",
                path.display()
            )));
        }
        Err(e) => return Err(file_err("저장소 파일을 열 수 없다", path, e)),
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

/// 이 경로에 딸린 SQLite 부속 파일(`-journal`, `-wal`, `-shm`).
fn sidecars(path: &Path) -> [PathBuf; 3] {
    ["-journal", "-wal", "-shm"].map(|suffix| {
        let mut name: OsString = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    })
}

/// 만들다 만 사본을 지운다. `keep`을 부르지 않고 떨어지면(오류·패닉) 사본과 부속 파일을 지운다.
struct PartialCopy<'a> {
    dest: &'a Path,
    keep: bool,
}

impl PartialCopy<'_> {
    fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for PartialCopy<'_> {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let _ = std::fs::remove_file(self.dest);
        for side in sidecars(self.dest) {
            let _ = std::fs::remove_file(side);
        }
    }
}

/// 사본 자리를 새 파일로만 만든다(있으면 실패, 덮어쓰지 않는다). 메일 내용과 기억이 들어 있으므로 0600이다.
fn create_private(dest: &Path) -> Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dest)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                StoreError::File(format!(
                    "사본 자리에 이미 파일이 있다(덮어쓰지 않는다): {}",
                    dest.display()
                ))
            } else {
                file_err("사본 파일을 만들 수 없다", dest, e)
            }
        })?;
    // 만들 때의 umask와 상관없이 정확히 0600으로 맞춘다.
    std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| file_err("사본 권한을 정할 수 없다", dest, e))
}

/// SQLite 온라인 백업으로 원본의 일관된 모습을 사본 파일로 옮긴다.
fn copy_pages(src: &Connection, dest_path: &Path) -> Result<()> {
    let mut dest = Connection::open_with_flags(
        dest_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    dest.busy_timeout(BUSY_TIMEOUT)?;
    {
        let backup = Backup::new(src, &mut dest)?;
        let deadline = Instant::now() + COPY_DEADLINE;
        loop {
            match backup.step(PAGES_PER_STEP)? {
                StepResult::Done => break,
                StepResult::More => {}
                // Busy·Locked(와 앞으로 생길 값)은 잠시 뒤 다시 한다.
                _ => std::thread::sleep(RETRY_PAUSE),
            }
            if Instant::now() > deadline {
                return Err(StoreError::File(format!(
                    "저장소 복사가 {}초 안에 끝나지 않았다",
                    COPY_DEADLINE.as_secs()
                )));
            }
        }
    }
    dest.close().map_err(|(_, e)| e)?;
    Ok(())
}

/// 사본을 다시 열어 부속 파일 없이 한 파일로 끝나게 하고(저널 방식 DELETE), 무결성을 확인한다.
///
/// 원본의 머리 쪽이 WAL이라는 표시까지 그대로 복사되므로, 이렇게 하지 않으면 사본을 읽을 때마다 `-wal`·`-shm`
/// 이 곁에 생겨 남는다. 사본을 복원할 때는 `Store::open`이 다시 WAL로 바꾼다.
fn finish_copy(dest_path: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(
        dest_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let mode: String = conn.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(StoreError::File(format!(
            "사본의 저널 방식을 바꾸지 못했다: {mode}"
        )));
    }
    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    if rows != ["ok"] {
        let shown: Vec<_> = rows.iter().take(3).map(String::as_str).collect();
        return Err(StoreError::File(format!(
            "사본이 무결성 검사를 통과하지 못했다: {}",
            shown.join(" / ")
        )));
    }
    conn.close().map_err(|(_, e)| e)?;
    Ok(())
}

/// 저장소 `src`의 일관된 사본을 새 파일 `dest`로 만든다.
///
/// - `src`는 읽기 전용으로만 연다. 없으면 만들지 않고 실패하고, 마이그레이션하지 않는다.
/// - 데몬이 같은 파일에 쓰는 중이어도 된다. 복사를 시작한 시점에 커밋된 것만, 한꺼번에 담는다.
/// - `dest`는 0600으로 새로 만든다. 이미 있으면 덮어쓰지 않고 실패한다(있던 파일은 건드리지 않는다).
/// - 복사한 뒤 사본을 열어 `PRAGMA integrity_check`가 `ok` 하나인지 보고, 아니면 사본을 지우고 실패한다.
///   어떤 실패에서도 만들다 만 파일을 남기지 않는다.
pub fn backup_file(src: &Path, dest: &Path) -> Result<()> {
    // 원본부터 연다: 원본이 없을 때 사본 파일이 생기지 않게.
    let src_conn = open_read_only(src)?;
    create_private(dest)?;
    // 여기부터는 실패하면 사본을 지운다(위에서 실패한 경우는 있던 파일이므로 지우지 않는다).
    let partial = PartialCopy { dest, keep: false };
    copy_pages(&src_conn, dest)?;
    drop(src_conn);
    finish_copy(dest)?;
    // 디스크에 내려갔는지까지 확인한 사본만 남긴다.
    std::fs::File::open(dest)
        .and_then(|f| f.sync_all())
        .map_err(|e| file_err("사본을 디스크에 쓰지 못했다", dest, e))?;
    if let Some(dir) = dest.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    }
    partial.keep();
    Ok(())
}

/// 배포 전에 보는 저장소 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StoreStatus {
    /// 하는 중(`running`)인 작업 수. 재시작하면 실패로 닫힌다.
    pub running_tasks: i64,
    /// 받은 편지함의 메시지 수(기다리는 것과 처리하는 것).
    pub inbox: i64,
    /// 정기 작업이 하는 중인가(설정 [`RUNTIME_RUNNING_KEY`]가 있다).
    pub run_in_flight: bool,
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |r| r.get(0),
    )?)
}

/// 표가 있으면 `sql`의 수를, 없으면(옛 스키마) 0을 돌려준다. 그 밖의 오류는 그대로 돌려준다.
fn count_in(conn: &Connection, table: &str, sql: &str, args: &[&str]) -> Result<i64> {
    if !table_exists(conn, table)? {
        return Ok(0);
    }
    Ok(conn.query_row(sql, rusqlite::params_from_iter(args), |r| r.get(0))?)
}

/// 저장소 `src`의 상태를 읽는다. 읽기 전용이고 마이그레이션하지 않는다(데몬이 돌고 있어도 된다).
///
/// 세 값은 한 번의 읽기 트랜잭션에서 읽어 서로 맞는다. 표가 아직 없는 옛 스키마는 0(꺼짐)으로 보고, 그 밖의
/// 오류(깨진 파일, 칸이 다른 표 등)는 그대로 돌려준다.
pub fn status(src: &Path) -> Result<StoreStatus> {
    let conn = open_read_only(src)?;
    let tx = conn.unchecked_transaction()?;
    let running_tasks = count_in(
        &tx,
        "tasks",
        "SELECT count(*) FROM tasks WHERE status = 'running'",
        &[],
    )?;
    let inbox = count_in(&tx, "inbox", "SELECT count(*) FROM inbox", &[])?;
    let in_flight = count_in(
        &tx,
        "settings",
        "SELECT count(*) FROM settings WHERE key = ?1",
        &[RUNTIME_RUNNING_KEY],
    )?;
    tx.commit()?;
    Ok(StoreStatus {
        running_tasks,
        inbox,
        run_in_flight: in_flight > 0,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use chrono::{DateTime, TimeZone, Utc};
    use lucia_core::{Initiator, TaskStatus};

    use super::*;
    use crate::{NewEpisode, NewTask, Store};

    /// 시험마다 따로 쓰는 임시 디렉터리(끝나면 지운다).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "lucia-maint-{}-{}-{}",
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

        /// 디렉터리의 파일 이름들(정렬).
        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn at(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 21, h, 0, 0).unwrap()
    }

    fn open_ro(path: &Path) -> Connection {
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// 진짜 스키마(마이그레이션 전부)와 WAL 방식을 갖춘 저장소에 몇 가지를 커밋해 둔다.
    fn seeded_store(path: &Path) -> Store {
        let store = Store::open(path).unwrap();
        store
            .add_episode(&NewEpisode::user("대화-1", "안녕", at(9)))
            .unwrap();
        for i in 0..3 {
            store.set_setting(&format!("k{i}"), "v", at(9)).unwrap();
        }
        store
    }

    #[test]
    fn backup_leaves_out_uncommitted_rows_of_a_writer_in_the_middle_of_a_transaction() {
        let dir = TempDir::new("midwrite");
        let src = dir.path("lucia.sqlite");
        let store = seeded_store(&src);
        let mode: String = Connection::open(&src)
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal", "운영과 같은 WAL 저장소로 시험한다");

        // 데몬 말고 다른 연결이 쓰기 트랜잭션을 연 채로 여러 줄을 넣고 있다.
        let writer = Connection::open(&src).unwrap();
        writer.busy_timeout(Duration::from_secs(5)).unwrap();
        writer
            .execute_batch(
                "BEGIN IMMEDIATE;
                 INSERT INTO settings (key, value, updated_at) VALUES ('w1', 'x', 't');
                 INSERT INTO settings (key, value, updated_at) VALUES ('w2', 'x', 't');
                 INSERT INTO settings (key, value, updated_at) VALUES ('w3', 'x', 't');",
            )
            .unwrap();

        let dest = dir.path("copy.sqlite");
        backup_file(&src, &dest).unwrap();

        let copy = open_ro(&dest);
        assert_eq!(count(&copy, "SELECT count(*) FROM episodes"), 1);
        assert_eq!(
            count(&copy, "SELECT count(*) FROM settings WHERE key LIKE 'k%'"),
            3,
            "백업 시작 전에 커밋된 줄은 모두 있다"
        );
        assert_eq!(
            count(&copy, "SELECT count(*) FROM settings WHERE key LIKE 'w%'"),
            0,
            "커밋되지 않은 줄은 하나도 없다(일부만 있지도 않다)"
        );
        assert_eq!(
            count(&copy, "PRAGMA user_version"),
            i64::from(store.schema_version().unwrap()),
            "마이그레이션하지 않고 원본 그대로의 버전이다"
        );

        // 커밋한 뒤의 새 사본에는 들어 있다. 앞의 사본은 그대로다.
        writer.execute_batch("COMMIT").unwrap();
        let dest2 = dir.path("copy2.sqlite");
        backup_file(&src, &dest2).unwrap();
        assert_eq!(
            count(
                &open_ro(&dest2),
                "SELECT count(*) FROM settings WHERE key LIKE 'w%'"
            ),
            3
        );
        assert_eq!(
            count(&copy, "SELECT count(*) FROM settings WHERE key LIKE 'w%'"),
            0
        );
    }

    #[test]
    fn backup_stays_consistent_while_a_writer_keeps_committing() {
        let dir = TempDir::new("parallel");
        let src = dir.path("lucia.sqlite");
        let _store = seeded_store(&src);

        // 쓰는 쪽: 한 트랜잭션에 a·b를 한 쌍으로 넣고 커밋하기를 계속한다(쌍이 깨진 사본은 일관되지 않은 것이다).
        let committed = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (src, committed, stop) = (src.clone(), committed.clone(), stop.clone());
            std::thread::spawn(move || {
                let conn = Connection::open(&src).unwrap();
                conn.busy_timeout(Duration::from_secs(10)).unwrap();
                let mut i = 0;
                while !stop.load(Ordering::Relaxed) {
                    conn.execute_batch(&format!(
                        "BEGIN IMMEDIATE;
                         INSERT INTO settings (key, value, updated_at) VALUES ('a{i}', 'x', 't');
                         INSERT INTO settings (key, value, updated_at) VALUES ('b{i}', 'x', 't');
                         COMMIT;"
                    ))
                    .unwrap();
                    i += 1;
                    committed.store(i, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
        };
        let started = Instant::now();
        while committed.load(Ordering::Relaxed) < 5 {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "쓰는 쪽이 멈췄다"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        for n in 0..4 {
            let before = committed.load(Ordering::Relaxed) as i64;
            let dest = dir.path(&format!("copy{n}.sqlite"));
            backup_file(&src, &dest).unwrap();
            let copy = open_ro(&dest);
            let a = count(&copy, "SELECT count(*) FROM settings WHERE key LIKE 'a%'");
            let b = count(&copy, "SELECT count(*) FROM settings WHERE key LIKE 'b%'");
            assert_eq!(a, b, "한 트랜잭션의 두 줄은 함께 있거나 함께 없다");
            assert!(
                a >= before,
                "시작 전에 커밋된 {before}쌍이 모두 있다(있는 것: {a})"
            );
        }
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
    }

    #[test]
    fn backup_of_a_cleanly_closed_store_works() {
        // 데몬이 멈춘 채로 배포하는 길: 저장소를 연 연결이 없다.
        let dir = TempDir::new("closed");
        let src = dir.path("lucia.sqlite");
        drop(seeded_store(&src));
        assert_eq!(dir.names(), ["lucia.sqlite"], "닫힌 저장소는 파일 하나다");

        let dest = dir.path("copy.sqlite");
        backup_file(&src, &dest).unwrap();
        assert_eq!(count(&open_ro(&dest), "SELECT count(*) FROM episodes"), 1);
        // 사본 곁에는 아무것도 남지 않는다. (읽기 전용으로 연 원본 곁에는 비어 있는 -wal·-shm이 남을 수 있다:
        // 읽기 전용 연결은 마지막으로 닫혀도 WAL 파일을 지우지 못한다. 다음에 연 데몬이 그대로 쓴다.)
        assert!(
            dir.names().iter().all(|n| !n.starts_with("copy.sqlite-")),
            "{:?}",
            dir.names()
        );
        // 원본은 그대로 열리고 안의 것도 그대로다.
        let again = Store::open(&src).unwrap();
        assert_eq!(again.search_episodes("안녕", 10).unwrap().len(), 1);
    }

    #[test]
    fn copy_is_a_single_file_in_delete_journal_mode() {
        let dir = TempDir::new("sidecar");
        let src = dir.path("lucia.sqlite");
        let _store = seeded_store(&src);
        let dest = dir.path("copy.sqlite");
        backup_file(&src, &dest).unwrap();

        let leftovers = |dir: &TempDir| -> Vec<String> {
            dir.names()
                .into_iter()
                .filter(|n| n.starts_with("copy.sqlite-"))
                .collect()
        };
        assert!(leftovers(&dir).is_empty(), "{:?}", dir.names());
        let mode: String = open_ro(&dest)
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "delete");
        // 사본을 읽는 것(상태 보기)도 곁에 파일을 남기지 않는다.
        status(&dest).unwrap();
        assert!(leftovers(&dir).is_empty(), "{:?}", dir.names());
        // 사본은 일반 저장소처럼 다시 열린다(복원 경로).
        let restored = Store::open(&dest).unwrap();
        assert_eq!(restored.search_episodes("안녕", 10).unwrap().len(), 1);
    }

    #[test]
    fn existing_dest_is_refused_and_left_untouched() {
        let dir = TempDir::new("exists");
        let src = dir.path("lucia.sqlite");
        let _store = seeded_store(&src);
        let dest = dir.path("copy.sqlite");
        std::fs::write(&dest, b"keep me").unwrap();

        let err = backup_file(&src, &dest).unwrap_err();
        assert!(err.to_string().contains("이미 파일이 있다"), "{err}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"keep me");
        assert!(
            !dir.names().iter().any(|n| n.starts_with("copy.sqlite-")),
            "{:?}",
            dir.names()
        );
    }

    #[test]
    fn missing_src_fails_and_creates_nothing() {
        let dir = TempDir::new("missing");
        let src = dir.path("nope.sqlite");
        let dest = dir.path("copy.sqlite");

        let err = backup_file(&src, &dest).unwrap_err();
        assert!(err.to_string().contains("열 수 없다"), "{err}");
        assert!(dir.names().is_empty(), "{:?}", dir.names());
        assert!(status(&src).is_err());
        assert!(dir.names().is_empty(), "{:?}", dir.names());
    }

    #[test]
    fn unwritable_dest_directory_fails_without_leftovers() {
        let dir = TempDir::new("nodir");
        let src = dir.path("lucia.sqlite");
        let _store = seeded_store(&src);
        let before = dir.names();
        let dest = dir.path("no-such-dir/copy.sqlite");

        assert!(backup_file(&src, &dest).is_err());
        assert_eq!(dir.names(), before);
    }

    #[test]
    fn dest_is_private_0600() {
        let dir = TempDir::new("mode");
        let src = dir.path("lucia.sqlite");
        let _store = seeded_store(&src);
        let dest = dir.path("copy.sqlite");
        backup_file(&src, &dest).unwrap();
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }

    #[test]
    fn non_database_src_fails_and_leaves_no_copy() {
        let dir = TempDir::new("garbage");
        let src = dir.path("lucia.sqlite");
        std::fs::write(&src, vec![0x61u8; 8192]).unwrap();
        let dest = dir.path("copy.sqlite");

        assert!(backup_file(&src, &dest).is_err());
        assert_eq!(dir.names(), ["lucia.sqlite"]);
        assert!(status(&src).is_err());
    }

    /// 원본 파일의 첫 `needle`을 같은 길이의 다른 글자로 덮는다.
    fn overwrite_first(path: &Path, needle: &[u8], with: u8) {
        let mut bytes = std::fs::read(path).unwrap();
        let at = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("바꿀 글자가 파일에 있다");
        bytes[at..at + needle.len()].fill(with);
        std::fs::write(path, bytes).unwrap();
    }

    /// 사본 쪽 이름이 남아 있는지(원본 곁의 부속 파일은 세지 않는다).
    fn copy_files(dir: &TempDir) -> Vec<String> {
        dir.names()
            .into_iter()
            .filter(|n| n.starts_with("copy.sqlite"))
            .collect()
    }

    #[test]
    fn copy_that_fails_the_integrity_check_is_deleted() {
        // 표와 인덱스가 어긋난 저장소: 쪽을 그대로 옮기는 백업은 알아채지 못하고, 사본의 integrity_check가
        // 오류가 아니라 결과 줄로 알려 준다.
        let dir = TempDir::new("mismatch");
        let src = dir.path("lucia.sqlite");
        {
            let conn = Connection::open(&src).unwrap();
            conn.execute_batch(
                "PRAGMA journal_mode = DELETE;
                 CREATE TABLE t (a TEXT);
                 CREATE INDEX t_a ON t (a);
                 INSERT INTO t VALUES ('qqqqqqqqqqqq-1'), ('qqqqqqqqqqqq-2'), ('qqqqqqqqqqqq-3');",
            )
            .unwrap();
        }
        overwrite_first(&src, b"qqqqqqqqqqqq-2", b'z');
        let dest = dir.path("copy.sqlite");

        let err = backup_file(&src, &dest).unwrap_err();
        assert!(err.to_string().contains("무결성 검사"), "{err}");
        assert!(copy_files(&dir).is_empty(), "{:?}", dir.names());
    }

    #[test]
    fn copy_of_a_malformed_store_fails_and_leaves_no_copy() {
        let dir = TempDir::new("malformed");
        let src = dir.path("lucia.sqlite");
        drop(seeded_store(&src));
        // 닫힌 저장소(WAL이 비었다)의 안쪽 쪽 하나를 쓰레기로 덮는다.
        {
            use std::io::{Seek, SeekFrom, Write};
            let page = 4096u64;
            let mut f = std::fs::OpenOptions::new().write(true).open(&src).unwrap();
            f.seek(SeekFrom::Start(page * 6)).unwrap();
            f.write_all(&[0xABu8; 4096]).unwrap();
        }
        let dest = dir.path("copy.sqlite");

        let err = backup_file(&src, &dest).unwrap_err();
        assert!(copy_files(&dir).is_empty(), "{err}: {:?}", dir.names());
    }

    #[test]
    fn status_counts_running_tasks_inbox_and_in_flight_run() {
        let dir = TempDir::new("status");
        let src = dir.path("lucia.sqlite");
        let store = Store::open(&src).unwrap();
        assert_eq!(status(&src).unwrap(), StoreStatus::default());

        for id in ["t1", "t2", "t3"] {
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
                    at(9),
                )
                .unwrap();
        }
        // t1, t2는 하는 중, t3는 만들기만 했다(세지 않는다).
        assert!(
            store
                .move_task("t1", TaskStatus::Running, "시작", at(10))
                .unwrap()
        );
        assert!(
            store
                .move_task("t2", TaskStatus::Running, "시작", at(10))
                .unwrap()
        );
        store.inbox_add("a", "{}", at(10)).unwrap();
        store.inbox_add("b", "{}", at(11)).unwrap();
        store.inbox_start("b", at(11)).unwrap();
        store
            .set_setting(RUNTIME_RUNNING_KEY, "{\"due\":1}", at(11))
            .unwrap();
        store
            .set_setting("runtime.running.other", "x", at(11))
            .unwrap();

        assert_eq!(
            status(&src).unwrap(),
            StoreStatus {
                running_tasks: 2,
                inbox: 2,
                run_in_flight: true
            }
        );

        store.inbox_done("a").unwrap();
        store.delete_setting(RUNTIME_RUNNING_KEY).unwrap();
        assert_eq!(
            status(&src).unwrap(),
            StoreStatus {
                running_tasks: 2,
                inbox: 1,
                run_in_flight: false
            },
            "비슷한 이름의 다른 설정은 세지 않는다"
        );
    }

    #[test]
    fn status_reports_zeros_for_a_schema_without_those_tables() {
        let dir = TempDir::new("oldschema");
        let src = dir.path("old.sqlite");
        {
            let conn = Connection::open(&src).unwrap();
            conn.execute_batch("CREATE TABLE episodes (id INTEGER PRIMARY KEY, body TEXT);")
                .unwrap();
        }
        assert_eq!(status(&src).unwrap(), StoreStatus::default());

        // 표 하나만 있는 중간 스키마: 있는 표만 센다.
        {
            let conn = Connection::open(&src).unwrap();
            conn.execute_batch(&format!(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT NOT NULL);
                 INSERT INTO settings VALUES ('{RUNTIME_RUNNING_KEY}', 'x', 't');"
            ))
            .unwrap();
        }
        assert_eq!(
            status(&src).unwrap(),
            StoreStatus {
                running_tasks: 0,
                inbox: 0,
                run_in_flight: true
            }
        );
    }

    #[test]
    fn status_on_an_empty_file_is_zeros_and_does_not_modify_it() {
        let dir = TempDir::new("empty");
        let src = dir.path("lucia.db");
        std::fs::write(&src, b"").unwrap();
        assert_eq!(status(&src).unwrap(), StoreStatus::default());
        assert_eq!(std::fs::metadata(&src).unwrap().len(), 0);
        assert_eq!(dir.names(), ["lucia.db"]);
    }

    #[test]
    fn status_returns_errors_other_than_a_missing_table() {
        let dir = TempDir::new("badcolumn");
        let src = dir.path("odd.sqlite");
        {
            let conn = Connection::open(&src).unwrap();
            // tasks 표는 있지만 status 칸이 없다: 옛 스키마가 아니라 이상한 파일이다.
            conn.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY);")
                .unwrap();
        }
        let err = status(&src).unwrap_err();
        assert!(matches!(err, StoreError::Sqlite(_)), "{err}");
    }

    #[test]
    fn status_reads_while_a_writer_is_in_the_middle_of_a_transaction() {
        let dir = TempDir::new("statuswrite");
        let src = dir.path("lucia.sqlite");
        let store = Store::open(&src).unwrap();
        store.inbox_add("a", "{}", at(9)).unwrap();
        let writer = Connection::open(&src).unwrap();
        writer
            .execute_batch(
                "BEGIN IMMEDIATE;
                 INSERT INTO inbox (key, payload, state, enqueued_at) VALUES ('b', '{}', 'queued', 't');",
            )
            .unwrap();
        assert_eq!(status(&src).unwrap().inbox, 1);
        writer.execute_batch("COMMIT").unwrap();
        assert_eq!(status(&src).unwrap().inbox, 2);
    }
}
