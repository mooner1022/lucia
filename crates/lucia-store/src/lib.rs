//! SQLite 저장소.
//!
//! 공유 상태는 이 파일 하나에 모은다(명세 §운영). 동기 API(rusqlite)이고, 데몬은 블로킹 작업으로
//! 감싸서 부른다. 시각은 모두 UTC로 저장한다.
//!
//! 마이그레이션은 `migrations/NNNN_*.sql`을 순서대로 적용하고 `PRAGMA user_version`으로 버전을 센다.

mod admin;
mod approvals;
mod attention;
mod calendar;
mod character;
mod clients;
mod episodes;
mod expressions;
mod files;
mod followups;
mod goals;
mod images;
mod inbox;
mod ledger;
pub mod maintenance;
mod memory;
mod notices;
mod routines;
mod schedules;
mod sessions;
mod signals;
mod site_grants;
mod skills;
mod sources;
mod tasks;
mod watches;

use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::Connection;

pub use admin::{Alert, keys};
pub use approvals::{Activity, Approval, ApprovalStatus, Decided, NewApproval};
pub use character::CharacterRevision;
pub use clients::{Client, OutboxFrame};
pub use episodes::{Episode, EpisodeKind, NewEpisode, Origin};
pub use expressions::{
    Expression, ExpressionCategory, ExpressionFile, name_key as expression_name_key,
};
pub use files::{FileRecord, KeptFile};
pub use followups::{Followup, FollowupStatus, NewFollowup};
pub use images::GeneratedImage;
pub use inbox::InboxItem;
pub use ledger::{RunRecord, RunTool, SpendSummary};
pub use memory::{
    MemoryAction, MemoryAuthor, MemoryEntry, MemoryError, MemoryRevision, MemorySection,
    MemoryWrite, WriteOutcome,
};
/// 기억 항목 크기 상한(결정 0007).
pub mod memory_limits {
    pub use crate::memory::{
        BODY_MAX_BYTES, MAX_ENTRIES, REASON_MAX, SLUG_MAX, SUMMARY_MAX, TITLE_MAX,
    };
}
pub use notices::Notice;
pub use routines::{MAX_ROUTINES, NewRoutine, Routine, RoutineRun, RoutineStatus, Trigger};
pub use schedules::{Schedule, ScheduleKind, ScheduleOutcome};
pub use sessions::{SessionRole, SessionRow};
pub use site_grants::{
    IDLE_DAYS as SITE_GRANT_IDLE_DAYS, MAX_SITE_GRANTS, NewSiteGrant, SiteGrant,
    covers as host_covers, valid_host,
};
pub use skills::{Skill, SkillError, SkillOrigin, SkillRevision, SkillStatus, SkillWrite};
pub mod skill_limits {
    pub use crate::skills::{
        BODY_MAX_BYTES, DESCRIPTION_MAX, MAX_SKILLS, SCRIPT_MAX_BYTES, TITLE_MAX,
    };
}
pub use attention::{MAX_OPEN_NUDGES, NoticeRule, Nudge};
pub use calendar::CalendarEdit;
pub use goals::{Goal, GoalChild, GoalLog, GoalStatus, MAX_OPEN_GOALS, NewGoal};
pub use signals::{NewSignal, Signal, verdict as signal_verdict};
pub use sources::{NewSourceChange, SourceChange, SourceState};
pub use tasks::{NewTask, Task, TaskEvent};
pub use watches::{
    MAX_WATCHES, NewWatch, Watch, WatchEdit, WatchStatus, fetcher as watch_fetcher,
    mode as watch_mode, notify as watch_notify, paused as watch_paused,
};

/// 순서대로 적용할 마이그레이션.
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_admin.sql"),
    include_str!("../migrations/0003_assist.sql"),
    include_str!("../migrations/0004_compaction.sql"),
    include_str!("../migrations/0005_files.sql"),
    include_str!("../migrations/0006_memories.sql"),
    include_str!("../migrations/0007_character.sql"),
    include_str!("../migrations/0008_followups.sql"),
    include_str!("../migrations/0009_tasks.sql"),
    include_str!("../migrations/0010_notices.sql"),
    include_str!("../migrations/0011_run_tools.sql"),
    include_str!("../migrations/0012_skills.sql"),
    include_str!("../migrations/0013_watches.sql"),
    include_str!("../migrations/0014_dev_tasks.sql"),
    include_str!("../migrations/0015_clients.sql"),
    include_str!("../migrations/0016_scripts_routines.sql"),
    include_str!("../migrations/0017_site_grants.sql"),
    include_str!("../migrations/0018_signals.sql"),
    include_str!("../migrations/0019_goals.sql"),
    include_str!("../migrations/0020_calendar_edits.sql"),
    include_str!("../migrations/0021_attention.sql"),
    include_str!("../migrations/0022_board_watch.sql"),
    include_str!("../migrations/0023_security_mode.sql"),
    include_str!("../migrations/0024_app_link.sql"),
    include_str!("../migrations/0025_expressions.sql"),
    include_str!("../migrations/0026_generated_images.sql"),
    include_str!("../migrations/0027_image_inputs.sql"),
    include_str!("../migrations/0028_inbox.sql"),
    include_str!("../migrations/0029_kept_files.sql"),
];

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("저장된 값이 잘못됐다: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Memory(#[from] MemoryError),
    /// 받을 수 없는 값(사람이 읽을 까닭).
    #[error("{0}")]
    Invalid(String),
    /// 저장소 파일을 열거나 복사하다 실패했다(사람이 읽을 까닭, `maintenance`).
    #[error("{0}")]
    File(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Self::init(conn)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// 적용된 스키마 버전.
    pub fn schema_version(&self) -> Result<u32> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let version = index as u32 + 1;
        if version <= current {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

pub(crate) fn ts(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(crate) fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| StoreError::Corrupt(format!("시각 {s:?}: {e}")))
}

pub(crate) fn parse_opt_ts(s: Option<String>) -> Result<Option<DateTime<Utc>>> {
    s.as_deref().map(parse_ts).transpose()
}

#[cfg(test)]
pub(crate) mod test_util {
    use chrono::{DateTime, TimeZone, Utc};

    pub fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, h, m, 0).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_once() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len() as u32);
    }

    #[test]
    fn reopen_keeps_data_and_version() {
        let dir = std::env::temp_dir().join(format!("lucia-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.sqlite");
        let _ = std::fs::remove_file(&path);
        {
            let store = Store::open(&path).unwrap();
            store
                .add_episode(&NewEpisode::user("대화-1", "안녕", test_util::at(9, 0)))
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len() as u32);
        assert_eq!(store.search_episodes("안녕", 10).unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    /// 0004 이전 원장은 세션 누적 비용을 적었다. 마이그레이션이 세션별 차이로 고친다.
    #[test]
    fn migration_0004_turns_cumulative_cost_into_per_run_cost() {
        let conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..3] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 3).unwrap();
        conn.execute_batch(
            "INSERT INTO sessions (id, role, status, created_at) VALUES
                ('a', 'resident', 'active', '2026-11-16T00:00:00.000Z'),
                ('b', 'resident', 'rotated', '2026-11-15T00:00:00.000Z');
             INSERT INTO ledger (run_id, origin, purpose, started_at, finished_at, called_model, session_id, cost_usd, outcome) VALUES
                ('r1', 'chat', 'chat', 't', 't', 1, 'b', 0.010, 'ok'),
                ('r2', 'chat', 'chat', 't', 't', 1, 'a', 0.006, 'ok'),
                ('r3', 'routine', 'morning_briefing', 't', 't', 0, NULL, 0, 'no_change'),
                ('r4', 'chat', 'chat', 't', 't', 1, 'a', 0.0135, 'ok'),
                ('r5', 'chat', 'chat', 't', 't', 1, 'b', 0.030, 'ok'),
                ('r6', 'chat', 'chat', 't', 't', 1, 'a', 0.0217, 'ok');",
        )
        .unwrap();
        let store = Store::init(conn).unwrap();
        let costs: Vec<f64> = store
            .conn
            .prepare("SELECT cost_usd FROM ledger ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|c| c.unwrap())
            .collect();
        let want = [0.010, 0.006, 0.0, 0.0075, 0.020, 0.0082];
        for (got, want) in costs.iter().zip(want) {
            assert!((got - want).abs() < 1e-9, "{costs:?}");
        }
        let a = store
            .active_session(SessionRole::Resident)
            .unwrap()
            .unwrap();
        assert!(
            (a.cost_total_usd - 0.0217).abs() < 1e-9,
            "누적값은 마지막 값과 같다"
        );
        assert_eq!(a.runs_since_compact, 3);
    }

    /// 0006: 예전 한 덩어리 핵심 기억(마지막 개정)을 항목 하나로 그대로 옮기고, 원래 기록은 남긴다.
    #[test]
    fn migration_0006_moves_core_memory_into_one_entry() {
        let conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 5).unwrap();
        conn.execute_batch(
            "INSERT INTO core_memory_revisions (at, author, run_id, reason, body) VALUES
                ('2026-11-16T09:00:00.000Z', 'agent', NULL, '처음', '# 옛것'),
                ('2026-11-16T10:00:00.000Z', 'agent', 'r1', '고침', '# 사용자\n- 말투: 존댓말');",
        )
        .unwrap();
        let store = Store::init(conn).unwrap();
        let all = store.memories().unwrap();
        assert_eq!(all.len(), 1);
        let e = &all[0];
        assert_eq!(e.slug, "core-notes");
        assert_eq!(e.section, MemorySection::Me);
        assert_eq!(e.body, "# 사용자\n- 말투: 존댓말", "마지막 개정을 그대로");
        assert_eq!(
            e.revision,
            store.memory_history("core-notes", 5).unwrap()[0].id
        );
        let kept: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM core_memory_revisions", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kept, 2, "원래 기록은 남긴다");

        // 핵심 기억이 없던 저장소는 빈 채로.
        let conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 5).unwrap();
        assert!(Store::init(conn).unwrap().memories().unwrap().is_empty());
    }
}
