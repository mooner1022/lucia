//! 앱 이모지·스티커 목록(결정 0045). 그림 바이트는 디스크에 두고 여기에는 목록과 파일 정보만 둔다.
//! 파일 ID는 내용 해시라 두 항목이 같은 파일을 가리킬 수 있다. 가리키는 항목이 하나도 없어지면 그때부터
//! 보관 기간을 센다(예전 메시지·기록이 바로 깨지지 않게).

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension as _, params};

use crate::{Result, Store, StoreError, parse_opt_ts, parse_ts, ts};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ExpressionFile {
    pub id: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub size: u64,
    pub animated: bool,
    pub created_at: DateTime<Utc>,
    pub unlisted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Expression {
    pub id: String,
    /// emoji | sticker
    pub kind: String,
    pub name: String,
    pub label: String,
    pub description: String,
    pub tags: Vec<String>,
    pub category: String,
    /// admin | agent
    pub origin: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub file: ExpressionFile,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ExpressionCategory {
    pub id: String,
    pub label: String,
    pub sort: i64,
    pub icon: Option<String>,
}

const COLUMNS: &str = "e.id, e.kind, e.name, e.label, e.description, e.tags, e.category, e.origin, e.created_at, e.updated_at, \
     f.id, f.mime, f.width, f.height, f.size, f.animated, f.created_at, f.unlisted_at";

type Raw = (
    (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    ),
    (String, String, u32, u32, i64, bool, String, Option<String>),
);

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Raw> {
    Ok((
        (
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
            r.get(7)?,
            r.get(8)?,
            r.get(9)?,
        ),
        (
            r.get(10)?,
            r.get(11)?,
            r.get(12)?,
            r.get(13)?,
            r.get(14)?,
            r.get(15)?,
            r.get(16)?,
            r.get(17)?,
        ),
    ))
}

fn finish((e, f): Raw) -> Result<Expression> {
    Ok(Expression {
        id: e.0,
        kind: e.1,
        name: e.2,
        label: e.3,
        description: e.4,
        tags: serde_json::from_str(&e.5)?,
        category: e.6,
        origin: e.7,
        created_at: parse_ts(&e.8)?,
        updated_at: parse_ts(&e.9)?,
        file: ExpressionFile {
            id: f.0,
            mime: f.1,
            width: f.2,
            height: f.3,
            size: f.4.max(0) as u64,
            animated: f.5,
            created_at: parse_ts(&f.6)?,
            unlisted_at: parse_opt_ts(f.7)?,
        },
    })
}

/// 겹침 확인에 쓰는 이름(앞뒤 공백 없이 소문자).
pub fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

impl Store {
    /// 목록 전체(이모지 먼저, 만든 순).
    pub fn expressions(&self) -> Result<Vec<Expression>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM expressions e JOIN expression_files f ON f.id = e.file_id \
             ORDER BY e.kind ASC, e.created_at ASC, e.id ASC"
        ))?;
        let rows = stmt.query_map([], row)?;
        rows.map(|r| finish(r?)).collect()
    }

    pub fn expression(&self, id: &str) -> Result<Option<Expression>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM expressions e JOIN expression_files f ON f.id = e.file_id WHERE e.id = ?1"
                ),
                [id],
                row,
            )
            .optional()?
            .map(finish)
            .transpose()
    }

    /// 항목을 더한다. 파일 정보는 없으면 넣고, 있으면 다시 쓰이는 것으로 돌린다. 같은 종류에 같은 이름이
    /// 있으면 `Invalid`.
    pub fn add_expression(&self, e: &Expression) -> Result<()> {
        let key = name_key(&e.name);
        let tx = self.conn.unchecked_transaction()?;
        let taken: Option<String> = tx
            .query_row(
                "SELECT id FROM expressions WHERE kind = ?1 AND name_key = ?2",
                params![e.kind, key],
                |r| r.get(0),
            )
            .optional()?;
        if taken.is_some() {
            return Err(StoreError::Invalid(format!(
                "같은 이름이 이미 있다: {}",
                e.name
            )));
        }
        let f = &e.file;
        tx.execute(
            "INSERT INTO expression_files (id, mime, width, height, size, animated, created_at, unlisted_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL) \
             ON CONFLICT (id) DO UPDATE SET unlisted_at = NULL",
            params![f.id, f.mime, f.width, f.height, f.size as i64, f.animated, ts(f.created_at)],
        )?;
        tx.execute(
            "INSERT INTO expressions (id, kind, name, name_key, label, description, tags, category, file_id, origin, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                e.id,
                e.kind,
                e.name,
                key,
                e.label,
                e.description,
                serde_json::to_string(&e.tags)?,
                e.category,
                f.id,
                e.origin,
                ts(e.created_at),
                ts(e.updated_at),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 보이는 이름·설명·태그·분류를 고친다(이름과 그림은 바꾸지 않는다: 기록의 표기가 깨진다).
    pub fn update_expression(
        &self,
        id: &str,
        label: &str,
        description: &str,
        tags: &[String],
        category: &str,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE expressions SET label = ?2, description = ?3, tags = ?4, category = ?5, updated_at = ?6 WHERE id = ?1",
            params![id, label, description, serde_json::to_string(tags)?, category, ts(now)],
        )?;
        Ok(n > 0)
    }

    /// 항목을 지운다. 그 파일을 가리키는 항목이 더 없으면 그때부터 보관 기간을 센다.
    pub fn delete_expression(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let file: Option<String> = tx
            .query_row("SELECT file_id FROM expressions WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(file) = file else {
            return Ok(false);
        };
        tx.execute("DELETE FROM expressions WHERE id = ?1", [id])?;
        tx.execute(
            "UPDATE expression_categories SET icon = NULL WHERE icon = ?1",
            [id],
        )?;
        tx.execute(
            "UPDATE expression_files SET unlisted_at = ?2 WHERE id = ?1 \
             AND NOT EXISTS (SELECT 1 FROM expressions WHERE file_id = ?1)",
            params![file, ts(now)],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn expression_file(&self, id: &str) -> Result<Option<ExpressionFile>> {
        let raw = self
            .conn
            .query_row(
                "SELECT id, mime, width, height, size, animated, created_at, unlisted_at FROM expression_files WHERE id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, u32>(2)?,
                        r.get::<_, u32>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, bool>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()?;
        raw.map(|f| {
            Ok(ExpressionFile {
                id: f.0,
                mime: f.1,
                width: f.2,
                height: f.3,
                size: f.4.max(0) as u64,
                animated: f.5,
                created_at: parse_ts(&f.6)?,
                unlisted_at: parse_opt_ts(f.7)?,
            })
        })
        .transpose()
    }

    /// 목록에서 빠진 지 오래된 파일을 표에서 지우고 그 ID를 돌려준다(디스크의 그림은 부른 쪽이 지운다).
    pub fn prune_expression_files(&self, before: DateTime<Utc>) -> Result<Vec<String>> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM expression_files WHERE unlisted_at IS NOT NULL AND unlisted_at < ?1 \
                 AND NOT EXISTS (SELECT 1 FROM expressions WHERE file_id = expression_files.id)",
            )?;
            let rows = stmt.query_map([ts(before)], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for id in &ids {
            self.conn
                .execute("DELETE FROM expression_files WHERE id = ?1", [id])?;
        }
        Ok(ids)
    }

    pub fn expression_categories(&self) -> Result<Vec<ExpressionCategory>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, label, sort, icon FROM expression_categories ORDER BY sort ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ExpressionCategory {
                id: r.get(0)?,
                label: r.get(1)?,
                sort: r.get(2)?,
                icon: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 분류를 넣거나 고친다.
    pub fn put_expression_category(&self, c: &ExpressionCategory) -> Result<()> {
        self.conn.execute(
            "INSERT INTO expression_categories (id, label, sort, icon) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (id) DO UPDATE SET label = excluded.label, sort = excluded.sort, icon = excluded.icon",
            params![c.id, c.label, c.sort, c.icon],
        )?;
        Ok(())
    }

    /// 분류를 지운다. 그 분류의 항목은 분류 없음이 된다.
    pub fn delete_expression_category(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let n = tx.execute("DELETE FROM expression_categories WHERE id = ?1", [id])?;
        if n > 0 {
            tx.execute(
                "UPDATE expressions SET category = '', updated_at = ?2 WHERE category = ?1",
                params![id, ts(now)],
            )?;
        }
        tx.commit()?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    fn at(d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, d, 9, 0, 0).unwrap()
    }

    fn file(id: &str) -> ExpressionFile {
        ExpressionFile {
            id: id.into(),
            mime: "image/png".into(),
            width: 128,
            height: 128,
            size: 900,
            animated: false,
            created_at: at(1),
            unlisted_at: None,
        }
    }

    fn item(id: &str, kind: &str, name: &str, file_id: &str) -> Expression {
        Expression {
            id: id.into(),
            kind: kind.into(),
            name: name.into(),
            label: "웃음".into(),
            description: "웃는 얼굴".into(),
            tags: vec!["웃음".into()],
            category: "lucia".into(),
            origin: "admin".into(),
            created_at: at(1),
            updated_at: at(1),
            file: file(file_id),
        }
    }

    #[test]
    fn items_share_files_and_retention_starts_when_unreferenced() {
        let store = Store::open_in_memory().unwrap();
        store
            .put_expression_category(&ExpressionCategory {
                id: "lucia".into(),
                label: "루시아".into(),
                sort: 0,
                icon: Some("e_1".into()),
            })
            .unwrap();
        store
            .add_expression(&item("e_1", "emoji", "smile", "x_a"))
            .unwrap();
        // 이름은 종류 안에서만 겹치지 않는다(대소문자·공백 무시). 스티커는 같은 이름을 쓸 수 있다.
        assert!(matches!(
            store.add_expression(&item("e_2", "emoji", " Smile ", "x_b")),
            Err(StoreError::Invalid(_))
        ));
        store
            .add_expression(&item("s_1", "sticker", "smile", "x_a"))
            .unwrap();
        let all = store.expressions().unwrap();
        assert_eq!(
            all.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["e_1", "s_1"]
        );
        assert_eq!(all[0].tags, vec!["웃음".to_owned()]);
        assert!(
            store
                .update_expression("e_1", "미소", "살짝 웃음", &[], "", at(2))
                .unwrap()
        );
        let e = store.expression("e_1").unwrap().unwrap();
        assert_eq!((e.label.as_str(), e.category.as_str()), ("미소", ""));
        // 하나를 지워도 다른 항목이 같은 파일을 쓰면 보관 기간을 세지 않는다. 분류 아이콘은 비운다.
        assert!(store.delete_expression("e_1", at(3)).unwrap());
        assert!(store.expression_categories().unwrap()[0].icon.is_none());
        assert!(
            store
                .expression_file("x_a")
                .unwrap()
                .unwrap()
                .unlisted_at
                .is_none()
        );
        assert!(store.delete_expression("s_1", at(4)).unwrap());
        assert_eq!(
            store.expression_file("x_a").unwrap().unwrap().unlisted_at,
            Some(at(4))
        );
        assert!(store.prune_expression_files(at(4)).unwrap().is_empty());
        // 다시 쓰이면 되살아난다.
        store
            .add_expression(&item("e_3", "emoji", "again", "x_a"))
            .unwrap();
        assert!(store.prune_expression_files(at(30)).unwrap().is_empty());
        store.delete_expression("e_3", at(5)).unwrap();
        assert_eq!(store.prune_expression_files(at(6)).unwrap(), ["x_a"]);
        assert!(store.expression_file("x_a").unwrap().is_none());
        // 분류를 지우면 그 분류의 항목은 분류 없음이 된다.
        store
            .add_expression(&item("e_4", "emoji", "four", "x_c"))
            .unwrap();
        assert!(store.delete_expression_category("lucia", at(7)).unwrap());
        assert_eq!(store.expression("e_4").unwrap().unwrap().category, "");
        assert!(!store.delete_expression_category("lucia", at(7)).unwrap());
    }
}
