//! 메모(결정 0066): 앱의 스티커 메모를 데몬이 함께 둔다. 앱 메서드(`notes.*`, `gateway::notes`)와 Lucia의 도구
//! (`note_*`)가 같은 서비스를 거친다. 모양 확인은 여기서 하고, 충돌·개수 상한·판은 저장소가 한 트랜잭션에서
//! 가린다([`lucia_store::Store::put_note`]).
//!
//! - 판(`rev`)이 바뀔 때마다 새 판을 알린다([`Notes::subscribe`]). 접속 API가 연결된 앱에 `notes` 프레임을 보낸다.
//! - 앱이 앱 없이도 메모를 쓰게, Lucia의 도구는 앱이 연결되어 있지 않아도 된다.
//! - 메모의 글은 사람이 붙여 넣은 것일 수 있다. Lucia에게 줄 때는 외부 데이터로 감싼다(`mcp::note_list`).

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use lucia_store::{MAX_NOTES, Note, NotePut, Store};
use serde_json::{Value, json};
use tokio::sync::watch;

/// 글 길이 상한(문자).
pub const TEXT_MAX: usize = 2000;
/// 뗀 메모(묘비)를 두는 기간(일). 그동안은 다른 기기가 뗀 것을 알 수 있게 `removed`로 준다.
pub const KEEP_DAYS: i64 = 30;
/// 쓸 수 있는 색. 모르는 색은 첫째로 본다.
pub const COLORS: [&str; 5] = ["yellow", "pink", "blue", "green", "white"];
/// 아이디 만드는 쪽이 정하는 모양: `n-` 뒤에 소문자·숫자 6~16자.
const ID_MIN: usize = 6;
const ID_MAX: usize = 16;

/// 누가 고쳤는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    User,
    Lucia,
}

impl By {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Lucia => "lucia",
        }
    }
}

/// 메모 서비스의 오류.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteError {
    /// 받을 수 없는 값(사람이 읽을 까닭). 앱에는 `bad_params`.
    Invalid(String),
    /// 그런 메모가 없다(Lucia가 없는 메모를 고치려 했다).
    Missing,
    /// 저장소 오류. 앱에는 `failed`.
    Store(String),
}

impl std::fmt::Display for NoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(why) | Self::Store(why) => f.write_str(why),
            Self::Missing => f.write_str("그런 메모가 없다"),
        }
    }
}

impl From<lucia_store::StoreError> for NoteError {
    fn from(e: lucia_store::StoreError) -> Self {
        Self::Store(e.to_string())
    }
}

/// [`Notes::put`]의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Put {
    /// 저장했다. `rev`는 이 바꿈이 만든 판이다.
    Saved { note: Note, rev: i64 },
    /// `base`가 맞지 않아 받지 않았다. 지금 메모를 돌려준다.
    Conflict(Note),
}

/// Lucia가 메모를 고치는 요청: `text`는 바꾸고, `append`는 끝에 새 줄로 붙인다(둘 다면 바꾼 뒤 붙인다).
#[derive(Debug, Clone, Default)]
pub struct Edit {
    pub text: Option<String>,
    pub append: Option<String>,
    pub color: Option<String>,
}

pub struct Notes {
    store: Arc<Mutex<Store>>,
    /// 지금까지 알린 가장 큰 판.
    rev: watch::Sender<i64>,
}

/// 앱에 주는 메모의 모양.
pub fn view(note: &Note) -> Value {
    json!({
        "id": note.id,
        "text": note.text,
        "color": note.color,
        "updated_at": note.updated_at,
        "by": note.by,
        "removed": note.removed,
    })
}

/// 아이디가 `n-` + 소문자·숫자 6~16자인가.
pub fn valid_id(id: &str) -> bool {
    id.strip_prefix("n-").is_some_and(|rest| {
        (ID_MIN..=ID_MAX).contains(&rest.len())
            && rest
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// 글은 비어 있으면 안 되고(공백만이어도) [`TEXT_MAX`]자 이하여야 한다. 고쳐 쓰지 않는다.
fn check_text(text: &str) -> Result<(), NoteError> {
    if text.trim().is_empty() {
        return Err(NoteError::Invalid("메모 글이 비어 있다".into()));
    }
    let len = text.chars().count();
    if len > TEXT_MAX {
        return Err(NoteError::Invalid(format!(
            "메모 글이 너무 길다({len}자, 최대 {TEXT_MAX}자)"
        )));
    }
    Ok(())
}

/// 모르는 색은 노랑으로 본다.
pub fn color_of(color: &str) -> &'static str {
    COLORS
        .iter()
        .find(|c| **c == color.trim())
        .copied()
        .unwrap_or(COLORS[0])
}

impl Notes {
    pub fn new(store: Arc<Mutex<Store>>) -> Arc<Self> {
        let rev = store
            .lock()
            .ok()
            .and_then(|s| s.notes_rev().ok())
            .unwrap_or(0);
        Arc::new(Self {
            store,
            rev: watch::channel(rev).0,
        })
    }

    /// 판이 바뀔 때마다 새 판을 받는다.
    pub fn subscribe(&self) -> watch::Receiver<i64> {
        self.rev.subscribe()
    }

    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().expect("store lock")
    }

    /// 판을 알린다(내려가지 않는다). 저장소 잠금을 쥔 채 부르므로 순서가 뒤바뀌지 않는다.
    fn publish(&self, rev: i64) {
        self.rev.send_if_modified(|v| {
            let newer = rev > *v;
            if newer {
                *v = rev;
            }
            newer
        });
    }

    /// 지금의 판과, `since` 뒤에 바뀐 메모(묘비 포함). `since`가 없으면 전부.
    pub fn list(&self, since: Option<i64>) -> Result<(i64, Vec<Note>), NoteError> {
        Ok(self.store().notes_since(since)?)
    }

    /// 살아 있는 메모(만든 순).
    pub fn live(&self) -> Result<Vec<Note>, NoteError> {
        Ok(self.store().live_notes()?)
    }

    /// 만들거나 고친다(앱의 `notes.put`, `by`는 호출한 쪽). 충돌은 오류가 아니라 [`Put::Conflict`]다.
    pub fn put(
        &self,
        id: &str,
        text: &str,
        color: &str,
        base: Option<&str>,
        by: By,
    ) -> Result<Put, NoteError> {
        if !valid_id(id) {
            return Err(NoteError::Invalid("id는 n- 뒤에 소문자·숫자 6~16자".into()));
        }
        check_text(text)?;
        let store = self.store();
        self.put_locked(&store, id, text, color_of(color), base, by)
    }

    fn put_locked(
        &self,
        store: &Store,
        id: &str,
        text: &str,
        color: &str,
        base: Option<&str>,
        by: By,
    ) -> Result<Put, NoteError> {
        match store.put_note(id, text, color, by.as_str(), base, Utc::now())? {
            NotePut::Saved(note) => {
                self.publish(note.rev);
                let rev = note.rev;
                Ok(Put::Saved { note, rev })
            }
            NotePut::Conflict(note) => Ok(Put::Conflict(note)),
            NotePut::Full => Err(NoteError::Invalid(format!(
                "메모는 {MAX_NOTES}개까지다. 하나를 떼고 다시 한다"
            ))),
        }
    }

    /// Lucia가 새 메모를 만든다(아이디는 데몬이 `n-` + 16진 10자로 짓는다).
    pub fn add(&self, text: &str, color: Option<&str>) -> Result<Note, NoteError> {
        check_text(text)?;
        let color = color_of(color.unwrap_or(""));
        let store = self.store();
        for _ in 0..5 {
            let id = format!("n-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
            if let Put::Saved { note, .. } =
                self.put_locked(&store, &id, text, color, None, By::Lucia)?
            {
                return Ok(note);
            }
        }
        Err(NoteError::Store("아이디를 짓지 못했다".into()))
    }

    /// Lucia가 있는 메모를 고친다. 읽고 쓰는 동안 저장소를 쥐므로 앱의 고침과 부딪히지 않는다.
    /// 없거나 뗀 메모는 [`NoteError::Missing`].
    pub fn update(&self, id: &str, edit: &Edit) -> Result<Note, NoteError> {
        let store = self.store();
        let current = store
            .note(id)?
            .filter(|n| !n.removed)
            .ok_or(NoteError::Missing)?;
        let mut text = edit.text.clone().unwrap_or_else(|| current.text.clone());
        if let Some(more) = &edit.append {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(more);
        }
        check_text(&text)?;
        let color = match edit.color.as_deref() {
            Some(c) => color_of(c),
            None => current.color.as_str(),
        };
        match self.put_locked(
            &store,
            id,
            &text,
            color,
            Some(&current.updated_at),
            By::Lucia,
        )? {
            Put::Saved { note, .. } => Ok(note),
            Put::Conflict(_) => Err(NoteError::Store("메모가 그새 바뀌었다".into())),
        }
    }

    /// 뗀다(앱의 `notes.remove`, Lucia의 `note_remove`). 없는 id도 오류가 아니다. 돌려주는 값은 (바꿨는가, 지금의 판).
    pub fn remove(&self, id: &str) -> Result<(bool, i64), NoteError> {
        let store = self.store();
        let changed = store.remove_note(id, Utc::now())?;
        let rev = store.notes_rev()?;
        if changed {
            self.publish(rev);
        }
        Ok((changed, rev))
    }

    /// 뗀 지 [`KEEP_DAYS`]일이 지난 묘비를 지운다. 판은 내려가지 않는다. 지운 수.
    pub fn purge(&self, now: DateTime<Utc>) -> usize {
        match self
            .store()
            .purge_removed_notes(now - Duration::days(KEEP_DAYS))
        {
            Ok(n) => n,
            Err(err) => {
                tracing::warn!(%err, "뗀 메모를 지우지 못했다");
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes() -> Arc<Notes> {
        Notes::new(Arc::new(Mutex::new(Store::open_in_memory().unwrap())))
    }

    fn saved(p: Put) -> Note {
        match p {
            Put::Saved { note, .. } => note,
            other => panic!("저장되어야 한다: {other:?}"),
        }
    }

    #[test]
    fn id_text_and_color_are_checked() {
        for ok in ["n-abc123", "n-7a89a834b9", "n-0123456789abcdef"] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in [
            "",
            "n-",
            "n-abc12",
            "n-0123456789abcdefg",
            "N-abc123",
            "x-abc123",
            "n-ABC123",
            "n-abc 123",
            "n-abc_123",
            "abc123",
        ] {
            assert!(!valid_id(bad), "{bad}");
        }
        assert_eq!(color_of("pink"), "pink");
        assert_eq!(color_of("magenta"), "yellow");
        assert_eq!(color_of(""), "yellow");

        let n = notes();
        let bad = |r: Result<Put, NoteError>| matches!(r, Err(NoteError::Invalid(_)));
        assert!(bad(n.put("n-abc123", "", "pink", None, By::User)));
        assert!(bad(n.put("n-abc123", " \n\t", "pink", None, By::User)));
        assert!(bad(n.put(
            "n-abc123",
            &"가".repeat(2001),
            "pink",
            None,
            By::User
        )));
        assert!(bad(n.put("nope", "글", "pink", None, By::User)));
        // 글자 수로 센다(바이트가 아니다). 앞뒤 공백은 그대로 둔다.
        let note = saved(
            n.put(
                "n-abc123",
                &format!(" {} ", "가".repeat(1998)),
                "magenta",
                None,
                By::User,
            )
            .unwrap(),
        );
        assert_eq!(note.text.chars().count(), 2000);
        assert!(note.text.starts_with(' '));
        assert_eq!(note.color, "yellow", "모르는 색은 노랑");
    }

    #[test]
    fn lucia_adds_updates_appends_and_removes() {
        let n = notes();
        let a = n.add("우유 사기", Some("green")).unwrap();
        assert!(valid_id(&a.id) && a.id.len() == 12, "{}", a.id);
        assert_eq!((a.by.as_str(), a.color.as_str()), ("lucia", "green"));

        let b = n
            .update(
                &a.id,
                &Edit {
                    append: Some("빵도".into()),
                    color: Some("blue".into()),
                    ..Edit::default()
                },
            )
            .unwrap();
        assert_eq!(b.text, "우유 사기\n빵도");
        assert_eq!(b.color, "blue");
        assert_ne!(b.updated_at, a.updated_at);
        let c = n
            .update(
                &a.id,
                &Edit {
                    text: Some("전부 새로".into()),
                    append: Some("덧붙임".into()),
                    ..Edit::default()
                },
            )
            .unwrap();
        assert_eq!(c.text, "전부 새로\n덧붙임");
        assert_eq!(c.color, "blue", "안 준 것은 그대로");

        // 너무 길어지면 받지 않는다.
        let long = Edit {
            append: Some("가".repeat(2000)),
            ..Edit::default()
        };
        assert!(matches!(n.update(&a.id, &long), Err(NoteError::Invalid(_))));
        assert!(matches!(
            n.update("n-nothere1", &Edit::default()),
            Err(NoteError::Missing)
        ));

        let (changed, rev) = n.remove(&a.id).unwrap();
        assert!(changed);
        assert_eq!(rev, 4);
        assert!(matches!(
            n.update(&a.id, &Edit::default()),
            Err(NoteError::Missing)
        ));
        assert_eq!(n.remove(&a.id).unwrap(), (false, 4), "뗀 것은 그대로");
        assert_eq!(n.remove("n-unknown1").unwrap(), (false, 4));
        assert!(n.live().unwrap().is_empty());
    }

    #[test]
    fn every_change_publishes_the_new_revision() {
        let n = notes();
        let mut rx = n.subscribe();
        assert_eq!(*rx.borrow_and_update(), 0);
        let a = saved(n.put("n-abc123", "하나", "pink", None, By::User).unwrap());
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), a.rev);
        // 충돌은 알리지 않는다.
        assert!(matches!(
            n.put("n-abc123", "둘", "pink", None, By::User).unwrap(),
            Put::Conflict(_)
        ));
        assert!(!rx.has_changed().unwrap());
        n.remove("n-abc123").unwrap();
        assert_eq!(*rx.borrow_and_update(), 2);
        // 없는 것을 떼도 알리지 않는다. 지워도 판은 그대로다.
        n.remove("n-abc123").unwrap();
        assert!(!rx.has_changed().unwrap());
        assert_eq!(n.purge(Utc::now() + Duration::days(31)), 1);
        assert_eq!(n.list(None).unwrap().0, 2);
        assert!(!rx.has_changed().unwrap());
        // 다시 시작해도(같은 저장소) 판을 이어 읽는다.
        assert_eq!(Notes::new(n.store.clone()).list(None).unwrap().0, 2);
        assert_eq!(*Notes::new(n.store.clone()).subscribe().borrow(), 2);
    }
}
