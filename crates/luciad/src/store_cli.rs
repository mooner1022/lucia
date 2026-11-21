//! `luciad store-backup`·`store-status`: 배포 스크립트(`deploy/install.sh`)가 `sqlite3` 명령 없이 저장소를
//! 다루는 길. 설정을 읽지 않고 저장소를 `Store::open`으로 열지도 않는다(마이그레이션하지 않는다). 경로는 인자로
//! 받고, 파일은 읽기 전용으로만 연다. 돌고 있는 데몬과 함께 써도 된다.

use std::path::Path;

use anyhow::anyhow;

/// 저장소 `db`의 일관된 사본을 `out`에 만들고 알릴 한 줄을 돌려준다.
pub fn backup(db: &Path, out: &Path) -> anyhow::Result<String> {
    lucia_store::maintenance::backup_file(db, out)
        .map_err(|e| anyhow!("저장소 백업 실패({}): {e}", db.display()))?;
    let size = std::fs::metadata(out)
        .map_err(|e| anyhow!("백업 파일을 읽을 수 없다({}): {e}", out.display()))?
        .len();
    Ok(format!(
        "백업: {} ({})",
        out.display(),
        crate::files::human_size(size)
    ))
}

/// 저장소 `db`의 상태를 셸 스크립트가 `grep`으로 읽는 세 줄로 돌려준다.
pub fn status(db: &Path) -> anyhow::Result<String> {
    let s = lucia_store::maintenance::status(db)
        .map_err(|e| anyhow!("저장소 상태를 읽지 못했다({}): {e}", db.display()))?;
    Ok(format_status(&s))
}

fn format_status(s: &lucia_store::maintenance::StoreStatus) -> String {
    format!(
        "running_tasks={}\ninbox={}\nrun_in_flight={}",
        s.running_tasks,
        s.inbox,
        u8::from(s.run_in_flight)
    )
}

#[cfg(test)]
mod tests {
    use lucia_store::maintenance::StoreStatus;

    use super::*;

    #[test]
    fn status_lines_are_the_three_fixed_keys() {
        let s = StoreStatus {
            running_tasks: 2,
            inbox: 0,
            run_in_flight: true,
        };
        assert_eq!(
            format_status(&s),
            "running_tasks=2\ninbox=0\nrun_in_flight=1"
        );
        assert_eq!(
            format_status(&StoreStatus::default()),
            "running_tasks=0\ninbox=0\nrun_in_flight=0"
        );
    }

    /// 저장소의 상태 확인이 보는 설정 키가 데몬이 정기 작업을 적는 키와 같아야 한다.
    #[test]
    fn running_key_is_the_one_the_daemon_writes() {
        assert_eq!(
            lucia_store::maintenance::RUNTIME_RUNNING_KEY,
            crate::engine::recovery::RUNNING_KEY
        );
    }
}
