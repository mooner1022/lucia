//! 데스크톱 앱이 낸 확장(결정 0066)의 시험.

use super::*;

use axum::body::Body;
use tower::ServiceExt as _;

/// HTTP 요청 하나(집 안 주소에서 온 것으로). (상태, 본문).
pub(super) async fn http(gw: &Arc<Gateway>, req: axum::http::Request<Body>) -> (u16, Vec<u8>) {
    let mut req = req;
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
    let r = gw.router().oneshot(req).await.unwrap();
    let status = r.status().as_u16();
    let body = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, body)
}

pub(super) fn get(path: &str, token: &str) -> axum::http::Request<Body> {
    axum::http::Request::get(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

pub(super) fn json_of(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap()
}

fn tools(list: Value) -> Vec<AppTool> {
    clean_tools(list.as_array().unwrap().clone())
}

/// 앱 기능의 `when`과 `timeout`(결정 0066): 정리, 부를 수 있는 때, invoke에 싣는 기다림.
#[tokio::test]
async fn tool_when_and_timeout_are_cleaned_and_enforced() {
    let list = tools(json!([
        {"name": "screen.capture", "description": "화면", "when": "here", "timeout": 90},
        {"name": "clipboard.get", "description": "클립보드", "when": "attended"},
        {"name": "odd.thing", "description": "모르는 값", "when": "someday", "timeout": 999},
        {"name": "notes.list", "description": "읽기"},
        {"name": "alarm.set", "description": "쓰기", "effect": "write", "timeout": 0.2},
    ]));
    let by = |n: &str| list.iter().find(|t| t.name == n).unwrap().clone();
    assert_eq!(by("screen.capture").when, When::Here);
    assert_eq!(by("screen.capture").timeout, Duration::from_secs(90));
    assert_eq!(by("clipboard.get").when, When::Attended);
    assert_eq!(by("odd.thing").when, When::Here, "모르는 값은 가장 좁게");
    assert_eq!(by("odd.thing").timeout, APP_CALL_TIMEOUT_MAX, "120초까지");
    assert_eq!(by("notes.list").when, When::Any);
    assert_eq!(by("notes.list").timeout, APP_CALL_TIMEOUT);
    assert_eq!(by("alarm.set").timeout, Duration::from_secs(1));

    let here = by("screen.capture");
    let own = conversation("cl-desk").0;
    let other = conversation("cl-phone").0;
    let ok = |t: &AppTool, human: bool, chat: Option<&str>| {
        allowed_now(t, "cl-desk", AppCaller { human, chat }).is_ok()
    };
    assert!(ok(&here, true, Some(&own)));
    assert!(!ok(&here, true, Some(&other)), "다른 대화에서 말하는 중");
    assert!(
        !ok(&here, true, None),
        "대화가 어디인지 모르면 부르지 않는다"
    );
    assert!(!ok(&here, false, Some(&own)));
    let attended = by("clipboard.get");
    assert!(ok(&attended, true, Some(&other)) && !ok(&attended, false, None));
    let read = by("notes.list");
    assert!(ok(&read, false, None), "읽기는 사람 없는 실행에서도");
    let write = by("alarm.set");
    assert!(ok(&write, true, None) && !ok(&write, false, None));
}

/// 늦게 답하는 앱 기능(결정 0066): 도구는 45초만 기다리고 부탁 ID를 돌려주고, 데몬은 그 기능의 시간까지 기다린다.
/// 답에 실은 파일은 그 앱이 올린 것만 받는다. 여러 앱이 내주면 `when`을 거른 앱을 부른다.
#[tokio::test(start_paused = true)]
async fn slow_app_calls_continue_in_the_daemon_and_carry_files() {
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    issue(&st, "cl-desk", "app");
    issue(&st, "cl-phone", "app");
    for id in ["cl-desk", "cl-phone"] {
        st.lock()
            .unwrap()
            .set_client_scopes(id, &[scopes::APP_TOOLS.to_owned()])
            .unwrap();
    }
    let offer =
        json!([{"name": "screen.capture", "description": "화면", "when": "here", "timeout": 90}]);
    let (_, mut desk) = gw.attach_with("cl-desk", vec!["screen".into()], tools(offer.clone()));
    let (_, mut phone) = gw.attach_with("cl-phone", vec!["screen".into()], tools(offer));
    let files = gw.files.get().unwrap().clone();
    let now = Utc::now();
    let mine = files
        .save(
            "in",
            "화면.png",
            b"\x89PNG\r\n\x1a\nxx",
            Some("app:cl-desk"),
            now,
        )
        .unwrap();
    let theirs = files
        .save("in", "남의.txt", b"hi", Some("app:cl-phone"), now)
        .unwrap();

    // 데스크톱 대화에서 말하는 중: 휴대폰이 아니라 데스크톱을 부른다.
    let chat = conversation("cl-desk").0;
    let gw2 = gw.clone();
    let task = tokio::spawn(async move {
        gw2.call_app(
            "screen.capture",
            json!({}),
            None,
            AppCaller {
                human: true,
                chat: Some(&chat),
            },
        )
        .await
    });
    let ask = desk.recv().await.unwrap();
    assert_eq!(
        (ask["type"].as_str(), ask["timeout"].as_u64()),
        (Some("invoke"), Some(90))
    );
    assert!(phone.try_recv().is_err(), "다른 대화의 앱은 부르지 않는다");
    // 45초가 지나도 답이 없다: 도구는 부탁 ID를 돌려준다.
    let first = task.await.unwrap().unwrap();
    let AppCall::Pending { id, left, .. } = first else {
        panic!("기다리는 중이어야 한다: {first:?}")
    };
    assert_eq!(id, ask["id"].as_str().unwrap());
    assert!(
        left <= Duration::from_secs(45) && left > Duration::from_secs(40),
        "{left:?}"
    );
    // 사용자가 허락했다: 그림과 남의 파일을 함께 실었다.
    assert!(gw.resolve(
        "cl-desk",
        &id,
        Ok((
            json!({"width": 2560}),
            vec![mine.id.clone(), theirs.id.clone(), "f_000000000000".into()]
        ))
    ));
    let AppCall::Done(reply) = gw.app_result(&id).await.unwrap() else {
        panic!("답이 왔다")
    };
    assert_eq!(reply.data["width"], 2560);
    assert_eq!(
        reply
            .files
            .iter()
            .map(|f| f.id.as_str())
            .collect::<Vec<_>>(),
        [mine.id.as_str()]
    );
    assert_eq!(reply.dropped_files, 2);
    assert_eq!(gw.app_result("d-nope").await.unwrap_err().code, "not_found");

    // 끝내 답이 없으면 그 기능의 시간(90초) 뒤 timeout(거절인 denied와 다르다).
    let chat = conversation("cl-desk").0;
    let gw3 = gw.clone();
    let task = tokio::spawn(async move {
        gw3.call_app(
            "screen.capture",
            json!({}),
            Some("cl-desk"),
            AppCaller {
                human: true,
                chat: Some(&chat),
            },
        )
        .await
    });
    let _ = desk.recv().await.unwrap();
    let AppCall::Pending { id, .. } = task.await.unwrap().unwrap() else {
        panic!("기다리는 중")
    };
    assert_eq!(gw.app_result(&id).await.unwrap_err().code, "timeout");
    // 다른 대화에서 부르면 거절한다.
    let err = gw
        .call_app(
            "screen.capture",
            json!({}),
            Some("cl-desk"),
            AppCaller {
                human: true,
                chat: Some("memory:dm"),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "forbidden");
}

/// `presence: away`, `desk`, `message.context`, 새 능력 표시(결정 0066).
#[tokio::test]
async fn away_desk_and_context_frames_over_websocket() {
    let st = store();
    let (gw, mut irx, _prx) = gateway(st.clone());
    let token = issue(&st, "cl-desk", "app");
    let addr = serve(&gw).await;
    let mut ws = connect(addr).await;
    let w = hello(&mut ws, json!({"type": "hello", "token": token})).await;
    for cap in [
        "tool_when",
        "tool_files",
        "tool_timeout",
        "short_files",
        "desk",
        "presence_away",
        "message_context",
        "seen",
        "owner_files",
    ] {
        assert_eq!(w["capabilities"][cap], true, "{cap}");
    }
    assert_eq!(w["capabilities"]["history_shape"], 2);
    assert!(w["capabilities"].get("voice").is_none(), "받아쓰기는 없다");

    say(&mut ws, json!({"type": "presence", "state": "away"})).await;
    say(&mut ws, json!({"type": "presence", "state": "asleep"})).await;
    assert_eq!(hear(&mut ws).await["code"], "bad_frame");
    assert_eq!(gw.presence("cl-desk"), Some(Presence::Away));

    // 화면 보여 주기를 켰다: 상태 알림과 활동 기록에 남고, 목록에 보인다.
    let conv = conversation("cl-desk").0;
    let _ = take_status_notes(&conv);
    let until = (Utc::now() + chrono::Duration::minutes(15)).to_rfc3339();
    say(
        &mut ws,
        json!({"type": "desk", "showing": true, "controlling": false, "until": until}),
    )
    .await;
    say(&mut ws, json!({"type": "ping"})).await;
    assert_eq!(hear(&mut ws).await["type"], "pong");
    let notes = take_status_notes(&conv);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("화면 보여 주기를 켰다") && notes[0].contains("UTC까지"));
    let desks = gw.desks();
    assert_eq!(desks.len(), 1);
    assert_eq!(desks[0].2.at(Utc::now()), (true, false));
    // 같은 상태를 다시 보내도 남기지 않는다. 끝나는 때가 지났으면 꺼진 것이다.
    say(
        &mut ws,
        json!({"type": "desk", "showing": true, "until": until}),
    )
    .await;
    say(&mut ws, json!({"type": "ping"})).await;
    let _ = hear(&mut ws).await;
    assert!(take_status_notes(&conv).is_empty());
    assert_eq!(
        desks[0].2.at(Utc::now() + chrono::Duration::minutes(16)),
        (false, false)
    );
    // 끝나는 때가 지난 상태를 알리면 꺼짐으로 한 번 남긴다(그때야 Lucia가 안다). 다시 켜면 켬으로 남긴다.
    let past = (Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
    say(
        &mut ws,
        json!({"type": "desk", "showing": true, "until": past}),
    )
    .await;
    say(&mut ws, json!({"type": "ping"})).await;
    let _ = hear(&mut ws).await;
    assert!(
        take_status_notes(&conv)
            .iter()
            .any(|n| n.contains("꺼졌다"))
    );
    say(&mut ws, json!({"type": "desk", "showing": false})).await;
    say(&mut ws, json!({"type": "ping"})).await;
    let _ = hear(&mut ws).await;
    assert!(take_status_notes(&conv).is_empty(), "꺼짐은 한 번만 남긴다");
    say(
        &mut ws,
        json!({"type": "desk", "showing": true, "until": until}),
    )
    .await;
    say(&mut ws, json!({"type": "ping"})).await;
    let _ = hear(&mut ws).await;
    assert!(take_status_notes(&conv).iter().any(|n| n.contains("켰다")));

    // 고른 글: 말과 따로, 4000자로 잘리고 출처는 한 줄로.
    say(
        &mut ws,
        json!({"type": "message", "text": "이거 번역해 줘", "id": "m1",
               "context": {"selection": format!("  {}  ", "가".repeat(5000)),
                           "source": {"app": "msedge.exe", "title": "제목\n둘째 줄"}}}),
    )
    .await;
    assert_eq!(hear(&mut ws).await["type"], "accepted");
    let msg = irx.recv().await.unwrap();
    assert_eq!(msg.body, "이거 번역해 줘");
    let ctx = msg.context.unwrap();
    assert_eq!(ctx.selection.chars().count(), 4000);
    assert_eq!(ctx.source_app.as_deref(), Some("msedge.exe"));
    assert_eq!(ctx.source_title.as_deref(), Some("제목 둘째 줄"));
    say(
        &mut ws,
        json!({"type": "message", "text": "빈 맥락", "context": {"selection": "   "}}),
    )
    .await;
    assert_eq!(hear(&mut ws).await["type"], "accepted");
    assert!(irx.recv().await.unwrap().context.is_none());

    // 끊기면 켜져 있던 화면 보여 주기는 꺼진 것으로 남긴다.
    drop(ws);
    for _ in 0..200 {
        if !gw.online("cl-desk") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let notes = take_status_notes(&conv);
    assert!(notes.iter().any(|n| n.contains("연결이 끊겨")), "{notes:?}");
}

/// 기록의 새 모양과 다른 대화의 파일(결정 0066): `shape: 2`일 때만 `parts`·`files`를 싣고 설명 줄을 뺀다.
/// 범위 `owner_files`가 있어야 Discord DM의 파일을 받는다.
#[tokio::test]
async fn history_shape_two_and_owner_files() {
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    let token = issue(&st, "cl-desk", "app");
    let now = Utc::now();
    st.lock()
        .unwrap()
        .set_setting(lucia_store::keys::NOTIFY_CONVERSATION, "memory:dm", now)
        .unwrap();
    let files = gw.files.get().unwrap().clone();
    // Lucia가 만든 파일(대화 없음)을 앱 대화로 보냈다. DM에는 사용자가 붙인 파일이 있다.
    let made = files.save("out", "정리.md", b"# hi", None, now).unwrap();
    let dm_file = files
        .save("in", "영수증.txt", b"receipt", Some("memory:dm"), now)
        .unwrap();
    let gone = files.save("out", "옛.md", b"old", None, now).unwrap();
    let episode = |conv: &str, body: &str| {
        st.lock()
            .unwrap()
            .add_episode(&lucia_store::NewEpisode {
                at: now,
                kind: lucia_store::EpisodeKind::AgentReport,
                origin: lucia_store::Origin::Agent,
                conversation: Some(conv.into()),
                session_id: None,
                run_id: None,
                body: body.into(),
            })
            .unwrap()
    };
    let app = conversation("cl-desk").0;
    let reply = episode(
        &app,
        "정리해서 드릴게요.\n\n한 장으로 줄였어요.\n[첨부: 정리.md, 옛.md]",
    );
    let dm = episode("memory:dm", "받았어요.\n[첨부: 영수증.txt]");
    let shape = |id: i64, conv: &str, parts: &[&str], f: &[&lucia_store::FileRecord]| {
        st.lock()
            .unwrap()
            .set_episode_shape(
                id,
                Some(conv),
                &parts.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
                &f.iter()
                    .map(|r| lucia_store::EpisodeFile::from(*r))
                    .collect::<Vec<_>>(),
                now,
            )
            .unwrap()
    };
    shape(
        reply,
        &app,
        &["정리해서 드릴게요.", "한 장으로 줄였어요."],
        &[&made, &gone],
    );
    shape(dm, "memory:dm", &[], &[&dm_file]);
    // 보관 기간이 지나 지운 파일.
    st.lock().unwrap().delete_file(&gone.id).unwrap();

    // 예전 모양: 그대로.
    let (status, body) = http(&gw, get("/v1/history?scope=owner", &token)).await;
    assert_eq!(status, 200);
    let old = json_of(&body);
    assert!(old["items"][0].get("parts").is_none() && old["items"][0].get("files").is_none());
    assert!(
        old["items"][0]["body"]
            .as_str()
            .unwrap()
            .ends_with("[첨부: 정리.md, 옛.md]")
    );

    let (status, body) = http(&gw, get("/v1/history?scope=owner&shape=2", &token)).await;
    assert_eq!(status, 200);
    let v = json_of(&body);
    let items = v["items"].as_array().unwrap();
    let first = &items[0];
    assert_eq!(
        first["parts"],
        json!(["정리해서 드릴게요.", "한 장으로 줄였어요."])
    );
    assert_eq!(first["body"], "정리해서 드릴게요.\n\n한 장으로 줄였어요.");
    assert_eq!(first["files"][0]["id"], made.id);
    assert_eq!(first["files"][0]["reachable"], true, "그 대화로 보낸 파일");
    assert_eq!(first["files"][1]["name"], "옛.md");
    assert_eq!(first["files"][1]["gone"], true);
    let second = &items[1];
    assert!(second.get("parts").is_none(), "한 조각이면 뺀다");
    assert_eq!(second["files"][0]["reachable"], false, "범위가 없다");
    assert_eq!(http(&gw, get("/v1/history?shape=3", &token)).await.0, 400);

    // 받기: 앱 대화로 보낸 파일은 받고, DM의 파일은 범위가 있어야 받는다.
    let path = |f: &lucia_store::FileRecord| format!("/v1/files/{}", f.id);
    assert_eq!(http(&gw, get(&path(&made), &token)).await.0, 200);
    assert_eq!(http(&gw, get(&path(&dm_file), &token)).await.0, 404);
    st.lock()
        .unwrap()
        .set_client_scopes("cl-desk", &[scopes::OWNER_FILES.to_owned()])
        .unwrap();
    assert_eq!(http(&gw, get(&path(&dm_file), &token)).await.0, 200);
    let v = json_of(
        &http(&gw, get("/v1/history?scope=owner&shape=2", &token))
            .await
            .1,
    );
    assert_eq!(v["items"][1]["files"][0]["reachable"], true);
    // 범위가 있어도 서버 채널 같은 남의 대화 파일은 받지 않는다.
    let elsewhere = files
        .save("in", "남의.txt", b"x", Some("memory:guild-room"), now)
        .unwrap();
    assert_eq!(http(&gw, get(&path(&elsewhere), &token)).await.0, 404);
    // 받아쓰기는 없다.
    let voice = axum::http::Request::post("/v1/voice")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(vec![0u8; 10]))
        .unwrap();
    assert_eq!(http(&gw, voice).await.0, 501);
}

/// 짧게 두는 올리기(결정 0066): 지우는 시각을 알리고, 최근 파일·보관함에서 빠지고, 시각이 지나면 없는 파일이다.
#[tokio::test]
async fn short_uploads_expire_and_stay_out_of_lists() {
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    let token = issue(&st, "cl-desk", "app");
    let up = |q: &str| {
        axum::http::Request::post(format!("/v1/files?name=screen.png{q}"))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(b"\x89PNG\r\n\x1a\nxx".to_vec()))
            .unwrap()
    };
    let (status, body) = http(&gw, up("&keep=short")).await;
    assert_eq!(status, 201);
    let short = json_of(&body);
    let at = chrono::DateTime::parse_from_rfc3339(short["expires_at"].as_str().unwrap()).unwrap();
    let left = at.with_timezone(&Utc) - Utc::now();
    assert!(left > chrono::Duration::minutes(59) && left <= chrono::Duration::hours(1));
    let (_, body) = http(&gw, up("")).await;
    let normal = json_of(&body);
    assert!(normal.get("expires_at").is_none());
    assert_eq!(http(&gw, up("&keep=forever")).await.0, 400);

    let files = gw.files.get().unwrap().clone();
    let id = short["id"].as_str().unwrap();
    let recent: Vec<String> = files.recent(10).into_iter().map(|f| f.id).collect();
    assert!(
        !recent.contains(&id.to_owned())
            && recent.contains(&normal["id"].as_str().unwrap().to_owned())
    );
    assert!(
        files.keep(id, "", "", Utc::now()).is_err(),
        "보관하지 않는다"
    );
    // 앱은 제 사본을 그동안 받을 수 있다.
    assert_eq!(
        http(&gw, get(&format!("/v1/files/{id}"), &token)).await.0,
        200
    );
    // 시각이 지났다: 정리 전이어도 없는 파일이고, 정리하면 지워진다.
    st.lock()
        .unwrap()
        .set_file_expiry(id, Utc::now() - chrono::Duration::seconds(1))
        .unwrap();
    assert!(files.get(id).is_none());
    assert_eq!(
        http(&gw, get(&format!("/v1/files/{id}"), &token)).await.0,
        404
    );
    assert_eq!(files.prune_short(Utc::now()), 1);
    assert!(st.lock().unwrap().file(id).unwrap().is_none());
}

/// 앱 둘(푸시가 없는 데스크톱, 푸시가 있는 휴대폰)을 알림 받을 곳 "앱 모두"로 둔다.
pub(super) fn two_apps() -> (
    Arc<Mutex<Store>>,
    Arc<Gateway>,
    mpsc::UnboundedReceiver<PushJob>,
) {
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    issue(&st, "cl-desk", "app");
    issue(&st, "cl-phone", "app");
    let (push_tx, push_rx) = mpsc::unbounded_channel();
    gw.set_push(push_tx);
    gw.set_push_ready(true);
    let now = Utc::now();
    let s = st.lock().unwrap();
    s.set_client_push("cl-phone", Some("tok"), Some("android"), now)
        .unwrap();
    s.set_setting(keys::NOTIFY_CLIENT, ALL_APPS, now).unwrap();
    drop(s);
    (st, gw, push_rx)
}

pub(super) fn conn_id(gw: &Gateway, client: &str) -> u64 {
    gw.conns.lock().unwrap().get(client).unwrap().id
}

pub(super) fn boxed(st: &Arc<Mutex<Store>>, client: &str) -> Vec<Value> {
    st.lock()
        .unwrap()
        .outbox_after(client, 0, 100)
        .unwrap()
        .into_iter()
        .map(|f| {
            // 상자에는 번호 없이 두고 다시 보낼 때 붙인다.
            let mut v: Value = serde_json::from_str(&f.frame).unwrap();
            v["seq"] = json!(f.seq);
            v
        })
        .collect()
}

pub(super) fn text(to: &str, body: &str) -> Outbound {
    Outbound::Text {
        conversation: ConversationId(to.into()),
        body: body.into(),
    }
}

/// 앱 모두(결정 0066): 앱마다 상자에 넣고 한 곳만 울린다. 데스크톱에 사람이 있으면 데스크톱이 울리고, 보지 않은 채
/// 자리를 뜨면 휴대폰으로 넘긴다. 한 곳에서 보면 다른 곳에 알린다. 앱 대화의 카드는 어느 앱에서나 누른다(결정 0067).
#[tokio::test(start_paused = true)]
async fn all_apps_ring_where_the_person_is_and_forward_what_was_not_seen() {
    let (st, gw, mut pushes) = two_apps();
    assert_eq!(
        st.lock().unwrap().notify_conversation().as_deref(),
        Some("app:*")
    );
    assert!(gw.reachable(ALL_APPS), "휴대폰은 푸시로 닿는다");
    let (_, mut desk) = gw.attach_for_test("cl-desk");
    gw.set_presence("cl-desk", conn_id(&gw, "cl-desk"), Presence::Background);

    // 데스크톱에 사람이 있다(창은 뒤): 데스크톱이 울리고, 휴대폰 상자에는 울리지 않게 둔다.
    gw.send(text("app:*", "회의 10분 전이에요")).await.unwrap();
    let f = desk.recv().await.unwrap();
    assert_eq!(f["body"], "회의 10분 전이에요");
    assert!(f.get("alert").is_none());
    let phone_box = boxed(&st, "cl-phone");
    assert_eq!(phone_box[0]["alert"], false);
    assert!(pushes.try_recv().is_err(), "휴대폰은 아직 울리지 않는다");
    // 데스크톱은 아직 `seen`을 말하지 않았지만 푸시가 없는 앱이다: 2분이 지나면 휴대폰으로 넘긴다(결정 0067: 늦게
    // 한 번 더 울리는 것이 아무 데도 닿지 않는 것보다 낫다).
    tokio::time::sleep(alerts::TEXT_GRACE + Duration::from_secs(1)).await;
    let job = pushes.try_recv().unwrap();
    assert_eq!(
        (job.kind.as_str(), job.seq),
        ("text", phone_box[0]["seq"].as_i64().unwrap())
    );
    assert!(pushes.try_recv().is_err(), "한 번만 넘긴다");

    // 데스크톱이 봤다고 알린다: 휴대폰(끊김)에는 알림을 지우는 푸시.
    gw.seen("cl-desk", f["seq"].as_i64().unwrap());
    let clear = pushes.try_recv().unwrap();
    assert_eq!(
        (clear.kind.as_str(), clear.seq),
        ("clear", phone_box[0]["seq"].as_i64().unwrap())
    );

    // 이제 데스크톱은 `seen`을 말한다: 보지 않은 카드는 20초 뒤 휴대폰으로 넘긴다.
    let r = gw
        .send_card(card(ConversationId("app:*".into())))
        .await
        .unwrap();
    assert!(r.starts_with("app:*:"), "{r}");
    let shown = desk.recv().await.unwrap();
    assert_eq!(shown["ref"], r);
    assert!(pushes.try_recv().is_err());
    tokio::time::sleep(alerts::CARD_GRACE + Duration::from_secs(1)).await;
    let job = pushes.try_recv().unwrap();
    assert_eq!((job.kind.as_str(), job.high), ("card", true));
    // 같은 카드는 어느 앱에서나 누를 수 있다(참조가 같다). 앱 대화는 하나라(결정 0067) 한 앱의 대화 카드도 그렇다.
    assert!(ref_shown_to(&r, "cl-phone") && ref_shown_to(&r, "cl-desk"));
    assert!(ref_shown_to("app:cl-desk:abc", "cl-phone"));
    assert!(
        !ref_shown_to("discord:123:abc", "cl-phone"),
        "앱 대화가 아니다"
    );
    gw.close_card(&r, &card(ConversationId("app:*".into())))
        .await
        .unwrap();
    assert_eq!(desk.recv().await.unwrap()["type"], "card_closed");
    let phone_box = boxed(&st, "cl-phone");
    let closed = phone_box.last().unwrap();
    assert_eq!(closed["type"], "card_closed");
    assert!(closed.get("alert").is_none(), "닫힘은 울리지 않는다");
    assert!(pushes.try_recv().is_err());

    // 글이 온 뒤 데스크톱이 자리를 비우면 기다리지 않고 바로 넘긴다.
    gw.send(text("app:*", "택배가 왔어요")).await.unwrap();
    let _ = desk.recv().await.unwrap();
    gw.set_presence("cl-desk", conn_id(&gw, "cl-desk"), Presence::Away);
    gw.left_desk("cl-desk");
    let job = pushes.try_recv().unwrap();
    assert_eq!(job.kind, "text");
    // 휴대폰에서 봤다(앱을 열어 누름): 데스크톱에 그 번호까지 봤다고 알린다.
    let phone_last = boxed(&st, "cl-phone").last().unwrap()["seq"]
        .as_i64()
        .unwrap();
    gw.seen("cl-phone", phone_last);
    let told = desk.recv().await.unwrap();
    assert_eq!(told["type"], "seen");

    // 사람이 없는 데스크톱(자리 비움)에 오면 처음부터 휴대폰이 울리고 데스크톱은 놓아만 둔다.
    gw.send(text("app:*", "자리 비운 동안")).await.unwrap();
    assert_eq!(desk.recv().await.unwrap()["alert"], false);
    assert_eq!(pushes.try_recv().unwrap().kind, "text");

    // 휴대폰이 앞에 있으면 휴대폰을 울린다(푸시 없이), 데스크톱은 놓아만 둔다.
    let (_, mut phone) = gw.attach_for_test("cl-phone");
    gw.send(text("app:*", "앞에 있는 휴대폰")).await.unwrap();
    assert!(phone.recv().await.unwrap().get("alert").is_none());
    assert_eq!(desk.recv().await.unwrap()["alert"], false);
    assert!(pushes.try_recv().is_err());
    // 연결된 앱이 모두 받는 입력 중 표시.
    gw.typing(&ConversationId("app:*".into())).await.unwrap();
    assert_eq!(phone.recv().await.unwrap()["type"], "typing");
    assert_eq!(desk.recv().await.unwrap()["type"], "typing");
}

/// 데스크톱 대화의 답(결정 0067): 처음부터 휴대폰 상자에도 들어 있다(울리지 않는 `alert: false`, 어느 대화의
/// 답인지 `conversation`을 실어). 데스크톱이 보지 않은 채 2분이 지나거나 자리를 뜨면 휴대폰은 프레임을 복사받지 않고
/// 이미 있는 그 번호에 대한 `alert{seq}`와 푸시만 받는다(0066의 복사 넘기기는 없다). 붙은 파일은 휴대폰에서도 받는다.
#[tokio::test(start_paused = true)]
async fn replies_in_a_desktop_conversation_wait_in_the_phone_box_and_only_the_alert_follows() {
    let (st, gw, mut pushes) = two_apps();
    let (desk_conn, mut desk) = gw.attach_for_test("cl-desk");
    let (phone_conn, mut phone) = gw.attach_for_test("cl-phone");
    gw.set_presence("cl-phone", phone_conn, Presence::Background);
    // 데스크톱은 `seen`을 말하는 앱이다(때를 기다려 넘기는 것은 그런 앱이 울린 것만).
    gw.seen("cl-desk", 0);
    let made = gw
        .files
        .get()
        .unwrap()
        .save("out", "정리.md", b"# hi", None, Utc::now())
        .unwrap();
    gw.send(text("app:cl-desk", "긴 일의 답이에요"))
        .await
        .unwrap();
    gw.send(Outbound::Files {
        conversation: ConversationId("app:cl-desk".into()),
        body: "파일로 드려요".into(),
        files: vec![lucia_core::OutFile {
            name: made.name.clone(),
            bytes: b"# hi".to_vec(),
            id: Some(made.id.clone()),
        }],
    })
    .await
    .unwrap();
    // 데스크톱(물은 곳)은 울린다: `alert`도 `conversation`도 없는 제 대화의 프레임.
    for kind in ["text", "files"] {
        let f = desk.recv().await.unwrap();
        assert_eq!(f["type"], kind);
        assert!(f.get("alert").is_none() && f.get("conversation").is_none());
    }
    // 휴대폰 상자에는 처음부터 둘 다 있다: 울리지 않고, 데스크톱 대화의 답이라고 밝힌다. 같은 파일 ID.
    let phone_box = boxed(&st, "cl-phone");
    assert_eq!(phone_box.len(), 2);
    for f in &phone_box {
        assert_eq!(f["alert"], false);
        assert_eq!(f["conversation"], "app:cl-desk");
    }
    assert_eq!(phone_box[0]["body"], "긴 일의 답이에요");
    assert_eq!(phone_box[1]["files"][0]["id"], made.id, "사본 없이 같은 ID");
    // 연결된 휴대폰은 같은 프레임을 바로 받는다(번호는 휴대폰의 것).
    for (kind, stored) in ["text", "files"].into_iter().zip(&phone_box) {
        let live = phone.recv().await.unwrap();
        assert_eq!(
            (live["type"].as_str(), &live["seq"]),
            (Some(kind), &stored["seq"])
        );
        assert_eq!(live["alert"], false);
    }
    let phone_client = st
        .lock()
        .unwrap()
        .active_client("cl-phone")
        .unwrap()
        .unwrap();
    assert!(
        methods::can_download(&st.lock().unwrap(), &phone_client, &made),
        "휴대폰도 데스크톱 대화로 간 파일을 받는다"
    );
    assert!(
        pushes.try_recv().is_err(),
        "처음에는 휴대폰이 울리지 않는다"
    );

    // 데스크톱이 2분 동안 보지 않았다: 휴대폰에는 알림만 넘어온다(`alert{seq}`와 푸시, 같은 번호).
    tokio::time::sleep(alerts::TEXT_GRACE + Duration::from_secs(1)).await;
    for (kind, stored) in ["text", "files"].into_iter().zip(&phone_box) {
        let alert = phone.recv().await.unwrap();
        assert_eq!(alert, json!({"type": "alert", "seq": stored["seq"]}));
        let job = pushes.try_recv().unwrap();
        assert_eq!(
            (job.kind.as_str(), job.client_id.as_str()),
            (kind, "cl-phone")
        );
        assert_eq!(json!(job.seq), stored["seq"]);
    }
    assert!(pushes.try_recv().is_err() && phone.try_recv().is_err());
    assert_eq!(
        boxed(&st, "cl-phone").len(),
        2,
        "프레임을 복사해 넣지 않는다"
    );

    // 데스크톱이 자리를 뜨면(`away`) 기다리지 않고 바로 넘긴다.
    gw.send(text("app:cl-desk", "셋째 답")).await.unwrap();
    assert_eq!(desk.recv().await.unwrap()["body"], "셋째 답");
    let third = boxed(&st, "cl-phone")[2]["seq"].as_i64().unwrap();
    assert_eq!(phone.recv().await.unwrap()["alert"], false);
    gw.set_presence("cl-desk", desk_conn, Presence::Away);
    gw.left_desk("cl-desk");
    assert_eq!(
        phone.recv().await.unwrap(),
        json!({"type": "alert", "seq": third})
    );
    let job = pushes.try_recv().unwrap();
    assert_eq!((job.kind.as_str(), job.seq), ("text", third));

    // 연결이 끊겨도 마찬가지다(앞에 있던 데스크톱이 닫혔다).
    gw.set_presence("cl-desk", desk_conn, Presence::Foreground);
    gw.send(text("app:cl-desk", "넷째 답")).await.unwrap();
    assert_eq!(desk.recv().await.unwrap()["body"], "넷째 답");
    let fourth = boxed(&st, "cl-phone")[3]["seq"].as_i64().unwrap();
    assert_eq!(phone.recv().await.unwrap()["alert"], false);
    gw.detach("cl-desk", desk_conn);
    assert_eq!(
        phone.recv().await.unwrap(),
        json!({"type": "alert", "seq": fourth})
    );
    assert_eq!(pushes.try_recv().unwrap().seq, fourth);
}

/// 데몬이 다시 뜬 뒤처럼 데스크톱이 아직 `seen`을 보내지 않았어도, 그 데스크톱(푸시 없는 앱)이 울린 카드는 20초가
/// 지나면 휴대폰으로 넘긴다(결정 0067). 그 뒤 `away`가 와도 다시 넘기지 않는다. 처음부터 `away`가 오면 바로 넘긴다.
#[tokio::test(start_paused = true)]
async fn a_card_rung_on_a_quiet_desktop_is_pushed_after_the_grace_or_at_once_when_it_goes_away() {
    let (st, gw, mut pushes) = two_apps();
    let (conn, mut desk) = gw.attach_for_test("cl-desk");
    gw.set_presence("cl-desk", conn, Presence::Background);
    let r = gw
        .send_card(card(ConversationId("app:*".into())))
        .await
        .unwrap();
    assert_eq!(desk.recv().await.unwrap()["ref"], r);
    tokio::time::sleep(alerts::CARD_GRACE * 2).await;
    let job = pushes.try_recv().unwrap();
    assert_eq!(
        (job.kind.as_str(), job.high),
        ("card", true),
        "`seen`을 말하지 않는 데스크톱이 울린 카드도 때가 되면 넘긴다"
    );
    // 이미 넘긴 것은 `away`에 다시 넘기지 않는다. 새 카드는 `away`에 바로 넘긴다.
    let r2 = gw
        .send_card(card(ConversationId("app:*".into())))
        .await
        .unwrap();
    assert_eq!(desk.recv().await.unwrap()["ref"], r2);
    let client = st
        .lock()
        .unwrap()
        .active_client("cl-desk")
        .unwrap()
        .unwrap();
    let mut budget = EventBudget(VecDeque::new());
    let reply = handle_frame(
        &gw,
        &client,
        conn,
        &conversation("cl-desk"),
        &mut budget,
        r#"{"type": "presence", "state": "away"}"#,
    )
    .await;
    assert!(reply.is_none());
    let job = pushes.try_recv().unwrap();
    assert_eq!((job.kind.as_str(), job.high), ("card", true));
    assert_eq!(
        job.seq,
        boxed(&st, "cl-phone")[1]["seq"].as_i64().unwrap(),
        "새 카드"
    );
    assert!(pushes.try_recv().is_err(), "넘긴 카드는 다시 넘기지 않는다");
    // 휴대폰이 그 카드를 누르면 데스크톱에 봤다고 알린다.
    let phone_seq = boxed(&st, "cl-phone")[0]["seq"].as_i64().unwrap();
    gw.seen_through_last("cl-phone");
    assert_eq!(phone_seq, 1);
    assert_eq!(desk.recv().await.unwrap()["type"], "seen");
}

/// 닿는 앱이 없으면 앱 모두여도 Discord DM으로 간다(지금과 같다).
#[tokio::test]
async fn all_apps_without_any_reachable_app_fall_back_to_the_dm() {
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    issue(&st, "cl-desk", "app");
    let dm = ConversationId("memory:dm".into());
    {
        let s = st.lock().unwrap();
        s.set_setting(keys::NOTIFY_CLIENT, ALL_APPS, Utc::now())
            .unwrap();
        s.set_setting(lucia_store::keys::NOTIFY_CONVERSATION, &dm.0, Utc::now())
            .unwrap();
    }
    let (primary, handle) = lucia_channel::memory::MemoryChannel::new(2000, mpsc::channel(1).0);
    let router = RouterChannel::new(Arc::new(primary), gw.clone(), st.clone(), None);
    router.send(text("app:*", "아무도 없을 때")).await.unwrap();
    assert_eq!(handle.take_sent()[0].0, dm);
    assert!(boxed(&st, "cl-desk").is_empty());
    // 앱이 붙으면 앱으로.
    let (_, mut rx) = gw.attach_for_test("cl-desk");
    router.send(text("app:*", "붙은 뒤")).await.unwrap();
    assert_eq!(rx.recv().await.unwrap()["body"], "붙은 뒤");
    assert!(router.ready());
    assert!(!notify_app_can_handover(&st.lock().unwrap()));
}
