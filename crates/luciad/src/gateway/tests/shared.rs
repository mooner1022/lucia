//! 앱은 기기가 여럿이어도 한 대화(결정 0067)의 시험: 모든 앱의 상자, 물은 곳에서 먼저 울리기, `sent` 사본, 카드·기록·
//! 파일·입력 중 표시의 공유, 알림 받을 곳, 라우터, 오류 안내.

use super::desk::{boxed, get, http, json_of, text, two_apps};
use super::*;

use axum::body::Body;

/// 연결해 hello까지 마친 소켓.
async fn join(addr: SocketAddr, token: &str) -> Ws {
    let mut ws = connect(addr).await;
    hello(&mut ws, json!({"type": "hello", "token": token})).await;
    ws
}

/// 저장소에 앱 둘(`cl-a`, `cl-b`)을 만들고 토큰을 돌려준다.
fn pair() -> (Arc<Mutex<Store>>, String, String) {
    let st = store();
    let a = issue(&st, "cl-a", "app");
    let b = issue(&st, "cl-b", "app");
    (st, a, b)
}

fn upload(token: &str, query: &str) -> axum::http::Request<Body> {
    axum::http::Request::post(format!("/v1/files?name=memo.txt{query}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("안녕 파일".as_bytes().to_vec()))
        .unwrap()
}

fn add_episode(st: &Arc<Mutex<Store>>, conversation: &str, body: &str) {
    st.lock()
        .unwrap()
        .add_episode(&lucia_store::NewEpisode::user(
            conversation,
            body,
            Utc::now(),
        ))
        .unwrap();
}

// ---------- 울릴 곳: 물은 앱에서 먼저(결정 0067) ----------

/// 휴대폰에서 물은 것의 답: 데스크톱 창이 앞에 있어도 휴대폰을 바로 울린다(푸시). 데스크톱 상자에는 울리지 않게 두고
/// 휴대폰 대화의 답이라고 밝힌다. 푸시를 받는 다른 앱(태블릿)은 바로 깨우지 않는다. 휴대폰이 푸시를 받지 않는 자리
/// (끊김·뒤·자리 비움)면 넘기기도 없다. 휴대폰이 앞에 있으면 푸시 없이 프레임으로 울리고, 안 보면 태블릿으로 넘긴다.
#[tokio::test(start_paused = true)]
async fn a_reply_to_the_phone_pushes_it_at_once_even_while_the_desktop_is_in_front() {
    // 휴대폰의 자리: 끊김(주머니), 뒤, 자리 비움, 앞.
    for phone_presence in [
        None,
        Some(Presence::Background),
        Some(Presence::Away),
        Some(Presence::Foreground),
    ] {
        let (st, gw, mut pushes) = two_apps();
        // 푸시를 받는 앱이 하나 더 있다(넘기기가 있다면 갈 곳). 데스크톱은 푸시가 없어 넘길 곳이 못 된다.
        issue(&st, "cl-tab", "app");
        st.lock()
            .unwrap()
            .set_client_push("cl-tab", Some("tok-tab"), Some("android"), Utc::now())
            .unwrap();
        let (_, mut desk) = gw.attach_for_test("cl-desk");
        gw.seen("cl-desk", 0);
        gw.seen("cl-phone", 0);
        let mut phone = phone_presence.map(|p| {
            let (conn, rx) = gw.attach_for_test("cl-phone");
            gw.set_presence("cl-phone", conn, p);
            rx
        });
        let case = format!("{phone_presence:?}");
        let front = phone_presence == Some(Presence::Foreground);

        // 계획: 휴대폰을 먼저 울린다. 앞에 있을 때만 때가 되면 태블릿으로 넘긴다.
        let apps = gw.app_clients();
        let (ring, forward, grace) = gw.plan_ring(&apps, false, Some("cl-phone"));
        assert_eq!(ring, ["cl-phone"], "{case}");
        assert_eq!(
            forward,
            if front {
                vec!["cl-tab".to_owned()]
            } else {
                vec![]
            },
            "{case}"
        );
        assert_eq!(grace, front.then_some(alerts::TEXT_GRACE), "{case}");

        gw.send(text("app:cl-phone", "휴대폰에서 물은 것의 답"))
            .await
            .unwrap();
        let phone_seq = boxed(&st, "cl-phone")[0]["seq"].as_i64().unwrap();
        let desk_seq = boxed(&st, "cl-desk")[0]["seq"].as_i64().unwrap();
        // 휴대폰: 제 대화의 답이라 `conversation`이 없다.
        let stored = &boxed(&st, "cl-phone")[0];
        assert!(stored.get("conversation").is_none(), "{case}");
        assert_eq!(stored.get("alert"), None, "{case}: 휴대폰이 울릴 곳");
        if front {
            assert!(pushes.try_recv().is_err(), "{case}: 앞에 있으면 푸시 없이");
            assert_eq!(
                phone.as_mut().unwrap().recv().await.unwrap()["seq"],
                phone_seq
            );
        } else {
            let job = pushes.try_recv().expect(&case);
            assert_eq!(
                (job.client_id.as_str(), job.kind.as_str(), job.seq, job.high),
                ("cl-phone", "text", phone_seq, false),
                "{case}: 바로 푸시"
            );
        }
        assert!(
            pushes.try_recv().is_err(),
            "{case}: 태블릿은 바로 깨우지 않는다"
        );
        // 데스크톱·태블릿: 울리지 않고, 휴대폰 대화의 답이라고 밝힌다.
        let on_desk = desk.recv().await.unwrap();
        assert_eq!(on_desk["alert"], false, "{case}");
        assert_eq!(on_desk["conversation"], "app:cl-phone", "{case}");
        assert_eq!(on_desk["seq"], desk_seq, "{case}");
        let on_tab = &boxed(&st, "cl-tab")[0];
        assert_eq!(on_tab["alert"], false, "{case}");
        assert_eq!(on_tab["conversation"], "app:cl-phone", "{case}");

        // 시간이 한참 지나면: 앞에 있던 휴대폰이 보지 않았을 때만 태블릿으로 넘어간다. 데스크톱에는 어느 쪽이든 없다.
        tokio::time::sleep(alerts::TEXT_GRACE * 2).await;
        if front {
            let job = pushes.try_recv().expect(&case);
            assert_eq!(
                (job.client_id.as_str(), job.kind.as_str()),
                ("cl-tab", "text")
            );
        }
        assert!(
            pushes.try_recv().is_err(),
            "{case}: 휴대폰이 푸시를 받았으면 넘기기는 없다"
        );
        assert!(
            desk.try_recv().is_err(),
            "{case}: 데스크톱에 alert를 넘기지 않는다"
        );

        // 휴대폰에서 보면 데스크톱에 그 번호까지 봤다고 알린다.
        gw.seen("cl-phone", phone_seq);
        assert_eq!(
            desk.recv().await.unwrap(),
            json!({"type": "seen", "seq": desk_seq}),
            "{case}"
        );
    }
}

/// 데스크톱에서 물은 것의 답: 앞에 있는 데스크톱만 울린다. 휴대폰 상자에는 울리지 않게 들어 있고, 데스크톱이
/// `seen`을 말하는데도 2분 동안 보지 않으면 휴대폰에 `alert{seq}`와 푸시가 간다(프레임은 이미 있으니 복사는 없다).
#[tokio::test(start_paused = true)]
async fn a_reply_to_the_desktop_rings_only_the_desktop_and_the_phone_hears_of_it_when_unseen() {
    let (st, gw, mut pushes) = two_apps();
    let (_, mut desk) = gw.attach_for_test("cl-desk");
    gw.seen("cl-desk", 0);
    gw.send(text("app:cl-desk", "데스크톱에서 물은 것의 답"))
        .await
        .unwrap();
    let on_desk = desk.recv().await.unwrap();
    assert!(on_desk.get("alert").is_none() && on_desk.get("conversation").is_none());
    let phone_box = boxed(&st, "cl-phone");
    assert_eq!(phone_box.len(), 1);
    assert_eq!(phone_box[0]["alert"], false);
    assert_eq!(phone_box[0]["conversation"], "app:cl-desk");
    let phone_seq = phone_box[0]["seq"].as_i64().unwrap();
    assert!(pushes.try_recv().is_err(), "휴대폰은 바로 울리지 않는다");

    // 2분 안에는 넘기지 않는다.
    tokio::time::sleep(alerts::TEXT_GRACE / 2).await;
    assert!(pushes.try_recv().is_err());
    // 2분이 지났는데 데스크톱이 보지 않았다: 휴대폰은 끊겨 있으니 푸시만(같은 번호). 프레임을 더 넣지 않는다.
    tokio::time::sleep(alerts::TEXT_GRACE).await;
    let job = pushes.try_recv().unwrap();
    assert_eq!(
        (job.client_id.as_str(), job.kind.as_str(), job.seq),
        ("cl-phone", "text", phone_seq)
    );
    assert!(pushes.try_recv().is_err());
    assert_eq!(boxed(&st, "cl-phone").len(), 1, "복사해 넣지 않는다");
    assert!(desk.try_recv().is_err());
}

/// 데스크톱이 2분 안에 `seen`을 보내면 아무것도 넘기지 않고, 휴대폰은 알림을 지우라는 신호를 받는다: 끊겨 있으면
/// 알림을 지우는 푸시(`clear`), 연결돼 있으면 `seen` 프레임.
#[tokio::test(start_paused = true)]
async fn a_reply_the_desktop_sees_in_time_is_not_forwarded_and_the_phone_is_told_to_clear_it() {
    for phone_connected in [false, true] {
        let (st, gw, mut pushes) = two_apps();
        let (_, mut desk) = gw.attach_for_test("cl-desk");
        gw.seen("cl-desk", 0);
        let mut phone = phone_connected.then(|| {
            let (conn, rx) = gw.attach_for_test("cl-phone");
            gw.set_presence("cl-phone", conn, Presence::Background);
            rx
        });
        gw.send(text("app:cl-desk", "곧 볼 답")).await.unwrap();
        let desk_seq = desk.recv().await.unwrap()["seq"].as_i64().unwrap();
        let phone_seq = boxed(&st, "cl-phone")[0]["seq"].as_i64().unwrap();
        if let Some(phone) = phone.as_mut() {
            assert_eq!(phone.recv().await.unwrap()["alert"], false);
        }
        assert!(pushes.try_recv().is_err());

        // 30초 뒤 데스크톱이 봤다.
        tokio::time::sleep(Duration::from_secs(30)).await;
        gw.seen("cl-desk", desk_seq);
        match phone.as_mut() {
            Some(phone) => {
                assert_eq!(
                    phone.recv().await.unwrap(),
                    json!({"type": "seen", "seq": phone_seq})
                );
                assert!(pushes.try_recv().is_err(), "연결돼 있으면 푸시 없이");
            }
            None => {
                let clear = pushes.try_recv().unwrap();
                assert_eq!(
                    (clear.client_id.as_str(), clear.kind.as_str(), clear.seq),
                    ("cl-phone", "clear", phone_seq)
                );
            }
        }
        // 넘기기는 없다.
        tokio::time::sleep(alerts::TEXT_GRACE * 2).await;
        assert!(pushes.try_recv().is_err(), "본 것은 넘기지 않는다");
        if let Some(phone) = phone.as_mut() {
            assert!(phone.try_recv().is_err());
        }
    }
}

/// 물은 앱의 자리에 따른 나머지 규칙(결정 0067의 6): 연결돼 있고 뒤에 있는 푸시 없는 앱은 울리고 때가 되면 넘긴다.
/// 자리를 비웠거나 끊긴 푸시 없는 앱이면 0066의 규칙(앞에 있는 다른 앱, 아니면 푸시를 받는 앱)으로 고른다. 취소한 앱의
/// 대화도 남은 앱에 간다.
#[tokio::test(start_paused = true)]
async fn the_asked_app_rings_first_and_the_old_rules_decide_when_it_is_away_or_gone() {
    // 뒤에 있는 데스크톱(사람은 있다): 데스크톱을 울리고 2분 뒤 휴대폰으로 넘긴다.
    let (st, gw, mut pushes) = two_apps();
    let (conn, mut desk) = gw.attach_for_test("cl-desk");
    gw.set_presence("cl-desk", conn, Presence::Background);
    gw.seen("cl-desk", 0);
    gw.send(text("app:cl-desk", "뒤에 있는 데스크톱"))
        .await
        .unwrap();
    assert!(desk.recv().await.unwrap().get("alert").is_none());
    assert_eq!(boxed(&st, "cl-phone")[0]["alert"], false);
    assert!(pushes.try_recv().is_err());
    tokio::time::sleep(alerts::TEXT_GRACE + Duration::from_secs(1)).await;
    assert_eq!(pushes.try_recv().unwrap().kind, "text");

    // 자리를 비운 데스크톱: 휴대폰이 바로 울리고 데스크톱은 놓아만 둔다. 넘기기는 없다.
    let (st, gw, mut pushes) = two_apps();
    let (conn, mut desk) = gw.attach_for_test("cl-desk");
    gw.set_presence("cl-desk", conn, Presence::Away);
    gw.send(text("app:cl-desk", "자리 비운 데스크톱"))
        .await
        .unwrap();
    assert_eq!(desk.recv().await.unwrap()["alert"], false);
    let job = pushes.try_recv().unwrap();
    assert_eq!(
        (job.client_id.as_str(), job.seq),
        (
            "cl-phone",
            boxed(&st, "cl-phone")[0]["seq"].as_i64().unwrap()
        )
    );
    assert_eq!(boxed(&st, "cl-phone")[0]["conversation"], "app:cl-desk");

    // 자리를 비운 데스크톱과 앞에 있는 휴대폰: 앞에 있는 휴대폰이 울린다(푸시 없이).
    let (st, gw, mut pushes) = two_apps();
    let (conn, mut desk) = gw.attach_for_test("cl-desk");
    gw.set_presence("cl-desk", conn, Presence::Away);
    let (_, mut phone) = gw.attach_for_test("cl-phone");
    gw.send(text("app:cl-desk", "휴대폰이 앞에")).await.unwrap();
    let on_phone = phone.recv().await.unwrap();
    assert!(on_phone.get("alert").is_none());
    assert_eq!(on_phone["conversation"], "app:cl-desk");
    assert_eq!(desk.recv().await.unwrap()["alert"], false);
    assert!(pushes.try_recv().is_err());
    assert_eq!(boxed(&st, "cl-phone").len(), 1);

    // 취소한 앱의 대화로 가는 글(오래된 카드의 닫힘 등): 남은 앱에 간다. 어느 대화의 것인지 밝힌다.
    let (st, gw, mut pushes) = two_apps();
    issue(&st, "cl-old", "app");
    st.lock()
        .unwrap()
        .revoke_client("cl-old", Utc::now())
        .unwrap();
    let (_, mut desk) = gw.attach_for_test("cl-desk");
    gw.send(text("app:cl-old", "옛 앱의 대화")).await.unwrap();
    let on_desk = desk.recv().await.unwrap();
    assert_eq!(on_desk["conversation"], "app:cl-old");
    assert!(
        on_desk.get("alert").is_none(),
        "앞에 있는 데스크톱이 울린다"
    );
    assert_eq!(boxed(&st, "cl-phone")[0]["alert"], false);
    assert!(
        boxed(&st, "cl-old").is_empty(),
        "취소한 앱의 상자에는 넣지 않는다"
    );
    assert!(pushes.try_recv().is_err());
}

/// `plan_ring`을 표로 본다(결정 0067의 6): 물은 앱(`origin`)이 앞에 있으면 그 앱 + 때가 되면 다른 푸시 앱으로 넘기기,
/// 푸시를 받는 앱이면 앞에 없어도 바로 그 앱만(넘기기 없음), 연결돼 뒤에 있는 푸시 없는 앱이면 그 앱 + 넘기기, 자리를
/// 비웠거나 끊긴 푸시 없는 앱이거나 앱 대화가 없으면(`origin` 없음, 취소한 앱) 0066의 ①~④.
#[tokio::test]
async fn plan_ring_picks_the_asked_app_first_and_falls_back_to_the_old_rules() {
    let (st, gw, _pushes) = two_apps();
    issue(&st, "cl-tab", "app");
    st.lock()
        .unwrap()
        .set_client_push("cl-tab", Some("tok-tab"), Some("android"), Utc::now())
        .unwrap();
    let apps = gw.app_clients();
    let plan = |card: bool, origin: Option<&str>| {
        let (mut ring, mut forward, grace) = gw.plan_ring(&apps, card, origin);
        ring.sort();
        forward.sort();
        (ring, forward, grace)
    };
    let v = |xs: &[&str]| xs.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let text_grace = Some(alerts::TEXT_GRACE);
    let mut keep = Vec::new();
    let mut attach = |id: &str, presence: Presence| {
        let (conn, rx) = gw.attach_for_test(id);
        gw.set_presence(id, conn, presence);
        keep.push(rx);
    };

    // 아무도 연결돼 있지 않다.
    assert_eq!(
        plan(false, Some("cl-desk")),
        (v(&["cl-phone", "cl-tab"]), v(&[]), None),
        "푸시 없는 앱이 끊겼다: ③"
    );
    assert_eq!(
        plan(false, Some("cl-phone")),
        (v(&["cl-phone"]), v(&[]), None),
        "푸시를 받는 앱이면 바로 그 앱만"
    );
    // 데스크톱이 앞에 있다.
    attach("cl-desk", Presence::Foreground);
    assert_eq!(
        plan(false, Some("cl-desk")),
        (v(&["cl-desk"]), v(&["cl-phone", "cl-tab"]), text_grace)
    );
    assert_eq!(
        plan(true, Some("cl-desk")),
        (
            v(&["cl-desk"]),
            v(&["cl-phone", "cl-tab"]),
            Some(alerts::CARD_GRACE)
        ),
        "카드는 20초"
    );
    assert_eq!(
        plan(false, Some("cl-phone")),
        (v(&["cl-phone"]), v(&[]), None),
        "물은 휴대폰이 먼저, 데스크톱이 앞에 있어도"
    );
    assert_eq!(
        plan(false, None),
        (v(&["cl-desk"]), v(&["cl-phone", "cl-tab"]), text_grace),
        "앱 모두: ①"
    );
    assert_eq!(
        plan(false, Some("cl-gone")),
        (v(&["cl-desk"]), v(&["cl-phone", "cl-tab"]), text_grace),
        "없는 앱의 대화: ①"
    );
    // 데스크톱이 뒤에 있다(사람은 있다).
    attach("cl-desk", Presence::Background);
    assert_eq!(
        plan(false, Some("cl-desk")),
        (v(&["cl-desk"]), v(&["cl-phone", "cl-tab"]), text_grace)
    );
    // 데스크톱이 자리를 비웠다: 물었어도 0066의 규칙(푸시를 받는 앱)으로.
    attach("cl-desk", Presence::Away);
    assert_eq!(
        plan(false, Some("cl-desk")),
        (v(&["cl-phone", "cl-tab"]), v(&[]), None)
    );
    // 자리를 비운 데스크톱과 앞에 있는 태블릿: 앞에 있는 태블릿이 울리고 휴대폰으로 넘긴다.
    attach("cl-tab", Presence::Foreground);
    assert_eq!(
        plan(false, Some("cl-desk")),
        (v(&["cl-tab"]), v(&["cl-phone"]), text_grace)
    );
    // 앞에 있는 휴대폰에서 물었다: 휴대폰 + 태블릿으로 넘기기.
    attach("cl-phone", Presence::Foreground);
    assert_eq!(
        plan(false, Some("cl-phone")),
        (v(&["cl-phone"]), v(&["cl-tab"]), text_grace)
    );
    // 휴대폰이 뒤에 있으면 푸시를 받는 앱이라 바로 그 앱만.
    attach("cl-phone", Presence::Background);
    assert_eq!(
        plan(false, Some("cl-phone")),
        (v(&["cl-phone"]), v(&[]), None)
    );
}

/// 알림 받을 곳 설정(앱 하나로 저장된 예전 값, 다른 앱, Discord DM)에 상관없이 앱 대화의 답은 모든 앱의 상자에
/// 들어가고, 보지 않으면 휴대폰으로 넘어간다(앱 하나만 알림 받을 곳이면 넘기지 않던 예전 동작은 없다).
#[tokio::test(start_paused = true)]
async fn replies_reach_every_app_and_forward_whatever_the_notify_setting_says() {
    for setting in [Some("cl-desk"), Some("cl-phone"), None] {
        let (st, gw, mut pushes) = two_apps();
        {
            let s = st.lock().unwrap();
            match setting {
                Some(id) => s.set_setting(keys::NOTIFY_CLIENT, id, Utc::now()).unwrap(),
                None => {
                    s.delete_setting(keys::NOTIFY_CLIENT).unwrap();
                    s.set_setting(
                        lucia_store::keys::NOTIFY_CONVERSATION,
                        "memory:dm",
                        Utc::now(),
                    )
                    .unwrap();
                }
            }
        }
        let case = format!("{setting:?}");
        let (_, mut desk) = gw.attach_for_test("cl-desk");
        gw.seen("cl-desk", 0);
        gw.send(text("app:cl-desk", "설정과 상관없는 답"))
            .await
            .unwrap();
        assert!(desk.recv().await.unwrap().get("alert").is_none(), "{case}");
        let phone_box = boxed(&st, "cl-phone");
        assert_eq!(phone_box.len(), 1, "{case}");
        assert_eq!(phone_box[0]["body"], "설정과 상관없는 답", "{case}");
        assert_eq!(phone_box[0]["conversation"], "app:cl-desk", "{case}");
        assert!(pushes.try_recv().is_err(), "{case}");
        tokio::time::sleep(alerts::TEXT_GRACE + Duration::from_secs(1)).await;
        let job = pushes.try_recv().expect(&case);
        assert_eq!(
            (job.client_id.as_str(), job.kind.as_str(), json!(job.seq)),
            ("cl-phone", "text", phone_box[0]["seq"].clone()),
            "{case}"
        );
    }
}

/// 취소하지 않은 앱이 하나도 없으면 어느 앱 대화로도 보낼 수 없다(`Closed`). 기기 역할은 앱이 아니다.
#[tokio::test]
async fn delivery_needs_at_least_one_active_app() {
    let st = store();
    issue(&st, "cl-d", "device");
    issue(&st, "cl-gone", "app");
    st.lock()
        .unwrap()
        .revoke_client("cl-gone", Utc::now())
        .unwrap();
    let (gw, _irx, _prx) = gateway(st.clone());
    for to in ["app:cl-gone", "app:cl-d", "app:*"] {
        let r = gw.send(text(to, "아무도 없다")).await;
        assert!(matches!(r, Err(ChannelError::Closed)), "{to}: {r:?}");
    }
    assert!(boxed(&st, "cl-d").is_empty(), "기기에는 상자가 없다");
    issue(&st, "cl-a", "app");
    gw.send(text("app:cl-gone", "이제 있다")).await.unwrap();
    assert_eq!(boxed(&st, "cl-a")[0]["conversation"], "app:cl-gone");
}

// ---------- `sent`: 내가 다른 기기에서 보낸 말 ----------

/// 한 앱에서 말하면 다른 앱의 상자에 번호 붙는 `sent`가 들어간다: 글, 올린 파일(같은 ID), 스티커, 고른 글 표지, 보낸 앱,
/// 그 앱의 대화, 시각. 보낸 앱·취소한 앱·기기는 받지 않고, 푸시하지 않는다. 끊긴 앱은 다시 받기로 받는다. 데몬이
/// 말을 받아 주지 못했으면(`closed`) 사본도 없다.
#[tokio::test]
async fn a_message_from_one_app_reaches_the_other_apps_as_sent_but_never_back_or_by_push() {
    use crate::expressions::testing::png;
    use crate::expressions::{Expressions, Kind, Meta};
    let (st, a, b) = pair();
    issue(&st, "cl-d", "device");
    issue(&st, "cl-old", "app");
    st.lock()
        .unwrap()
        .revoke_client("cl-old", Utc::now())
        .unwrap();
    let (gw, mut irx, _prx) = gateway(st.clone());
    let (push_tx, mut pushes) = mpsc::unbounded_channel();
    gw.set_push(push_tx);
    gw.set_push_ready(true);
    st.lock()
        .unwrap()
        .set_client_push("cl-b", Some("tok"), Some("android"), Utc::now())
        .unwrap();
    let dir = std::env::temp_dir().join(format!("lucia-gw-sent-{}", uuid::Uuid::new_v4().simple()));
    let x = Expressions::new(st.clone(), dir);
    let sticker = x
        .add(
            Kind::Sticker,
            "축하",
            &Meta {
                description: "꽃가루".into(),
                ..Meta::default()
            },
            &png(320),
            "admin",
            Utc::now(),
        )
        .unwrap();
    gw.set_expressions(x);
    let addr = serve(&gw).await;
    let mut wa = join(addr, &a).await;

    // A가 파일을 올리고, 고른 글과 함께 말한다. B는 끊겨 있다(푸시 토큰이 있는데도 `sent`는 푸시하지 않는다).
    let (status, body) = http(&gw, upload(&a, "")).await;
    assert_eq!(status, 201);
    let file = json_of(&body);
    let fid = file["id"].as_str().unwrap().to_owned();
    say(
        &mut wa,
        json!({"type": "message", "text": " 이거 봐 줘 ", "files": [fid], "id": "m1",
               "context": {"selection": "가나다", "source": {"app": "msedge.exe", "title": "제목"}}}),
    )
    .await;
    assert_eq!(hear(&mut wa).await["id"], "m1");
    let inbound = irx.recv().await.unwrap();
    assert_eq!(inbound.body, "이거 봐 줘");
    assert_eq!(inbound.attachments.len(), 1);
    // A의 말에 `accepted`가 먼저 왔고, A는 제 말의 사본을 받지 않는다(핑 뒤에 퐁이 바로 온다).
    say(&mut wa, json!({"type": "ping"})).await;
    assert_eq!(hear(&mut wa).await["type"], "pong");
    assert!(boxed(&st, "cl-a").is_empty(), "보낸 앱의 상자에는 없다");
    assert!(boxed(&st, "cl-d").is_empty(), "기기는 받지 않는다");
    assert!(boxed(&st, "cl-old").is_empty(), "취소한 앱은 받지 않는다");
    assert!(pushes.try_recv().is_err(), "`sent`는 푸시하지 않는다");

    // B의 상자: 번호 붙는 `sent` 하나.
    let kept = boxed(&st, "cl-b");
    assert_eq!(kept.len(), 1);
    let sent = &kept[0];
    assert_eq!(
        (sent["type"].as_str(), sent["seq"].as_i64()),
        (Some("sent"), Some(1))
    );
    assert_eq!(sent["body"], "이거 봐 줘", "다듬은 글");
    assert_eq!(sent["files"][0]["id"], fid.as_str(), "같은 파일 ID");
    assert_eq!(
        (
            sent["files"][0]["name"].as_str(),
            sent["files"][0]["size"].as_u64()
        ),
        (Some("memo.txt"), Some("안녕 파일".len() as u64))
    );
    assert_eq!(sent["conversation"], "app:cl-a");
    assert_eq!(sent["from"], json!({"client": "cl-a", "name": "시험 앱"}));
    assert_eq!(sent["context_note"], "[고른 글 3자 함께 보냄 · msedge.exe]");
    assert!(sent.get("sticker").is_none() && sent.get("alert").is_none());
    assert!(chrono::DateTime::parse_from_rfc3339(sent["at"].as_str().unwrap()).is_ok());

    // B가 이어 붙으면 끊긴 동안의 `sent`를 다시 받기로 받는다. 붙어 있는 동안의 말은 바로 받는다(스티커 포함).
    let mut wb = connect(addr).await;
    hello(&mut wb, json!({"type": "hello", "token": b, "resume": 0})).await;
    let replay = hear(&mut wb).await;
    assert_eq!(
        (replay["type"].as_str(), replay["replay"].as_bool()),
        (Some("sent"), Some(true))
    );
    assert_eq!(replay["files"][0]["id"], fid.as_str());
    assert_eq!(hear(&mut wb).await["type"], "resumed");
    say(
        &mut wa,
        json!({"type": "message", "text": "축하해 줘", "sticker": sticker.id, "id": "m2"}),
    )
    .await;
    assert_eq!(hear(&mut wa).await["id"], "m2");
    let live = hear(&mut wb).await;
    assert_eq!(
        (live["type"].as_str(), live["seq"].as_i64()),
        (Some("sent"), Some(2))
    );
    assert_eq!(
        live["body"], "축하해 줘",
        "스티커 설명 줄이 아니라 사용자가 쓴 글"
    );
    assert_eq!(live["sticker"], json!({"id": sticker.id, "name": "축하"}));
    assert!(live.get("files").is_none() && live.get("context_note").is_none());
    assert!(pushes.try_recv().is_err());
    // B가 한 말도 A에게 `sent`로 간다(방향은 상관없다).
    say(
        &mut wb,
        json!({"type": "message", "text": "B에서 한 말", "id": "m3"}),
    )
    .await;
    assert_eq!(hear(&mut wb).await["id"], "m3");
    let back = hear(&mut wa).await;
    assert_eq!(
        (
            back["type"].as_str(),
            back["conversation"].as_str(),
            back["from"]["client"].as_str()
        ),
        (Some("sent"), Some("app:cl-b"), Some("cl-b"))
    );

    // 데몬이 말을 받아 주지 못했으면(멈추는 중) 사본도 없다.
    irx.close();
    let before = (boxed(&st, "cl-a").len(), boxed(&st, "cl-b").len());
    say(
        &mut wa,
        json!({"type": "message", "text": "받아 주지 못할 말", "id": "m4"}),
    )
    .await;
    assert_eq!(hear(&mut wa).await["code"], "closed");
    assert_eq!((boxed(&st, "cl-a").len(), boxed(&st, "cl-b").len()), before);
}

/// 글이 없는 말(파일만, 스티커만)과 빈 말: 사본은 말이 받아들여졌을 때만, 있는 것만 싣는다.
#[tokio::test]
async fn a_file_only_message_is_carbon_copied_with_an_empty_body_and_an_empty_one_is_not() {
    let (st, a, b) = pair();
    let (gw, mut irx, _prx) = gateway(st.clone());
    let addr = serve(&gw).await;
    let mut wa = join(addr, &a).await;
    let mut wb = join(addr, &b).await;
    let (_, body) = http(&gw, upload(&a, "")).await;
    let fid = json_of(&body)["id"].as_str().unwrap().to_owned();
    say(&mut wa, json!({"type": "message", "files": [fid]})).await;
    assert_eq!(hear(&mut wa).await["type"], "accepted");
    irx.recv().await.unwrap();
    let sent = hear(&mut wb).await;
    assert_eq!(sent["type"], "sent");
    assert_eq!(sent["body"], "");
    assert_eq!(sent["files"][0]["id"], fid.as_str());
    // 짧게 둔 파일(화면 사본)은 B가 받지 못하므로 사본에 싣지 않는다(말 자체의 사본은 간다).
    let (_, body) = http(&gw, upload(&a, "&keep=short")).await;
    let short = json_of(&body)["id"].as_str().unwrap().to_owned();
    say(
        &mut wa,
        json!({"type": "message", "text": "이 화면 봐", "files": [short]}),
    )
    .await;
    assert_eq!(hear(&mut wa).await["type"], "accepted");
    irx.recv().await.unwrap();
    let sent = hear(&mut wb).await;
    assert_eq!(sent["type"], "sent");
    assert_eq!(sent["body"], "이 화면 봐");
    assert!(sent.get("files").is_none(), "{sent}");
    // 빈 말은 거절하고 사본도 없다.
    say(&mut wa, json!({"type": "message", "text": "  "})).await;
    assert_eq!(hear(&mut wa).await["code"], "empty");
    say(&mut wb, json!({"type": "ping"})).await;
    assert_eq!(hear(&mut wb).await["type"], "pong", "B에게 더 온 것이 없다");
}

// ---------- 카드·입력 중 표시·기록·파일 ----------

/// 앱 대화의 카드는 어느 앱에서 눌러도 된다: 한 앱의 대화로 보낸 카드가 모든 앱에 가고(다른 앱에는 `conversation`),
/// 다른 앱에서 누른 것이 그대로 누름 통로로 넘어간다. 앱 대화가 아닌 참조는 거절한다.
#[tokio::test]
async fn a_card_in_one_apps_conversation_can_be_pressed_from_another_app() {
    let (st, a, b) = pair();
    let (gw, _irx, mut prx) = gateway(st.clone());
    let addr = serve(&gw).await;
    let mut wa = join(addr, &a).await;
    let mut wb = join(addr, &b).await;

    let r = gw.send_card(card(conversation("cl-a"))).await.unwrap();
    assert!(r.starts_with("app:cl-a:"), "{r}");
    let on_a = hear(&mut wa).await;
    let on_b = hear(&mut wb).await;
    assert_eq!(
        (on_a["type"].as_str(), on_b["type"].as_str()),
        (Some("card"), Some("card"))
    );
    assert_eq!(
        (on_a["ref"].as_str(), on_b["ref"].as_str()),
        (Some(r.as_str()), Some(r.as_str()))
    );
    assert!(on_a.get("conversation").is_none() && on_a.get("alert").is_none());
    assert_eq!(on_b["conversation"], "app:cl-a");
    assert_eq!(on_b["alert"], false, "A가 앞에 있어 A만 울린다");

    // B가 누른다: 거절 없이 그대로 넘어간다(누른 쪽이 B로 남는다).
    say(
        &mut wb,
        json!({"type": "press", "ref": r, "action": "approve:1", "input": " 123456 "}),
    )
    .await;
    let press = prx.recv().await.unwrap();
    assert_eq!(
        (
            press.sender.as_str(),
            press.message_ref.as_str(),
            press.action.as_str()
        ),
        ("cl-b", r.as_str(), "approve:1")
    );
    assert_eq!(press.input.unwrap().expose(), "123456");
    say(&mut wb, json!({"type": "ping"})).await;
    assert_eq!(hear(&mut wb).await["type"], "pong", "오류 프레임이 없다");
    // 누른 것은 그 카드를 본 것이라 울렸던 A에게 봤다고 알린다.
    assert_eq!(
        hear(&mut wa).await,
        json!({"type": "seen", "seq": on_a["seq"]})
    );

    // 닫힘도 모든 앱에 간다. 앱 대화가 아닌 참조는 거절한다.
    gw.close_card(&r, &card(conversation("cl-a")))
        .await
        .unwrap();
    assert_eq!(hear(&mut wa).await["type"], "card_closed");
    assert_eq!(hear(&mut wb).await["type"], "card_closed");
    say(
        &mut wb,
        json!({"type": "press", "ref": "discord:1:x", "action": "approve:1"}),
    )
    .await;
    assert_eq!(hear(&mut wb).await["code"], "bad_ref");
    assert!(prx.try_recv().is_err());
}

/// 입력 중 표시는 연결된 앱 모두에 간다(상자에 넣지 않는다). 끊긴 앱·취소한 앱·기기는 오류도 받음도 없다.
#[tokio::test]
async fn typing_reaches_every_connected_app_and_is_never_stored() {
    let (st, _a, _b) = pair();
    issue(&st, "cl-c", "app");
    issue(&st, "cl-old", "app");
    st.lock()
        .unwrap()
        .revoke_client("cl-old", Utc::now())
        .unwrap();
    let (gw, _irx, _prx) = gateway(st.clone());
    let (_, mut a) = gw.attach_for_test("cl-a");
    let (_, mut b) = gw.attach_for_test("cl-b");
    let (_, mut old) = gw.attach_for_test("cl-old");
    // cl-c는 끊겨 있다.
    gw.typing(&conversation("cl-a")).await.unwrap();
    assert_eq!(a.recv().await.unwrap(), json!({"type": "typing"}));
    assert_eq!(b.recv().await.unwrap(), json!({"type": "typing"}));
    assert!(old.try_recv().is_err(), "취소한 앱은 받지 않는다");
    // 어느 앱의 대화로든 같다(앱 모두의 대화도).
    gw.typing(&conversation("cl-c")).await.unwrap();
    gw.typing(&conversation(ALL_APPS)).await.unwrap();
    for _ in 0..2 {
        assert_eq!(a.recv().await.unwrap()["type"], "typing");
        assert_eq!(b.recv().await.unwrap()["type"], "typing");
    }
    for id in ["cl-a", "cl-b", "cl-c"] {
        assert!(
            boxed(&st, id).is_empty(),
            "{id}: 입력 중은 상자에 넣지 않는다"
        );
    }
    // 라우터를 거쳐도 같다. 앱 대화가 아니면 기본 채널로.
    let (primary, handle) = lucia_channel::memory::MemoryChannel::new(2000, mpsc::channel(1).0);
    let router = RouterChannel::new(Arc::new(primary), gw.clone(), st, None);
    router.typing(&conversation("cl-c")).await.unwrap();
    assert_eq!(a.recv().await.unwrap()["type"], "typing");
    assert_eq!(b.recv().await.unwrap()["type"], "typing");
    router
        .typing(&ConversationId("memory:dm".into()))
        .await
        .unwrap();
    assert_eq!(handle.typing_count(), 1);
    assert!(a.try_recv().is_err() && b.try_recv().is_err());
}

/// 기록과 파일도 하나다: 기본 범위(`app`)가 모든 앱 대화(앱 모두, 취소한 앱의 지난 대화 포함)를 읽고, `owner`는
/// Discord DM을 더한다. 앱은 어느 앱 대화의 파일이든 받지만 다른 앱이 짧게 둔 파일은 그 앱만 받고, DM의 파일은
/// 범위 `owner_files`가 있어야 받는다(그 범위로도 서버 채널의 파일은 못 받는다).
#[tokio::test]
async fn history_and_files_are_shared_across_apps_except_short_copies_and_the_dm() {
    let (st, a, b) = pair();
    issue(&st, "cl-old", "app");
    st.lock()
        .unwrap()
        .revoke_client("cl-old", Utc::now())
        .unwrap();
    st.lock()
        .unwrap()
        .set_setting(
            lucia_store::keys::NOTIFY_CONVERSATION,
            "memory:dm",
            Utc::now(),
        )
        .unwrap();
    let (gw, _irx, _prx) = gateway(st.clone());
    for (conv, body) in [
        ("app:cl-a", "A에서"),
        ("app:cl-b", "B에서"),
        ("app:*", "모두에게"),
        ("app:cl-old", "옛 앱에서"),
        ("memory:dm", "DM에서"),
        ("discord:guild", "서버에서"),
    ] {
        add_episode(&st, conv, body);
    }
    let bodies = |v: &Value| -> Vec<String> {
        let mut list: Vec<String> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["body"].as_str().unwrap().to_owned())
            .collect();
        list.sort();
        list
    };
    let sorted = |mut v: Vec<&str>| -> Vec<String> {
        v.sort();
        v.into_iter().map(str::to_owned).collect()
    };
    let apps = ["A에서", "B에서", "모두에게", "옛 앱에서"];
    for token in [&a, &b] {
        let (status, body) = http(&gw, get("/v1/history", token)).await;
        assert_eq!(status, 200);
        assert_eq!(
            bodies(&json_of(&body)),
            sorted(apps.to_vec()),
            "기본 범위는 모든 앱 대화"
        );
        let (_, body) = http(&gw, get("/v1/history?scope=app", token)).await;
        assert_eq!(bodies(&json_of(&body)), sorted(apps.to_vec()));
        let (_, body) = http(&gw, get("/v1/history?scope=owner", token)).await;
        let mut with_dm = apps.to_vec();
        with_dm.push("DM에서");
        assert_eq!(
            bodies(&json_of(&body)),
            sorted(with_dm),
            "DM만 더하고 서버 채널은 뺀다"
        );
    }

    // 파일: A가 올린 것은 B가 받는다. A가 짧게 둔 것은 A만.
    let (_, body) = http(&gw, upload(&a, "")).await;
    let normal = json_of(&body)["id"].as_str().unwrap().to_owned();
    let (_, body) = http(&gw, upload(&a, "&keep=short")).await;
    let short = json_of(&body)["id"].as_str().unwrap().to_owned();
    let path = |id: &str| format!("/v1/files/{id}");
    let (status, content) = http(&gw, get(&path(&normal), &b)).await;
    assert_eq!((status, content.as_slice()), (200, "안녕 파일".as_bytes()));
    assert_eq!(
        http(&gw, get(&path(&short), &a)).await.0,
        200,
        "제 사본은 받는다"
    );
    assert_eq!(
        http(&gw, get(&path(&short), &b)).await.0,
        404,
        "남이 짧게 둔 파일"
    );
    // 데몬이 앱 대화로 보낸 파일(그 대화로 보냄)도, 앱 모두에 둔 것도 모든 앱이 받는다.
    let files = gw.files.get().unwrap().clone();
    gw.send(Outbound::Files {
        conversation: conversation("cl-a"),
        body: "정리했어요".into(),
        files: vec![lucia_core::OutFile {
            name: "정리.md".into(),
            bytes: b"# hi".to_vec(),
            id: None,
        }],
    })
    .await
    .unwrap();
    let sent_id = boxed(&st, "cl-b")[0]["files"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(http(&gw, get(&path(&sent_id), &b)).await.0, 200);
    let everyone = files
        .save("out", "모두.md", b"x", Some("app:*"), Utc::now())
        .unwrap();
    assert_eq!(http(&gw, get(&path(&everyone.id), &b)).await.0, 200);
    // 다른 대화의 파일: DM은 범위가 있어야, 서버 채널은 범위가 있어도 안 된다.
    let dm = files
        .save(
            "in",
            "영수증.txt",
            b"receipt",
            Some("memory:dm"),
            Utc::now(),
        )
        .unwrap();
    let guild = files
        .save("in", "서버.txt", b"x", Some("discord:guild"), Utc::now())
        .unwrap();
    for token in [&a, &b] {
        assert_eq!(http(&gw, get(&path(&dm.id), token)).await.0, 404);
    }
    st.lock()
        .unwrap()
        .set_client_scopes("cl-b", &[scopes::OWNER_FILES.to_owned()])
        .unwrap();
    assert_eq!(http(&gw, get(&path(&dm.id), &b)).await.0, 200);
    assert_eq!(
        http(&gw, get(&path(&dm.id), &a)).await.0,
        404,
        "범위가 없는 앱은 여전히 못 받는다"
    );
    assert_eq!(http(&gw, get(&path(&guild.id), &b)).await.0, 404);
    // 범위는 DM에만 보태진다: 다른 앱이 짧게 둔 파일은 범위가 있어도 못 받는다.
    assert_eq!(http(&gw, get(&path(&short), &b)).await.0, 404);
}

// ---------- 라우터와 알림 받을 곳 ----------

/// 한 앱의 대화로 가는 글이 그 앱이 끊겨 있고 푸시도 없어도, 다른 앱이 닿으면(연결됐거나 푸시를 받는다) Discord DM이
/// 아니라 앱으로 간다. 닿는 앱이 하나도 없을 때만 DM으로 돌린다.
#[tokio::test]
async fn router_delivers_an_unreachable_apps_conversation_to_the_apps_and_falls_back_to_the_dm_only_when_none_is_reachable()
 {
    let (st, gw, mut pushes) = two_apps();
    let dm = ConversationId("memory:dm".into());
    st.lock()
        .unwrap()
        .set_setting(lucia_store::keys::NOTIFY_CONVERSATION, &dm.0, Utc::now())
        .unwrap();
    let (itx, _irx) = mpsc::channel(4);
    let (ptx, _prx) = mpsc::channel(4);
    let (primary, handle) = lucia_channel::memory::MemoryChannel::with_buttons(2000, itx, ptx);
    let router = RouterChannel::new(Arc::new(primary), gw.clone(), st.clone(), None);
    let to_desk = conversation("cl-desk");

    // 데스크톱은 끊겼고 푸시도 없다. 휴대폰은 푸시로 닿는다: 앱으로 간다(휴대폰이 울린다).
    assert!(!gw.reachable("cl-desk") && gw.reachable("cl-phone") && gw.reachable(ALL_APPS));
    router
        .send(text(&to_desk.0, "데스크톱 대화의 답"))
        .await
        .unwrap();
    assert!(handle.take_sent().is_empty(), "DM으로 돌리지 않는다");
    let phone_box = boxed(&st, "cl-phone");
    assert_eq!(phone_box[0]["body"], "데스크톱 대화의 답");
    assert_eq!(phone_box[0]["conversation"], "app:cl-desk");
    assert_eq!(
        boxed(&st, "cl-desk")[0]["alert"],
        false,
        "끊긴 데스크톱은 울릴 수 없다"
    );
    let job = pushes.try_recv().unwrap();
    assert_eq!(
        (job.client_id.as_str(), job.kind.as_str()),
        ("cl-phone", "text")
    );
    // 카드와 닫힘도 같다.
    let r = router.send_card(card(to_desk.clone())).await.unwrap();
    assert!(r.starts_with("app:cl-desk:"), "{r}");
    assert!(handle.cards().is_empty());
    let job = pushes.try_recv().unwrap();
    assert!(job.high && job.kind == "card");
    router.close_card(&r, &card(to_desk.clone())).await.unwrap();
    assert_eq!(
        boxed(&st, "cl-phone").last().unwrap()["type"],
        "card_closed"
    );
    assert!(handle.cards().is_empty());
    // 휴대폰이 연결돼 앞에 있으면 프레임으로 바로, 푸시 없이.
    let (_, mut phone) = gw.attach_for_test("cl-phone");
    router
        .send(text(&to_desk.0, "연결된 휴대폰"))
        .await
        .unwrap();
    assert_eq!(phone.recv().await.unwrap()["body"], "연결된 휴대폰");
    assert!(pushes.try_recv().is_err());

    // 닿는 앱이 하나도 없다(휴대폰도 끊기고 푸시를 쓸 수 없다): 알림 대화로.
    gw.disconnect("cl-phone");
    gw.set_push_ready(false);
    assert!(!gw.reachable(ALL_APPS));
    router
        .send(text(&to_desk.0, "아무도 없을 때"))
        .await
        .unwrap();
    assert_eq!(
        handle.take_sent(),
        vec![(dm.clone(), "아무도 없을 때".into())]
    );
    let r = router.send_card(card(to_desk.clone())).await.unwrap();
    assert!(!r.starts_with("app:"), "카드도 기본 채널로: {r}");
    assert_eq!(handle.cards().len(), 1);
    assert_eq!(boxed(&st, "cl-phone").len(), 4, "앱 상자는 그대로");
}

/// `welcome`: 한 대화라는 표시와, 알림을 앱으로 받는 설정은 모든 앱에 켜짐으로 보인다. 예전에 앱 하나로 저장한 값도
/// 앱 모두로 읽는다.
#[tokio::test]
async fn welcome_announces_the_shared_conversation_and_reads_a_single_notify_app_as_all() {
    let (st, a, b) = pair();
    let (gw, _irx, _prx) = gateway(st.clone());
    let addr = serve(&gw).await;
    let notify_target = |w: &Value| w["notify_target"].as_bool().unwrap();

    // 알림 받을 앱을 정하지 않았다: 아무 앱도 알림 대상이 아니다.
    let mut ws = connect(addr).await;
    let w = hello(&mut ws, json!({"type": "hello", "token": a})).await;
    assert_eq!(w["capabilities"]["shared_conversation"], true);
    assert!(!notify_target(&w));
    assert_eq!(notify_client(&st.lock().unwrap()), None);

    // 앱 하나(`cl-a`)로 저장된 예전 값: 앱 모두로 읽어 모든 앱이 알림 대상이다.
    st.lock()
        .unwrap()
        .set_setting(keys::NOTIFY_CLIENT, "cl-a", Utc::now())
        .unwrap();
    assert_eq!(
        notify_client(&st.lock().unwrap()).as_deref(),
        Some(ALL_APPS)
    );
    assert_eq!(
        st.lock().unwrap().notify_conversation().as_deref(),
        Some("app:*")
    );
    for token in [&a, &b] {
        let mut ws = connect(addr).await;
        let w = hello(&mut ws, json!({"type": "hello", "token": token})).await;
        assert_eq!(w["capabilities"]["shared_conversation"], true);
        assert!(notify_target(&w), "{}", w["client"]);
        assert_eq!(
            w["conversation"],
            format!("app:{}", w["client"].as_str().unwrap())
        );
    }
    // 저장된 앱이 취소돼도 남은 앱이 있으면 앱 모두다. 앱이 하나도 없으면 앱으로 받는 설정이 아니다.
    st.lock()
        .unwrap()
        .revoke_client("cl-a", Utc::now())
        .unwrap();
    assert_eq!(
        notify_client(&st.lock().unwrap()).as_deref(),
        Some(ALL_APPS)
    );
    st.lock()
        .unwrap()
        .revoke_client("cl-b", Utc::now())
        .unwrap();
    assert_eq!(notify_client(&st.lock().unwrap()), None);
}
