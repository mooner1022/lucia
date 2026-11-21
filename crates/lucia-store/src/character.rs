//! 캐릭터 파일 변경 기록(결정 0009). 파일 자체는 레포의 `character/character.md`에 있고(샌드박스가 읽기 전용으로
//! 마운트한다), 여기에는 관리 페이지에서 고칠 때마다 전체 내용을 남긴다.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension as _, params};

use crate::{Result, Store, parse_ts, ts};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CharacterRevision {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub note: String,
    pub content: String,
}

impl Store {
    pub fn add_character_revision(
        &self,
        content: &str,
        note: &str,
        at: DateTime<Utc>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO character_revisions (at, note, content) VALUES (?1, ?2, ?3)",
            params![ts(at), note, content],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 최근 개정(새것부터).
    pub fn character_revisions(&self, limit: usize) -> Result<Vec<CharacterRevision>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, at, note, content FROM character_revisions ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit as i64], |r| {
                Ok((r.get(0)?, r.get::<_, String>(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<std::result::Result<Vec<(i64, String, String, String)>, _>>()?;
        rows.into_iter()
            .map(|(id, at, note, content)| {
                Ok(CharacterRevision {
                    id,
                    at: parse_ts(&at)?,
                    note,
                    content,
                })
            })
            .collect()
    }

    pub fn character_revision(&self, id: i64) -> Result<Option<CharacterRevision>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, at, note, content FROM character_revisions WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get::<_, String>(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        row.map(|(id, at, note, content)| {
            Ok(CharacterRevision {
                id,
                at: parse_ts(&at)?,
                note,
                content,
            })
        })
        .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    #[test]
    fn revisions_keep_full_content_newest_first() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.character_revisions(5).unwrap().is_empty());
        let a = store
            .add_character_revision("# 비서\n존댓말\n", "처음", at(9, 0))
            .unwrap();
        let b = store
            .add_character_revision("# 비서\n반말\n", "말투 바꿈", at(9, 5))
            .unwrap();
        let list = store.character_revisions(5).unwrap();
        assert_eq!(list.iter().map(|r| r.id).collect::<Vec<_>>(), [b, a]);
        assert_eq!(list[0].content, "# 비서\n반말\n");
        let first = store.character_revision(a).unwrap().unwrap();
        assert_eq!(first.note, "처음");
        assert_eq!(first.at, at(9, 0));
        assert!(store.character_revision(999).unwrap().is_none());
    }
}
