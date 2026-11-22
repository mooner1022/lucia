//! 메모를 데몬에 두기(결정 0066, 앱 메서드 `notes.list`·`notes.put`·`notes.remove`와 번호 없는 `notes` 프레임).
//! 범위가 필요 없다(메모는 소유자의 것이다). 앱이 올린 고침의 `by`는 `user`다.
//!
//! 메모의 판(`rev`)이 바뀔 때마다 연결된 모든 앱에 `{"type": "notes", "rev": N}`을 보낸다. 바꾼 앱에도 간다
//! (앱은 제 고침이 이미 반영됐음을 판으로 알아본다).

use std::sync::Arc;

use serde_json::{Value, json};

use super::Gateway;
use super::methods::{AppMethod, CallContext, CallError, CallFuture};
use crate::notes::{By, NoteError, Notes, Put, view};

/// 모든 메모 메서드를 등록하고, 판이 바뀌면 앱에 알리는 일을 띄운다. 한 번만 부른다([`Gateway::set_notes`]).
pub(super) fn register(gw: &Arc<Gateway>, notes: Arc<Notes>) {
    gw.register(Arc::new(NotesList(notes.clone())));
    gw.register(Arc::new(NotesPut(notes.clone())));
    gw.register(Arc::new(NotesRemove(notes.clone())));
    let mut changes = notes.subscribe();
    let gw = Arc::downgrade(gw);
    // 구독은 여기서 한다(띄운 일이 처음 돌기 전의 바뀜도 놓치지 않는다). 처음 판은 앱이 `notes.list`로 읽는다.
    tokio::spawn(async move {
        while changes.changed().await.is_ok() {
            let rev = *changes.borrow_and_update();
            let Some(gw) = gw.upgrade() else { return };
            gw.broadcast(json!({"type": "notes", "rev": rev}));
        }
    });
}

fn bad(why: &str) -> CallError {
    CallError::new("bad_params", why)
}

fn failed(e: NoteError) -> CallError {
    match e {
        NoteError::Invalid(why) => CallError::owned("bad_params", &why),
        NoteError::Missing => CallError::new("bad_params", "그런 메모가 없다"),
        NoteError::Store(why) => CallError::owned("failed", &why),
    }
}

/// 글 인자: 문자열이어야 한다(없거나 다른 종류면 `bad_params`).
fn string<'a>(params: &'a Value, key: &str) -> Result<&'a str, CallError> {
    params[key]
        .as_str()
        .ok_or_else(|| bad(&format!("{key}는 문자열이어야 한다")))
}

/// `notes.list {since?}`: `since`(판) 뒤에 바뀐 메모. 뗀 메모도 `removed: true`로 온다(30일).
struct NotesList(Arc<Notes>);

impl AppMethod for NotesList {
    fn name(&self) -> &'static str {
        "notes.list"
    }

    fn call<'a>(&'a self, _ctx: CallContext<'a>, params: Value) -> CallFuture<'a> {
        Box::pin(async move {
            let since = match &params["since"] {
                Value::Null => None,
                v => Some(v.as_i64().ok_or_else(|| bad("since는 정수여야 한다"))?),
            };
            let (rev, notes) = self.0.list(since).map_err(failed)?;
            Ok(json!({"rev": rev, "notes": notes.iter().map(view).collect::<Vec<_>>()}))
        })
    }
}

/// `notes.put {id, text, color, base?}`: 만들거나 고친다. `base`가 맞지 않으면 오류가 아니라
/// `{"conflict": true, "note": 지금 것}`으로 답한다.
struct NotesPut(Arc<Notes>);

impl AppMethod for NotesPut {
    fn name(&self) -> &'static str {
        "notes.put"
    }

    fn call<'a>(&'a self, _ctx: CallContext<'a>, params: Value) -> CallFuture<'a> {
        Box::pin(async move {
            let id = string(&params, "id")?;
            let text = string(&params, "text")?;
            let color = string(&params, "color")?;
            let base = match &params["base"] {
                Value::Null => None,
                v => Some(v.as_str().ok_or_else(|| bad("base는 문자열이어야 한다"))?),
            };
            match self
                .0
                .put(id, text, color, base, By::User)
                .map_err(failed)?
            {
                Put::Saved { note, rev } => {
                    Ok(json!({"conflict": false, "note": view(&note), "rev": rev}))
                }
                Put::Conflict(note) => Ok(json!({"conflict": true, "note": view(&note)})),
            }
        })
    }
}

/// `notes.remove {id}`: 뗀다. 모르는 `id`도 `ok`다.
struct NotesRemove(Arc<Notes>);

impl AppMethod for NotesRemove {
    fn name(&self) -> &'static str {
        "notes.remove"
    }

    fn call<'a>(&'a self, _ctx: CallContext<'a>, params: Value) -> CallFuture<'a> {
        Box::pin(async move {
            let id = string(&params, "id")?;
            let (_, rev) = self.0.remove(id).map_err(failed)?;
            Ok(json!({"ok": true, "rev": rev}))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use super::super::methods::dispatch;
    use super::*;
    use lucia_store::Store;

    fn setup() -> (Arc<Gateway>, lucia_store::Client, Arc<Notes>) {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        store
            .lock()
            .unwrap()
            .create_client("cl-1", "시험 앱", "app", "hash", chrono::Utc::now())
            .unwrap();
        let client = store
            .lock()
            .unwrap()
            .active_client("cl-1")
            .unwrap()
            .unwrap();
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let (ptx, _prx) = tokio::sync::mpsc::channel(8);
        let gw = Gateway::new(store.clone(), itx, ptx);
        let notes = Notes::new(store);
        gw.set_notes(notes.clone());
        (gw, client, notes)
    }

    async fn call(
        gw: &Arc<Gateway>,
        client: &lucia_store::Client,
        method: &str,
        params: Value,
    ) -> Result<Value, CallError> {
        dispatch(gw, client, method, params).await
    }

    /// 프레임 하나를 기다린다(없으면 `None`).
    async fn next(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Value>) -> Option<Value> {
        tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .ok()
            .flatten()
    }

    #[tokio::test]
    async fn the_methods_are_listed_for_every_app_without_a_scope() {
        let (gw, client, _) = setup();
        let names = gw.method_names(&client);
        for name in ["notes.list", "notes.put", "notes.remove"] {
            assert!(names.contains(&name), "{name}");
        }
    }

    #[tokio::test]
    async fn put_creates_updates_and_answers_conflicts_as_data() {
        let (gw, client, _) = setup();
        let made = call(
            &gw,
            &client,
            "notes.put",
            json!({"id": "n-7a89a834b9", "text": "우유 사기", "color": "pink"}),
        )
        .await
        .unwrap();
        assert_eq!(made["conflict"], false);
        assert_eq!(made["rev"], 1);
        let note = &made["note"];
        assert_eq!(note["id"], "n-7a89a834b9");
        assert_eq!(note["by"], "user");
        assert_eq!(note["removed"], false);
        assert_eq!(note["color"], "pink");
        let token = note["updated_at"].as_str().unwrap().to_owned();

        // 맞는 base로 고친다: 표가 달라진다.
        let edited = call(
            &gw,
            &client,
            "notes.put",
            json!({"id": "n-7a89a834b9", "text": "우유와 빵", "color": "magenta", "base": token}),
        )
        .await
        .unwrap();
        assert_eq!(edited["conflict"], false);
        assert_eq!(edited["rev"], 2);
        assert_eq!(edited["note"]["color"], "yellow", "모르는 색은 노랑");
        assert_ne!(edited["note"]["updated_at"], token.as_str());

        // 옛 base, base 없음: 충돌은 답이다(오류가 아니다). 지금 것을 준다.
        for base in [json!({"base": token}), json!({})] {
            let mut p = json!({"id": "n-7a89a834b9", "text": "덮어쓰기", "color": "blue"});
            p.as_object_mut()
                .unwrap()
                .extend(base.as_object().unwrap().clone());
            let r = call(&gw, &client, "notes.put", p).await.unwrap();
            assert_eq!(r["conflict"], true, "{r}");
            assert_eq!(r["note"]["text"], "우유와 빵");
            assert!(r.get("rev").is_none());
        }

        // 뗀 뒤: list는 묘비를 removed로 준다. base 없이 올리면 충돌, 맞는 base면 되살린다.
        let gone = call(&gw, &client, "notes.remove", json!({"id": "n-7a89a834b9"}))
            .await
            .unwrap();
        assert_eq!(gone, json!({"ok": true, "rev": 3}));
        let list = call(&gw, &client, "notes.list", json!({"since": 2}))
            .await
            .unwrap();
        assert_eq!(list["rev"], 3);
        assert_eq!(list["notes"][0]["removed"], true);
        let tomb = list["notes"][0]["updated_at"].as_str().unwrap().to_owned();
        let r = call(
            &gw,
            &client,
            "notes.put",
            json!({"id": "n-7a89a834b9", "text": "다시", "color": "white"}),
        )
        .await
        .unwrap();
        assert_eq!(
            (r["conflict"].as_bool(), r["note"]["removed"].as_bool()),
            (Some(true), Some(true))
        );
        let r = call(
            &gw,
            &client,
            "notes.put",
            json!({"id": "n-7a89a834b9", "text": "다시", "color": "white", "base": tomb}),
        )
        .await
        .unwrap();
        assert_eq!(
            (r["conflict"].as_bool(), r["rev"].as_i64()),
            (Some(false), Some(4))
        );
        assert_eq!(r["note"]["removed"], false);
    }

    #[tokio::test]
    async fn list_since_and_remove_of_an_unknown_id() {
        let (gw, client, _) = setup();
        let r = call(&gw, &client, "notes.list", Value::Null).await.unwrap();
        assert_eq!(r, json!({"rev": 0, "notes": []}));
        for (i, id) in ["n-aaaaaa", "n-bbbbbb"].into_iter().enumerate() {
            call(
                &gw,
                &client,
                "notes.put",
                json!({"id": id, "text": format!("메모 {i}"), "color": "green"}),
            )
            .await
            .unwrap();
        }
        let all = call(&gw, &client, "notes.list", json!({})).await.unwrap();
        assert_eq!(all["notes"].as_array().unwrap().len(), 2);
        let some = call(&gw, &client, "notes.list", json!({"since": 1}))
            .await
            .unwrap();
        assert_eq!(some["notes"].as_array().unwrap().len(), 1);
        assert_eq!(some["notes"][0]["id"], "n-bbbbbb");
        // 모르는 id를 떼도 ok. 판은 그대로다.
        let r = call(&gw, &client, "notes.remove", json!({"id": "n-zzzzzz"}))
            .await
            .unwrap();
        assert_eq!(r, json!({"ok": true, "rev": 2}));
    }

    #[tokio::test]
    async fn bad_params_are_refused_and_change_nothing() {
        let (gw, client, notes) = setup();
        let code = |r: Result<Value, CallError>| r.unwrap_err().code;
        let put = |p: Value| {
            let (gw, client) = (gw.clone(), client.clone());
            async move { call(&gw, &client, "notes.put", p).await }
        };
        let ok = json!({"id": "n-abc123", "text": "글", "color": "pink"});
        for p in [
            json!({"text": "글", "color": "pink"}),
            json!({"id": 7, "text": "글", "color": "pink"}),
            json!({"id": "n-abc123", "color": "pink"}),
            json!({"id": "n-abc123", "text": 5, "color": "pink"}),
            json!({"id": "n-abc123", "text": "글"}),
            json!({"id": "n-abc123", "text": "글", "color": 3}),
            json!({"id": "n-abc123", "text": "글", "color": "pink", "base": 9}),
            // 아이디 모양
            json!({"id": "abc123", "text": "글", "color": "pink"}),
            json!({"id": "n-ab", "text": "글", "color": "pink"}),
            json!({"id": "n-ABCDEF", "text": "글", "color": "pink"}),
            json!({"id": "n-0123456789abcdefg", "text": "글", "color": "pink"}),
            // 글
            json!({"id": "n-abc123", "text": "", "color": "pink"}),
            json!({"id": "n-abc123", "text": "   ", "color": "pink"}),
            json!({"id": "n-abc123", "text": "가".repeat(2001), "color": "pink"}),
        ] {
            assert_eq!(code(put(p.clone()).await), "bad_params", "{p}");
        }
        for p in [json!({"since": "1"}), json!({"since": 1.5})] {
            assert_eq!(
                code(call(&gw, &client, "notes.list", p.clone()).await),
                "bad_params",
                "{p}"
            );
        }
        for p in [json!({}), json!({"id": 5}), Value::Null] {
            assert_eq!(
                code(call(&gw, &client, "notes.remove", p.clone()).await),
                "bad_params",
                "{p}"
            );
        }
        assert_eq!(
            notes.list(None).unwrap(),
            (0, Vec::new()),
            "아무것도 바꾸지 않았다"
        );
        // 2000자는 된다(글자 수로 센다).
        let mut full = ok.clone();
        full["text"] = json!("가".repeat(2000));
        assert_eq!(put(full).await.unwrap()["conflict"], false);
    }

    #[tokio::test]
    async fn the_forty_first_note_is_refused() {
        let (gw, client, notes) = setup();
        for i in 0..40 {
            let r = call(
                &gw,
                &client,
                "notes.put",
                json!({"id": format!("n-{i:06}"), "text": "메모", "color": "white"}),
            )
            .await
            .unwrap();
            assert_eq!(r["conflict"], false);
        }
        let r = call(
            &gw,
            &client,
            "notes.put",
            json!({"id": "n-extra01", "text": "메모", "color": "white"}),
        )
        .await;
        assert_eq!(r.unwrap_err().code, "bad_params");
        assert_eq!(notes.list(None).unwrap().1.len(), 40);
    }

    #[tokio::test]
    async fn a_change_is_broadcast_as_a_notes_frame() {
        let (gw, client, notes) = setup();
        let (_, mut rx) = gw.attach_for_test("cl-1");
        let r = call(
            &gw,
            &client,
            "notes.put",
            json!({"id": "n-abc123", "text": "글", "color": "blue"}),
        )
        .await
        .unwrap();
        assert_eq!(r["rev"], 1);
        assert_eq!(
            next(&mut rx).await.unwrap(),
            json!({"type": "notes", "rev": 1})
        );

        // 충돌과 잘못된 인자는 알리지 않는다.
        let _ = call(
            &gw,
            &client,
            "notes.put",
            json!({"id": "n-abc123", "text": "다른 글", "color": "blue"}),
        )
        .await
        .unwrap();
        let _ = call(&gw, &client, "notes.put", json!({"id": "n-abc123"})).await;
        assert!(next(&mut rx).await.is_none());

        // Lucia(도구)가 고친 것도, 뗀 것도 알린다.
        let lucia = notes.add("루시아의 메모", None).unwrap();
        assert_eq!(
            next(&mut rx).await.unwrap(),
            json!({"type": "notes", "rev": 2})
        );
        call(&gw, &client, "notes.remove", json!({"id": lucia.id}))
            .await
            .unwrap();
        assert_eq!(
            next(&mut rx).await.unwrap(),
            json!({"type": "notes", "rev": 3})
        );
        // 없는 것을 떼면 알릴 것이 없다.
        call(&gw, &client, "notes.remove", json!({"id": lucia.id}))
            .await
            .unwrap();
        assert!(next(&mut rx).await.is_none());
    }
}
