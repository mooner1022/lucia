//! 감시(결정 0019): 사람이 승인한 공개 페이지 구간. 확인 기록(기준점, 마지막 확인)은 사전 체크 소스
//! 상태(`source_state`, 이름 `watch:<id>`)에 둔다.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension as _, params};

use crate::{Result, Store, StoreError, parse_ts, ts};

pub const MAX_WATCHES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum WatchStatus {
    Pending,
    Active,
    /// 계속 바뀌어(잡음) 멈췄다.
    Paused,
    Removed,
}

impl WatchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Removed => "removed",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "active" => Self::Active,
            "paused" => Self::Paused,
            "removed" => Self::Removed,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "승인 대기",
            Self::Active => "감시 중",
            Self::Paused => "멈춤",
            Self::Removed => "지움",
        }
    }
}

/// 감시 방식(결정 0036).
pub mod mode {
    /// 페이지 한 구간이 바뀌었는가(결정 0019).
    pub const REGION: &str = "region";
    /// 글 목록의 새 항목.
    pub const ITEMS: &str = "items";
}

/// 새 항목을 알리는 방식(결정 0036).
pub mod notify {
    /// 모델 없이 바로 알린다.
    pub const NOW: &str = "now";
    /// 신호 처리(작은 모델 분류, 결정 0031)에 넣는다.
    pub const CLASSIFY: &str = "classify";
    /// 묶음 소식과 브리핑으로만.
    pub const DIGEST: &str = "digest";
    pub const ALL: [&str; 3] = [NOW, CLASSIFY, DIGEST];
}

/// 페이지를 받는 방식(결정 0036).
pub mod fetcher {
    /// 쿠키 없이 HTTP.
    pub const PLAIN: &str = "plain";
    /// 로그인된 브라우저의 쿠키를 빌려 HTTP.
    pub const SESSION: &str = "session";
    /// 로그인된 브라우저의 감시 탭에서 읽기.
    pub const BROWSER: &str = "browser";
    pub const ALL: [&str; 3] = [PLAIN, SESSION, BROWSER];
}

/// 멈춘 까닭(`paused_reason`).
pub mod paused {
    /// 확인할 때마다 바뀜(구간 감시의 잡음).
    pub const NOISE: &str = "noise";
    /// 로그인이 풀렸고 다시 들어가지 못했다.
    pub const LOGIN: &str = "login";
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Watch {
    pub id: String,
    pub url: String,
    pub what: String,
    pub start_text: String,
    pub end_text: String,
    pub interval_minutes: i64,
    pub status: WatchStatus,
    pub changes_in_row: i64,
    pub run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// `mode::REGION` | `mode::ITEMS`.
    pub mode: String,
    pub item_pattern: String,
    /// 줄마다 하나.
    pub include_words: String,
    pub exclude_words: String,
    /// `notify::*`.
    pub notify: String,
    /// `fetcher::*`.
    pub fetcher: String,
    pub failures: i64,
    pub paused_reason: String,
    pub sent_day: String,
    pub sent_count: i64,
}

impl Watch {
    pub fn is_items(&self) -> bool {
        self.mode == mode::ITEMS
    }

    /// 사람에게 보이는 상태(멈춘 까닭 포함).
    pub fn status_label(&self) -> &'static str {
        match (self.status, self.paused_reason.as_str()) {
            (WatchStatus::Paused, paused::NOISE) => "멈춤(계속 바뀜)",
            (WatchStatus::Paused, paused::LOGIN) => "멈춤(로그인 필요)",
            (s, _) => s.label(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewWatch<'a> {
    pub id: &'a str,
    pub url: &'a str,
    pub what: &'a str,
    pub start_text: &'a str,
    pub end_text: &'a str,
    pub interval_minutes: i64,
    pub run_id: Option<&'a str>,
    pub mode: &'a str,
    pub item_pattern: &'a str,
    pub include_words: &'a str,
    pub exclude_words: &'a str,
    pub notify: &'a str,
    pub fetcher: &'a str,
}

impl Default for NewWatch<'_> {
    fn default() -> Self {
        Self {
            id: "",
            url: "",
            what: "",
            start_text: "",
            end_text: "",
            interval_minutes: 60,
            run_id: None,
            mode: mode::REGION,
            item_pattern: "",
            include_words: "",
            exclude_words: "",
            notify: notify::DIGEST,
            fetcher: fetcher::PLAIN,
        }
    }
}

const COLUMNS: &str = "id, url, what, start_text, end_text, interval_minutes, status, changes_in_row, run_id, created_at, updated_at, \
    mode, item_pattern, include_words, exclude_words, notify, fetcher, failures, paused_reason, sent_day, sent_count";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Watch>> {
    let status: String = r.get(6)?;
    let created: String = r.get(9)?;
    let updated: String = r.get(10)?;
    let base = (
        r.get::<_, String>(0)?,
        r.get::<_, String>(1)?,
        r.get::<_, String>(2)?,
        r.get::<_, String>(3)?,
        r.get::<_, String>(4)?,
        r.get::<_, i64>(5)?,
        r.get::<_, i64>(7)?,
        r.get::<_, Option<String>>(8)?,
    );
    let mode: String = r.get(11)?;
    let item_pattern: String = r.get(12)?;
    let include_words: String = r.get(13)?;
    let exclude_words: String = r.get(14)?;
    let notify: String = r.get(15)?;
    let fetcher: String = r.get(16)?;
    let failures: i64 = r.get(17)?;
    let paused_reason: String = r.get(18)?;
    let sent_day: String = r.get(19)?;
    let sent_count: i64 = r.get(20)?;
    Ok((|| {
        let (id, url, what, start_text, end_text, interval_minutes, changes_in_row, run_id) = base;
        Ok(Watch {
            status: WatchStatus::parse(&status)
                .ok_or_else(|| StoreError::Corrupt(format!("watch status {status:?}")))?,
            created_at: parse_ts(&created)?,
            updated_at: parse_ts(&updated)?,
            id,
            url,
            what,
            start_text,
            end_text,
            interval_minutes,
            changes_in_row,
            run_id,
            mode,
            item_pattern,
            include_words,
            exclude_words,
            notify,
            fetcher,
            failures,
            paused_reason,
            sent_day,
            sent_count,
        })
    })())
}

impl Store {
    pub fn create_watch(&self, w: &NewWatch<'_>, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO watches (id, url, what, start_text, end_text, interval_minutes, status, run_id, created_at, updated_at,
                mode, item_pattern, include_words, exclude_words, notify, fetcher)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                w.id, w.url, w.what, w.start_text, w.end_text, w.interval_minutes, w.run_id, ts(now),
                w.mode, w.item_pattern, w.include_words, w.exclude_words, w.notify, w.fetcher
            ],
        )?;
        Ok(())
    }

    pub fn watch(&self, id: &str) -> Result<Option<Watch>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM watches WHERE id = ?1"),
                [id],
                row,
            )
            .optional()?
            .transpose()
    }

    /// 지우지 않은 감시(오래된 것부터).
    pub fn watches(&self) -> Result<Vec<Watch>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM watches WHERE status != 'removed' ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([], row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    pub fn set_watch_status(
        &self,
        id: &str,
        status: WatchStatus,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE watches SET status = ?2, changes_in_row = 0, failures = 0,
                paused_reason = CASE WHEN ?2 = 'paused' THEN paused_reason ELSE '' END, updated_at = ?3
             WHERE id = ?1",
            params![id, status.as_str(), ts(now)],
        )?;
        Ok(n == 1)
    }

    /// 확인 한 번의 결과를 센다. 바뀌었으면 연속 횟수를 늘리고, `pause_at`번 이어 바뀌면 멈춘다(`true`).
    pub fn note_watch_check(
        &self,
        id: &str,
        changed: bool,
        pause_at: i64,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        if changed {
            self.conn.execute(
                "UPDATE watches SET changes_in_row = changes_in_row + 1, updated_at = ?2 WHERE id = ?1",
                params![id, ts(now)],
            )?;
        } else {
            self.conn
                .execute("UPDATE watches SET changes_in_row = 0 WHERE id = ?1", [id])?;
        }
        let paused = self.conn.execute(
            "UPDATE watches SET status = 'paused', paused_reason = 'noise', updated_at = ?3
             WHERE id = ?1 AND status = 'active' AND changes_in_row >= ?2",
            params![id, pause_at, ts(now)],
        )?;
        Ok(paused == 1)
    }

    /// 멈춘다(까닭과 함께). 감시 중인 것만.
    pub fn pause_watch(&self, id: &str, reason: &str, now: DateTime<Utc>) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE watches SET status = 'paused', paused_reason = ?2, updated_at = ?3
             WHERE id = ?1 AND status = 'active'",
            params![id, reason, ts(now)],
        )? == 1)
    }

    /// 확인이 성공하면 이어진 실패를 비우고, 실패하면 하나 늘린다. 늘린 뒤의 수를 돌려준다.
    pub fn note_watch_fetch(&self, id: &str, ok: bool) -> Result<i64> {
        if ok {
            self.conn
                .execute("UPDATE watches SET failures = 0 WHERE id = ?1", [id])?;
            return Ok(0);
        }
        self.conn.execute(
            "UPDATE watches SET failures = failures + 1 WHERE id = ?1",
            [id],
        )?;
        Ok(self
            .conn
            .query_row("SELECT failures FROM watches WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or(0))
    }

    /// 바로 알림을 `n`건 쓸 수 있는 만큼 쓴다(하루 `cap`건). 쓸 수 있었던 수를 돌려준다.
    pub fn take_watch_sends(&self, id: &str, day: &str, n: i64, cap: i64) -> Result<i64> {
        let (sent_day, sent): (String, i64) = self.conn.query_row(
            "SELECT sent_day, sent_count FROM watches WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let used = if sent_day == day { sent } else { 0 };
        let take = n.min((cap - used).max(0));
        self.conn.execute(
            "UPDATE watches SET sent_day = ?2, sent_count = ?3 WHERE id = ?1",
            params![id, day, used + take],
        )?;
        Ok(take)
    }

    /// 감시 설정을 고친다(관리 페이지·대화). 바뀐 것만 `Some`.
    pub fn update_watch(&self, id: &str, edit: &WatchEdit<'_>, now: DateTime<Utc>) -> Result<bool> {
        let Some(w) = self.watch(id)? else {
            return Ok(false);
        };
        let n = self.conn.execute(
            "UPDATE watches SET interval_minutes = ?2, include_words = ?3, exclude_words = ?4, notify = ?5,
                fetcher = ?6, item_pattern = ?7, updated_at = ?8 WHERE id = ?1",
            params![
                id,
                edit.interval_minutes.unwrap_or(w.interval_minutes),
                edit.include_words.unwrap_or(&w.include_words),
                edit.exclude_words.unwrap_or(&w.exclude_words),
                edit.notify.unwrap_or(&w.notify),
                edit.fetcher.unwrap_or(&w.fetcher),
                edit.item_pattern.unwrap_or(&w.item_pattern),
                ts(now)
            ],
        )?;
        Ok(n == 1)
    }
}

/// 감시 고치기(바꿀 것만).
#[derive(Debug, Clone, Default)]
pub struct WatchEdit<'a> {
    pub interval_minutes: Option<i64>,
    pub include_words: Option<&'a str>,
    pub exclude_words: Option<&'a str>,
    pub notify: Option<&'a str>,
    pub fetcher: Option<&'a str>,
    pub item_pattern: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    #[test]
    fn lifecycle_and_noise_guard() {
        let store = Store::open_in_memory().unwrap();
        store
            .create_watch(
                &NewWatch {
                    id: "w1",
                    url: "https://example.com/notice",
                    what: "학과 공지",
                    start_text: "공지사항",
                    end_text: "",
                    interval_minutes: 60,
                    run_id: Some("chat-1"),
                    ..NewWatch::default()
                },
                at(9, 0),
            )
            .unwrap();
        assert_eq!(
            store.watch("w1").unwrap().unwrap().status,
            WatchStatus::Pending
        );
        store
            .set_watch_status("w1", WatchStatus::Active, at(9, 1))
            .unwrap();
        assert!(!store.note_watch_check("w1", true, 3, at(10, 0)).unwrap());
        assert!(
            !store.note_watch_check("w1", false, 3, at(11, 0)).unwrap(),
            "안 바뀌면 다시 센다"
        );
        assert!(!store.note_watch_check("w1", true, 3, at(12, 0)).unwrap());
        assert!(!store.note_watch_check("w1", true, 3, at(13, 0)).unwrap());
        assert!(
            store.note_watch_check("w1", true, 3, at(14, 0)).unwrap(),
            "세 번 이어 바뀌면 멈춘다"
        );
        assert_eq!(
            store.watch("w1").unwrap().unwrap().status,
            WatchStatus::Paused
        );
        assert_eq!(
            store.watch("w1").unwrap().unwrap().paused_reason,
            paused::NOISE
        );
        store
            .set_watch_status("w1", WatchStatus::Active, at(14, 30))
            .unwrap();
        assert_eq!(store.watch("w1").unwrap().unwrap().paused_reason, "");
        assert_eq!(store.note_watch_fetch("w1", false).unwrap(), 1);
        assert_eq!(store.note_watch_fetch("w1", false).unwrap(), 2);
        assert_eq!(store.note_watch_fetch("w1", true).unwrap(), 0);
        assert_eq!(store.take_watch_sends("w1", "2026-11-18", 3, 4).unwrap(), 3);
        assert_eq!(store.take_watch_sends("w1", "2026-11-18", 3, 4).unwrap(), 1);
        assert_eq!(
            store.take_watch_sends("w1", "2026-11-19", 3, 4).unwrap(),
            3,
            "날이 바뀌면 다시 센다"
        );
        assert!(store.pause_watch("w1", paused::LOGIN, at(14, 40)).unwrap());
        assert_eq!(
            store.watch("w1").unwrap().unwrap().paused_reason,
            paused::LOGIN
        );
        store
            .update_watch(
                "w1",
                &WatchEdit {
                    notify: Some(notify::NOW),
                    ..WatchEdit::default()
                },
                at(14, 50),
            )
            .unwrap();
        let w = store.watch("w1").unwrap().unwrap();
        assert_eq!((w.notify.as_str(), w.interval_minutes), (notify::NOW, 60));
        store
            .set_watch_status("w1", WatchStatus::Removed, at(15, 0))
            .unwrap();
        assert!(store.watches().unwrap().is_empty());
    }
}
