use super::*;

use lucia_channel::contract::{self, ContractHarness};
use lucia_core::{CardButton, CardField, ConversationId};
use tokio_tungstenite::tungstenite::Message as WsMessage;

fn store() -> Arc<Mutex<Store>> {
    Arc::new(Mutex::new(Store::open_in_memory().unwrap()))
}

fn gateway(
    store: Arc<Mutex<Store>>,
) -> (
    Arc<Gateway>,
    mpsc::Receiver<Inbound>,
    mpsc::Receiver<ButtonPress>,
) {
    let (itx, irx) = mpsc::channel(8);
    let (ptx, prx) = mpsc::channel(8);
    let _ = rustls::crypto::ring::default_provider().install_default();
    let gw = Gateway::new(store.clone(), itx, ptx);
    gw.set_files(Arc::new(crate::files::FileStore::new(
        std::env::temp_dir().join(format!("lucia-gw-{}", uuid::Uuid::new_v4().simple())),
        reqwest::Client::new(),
        store,
    )));
    (gw, irx, prx)
}

fn issue(store: &Arc<Mutex<Store>>, id: &str, role: &str) -> String {
    let token = new_token();
    store
        .lock()
        .unwrap()
        .create_client(id, "시험 앱", role, &token_hash(&token), Utc::now())
        .unwrap();
    token
}

fn card(conversation: ConversationId) -> Card {
    Card {
        conversation,
        headline: "확인해 주세요".into(),
        title: "메일 보내기".into(),
        fields: vec![CardField {
            name: "받는 사람".into(),
            value: "a@example.com".into(),
            inline: true,
        }],
        body: "본문".into(),
        footer: None,
        tone: CardTone::Pending,
        buttons: vec![CardButton {
            action: "approve:1".into(),
            label: "보내기".into(),
            primary: true,
            prompt: None,
            form: None,
        }],
    }
}

// ---------- 어댑터 공통 계약(명세 §운영 테스트) ----------

struct Harness {
    gw: Arc<Gateway>,
    rx: mpsc::UnboundedReceiver<Value>,
    client: String,
}

impl ContractHarness for Harness {
    type Adapter = Gateway;

    fn adapter(&self) -> &Gateway {
        &self.gw
    }

    async fn deliver_inbound(&mut self, _: &str, _: &str, _: &str) -> Inbound {
        unreachable!("들어오는 쪽은 실제 연결로 따로 시험한다")
    }

    fn take_sent(&mut self) -> Vec<(ConversationId, String)> {
        let mut out = Vec::new();
        while let Ok(v) = self.rx.try_recv() {
            if let Some(body) = v["body"].as_str() {
                out.push((conversation(&self.client), body.to_owned()));
            }
        }
        out
    }
}

#[tokio::test]
async fn gateway_passes_the_outbound_contract() {
    let (gw, _irx, _prx) = gateway(store());
    let (_, rx) = gw.attach("cl-1");
    let mut h = Harness {
        gw: gw.clone(),
        rx,
        client: "cl-1".into(),
    };
    let conv = conversation("cl-1");
    contract::capabilities_are_sane(&h);
    contract::long_messages_are_split_without_loss(&mut h, conv.clone()).await;
    contract::empty_messages_are_rejected(&mut h, conv.clone()).await;
    contract::notes_files_and_typing(&mut h, conv).await;
}

#[test]
fn ids_and_peers() {
    assert_eq!(conversation("cl-a").0, "app:cl-a");
    assert_eq!(client_of("app:cl-a"), Some("cl-a"));
    assert_eq!(client_of("app:cl-a:1234"), Some("cl-a"));
    assert_eq!(client_of("discord:123"), None);
    assert_eq!(client_of("app:"), None);
    for ok in [
        "127.0.0.1",
        "192.168.0.5",
        "10.1.2.3",
        "100.101.1.1",
        "fd00::1",
        "::1",
        "::ffff:192.168.1.1",
    ] {
        assert!(local_peer(ok.parse().unwrap()), "{ok}");
    }
    for bad in [
        "8.8.8.8",
        "100.128.0.1",
        "2001:4860::8888",
        "::ffff:1.1.1.1",
    ] {
        assert!(!local_peer(bad.parse().unwrap()), "{bad}");
    }
    assert!(new_token().starts_with("lucia_"));
    assert_ne!(new_token(), new_token());
}

// ---------- 라우터 ----------

#[tokio::test]
async fn router_sends_app_conversations_to_the_app_and_falls_back_when_offline() {
    let st = store();
    st.lock()
        .unwrap()
        .set_setting(
            lucia_store::keys::NOTIFY_CONVERSATION,
            "memory:dm",
            Utc::now(),
        )
        .unwrap();
    let (gw, _irx, _prx) = gateway(st.clone());
    let (itx, _irx2) = mpsc::channel(4);
    let (ptx, _prx2) = mpsc::channel(4);
    let (primary, handle) = lucia_channel::memory::MemoryChannel::with_buttons(2000, itx, ptx);
    let router = RouterChannel::new(Arc::new(primary), gw.clone(), st, None);

    // 연결된 앱: 앱으로.
    let (_, mut rx) = gw.attach("cl-1");
    router
        .send(Outbound::Text {
            conversation: conversation("cl-1"),
            body: "안녕".into(),
        })
        .await
        .unwrap();
    assert_eq!(rx.try_recv().unwrap()["body"], "안녕");
    let r = router.send_card(card(conversation("cl-1"))).await.unwrap();
    assert!(r.starts_with("app:cl-1:"), "{r}");
    let sent = rx.try_recv().unwrap();
    assert_eq!(sent["type"], "card");
    assert_eq!(sent["card"]["buttons"][0]["action"], "approve:1");
    router
        .close_card(&r, &card(conversation("cl-1")))
        .await
        .unwrap();
    assert_eq!(rx.try_recv().unwrap()["type"], "card_closed");
    assert!(handle.take_sent().is_empty(), "Discord로는 가지 않는다");
    assert!(handle.cards().is_empty());

    // 다른 대화: 기본 채널로.
    router
        .send(Outbound::Text {
            conversation: ConversationId("memory:dm".into()),
            body: "브리핑".into(),
        })
        .await
        .unwrap();
    assert_eq!(handle.take_sent().len(), 1);

    // 연결이 끊긴 앱: 알림 대화로 돌린다(글이 사라지지 않게).
    gw.disconnect("cl-1");
    router
        .send(Outbound::Text {
            conversation: conversation("cl-1"),
            body: "늦은 답".into(),
        })
        .await
        .unwrap();
    let sent = handle.take_sent();
    assert_eq!(sent.last().unwrap().0, ConversationId("memory:dm".into()));
    assert_eq!(sent.last().unwrap().1, "늦은 답");
    let r = router.send_card(card(conversation("cl-1"))).await.unwrap();
    assert!(!r.starts_with("app:"), "카드도 기본 채널로: {r}");
    assert_eq!(handle.cards().len(), 1);
    // 앱이 연결되지 않은 채로 앱 카드를 닫아도 오류가 아니다.
    router
        .close_card("app:cl-1:abc", &card(conversation("cl-1")))
        .await
        .unwrap();
    assert!(
        router
            .open_thread(&conversation("cl-1"), "x")
            .await
            .is_err(),
        "앱 대화에는 스레드를 열지 않는다"
    );
}

#[tokio::test]
async fn router_without_a_fallback_target_reports_closed() {
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    let (itx, _irx2) = mpsc::channel(4);
    let (primary, _handle) = lucia_channel::memory::MemoryChannel::new(2000, itx);
    let router = RouterChannel::new(Arc::new(primary), gw, st, None);
    let r = router
        .send(Outbound::Text {
            conversation: conversation("cl-x"),
            body: "x".into(),
        })
        .await;
    assert!(matches!(r, Err(ChannelError::Closed)));
}

// ---------- 실제 WebSocket ----------

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn serve(gw: &Arc<Gateway>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = gw
        .router()
        .into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

async fn connect(addr: SocketAddr) -> Ws {
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (ws, _) = tokio_tungstenite::client_async(format!("ws://{addr}/v1/ws"), tcp)
        .await
        .unwrap();
    ws
}

async fn say(ws: &mut Ws, v: Value) {
    ws.send(WsMessage::Text(v.to_string().into()))
        .await
        .unwrap();
}

async fn hear(ws: &mut Ws) -> Value {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("응답 시간 초과")
            .expect("연결이 닫혔다")
            .unwrap();
        if let WsMessage::Text(t) = m {
            return serde_json::from_str(t.as_str()).unwrap();
        }
    }
}

#[tokio::test]
async fn app_chats_and_presses_over_websocket() {
    let st = store();
    let token = issue(&st, "cl-1", "app");
    let (gw, mut irx, mut prx) = gateway(st.clone());
    let addr = serve(&gw).await;

    let mut ws = connect(addr).await;
    say(&mut ws, json!({"type": "hello", "token": token})).await;
    let welcome = hear(&mut ws).await;
    assert_eq!(welcome["type"], "welcome");
    assert_eq!(welcome["v"], PROTOCOL_VERSION);
    assert_eq!(welcome["conversation"], "app:cl-1");
    assert!(gw.online("cl-1"));
    assert!(
        st.lock().unwrap().clients().unwrap()[0]
            .last_seen_at
            .is_some()
    );

    say(
        &mut ws,
        json!({"type": "message", "text": " 오늘 일정 알려 줘 "}),
    )
    .await;
    assert_eq!(hear(&mut ws).await["type"], "accepted");
    let msg = irx.recv().await.unwrap();
    assert_eq!(msg.platform, PLATFORM);
    assert_eq!(msg.sender, "cl-1");
    assert_eq!(msg.conversation, conversation("cl-1"));
    assert_eq!(msg.body, "오늘 일정 알려 줘");
    // 코어의 허용 규칙: 발급한 앱 클라이언트는 받는다.
    assert!(
        crate::engine::access_allows(
            &crate::engine::Access::Owner,
            &st.lock().unwrap(),
            PLATFORM,
            "cl-1"
        )
        .unwrap()
    );

    // 데몬 → 앱: 답과 카드.
    gw.send(Outbound::Text {
        conversation: conversation("cl-1"),
        body: "오늘은 일정이 없습니다".into(),
    })
    .await
    .unwrap();
    assert_eq!(hear(&mut ws).await["body"], "오늘은 일정이 없습니다");
    let r = gw.send_card(card(conversation("cl-1"))).await.unwrap();
    let c = hear(&mut ws).await;
    assert_eq!(c["type"], "card");
    assert_eq!(c["ref"], r.as_str());

    // 버튼: 자기 카드만, 값은 비밀로.
    say(
        &mut ws,
        json!({"type": "press", "ref": r, "action": "approve:1", "input": "123456"}),
    )
    .await;
    let press = prx.recv().await.unwrap();
    assert_eq!(press.platform, PLATFORM);
    assert_eq!(press.sender, "cl-1");
    assert_eq!(press.message_ref, r);
    assert_eq!(press.input.unwrap().expose(), "123456");
    say(
        &mut ws,
        json!({"type": "press", "ref": "app:cl-2:x", "action": "approve:1"}),
    )
    .await;
    assert_eq!(hear(&mut ws).await["code"], "bad_ref");
    assert!(prx.try_recv().is_err());

    // 잘못된 프레임과 핑.
    say(&mut ws, json!({"type": "message", "text": "  "})).await;
    assert_eq!(hear(&mut ws).await["code"], "empty");
    say(&mut ws, json!({"type": "nope"})).await;
    assert_eq!(hear(&mut ws).await["code"], "bad_frame");
    say(&mut ws, json!({"type": "ping"})).await;
    assert_eq!(hear(&mut ws).await["type"], "pong");

    // 취소하면 끊고, 다음 말은 받지 않는다.
    st.lock()
        .unwrap()
        .revoke_client("cl-1", Utc::now())
        .unwrap();
    say(&mut ws, json!({"type": "message", "text": "또"})).await;
    assert_eq!(hear(&mut ws).await["code"], "unauthorized");
    assert!(irx.try_recv().is_err());
    assert!(
        !crate::engine::access_allows(
            &crate::engine::Access::Owner,
            &st.lock().unwrap(),
            PLATFORM,
            "cl-1"
        )
        .unwrap()
    );
}

#[tokio::test]
async fn bad_tokens_devices_and_late_hellos_are_refused() {
    let st = store();
    let device = issue(&st, "cl-d", "device");
    let (gw, mut irx, _prx) = gateway(st.clone());
    let addr = serve(&gw).await;

    // 첫 프레임이 hello가 아니면.
    let mut ws = connect(addr).await;
    say(&mut ws, json!({"type": "message", "text": "안녕"})).await;
    assert_eq!(hear(&mut ws).await["code"], "hello_required");

    // 틀린 토큰.
    let mut ws = connect(addr).await;
    say(&mut ws, json!({"type": "hello", "token": "lucia_wrong"})).await;
    assert_eq!(hear(&mut ws).await["code"], "unauthorized");

    // 기기 역할: 이벤트 입구가 없으면 받지 않는다. 허용 규칙도 사람으로 보지 않는다.
    let mut ws = connect(addr).await;
    say(&mut ws, json!({"type": "hello", "token": device})).await;
    assert_eq!(hear(&mut ws).await["code"], "unsupported_role");
    assert!(!gw.online("cl-d"));
    assert!(
        !crate::engine::access_allows(
            &crate::engine::Access::Owner,
            &st.lock().unwrap(),
            PLATFORM,
            "cl-d"
        )
        .unwrap()
    );
    assert!(irx.try_recv().is_err());

    // 실패가 쌓이면 그 주소를 막는다(맞는 토큰도).
    for _ in 0..MAX_FAILURES {
        let mut ws = connect(addr).await;
        say(&mut ws, json!({"type": "hello", "token": "x"})).await;
        hear(&mut ws).await;
    }
    let token = issue(&st, "cl-1", "app");
    let mut ws = connect(addr).await;
    say(&mut ws, json!({"type": "hello", "token": token})).await;
    assert_eq!(hear(&mut ws).await["code"], "too_many_attempts");
}

#[tokio::test]
async fn a_new_connection_replaces_the_old_one() {
    let st = store();
    let token = issue(&st, "cl-1", "app");
    let (gw, _irx, _prx) = gateway(st);
    let addr = serve(&gw).await;
    let mut a = connect(addr).await;
    say(&mut a, json!({"type": "hello", "token": token})).await;
    hear(&mut a).await;
    let mut b = connect(addr).await;
    say(&mut b, json!({"type": "hello", "token": token})).await;
    hear(&mut b).await;
    gw.send(Outbound::Text {
        conversation: conversation("cl-1"),
        body: "새 연결로".into(),
    })
    .await
    .unwrap();
    assert_eq!(hear(&mut b).await["body"], "새 연결로");
    // 예전 연결은 닫힌다.
    let closed = tokio::time::timeout(Duration::from_secs(5), a.next())
        .await
        .unwrap();
    assert!(matches!(
        closed,
        None | Some(Ok(WsMessage::Close(_))) | Some(Err(_))
    ));
}

/// 반쯤 죽은 연결: 퐁이 오지 않으면 끊고, 그 앱으로 갈 글은 알림 대화로 간다. 살아 있는 연결은 그대로다.
#[tokio::test]
async fn silent_connections_are_dropped_and_messages_fall_back() {
    let st = store();
    st.lock()
        .unwrap()
        .set_setting(
            lucia_store::keys::NOTIFY_CONVERSATION,
            "memory:dm",
            Utc::now(),
        )
        .unwrap();
    let token = issue(&st, "cl-1", "app");
    let alive = issue(&st, "cl-2", "app");
    let (itx, _irx) = mpsc::channel(8);
    let (ptx, _prx) = mpsc::channel(8);
    let gw = Gateway::with_heartbeat(
        st.clone(),
        itx.clone(),
        ptx.clone(),
        Duration::from_millis(100),
        Duration::from_millis(300),
    );
    let addr = serve(&gw).await;

    // 읽지 않는 연결(잠든 휴대폰): 핑에 답하지 않는다.
    let mut dead = connect(addr).await;
    say(&mut dead, json!({"type": "hello", "token": token})).await;
    hear(&mut dead).await;
    // 읽는 연결: 클라이언트 라이브러리가 핑에 퐁으로 답한다.
    let mut live = connect(addr).await;
    say(&mut live, json!({"type": "hello", "token": alive})).await;
    hear(&mut live).await;
    let reader = tokio::spawn(async move { while live.next().await.is_some() {} });

    assert!(gw.online("cl-1"));
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(!gw.online("cl-1"), "답이 없는 연결은 끊는다");
    assert!(gw.online("cl-2"), "퐁을 보내는 연결은 그대로");

    let (primary, handle) = lucia_channel::memory::MemoryChannel::new(2000, itx);
    let router = RouterChannel::new(Arc::new(primary), gw.clone(), st, None);
    router
        .send(Outbound::Text {
            conversation: conversation("cl-1"),
            body: "늦은 답".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        handle.take_sent(),
        vec![(ConversationId("memory:dm".into()), "늦은 답".into())]
    );
    reader.abort();
    drop(dead);
}

/// 기기(결정 0031 밑작업): 이벤트 입구가 이어져 있으면 이벤트 프레임만 받아 넘긴다. 말과 누르기는 받지 않는다.
#[tokio::test]
async fn devices_send_events_only() {
    let st = store();
    let device = issue(&st, "cl-d", "device");
    let (gw, mut irx, _prx) = gateway(st.clone());
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    gw.set_events(tx);
    let addr = serve(&gw).await;

    let mut ws = connect(addr).await;
    say(&mut ws, json!({"type": "hello", "token": device})).await;
    let welcome = hear(&mut ws).await;
    assert_eq!(welcome["role"], "device");
    assert_eq!(welcome["capabilities"]["events"], true);

    say(
        &mut ws,
        json!({"type": "event", "key": "k1", "text": " 회의실 도착 "}),
    )
    .await;
    assert_eq!(hear(&mut ws).await["type"], "accepted");
    match events.recv().await.unwrap() {
        crate::events::Poke::Event(ev) => {
            assert_eq!(ev.text, "회의실 도착");
            assert_eq!(ev.key, "k1");
        }
        other => panic!("이벤트가 아니다: {other:?}"),
    }
    say(&mut ws, json!({"type": "message", "text": "메일 보내"})).await;
    assert_eq!(hear(&mut ws).await["code"], "forbidden");
    say(&mut ws, json!({"type": "event", "text": "  "})).await;
    assert_eq!(hear(&mut ws).await["code"], "empty");
    assert!(
        irx.try_recv().is_err(),
        "기기의 글은 사람의 말로 들어가지 않는다"
    );
    assert!(!gw.online("cl-d"), "앱 대화로 붙지 않는다");
}

// ---------- 판 2(결정 0043) ----------

/// hello를 보내고 welcome을 받는다.
async fn hello(ws: &mut Ws, v: Value) -> Value {
    say(ws, v).await;
    let w = hear(ws).await;
    assert_eq!(w["type"], "welcome", "{w}");
    w
}

#[tokio::test]
async fn frames_are_numbered_kept_and_replayed_on_resume() {
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    let token = issue(&st, "cl-1", "app");
    let addr = serve(&gw).await;
    let mut ws = connect(addr).await;
    let w = hello(&mut ws, json!({"type": "hello", "token": token})).await;
    assert_eq!(w["v"], 2);
    assert_eq!(w["seq"], 0);
    assert!(
        w["methods"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m == "history")
    );
    assert!(
        !w["methods"].as_array().unwrap().is_empty() && w["capabilities"]["attachments"] == true
    );
    // 연결돼 있으면 번호를 붙여 바로 받는다.
    let conv = conversation("cl-1");
    gw.send(Outbound::Text {
        conversation: conv.clone(),
        body: "첫 글".into(),
    })
    .await
    .unwrap();
    let f = hear(&mut ws).await;
    assert_eq!(
        (f["type"].as_str(), f["seq"].as_i64()),
        (Some("text"), Some(1))
    );
    say(&mut ws, json!({"type": "ack", "seq": 1})).await;
    say(&mut ws, json!({"type": "ping"})).await;
    assert_eq!(hear(&mut ws).await["type"], "pong");
    assert_eq!(
        st.lock()
            .unwrap()
            .active_client("cl-1")
            .unwrap()
            .unwrap()
            .acked_seq,
        1
    );
    drop(ws);
    // 끊긴 동안의 글·카드·닫힘은 상자에 남는다(푸시가 없어도 게이트웨이에 직접 보내면 쌓인다).
    tokio::time::sleep(Duration::from_millis(50)).await;
    gw.send(Outbound::Text {
        conversation: conv.clone(),
        body: "끊긴 동안".into(),
    })
    .await
    .unwrap();
    let r = gw.send_card(card(conv.clone())).await.unwrap();
    gw.close_card(&r, &card(conv.clone())).await.unwrap();
    gw.typing(&conv).await.unwrap();
    // 다시 연결: 1 뒤의 것을 차례로 다시 받고, resumed로 끝을 안다.
    let mut ws = connect(addr).await;
    let w = hello(
        &mut ws,
        json!({"type": "hello", "token": token, "resume": 1}),
    )
    .await;
    assert_eq!(w["seq"], 4);
    let kinds: Vec<(String, i64, bool)> = {
        let mut v = Vec::new();
        for _ in 0..3 {
            let f = hear(&mut ws).await;
            v.push((
                f["type"].as_str().unwrap().to_owned(),
                f["seq"].as_i64().unwrap(),
                f["replay"] == true,
            ));
        }
        v
    };
    assert_eq!(
        kinds,
        vec![
            ("text".into(), 2, true),
            ("card".into(), 3, true),
            ("card_closed".into(), 4, true)
        ]
    );
    let done = hear(&mut ws).await;
    assert_eq!(done["type"], "resumed");
    assert_eq!(
        (done["to"].as_i64(), done["truncated"].as_bool()),
        (Some(4), Some(false))
    );
    // 지워진 구간을 건너뛰어야 하면 truncated.
    drop(ws);
    st.lock()
        .unwrap()
        .prune_outbox(
            Utc::now() + chrono::Duration::days(1),
            Utc::now() + chrono::Duration::days(1),
        )
        .unwrap();
    let mut ws = connect(addr).await;
    hello(
        &mut ws,
        json!({"type": "hello", "token": token, "resume": 1}),
    )
    .await;
    let done = hear(&mut ws).await;
    assert_eq!(
        (done["type"].as_str(), done["truncated"].as_bool()),
        (Some("resumed"), Some(true))
    );
}

#[tokio::test]
async fn calls_history_commands_and_scopes() {
    let st = store();
    let (gw, mut irx, _prx) = gateway(st.clone());
    let (ctx, mut crx) = mpsc::channel(4);
    gw.set_commands(ctx);
    let token = issue(&st, "cl-1", "app");
    {
        let s = st.lock().unwrap();
        s.set_setting(
            lucia_store::keys::NOTIFY_CONVERSATION,
            "discord:dm",
            Utc::now(),
        )
        .unwrap();
        for (conv, body) in [
            ("app:cl-1", "앱에서"),
            ("discord:dm", "DM에서"),
            ("discord:guild", "서버"),
        ] {
            s.add_episode(&lucia_store::NewEpisode::user(conv, body, Utc::now()))
                .unwrap();
        }
    }
    let addr = serve(&gw).await;
    let mut ws = connect(addr).await;
    hello(&mut ws, json!({"type": "hello", "token": token})).await;
    say(
        &mut ws,
        json!({"type": "call", "id": "h1", "method": "history"}),
    )
    .await;
    let r = hear(&mut ws).await;
    assert_eq!(
        (r["type"].as_str(), r["id"].as_str(), r["ok"].as_bool()),
        (Some("result"), Some("h1"), Some(true))
    );
    let bodies = |r: &Value| -> Vec<String> {
        r["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["body"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(bodies(&r), ["앱에서"]);
    say(
        &mut ws,
        json!({"type": "call", "id": "h2", "method": "history", "params": {"scope": "owner"}}),
    )
    .await;
    assert_eq!(
        bodies(&hear(&mut ws).await),
        ["앱에서", "DM에서"],
        "서버 대화는 넣지 않는다"
    );
    // 모르는 메서드, 범위 없는 모드 바꾸기.
    say(
        &mut ws,
        json!({"type": "call", "id": "x", "method": "nope"}),
    )
    .await;
    assert_eq!(hear(&mut ws).await["error"]["code"], "unknown_method");
    say(&mut ws, json!({"type": "call", "id": "c1", "method": "command", "params": {"name": "권한", "option": "해제"}})).await;
    assert_eq!(hear(&mut ws).await["error"]["code"], "forbidden");
    assert!(
        crx.try_recv().is_err(),
        "범위가 없으면 처리기로 보내지도 않는다"
    );
    // 보기는 범위 없이, 처리기의 답이 돌아온다.
    say(
        &mut ws,
        json!({"type": "call", "id": "c2", "method": "command", "params": {"name": "권한"}}),
    )
    .await;
    let cmd = crx.recv().await.unwrap();
    assert_eq!(
        (
            cmd.platform.as_str(),
            cmd.sender.as_str(),
            cmd.option.as_deref()
        ),
        ("app", "cl-1", None)
    );
    cmd.reply
        .send(lucia_channel::CommandReply::private("🛡️ 기본"))
        .unwrap();
    let r = hear(&mut ws).await;
    assert_eq!(
        (r["id"].as_str(), r["data"]["text"].as_str()),
        (Some("c2"), Some("🛡️ 기본"))
    );
    // 주 모델(결정 0055)은 범위 없이 바꾸기까지 처리기로 간다. 명령 목록에도 있다.
    say(&mut ws, json!({"type": "call", "id": "m1", "method": "command", "params": {"name": "모델", "option": "opus"}})).await;
    let cmd = crx.recv().await.unwrap();
    assert_eq!(
        (cmd.name.as_str(), cmd.option.as_deref()),
        ("모델", Some("opus"))
    );
    cmd.reply
        .send(lucia_channel::CommandReply::private("주 모델: Opus"))
        .unwrap();
    assert_eq!(hear(&mut ws).await["data"]["text"], "주 모델: Opus");
    say(
        &mut ws,
        json!({"type": "call", "id": "m2", "method": "commands"}),
    )
    .await;
    let r = hear(&mut ws).await;
    let listed = r["data"]["commands"].as_array().unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["권한", "모델"]
    );
    assert_eq!(
        (
            listed[0]["can_change"].as_bool(),
            listed[1]["can_change"].as_bool()
        ),
        (Some(false), Some(true))
    );
    assert_eq!(
        listed[1]["options"],
        json!(["fable", "opus", "sonnet", "haiku", "default"])
    );
    assert_eq!(
        (listed[0]["current"].as_str(), listed[1]["current"].as_str()),
        (Some("기본"), Some("default")),
        "지금 값은 options 가운데 하나"
    );
    st.lock()
        .unwrap()
        .set_setting(crate::models::KEY, "opus", Utc::now())
        .unwrap();
    say(
        &mut ws,
        json!({"type": "call", "id": "m3", "method": "commands"}),
    )
    .await;
    assert_eq!(
        hear(&mut ws).await["data"]["commands"][1]["current"],
        "opus"
    );
    // 범위를 주면 바꾸기도 처리기로 간다.
    st.lock()
        .unwrap()
        .set_client_scopes("cl-1", &[scopes::SECURITY_MODE.into()])
        .unwrap();
    say(&mut ws, json!({"type": "call", "id": "c3", "method": "command", "params": {"name": "권한", "option": "해제"}})).await;
    let cmd = crx.recv().await.unwrap();
    assert_eq!(cmd.option.as_deref(), Some("해제"));
    drop(cmd);
    assert_eq!(hear(&mut ws).await["error"]["code"], "failed");
    // 푸시 토큰 등록(형식 확인).
    say(&mut ws, json!({"type": "call", "id": "p1", "method": "push.register", "params": {"token": "bad token!", "platform": "android"}})).await;
    assert_eq!(hear(&mut ws).await["error"]["code"], "bad_params");
    say(&mut ws, json!({"type": "call", "id": "p2", "method": "push.register", "params": {"token": "abc:DEF_12-3", "platform": "android"}})).await;
    assert_eq!(hear(&mut ws).await["data"]["registered"], true);
    assert_eq!(
        st.lock()
            .unwrap()
            .active_client("cl-1")
            .unwrap()
            .unwrap()
            .push_token
            .as_deref(),
        Some("abc:DEF_12-3")
    );
    // 앱의 이벤트는 이벤트 입구가 없으면 받지 않고, 말은 그대로.
    say(&mut ws, json!({"type": "event", "text": "알림 왔어"})).await;
    assert_eq!(hear(&mut ws).await["code"], "unsupported");
    say(
        &mut ws,
        json!({"type": "message", "text": "안녕", "id": "m1"}),
    )
    .await;
    assert_eq!(hear(&mut ws).await["id"], "m1");
    assert_eq!(irx.recv().await.unwrap().body, "안녕");
}

/// `session.compact`(결정 0056): 작업 큐에 넣고 결과를 기다려 답한다. 겹친 요청은 `busy`, 제때 끝나지 않으면
/// `timeout`이고 그때 답 통로가 닫혀 작업자가 건너뛸 수 있다.
#[tokio::test]
async fn session_compact_waits_for_the_worker_and_reports_the_outcome() {
    use crate::worker::{CompactReply, Job};
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    let token = issue(&st, "cl-1", "app");
    let addr = serve(&gw).await;
    let mut ws = connect(addr).await;
    let w = hello(&mut ws, json!({"type": "hello", "token": token})).await;
    assert!(
        w["methods"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m == "session.compact"),
        "{w}"
    );
    let call = |id: &str| json!({"type": "call", "id": id, "method": "session.compact"});
    // 작업 큐가 없는 데몬.
    say(&mut ws, call("s0")).await;
    assert_eq!(hear(&mut ws).await["error"]["code"], "unsupported");

    let (jobs_tx, mut jobs_rx) = mpsc::channel(4);
    gw.set_jobs(jobs_tx, Duration::from_millis(300));
    let next = async |rx: &mut mpsc::Receiver<Job>| match rx.recv().await {
        Some(Job::Compact { reply, .. }) => reply,
        other => panic!("정리 요청이 아니다: {other:?}"),
    };
    // 도는 동안 겹친 요청은 받지 않는다. 끝나면 결과가 처음 요청의 답으로 온다.
    say(&mut ws, call("s1")).await;
    let reply = next(&mut jobs_rx).await;
    say(&mut ws, call("s2")).await;
    let busy = hear(&mut ws).await;
    assert_eq!(
        (busy["id"].as_str(), busy["error"]["code"].as_str()),
        (Some("s2"), Some("busy"))
    );
    reply
        .send(CompactReply::Compacted {
            pre: 121_000,
            post: 14_000,
        })
        .unwrap();
    let r = hear(&mut ws).await;
    assert_eq!(
        (r["id"].as_str(), r["ok"].as_bool()),
        (Some("s1"), Some(true))
    );
    assert_eq!(
        (
            r["data"]["outcome"].as_str(),
            r["data"]["pre_tokens"].as_u64(),
            r["data"]["context_tokens"].as_u64()
        ),
        (Some("compacted"), Some(121_000), Some(14_000))
    );
    assert!(r["data"]["text"].as_str().unwrap().contains("121,000"));
    // 나머지 결과.
    for (id, done, outcome, tokens) in [
        (
            "s3",
            CompactReply::Unchanged { tokens: 14_000 },
            "unchanged",
            14_000,
        ),
        ("s4", CompactReply::NoSession, "none", 0),
        (
            "s4b",
            CompactReply::Small { tokens: 30_000 },
            "small",
            30_000,
        ),
        (
            "s5",
            CompactReply::Reset("요약이 비었다".into()),
            "reset",
            0,
        ),
    ] {
        say(&mut ws, call(id)).await;
        next(&mut jobs_rx).await.send(done).unwrap();
        let r = hear(&mut ws).await;
        assert_eq!(
            (
                r["id"].as_str(),
                r["data"]["outcome"].as_str(),
                r["data"]["context_tokens"].as_u64()
            ),
            (Some(id), Some(outcome), Some(tokens)),
            "{r}"
        );
    }
    say(&mut ws, call("s6")).await;
    next(&mut jobs_rx)
        .await
        .send(CompactReply::Error("저장소 오류".into()))
        .unwrap();
    let r = hear(&mut ws).await;
    assert_eq!(
        (r["ok"].as_bool(), r["error"]["code"].as_str()),
        (Some(false), Some("failed"))
    );
    // 제때 끝나지 않으면 timeout이고, 답 통로가 닫혀 있다(작업자는 아직 시작하지 않았으면 건너뛴다).
    say(&mut ws, call("s7")).await;
    let late = next(&mut jobs_rx).await;
    assert_eq!(hear(&mut ws).await["error"]["code"], "timeout");
    assert!(late.is_closed());
    // 그 뒤에는 다시 받는다. min_tokens는 작업자에게 그대로 간다.
    say(
        &mut ws,
        json!({"type": "call", "id": "s8", "method": "session.compact", "params": {"min_tokens": 80000}}),
    )
    .await;
    match jobs_rx.recv().await {
        Some(Job::Compact { min_tokens, reply }) => {
            assert_eq!(min_tokens, 80_000);
            reply.send(CompactReply::NoSession).unwrap();
        }
        other => panic!("정리 요청이 아니다: {other:?}"),
    }
    assert_eq!(hear(&mut ws).await["data"]["outcome"], "none");
    say(
        &mut ws,
        json!({"type": "call", "id": "s9", "method": "session.compact", "params": {"min_tokens": "많이"}}),
    )
    .await;
    assert_eq!(hear(&mut ws).await["error"]["code"], "bad_params");
    assert!(jobs_rx.try_recv().is_err(), "틀린 요청은 큐에 넣지 않는다");
}

#[tokio::test]
async fn invoke_asks_only_declared_features_and_waits_for_the_result() {
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    let token = issue(&st, "cl-1", "app");
    let addr = serve(&gw).await;
    let mut ws = connect(addr).await;
    let w = hello(&mut ws, json!({"type": "hello", "token": token, "features": ["Location", "bad feature!", "location"]})).await;
    assert_eq!(w["features"], json!(["location"]));
    let e = gw
        .invoke("cl-1", "camera.snap", json!({}), Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(e.code, "unsupported");
    assert_eq!(
        gw.invoke("cl-9", "location.get", json!({}), Duration::from_secs(1))
            .await
            .unwrap_err()
            .code,
        "offline"
    );
    let gw2 = gw.clone();
    let task = tokio::spawn(async move {
        gw2.invoke(
            "cl-1",
            "location.get",
            json!({"precise": true}),
            Duration::from_secs(5),
        )
        .await
    });
    let ask = hear(&mut ws).await;
    assert_eq!(
        (ask["type"].as_str(), ask["method"].as_str()),
        (Some("invoke"), Some("location.get"))
    );
    // 엉뚱한 ID의 답은 받지 않는다.
    say(
        &mut ws,
        json!({"type": "result", "id": "d-other", "ok": true}),
    )
    .await;
    assert_eq!(hear(&mut ws).await["code"], "bad_ref");
    say(
        &mut ws,
        json!({"type": "result", "id": ask["id"], "ok": true, "data": {"lat": 37.5}}),
    )
    .await;
    assert_eq!(task.await.unwrap().unwrap(), json!({"lat": 37.5}));
    // 끊기면 기다리던 부탁은 offline으로 끝난다.
    let gw3 = gw.clone();
    let task = tokio::spawn(async move {
        gw3.invoke("cl-1", "location.get", json!({}), Duration::from_secs(5))
            .await
    });
    let _ = hear(&mut ws).await;
    drop(ws);
    assert_eq!(task.await.unwrap().unwrap_err().code, "offline");
}

#[tokio::test]
async fn files_go_up_and_down_over_http_with_the_token() {
    use axum::body::Body;
    use tower::ServiceExt as _;
    let st = store();
    let (gw, mut irx, _prx) = gateway(st.clone());
    let token = issue(&st, "cl-1", "app");
    let other = issue(&st, "cl-2", "app");
    let peer = ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000)));
    let call = |req: axum::http::Request<Body>| {
        let gw = gw.clone();
        async move {
            let mut req = req;
            req.extensions_mut().insert(peer);
            let r = gw.router().oneshot(req).await.unwrap();
            let status = r.status().as_u16();
            let headers = r.headers().clone();
            let body = axum::body::to_bytes(r.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec();
            (status, body, headers)
        }
    };
    let req = |method: &str, path: &str, tok: Option<&str>, body: Vec<u8>| {
        let mut b = axum::http::Request::builder().method(method).uri(path);
        if let Some(t) = tok {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        b.body(Body::from(body)).unwrap()
    };
    // 토큰 없이는 안 된다.
    assert_eq!(call(req("GET", "/v1/me", None, vec![])).await.0, 401);
    let (s, body, _) = call(req("GET", "/v1/me", Some(&token), vec![])).await;
    assert_eq!(s, 200);
    let me: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(me["client"], "cl-1");
    // 올리기 → 말에 붙이기 → 첨부로 들어간다.
    let (s, body, _) = call(req(
        "POST",
        "/v1/files?name=%EB%A9%94%EB%AA%A8.txt",
        Some(&token),
        "안녕 파일".as_bytes().to_vec(),
    ))
    .await;
    assert_eq!(s, 201, "{}", String::from_utf8_lossy(&body));
    let up: Value = serde_json::from_slice(&body).unwrap();
    let fid = up["id"].as_str().unwrap().to_owned();
    assert_eq!(up["name"], "메모.txt");
    // 다른 앱은 그 파일을 말에 붙이거나 받지 못한다.
    let (s, _, _) = call(req(
        "GET",
        &format!("/v1/files/{fid}"),
        Some(&other),
        vec![],
    ))
    .await;
    assert_eq!(s, 404);
    let conv = conversation("cl-1");
    let atts = attachments(&gw, &conversation("cl-2"), std::slice::from_ref(&fid));
    assert!(atts.is_err());
    let atts = attachments(&gw, &conv, std::slice::from_ref(&fid)).unwrap();
    assert_eq!(atts[0].url, format!("lucia-file:{fid}"));
    // 실제 말로.
    let addr = serve(&gw).await;
    let mut ws = connect(addr).await;
    hello(&mut ws, json!({"type": "hello", "token": token})).await;
    say(&mut ws, json!({"type": "message", "files": [fid]})).await;
    assert_eq!(hear(&mut ws).await["type"], "accepted");
    let inbound = irx.recv().await.unwrap();
    assert_eq!(inbound.attachments.len(), 1);
    // 데몬이 보낸 파일은 ID로 알리고 HTTP로 받는다.
    gw.send(Outbound::Files {
        conversation: conv.clone(),
        body: "정리했어요".into(),
        files: vec![lucia_core::OutFile {
            name: "정리.md".into(),
            bytes: b"# hi".to_vec(),
        }],
    })
    .await
    .unwrap();
    let f = hear(&mut ws).await;
    assert_eq!(f["type"], "files");
    let out_id = f["files"][0]["id"].as_str().unwrap().to_owned();
    let (s, body, headers) = call(req(
        "GET",
        &format!("/v1/files/{out_id}"),
        Some(&token),
        vec![],
    ))
    .await;
    assert_eq!((s, body.as_slice()), (200, b"# hi".as_slice()));
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains("UTF-8''")
    );
    // 기록과 푸시 토큰도 HTTP로.
    let (s, _, _) = call(req("GET", "/v1/history?limit=5", Some(&token), vec![])).await;
    assert_eq!(s, 200);
    let (s, _, _) = call(
        axum::http::Request::builder()
            .method("PUT")
            .uri("/v1/push")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"token":"tok-1","platform":"ios"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(
        st.lock()
            .unwrap()
            .active_client("cl-1")
            .unwrap()
            .unwrap()
            .push_platform
            .as_deref(),
        Some("ios")
    );
    assert_eq!(
        call(req("DELETE", "/v1/push", Some(&token), vec![]))
            .await
            .0,
        200
    );
    assert!(
        st.lock()
            .unwrap()
            .active_client("cl-1")
            .unwrap()
            .unwrap()
            .push_token
            .is_none()
    );
}

#[tokio::test]
async fn proxied_requests_need_a_public_url_and_count_failures_by_real_address() {
    use axum::body::Body;
    use tower::ServiceExt as _;
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    let token = issue(&st, "cl-1", "app");
    let proxies = crate::public::Proxies::parse(&["172.18.0.11".into()]).unwrap();
    let call = |xff: Option<&str>, tok: &str| {
        let gw = gw.clone();
        let mut b =
            axum::http::Request::get("/v1/me").header("authorization", format!("Bearer {tok}"));
        if let Some(x) = xff {
            b = b.header("x-forwarded-for", x);
        }
        let mut req = b.body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([172, 18, 0, 11], 40000))));
        async move { gw.router().oneshot(req).await.unwrap().status().as_u16() }
    };
    // 프록시를 믿지 않으면: 머리 없는 연결은 집 안, 전달 머리를 실은 연결은 거절(닫힌 쪽으로 실패).
    assert_eq!(call(None, &token).await, 200);
    assert_eq!(call(Some("8.8.8.8"), &token).await, 400);
    gw.set_public(proxies.clone(), false);
    assert_eq!(
        call(Some("8.8.8.8"), &token).await,
        403,
        "공개 주소가 없으면 외부는 받지 않는다"
    );
    gw.set_public(proxies, true);
    assert_eq!(call(None, &token).await, 400, "프록시가 머리를 주지 않았다");
    assert_eq!(call(Some("8.8.8.8"), &token).await, 200);
    // 실패는 실제 주소로 센다: 한 주소가 막혀도 다른 주소는 그대로.
    for _ in 0..MAX_FAILURES {
        assert_eq!(call(Some("1.1.1.1"), "lucia_wrong").await, 401);
    }
    assert_eq!(call(Some("1.1.1.1"), &token).await, 429);
    assert_eq!(call(Some("8.8.8.8"), &token).await, 200);
}

#[tokio::test]
async fn notify_target_app_gets_owner_notices_and_pushes_when_away() {
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    issue(&st, "cl-1", "app");
    let (push_tx, mut push_rx) = mpsc::unbounded_channel();
    gw.set_push(push_tx);
    let (primary, handle) = lucia_channel::memory::MemoryChannel::new(2000, mpsc::channel(1).0);
    let take = || handle.take_sent();
    let router = RouterChannel::new(Arc::new(primary), gw.clone(), st.clone(), None);
    let dm = ConversationId("memory:dm".into());
    st.lock()
        .unwrap()
        .set_setting(lucia_store::keys::NOTIFY_CONVERSATION, &dm.0, Utc::now())
        .unwrap();
    // 알릴 곳: 앱을 정하지 않았으면 DM, 정하면 앱 대화(용도로 가른다).
    assert_eq!(
        st.lock().unwrap().notify_conversation().as_deref(),
        Some("memory:dm")
    );
    st.lock()
        .unwrap()
        .set_setting(keys::NOTIFY_CLIENT, "cl-1", Utc::now())
        .unwrap();
    let target = ConversationId(st.lock().unwrap().notify_conversation().unwrap());
    assert_eq!(target, conversation("cl-1"));
    let text = |to: &ConversationId, body: &str| Outbound::Text {
        conversation: to.clone(),
        body: body.into(),
    };
    // 앱이 끊겨 있고 푸시도 없으면 DM으로.
    router.send(text(&target, "하나")).await.unwrap();
    assert_eq!(take()[0].0, dm);
    // 연결돼 있으면 앱으로. 소유자가 DM에서 직접 한 대화의 답은 DM 그대로.
    let (_, mut rx) = gw.attach_for_test("cl-1");
    router.send(text(&target, "둘")).await.unwrap();
    assert_eq!(rx.try_recv().unwrap()["body"], "둘");
    router.send(text(&dm, "DM 대화의 답")).await.unwrap();
    assert_eq!(take()[0].1, "DM 대화의 답");
    assert!(rx.try_recv().is_err());
    let r = router.send_card(card(target.clone())).await.unwrap();
    assert!(r.starts_with("app:cl-1:"));
    assert_eq!(rx.try_recv().unwrap()["type"], "card");
    // 뒤로 가 있으면 연결돼 있어도 푸시로 깨운다(진행 안내는 깨우지 않는다).
    gw.set_push_ready(true);
    st.lock()
        .unwrap()
        .set_client_push("cl-1", Some("tok"), Some("android"), Utc::now())
        .unwrap();
    let conn = gw.conns.lock().unwrap().get("cl-1").unwrap().id;
    gw.set_background("cl-1", conn, true);
    router.send(text(&target, "셋")).await.unwrap();
    let job = push_rx.try_recv().unwrap();
    assert_eq!(
        (job.kind.as_str(), job.token.as_str(), job.high),
        ("text", "tok", false)
    );
    router
        .send(Outbound::Note {
            conversation: target.clone(),
            body: "진행 중".into(),
        })
        .await
        .unwrap();
    assert!(push_rx.try_recv().is_err());
    // 끊겨도 푸시가 있으면 앱으로(상자에 쌓고 깨운다). 카드는 높은 우선순위, DM과 겹치지 않는다.
    gw.disconnect("cl-1");
    let r = router.send_card(card(target.clone())).await.unwrap();
    assert!(r.starts_with("app:cl-1:"));
    let job = push_rx.try_recv().unwrap();
    assert!(job.high && job.kind == "card");
    assert!(
        take().is_empty() && handle.cards().is_empty(),
        "DM으로 겹쳐 보내지 않는다"
    );
    assert!(router.ready());
    // 푸시를 쓸 수 없게 되면 다시 DM으로.
    gw.set_push_ready(false);
    router.send(text(&target, "넷")).await.unwrap();
    assert_eq!(take()[0].0, dm);
}

// ---------- 이모지·스티커(결정 0045) ----------

#[tokio::test]
async fn expressions_are_listed_served_announced_and_sent_as_stickers() {
    use crate::expressions::testing::png;
    use crate::expressions::{Expressions, Kind, Meta};
    use axum::body::Body;
    use tower::ServiceExt as _;
    let st = store();
    let (gw, mut irx, _prx) = gateway(st.clone());
    let token = issue(&st, "cl-1", "app");
    let dir = std::env::temp_dir().join(format!("lucia-gw-expr-{}", uuid::Uuid::new_v4().simple()));
    let peer = ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000)));
    let call = |path: &str, tok: Option<&str>, etag: Option<&str>| {
        let gw = gw.clone();
        let mut b = axum::http::Request::builder().method("GET").uri(path);
        if let Some(t) = tok {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        if let Some(e) = etag {
            b = b.header("if-none-match", e);
        }
        let mut req = b.body(Body::empty()).unwrap();
        async move {
            req.extensions_mut().insert(peer);
            let r = gw.router().oneshot(req).await.unwrap();
            let status = r.status().as_u16();
            let headers = r.headers().clone();
            let body = axum::body::to_bytes(r.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec();
            (status, body, headers)
        }
    };
    // 서비스가 붙기 전: 이 기능이 없는 데몬으로 보인다.
    assert_eq!(call("/v1/expressions", Some(&token), None).await.0, 501);
    let me: Value = serde_json::from_slice(&call("/v1/me", Some(&token), None).await.1).unwrap();
    assert!(me.get("expressions").is_none());

    let x = Expressions::new(st.clone(), dir.clone());
    let now = Utc::now();
    let meta = |d: &str| Meta {
        description: d.into(),
        ..Meta::default()
    };
    let emoji = x
        .add(
            Kind::Emoji,
            "lucia_smile",
            &meta("웃는 얼굴"),
            &png(128),
            "admin",
            now,
        )
        .unwrap();
    let sticker = x
        .add(
            Kind::Sticker,
            "축하",
            &meta("꽃가루"),
            &png(320),
            "admin",
            now,
        )
        .unwrap();
    gw.set_expressions(x.clone());

    // 목록: 토큰이 있어야 하고, ETag가 같으면 304.
    assert_eq!(call("/v1/expressions", None, None).await.0, 401);
    let (s, body, headers) = call("/v1/expressions", Some(&token), None).await;
    assert_eq!(s, 200);
    let list: Value = serde_json::from_slice(&body).unwrap();
    let version = list["version"].as_str().unwrap().to_owned();
    assert_eq!(headers["etag"], format!("\"{version}\""));
    assert_eq!(headers["cache-control"], "private, no-cache");
    assert_eq!(list["emoji"][0]["id"], emoji.id.as_str());
    assert_eq!(list["stickers"][0]["name"], "축하");
    let (s, body, _) = call(
        "/v1/expressions",
        Some(&token),
        Some(&format!("\"{version}\"")),
    )
    .await;
    assert_eq!((s, body.len()), (304, 0));
    assert_eq!(
        call("/v1/expressions", Some(&token), Some("\"old\""))
            .await
            .0,
        200
    );
    // 그림: 바뀌지 않는 주소라 오래 캐시한다.
    let (s, body, headers) = call(
        &format!("/v1/expressions/files/{}", emoji.file.id),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(body, png(128));
    assert_eq!(headers["content-type"], "image/png");
    assert!(
        headers["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(
        call("/v1/expressions/files/x_00", Some(&token), None)
            .await
            .0,
        404
    );
    assert_eq!(
        call(
            &format!("/v1/expressions/files/{}", emoji.file.id),
            None,
            None
        )
        .await
        .0,
        401
    );

    // 연결: welcome에 요약이 실린다.
    let addr = serve(&gw).await;
    let mut ws = connect(addr).await;
    let w = hello(&mut ws, json!({"type": "hello", "token": token})).await;
    assert_eq!(w["expressions"]["version"], version.as_str());
    assert_eq!(
        (
            w["expressions"]["emoji"].as_i64(),
            w["expressions"]["stickers"].as_i64()
        ),
        (Some(1), Some(1))
    );
    // 앱 → 데몬: 스티커는 이름과 설명 한 줄로 Lucia에게 간다. 목록에 없으면 bad_sticker(연결은 유지).
    say(
        &mut ws,
        json!({"type": "message", "text": "나 합격했어 :lucia_smile:", "sticker": sticker.id, "id": "m1"}),
    )
    .await;
    assert_eq!(hear(&mut ws).await, json!({"type": "accepted", "id": "m1"}));
    assert_eq!(
        irx.recv().await.unwrap().body,
        "나 합격했어 :lucia_smile:\n[스티커:축하 — 꽃가루]"
    );
    say(&mut ws, json!({"type": "message", "sticker": sticker.id})).await;
    assert_eq!(hear(&mut ws).await["type"], "accepted");
    assert_eq!(irx.recv().await.unwrap().body, "[스티커:축하 — 꽃가루]");
    say(&mut ws, json!({"type": "message", "sticker": "s_none"})).await;
    assert_eq!(hear(&mut ws).await["code"], "bad_sticker");
    say(&mut ws, json!({"type": "message", "sticker": "  "})).await;
    assert_eq!(hear(&mut ws).await["code"], "empty");
    // 데몬 → 앱: 번호 붙는 sticker 프레임. 상자에 남아 다시 받을 수 있다.
    let conv = conversation("cl-1");
    gw.send(Outbound::Text {
        conversation: conv.clone(),
        body: "축하해요 :lucia_smile:".into(),
    })
    .await
    .unwrap();
    gw.send_sticker(&conv, &sticker.id).await.unwrap();
    let t = hear(&mut ws).await;
    assert_eq!(
        t["body"], "축하해요 :lucia_smile:",
        "앱에는 이모지 표기가 그대로 간다"
    );
    let f = hear(&mut ws).await;
    assert_eq!(
        f,
        json!({"type": "sticker", "seq": 2, "sticker": {"id": sticker.id, "name": "축하"}})
    );
    assert!(gw.send_sticker(&conv, "s_none").await.is_err());
    let kept = st.lock().unwrap().outbox_after("cl-1", 1, 10).unwrap();
    assert_eq!((kept.len(), kept[0].kind.as_str()), (1, "sticker"));
    // 목록이 바뀌면 연결된 앱에 번호 없는 프레임이 간다.
    x.add(
        Kind::Emoji,
        "lucia_clap",
        &meta("박수"),
        &png(64),
        "agent",
        now,
    )
    .unwrap();
    let changed = hear(&mut ws).await;
    assert_eq!(changed["type"], "expressions");
    assert_ne!(changed["version"], version.as_str());
    assert!(changed.get("seq").is_none());
    assert_eq!(changed["version"], x.catalog().version.as_str());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn router_keeps_app_expressions_off_other_channels() {
    use crate::expressions::testing::png;
    use crate::expressions::{Expressions, Kind, Meta};
    let st = store();
    st.lock()
        .unwrap()
        .set_setting(
            lucia_store::keys::NOTIFY_CONVERSATION,
            "memory:dm",
            Utc::now(),
        )
        .unwrap();
    let (gw, _irx, _prx) = gateway(st.clone());
    issue(&st, "cl-1", "app");
    let dir = std::env::temp_dir().join(format!("lucia-gw-expr-{}", uuid::Uuid::new_v4().simple()));
    let x = Expressions::new(st.clone(), dir.clone());
    let now = Utc::now();
    x.add(
        Kind::Emoji,
        "lucia_smile",
        &Meta::default(),
        &png(128),
        "admin",
        now,
    )
    .unwrap();
    let sticker = x
        .add(
            Kind::Sticker,
            "축하",
            &Meta::default(),
            &png(320),
            "admin",
            now,
        )
        .unwrap();
    gw.set_expressions(x);
    let (push_tx, mut push_rx) = mpsc::unbounded_channel();
    gw.set_push(push_tx);
    let (primary, handle) = lucia_channel::memory::MemoryChannel::new(2000, mpsc::channel(1).0);
    let router = RouterChannel::new(Arc::new(primary), gw.clone(), st.clone(), None);
    let text = |to: &str, body: &str| Outbound::Text {
        conversation: ConversationId(to.into()),
        body: body.into(),
    };
    // 기본 채널(Discord)로 가는 글: 목록의 이모지 표기만 지운다. 표기뿐인 글은 그대로 둔다.
    router
        .send(text(
            "memory:dm",
            "좋아요 :lucia_smile: 그리고 std::fmt::Display",
        ))
        .await
        .unwrap();
    router
        .send(text("memory:dm", ":lucia_smile:"))
        .await
        .unwrap();
    let sent = handle.take_sent();
    assert_eq!(sent[0].1, "좋아요 그리고 std::fmt::Display");
    assert_eq!(sent[1].1, ":lucia_smile:");
    // 앱이 끊겨 알림 대화로 돌릴 때도 지우고, 앱 스티커는 보내지 않는다.
    let app = conversation("cl-1");
    router
        .send(text(&app.0, "늦은 답 :lucia_smile:"))
        .await
        .unwrap();
    assert_eq!(handle.take_sent()[0].1, "늦은 답");
    assert!(matches!(
        router.send_sticker(&app, &sticker.id).await,
        Err(ChannelError::Unsupported(_))
    ));
    assert!(handle.take_stickers().is_empty());
    // 연결된 앱: 표기는 그대로, 스티커는 앱으로. 다른 대화의 스티커는 기본 채널로(서버 스티커).
    let (_, mut rx) = gw.attach_for_test("cl-1");
    router.send(text(&app.0, "답 :lucia_smile:")).await.unwrap();
    assert_eq!(rx.try_recv().unwrap()["body"], "답 :lucia_smile:");
    router.send_sticker(&app, &sticker.id).await.unwrap();
    assert_eq!(rx.try_recv().unwrap()["type"], "sticker");
    router
        .send_sticker(&ConversationId("memory:guild".into()), "123")
        .await
        .unwrap();
    assert_eq!(handle.take_stickers()[0].1, "123");
    // 뒤로 가 있으면 스티커도 푸시로 깨운다.
    gw.set_push_ready(true);
    st.lock()
        .unwrap()
        .set_client_push("cl-1", Some("tok"), Some("android"), Utc::now())
        .unwrap();
    gw.disconnect("cl-1");
    router.send_sticker(&app, &sticker.id).await.unwrap();
    let job = push_rx.try_recv().unwrap();
    assert_eq!((job.kind.as_str(), job.high), ("sticker", false));
    let _ = std::fs::remove_dir_all(dir);
}

// ---------- 앱이 내주는 기능(결정 0053) ----------

#[tokio::test]
async fn app_offers_tools_and_lucia_calls_them_when_the_owner_allows() {
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    let token = issue(&st, "cl-1", "app");
    let addr = serve(&gw).await;
    let mut ws = connect(addr).await;
    let w = hello(
        &mut ws,
        json!({"type": "hello", "token": token, "tools": [
            {"name": "location.get", "description": "휴대폰의  현재\n위치", "params": {"precise": "boolean"}},
            {"name": "alarm.set", "description": "알람 맞추기", "effect": "write"},
            {"name": "Bad Name", "description": "버린다"},
            {"name": "nodot", "description": "버린다"},
            {"name": "location.get", "description": "겹치면 처음 것"},
            {"name": "big.params", "params": {"x": "y".repeat(3000)}},
        ]}),
    )
    .await;
    // 정리된 이름만 받아들이고, 첫 마디는 기능으로도 친다.
    assert_eq!(
        w["tools"],
        json!(["location.get", "alarm.set", "big.params"])
    );
    assert_eq!(w["features"], json!(["alarm", "big", "location"]));
    // 범위가 없으면 Lucia에게 보이지도, 불리지도 않는다.
    assert!(gw.app_tools().is_empty());
    assert_eq!(
        gw.call_app("location.get", json!({}), None, true)
            .await
            .unwrap_err()
            .code,
        "unsupported"
    );
    st.lock()
        .unwrap()
        .set_client_scopes("cl-1", &[scopes::APP_TOOLS.to_owned()])
        .unwrap();
    let tools = gw.app_tools();
    assert_eq!(
        tools
            .iter()
            .map(|(_, _, t)| t.name.as_str())
            .collect::<Vec<_>>(),
        ["alarm.set", "big.params", "location.get"]
    );
    let loc = &tools[2].2;
    assert_eq!(loc.description, "휴대폰의 현재 위치", "설명은 한 줄로");
    assert_eq!(loc.params, json!({"precise": "boolean"}));
    assert!(tools[1].2.params.is_null(), "너무 큰 인자 설명은 버린다");
    assert!(tools[0].2.write && !loc.write);
    // 부르기: invoke 프레임이 가고, 앱의 답이 돌아온다.
    let gw2 = gw.clone();
    let task = tokio::spawn(async move {
        gw2.call_app("location.get", json!({"precise": true}), None, false)
            .await
    });
    let ask = hear(&mut ws).await;
    assert_eq!(
        (
            ask["type"].as_str(),
            ask["method"].as_str(),
            ask["params"]["precise"].as_bool()
        ),
        (Some("invoke"), Some("location.get"), Some(true))
    );
    say(
        &mut ws,
        json!({"type": "result", "id": ask["id"], "ok": true, "data": {"lat": 37.5, "lng": 127.0}}),
    )
    .await;
    let (app, data) = task.await.unwrap().unwrap();
    assert_eq!(
        (app.as_str(), data["lat"].as_f64()),
        ("시험 앱", Some(37.5))
    );
    // 무엇을 바꾸는 기능은 사람이 대화 중일 때만.
    assert_eq!(
        gw.call_app("alarm.set", json!({}), None, false)
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    // 앱이 거절하면 그 까닭이 온다. 다른 앱을 짚으면 없다고 한다.
    let gw3 = gw.clone();
    let task = tokio::spawn(async move { gw3.call_app("alarm.set", json!({}), None, true).await });
    let ask = hear(&mut ws).await;
    say(
        &mut ws,
        json!({"type": "result", "id": ask["id"], "ok": false, "error": {"code": "denied", "message": "사용자가 거절"}}),
    )
    .await;
    let err = task.await.unwrap().unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.message.as_str()),
        ("denied", "사용자가 거절")
    );
    assert_eq!(
        gw.call_app("location.get", json!({}), Some("cl-other"), true)
            .await
            .unwrap_err()
            .code,
        "unsupported"
    );
    // 끊기면 내준 기능도 사라진다.
    drop(ws);
    for _ in 0..200 {
        if gw.app_tools().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(gw.app_tools().is_empty());
}

/// 결정 0060: Lucia의 프로필 이미지를 앱이 받는다. `welcome`과 `/v1/me`에 실리고, 그림은 바뀌지 않는 주소로
/// 받고, 바뀌거나 없어지면 연결된 앱에 번호 없는 `profile` 프레임이 간다.
#[tokio::test]
async fn the_profile_image_is_served_and_announced_to_apps() {
    use crate::profile::tests::jpeg;
    use axum::body::Body;
    use tower::ServiceExt as _;
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    let token = issue(&st, "cl-1", "app");
    let dir = std::env::temp_dir().join(format!(
        "lucia-gw-profile-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let peer = ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000)));
    let call = |path: &str, tok: Option<&str>| {
        let gw = gw.clone();
        let mut b = axum::http::Request::builder().method("GET").uri(path);
        if let Some(t) = tok {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let mut req = b.body(Body::empty()).unwrap();
        async move {
            req.extensions_mut().insert(peer);
            let r = gw.router().oneshot(req).await.unwrap();
            let status = r.status().as_u16();
            let headers = r.headers().clone();
            let body = axum::body::to_bytes(r.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec();
            (status, body, headers)
        }
    };
    let json_of = |body: &[u8]| serde_json::from_slice::<Value>(body).unwrap();
    // 서비스가 붙기 전: 이 기능이 없는 데몬으로 보인다(`profile` 필드가 없다).
    assert_eq!(call("/v1/profile", Some(&token)).await.0, 501);
    assert!(
        json_of(&call("/v1/me", Some(&token)).await.1)
            .get("profile")
            .is_none()
    );

    let profile = crate::profile::Profile::new(st.clone(), dir.clone());
    gw.set_profile(profile.clone());
    // 정한 그림이 없으면 avatar가 null이다.
    assert_eq!(call("/v1/profile", None).await.0, 401);
    let (s, body, _) = call("/v1/profile", Some(&token)).await;
    assert_eq!(
        (s, json_of(&body)),
        (200, json!({"name": "Lucia", "avatar": null}))
    );
    assert_eq!(
        json_of(&call("/v1/me", Some(&token)).await.1)["profile"],
        json!({"name": "Lucia", "avatar": null})
    );
    let addr = serve(&gw).await;
    let mut ws = connect(addr).await;
    let w = hello(&mut ws, json!({"type": "hello", "token": token})).await;
    assert_eq!(w["profile"], json!({"name": "Lucia", "avatar": null}));

    // 정하면 연결된 앱에 번호 없는 프레임이 간다.
    let now: chrono::DateTime<chrono::Utc> = "2026-11-20T04:00:00Z".parse().unwrap();
    let avatar = profile.set(&jpeg(512, 512), "lucia", now).unwrap();
    let changed = hear(&mut ws).await;
    assert_eq!(changed["type"], "profile");
    assert!(changed.get("seq").is_none());
    assert_eq!(changed["name"], "Lucia", "프레임은 이름도 싣는다");
    assert_eq!(changed["avatar"]["id"], avatar.id.as_str());
    assert_eq!(
        (
            changed["avatar"]["mime"].as_str(),
            changed["avatar"]["width"].as_i64(),
            changed["avatar"]["size"].as_u64(),
            changed["avatar"]["updated_at"].as_str()
        ),
        (
            Some("image/jpeg"),
            Some(512),
            Some(jpeg(512, 512).len() as u64),
            Some("2026-11-20T04:00:00Z")
        )
    );
    assert_eq!(
        json_of(&call("/v1/me", Some(&token)).await.1)["profile"]["avatar"]["id"],
        avatar.id.as_str()
    );
    assert_eq!(
        json_of(&call("/v1/profile", Some(&token)).await.1)["avatar"]["id"],
        avatar.id.as_str()
    );
    // 그림: 토큰이 있어야 하고, id가 내용 해시라 오래 캐시한다.
    let path = format!("/v1/profile/avatar/{}", avatar.id);
    assert_eq!(call(&path, None).await.0, 401);
    let (s, body, headers) = call(&path, Some(&token)).await;
    assert_eq!(s, 200);
    assert_eq!(body, jpeg(512, 512));
    assert_eq!(headers["content-type"], "image/jpeg");
    assert_eq!(headers["etag"], format!("\"{}\"", avatar.id).as_str());
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(
        headers["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    for missing in [
        "/v1/profile/avatar/a_000000000000000000000000",
        "/v1/profile/avatar/..%2Fx",
    ] {
        assert_eq!(call(missing, Some(&token)).await.0, 404, "{missing}");
    }
    // 새로 붙는 앱은 welcome에서 받는다.
    let mut second = connect(addr).await;
    let w = hello(
        &mut second,
        json!({"type": "hello", "token": issue(&st, "cl-2", "app")}),
    )
    .await;
    assert_eq!(w["profile"]["avatar"]["id"], avatar.id.as_str());
    // 바꾸면 새 id가 가고(예전 그림도 한동안 받을 수 있다), 없애면 null이 간다.
    let next = profile
        .set(&crate::expressions::testing::png(256), "lucia", now)
        .unwrap();
    let changed = hear(&mut ws).await;
    assert_eq!(changed["avatar"]["id"], next.id.as_str());
    assert_eq!(call(&path, Some(&token)).await.0, 200);
    profile.clear().unwrap();
    let cleared = hear(&mut ws).await;
    assert_eq!(
        (cleared["type"].as_str(), &cleared["avatar"]),
        (Some("profile"), &Value::Null)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// 결정 0064: 이름은 `welcome`과 `/v1/me`·`/v1/profile`에 실리고, 바뀌면(기본으로 되돌릴 때도) 연결된 앱에
/// 그림과 함께 번호 없는 `profile` 프레임이 간다. 그림은 그대로 실린다.
#[tokio::test]
async fn the_profile_name_is_served_and_announced_to_apps() {
    use crate::profile::tests::jpeg;
    use axum::body::Body;
    use tower::ServiceExt as _;
    let st = store();
    let (gw, _irx, _prx) = gateway(st.clone());
    let token = issue(&st, "cl-1", "app");
    let dir = std::env::temp_dir().join(format!(
        "lucia-gw-profile-name-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let profile = crate::profile::Profile::new(st.clone(), dir.clone());
    gw.set_profile(profile.clone());
    let peer = ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000)));
    let get_json = |path: &str| {
        let gw = gw.clone();
        let token = token.clone();
        let mut req = axum::http::Request::builder()
            .method("GET")
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        async move {
            req.extensions_mut().insert(peer);
            let r = gw.router().oneshot(req).await.unwrap();
            let body = axum::body::to_bytes(r.into_body(), usize::MAX)
                .await
                .unwrap();
            serde_json::from_slice::<Value>(&body).unwrap()
        }
    };
    let now: chrono::DateTime<chrono::Utc> = "2026-11-21T04:00:00Z".parse().unwrap();
    // 정하지 않았으면 기본 이름이다.
    assert_eq!(get_json("/v1/profile").await["name"], "Lucia");
    let addr = serve(&gw).await;
    let mut ws = connect(addr).await;
    let w = hello(&mut ws, json!({"type": "hello", "token": token})).await;
    assert_eq!(w["profile"]["name"], "Lucia");
    // 그림도 있는 채로 이름을 바꾸면 프레임이 둘 다 싣는다.
    let avatar = profile.set(&jpeg(512, 512), "owner", now).unwrap();
    let first = hear(&mut ws).await;
    assert_eq!(first["avatar"]["id"], avatar.id.as_str());
    assert_eq!(profile.set_name("  루시아 ", now).unwrap(), "루시아");
    let changed = hear(&mut ws).await;
    assert_eq!(changed["type"], "profile");
    assert!(changed.get("seq").is_none());
    assert_eq!(changed["name"], "루시아", "다듬은 이름이 간다");
    assert_eq!(changed["avatar"]["id"], avatar.id.as_str());
    for path in ["/v1/profile", "/v1/me"] {
        let v = get_json(path).await;
        let shown = if path == "/v1/me" { &v["profile"] } else { &v };
        assert_eq!(shown["name"], "루시아", "{path}");
        assert_eq!(shown["avatar"]["id"], avatar.id.as_str(), "{path}");
    }
    // 새로 붙는 앱은 welcome에서 받는다.
    let mut second = connect(addr).await;
    let w = hello(
        &mut second,
        json!({"type": "hello", "token": issue(&st, "cl-2", "app")}),
    )
    .await;
    assert_eq!(w["profile"]["name"], "루시아");
    // 규칙에 맞지 않으면 바꾸지 않고 알리지도 않는다. 비우면 기본 이름으로 돌아가고 그 프레임이 간다.
    assert!(profile.set_name("한\n줄", now).is_err());
    assert!(profile.set_name(&"가".repeat(25), now).is_err());
    assert_eq!(profile.name(), "루시아");
    assert_eq!(profile.set_name("", now).unwrap(), "Lucia");
    let reset = hear(&mut ws).await;
    assert_eq!(reset["name"], "Lucia");
    assert_eq!(reset["avatar"]["id"], avatar.id.as_str());
    let _ = std::fs::remove_dir_all(dir);
}

// ---------- 결정 0063: 여러 칸 버튼(로그인 입력 카드) ----------

/// 로그인 입력 카드가 될 승인 요청(저장소에는 넣지 않고 카드 모양만 본다).
fn login_input_approval(token: &str) -> lucia_store::Approval {
    lucia_store::Approval {
        id: "approval-login-0063".into(),
        kind: crate::approvals::KIND_LOGIN_INPUT.into(),
        payload: json!({
            "site": "example.com",
            "site_name": "example.com",
            "origin": "https://example.com",
            "reason": "시험",
            "token": token,
            "link": format!("http://192.168.0.2:8780/site-login/{token}"),
            "error": null,
            "attempts": 3,
        }),
        summary: "로그인 입력 → https://example.com".into(),
        status: lucia_store::ApprovalStatus::Pending,
        requested_at: Utc::now(),
        expires_at: Utc::now() + chrono::Duration::minutes(10),
        decided_at: None,
        run_id: None,
        conversation: None,
        card_ref: None,
        result: None,
        auto: false,
    }
}

/// 로그인 입력 카드: `welcome`이 폼 능력을 알리고, 카드 프레임의 첫 버튼에 칸 셋이 실린다. 누름의 값은 다듬지
/// 않고(비밀번호의 앞뒤 공백도 값이다) `ButtonPress::fields`로 가며, 상자·기록·`Debug`에 남지 않는다.
#[tokio::test]
async fn form_card_reaches_the_app_and_its_values_arrive_untrimmed_and_unrecorded() {
    let st = store();
    let token = issue(&st, "cl-1", "app");
    let (gw, _irx, mut prx) = gateway(st.clone());
    let addr = serve(&gw).await;
    let mut ws = connect(addr).await;
    let welcome = hello(&mut ws, json!({"type": "hello", "token": token})).await;
    assert_eq!(welcome["capabilities"]["forms"], true);
    assert_eq!(
        welcome["capabilities"]["prompts"], true,
        "입력창 능력은 그대로"
    );
    assert!(gw.capabilities().forms);

    // 카드: 첫 버튼에만 폼이 있고 칸의 이름·종류·길이가 그대로 실린다.
    let link_token = "t".repeat(64);
    let approval = login_input_approval(&link_token);
    let card = crate::approvals::login_input_card(
        &approval,
        conversation("cl-1"),
        chrono_tz::Asia::Seoul,
        crate::approvals::CardState::Open,
    );
    let r = gw.send_card(card).await.unwrap();
    let frame = hear(&mut ws).await;
    assert_eq!(frame["type"], "card");
    let buttons = frame["card"]["buttons"].as_array().unwrap();
    assert_eq!(buttons.len(), 2);
    let form = &buttons[0]["form"];
    assert_eq!(form["title"], "로그인 정보 입력");
    let fields: Vec<(&str, &str, i64, i64)> = form["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["key"].as_str().unwrap(),
                f["kind"].as_str().unwrap(),
                f["min_len"].as_i64().unwrap(),
                f["max_len"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        fields,
        [
            ("username", "text", 1, 100),
            ("password", "secret", 1, 200),
            ("save", "check", 0, 0),
        ]
    );
    assert!(buttons[0]["prompt"].is_null(), "입력창과 폼은 따로다");
    assert!(
        buttons[1]["form"].is_null(),
        "취소 버튼에는 폼이 없다(null로 실린다)"
    );
    assert!(
        frame["card"]["body"]
            .as_str()
            .unwrap()
            .contains(&format!("/site-login/{link_token}")),
        "폼을 못 그리는 앱을 위한 링크가 본문에 있다"
    );
    let approve = buttons[0]["action"].as_str().unwrap().to_owned();

    // 누름: 앞뒤 공백을 그대로 두고, 켜고 끄는 값은 "true"/"false" 글로 온다. 한 값(`input`)은 없다.
    say(
        &mut ws,
        json!({"type": "press", "ref": r, "action": approve, "inputs": {
            "username": "  me_1  ",
            "password": " pw-시험-1234 ",
            "save": true,
        }}),
    )
    .await;
    let press = prx.recv().await.unwrap();
    assert!(press.input.is_none());
    let fields = press.fields.as_ref().expect("여러 칸의 값이 온다");
    assert_eq!(fields.len(), 3);
    assert_eq!(fields["username"].expose(), "  me_1  ", "다듬지 않는다");
    assert_eq!(fields["password"].expose(), " pw-시험-1234 ");
    assert_eq!(fields["save"].expose(), "true");
    let shown = format!("{press:?}");
    assert!(
        !shown.contains("pw-시험-1234") && !shown.contains("me_1"),
        "{shown}"
    );

    say(
        &mut ws,
        json!({"type": "press", "ref": r, "action": approve, "inputs": {"save": false}}),
    )
    .await;
    let press = prx.recv().await.unwrap();
    assert_eq!(press.fields.unwrap()["save"].expose(), "false");

    // `inputs`가 없거나 null이면 칸 없는 누름이다(취소 버튼, 옛 앱).
    for press_frame in [
        json!({"type": "press", "ref": r, "action": approve}),
        json!({"type": "press", "ref": r, "action": approve, "inputs": null}),
    ] {
        say(&mut ws, press_frame).await;
        assert!(prx.recv().await.unwrap().fields.is_none());
    }
    // 한 값을 받는 누름은 그대로 다듬어 온다(결정 0017).
    say(
        &mut ws,
        json!({"type": "press", "ref": r, "action": approve, "input": " 123456 "}),
    )
    .await;
    let press = prx.recv().await.unwrap();
    assert_eq!(press.input.unwrap().expose(), "123456");
    assert!(press.fields.is_none());

    // 값은 상자·기록 어디에도 남지 않는다(카드 프레임은 상자에 있다 = 비어 있지 않다).
    {
        let s = st.lock().unwrap();
        let outbox = s.outbox_after("cl-1", 0, 100).unwrap();
        assert!(!outbox.is_empty());
        for needle in ["pw-시험-1234", "me_1"] {
            assert!(
                outbox
                    .iter()
                    .all(|row| !format!("{row:?}").contains(needle)),
                "상자에 {needle} 없음"
            );
            assert!(
                s.recent_activity(100)
                    .unwrap()
                    .iter()
                    .all(|a| !a.summary.contains(needle))
            );
        }
        s.add_episode(&lucia_store::NewEpisode::user(
            "app:cl-1",
            "안녕",
            Utc::now(),
        ))
        .unwrap();
    }
    say(
        &mut ws,
        json!({"type": "call", "id": "h1", "method": "history"}),
    )
    .await;
    let history = hear(&mut ws).await;
    let text = history.to_string();
    assert!(text.contains("안녕"), "기록이 비어 있지 않다: {text}");
    assert!(
        !text.contains("pw-시험-1234") && !text.contains("me_1"),
        "{text}"
    );
}

/// 여러 칸 값의 한도: 8칸·이름 40자·값 1000자(문자 수)까지. 넘기면 `bad_frame`이고 아무것도 넘기지 않는다.
#[tokio::test]
async fn form_press_inputs_are_bounded_and_nothing_bad_is_forwarded() {
    let st = store();
    let token = issue(&st, "cl-1", "app");
    let (gw, _irx, mut prx) = gateway(st.clone());
    let addr = serve(&gw).await;
    let mut ws = connect(addr).await;
    hello(&mut ws, json!({"type": "hello", "token": token})).await;
    let r = gw.send_card(card(conversation("cl-1"))).await.unwrap();
    assert_eq!(hear(&mut ws).await["type"], "card");
    let press =
        |inputs: Value| json!({"type": "press", "ref": r, "action": "approve:1", "inputs": inputs});

    // 한도 안: 8칸, 한글 1000자 값, 이름 40자.
    let eight: serde_json::Map<String, Value> = (0..8)
        .map(|i| (format!("k{i}"), json!(format!("v{i}"))))
        .collect();
    say(&mut ws, press(Value::Object(eight))).await;
    assert_eq!(prx.recv().await.unwrap().fields.unwrap().len(), 8);
    let long_ok = "가".repeat(1000);
    say(&mut ws, press(json!({"password": long_ok}))).await;
    assert_eq!(
        prx.recv().await.unwrap().fields.unwrap()["password"]
            .expose()
            .chars()
            .count(),
        1000,
        "한도는 바이트가 아니라 문자 수다"
    );
    say(&mut ws, press(json!({ "k".repeat(40): "x" }))).await;
    assert_eq!(prx.recv().await.unwrap().fields.unwrap().len(), 1);

    // 한도 밖: 9칸, 1001자 값, 41자 이름, 빈 이름, 글·참거짓이 아닌 값.
    let nine: serde_json::Map<String, Value> =
        (0..9).map(|i| (format!("k{i}"), json!("v"))).collect();
    for bad in [
        Value::Object(nine),
        json!({"password": "가".repeat(1001)}),
        json!({ "k".repeat(41): "x" }),
        json!({"": "x"}),
        json!({"password": 1234}),
        json!({"password": ["a"]}),
        json!("문자열"),
    ] {
        say(&mut ws, press(bad)).await;
        let reply = hear(&mut ws).await;
        assert_eq!(reply["type"], "error");
        assert_eq!(reply["code"], "bad_frame", "{reply}");
        assert!(prx.try_recv().is_err(), "넘어가지 않는다");
    }
    // 연결은 살아 있고 다음 누름은 받는다.
    say(&mut ws, press(json!({"username": "ok"}))).await;
    assert!(prx.recv().await.unwrap().fields.is_some());
    // 남의 카드에 칸 값을 실어도 그대로 거절한다.
    say(
        &mut ws,
        json!({"type": "press", "ref": "app:cl-2:x", "action": "approve:1", "inputs": {"a": "b"}}),
    )
    .await;
    assert_eq!(hear(&mut ws).await["code"], "bad_ref");
    assert!(prx.try_recv().is_err());
}

// ---------- 결정 0063: 브라우저 화면 넘겨받기 ----------

/// 시험용 넘겨받기 대상: 메모리 안 상태. 넣은 글·누른 자리는 값 없이 종류와 크기만 기록한다.
struct FakeHandover {
    active: AtomicBool,
    begins: std::sync::atomic::AtomicUsize,
    ends: std::sync::atomic::AtomicUsize,
    tabs: Vec<crate::browser::Tab>,
    /// 받은 입력의 요약(`click:x,y`, `text:글자수`, `key:이름`, `scroll:양`).
    inputs: Mutex<Vec<String>>,
    fail_begin: AtomicBool,
    end_warning: Mutex<Option<String>>,
}

impl FakeHandover {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(false),
            begins: std::sync::atomic::AtomicUsize::new(0),
            ends: std::sync::atomic::AtomicUsize::new(0),
            tabs: vec![crate::browser::Tab {
                id: "t1".into(),
                url: "https://example.com/login".into(),
                title: "로그인".into(),
            }],
            inputs: Mutex::default(),
            fail_begin: AtomicBool::new(false),
            end_warning: Mutex::default(),
        })
    }
}

impl Handover for FakeHandover {
    fn left(&self) -> crate::browser::BoxFut<'_, Option<Duration>> {
        let on = self.active.load(Ordering::SeqCst);
        Box::pin(async move { on.then(|| Duration::from_secs(540)) })
    }
    fn begin(&self) -> crate::browser::BoxFut<'_, Result<(), String>> {
        self.begins
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let result = if self.fail_begin.load(Ordering::SeqCst) {
            Err("브라우저를 띄우지 못했다".to_owned())
        } else {
            self.active.store(true, Ordering::SeqCst);
            Ok(())
        };
        Box::pin(async move { result })
    }
    fn end<'a>(&'a self, _why: &'a str) -> crate::browser::BoxFut<'a, Result<(), String>> {
        self.ends.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.active.store(false, Ordering::SeqCst);
        let warning = self.end_warning.lock().unwrap().clone();
        Box::pin(async move { warning.map_or(Ok(()), Err) })
    }
    fn tabs(&self) -> crate::browser::BoxFut<'_, Result<Vec<crate::browser::Tab>, String>> {
        let tabs = self.tabs.clone();
        Box::pin(async move { Ok(tabs) })
    }
    fn frame<'a>(
        &'a self,
        tab: &'a str,
    ) -> crate::browser::BoxFut<'a, Result<crate::browser::Frame, String>> {
        let known = self.tabs.iter().any(|t| t.id == tab);
        Box::pin(async move {
            if known {
                Ok(crate::browser::Frame {
                    jpeg: vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3],
                })
            } else {
                Err("없는 탭".to_owned())
            }
        })
    }
    fn input<'a>(
        &'a self,
        _tab: &'a str,
        input: &'a crate::browser::HandInput,
    ) -> crate::browser::BoxFut<'a, Result<(), String>> {
        use crate::browser::HandInput;
        let summary = match input {
            HandInput::Click { x, y } => format!("click:{x},{y}"),
            HandInput::Text(t) => format!("text:{}", t.chars().count()),
            HandInput::Key(k) => format!("key:{k}"),
            HandInput::Scroll(d) => format!("scroll:{d}"),
        };
        self.inputs.lock().unwrap().push(summary);
        Box::pin(async { Ok(()) })
    }
}

/// 결과 프레임을 받는다(`id`가 맞는 것).
async fn call_result(ws: &mut Ws, id: &str, method: &str, params: Value) -> Value {
    say(
        ws,
        json!({"type": "call", "id": id, "method": method, "params": params}),
    )
    .await;
    let r = hear(ws).await;
    assert_eq!(
        (r["type"].as_str(), r["id"].as_str()),
        (Some("result"), Some(id)),
        "{r}"
    );
    r
}

fn error_code(r: &Value) -> &str {
    assert_eq!(r["ok"], false, "{r}");
    r["error"]["code"].as_str().unwrap_or("")
}

/// 결정 0063: 브라우저 넘겨받기 메서드. 브라우저가 없는 데몬에는 등록되지 않고, 범위가 없는 앱에는 보이지도
/// 불리지도 않는다. 범위가 있으면 상태 → 시작 → 입력 → 끝내기가 된다.
#[tokio::test]
async fn handover_methods_need_the_scope_and_a_browser_and_run_the_whole_flow() {
    let st = store();
    let scoped = issue(&st, "cl-1", "app");
    let plain = issue(&st, "cl-2", "app");
    st.lock()
        .unwrap()
        .set_client_scopes("cl-1", &[scopes::BROWSER_HANDOVER.into()])
        .unwrap();
    let names = [
        "browser.handover.state",
        "browser.handover.start",
        "browser.handover.input",
        "browser.handover.end",
    ];
    let (gw, _irx, _prx) = gateway(st.clone());
    let addr = serve(&gw).await;
    assert!(gw.handover().is_none());

    // 브라우저를 붙이기 전: 범위가 있어도 메서드가 없다(`welcome.methods`에도 없다).
    let mut ws = connect(addr).await;
    let w = hello(&mut ws, json!({"type": "hello", "token": scoped})).await;
    for name in names {
        assert!(!w["methods"].as_array().unwrap().iter().any(|m| m == name));
        let r = call_result(&mut ws, "u", name, json!({})).await;
        assert_eq!(error_code(&r), "unknown_method", "{name}");
    }
    drop(ws);

    // 붙이면 메서드가 등록된다. 두 번 붙여도 처음 것이 그대로다.
    let browser = FakeHandover::new();
    gw.set_handover(browser.clone());
    gw.set_handover(FakeHandover::new());
    assert!(gw.handover().is_some());

    // 범위 없는 앱: 목록에 없고, 불러도 `forbidden`이며, 브라우저는 건드리지 않는다.
    let mut other = connect(addr).await;
    let w = hello(&mut other, json!({"type": "hello", "token": plain})).await;
    for name in names {
        assert!(!w["methods"].as_array().unwrap().iter().any(|m| m == name));
    }
    for name in names {
        let r = call_result(
            &mut other,
            "f",
            name,
            json!({"tab": "t1", "kind": "key", "key": "enter"}),
        )
        .await;
        assert_eq!(error_code(&r), "forbidden", "{name}");
    }
    assert_eq!(browser.begins.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(browser.inputs.lock().unwrap().is_empty());

    // 범위 있는 앱: 네 메서드가 목록에 있다.
    let mut ws = connect(addr).await;
    let w = hello(&mut ws, json!({"type": "hello", "token": scoped})).await;
    for name in names {
        assert!(
            w["methods"].as_array().unwrap().iter().any(|m| m == name),
            "{name}"
        );
    }

    // 넘겨받기 전: 상태는 꺼짐, 입력은 `not_active`(인자가 맞을 때). 인자가 틀리면 켜짐 여부와 상관없이 `bad_params`.
    let state = call_result(&mut ws, "s0", "browser.handover.state", json!({})).await;
    assert_eq!(state["ok"], true);
    assert_eq!(state["data"]["active"], false);
    assert!(state["data"]["left_seconds"].is_null());
    assert_eq!(
        state["data"]["max_seconds"],
        crate::browser::tools::HANDOVER_MAX.as_secs()
    );
    assert_eq!(state["data"]["tabs"], json!([]));
    let r = call_result(
        &mut ws,
        "i0",
        "browser.handover.input",
        json!({"tab": "t1", "kind": "click", "x": 10, "y": 20}),
    )
    .await;
    assert_eq!(error_code(&r), "not_active");
    for (i, bad) in [
        json!({"kind": "click", "x": 1, "y": 2}),
        json!({"tab": "", "kind": "click", "x": 1, "y": 2}),
        json!({"tab": "t1", "kind": "drag"}),
        json!({"tab": "t1"}),
        json!({"tab": "t1", "kind": "click", "x": 1}),
        json!({"tab": "t1", "kind": "click", "x": -1, "y": 2}),
        json!({"tab": "t1", "kind": "text", "text": ""}),
        json!({"tab": "t1", "kind": "text", "text": "가".repeat(2001)}),
        json!({"tab": "t1", "kind": "key", "key": ""}),
        json!({"tab": "t1", "kind": "key", "key": "k".repeat(21)}),
        json!({"tab": "t1", "kind": "scroll"}),
    ]
    .into_iter()
    .enumerate()
    {
        let r = call_result(
            &mut ws,
            &format!("b{i}"),
            "browser.handover.input",
            bad.clone(),
        )
        .await;
        assert_eq!(error_code(&r), "bad_params", "{bad}");
    }
    assert!(browser.inputs.lock().unwrap().is_empty());

    // HTTP: 화면 그림(범위 확인 → 브라우저 확인 → 넘겨받는 중인지 → 탭).
    use axum::body::Body;
    use tower::ServiceExt as _;
    let peer = ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000)));
    let frame_of = |path: String, tok: Option<String>| {
        let gw = gw.clone();
        async move {
            let mut b = axum::http::Request::builder().method("GET").uri(path);
            if let Some(t) = tok {
                b = b.header("authorization", format!("Bearer {t}"));
            }
            let mut req = b.body(Body::empty()).unwrap();
            req.extensions_mut().insert(peer);
            let r = gw.router().oneshot(req).await.unwrap();
            let status = r.status().as_u16();
            let headers = r.headers().clone();
            let body = axum::body::to_bytes(r.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec();
            (status, headers, body)
        }
    };
    let code_of = |body: &[u8]| {
        serde_json::from_slice::<Value>(body).unwrap()["error"]["code"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    };
    let path = "/v1/browser/handover/frame/t1".to_owned();
    assert_eq!(
        frame_of(path.clone(), None).await.0,
        401,
        "토큰 없이는 안 된다"
    );
    let (s, _, body) = frame_of(path.clone(), Some(plain.clone())).await;
    assert_eq!(
        (s, code_of(&body).as_str()),
        (403, "forbidden"),
        "범위 없음"
    );
    let (s, _, body) = frame_of(path.clone(), Some(scoped.clone())).await;
    assert_eq!((s, code_of(&body).as_str()), (409, "not_active"));

    // 시작: 상태가 켜짐이 되고 탭과 남은 시간이 실린다. 이미 켜져 있으면 다시 시작하지 않는다.
    let started = call_result(&mut ws, "st", "browser.handover.start", json!({})).await;
    assert_eq!(started["ok"], true, "{started}");
    assert_eq!(started["data"]["active"], true);
    assert_eq!(started["data"]["left_seconds"], 540);
    assert_eq!(
        started["data"]["tabs"],
        json!([{"id": "t1", "url": "https://example.com/login", "title": "로그인"}])
    );
    let again = call_result(&mut ws, "st2", "browser.handover.start", json!({})).await;
    assert_eq!(again["data"]["active"], true);
    assert_eq!(browser.begins.load(std::sync::atomic::Ordering::SeqCst), 1);
    let state = call_result(&mut ws, "s1", "browser.handover.state", json!({})).await;
    assert_eq!(state["data"]["active"], true);

    // 입력: 네 가지. 굴리는 양은 한도로 줄인다. 글은 기록하지 않고 크기만 센다.
    for (i, params) in [
        json!({"tab": "t1", "kind": "click", "x": 10, "y": 20}),
        json!({"tab": "t1", "kind": "text", "text": "pw-시험-1234"}),
        json!({"tab": "t1", "kind": "key", "key": "enter"}),
        json!({"tab": "t1", "kind": "scroll", "dy": 99999}),
        json!({"tab": "t1", "kind": "scroll", "dy": -99999}),
        json!({"tab": "t1", "kind": "text", "text": "가".repeat(2000)}),
    ]
    .into_iter()
    .enumerate()
    {
        let r = call_result(&mut ws, &format!("in{i}"), "browser.handover.input", params).await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["data"], json!({"sent": true}));
    }
    assert_eq!(
        *browser.inputs.lock().unwrap(),
        [
            "click:10,20",
            "text:10",
            "key:enter",
            "scroll:3000",
            "scroll:-3000",
            "text:2000"
        ]
    );

    // 화면 그림: JPEG 그대로, 저장하지 않게 하고, `.jpg`를 붙여도 된다. 모르는 탭은 404.
    let (s, headers, body) = frame_of(path.clone(), Some(scoped.clone())).await;
    assert_eq!(s, 200);
    assert_eq!(headers["content-type"], "image/jpeg");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(body, [0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]);
    let (s, _, body) = frame_of(format!("{path}.jpg"), Some(scoped.clone())).await;
    assert_eq!((s, body.len()), (200, 7));
    let (s, _, body) = frame_of(
        "/v1/browser/handover/frame/nope".into(),
        Some(scoped.clone()),
    )
    .await;
    assert_eq!((s, code_of(&body).as_str()), (404, "not_found"));
    // 범위 없는 토큰은 켜진 뒤에도 그림을 못 받는다.
    let (s, _, _) = frame_of(path.clone(), Some(plain.clone())).await;
    assert_eq!(s, 403);

    // 끝내기: 꺼짐이 되고, 끝났으면 다시 불러도 성공이다. 탭 정리 경고는 실려 온다.
    let ended = call_result(&mut ws, "e1", "browser.handover.end", json!({})).await;
    assert_eq!(ended["data"], json!({"active": false, "warning": null}));
    assert_eq!(browser.ends.load(std::sync::atomic::Ordering::SeqCst), 1);
    let state = call_result(&mut ws, "s2", "browser.handover.state", json!({})).await;
    assert_eq!(state["data"]["active"], false);
    let (s, _, body) = frame_of(path.clone(), Some(scoped.clone())).await;
    assert_eq!(
        (s, code_of(&body).as_str()),
        (409, "not_active"),
        "끝난 뒤에는 그림이 없다"
    );
    let r = call_result(
        &mut ws,
        "i9",
        "browser.handover.input",
        json!({"tab": "t1", "kind": "key", "key": "enter"}),
    )
    .await;
    assert_eq!(error_code(&r), "not_active");
    *browser.end_warning.lock().unwrap() = Some("탭을 닫지 못했다".into());
    let ended = call_result(&mut ws, "e2", "browser.handover.end", json!({})).await;
    assert_eq!(ended["ok"], true);
    assert_eq!(ended["data"]["active"], false);
    assert_eq!(ended["data"]["warning"], "탭을 닫지 못했다");

    // 시작이 실패하면 `failed`이고 켜지지 않는다.
    browser.fail_begin.store(true, Ordering::SeqCst);
    let r = call_result(&mut ws, "st3", "browser.handover.start", json!({})).await;
    assert_eq!(error_code(&r), "failed");
    let state = call_result(&mut ws, "s3", "browser.handover.state", json!({})).await;
    assert_eq!(state["data"]["active"], false);
}

/// 브라우저가 붙지 않은 데몬: 범위가 있는 앱도 화면 그림은 `unavailable`(404)이다.
#[tokio::test]
async fn handover_frame_is_unavailable_without_a_browser() {
    use axum::body::Body;
    use tower::ServiceExt as _;
    let st = store();
    let token = issue(&st, "cl-1", "app");
    st.lock()
        .unwrap()
        .set_client_scopes("cl-1", &[scopes::BROWSER_HANDOVER.into()])
        .unwrap();
    let (gw, _irx, _prx) = gateway(st.clone());
    let mut req = axum::http::Request::builder()
        .uri("/v1/browser/handover/frame/t1")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
    let r = gw.router().oneshot(req).await.unwrap();
    assert_eq!(r.status().as_u16(), 404);
    let body = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
        "unavailable"
    );
}

/// 알림 받을 앱이 화면 넘겨받기 범위를 가졌는지(로그인 안내 글이 쓴다).
#[test]
fn notify_app_can_handover_follows_the_notify_app_scope() {
    let st = store();
    issue(&st, "cl-1", "app");
    let s = st.lock().unwrap();
    assert!(!notify_app_can_handover(&s), "알림 받을 앱이 없다");
    s.set_setting(keys::NOTIFY_CLIENT, "cl-1", Utc::now())
        .unwrap();
    assert!(!notify_app_can_handover(&s), "범위가 없다");
    s.set_client_scopes("cl-1", &[scopes::APP_TOOLS.into()])
        .unwrap();
    assert!(!notify_app_can_handover(&s), "다른 범위만 있다");
    s.set_client_scopes(
        "cl-1",
        &[scopes::APP_TOOLS.into(), scopes::BROWSER_HANDOVER.into()],
    )
    .unwrap();
    assert!(notify_app_can_handover(&s));
    s.revoke_client("cl-1", Utc::now()).unwrap();
    assert!(!notify_app_can_handover(&s), "취소한 앱은 아니다");
}
