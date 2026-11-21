//! 스킬(결정 0018): 성공한 작업에서 정리한 절차 메모와 그 생애 주기(검토 대기 → 시험 → 활성 → 오래됨 → 보관).
//!
//! 본문은 개정으로 남긴다. 고칠 때는 지금 개정 번호를 맞춰야 한다(그새 바뀌었으면 쓰지 않는다).

use chrono::{DateTime, Duration, Utc};
use rusqlite::{OptionalExtension as _, params};

use crate::{Result, Store, StoreError, parse_opt_ts, parse_ts, ts};

pub const TITLE_MAX: usize = 60;
pub const DESCRIPTION_MAX: usize = 200;
pub const BODY_MAX_BYTES: usize = 12 * 1024;
pub const MAX_SKILLS: usize = 100;
/// 스킬·루틴 스크립트(Python) 상한(결정 0025).
pub const SCRIPT_MAX_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SkillStatus {
    /// 사람의 승인을 기다린다(외부 쓰기나 사이트 열기가 들어 있다).
    Pending,
    /// 시험 중: 쓰면서 결과를 쌓는다.
    Trial,
    Active,
    /// 오래 쓰지 않았다(색인에서 빠진다).
    Stale,
    /// 보관(꺼짐).
    Archived,
}

impl SkillStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Trial => "trial",
            Self::Active => "active",
            Self::Stale => "stale",
            Self::Archived => "archived",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "trial" => Self::Trial,
            "active" => Self::Active,
            "stale" => Self::Stale,
            "archived" => Self::Archived,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "승인 대기",
            Self::Trial => "시험 중",
            Self::Active => "활성",
            Self::Stale => "오래됨",
            Self::Archived => "보관",
        }
    }

    /// 맥락 색인에 보이는가(쓸 수 있는가).
    pub fn usable(self) -> bool {
        matches!(self, Self::Trial | Self::Active)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Skill {
    pub slug: String,
    pub title: String,
    pub description: String,
    pub status: SkillStatus,
    pub pinned: bool,
    pub revision: i64,
    pub source: String,
    pub task_id: Option<String>,
    pub footprint: serde_json::Value,
    pub uses: i64,
    pub successes: i64,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// 지금 개정에 스크립트가 있으면 그것이 쓸 도구(결정 0025).
    pub script_tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SkillRevision {
    pub revision: i64,
    pub at: DateTime<Utc>,
    pub author: String,
    pub reason: String,
    pub body: String,
    pub script: Option<String>,
}

/// 쓰기 요청.
#[derive(Debug, Clone)]
pub struct SkillWrite<'a> {
    pub slug: &'a str,
    pub title: &'a str,
    pub description: &'a str,
    pub body: &'a str,
    /// 이 개정의 스크립트와 그것이 쓸 도구(결정 0025). 없으면 스크립트 없는 개정.
    pub script: Option<(&'a str, &'a [String])>,
}

/// 새로 만들 때의 정보.
#[derive(Debug, Clone)]
pub struct SkillOrigin<'a> {
    pub status: SkillStatus,
    pub source: &'a str,
    pub task_id: Option<&'a str>,
    pub footprint: &'a serde_json::Value,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SkillError {
    #[error("{field}: {why}")]
    Invalid { field: &'static str, why: String },
    #[error("개정 번호가 맞지 않는다(지금 {current:?}). skill_read로 다시 읽고 고친다")]
    Stale { current: Option<i64> },
    #[error("고정된 스킬은 사람만 고친다")]
    Pinned,
    #[error("스킬이 {MAX_SKILLS}개라 더 만들 수 없다")]
    Full,
}

const COLUMNS: &str = "slug, title, description, status, pinned, revision, source, task_id, footprint, uses, successes, last_used_at, created_at, updated_at, script_tools";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Skill>> {
    let status: String = r.get(3)?;
    let footprint: String = r.get(8)?;
    let last: Option<String> = r.get(11)?;
    let created: String = r.get(12)?;
    let updated: String = r.get(13)?;
    let script_tools: Option<String> = r.get(14)?;
    let base = (
        r.get::<_, String>(0)?,
        r.get::<_, String>(1)?,
        r.get::<_, String>(2)?,
        r.get::<_, i64>(4)?,
        r.get::<_, i64>(5)?,
        r.get::<_, String>(6)?,
        r.get::<_, Option<String>>(7)?,
        r.get::<_, i64>(9)?,
        r.get::<_, i64>(10)?,
    );
    Ok((|| {
        let (slug, title, description, pinned, revision, source, task_id, uses, successes) = base;
        Ok(Skill {
            status: SkillStatus::parse(&status)
                .ok_or_else(|| StoreError::Corrupt(format!("skill status {status:?}")))?,
            footprint: serde_json::from_str(&footprint).unwrap_or(serde_json::Value::Null),
            last_used_at: parse_opt_ts(last)?,
            created_at: parse_ts(&created)?,
            updated_at: parse_ts(&updated)?,
            script_tools: script_tools.map(|t| serde_json::from_str(&t).unwrap_or_default()),
            slug,
            title,
            description,
            pinned: pinned != 0,
            revision,
            source,
            task_id,
            uses,
            successes,
        })
    })())
}

fn one_line(field: &'static str, text: &str, max: usize) -> std::result::Result<(), SkillError> {
    let why = |w: String| SkillError::Invalid { field, why: w };
    if text.trim().is_empty() {
        return Err(why("비었다".into()));
    }
    if text.chars().count() > max {
        return Err(why(format!("{max}자까지")));
    }
    if text.contains(['\n', '\r']) {
        return Err(why("한 줄이어야 한다".into()));
    }
    Ok(())
}

impl Store {
    pub fn skill(&self, slug: &str) -> Result<Option<Skill>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM skills WHERE slug = ?1"),
                [slug],
                row,
            )
            .optional()?
            .transpose()
    }

    /// 스킬 목록(상태 순서 없이 이름순). `statuses`가 비면 모두.
    pub fn skills(&self, statuses: &[SkillStatus]) -> Result<Vec<Skill>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {COLUMNS} FROM skills ORDER BY slug"))?;
        let rows = stmt.query_map([], row)?;
        let mut out = Vec::new();
        for r in rows {
            let s = r??;
            if statuses.is_empty() || statuses.contains(&s.status) {
                out.push(s);
            }
        }
        Ok(out)
    }

    /// 지금 본문.
    pub fn skill_body(&self, slug: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT r.body FROM skill_revisions r JOIN skills s ON s.slug = r.slug AND s.revision = r.revision WHERE s.slug = ?1",
                [slug],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// 지금 개정의 스크립트와 그것이 쓸 도구(결정 0025).
    pub fn skill_script(&self, slug: &str) -> Result<Option<(String, Vec<String>)>> {
        let row: Option<(Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT r.script, s.script_tools FROM skill_revisions r JOIN skills s ON s.slug = r.slug AND s.revision = r.revision WHERE s.slug = ?1",
                [slug],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((Some(code), tools)) => Some((
                code,
                tools
                    .and_then(|t| serde_json::from_str(&t).ok())
                    .unwrap_or_default(),
            )),
            _ => None,
        })
    }

    pub fn skill_revisions(&self, slug: &str) -> Result<Vec<SkillRevision>> {
        let mut stmt = self.conn.prepare(
            "SELECT revision, at, author, reason, body, script FROM skill_revisions WHERE slug = ?1 ORDER BY revision DESC",
        )?;
        let rows = stmt.query_map([slug], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (revision, at, author, reason, body, script) = r?;
            out.push(SkillRevision {
                revision,
                at: parse_ts(&at)?,
                author,
                reason,
                body,
                script,
            });
        }
        Ok(out)
    }

    /// 스킬을 만들거나 고친다. 새로 만들면 `revision`은 `None`, 고치면 지금 개정 번호. 새 개정 번호를 돌려준다.
    /// 고정된 스킬은 사람(`author = "user"`)만 고친다.
    #[allow(clippy::too_many_arguments)]
    pub fn save_skill(
        &mut self,
        w: &SkillWrite<'_>,
        revision: Option<i64>,
        origin: &SkillOrigin<'_>,
        author: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<std::result::Result<i64, SkillError>> {
        if !crate::memory::valid_slug(w.slug) {
            return Ok(Err(SkillError::Invalid {
                field: "name",
                why: "영문 소문자·숫자·하이픈, 40자까지".into(),
            }));
        }
        for (field, text, max) in [
            ("title", w.title, TITLE_MAX),
            ("description", w.description, DESCRIPTION_MAX),
        ] {
            if let Err(e) = one_line(field, text, max) {
                return Ok(Err(e));
            }
        }
        if w.body.trim().is_empty() || w.body.len() > BODY_MAX_BYTES {
            return Ok(Err(SkillError::Invalid {
                field: "body",
                why: format!("1바이트~{BODY_MAX_BYTES}바이트"),
            }));
        }
        if let Some((code, tools)) = w.script
            && (code.trim().is_empty() || code.len() > SCRIPT_MAX_BYTES || tools.len() > 30)
        {
            return Ok(Err(SkillError::Invalid {
                field: "script",
                why: format!("1바이트~{SCRIPT_MAX_BYTES}바이트, 도구 30개까지"),
            }));
        }
        let script_tools = w
            .script
            .map(|(_, tools)| serde_json::to_string(tools).unwrap_or_else(|_| "[]".into()));
        let tx = self.conn.transaction()?;
        let current: Option<(i64, i64)> = tx
            .query_row(
                "SELECT revision, pinned FROM skills WHERE slug = ?1",
                [w.slug],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let next = match (current, revision) {
            (None, None) => {
                let count: i64 = tx.query_row("SELECT count(*) FROM skills", [], |r| r.get(0))?;
                if count as usize >= MAX_SKILLS {
                    return Ok(Err(SkillError::Full));
                }
                tx.execute(
                    "INSERT INTO skills (slug, title, description, status, revision, source, task_id, footprint, created_at, updated_at, script_tools)
                     VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8, ?8, ?9)",
                    params![
                        w.slug,
                        w.title.trim(),
                        w.description.trim(),
                        origin.status.as_str(),
                        origin.source,
                        origin.task_id,
                        origin.footprint.to_string(),
                        ts(now),
                        script_tools
                    ],
                )?;
                1
            }
            (Some((cur, pinned)), Some(expected)) if cur == expected => {
                if pinned != 0 && author != "user" {
                    return Ok(Err(SkillError::Pinned));
                }
                tx.execute(
                    "UPDATE skills SET title = ?2, description = ?3, revision = ?4, updated_at = ?5, script_tools = ?6 WHERE slug = ?1",
                    params![
                        w.slug,
                        w.title.trim(),
                        w.description.trim(),
                        cur + 1,
                        ts(now),
                        script_tools
                    ],
                )?;
                cur + 1
            }
            (cur, _) => {
                return Ok(Err(SkillError::Stale {
                    current: cur.map(|c| c.0),
                }));
            }
        };
        tx.execute(
            "INSERT INTO skill_revisions (slug, revision, at, author, reason, body, script) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![w.slug, next, ts(now), author, reason, w.body, w.script.map(|(c, _)| c)],
        )?;
        tx.commit()?;
        Ok(Ok(next))
    }

    pub fn set_skill_status(
        &self,
        slug: &str,
        status: SkillStatus,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE skills SET status = ?2, updated_at = ?3 WHERE slug = ?1",
            params![slug, status.as_str(), ts(now)],
        )? == 1)
    }

    pub fn set_skill_footprint(
        &self,
        slug: &str,
        footprint: &serde_json::Value,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE skills SET footprint = ?2, updated_at = ?3 WHERE slug = ?1",
            params![slug, footprint.to_string(), ts(now)],
        )? == 1)
    }

    pub fn set_skill_pinned(&self, slug: &str, pinned: bool, now: DateTime<Utc>) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE skills SET pinned = ?2, updated_at = ?3 WHERE slug = ?1",
            params![slug, pinned as i64, ts(now)],
        )? == 1)
    }

    /// 스킬을 읽은 실행을 남긴다.
    pub fn record_skill_use(
        &self,
        slug: &str,
        run_id: &str,
        task_id: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO skill_uses (slug, run_id, task_id, at) VALUES (?1, ?2, ?3, ?4)",
            params![slug, run_id, task_id, ts(now)],
        )?;
        self.conn.execute(
            "UPDATE skills SET uses = uses + 1, last_used_at = ?2 WHERE slug = ?1",
            params![slug, ts(now)],
        )?;
        Ok(())
    }

    /// 사람이 완료로 확인한 작업이 읽은 스킬마다 성공을 하나씩 센다(같은 작업은 한 번만).
    /// 시험 중인 스킬이 `promote_at`번 성공하면 활성으로 올린다. 센 스킬을 돌려준다.
    pub fn count_skill_successes(
        &self,
        task_id: &str,
        promote_at: i64,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT slug FROM skill_uses WHERE task_id = ?1 AND counted = 0")?;
        let slugs: Vec<String> = stmt
            .query_map([task_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for slug in &slugs {
            self.conn.execute(
                "UPDATE skill_uses SET counted = 1 WHERE task_id = ?1 AND slug = ?2",
                params![task_id, slug],
            )?;
            self.conn.execute(
                "UPDATE skills SET successes = successes + 1, updated_at = ?2 WHERE slug = ?1",
                params![slug, ts(now)],
            )?;
            self.conn.execute(
                "UPDATE skills SET status = 'active' WHERE slug = ?1 AND status = 'trial' AND successes >= ?2",
                params![slug, promote_at],
            )?;
        }
        Ok(slugs)
    }

    /// 오래 쓰지 않은 스킬: 시험·활성은 오래됨으로, 오래됨은 보관으로. 고정된 것은 빼고. (오래됨, 보관) 수.
    pub fn age_skills(
        &self,
        now: DateTime<Utc>,
        stale_after: Duration,
        archive_after: Duration,
    ) -> Result<(usize, usize)> {
        let last = "COALESCE(last_used_at, created_at)";
        let archived = self.conn.execute(
            &format!(
                "UPDATE skills SET status = 'archived', updated_at = ?2 WHERE pinned = 0 AND status = 'stale' AND {last} < ?1"
            ),
            params![ts(now - archive_after), ts(now)],
        )?;
        let staled = self.conn.execute(
            &format!(
                "UPDATE skills SET status = 'stale', updated_at = ?2 WHERE pinned = 0 AND status IN ('trial', 'active') AND {last} < ?1"
            ),
            params![ts(now - stale_after), ts(now)],
        )?;
        Ok((staled, archived))
    }

    /// 회고하지 않은, 사람이 완료로 확인한 작업.
    pub fn tasks_needing_retro(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM tasks WHERE status = 'done' AND retro_at IS NULL ORDER BY closed_at",
        )?;
        Ok(stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn mark_retro(&self, task_id: &str, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET retro_at = ?2 WHERE id = ?1",
            params![task_id, ts(now)],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    fn write<'a>(slug: &'a str, body: &'a str) -> SkillWrite<'a> {
        SkillWrite {
            slug,
            title: "쇼핑몰 최저가 확인",
            description: "제품 최저가를 쇼핑몰에서 확인할 때",
            body,
            script: None,
        }
    }

    fn origin(status: SkillStatus) -> (SkillStatus, serde_json::Value) {
        (
            status,
            serde_json::json!({"tools": ["WebSearch"], "hosts": []}),
        )
    }

    #[test]
    fn revisions_lifecycle_and_success_counting() {
        let mut store = Store::open_in_memory().unwrap();
        let (status, fp) = origin(SkillStatus::Trial);
        let o = SkillOrigin {
            status,
            source: "retro",
            task_id: Some("task-1"),
            footprint: &fp,
        };
        assert_eq!(
            store
                .save_skill(
                    &write("price-check", "1. 검색"),
                    None,
                    &o,
                    "agent",
                    "처음",
                    at(9, 0)
                )
                .unwrap(),
            Ok(1)
        );
        assert!(matches!(
            store
                .save_skill(
                    &write("price-check", "x"),
                    None,
                    &o,
                    "agent",
                    "다시",
                    at(9, 1)
                )
                .unwrap(),
            Err(SkillError::Stale { current: Some(1) })
        ));
        assert_eq!(
            store
                .save_skill(
                    &write("price-check", "1. 검색\n2. 표"),
                    Some(1),
                    &o,
                    "agent",
                    "고침",
                    at(9, 2)
                )
                .unwrap(),
            Ok(2)
        );
        assert_eq!(
            store.skill_body("price-check").unwrap().unwrap(),
            "1. 검색\n2. 표"
        );
        assert_eq!(store.skill_revisions("price-check").unwrap().len(), 2);

        // 고정하면 Lucia는 고치지 못하고 사람만 고친다.
        store
            .set_skill_pinned("price-check", true, at(9, 3))
            .unwrap();
        assert_eq!(
            store
                .save_skill(
                    &write("price-check", "y"),
                    Some(2),
                    &o,
                    "agent",
                    "x",
                    at(9, 4)
                )
                .unwrap(),
            Err(SkillError::Pinned)
        );
        store
            .set_skill_pinned("price-check", false, at(9, 3))
            .unwrap();

        // 완료로 확인된 작업이 읽은 것만 성공으로 센다. 두 번이면 활성.
        store
            .record_skill_use("price-check", "task-2-r1", Some("task-2"), at(10, 0))
            .unwrap();
        store
            .record_skill_use("price-check", "task-2-r2", Some("task-2"), at(10, 1))
            .unwrap();
        assert_eq!(
            store.count_skill_successes("task-2", 2, at(11, 0)).unwrap(),
            ["price-check"]
        );
        assert!(
            store
                .count_skill_successes("task-2", 2, at(11, 0))
                .unwrap()
                .is_empty(),
            "한 번만"
        );
        assert_eq!(
            store.skill("price-check").unwrap().unwrap().status,
            SkillStatus::Trial
        );
        store
            .record_skill_use("price-check", "task-3-r1", Some("task-3"), at(12, 0))
            .unwrap();
        store
            .count_skill_successes("task-3", 2, at(12, 30))
            .unwrap();
        let s = store.skill("price-check").unwrap().unwrap();
        assert_eq!((s.status, s.uses, s.successes), (SkillStatus::Active, 3, 2));

        // 오래 안 쓰면 오래됨 → 보관.
        let later = at(12, 0) + Duration::days(61);
        assert_eq!(
            store
                .age_skills(later, Duration::days(60), Duration::days(60))
                .unwrap(),
            (1, 0)
        );
        let much_later = at(12, 0) + Duration::days(125);
        assert_eq!(
            store
                .age_skills(much_later, Duration::days(60), Duration::days(60))
                .unwrap(),
            (0, 1)
        );
        assert_eq!(
            store.skill("price-check").unwrap().unwrap().status,
            SkillStatus::Archived
        );
    }

    #[test]
    fn fields_are_checked() {
        let mut store = Store::open_in_memory().unwrap();
        let fp = serde_json::json!({});
        let o = SkillOrigin {
            status: SkillStatus::Trial,
            source: "chat",
            task_id: None,
            footprint: &fp,
        };
        for bad in ["Bad Name", "../x", ""] {
            assert!(
                store
                    .save_skill(&write(bad, "b"), None, &o, "agent", "r", at(9, 0))
                    .unwrap()
                    .is_err(),
                "{bad}"
            );
        }
        let mut w = write("ok", "b");
        w.description = "두 줄\n설명";
        assert!(
            store
                .save_skill(&w, None, &o, "agent", "r", at(9, 0))
                .unwrap()
                .is_err()
        );
    }
}
