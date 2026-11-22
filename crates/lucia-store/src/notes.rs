//! 메모(결정 0066, 앱의 스티커 메모를 데몬에도 둔다). 앱은 제 사본을 이것과 맞춘다.
//!
//! - 판(`rev`)은 전체에서 하나씩 늘고, 설정 `notes.rev`에 둔다(묘비를 지워도 내려가지 않는다). 바뀐 메모는 바뀐
//!   판을 제 `rev` 칸에 적는다. 앱은 마지막으로 본 판 뒤의 것만 다시 읽는다.
//! - `updated_at`은 앱이 뜻을 읽지 않는 표다. 고칠 때마다 달라지고(같은 순간이어도), 앱이 되돌려 보내는 `base`와
//!   맞는지로 충돌을 가린다.
//! - 뗀 메모는 지우지 않고 묘비(`removed`)로 둔다. 다른 기기가 뗀 것을 알 수 있게 하고, 30일 뒤 지운다.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{OptionalExtension as _, params};

use crate::{Result, Store, parse_ts, ts};

/// 메모 수 상한(묘비는 세지 않는다).
pub const MAX_NOTES: usize = 40;

/// 판을 적어 두는 설정 키.
const REV_KEY: &str = "notes.rev";

/// 메모 하나(표의 한 줄).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Note {
    pub id: String,
    pub text: String,
    pub color: String,
    /// 마지막으로 고친 쪽: `user` | `lucia`.
    pub by: String,
    /// 고칠 때마다 달라지는 표(RFC 3339, 마이크로초). 앱은 뜻을 읽지 않는다.
    pub updated_at: String,
    /// 마지막으로 바뀐 판.
    pub rev: i64,
    pub removed: bool,
    pub created_at: DateTime<Utc>,
}

/// [`Store::put_note`]의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotePut {
    /// 저장했다(만들었거나, 고쳤거나, 되살렸다).
    Saved(Note),
    /// `base`가 없거나 지금 것과 달라 받지 않았다. 지금 메모(묘비면 `removed`)를 돌려준다.
    Conflict(Note),
    /// 새로 만들거나 되살릴 때 살아 있는 메모가 이미 [`MAX_NOTES`]개다.
    Full,
}

const COLUMNS: &str = "id, text, color, by, updated_at, rev, removed, created_at";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(Note, String)> {
    let created: String = r.get(7)?;
    let removed: i64 = r.get(6)?;
    Ok((
        Note {
            id: r.get(0)?,
            text: r.get(1)?,
            color: r.get(2)?,
            by: r.get(3)?,
            updated_at: r.get(4)?,
            rev: r.get(5)?,
            removed: removed != 0,
            created_at: DateTime::<Utc>::MIN_UTC,
        },
        created,
    ))
}

fn finish((mut note, created): (Note, String)) -> Result<Note> {
    note.created_at = parse_ts(&created)?;
    Ok(note)
}

/// 표로 쓰는 시각(마이크로초, 사전순이 시간순이 되게 늘 같은 모양).
fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// 이번에 쓸 `updated_at`: 지금 시각이되, 앞의 값보다 늘 커진다(같거나 시계가 뒤로 가도 1마이크로초씩).
fn next_stamp(now: DateTime<Utc>, previous: Option<&str>) -> String {
    let now = stamp(now);
    match previous {
        Some(prev) if now.as_str() <= prev => match parse_ts(prev) {
            Ok(p) => stamp(p + Duration::microseconds(1)),
            Err(_) => now,
        },
        _ => now,
    }
}

impl Store {
    /// 지금의 판(메모가 하나도 바뀐 적 없으면 0).
    pub fn notes_rev(&self) -> Result<i64> {
        Ok(self
            .setting(REV_KEY)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    /// 판을 하나 올리고 그 값을 돌려준다(바꾸는 쪽이 같은 트랜잭션 안에서 부른다).
    fn bump_notes_rev(&self, now: DateTime<Utc>) -> Result<i64> {
        let rev = self.notes_rev()? + 1;
        self.set_setting(REV_KEY, &rev.to_string(), now)?;
        Ok(rev)
    }

    /// 지금의 판과, `since` 뒤에 바뀐 메모 전부(묘비 포함, 판 순서). `since`가 없으면 전부.
    pub fn notes_since(&self, since: Option<i64>) -> Result<(i64, Vec<Note>)> {
        let rev = self.notes_rev()?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM notes WHERE rev > ?1 ORDER BY rev, id"
        ))?;
        let rows = stmt.query_map([since.unwrap_or(i64::MIN)], from_row)?;
        let notes = rows.map(|r| finish(r?)).collect::<Result<Vec<_>>>()?;
        Ok((rev, notes))
    }

    pub fn note(&self, id: &str) -> Result<Option<Note>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM notes WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()?
            .map(finish)
            .transpose()
    }

    /// 살아 있는 메모 수.
    pub fn live_note_count(&self) -> Result<usize> {
        let n: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM notes WHERE removed = 0", [], |r| {
                    r.get(0)
                })?;
        Ok(n as usize)
    }

    /// 살아 있는 메모(만든 순). Lucia가 읽는다.
    pub fn live_notes(&self) -> Result<Vec<Note>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM notes WHERE removed = 0 ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([], from_row)?;
        rows.map(|r| finish(r?)).collect()
    }

    /// 메모를 만들거나 고친다(`base`는 앱이 마지막으로 받은 `updated_at`).
    ///
    /// - 이 `id`가 이미 있으면(묘비도) `base`가 있고 지금 `updated_at`과 같을 때만 받는다. 다르거나 없으면
    ///   [`NotePut::Conflict`]다. 묘비를 맞는 `base`로 올리면 되살아난다.
    /// - 없으면 만든다(`base`가 있어도: 그새 지워진 것이다).
    /// - 새로 살아나는 메모는 [`MAX_NOTES`]를 넘지 않는다.
    ///
    /// 글·색의 모양은 부르는 쪽이 이미 확인했다. 확인과 쓰기는 한 트랜잭션이다.
    pub fn put_note(
        &self,
        id: &str,
        text: &str,
        color: &str,
        by: &str,
        base: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<NotePut> {
        let tx = self.conn.unchecked_transaction()?;
        let current = self.note(id)?;
        let outcome = match current {
            Some(cur) if base != Some(cur.updated_at.as_str()) => NotePut::Conflict(cur),
            Some(cur) => {
                if cur.removed && self.live_note_count()? >= MAX_NOTES {
                    NotePut::Full
                } else {
                    let rev = self.bump_notes_rev(now)?;
                    let updated_at = next_stamp(now, Some(&cur.updated_at));
                    self.conn.execute(
                        "UPDATE notes SET text = ?2, color = ?3, by = ?4, updated_at = ?5, rev = ?6, removed = 0 \
                         WHERE id = ?1",
                        params![id, text, color, by, updated_at, rev],
                    )?;
                    NotePut::Saved(Note {
                        id: cur.id,
                        text: text.to_owned(),
                        color: color.to_owned(),
                        by: by.to_owned(),
                        updated_at,
                        rev,
                        removed: false,
                        created_at: cur.created_at,
                    })
                }
            }
            None if self.live_note_count()? >= MAX_NOTES => NotePut::Full,
            None => {
                let rev = self.bump_notes_rev(now)?;
                let updated_at = next_stamp(now, None);
                self.conn.execute(
                    "INSERT INTO notes (id, text, color, by, updated_at, rev, removed, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
                    params![id, text, color, by, updated_at, rev, ts(now)],
                )?;
                NotePut::Saved(Note {
                    id: id.to_owned(),
                    text: text.to_owned(),
                    color: color.to_owned(),
                    by: by.to_owned(),
                    updated_at,
                    rev,
                    removed: false,
                    created_at: parse_ts(&ts(now))?,
                })
            }
        };
        tx.commit()?;
        Ok(outcome)
    }

    /// 메모를 뗀다(묘비로 남긴다: 뗀 판과 새 `updated_at`). 없거나 이미 뗀 메모는 바뀌는 것이 없고 `false`다.
    pub fn remove_note(&self, id: &str, now: DateTime<Utc>) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let changed = match self.note(id)? {
            Some(cur) if !cur.removed => {
                let rev = self.bump_notes_rev(now)?;
                self.conn.execute(
                    "UPDATE notes SET removed = 1, updated_at = ?2, rev = ?3 WHERE id = ?1",
                    params![id, next_stamp(now, Some(&cur.updated_at)), rev],
                )?;
                true
            }
            _ => false,
        };
        tx.commit()?;
        Ok(changed)
    }

    /// `before`보다 앞서 뗀 묘비를 지운다(판은 그대로다). 지운 수.
    pub fn purge_removed_notes(&self, before: DateTime<Utc>) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM notes WHERE removed = 1 AND updated_at < ?1",
            [stamp(before)],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    fn saved(p: NotePut) -> Note {
        match p {
            NotePut::Saved(n) => n,
            other => panic!("저장되어야 한다: {other:?}"),
        }
    }

    #[test]
    fn create_then_update_with_the_right_base() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.notes_rev().unwrap(), 0);
        let a = saved(
            store
                .put_note("n-aaaaaa", "우유 사기", "yellow", "user", None, at(9, 0))
                .unwrap(),
        );
        assert_eq!((a.rev, a.by.as_str(), a.removed), (1, "user", false));
        assert_eq!(store.notes_rev().unwrap(), 1);
        assert_eq!(store.note("n-aaaaaa").unwrap().unwrap(), a);

        let b = saved(
            store
                .put_note(
                    "n-aaaaaa",
                    "우유와 빵 사기",
                    "pink",
                    "lucia",
                    Some(&a.updated_at),
                    at(9, 5),
                )
                .unwrap(),
        );
        assert_eq!(
            (b.rev, b.by.as_str(), b.color.as_str()),
            (2, "lucia", "pink")
        );
        assert_ne!(a.updated_at, b.updated_at);
        assert_eq!(b.created_at, a.created_at, "만든 때는 그대로");
        assert_eq!(store.live_note_count().unwrap(), 1);
    }

    #[test]
    fn a_wrong_or_missing_base_is_a_conflict_that_returns_the_current_note() {
        let store = Store::open_in_memory().unwrap();
        let a = saved(
            store
                .put_note("n-aaaaaa", "하나", "yellow", "user", None, at(9, 0))
                .unwrap(),
        );
        let b = saved(
            store
                .put_note(
                    "n-aaaaaa",
                    "둘",
                    "yellow",
                    "user",
                    Some(&a.updated_at),
                    at(9, 1),
                )
                .unwrap(),
        );
        // 옛 표(a)로 올리면 충돌: 지금 것(b)을 돌려주고 아무것도 바꾸지 않는다.
        let rev_before = store.notes_rev().unwrap();
        for base in [Some(a.updated_at.as_str()), Some("엉뚱한 표"), None] {
            match store
                .put_note("n-aaaaaa", "셋", "blue", "user", base, at(9, 2))
                .unwrap()
            {
                NotePut::Conflict(cur) => assert_eq!(cur, b),
                other => panic!("충돌이어야 한다: {other:?}"),
            }
        }
        assert_eq!(
            store.notes_rev().unwrap(),
            rev_before,
            "충돌은 판을 올리지 않는다"
        );
        assert_eq!(store.note("n-aaaaaa").unwrap().unwrap().text, "둘");
        // 없는 메모에 base가 붙어 오면(그새 지워져 비워진 것) 만든다.
        let c = saved(
            store
                .put_note("n-gone01", "새로", "green", "user", Some("옛 표"), at(9, 3))
                .unwrap(),
        );
        assert_eq!(c.text, "새로");
    }

    #[test]
    fn updated_at_changes_on_every_edit_even_at_the_same_instant() {
        let store = Store::open_in_memory().unwrap();
        let now = at(9, 0);
        let mut last = saved(
            store
                .put_note("n-aaaaaa", "0", "yellow", "user", None, now)
                .unwrap(),
        );
        for i in 1..=5 {
            let next = saved(
                store
                    .put_note(
                        "n-aaaaaa",
                        &i.to_string(),
                        "yellow",
                        "user",
                        Some(&last.updated_at),
                        now,
                    )
                    .unwrap(),
            );
            assert_ne!(next.updated_at, last.updated_at, "{i}번째");
            assert!(next.updated_at > last.updated_at, "사전순으로도 커진다");
            last = next;
        }
        // 시계가 뒤로 가도 달라진다.
        let back = saved(
            store
                .put_note(
                    "n-aaaaaa",
                    "뒤로",
                    "yellow",
                    "user",
                    Some(&last.updated_at),
                    at(8, 0),
                )
                .unwrap(),
        );
        assert!(back.updated_at > last.updated_at);
        // 떼도 달라지므로, 떼기 전 표로는 되살리지 못한다.
        assert!(store.remove_note("n-aaaaaa", now).unwrap());
        let tomb = store.note("n-aaaaaa").unwrap().unwrap();
        assert_ne!(tomb.updated_at, back.updated_at);
    }

    #[test]
    fn removing_leaves_a_tombstone_that_since_returns() {
        let store = Store::open_in_memory().unwrap();
        let a = saved(
            store
                .put_note("n-aaaaaa", "하나", "yellow", "user", None, at(9, 0))
                .unwrap(),
        );
        saved(
            store
                .put_note("n-bbbbbb", "둘", "blue", "user", None, at(9, 1))
                .unwrap(),
        );
        let (rev, all) = store.notes_since(None).unwrap();
        assert_eq!((rev, all.len()), (2, 2));

        assert!(store.remove_note("n-aaaaaa", at(9, 2)).unwrap());
        assert_eq!(store.notes_rev().unwrap(), 3);
        let (rev, after) = store.notes_since(Some(2)).unwrap();
        assert_eq!(rev, 3);
        assert_eq!(after.len(), 1);
        assert_eq!(
            (after[0].id.as_str(), after[0].removed, after[0].rev),
            ("n-aaaaaa", true, 3)
        );
        assert_eq!(
            after[0].text, a.text,
            "묘비도 글을 지니지만 앱은 removed로 거른다"
        );
        // 전부 읽으면 묘비도 있다. 살아 있는 것만 세면 하나.
        assert_eq!(store.notes_since(None).unwrap().1.len(), 2);
        assert_eq!(store.live_note_count().unwrap(), 1);
        assert_eq!(store.live_notes().unwrap().len(), 1);
        // 최신 판 뒤로는 바뀐 것이 없다.
        assert!(store.notes_since(Some(3)).unwrap().1.is_empty());

        // 없는 것이나 이미 뗀 것은 바뀌는 것이 없다.
        assert!(!store.remove_note("n-zzzzzz", at(9, 3)).unwrap());
        assert!(!store.remove_note("n-aaaaaa", at(9, 3)).unwrap());
        assert_eq!(store.notes_rev().unwrap(), 3);
    }

    #[test]
    fn a_tombstone_is_a_conflict_without_the_base_and_revives_with_it() {
        let store = Store::open_in_memory().unwrap();
        saved(
            store
                .put_note("n-aaaaaa", "하나", "yellow", "user", None, at(9, 0))
                .unwrap(),
        );
        store.remove_note("n-aaaaaa", at(9, 1)).unwrap();
        let tomb = store.note("n-aaaaaa").unwrap().unwrap();
        assert!(tomb.removed);
        match store
            .put_note("n-aaaaaa", "다시", "yellow", "user", None, at(9, 2))
            .unwrap()
        {
            NotePut::Conflict(cur) => assert!(cur.removed, "묘비도 removed로 돌려준다"),
            other => panic!("{other:?}"),
        }
        let back = saved(
            store
                .put_note(
                    "n-aaaaaa",
                    "다시",
                    "yellow",
                    "user",
                    Some(&tomb.updated_at),
                    at(9, 3),
                )
                .unwrap(),
        );
        assert!(!back.removed);
        assert_eq!(back.rev, 3);
        assert_eq!(store.live_note_count().unwrap(), 1);
        assert!(!store.note("n-aaaaaa").unwrap().unwrap().removed);
    }

    #[test]
    fn at_most_forty_live_notes_and_reviving_counts() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..MAX_NOTES {
            saved(
                store
                    .put_note(
                        &format!("n-{i:06}"),
                        "메모",
                        "white",
                        "user",
                        None,
                        at(9, 0),
                    )
                    .unwrap(),
            );
        }
        assert_eq!(store.live_note_count().unwrap(), MAX_NOTES);
        let rev = store.notes_rev().unwrap();
        assert_eq!(
            store
                .put_note("n-extra01", "하나 더", "white", "user", None, at(9, 1))
                .unwrap(),
            NotePut::Full
        );
        assert_eq!(store.notes_rev().unwrap(), rev, "못 만들면 판도 그대로");
        assert!(store.note("n-extra01").unwrap().is_none());
        // 있는 것을 고치는 것은 된다.
        let first = store.note("n-000000").unwrap().unwrap();
        saved(
            store
                .put_note(
                    "n-000000",
                    "고침",
                    "white",
                    "user",
                    Some(&first.updated_at),
                    at(9, 2),
                )
                .unwrap(),
        );
        // 하나 떼면 새로 만들 수 있고, 그러면 뗀 것은 되살릴 수 없다(다시 40개).
        store.remove_note("n-000001", at(9, 3)).unwrap();
        let tomb = store.note("n-000001").unwrap().unwrap();
        saved(
            store
                .put_note("n-extra01", "하나 더", "white", "user", None, at(9, 4))
                .unwrap(),
        );
        assert_eq!(
            store
                .put_note(
                    "n-000001",
                    "되살림",
                    "white",
                    "user",
                    Some(&tomb.updated_at),
                    at(9, 5)
                )
                .unwrap(),
            NotePut::Full
        );
        assert!(store.note("n-000001").unwrap().unwrap().removed);
    }

    #[test]
    fn purging_tombstones_keeps_the_revision() {
        let store = Store::open_in_memory().unwrap();
        saved(
            store
                .put_note("n-aaaaaa", "하나", "yellow", "user", None, at(9, 0))
                .unwrap(),
        );
        saved(
            store
                .put_note("n-bbbbbb", "둘", "yellow", "user", None, at(9, 0))
                .unwrap(),
        );
        store.remove_note("n-aaaaaa", at(10, 0)).unwrap();
        store.remove_note("n-bbbbbb", at(12, 0)).unwrap();
        assert_eq!(store.notes_rev().unwrap(), 4);
        // 11시 앞서 뗀 것만 지운다.
        assert_eq!(store.purge_removed_notes(at(11, 0)).unwrap(), 1);
        assert!(store.note("n-aaaaaa").unwrap().is_none());
        assert!(store.note("n-bbbbbb").unwrap().is_some());
        assert_eq!(store.notes_rev().unwrap(), 4, "판은 내려가지 않는다");
        // 살아 있는 메모는 지우지 않는다.
        saved(
            store
                .put_note("n-cccccc", "셋", "yellow", "user", None, at(1, 0))
                .unwrap(),
        );
        assert_eq!(store.purge_removed_notes(at(23, 0)).unwrap(), 1);
        assert!(store.note("n-cccccc").unwrap().is_some());
        // 지운 id로 다시 만들면 판이 이어서 오른다.
        let again = saved(
            store
                .put_note("n-aaaaaa", "새 하나", "yellow", "user", None, at(13, 0))
                .unwrap(),
        );
        assert_eq!(again.rev, 6);
    }
}
