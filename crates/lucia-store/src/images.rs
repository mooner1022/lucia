//! 이미지 생성 기록(결정 0046). 요청 하나가 한 줄이고, 끝나면 결과와 비용을 채운다.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension as _, params};

use crate::{Result, Store, parse_opt_ts, parse_ts, ts};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct GeneratedImage {
    pub id: String,
    pub run_id: Option<String>,
    pub model: String,
    pub quality: String,
    pub size: String,
    /// opaque | transparent
    pub background: String,
    pub prompt: String,
    /// running | done | failed
    pub status: String,
    pub file_id: Option<String>,
    pub error: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// 요청에 넣은 그림 수(편집). 0이면 새로 그린 것.
    pub inputs: i64,
}

const COLUMNS: &str = "id, run_id, model, quality, size, background, prompt, status, file_id, error, input_tokens, output_tokens, cost_usd, created_at, finished_at, inputs";

type Raw = (
    (
        String,
        Option<String>,
        String,
        String,
        String,
        String,
        String,
        String,
    ),
    (
        Option<String>,
        Option<String>,
        i64,
        i64,
        f64,
        String,
        Option<String>,
        i64,
    ),
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
        ),
        (
            r.get(8)?,
            r.get(9)?,
            r.get(10)?,
            r.get(11)?,
            r.get(12)?,
            r.get(13)?,
            r.get(14)?,
            r.get(15)?,
        ),
    ))
}

fn finish((a, b): Raw) -> Result<GeneratedImage> {
    Ok(GeneratedImage {
        id: a.0,
        run_id: a.1,
        model: a.2,
        quality: a.3,
        size: a.4,
        background: a.5,
        prompt: a.6,
        status: a.7,
        file_id: b.0,
        error: b.1,
        input_tokens: b.2,
        output_tokens: b.3,
        cost_usd: b.4,
        created_at: parse_ts(&b.5)?,
        finished_at: parse_opt_ts(b.6)?,
        inputs: b.7,
    })
}

impl Store {
    /// 새 요청을 `running`으로 넣는다.
    pub fn add_generated_image(&self, g: &GeneratedImage) -> Result<()> {
        self.conn.execute(
            "INSERT INTO generated_images (id, run_id, model, quality, size, background, prompt, status, created_at, inputs) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?8, ?9)",
            params![
                g.id,
                g.run_id,
                g.model,
                g.quality,
                g.size,
                g.background,
                g.prompt,
                ts(g.created_at),
                g.inputs
            ],
        )?;
        Ok(())
    }

    /// 끝난 요청: 성공이면 그림 파일, 실패면 까닭. 비용은 실패해도 받은 만큼 남긴다.
    #[allow(clippy::too_many_arguments)]
    pub fn finish_generated_image(
        &self,
        id: &str,
        file_id: Option<&str>,
        error: Option<&str>,
        input_tokens: i64,
        output_tokens: i64,
        cost_usd: f64,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let status = if file_id.is_some() { "done" } else { "failed" };
        let n = self.conn.execute(
            "UPDATE generated_images SET status = ?2, file_id = ?3, error = ?4, input_tokens = ?5, output_tokens = ?6, \
             cost_usd = ?7, finished_at = ?8 WHERE id = ?1 AND status = 'running'",
            params![id, status, file_id, error, input_tokens, output_tokens, cost_usd, ts(now)],
        )?;
        Ok(n > 0)
    }

    pub fn generated_image(&self, id: &str) -> Result<Option<GeneratedImage>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM generated_images WHERE id = ?1"),
                [id],
                row,
            )
            .optional()?
            .map(finish)
            .transpose()
    }

    /// 최근 요청(새것부터).
    pub fn recent_generated_images(&self, limit: usize) -> Result<Vec<GeneratedImage>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM generated_images ORDER BY created_at DESC, id DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map([limit as i64], row)?;
        rows.map(|r| finish(r?)).collect()
    }

    /// `since` 뒤에 시작한 요청의 비용 합.
    pub fn generated_image_spend_since(&self, since: DateTime<Utc>) -> Result<f64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(cost_usd), 0) FROM generated_images WHERE created_at >= ?1",
            [ts(since)],
            |r| r.get(0),
        )?)
    }

    /// 데몬이 다시 뜰 때: 끝을 보지 못한 요청을 실패로 닫는다(결과를 받을 길이 없다). 닫은 수.
    pub fn close_stale_generated_images(&self, why: &str, now: DateTime<Utc>) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE generated_images SET status = 'failed', error = ?1, finished_at = ?2 WHERE status = 'running'",
            params![why, ts(now)],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    fn at(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 19, h, 0, 0).unwrap()
    }

    fn job(id: &str, when: DateTime<Utc>) -> GeneratedImage {
        GeneratedImage {
            id: id.into(),
            run_id: Some("chat-1".into()),
            model: "gpt-image-2.5-flare".into(),
            quality: "medium".into(),
            size: "1024x1024".into(),
            background: "opaque".into(),
            prompt: "고양이".into(),
            status: "running".into(),
            file_id: None,
            error: None,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            created_at: when,
            finished_at: None,
            inputs: 0,
        }
    }

    #[test]
    fn jobs_finish_once_and_spend_is_summed_by_day() {
        let store = Store::open_in_memory().unwrap();
        store.add_generated_image(&job("img_1", at(1))).unwrap();
        store.add_generated_image(&job("img_2", at(9))).unwrap();
        store.add_generated_image(&job("img_3", at(10))).unwrap();
        assert!(
            store
                .finish_generated_image("img_1", Some("f_1"), None, 20, 1000, 0.03, at(2))
                .unwrap()
        );
        assert!(
            !store
                .finish_generated_image("img_1", None, Some("늦은 답"), 0, 0, 9.0, at(3))
                .unwrap(),
            "끝난 요청은 다시 닫지 않는다"
        );
        store
            .finish_generated_image("img_2", None, Some("안전 검사"), 0, 0, 0.0, at(9))
            .unwrap();
        let one = store.generated_image("img_1").unwrap().unwrap();
        assert_eq!(
            (
                one.status.as_str(),
                one.file_id.as_deref(),
                one.output_tokens
            ),
            ("done", Some("f_1"), 1000)
        );
        let two = store.generated_image("img_2").unwrap().unwrap();
        assert_eq!(
            (two.status.as_str(), two.error.as_deref()),
            ("failed", Some("안전 검사"))
        );
        assert_eq!(
            store
                .recent_generated_images(2)
                .unwrap()
                .iter()
                .map(|g| g.id.as_str())
                .collect::<Vec<_>>(),
            ["img_3", "img_2"]
        );
        assert!((store.generated_image_spend_since(at(0)).unwrap() - 0.03).abs() < 1e-9);
        assert_eq!(store.generated_image_spend_since(at(5)).unwrap(), 0.0);
        // 다시 뜰 때 끝을 보지 못한 것만 닫는다.
        assert_eq!(
            store
                .close_stale_generated_images("데몬이 다시 시작됐다", at(11))
                .unwrap(),
            1
        );
        assert_eq!(
            store.generated_image("img_3").unwrap().unwrap().status,
            "failed"
        );
        assert!(store.generated_image("img_x").unwrap().is_none());
    }
}
