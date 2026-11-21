//! 주고받은 파일 목록(결정 0005). 파일 내용은 데몬의 파일 디렉터리에 있고 여기는 목록만 둔다.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Row, params};

use crate::{Result, Store, parse_ts, ts};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FileRecord {
    pub id: String,
    /// `in`(사용자가 보냄), `out`(에이전트가 만듦), `web`(에이전트가 웹에서 받음, 결정 0057).
    pub direction: String,
    pub conversation: Option<String>,
    pub name: String,
    pub mime: String,
    pub kind: String,
    pub size: u64,
    pub sha256: String,
    pub rel_path: String,
    pub created_at: DateTime<Utc>,
    pub run_id: Option<String>,
}

/// 보관함에 넣은 파일(결정 0052).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct KeptFile {
    pub file: FileRecord,
    pub title: String,
    pub note: String,
    pub kept_at: DateTime<Utc>,
}

const COLUMNS: &str =
    "id, direction, conversation, name, mime, kind, size, sha256, rel_path, created_at, run_id";

fn row(r: &Row<'_>) -> rusqlite::Result<(FileRecord, String)> {
    Ok((
        FileRecord {
            id: r.get(0)?,
            direction: r.get(1)?,
            conversation: r.get(2)?,
            name: r.get(3)?,
            mime: r.get(4)?,
            kind: r.get(5)?,
            size: r.get::<_, i64>(6)?.max(0) as u64,
            sha256: r.get(7)?,
            rel_path: r.get(8)?,
            created_at: Utc::now(),
            run_id: r.get(10)?,
        },
        r.get::<_, String>(9)?,
    ))
}

fn finish((mut f, at): (FileRecord, String)) -> Result<FileRecord> {
    f.created_at = parse_ts(&at)?;
    Ok(f)
}

impl Store {
    pub fn add_file(&self, f: &FileRecord) -> Result<()> {
        self.conn.execute(
            &format!("INSERT INTO files ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
            params![
                f.id,
                f.direction,
                f.conversation,
                f.name,
                f.mime,
                f.kind,
                f.size as i64,
                f.sha256,
                f.rel_path,
                ts(f.created_at),
                f.run_id
            ],
        )?;
        Ok(())
    }

    pub fn file(&self, id: &str) -> Result<Option<FileRecord>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM files WHERE id = ?1"),
                [id],
                row,
            )
            .optional()?
            .map(finish)
            .transpose()
    }

    /// 최근 파일(새것부터).
    pub fn recent_files(&self, limit: usize) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM files ORDER BY created_at DESC, id DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map([limit as i64], row)?;
        rows.map(|r| finish(r?)).collect()
    }

    /// 파일을 보관함에 넣는다(이미 있으면 이름과 설명을 고친다). 없는 파일이면 `false`.
    pub fn keep_file(&self, id: &str, title: &str, note: &str, now: DateTime<Utc>) -> Result<bool> {
        if self.file(id)?.is_none() {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO kept_files (file_id, title, note, kept_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (file_id) DO UPDATE SET title = excluded.title, note = excluded.note",
            params![id, title, note, ts(now)],
        )?;
        Ok(true)
    }

    /// 보관함에서 뺀다(파일은 남고, 보관 기간이 지나면 지워진다).
    pub fn unkeep_file(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM kept_files WHERE file_id = ?1", [id])?
            > 0)
    }

    /// 보관함(새로 넣은 것부터).
    pub fn kept_files(&self) -> Result<Vec<KeptFile>> {
        let columns = COLUMNS
            .split(", ")
            .map(|c| format!("f.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {columns}, k.title, k.note, k.kept_at FROM kept_files k JOIN files f ON f.id = k.file_id \
             ORDER BY k.kept_at DESC, f.id DESC"
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok((
                row(r)?,
                r.get::<_, String>(11)?,
                r.get::<_, String>(12)?,
                r.get::<_, String>(13)?,
            ))
        })?;
        rows.map(|r| {
            let (f, title, note, at) = r?;
            Ok(KeptFile {
                file: finish(f)?,
                title,
                note,
                kept_at: parse_ts(&at)?,
            })
        })
        .collect()
    }

    /// 이 시각보다 오래된 파일 목록(지우기 전에 경로를 얻는다). 보관함의 파일은 뺀다.
    pub fn files_older_than(&self, before: DateTime<Utc>) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM files WHERE created_at < ?1 \
             AND id NOT IN (SELECT file_id FROM kept_files) ORDER BY created_at"
        ))?;
        let rows = stmt.query_map([ts(before)], row)?;
        rows.map(|r| finish(r?)).collect()
    }

    /// 이 방향의 파일이 이 시각부터 차지한 바이트 합(웹에서 받은 파일의 하루 한도, 결정 0057).
    pub fn file_bytes_since(&self, direction: &str, since: DateTime<Utc>) -> Result<u64> {
        let n: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(size), 0) FROM files WHERE direction = ?1 AND created_at >= ?2",
            params![direction, ts(since)],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u64)
    }

    pub fn delete_file(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM kept_files WHERE file_id = ?1", [id])?;
        self.conn.execute("DELETE FROM files WHERE id = ?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    #[test]
    fn files_roundtrip_and_age_out() {
        let store = Store::open_in_memory().unwrap();
        let f = FileRecord {
            id: "f_1".into(),
            direction: "in".into(),
            conversation: Some("dm".into()),
            name: "메모.pdf".into(),
            mime: "application/pdf".into(),
            kind: "pdf".into(),
            size: 1234,
            sha256: "ab".into(),
            rel_path: "in/f_1-메모.pdf".into(),
            created_at: at(9, 0),
            run_id: None,
        };
        store.add_file(&f).unwrap();
        assert_eq!(store.file("f_1").unwrap().unwrap(), f);
        assert!(store.file("f_2").unwrap().is_none());
        assert!(store.files_older_than(at(9, 0)).unwrap().is_empty());
        assert_eq!(store.files_older_than(at(10, 0)).unwrap().len(), 1);
        // 방향별 합계(결정 0057): 그 시각부터의 것만 센다.
        assert_eq!(store.file_bytes_since("in", at(9, 0)).unwrap(), 1234);
        assert_eq!(store.file_bytes_since("in", at(9, 1)).unwrap(), 0);
        assert_eq!(store.file_bytes_since("web", at(8, 0)).unwrap(), 0);
        // 보관함(결정 0052): 넣은 파일은 정리 대상에서 빠진다. 빼면 다시 대상이 된다.
        assert!(!store.keep_file("f_none", "", "", at(9, 30)).unwrap());
        assert!(
            store
                .keep_file("f_1", "계약서", "전세 계약", at(9, 30))
                .unwrap()
        );
        assert!(store.files_older_than(at(10, 0)).unwrap().is_empty());
        assert!(
            store
                .keep_file("f_1", "계약서", "전세 계약(2026)", at(9, 40))
                .unwrap()
        );
        let kept = store.kept_files().unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            (
                kept[0].title.as_str(),
                kept[0].note.as_str(),
                kept[0].kept_at
            ),
            ("계약서", "전세 계약(2026)", at(9, 30))
        );
        assert_eq!(kept[0].file, f);
        assert!(store.unkeep_file("f_1").unwrap());
        assert!(!store.unkeep_file("f_1").unwrap());
        assert_eq!(store.files_older_than(at(10, 0)).unwrap().len(), 1);
        store.delete_file("f_1").unwrap();
        assert!(store.file("f_1").unwrap().is_none());
    }
}
