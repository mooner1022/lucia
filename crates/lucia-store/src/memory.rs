//! 기억 항목(결정 0007): 나·주제·영역으로 나눈 항목. 평소 맥락에는 색인(이름·제목·요약)만 붙고,
//! 세부 내용은 MCP 도구로 그때그때 읽는다(Claude 서비스의 메모리와 같은 방식).
//!
//! - 모든 변경(쓰기·지우기)을 개정으로 남긴다. 지우기는 지우기 전 내용을 남긴다.
//! - 이미 있는 항목을 고칠 때는 읽은 개정 번호를 함께 받는다. 본문이 맥락에 없으므로, 읽지 않은 내용을
//!   통째로 덮어쓰지 못하게 한다.
//! - 데몬이 형식과 크기를 강제한다. 제목과 요약은 매 실행 맥락(색인)에 들어가므로 짧은 한 줄이다.
//! - 예전 한 덩어리 핵심 기억(`core_memory_revisions`)은 기록으로만 남긴다(마이그레이션 0006이 항목 하나로 옮김).

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

use crate::{Result, Store, StoreError, parse_ts, ts};

/// 이름(slug) 최대 길이(문자).
pub const SLUG_MAX: usize = 40;
/// 제목 최대 길이(문자).
pub const TITLE_MAX: usize = 40;
/// 요약 최대 길이(문자). 색인에 그대로 들어간다.
pub const SUMMARY_MAX: usize = 120;
/// 세부 정보 최대 크기(바이트).
pub const BODY_MAX_BYTES: usize = 8 * 1024;
/// 항목 수 상한.
pub const MAX_ENTRIES: usize = 200;
/// 변경 이유 최대 길이(문자).
pub const REASON_MAX: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum MemoryAuthor {
    Agent,
    User,
    Daemon,
}

impl MemoryAuthor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::User => "user",
            Self::Daemon => "daemon",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "agent" => Self::Agent,
            "user" => Self::User,
            "daemon" => Self::Daemon,
            other => return Err(StoreError::Corrupt(format!("memory author {other:?}"))),
        })
    }
}

/// 기억의 큰 갈래.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub enum MemorySection {
    /// 나: 사용자가 누구인지와 Lucia에게 바라는 방식(프로필, 선호, 규칙).
    Me,
    /// 주제: 생활의 주제별 사실(기기, 건강, 취미, 장소, 사람 등).
    Topic,
    /// 영역: 진행 중인 일·프로젝트·활동 영역.
    Area,
}

impl MemorySection {
    pub const ALL: [MemorySection; 3] = [Self::Me, Self::Topic, Self::Area];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Me => "me",
            Self::Topic => "topic",
            Self::Area => "area",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Me => "나",
            Self::Topic => "주제",
            Self::Area => "영역",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|x| x.as_str() == s)
    }

    /// 모델이 낸 값: 영문 값(me·topic·area)이나 한국어 이름(나·주제·영역).
    pub fn parse_loose(s: &str) -> Option<Self> {
        let s = s.trim();
        Self::ALL
            .into_iter()
            .find(|x| x.as_str().eq_ignore_ascii_case(s) || x.label() == s)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MemoryEntry {
    pub slug: String,
    pub section: MemorySection,
    pub title: String,
    pub summary: String,
    pub body: String,
    /// 마지막 개정 ID. 고칠 때 이 값을 함께 낸다.
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum MemoryAction {
    Write,
    Delete,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MemoryRevision {
    pub id: i64,
    pub slug: String,
    pub at: DateTime<Utc>,
    pub author: MemoryAuthor,
    pub run_id: Option<String>,
    pub reason: String,
    pub action: MemoryAction,
    pub section: MemorySection,
    pub title: String,
    pub summary: String,
    pub body: String,
}

/// 쓸 항목(만들기 또는 고치기).
#[derive(Debug, Clone, Copy)]
pub struct MemoryWrite<'a> {
    pub slug: &'a str,
    pub section: MemorySection,
    pub title: &'a str,
    pub summary: &'a str,
    pub body: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Created {
        revision: i64,
    },
    Updated {
        revision: i64,
    },
    /// 내용이 같아 개정을 만들지 않았다.
    Unchanged {
        revision: i64,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MemoryError {
    #[error(
        "이름이 잘못됐다: {0:?}(영문 소문자·숫자·하이픈, 첫 글자는 영문 소문자나 숫자, {SLUG_MAX}자까지)"
    )]
    InvalidSlug(String),
    #[error("{field}: {why}")]
    InvalidField { field: &'static str, why: String },
    #[error("세부 정보가 상한을 넘는다: {size}바이트 > {max}바이트(항목을 나눈다)")]
    TooLarge { size: usize, max: usize },
    #[error("변경 이유가 비어 있다")]
    EmptyReason,
    #[error("기억 항목이 {max}개를 넘는다(비슷한 항목을 합치거나 낡은 것을 지운다)")]
    TooMany { max: usize },
    #[error(
        "이미 있는 항목이다(지금 개정 {current}). memory_read로 지금 내용을 읽고, 고친 완성본을 그 개정 번호와 함께 쓴다"
    )]
    Stale { current: i64 },
    #[error("없는 항목이다: {0}(지워졌을 수 있다. 새로 만들려면 개정 번호 없이 쓴다)")]
    NotFound(String),
}

pub(crate) fn valid_slug(slug: &str) -> bool {
    let mut chars = slug.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && slug.chars().count() <= SLUG_MAX
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// 한 줄짜리 짧은 글(제목, 요약).
fn line(field: &'static str, text: &str, max: usize) -> std::result::Result<(), MemoryError> {
    let why = |w: &str| MemoryError::InvalidField {
        field,
        why: w.to_owned(),
    };
    if text.trim().is_empty() {
        return Err(why("비어 있다"));
    }
    if text.chars().any(char::is_control) {
        return Err(why("줄바꿈이나 제어 문자를 넣을 수 없다(한 줄)"));
    }
    if text.chars().count() > max {
        return Err(why(&format!("{max}자까지")));
    }
    Ok(())
}

fn validate(w: &MemoryWrite<'_>) -> std::result::Result<(), MemoryError> {
    if !valid_slug(w.slug) {
        return Err(MemoryError::InvalidSlug(w.slug.to_owned()));
    }
    line("제목", w.title, TITLE_MAX)?;
    line("요약", w.summary, SUMMARY_MAX)?;
    if w.body.len() > BODY_MAX_BYTES {
        return Err(MemoryError::TooLarge {
            size: w.body.len(),
            max: BODY_MAX_BYTES,
        });
    }
    Ok(())
}

fn check_reason(reason: &str) -> std::result::Result<(), MemoryError> {
    if reason.trim().is_empty() {
        return Err(MemoryError::EmptyReason);
    }
    line("이유", reason, REASON_MAX)
}

const ENTRY_COLUMNS: &str = "slug, section, title, summary, body, revision, created_at, updated_at";
const SECTION_ORDER: &str = "CASE section WHEN 'me' THEN 0 WHEN 'topic' THEN 1 ELSE 2 END";

type EntryRow = (String, String, String, String, String, i64, String, String);

fn entry_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<EntryRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
    ))
}

fn to_entry(row: EntryRow) -> Result<MemoryEntry> {
    let (slug, section, title, summary, body, revision, created_at, updated_at) = row;
    Ok(MemoryEntry {
        section: MemorySection::parse(&section)
            .ok_or_else(|| StoreError::Corrupt(format!("memory section {section:?}")))?,
        slug,
        title,
        summary,
        body,
        revision,
        created_at: parse_ts(&created_at)?,
        updated_at: parse_ts(&updated_at)?,
    })
}

impl Store {
    /// 모든 항목: 나 → 주제 → 영역, 그 안에서 제목 순.
    pub fn memories(&self) -> Result<Vec<MemoryEntry>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ENTRY_COLUMNS} FROM memories ORDER BY {SECTION_ORDER}, title, slug"
        ))?;
        let rows = stmt.query_map([], entry_row)?;
        rows.map(|r| to_entry(r?)).collect()
    }

    pub fn memory(&self, slug: &str) -> Result<Option<MemoryEntry>> {
        self.conn
            .query_row(
                &format!("SELECT {ENTRY_COLUMNS} FROM memories WHERE slug = ?1"),
                [slug],
                entry_row,
            )
            .optional()?
            .map(to_entry)
            .transpose()
    }

    /// 항목을 만들거나 고친다. 이미 있는 항목은 `expected`(읽은 개정 번호)가 지금 것과 같아야 한다.
    #[allow(clippy::too_many_arguments)]
    pub fn write_memory(
        &self,
        w: &MemoryWrite<'_>,
        expected: Option<i64>,
        author: MemoryAuthor,
        run_id: Option<&str>,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<WriteOutcome> {
        validate(w)?;
        check_reason(reason)?;
        let title = w.title.trim();
        let summary = w.summary.trim();
        let body = w.body.trim_end();
        let tx = self.conn.unchecked_transaction()?;
        let existing = self.memory(w.slug)?;
        match (&existing, expected) {
            (Some(e), Some(rev)) if rev == e.revision => {}
            (Some(e), _) => {
                return Err(MemoryError::Stale {
                    current: e.revision,
                }
                .into());
            }
            (None, Some(_)) => return Err(MemoryError::NotFound(w.slug.to_owned()).into()),
            (None, None) => {
                let count: i64 = tx.query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))?;
                if count as usize >= MAX_ENTRIES {
                    return Err(MemoryError::TooMany { max: MAX_ENTRIES }.into());
                }
            }
        }
        if let Some(e) = &existing
            && e.section == w.section
            && e.title == title
            && e.summary == summary
            && e.body == body
        {
            return Ok(WriteOutcome::Unchanged {
                revision: e.revision,
            });
        }
        tx.execute(
            "INSERT INTO memory_revisions (slug, at, author, run_id, reason, action, section, title, summary, body)
             VALUES (?1, ?2, ?3, ?4, ?5, 'write', ?6, ?7, ?8, ?9)",
            params![
                w.slug,
                ts(at),
                author.as_str(),
                run_id,
                reason.trim(),
                w.section.as_str(),
                title,
                summary,
                body
            ],
        )?;
        let revision = tx.last_insert_rowid();
        if existing.is_some() {
            tx.execute(
                "UPDATE memories SET section = ?2, title = ?3, summary = ?4, body = ?5, revision = ?6, updated_at = ?7
                 WHERE slug = ?1",
                params![w.slug, w.section.as_str(), title, summary, body, revision, ts(at)],
            )?;
        } else {
            tx.execute(
                "INSERT INTO memories (slug, section, title, summary, body, revision, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![w.slug, w.section.as_str(), title, summary, body, revision, ts(at)],
            )?;
        }
        tx.commit()?;
        Ok(if existing.is_some() {
            WriteOutcome::Updated { revision }
        } else {
            WriteOutcome::Created { revision }
        })
    }

    /// 항목을 지운다(지우기 전 내용을 개정으로 남긴다). 없으면 `false`.
    pub fn delete_memory(
        &self,
        slug: &str,
        author: MemoryAuthor,
        run_id: Option<&str>,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<bool> {
        check_reason(reason)?;
        let tx = self.conn.unchecked_transaction()?;
        let Some(e) = self.memory(slug)? else {
            return Ok(false);
        };
        tx.execute(
            "INSERT INTO memory_revisions (slug, at, author, run_id, reason, action, section, title, summary, body)
             VALUES (?1, ?2, ?3, ?4, ?5, 'delete', ?6, ?7, ?8, ?9)",
            params![
                e.slug,
                ts(at),
                author.as_str(),
                run_id,
                reason.trim(),
                e.section.as_str(),
                e.title,
                e.summary,
                e.body
            ],
        )?;
        tx.execute("DELETE FROM memories WHERE slug = ?1", [slug])?;
        tx.commit()?;
        Ok(true)
    }

    /// 한 항목의 변경 기록(최근 것부터).
    pub fn memory_history(&self, slug: &str, limit: usize) -> Result<Vec<MemoryRevision>> {
        self.revisions_where("slug = ?1", params![slug, limit as i64])
    }

    /// 개정 하나.
    pub fn memory_revision(&self, id: i64) -> Result<Option<MemoryRevision>> {
        Ok(self
            .revisions_where("id = ?1", params![id, 1i64])?
            .into_iter()
            .next())
    }

    fn revisions_where(
        &self,
        filter: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<MemoryRevision>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, slug, at, author, run_id, reason, action, section, title, summary, body
             FROM memory_revisions WHERE {filter} ORDER BY id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(args, |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, String>(10)?,
            ))
        })?;
        rows.map(|row| {
            let (id, slug, at, author, run_id, reason, action, section, title, summary, body) =
                row?;
            Ok(MemoryRevision {
                id,
                slug,
                at: parse_ts(&at)?,
                author: MemoryAuthor::parse(&author)?,
                run_id,
                reason,
                action: match action.as_str() {
                    "write" => MemoryAction::Write,
                    "delete" => MemoryAction::Delete,
                    other => {
                        return Err(StoreError::Corrupt(format!("memory action {other:?}")));
                    }
                },
                section: MemorySection::parse(&section)
                    .ok_or_else(|| StoreError::Corrupt(format!("memory section {section:?}")))?,
                title,
                summary,
                body,
            })
        })
        .collect()
    }

    /// 이름·제목·요약·세부 정보에 낱말이 들어 있는 항목(최근에 고친 것부터).
    pub fn search_memories(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ENTRY_COLUMNS} FROM memories
             WHERE slug LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\'
                OR summary LIKE ?1 ESCAPE '\\' OR body LIKE ?1 ESCAPE '\\'
             ORDER BY updated_at DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![pattern, limit as i64], entry_row)?;
        rows.map(|r| to_entry(r?)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    fn w<'a>(
        slug: &'a str,
        section: MemorySection,
        summary: &'a str,
        body: &'a str,
    ) -> MemoryWrite<'a> {
        MemoryWrite {
            slug,
            section,
            title: "기기",
            summary,
            body,
        }
    }

    #[test]
    fn create_read_update_needs_the_current_revision() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.memories().unwrap().is_empty());
        let created = store
            .write_memory(
                &w(
                    "devices",
                    MemorySection::Topic,
                    "태블릿과 휴대폰",
                    "- 서피스 프로 8",
                ),
                None,
                MemoryAuthor::Agent,
                Some("run-1"),
                "대화에서 알게 됨",
                at(9, 0),
            )
            .unwrap();
        let WriteOutcome::Created { revision: r1 } = created else {
            panic!("{created:?}")
        };
        let e = store.memory("devices").unwrap().unwrap();
        assert_eq!(
            (e.revision, e.section, e.title.as_str()),
            (r1, MemorySection::Topic, "기기")
        );

        // 읽은 개정 없이, 또는 낡은 개정으로는 덮어쓰지 못한다.
        let body = "- 서피스 프로 8\n- 갤럭시 Z 폴드 7";
        let new = w("devices", MemorySection::Topic, "태블릿과 휴대폰", body);
        for expected in [None, Some(r1 + 100)] {
            let err = store
                .write_memory(&new, expected, MemoryAuthor::Agent, None, "추가", at(9, 5))
                .unwrap_err();
            assert!(
                matches!(err, StoreError::Memory(MemoryError::Stale { current }) if current == r1),
                "{err}"
            );
        }
        let updated = store
            .write_memory(
                &new,
                Some(r1),
                MemoryAuthor::Agent,
                None,
                "휴대폰 추가",
                at(10, 0),
            )
            .unwrap();
        let WriteOutcome::Updated { revision: r2 } = updated else {
            panic!("{updated:?}")
        };
        let e = store.memory("devices").unwrap().unwrap();
        assert_eq!((e.revision, e.body.as_str()), (r2, body));
        assert_eq!(e.created_at, at(9, 0));
        assert_eq!(e.updated_at, at(10, 0));

        // 같은 내용이면 개정을 만들지 않는다.
        assert_eq!(
            store
                .write_memory(
                    &new,
                    Some(r2),
                    MemoryAuthor::Agent,
                    None,
                    "그대로",
                    at(11, 0)
                )
                .unwrap(),
            WriteOutcome::Unchanged { revision: r2 }
        );
        assert_eq!(store.memory_history("devices", 10).unwrap().len(), 2);

        // 없는 항목을 개정 번호와 함께 쓰면(지워졌을 수 있다) 거부.
        let err = store
            .write_memory(
                &w("gone", MemorySection::Topic, "요약", ""),
                Some(1),
                MemoryAuthor::Agent,
                None,
                "x",
                at(11, 0),
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::Memory(MemoryError::NotFound(_))));
    }

    #[test]
    fn delete_keeps_the_last_content_in_history() {
        let store = Store::open_in_memory().unwrap();
        store
            .write_memory(
                &w("health", MemorySection::Topic, "건강 메모", "- 비타민 D"),
                None,
                MemoryAuthor::User,
                None,
                "처음",
                at(9, 0),
            )
            .unwrap();
        assert!(
            store
                .delete_memory(
                    "health",
                    MemoryAuthor::Agent,
                    Some("run-9"),
                    "사용자가 지워 달라고 함",
                    at(9, 30)
                )
                .unwrap()
        );
        assert!(store.memory("health").unwrap().is_none());
        assert!(
            !store
                .delete_memory("health", MemoryAuthor::Agent, None, "다시", at(9, 31))
                .unwrap()
        );
        let history = store.memory_history("health", 10).unwrap();
        assert_eq!(history[0].action, MemoryAction::Delete);
        assert_eq!(history[0].body, "- 비타민 D", "지우기 전 내용을 남긴다");
        assert_eq!(history[0].run_id.as_deref(), Some("run-9"));
        assert_eq!(history[1].action, MemoryAction::Write);
    }

    #[test]
    fn fields_are_validated() {
        let store = Store::open_in_memory().unwrap();
        let attempt = |slug: &str, title: &str, summary: &str, body: &str, reason: &str| {
            store.write_memory(
                &MemoryWrite {
                    slug,
                    section: MemorySection::Me,
                    title,
                    summary,
                    body,
                },
                None,
                MemoryAuthor::Agent,
                None,
                reason,
                at(9, 0),
            )
        };
        for slug in [
            "",
            "Profile",
            "-x",
            "pc build",
            "pc_build",
            "프로필",
            &"a".repeat(41),
        ] {
            assert!(
                matches!(
                    attempt(slug, "제목", "요약", "", "이유"),
                    Err(StoreError::Memory(MemoryError::InvalidSlug(_)))
                ),
                "{slug:?}"
            );
        }
        assert!(attempt("ok", "", "요약", "", "이유").is_err(), "빈 제목");
        assert!(
            attempt("ok", "제목\n둘째 줄", "요약", "", "이유").is_err(),
            "제목은 한 줄"
        );
        assert!(
            attempt("ok", "제목", &"가".repeat(121), "", "이유").is_err(),
            "요약 길이"
        );
        assert!(
            attempt("ok", "제목", "요약", &"가".repeat(3000), "이유").is_err(),
            "본문 크기"
        );
        assert!(matches!(
            attempt("ok", "제목", "요약", "", " "),
            Err(StoreError::Memory(MemoryError::EmptyReason))
        ));
        assert!(attempt("pc-build-2", "제목", "요약", "", "이유").is_ok());
        assert_eq!(store.memories().unwrap().len(), 1);
    }

    #[test]
    fn count_is_capped_and_order_is_by_section() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..MAX_ENTRIES {
            let section = MemorySection::ALL[i % 3];
            store
                .write_memory(
                    &MemoryWrite {
                        slug: &format!("e{i}"),
                        section,
                        title: &format!("항목 {i:03}"),
                        summary: "요약",
                        body: "",
                    },
                    None,
                    MemoryAuthor::Agent,
                    None,
                    "채우기",
                    at(9, 0),
                )
                .unwrap();
        }
        let err = store
            .write_memory(
                &w("one-more", MemorySection::Area, "요약", ""),
                None,
                MemoryAuthor::Agent,
                None,
                "하나 더",
                at(9, 1),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            StoreError::Memory(MemoryError::TooMany { .. })
        ));
        let all = store.memories().unwrap();
        let sections: Vec<_> = all.iter().map(|e| e.section).collect();
        let mut sorted = sections.clone();
        sorted.sort();
        assert_eq!(sections, sorted, "나 → 주제 → 영역");
    }

    #[test]
    fn search_matches_any_field_and_escapes_wildcards() {
        let store = Store::open_in_memory().unwrap();
        for (slug, summary, body) in [
            ("pc-build", "데스크톱 사양", "- RTX 5080\n- 100% 무소음"),
            ("devices", "휴대 기기", "- 폴드 7"),
        ] {
            store
                .write_memory(
                    &w(slug, MemorySection::Topic, summary, body),
                    None,
                    MemoryAuthor::Agent,
                    None,
                    "x",
                    at(9, 0),
                )
                .unwrap();
        }
        let slugs = |q: &str| -> Vec<String> {
            store
                .search_memories(q, 10)
                .unwrap()
                .into_iter()
                .map(|e| e.slug)
                .collect()
        };
        assert_eq!(slugs("RTX"), ["pc-build"]);
        assert_eq!(slugs("rtx"), ["pc-build"], "영문 대소문자 무시");
        assert_eq!(slugs("폴드"), ["devices"]);
        assert_eq!(slugs("100%"), ["pc-build"]);
        assert!(slugs("5_80").is_empty(), "밑줄은 와일드카드가 아니라 글자");
        assert!(slugs("  ").is_empty());
    }
}
