//! 에피소드 기억: 대화, 보고, 브리핑. trigram 전문 검색.

use chrono::{DateTime, Utc};
use rusqlite::{ToSql, params};

use crate::{Result, Store, StoreError, parse_ts, ts};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeKind {
    UserMessage,
    AgentReport,
    Briefing,
    RotationSummary,
}

impl EpisodeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::UserMessage => "user_message",
            Self::AgentReport => "agent_report",
            Self::Briefing => "briefing",
            Self::RotationSummary => "rotation_summary",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "user_message" => Self::UserMessage,
            "agent_report" => Self::AgentReport,
            "briefing" => Self::Briefing,
            "rotation_summary" => Self::RotationSummary,
            other => return Err(StoreError::Corrupt(format!("episode kind {other:?}"))),
        })
    }
}

/// 누가 만든 에피소드인가. 브리핑 판단에서 "사용자가 시작한 것"만 변화로 센다(PLAN §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    User,
    Agent,
    Daemon,
}

impl Origin {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
            Self::Daemon => "daemon",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "user" => Self::User,
            "agent" => Self::Agent,
            "daemon" => Self::Daemon,
            other => return Err(StoreError::Corrupt(format!("origin {other:?}"))),
        })
    }
}

#[derive(Debug, Clone)]
pub struct NewEpisode {
    pub at: DateTime<Utc>,
    pub kind: EpisodeKind,
    pub origin: Origin,
    pub conversation: Option<String>,
    pub session_id: Option<String>,
    pub run_id: Option<String>,
    pub body: String,
}

impl NewEpisode {
    /// 사용자가 보낸 메시지.
    pub fn user(conversation: &str, body: &str, at: DateTime<Utc>) -> Self {
        Self {
            at,
            kind: EpisodeKind::UserMessage,
            origin: Origin::User,
            conversation: Some(conversation.to_owned()),
            session_id: None,
            run_id: None,
            body: body.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub kind: EpisodeKind,
    pub origin: Origin,
    pub conversation: Option<String>,
    pub session_id: Option<String>,
    pub run_id: Option<String>,
    pub body: String,
}

const COLUMNS: &str = "id, at, kind, origin, conversation, session_id, run_id, body";

type RawEpisode = (
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

fn row_to_episode(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawEpisode> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn finish(raw: RawEpisode) -> Result<Episode> {
    let (id, at, kind, origin, conversation, session_id, run_id, body) = raw;
    Ok(Episode {
        id,
        at: parse_ts(&at)?,
        kind: EpisodeKind::parse(&kind)?,
        origin: Origin::parse(&origin)?,
        conversation,
        session_id,
        run_id,
        body,
    })
}

/// 에피소드에 붙은 파일 하나(결정 0066). 파일이 지워져도 이름이 남게 그때의 정보를 둔다.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EpisodeFile {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub kind: String,
}

impl From<&crate::FileRecord> for EpisodeFile {
    fn from(f: &crate::FileRecord) -> Self {
        Self {
            id: f.id.clone(),
            name: f.name.clone(),
            size: f.size,
            mime: f.mime.clone(),
            kind: f.kind.clone(),
        }
    }
}

/// 말 하나의 모양(결정 0066): 여러 프레임으로 나뉘어 갔으면 그 조각들, 붙은 파일.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EpisodeShape {
    /// 둘 이상으로 나뉘어 갔을 때만(보낸 차례). 하나로 갔으면 `None`.
    pub parts: Option<Vec<String>>,
    pub files: Vec<EpisodeFile>,
}

impl Store {
    /// 에피소드의 모양을 남긴다(결정 0066). 조각이 하나뿐이고 파일도 없으면 남기지 않는다. 붙은 파일은 그 대화가
    /// 받을 수 있게 한다([`Store::grant_file`]).
    pub fn set_episode_shape(
        &self,
        episode_id: i64,
        conversation: Option<&str>,
        parts: &[String],
        files: &[EpisodeFile],
        now: DateTime<Utc>,
    ) -> Result<()> {
        let parts = (parts.len() > 1).then_some(parts);
        if parts.is_none() && files.is_empty() {
            return Ok(());
        }
        let parts_json = parts
            .map(|p| serde_json::to_string(p).map_err(|e| StoreError::Corrupt(e.to_string())))
            .transpose()?;
        let files_json =
            serde_json::to_string(files).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        self.conn.execute(
            "INSERT INTO episode_shapes (episode_id, parts, files) VALUES (?1, ?2, ?3)
             ON CONFLICT (episode_id) DO UPDATE SET parts = excluded.parts, files = excluded.files",
            params![episode_id, parts_json, files_json],
        )?;
        if let Some(conversation) = conversation {
            for f in files {
                self.grant_file(&f.id, conversation, now)?;
            }
        }
        Ok(())
    }

    /// 여러 에피소드의 모양(남긴 것만).
    pub fn episode_shapes(
        &self,
        ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, EpisodeShape>> {
        let mut out = std::collections::HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let marks = (1..=ids.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT episode_id, parts, files FROM episode_shapes WHERE episode_id IN ({marks})"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for r in rows {
            let (id, parts, files) = r?;
            let parts = parts
                .map(|p| serde_json::from_str::<Vec<String>>(&p))
                .transpose()
                .map_err(|e| StoreError::Corrupt(format!("episode shape parts: {e}")))?;
            let files: Vec<EpisodeFile> = serde_json::from_str(&files)
                .map_err(|e| StoreError::Corrupt(format!("episode shape files: {e}")))?;
            out.insert(id, EpisodeShape { parts, files });
        }
        Ok(out)
    }

    pub fn add_episode(&self, e: &NewEpisode) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO episodes (at, kind, origin, conversation, session_id, run_id, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                ts(e.at),
                e.kind.as_str(),
                e.origin.as_str(),
                e.conversation,
                e.session_id,
                e.run_id,
                e.body
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 낱말마다: 세 글자 이상은 trigram MATCH, 두 글자 이하는 LIKE. 모두 만족(AND)하는 것을
    /// 최근 순으로 돌려준다. 빈 질의는 빈 결과.
    pub fn search_episodes(&self, query: &str, limit: usize) -> Result<Vec<Episode>> {
        let terms: Vec<&str> = query.split_whitespace().take(8).collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut clauses = Vec::new();
        let mut args: Vec<String> = Vec::new();
        for term in &terms {
            args.push(if term.chars().count() >= 3 {
                clauses.push(format!(
                    "e.id IN (SELECT rowid FROM episodes_fts WHERE episodes_fts MATCH ?{})",
                    args.len() + 1
                ));
                // 따옴표로 감싸 FTS 연산자로 해석되지 않게 한다.
                format!("\"{}\"", term.replace('"', "\"\""))
            } else {
                clauses.push(format!(
                    "e.body LIKE '%' || ?{} || '%' ESCAPE '\\'",
                    args.len() + 1
                ));
                term.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            });
        }
        let sql = format!(
            "SELECT {} FROM episodes e WHERE {} ORDER BY e.at DESC, e.id DESC LIMIT {}",
            COLUMNS
                .split(", ")
                .map(|c| format!("e.{c}"))
                .collect::<Vec<_>>()
                .join(", "),
            clauses.join(" AND "),
            limit
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn ToSql> = args.iter().map(|a| a as &dyn ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), row_to_episode)?;
        rows.map(|r| finish(r?)).collect()
    }

    /// `since` 이후(포함하지 않음)의 에피소드. `origin`이 있으면 그 출처만.
    pub fn episodes_since(
        &self,
        since: DateTime<Utc>,
        origin: Option<Origin>,
        limit: usize,
    ) -> Result<Vec<Episode>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM episodes
             WHERE at > ?1 AND (?2 IS NULL OR origin = ?2)
             ORDER BY at ASC, id ASC LIMIT ?3"
        ))?;
        let rows = stmt.query_map(
            params![ts(since), origin.map(Origin::as_str), limit as i64],
            row_to_episode,
        )?;
        rows.map(|r| finish(r?)).collect()
    }

    /// 대화 하나의 최근 에피소드 `limit`개(오래된 것부터). 세션 교대 직후 직전 대화를 잇는 데 쓴다.
    pub fn recent_in_conversation(&self, conversation: &str, limit: usize) -> Result<Vec<Episode>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM episodes WHERE conversation = ?1
             ORDER BY at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![conversation, limit as i64], row_to_episode)?;
        let mut list: Vec<Episode> = rows.map(|r| finish(r?)).collect::<Result<_>>()?;
        list.reverse();
        Ok(list)
    }

    /// 여러 대화의 에피소드를 최근 것부터(`before_id`보다 앞만), 돌려줄 때는 오래된 것부터. 앱의 대화 기록(결정 0043).
    pub fn episodes_in_before(
        &self,
        conversations: &[String],
        before_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<Episode>> {
        if conversations.is_empty() {
            return Ok(Vec::new());
        }
        let marks = (0..conversations.len())
            .map(|i| format!("?{}", i + 3))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM episodes
             WHERE (?1 IS NULL OR id < ?1) AND conversation IN ({marks})
             ORDER BY id DESC LIMIT ?2"
        ))?;
        let mut args: Vec<rusqlite::types::Value> = vec![
            before_id.map_or(
                rusqlite::types::Value::Null,
                rusqlite::types::Value::Integer,
            ),
            rusqlite::types::Value::Integer(limit as i64),
        ];
        args.extend(
            conversations
                .iter()
                .cloned()
                .map(rusqlite::types::Value::Text),
        );
        let rows = stmt.query_map(rusqlite::params_from_iter(args), row_to_episode)?;
        let mut list: Vec<Episode> = rows.map(|r| finish(r?)).collect::<Result<_>>()?;
        list.reverse();
        Ok(list)
    }

    /// `before`보다 오래된 원본 에피소드를 지운다(하루 정리, 결정 0016). 세션 교대 요약(`RotationSummary`)은
    /// 남긴다(긴 맥락을 이어 주는 요약이다). 지운 수를 돌려준다. 되돌릴 수 없으므로 사용자가 보관 기간을 정했을 때만 부른다.
    pub fn delete_episodes_before(&self, before: DateTime<Utc>) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM episodes WHERE at < ?1 AND kind != ?2",
            params![ts(before), EpisodeKind::RotationSummary.as_str()],
        )?;
        // 모양(결정 0066)도 함께 지운다.
        self.conn.execute(
            "DELETE FROM episode_shapes WHERE episode_id NOT IN (SELECT id FROM episodes)",
            [],
        )?;
        Ok(n)
    }

    /// 가장 최근 `kind` 에피소드의 시각.
    pub fn last_episode_at(&self, kind: EpisodeKind) -> Result<Option<DateTime<Utc>>> {
        let at: Option<String> = self.conn.query_row(
            "SELECT MAX(at) FROM episodes WHERE kind = ?1",
            [kind.as_str()],
            |r| r.get(0),
        )?;
        crate::parse_opt_ts(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    #[test]
    fn old_episodes_go_but_rotation_summaries_stay() {
        let store = Store::open_in_memory().unwrap();
        store
            .add_episode(&NewEpisode::user("c", "오래된 말 사과", at(8, 0)))
            .unwrap();
        store
            .add_episode(&NewEpisode {
                at: at(8, 30),
                kind: EpisodeKind::RotationSummary,
                origin: Origin::Daemon,
                conversation: None,
                session_id: None,
                run_id: None,
                body: "요약".into(),
            })
            .unwrap();
        store
            .add_episode(&NewEpisode::user("c", "새 말 사과", at(10, 0)))
            .unwrap();
        assert_eq!(store.delete_episodes_before(at(9, 0)).unwrap(), 1);
        let left = store.search_episodes("사과", 5).unwrap();
        assert_eq!(left.len(), 1, "검색 색인에서도 빠진다");
        assert_eq!(left[0].body, "새 말 사과");
        assert_eq!(store.episodes_since(at(0, 0), None, 10).unwrap().len(), 2);
    }

    #[test]
    fn history_pages_backwards_over_conversations() {
        let store = Store::open_in_memory().unwrap();
        for (conv, body, hour) in [
            ("app:c1", "앱 1", 8),
            ("dm", "DM 1", 9),
            ("other", "남", 10),
            ("app:c1", "앱 2", 11),
        ] {
            store
                .add_episode(&NewEpisode::user(conv, body, at(hour, 0)))
                .unwrap();
        }
        let convs = vec!["app:c1".to_owned(), "dm".to_owned()];
        let page = store.episodes_in_before(&convs, None, 2).unwrap();
        assert_eq!(
            page.iter().map(|e| e.body.as_str()).collect::<Vec<_>>(),
            ["DM 1", "앱 2"]
        );
        let older = store
            .episodes_in_before(&convs, Some(page[0].id), 10)
            .unwrap();
        assert_eq!(
            older.iter().map(|e| e.body.as_str()).collect::<Vec<_>>(),
            ["앱 1"]
        );
        assert!(store.episodes_in_before(&[], None, 10).unwrap().is_empty());
    }

    fn store_with(docs: &[(&str, Origin, u32)]) -> Store {
        let store = Store::open_in_memory().unwrap();
        for (body, origin, hour) in docs {
            let mut e = NewEpisode::user("c", body, at(*hour, 0));
            e.origin = *origin;
            if *origin != Origin::User {
                e.kind = EpisodeKind::AgentReport;
            }
            store.add_episode(&e).unwrap();
        }
        store
    }

    #[test]
    fn search_mixes_match_and_like_for_korean() {
        let store = store_with(&[
            ("운영체제 과제를 금요일까지 제출", Origin::User, 9),
            ("기말과제 마감이 수요일로 바뀜", Origin::Agent, 10),
            ("점심 약속 취소", Origin::User, 11),
        ]);
        let two = store.search_episodes("과제", 10).unwrap();
        assert_eq!(two.len(), 2, "두 글자는 LIKE로 합성어까지");
        assert_eq!(two[0].body, "기말과제 마감이 수요일로 바뀜", "최근 순");

        let three = store.search_episodes("금요일", 10).unwrap();
        assert_eq!(three.len(), 1);

        let both = store.search_episodes("과제 금요일", 10).unwrap();
        assert_eq!(both.len(), 1, "낱말은 AND");

        assert!(store.search_episodes("   ", 10).unwrap().is_empty());
    }

    #[test]
    fn search_treats_operators_and_wildcards_as_text() {
        let store = store_with(&[
            ("100% 완료 OR 아님", Origin::User, 9),
            ("다른 글", Origin::User, 10),
        ]);
        assert_eq!(store.search_episodes("%", 10).unwrap().len(), 1);
        assert_eq!(store.search_episodes("OR", 10).unwrap().len(), 1);
        assert_eq!(store.search_episodes("\"완료", 10).unwrap().len(), 0);
    }

    #[test]
    fn since_filters_by_origin() {
        let store = store_with(&[
            ("어제 사용자 말", Origin::User, 8),
            ("에이전트 보고", Origin::Agent, 9),
            ("또 사용자 말", Origin::User, 10),
        ]);
        let user = store
            .episodes_since(at(8, 30), Some(Origin::User), 10)
            .unwrap();
        assert_eq!(user.len(), 1);
        assert_eq!(user[0].body, "또 사용자 말");
        let all = store.episodes_since(at(8, 30), None, 10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(
            store.last_episode_at(EpisodeKind::AgentReport).unwrap(),
            Some(at(9, 0))
        );
        assert_eq!(store.last_episode_at(EpisodeKind::Briefing).unwrap(), None);
    }
}
