//! 승인 데스크: 메모리 채널과 가짜 메일러로 승인·거절·만료·중복 누름·실패·재시작을 확인한다.

use std::sync::atomic::{AtomicUsize, Ordering};

use lucia_channel::memory::{MemoryChannel, MemoryHandle};
use lucia_core::{Allowlist, UserId};

use super::*;

const CONV: &str = "dm-1";

#[derive(Default)]
struct FakeMailer {
    sent: Arc<AtomicUsize>,
    fail: Option<SendError>,
}

impl Mailer for FakeMailer {
    async fn send(&self, _mail: &OutgoingMail) -> Result<String, SendError> {
        self.sent.fetch_add(1, Ordering::SeqCst);
        match &self.fail {
            Some(err) => Err(err.clone()),
            None => Ok("gmail-1".into()),
        }
    }
}

/// 메신저가 보여 주는 글: 문장부호 앞 `\\`를 푼다.
fn shown(card: &str) -> String {
    let mut out = String::new();
    let mut chars = card.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek().is_some_and(|n| n.is_ascii_punctuation()) {
            out.push(chars.next().unwrap());
        } else {
            out.push(c);
        }
    }
    out
}

fn mail() -> OutgoingMail {
    OutgoingMail {
        from: "me@example.com".into(),
        from_name: None,
        reply_to: None,
        to: vec!["prof@example.edu".into()],
        cc: vec![],
        subject: "과제 제출 확인".into(),
        body: "교수님, 과제를 제출했습니다.".into(),
        in_reply_to: None,
        references: None,
        thread_id: None,
    }
}

struct Setup {
    desk: ApprovalDesk<MemoryChannel>,
    handle: MemoryHandle,
    sent: Arc<AtomicUsize>,
    presses: mpsc::Receiver<ButtonPress>,
}

fn setup(fail: Option<SendError>, conversation: Option<&str>) -> Setup {
    let (tx, _rx) = mpsc::channel(4);
    let (ptx, prx) = mpsc::channel(4);
    let (adapter, handle) = MemoryChannel::with_buttons(2000, tx, ptx);
    let mailer = FakeMailer {
        fail,
        ..Default::default()
    };
    let sent = mailer.sent.clone();
    Setup {
        desk: ApprovalDesk::new(
            Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
            Arc::new(adapter),
            Access::Fixed(Allowlist::new().allow("memory", "42", UserId("me".into()))),
            chrono_tz::Asia::Seoul,
            conversation.map(|c| ConversationId(c.into())),
        )
        .with_builtin_kinds(mailer, None, None),
        handle,
        sent,
        presses: prx,
    }
}

impl Setup {
    fn request(&self, ttl_minutes: i64) -> String {
        request_mail_send(
            &self.desk.store.lock().unwrap(),
            &mail(),
            Some("chat-1"),
            chrono::Duration::minutes(ttl_minutes),
            Utc::now(),
        )
        .unwrap()
    }

    async fn press(&mut self, sender: &str, card: &str, action: &str) {
        self.handle.press(sender, CONV, card, action).await.unwrap();
        let p = self.presses.recv().await.unwrap();
        self.desk.handle_press(p).await;
    }

    fn status(&self, id: &str) -> ApprovalStatus {
        self.desk
            .store
            .lock()
            .unwrap()
            .approval(id)
            .unwrap()
            .unwrap()
            .status
    }
}

#[tokio::test]
async fn approve_sends_once_and_closes_the_card() {
    let mut s = setup(None, Some(CONV));
    let id = s.request(60);
    s.desk.post_card(&id).await;
    let cards = s.handle.cards();
    assert_eq!(cards.len(), 1);
    let body = shown(&cards[0].text());
    assert_eq!(cards[0].card.tone, CardTone::Pending);
    assert!(
        cards[0].card.headline.contains("과제 제출 확인"),
        "알림 미리보기에 제목"
    );
    for must in [
        "prof@example.edu",
        "me@example.com",
        "과제 제출 확인",
        "과제를 제출했습니다",
    ] {
        assert!(body.contains(must), "카드에 {must}");
    }
    let card = cards[0].message_ref.clone();

    s.press("42", &card, &action(&id, true)).await;
    s.press("42", &card, &action(&id, true)).await;
    assert_eq!(
        s.sent.load(Ordering::SeqCst),
        1,
        "두 번 눌러도 한 번만 보낸다"
    );
    assert_eq!(s.status(&id), ApprovalStatus::Sent);
    let closed = s.handle.cards()[0].clone();
    let done = closed.closed.clone().unwrap();
    assert_eq!(done.headline, "✅ 보냈습니다.");
    assert_eq!(done.tone, CardTone::Success);
    assert!(
        done.buttons.is_empty() && closed.actions.is_empty(),
        "버튼을 없앤다"
    );
    assert_eq!(
        (&done.title, &done.fields, &done.body),
        (&closed.card.title, &closed.card.fields, &closed.card.body),
        "닫아도 같은 제목·칸·본문(어댑터가 같은 모양으로 고친다)"
    );

    let store = s.desk.store.lock().unwrap();
    let kinds: Vec<String> = store
        .recent_activity(10)
        .unwrap()
        .into_iter()
        .map(|a| a.kind)
        .collect();
    assert_eq!(kinds, ["mail_sent", "approval_requested"]);
    let episodes = store.search_episodes("승인 결과", 5).unwrap();
    assert_eq!(episodes.len(), 1, "에이전트가 결과를 알 수 있게 남긴다");
    assert_eq!(episodes[0].conversation.as_deref(), Some(CONV));
}

#[tokio::test]
async fn deny_and_strangers_do_not_send() {
    let mut s = setup(None, Some(CONV));
    let id = s.request(60);
    s.desk.post_card(&id).await;
    let card = s.handle.cards()[0].message_ref.clone();

    s.press("7", &card, &action(&id, true)).await;
    assert_eq!(
        s.status(&id),
        ApprovalStatus::Pending,
        "소유자가 아니면 무시"
    );

    s.press("42", &card, &action(&id, false)).await;
    assert_eq!(s.status(&id), ApprovalStatus::Denied);
    s.press("42", &card, &action(&id, true)).await;
    assert_eq!(
        s.sent.load(Ordering::SeqCst),
        0,
        "거절한 뒤 승인은 효과가 없다"
    );
    let closed = s.handle.cards()[0].closed.clone().unwrap();
    assert!(closed.headline.contains("거절"));
    assert_eq!(closed.tone, CardTone::Neutral);
}

#[tokio::test]
async fn expired_requests_are_closed_and_never_sent() {
    let mut s = setup(None, Some(CONV));
    let id = s.request(-1);
    s.desk.post_card(&id).await;
    let card = s.handle.cards()[0].message_ref.clone();
    s.press("42", &card, &action(&id, true)).await;
    assert_eq!(s.sent.load(Ordering::SeqCst), 0);
    assert_eq!(s.status(&id), ApprovalStatus::Expired);
    assert!(s.handle.cards()[0].text().contains("만료"));

    let other = s.request(-1);
    s.desk.post_card(&other).await;
    s.desk.sweep().await;
    assert_eq!(s.status(&other), ApprovalStatus::Expired);
    let closed = s.handle.cards()[1].closed.clone().unwrap();
    assert!(closed.headline.contains("만료"));
    assert_eq!(closed.tone, CardTone::Neutral);
}

#[tokio::test]
async fn failures_are_reported_not_retried() {
    let mut s = setup(Some(SendError::Unknown("연결 끊김".into())), Some(CONV));
    let id = s.request(60);
    s.desk.post_card(&id).await;
    let card = s.handle.cards()[0].message_ref.clone();
    s.press("42", &card, &action(&id, true)).await;
    s.press("42", &card, &action(&id, true)).await;
    assert_eq!(s.sent.load(Ordering::SeqCst), 1);
    assert_eq!(s.status(&id), ApprovalStatus::Failed);
    let closed = s.handle.cards()[0].closed.clone().unwrap();
    assert!(closed.headline.contains("보낸편지함"));
    assert_eq!(closed.tone, CardTone::Warning, "결과 모름은 확인 필요");
}

#[tokio::test]
async fn restart_closes_interrupted_sends_and_posts_missing_cards() {
    let s = setup(None, Some(CONV));
    let interrupted = s.request(60);
    s.desk.post_card(&interrupted).await;
    s.desk
        .store
        .lock()
        .unwrap()
        .decide_approval(&interrupted, true, Utc::now())
        .unwrap();
    let unposted = s.request(60);

    s.desk.recover().await;
    assert_eq!(s.status(&interrupted), ApprovalStatus::Failed);
    assert_eq!(
        s.sent.load(Ordering::SeqCst),
        0,
        "멈춘 발송을 다시 보내지 않는다"
    );
    let cards = s.handle.cards();
    assert!(cards[0].text().contains("결과를 모릅니다"));
    assert_eq!(cards.len(), 2, "카드가 없던 대기 요청은 다시 올린다");
    assert_eq!(s.status(&unposted), ApprovalStatus::Pending);
}

#[tokio::test]
async fn no_conversation_closes_the_request() {
    let s = setup(None, None);
    let id = s.request(60);
    s.desk.post_card(&id).await;
    assert_eq!(s.status(&id), ApprovalStatus::Denied);
    assert!(s.handle.cards().is_empty());
    assert_eq!(
        s.desk.store.lock().unwrap().recent_alerts(5).unwrap().len(),
        1
    );
}

#[test]
fn requests_are_validated_and_actions_round_trip() {
    let store = Store::open_in_memory().unwrap();
    let mut bad = mail();
    bad.subject = "x\r\nBcc: evil@example.com".into();
    assert!(request_mail_send(&store, &bad, None, chrono::Duration::hours(1), Utc::now()).is_err());
    assert!(store.recent_approvals(5).unwrap().is_empty());

    assert_eq!(parse_action(&action("abc", true)), Some(("abc", true)));
    assert_eq!(parse_action(&action("abc", false)), Some(("abc", false)));
    assert_eq!(parse_action("approval::approve"), None);
    assert_eq!(parse_action("other:abc:approve"), None);
}

/// 위험 줄(결정 0028): 처음 보내는 주소와 개인정보 패턴을 요청 때 찾아 카드 맨 위에 고정한다.
/// 같은 주소로 보낸 적이 있으면 "처음"이 아니다.
#[test]
fn mail_card_carries_a_risk_line_fixed_at_request_time() {
    let store = Store::open_in_memory().unwrap();
    let mut m = mail();
    m.body = "제 번호는 010-1234-5678입니다.".into();
    let id = request_mail_send(&store, &m, None, chrono::Duration::hours(1), Utc::now()).unwrap();
    let a = store.approval(&id).unwrap().unwrap();
    let card = mail_card(
        &a,
        ConversationId(CONV.into()),
        chrono_tz::Asia::Seoul,
        CardState::Open,
    );
    assert_eq!(card.fields[0].name, "확인할 점");
    assert_eq!(card.fields[0].value, "⚠ 처음 보내는 주소 · 전화번호");
    // 보낸 기록이 생기면 같은 주소는 처음이 아니다. 개인정보가 없으면 칸도 없다.
    store.decide_approval(&id, true, Utc::now()).unwrap();
    store
        .finish_approval(&id, true, "보냄", Utc::now())
        .unwrap();
    let id2 = request_mail_send(
        &store,
        &mail(),
        None,
        chrono::Duration::hours(1),
        Utc::now(),
    )
    .unwrap();
    let a2 = store.approval(&id2).unwrap().unwrap();
    let card2 = mail_card(
        &a2,
        ConversationId(CONV.into()),
        chrono_tz::Asia::Seoul,
        CardState::Open,
    );
    assert_ne!(card2.fields[0].name, "확인할 점");
}

#[test]
fn card_shows_model_text_verbatim() {
    let store = Store::open_in_memory().unwrap();
    let mut m = mail();
    m.to = vec!["first_last*x@example.com".into()];
    m.subject = "||숨김|| 제목".into();
    m.body =
        "안녕하세요 [여기](https://evil.example/?q=1) @everyone\n# 큰 글씨\n> 인용 `코드`".into();
    let id = request_mail_send(&store, &m, None, chrono::Duration::hours(1), Utc::now()).unwrap();
    let a = store.approval(&id).unwrap().unwrap();
    let card = lucia_channel::card_text(&mail_card(
        &a,
        ConversationId(CONV.into()),
        chrono_tz::Asia::Seoul,
        CardState::Open,
    ));
    // 마크다운이 먹지 않는다: 이스케이프되지 않은 표시가 남지 않는다.
    let unescaped_pairs = |needle: &str| {
        card.match_indices(needle)
            .any(|(i, _)| i == 0 || !card[..i].ends_with('\\'))
    };
    assert!(!unescaped_pairs("||"), "스포일러: {card}");
    assert!(!unescaped_pairs("]("), "가린 링크: {card}");
    assert!(!unescaped_pairs("@everyone"), "멘션: {card}");
    // 이스케이프를 풀면 원문 그대로(사람이 보는 것 = 보내는 것).
    let shown = shown(&card);
    for part in [m.to[0].as_str(), m.subject.as_str(), m.body.as_str()] {
        assert!(
            shown.contains(part),
            "{part} 이(가) 그대로 보여야 한다: {shown}"
        );
    }
}

#[test]
fn card_shows_alias_name_and_reply_to() {
    let store = Store::open_in_memory().unwrap();
    let mut m = mail();
    m.from_name = Some("지금 한강은".into());
    m.reply_to = Some("reply@example.com".into());
    let id = request_mail_send(&store, &m, None, chrono::Duration::hours(1), Utc::now()).unwrap();
    let card = mail_card(
        &store.approval(&id).unwrap().unwrap(),
        ConversationId(CONV.into()),
        chrono_tz::Asia::Seoul,
        CardState::Open,
    );
    let value = |name: &str| {
        card.fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| shown(&f.value))
    };
    assert_eq!(
        value("보내는 주소").as_deref(),
        Some("지금 한강은 <me@example.com>")
    );
    assert_eq!(
        value("답장 받을 주소").as_deref(),
        Some("reply@example.com")
    );
    assert_eq!(value("받는 사람").as_deref(), Some("prof@example.edu"));
    assert_eq!(value("참조"), None, "참조가 없으면 칸도 없다");
    assert_eq!(card.buttons.len(), 2);
    assert!(card.footer.as_deref().unwrap().contains("답이 없으면"));
}

/// 카드의 고정 문구 자리(제목, 칸 이름, 꼬리말)에는 모델이 쓴 글이 들어가지 않는다.
#[test]
fn model_text_stays_out_of_fixed_slots() {
    let store = Store::open_in_memory().unwrap();
    let mut m = mail();
    m.from_name = Some("FROMNAME".into());
    m.subject = "SUBJECT".into();
    m.body = "BODY".into();
    let id = request_mail_send(&store, &m, None, chrono::Duration::hours(1), Utc::now()).unwrap();
    let card = mail_card(
        &store.approval(&id).unwrap().unwrap(),
        ConversationId(CONV.into()),
        chrono_tz::Asia::Seoul,
        CardState::Open,
    );
    let fixed = format!(
        "{}{}{}",
        card.title,
        card.footer.clone().unwrap_or_default(),
        card.fields
            .iter()
            .map(|f| f.name.as_str())
            .collect::<String>()
    );
    for model in ["FROMNAME", "SUBJECT", "BODY", "example"] {
        assert!(
            !fixed.contains(model),
            "{model} 이(가) 고정 자리에: {fixed}"
        );
    }
}

/// 검증을 통과하는 가장 큰 메일도 Discord 카드 한도 안에 들어가고, 본문이 빠짐없이 보인다.
#[test]
fn largest_valid_mail_fits_discord_cards() {
    use lucia_channel::discord::card::{
        CONTENT_MAX, DESCRIPTION_MAX, EMBED_TOTAL_MAX, EMBEDS_PER_MESSAGE, FIELD_VALUE_MAX,
        embed_chars, render, shown_sections,
    };
    use lucia_precheck::google::assist::{MAX_RECIPIENTS, MAX_SEND_BODY_CHARS};
    let store = Store::open_in_memory().unwrap();
    let mut m = mail();
    let addr = |i: usize| {
        format!(
            "{}.{i}@{}.example.com",
            "a_b-c".repeat(12),
            "d-e".repeat(20)
        )
    };
    m.to = (0..MAX_RECIPIENTS / 2).map(addr).collect();
    m.cc = (MAX_RECIPIENTS / 2..MAX_RECIPIENTS).map(addr).collect();
    m.from_name = Some("*".repeat(100));
    m.reply_to = Some(addr(99));
    m.subject = "|".repeat(250);
    m.body = "*_".repeat(MAX_SEND_BODY_CHARS / 2);
    let id = request_mail_send(&store, &m, None, chrono::Duration::hours(1), Utc::now()).unwrap();
    let a = store.approval(&id).unwrap().unwrap();
    for state in [
        CardState::Open,
        CardState::Closed {
            status: &format!("❌ {}", "오류 ".repeat(800)),
            tone: CardTone::Danger,
        },
    ] {
        let messages = render(&mail_card(
            &a,
            ConversationId("discord:1".into()),
            chrono_tz::Asia::Seoul,
            state,
        ));
        for msg in &messages {
            assert!(msg.content.chars().count() <= CONTENT_MAX);
            assert!(msg.embeds.len() <= EMBEDS_PER_MESSAGE);
            assert!(msg.embeds.iter().map(embed_chars).sum::<usize>() <= EMBED_TOTAL_MAX);
            for e in &msg.embeds {
                assert!(
                    e.description
                        .as_deref()
                        .is_none_or(|d| d.chars().count() <= DESCRIPTION_MAX)
                );
                assert!(
                    e.fields
                        .iter()
                        .all(|f| f.value.chars().count() <= FIELD_VALUE_MAX)
                );
            }
        }
        let body = shown_sections(&messages)
            .into_iter()
            .find(|(l, _)| l == "본문")
            .map(|(_, t)| shown(&t));
        assert_eq!(body.as_deref(), Some(m.body.as_str()), "본문이 빠짐없이");
    }
}

/// 시험용 재로그인 실행기: 부른 사이트를 기록하고 정해 둔 결과를 돌려준다.
struct FakeLogin {
    sites: Mutex<Vec<String>>,
    result: Result<String, String>,
}

impl SiteLoginExec for FakeLogin {
    fn login_saved<'a>(
        &'a self,
        site: &'a str,
    ) -> crate::browser::BoxFut<'a, Result<crate::browser::logins::SavedLogin, String>> {
        self.sites.lock().unwrap().push(site.to_owned());
        let r = self
            .result
            .clone()
            .map(crate::browser::logins::SavedLogin::LoggedIn);
        Box::pin(async move { r })
    }

    fn submit_code<'a>(
        &'a self,
        site: &'a str,
        code: &'a str,
    ) -> crate::browser::BoxFut<'a, Result<String, String>> {
        self.sites.lock().unwrap().push(format!("{site}:{code}"));
        let r = self.result.clone();
        Box::pin(async move { r })
    }

    fn submit_input(
        self: Arc<Self>,
        token: &str,
        username: String,
        password: String,
        save: bool,
    ) -> Result<String, String> {
        // 비밀번호는 길이만 남긴다(시험이 값이 흘러가지 않았는지 다른 곳에서 본다).
        self.sites.lock().unwrap().push(format!(
            "input:{token}:{username}:{}:{save}",
            password.chars().count()
        ));
        self.result.clone()
    }

    fn cancel_input(&self, token: &str) {
        self.sites.lock().unwrap().push(format!("cancel:{token}"));
    }
}

fn login_request(s: &Setup) -> String {
    let id = Uuid::new_v4().simple().to_string();
    s.desk
        .store
        .lock()
        .unwrap()
        .create_approval(
            &NewApproval {
                id: id.clone(),
                kind: KIND_SITE_LOGIN.into(),
                payload: serde_json::json!({
                    "site": "quasarzone",
                    "site_name": "퀘이사존",
                    "host": "quasarzone.com",
                    "username": "me_1",
                    "reason": "세션이 끝났다.",
                }),
                summary: "로그인 → 퀘이사존 (me_1)".into(),
                run_id: None,
                expires_at: Utc::now() + chrono::Duration::minutes(30),
            },
            Utc::now(),
        )
        .unwrap();
    id
}

/// 재로그인 승인: 로그인 카드 모양, 승인하면 한 번 실행, 거절하면 실행하지 않는다. 메일 경로와 섞이지 않는다.
#[tokio::test]
async fn site_login_approvals_use_their_own_card_and_executor() {
    let mut s = setup(None, Some(CONV));
    let exec = Arc::new(FakeLogin {
        sites: Mutex::default(),
        result: Ok("로그인했습니다.".into()),
    });
    s.desk.set_kind(SiteLoginKind {
        exec: Some(exec.clone()),
    });

    let id = login_request(&s);
    s.desk.post_card(&id).await;
    let card = s.handle.cards()[0].clone();
    assert_eq!(card.card.title, "사이트 로그인 승인 요청");
    assert!(card.card.headline.contains("퀘이사존"));
    assert_eq!(card.card.buttons[0].label, "로그인");
    let user = card
        .card
        .fields
        .iter()
        .find(|f| f.name == "아이디")
        .unwrap();
    assert_eq!(shown(&user.value), "me_1", "칸 값은 이스케이프된다");

    s.press("42", &card.message_ref, &action(&id, true)).await;
    s.press("42", &card.message_ref, &action(&id, true)).await;
    assert_eq!(*exec.sites.lock().unwrap(), ["quasarzone"], "한 번만");
    assert_eq!(s.sent.load(Ordering::SeqCst), 0, "메일은 보내지 않는다");
    let closed = s.handle.cards()[0].closed.clone().unwrap();
    assert_eq!(closed.tone, CardTone::Success);
    assert!(closed.headline.contains("로그인했습니다"));
    assert_eq!(closed.title, card.card.title, "닫아도 같은 카드");

    let other = login_request(&s);
    s.desk.post_card(&other).await;
    let card = s.handle.cards()[1].message_ref.clone();
    s.press("42", &card, &action(&other, false)).await;
    assert_eq!(
        exec.sites.lock().unwrap().len(),
        1,
        "거절하면 실행하지 않는다"
    );
    let closed = s.handle.cards()[1].closed.clone().unwrap();
    assert!(
        closed.headline.contains("로그인하지 않았습니다"),
        "{}",
        closed.headline
    );

    // 실행기가 없으면(브라우저 꺼짐) 실패로 닫는다.
    s.desk.set_kind(SiteLoginKind { exec: None });
    let third = login_request(&s);
    s.desk.post_card(&third).await;
    let card = s.handle.cards()[2].message_ref.clone();
    s.press("42", &card, &action(&third, true)).await;
    assert_eq!(s.status(&third), ApprovalStatus::Failed);
    assert_eq!(
        s.handle.cards()[2].closed.clone().unwrap().tone,
        CardTone::Danger
    );
}

#[derive(Default)]
struct FakeBrowserExec {
    ran: Mutex<Vec<String>>,
    closed: Mutex<Vec<(String, String)>>,
}

impl BrowserActionExec for FakeBrowserExec {
    fn run_approved<'a>(
        &'a self,
        id: &'a str,
        _payload: &'a serde_json::Value,
    ) -> crate::browser::BoxFut<'a, Result<String, String>> {
        self.ran.lock().unwrap().push(id.to_owned());
        Box::pin(async {
            Ok("실행했습니다 → 글 (https://www.example.com/board/9)".to_owned())
        })
    }

    fn closed(&self, id: &str, why: &str) {
        self.closed
            .lock()
            .unwrap()
            .push((id.to_owned(), why.to_owned()));
    }
}

fn browser_request(s: &Setup, ttl: chrono::Duration) -> String {
    use crate::browser::fake::element;
    use crate::browser::{Act, Expect, FormInfo};
    let id = Uuid::new_v4().simple().to_string();
    let mut el = element("e4", "submit", "글 등록 **굵게** ||가림||");
    el.form = Some(FormInfo {
        method: "post".into(),
        action: "https://www.example.com/write".into(),
        fields: vec![("제목".into(), "안녕 @everyone".into())],
        hidden: 2,
        password: false,
    });
    s.desk
        .store
        .lock()
        .unwrap()
        .create_approval(
            &NewApproval {
                id: id.clone(),
                kind: KIND_BROWSER_ACTION.into(),
                payload: serde_json::json!({
                    "act": Act::Click { r: "e4".into() },
                    "expect": Expect { url: "https://www.example.com/board/1".into(), element: el, approved: true },
                    "host": "www.example.com",
                    "reason": "POST 폼 제출(외부에 남는 쓰기일 수 있다)",
                }),
                summary: "브라우저 클릭 제출 단추 “글 등록”".into(),
                run_id: None,
                expires_at: Utc::now() + ttl,
            },
            Utc::now(),
        )
        .unwrap();
    id
}

/// 브라우저 동작 승인(결정 0008): 카드에 사이트·대상·보낼 곳·폼 값이 이스케이프돼 보이고, 승인하면 한 번 실행,
/// 거절·만료는 기다리는 도구에 알린다.
#[tokio::test]
async fn browser_action_approvals_show_the_form_and_notify_waiters() {
    let mut s = setup(None, Some(CONV));
    let exec = Arc::new(FakeBrowserExec::default());
    s.desk.set_kind(BrowserActionKind {
        exec: Some(exec.clone()),
    });

    let id = browser_request(&s, chrono::Duration::minutes(5));
    s.desk.post_card(&id).await;
    let card = s.handle.cards()[0].clone();
    assert_eq!(card.card.title, "브라우저 동작 승인 요청");
    assert_eq!(card.card.buttons[0].label, "실행");
    let field = |name: &str| {
        card.card
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.value.clone())
            .unwrap_or_default()
    };
    assert_eq!(shown(&field("사이트")), "www.example.com");
    assert_eq!(field("동작"), "클릭");
    assert!(shown(&field("보낼 곳")).ends_with("https://www.example.com/write"));
    assert!(
        field("대상").contains("\\*\\*굵게"),
        "마크다운이 꺼진다: {}",
        field("대상")
    );
    assert!(card.card.body.contains("숨은 칸 2개"), "{}", card.card.body);
    assert!(
        card.card.body.contains("\\@everyone"),
        "멘션이 꺼진다: {}",
        card.card.body
    );

    s.press("42", &card.message_ref, &action(&id, true)).await;
    s.press("42", &card.message_ref, &action(&id, true)).await;
    assert_eq!(
        *exec.ran.lock().unwrap(),
        std::slice::from_ref(&id),
        "한 번만"
    );
    let closed = s.handle.cards()[0].closed.clone().unwrap();
    assert_eq!(closed.tone, CardTone::Success);
    assert!(closed.headline.contains("실행했습니다"));

    let other = browser_request(&s, chrono::Duration::minutes(5));
    s.desk.post_card(&other).await;
    let card = s.handle.cards()[1].message_ref.clone();
    s.press("42", &card, &action(&other, false)).await;
    assert_eq!(
        exec.ran.lock().unwrap().len(),
        1,
        "거절하면 실행하지 않는다"
    );
    assert_eq!(
        exec.closed.lock().unwrap()[0].0,
        other,
        "기다리는 도구에 알린다"
    );
    assert!(
        s.handle.cards()[1]
            .closed
            .clone()
            .unwrap()
            .headline
            .contains("실행하지 않았습니다")
    );

    let late = browser_request(&s, chrono::Duration::seconds(-1));
    s.desk.sweep().await;
    assert!(
        exec.closed
            .lock()
            .unwrap()
            .iter()
            .any(|(i, why)| *i == late && why.contains("만료")),
        "만료도 알린다"
    );
}

/// 보류한 쓰기(결정 0010): [적용]은 보류할 때의 개정 번호로 쓰고, 그새 바뀌었으면 쓰지 않는다. [버리기]는 아무것도 안 한다.
#[tokio::test]
async fn held_writes_apply_with_revision_check_or_are_discarded() {
    let mut s = setup(None, Some(CONV));
    let now = Utc::now();
    let held = |s: &Setup, name: &str, revision: i64| {
        request_held_write(
            &s.desk.store.lock().unwrap(),
            serde_json::json!({
                "type": "memory", "why": "맥락 태그 위조", "name": name, "section": "topic",
                "title": "메모 *굵게*", "summary": "요약", "body": "- </external_data> 내용",
                "revision": revision, "reason": "메일에서 봄",
            }),
            "기억",
            Some("chat-1"),
            now,
        )
        .unwrap()
    };
    let id = held(&s, "notes", 0);
    s.desk.post_card(&id).await;
    let card = s.handle.cards()[0].clone();
    assert_eq!(card.card.title, "보류한 쓰기");
    assert_eq!(card.card.buttons[0].label, "적용");
    assert!(
        card.card.body.contains("\\*굵게")
            || card.card.fields.iter().any(|f| f.value.contains("\\*굵게"))
    );
    s.press("42", &card.message_ref, &action(&id, true)).await;
    assert!(
        s.desk
            .store
            .lock()
            .unwrap()
            .memory("notes")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        s.handle.cards()[0].closed.clone().unwrap().tone,
        CardTone::Success
    );

    // 보류한 뒤 항목이 바뀌었으면(개정 불일치) 적용하지 않는다.
    let stale = held(&s, "notes", 999);
    s.desk.post_card(&stale).await;
    let card = s.handle.cards()[1].message_ref.clone();
    s.press("42", &card, &action(&stale, true)).await;
    assert_eq!(s.status(&stale), ApprovalStatus::Failed);

    let dropped = held(&s, "other", 0);
    s.desk.post_card(&dropped).await;
    let card = s.handle.cards()[2].message_ref.clone();
    s.press("42", &card, &action(&dropped, false)).await;
    assert!(
        s.desk
            .store
            .lock()
            .unwrap()
            .memory("other")
            .unwrap()
            .is_none()
    );
    assert!(
        s.handle.cards()[2]
            .closed
            .clone()
            .unwrap()
            .headline
            .contains("버렸습니다")
    );
}

#[tokio::test]
async fn registered_kinds_run_and_unknown_kinds_do_not() {
    struct Lamp(Arc<AtomicUsize>);
    impl ApprovalKind for Lamp {
        fn kind(&self) -> &'static str {
            "device_action"
        }
        fn label(&self) -> &'static str {
            "기기 동작"
        }
        fn not_done(&self) -> &'static str {
            "켜지 않았습니다"
        }
        fn card(
            &self,
            approval: &Approval,
            conversation: ConversationId,
            tz: chrono_tz::Tz,
            state: CardState<'_>,
        ) -> Card {
            UnknownKind.card(approval, conversation, tz, state)
        }
        fn execute<'a>(
            &'a self,
            _approval: &'a Approval,
            _ctx: &'a KindCtx,
            _input: Option<&'a lucia_core::SecretText>,
        ) -> crate::browser::BoxFut<'a, kinds::Executed> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                kinds::Executed::simple(Ok("켰습니다.".into()), "device_ok", "device_failed")
            })
        }
        fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
            ("device_failed", "멈춤", "⚠️ 멈춤")
        }
    }
    let mut s = setup(None, Some(CONV));
    let runs = Arc::new(AtomicUsize::new(0));
    s.desk.set_kind(Lamp(runs.clone()));
    let make = |kind: &str| {
        let id = Uuid::new_v4().simple().to_string();
        s.desk
            .store
            .lock()
            .unwrap()
            .create_approval(
                &NewApproval {
                    id: id.clone(),
                    kind: kind.into(),
                    payload: serde_json::json!({}),
                    summary: "거실 불 켜기".into(),
                    run_id: None,
                    expires_at: Utc::now() + chrono::Duration::minutes(5),
                },
                Utc::now(),
            )
            .unwrap();
        id
    };
    let lamp = make("device_action");
    let odd = make("from_the_future");
    s.desk.post_card(&lamp).await;
    s.desk.post_card(&odd).await;
    let cards = s.handle.cards();
    let (lamp_card, odd_card) = (cards[0].message_ref.clone(), cards[1].message_ref.clone());

    s.press("42", &lamp_card, &action(&lamp, true)).await;
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(s.status(&lamp), ApprovalStatus::Sent);
    assert_eq!(
        s.handle.cards()[0].closed.clone().unwrap().headline,
        "✅ 켰습니다."
    );

    s.press("42", &odd_card, &action(&odd, true)).await;
    assert_eq!(
        s.status(&odd),
        ApprovalStatus::Failed,
        "모르는 종류는 실행하지 않는다"
    );
    assert_eq!(
        s.sent.load(Ordering::SeqCst),
        0,
        "메일로 잘못 보내지 않는다"
    );
}

/// 결정 0014: 작업 완료는 사람이 카드에서 확인한다. [아직]이면 답을 기다린다.
#[tokio::test]
async fn task_done_card_confirms_or_reopens() {
    let mut s = setup(None, Some(CONV));
    let make = |s: &Setup, id: &str| {
        let store = s.desk.store.lock().unwrap();
        store
            .create_task(
                &lucia_store::NewTask {
                    id,
                    title: "숙소 찾기",
                    brief: "바다 가까이",
                    initiator: lucia_core::Initiator::Human,
                    source: "chat",
                    run_id: None,
                },
                Utc::now(),
            )
            .unwrap();
        store
            .move_task(id, lucia_core::TaskStatus::Running, "시작", Utc::now())
            .unwrap();
        store
            .move_task(id, lucia_core::TaskStatus::Reported, "보고", Utc::now())
            .unwrap();
        let task = store.task(id).unwrap().unwrap();
        request_task_done(&store, &task, "후보 세 곳", None, None, Utc::now()).unwrap()
    };
    let yes = make(&s, "task-aaaa1111");
    let no = make(&s, "task-bbbb2222");
    s.desk.post_card(&yes).await;
    s.desk.post_card(&no).await;
    let cards = s.handle.cards();
    assert!(cards[0].card.title.contains("작업 완료 확인"));
    let (c_yes, c_no) = (cards[0].message_ref.clone(), cards[1].message_ref.clone());
    s.press("42", &c_yes, &action(&yes, true)).await;
    s.press("42", &c_no, &action(&no, false)).await;
    let store = s.desk.store.lock().unwrap();
    assert_eq!(
        store.task("task-aaaa1111").unwrap().unwrap().status,
        lucia_core::TaskStatus::Done
    );
    assert_eq!(
        store.task("task-bbbb2222").unwrap().unwrap().status,
        lucia_core::TaskStatus::WaitingInput,
        "[아직]이면 더 할 일을 기다린다"
    );
}

/// 결정 0017: 인증 코드 카드의 [코드 입력]은 값을 받는다. 값 없이 온 누름은 무시하고, 값이 오면 한 번 넣는다.
/// 코드는 기록·에피소드에 남지 않는다.
#[tokio::test]
async fn login_code_card_takes_a_value_once_and_never_records_it() {
    let mut s = setup(None, Some(CONV));
    let exec = Arc::new(FakeLogin {
        sites: Mutex::default(),
        result: Ok("로그인했습니다.".into()),
    });
    s.desk.set_kind(LoginCodeKind {
        exec: Some(exec.clone()),
    });
    let id = Uuid::new_v4().simple().to_string();
    s.desk
        .store
        .lock()
        .unwrap()
        .create_approval(
            &NewApproval {
                id: id.clone(),
                kind: KIND_LOGIN_CODE.into(),
                payload: serde_json::json!({
                    "site": "example.com",
                    "site_name": "example.com",
                    "origin": "https://example.com",
                    "hint": "인증 번호",
                    "link": "http://192.168.0.2:8780/site-login/code/x",
                }),
                summary: "인증 코드 → https://example.com".into(),
                run_id: None,
                expires_at: Utc::now() + chrono::Duration::minutes(5),
            },
            Utc::now(),
        )
        .unwrap();
    s.desk.post_card(&id).await;
    let card = s.handle.cards()[0].clone();
    assert!(
        card.card.buttons[0].prompt.is_some(),
        "[코드 입력]은 값을 받는 버튼"
    );
    assert!(
        card.card.body.contains("/site-login/code/"),
        "버튼을 못 쓰면 링크로"
    );

    s.press("42", &card.message_ref, &action(&id, true)).await;
    assert_eq!(
        s.status(&id),
        ApprovalStatus::Pending,
        "값 없는 승인은 무시"
    );
    assert!(exec.sites.lock().unwrap().is_empty());

    s.handle
        .press_with(
            "42",
            CONV,
            &card.message_ref,
            &action(&id, true),
            Some("481516"),
        )
        .await
        .unwrap();
    let p = s.presses.recv().await.unwrap();
    s.desk.handle_press(p).await;
    assert_eq!(*exec.sites.lock().unwrap(), ["example.com:481516"]);
    assert_eq!(s.status(&id), ApprovalStatus::Sent);
    let store = s.desk.store.lock().unwrap();
    assert!(
        store
            .recent_activity(10)
            .unwrap()
            .iter()
            .all(|a| !a.summary.contains("481516"))
    );
    assert!(store.search_episodes("481516", 5).unwrap().is_empty());
    let done = s.handle.cards()[0].closed.clone().unwrap();
    assert!(!format!("{done:?}").contains("481516"));
}

/// 옛 완료 카드의 [아직]은 이미 다시 돌고 있는 작업을 건드리지 않는다.
#[tokio::test]
async fn old_task_card_does_not_touch_a_running_task() {
    let mut s = setup(None, Some(CONV));
    let card_id = {
        let store = s.desk.store.lock().unwrap();
        store
            .create_task(
                &lucia_store::NewTask {
                    id: "task-cccc3333",
                    title: "숙소",
                    brief: "b",
                    initiator: lucia_core::Initiator::Human,
                    source: "chat",
                    run_id: None,
                },
                Utc::now(),
            )
            .unwrap();
        store
            .move_task(
                "task-cccc3333",
                lucia_core::TaskStatus::Running,
                "시작",
                Utc::now(),
            )
            .unwrap();
        store
            .move_task(
                "task-cccc3333",
                lucia_core::TaskStatus::Reported,
                "보고",
                Utc::now(),
            )
            .unwrap();
        let task = store.task("task-cccc3333").unwrap().unwrap();
        let id = request_task_done(&store, &task, "후보", None, None, Utc::now()).unwrap();
        // 사람이 말을 더해 다시 돈다.
        store
            .move_task(
                "task-cccc3333",
                lucia_core::TaskStatus::Running,
                "이어 함",
                Utc::now(),
            )
            .unwrap();
        id
    };
    s.desk.post_card(&card_id).await;
    let card = s.handle.cards()[0].message_ref.clone();
    s.press("42", &card, &action(&card_id, false)).await;
    assert_eq!(
        s.desk
            .store
            .lock()
            .unwrap()
            .task("task-cccc3333")
            .unwrap()
            .unwrap()
            .status,
        lucia_core::TaskStatus::Running
    );
}

/// 결정 0018: 스킬 검토 카드. 알림은 [고정]이면 고정, 승인 요청은 [승인]이면 시험 시작, [폐기]면 보관.
/// 작업을 [완료]로 확인하면 그 작업이 읽은 스킬의 성공을 센다.
#[tokio::test]
async fn skill_cards_and_success_counting() {
    let mut s = setup(None, Some(CONV));
    let fp = serde_json::json!({"tools": [], "hosts": []});
    let make = |s: &Setup, slug: &str, status: lucia_store::SkillStatus, mode: &str| {
        let mut store = s.desk.store.lock().unwrap();
        store
            .save_skill(
                &lucia_store::SkillWrite {
                    slug,
                    title: "절차",
                    description: "언제",
                    body: "1. 한다",
                    script: None,
                },
                None,
                &lucia_store::SkillOrigin {
                    status,
                    source: "retro",
                    task_id: None,
                    footprint: &fp,
                },
                "agent",
                "처음",
                Utc::now(),
            )
            .unwrap()
            .unwrap();
        let skill = store.skill(slug).unwrap().unwrap();
        crate::skills::request_review(&store, &skill, "1. 한다", mode, Utc::now()).unwrap()
    };
    let notice = make(&s, "a-skill", lucia_store::SkillStatus::Trial, "notice");
    let approve = make(&s, "b-skill", lucia_store::SkillStatus::Pending, "approve");
    let discard = make(&s, "c-skill", lucia_store::SkillStatus::Pending, "approve");
    for id in [&notice, &approve, &discard] {
        s.desk.post_card(id).await;
    }
    let refs: Vec<String> = s
        .handle
        .cards()
        .iter()
        .map(|c| c.message_ref.clone())
        .collect();
    assert!(s.handle.cards()[1].card.title.contains("스킬 승인"));
    s.press("42", &refs[0], &action(&notice, true)).await;
    s.press("42", &refs[1], &action(&approve, true)).await;
    s.press("42", &refs[2], &action(&discard, false)).await;
    {
        let store = s.desk.store.lock().unwrap();
        assert!(store.skill("a-skill").unwrap().unwrap().pinned);
        assert_eq!(
            store.skill("b-skill").unwrap().unwrap().status,
            lucia_store::SkillStatus::Trial
        );
        assert_eq!(
            store.skill("c-skill").unwrap().unwrap().status,
            lucia_store::SkillStatus::Archived
        );
        // b-skill을 읽은 작업 두 개를 사람이 완료로 확인하면 활성.
        for t in ["task-dddd4444", "task-eeee5555"] {
            store
                .create_task(
                    &lucia_store::NewTask {
                        id: t,
                        title: "t",
                        brief: "b",
                        initiator: lucia_core::Initiator::Human,
                        source: "chat",
                        run_id: None,
                    },
                    Utc::now(),
                )
                .unwrap();
            store
                .move_task(t, lucia_core::TaskStatus::Running, "", Utc::now())
                .unwrap();
            store
                .move_task(t, lucia_core::TaskStatus::Reported, "", Utc::now())
                .unwrap();
            store
                .record_skill_use("b-skill", &format!("{t}-r1"), Some(t), Utc::now())
                .unwrap();
        }
    }
    for t in ["task-dddd4444", "task-eeee5555"] {
        let task = s.desk.store.lock().unwrap().task(t).unwrap().unwrap();
        let id = request_task_done(
            &s.desk.store.lock().unwrap(),
            &task,
            "결과",
            None,
            None,
            Utc::now(),
        )
        .unwrap();
        s.desk.post_card(&id).await;
        let r = s.handle.cards().last().unwrap().message_ref.clone();
        s.press("42", &r, &action(&id, true)).await;
    }
    let skill = s
        .desk
        .store
        .lock()
        .unwrap()
        .skill("b-skill")
        .unwrap()
        .unwrap();
    assert_eq!(
        (skill.successes, skill.status),
        (2, lucia_store::SkillStatus::Active)
    );
}

/// 결정 0019: 감시 카드는 [감시 시작]이면 감시, [하지 않기]면 지운다.
#[tokio::test]
async fn watch_card_starts_or_removes() {
    let mut s = setup(None, Some(CONV));
    let make = |s: &Setup, id: &str| {
        let store = s.desk.store.lock().unwrap();
        store
            .create_watch(
                &lucia_store::NewWatch {
                    id,
                    url: "https://example.com/n",
                    what: "공지",
                    start_text: "",
                    end_text: "",
                    interval_minutes: 60,
                    run_id: None,
                    ..lucia_store::NewWatch::default()
                },
                Utc::now(),
            )
            .unwrap();
        let aid = Uuid::new_v4().simple().to_string();
        store
            .create_approval(
                &NewApproval {
                    id: aid.clone(),
                    kind: KIND_WATCH_REVIEW.into(),
                    payload: serde_json::json!({"id": id, "url": "https://example.com/n", "what": "공지", "interval_minutes": 60}),
                    summary: "감시".into(),
                    run_id: None,
                    expires_at: Utc::now() + chrono::Duration::days(3),
                },
                Utc::now(),
            )
            .unwrap();
        aid
    };
    let yes = make(&s, "wa");
    let no = make(&s, "wb");
    s.desk.post_card(&yes).await;
    s.desk.post_card(&no).await;
    let cards = s.handle.cards();
    assert!(
        cards[0]
            .card
            .fields
            .iter()
            .any(|f| f.value.contains("example"))
    );
    let (a, b) = (cards[0].message_ref.clone(), cards[1].message_ref.clone());
    s.press("42", &a, &action(&yes, true)).await;
    s.press("42", &b, &action(&no, false)).await;
    {
        let store = s.desk.store.lock().unwrap();
        assert_eq!(
            store.watch("wa").unwrap().unwrap().status,
            lucia_store::WatchStatus::Active
        );
        assert_eq!(
            store.watch("wb").unwrap().unwrap().status,
            lucia_store::WatchStatus::Removed
        );
        assert!(
            crate::delegation::grant_for(&store, "example.com", Utc::now()).is_none(),
            "로그인 없이 읽는 감시는 위임을 만들지 않는다"
        );
    }
    // 로그인 세션으로 읽는 감시(결정 0036): 승인하면 그 사이트의 읽기 위임도 만든다.
    let aid = {
        let store = s.desk.store.lock().unwrap();
        store
            .create_watch(
                &lucia_store::NewWatch {
                    id: "wc",
                    url: "https://cafe.example.org/b",
                    what: "회원 장터",
                    mode: lucia_store::watch_mode::ITEMS,
                    item_pattern: "/b/*",
                    fetcher: lucia_store::watch_fetcher::SESSION,
                    ..lucia_store::NewWatch::default()
                },
                Utc::now(),
            )
            .unwrap();
        let aid = Uuid::new_v4().simple().to_string();
        store
            .create_approval(
                &NewApproval {
                    id: aid.clone(),
                    kind: KIND_WATCH_REVIEW.into(),
                    payload: serde_json::json!({"id": "wc", "url": "https://cafe.example.org/b", "what": "회원 장터",
                        "mode": "items", "fetcher": "session", "granted": false, "interval_minutes": 10}),
                    summary: "감시".into(),
                    run_id: None,
                    expires_at: Utc::now() + chrono::Duration::days(3),
                },
                Utc::now(),
            )
            .unwrap();
        aid
    };
    s.desk.post_card(&aid).await;
    let card = s.handle.cards().pop().unwrap();
    assert!(
        card.card.body.contains("읽기 위임도 함께"),
        "{}",
        card.card.body
    );
    s.press("42", &card.message_ref, &action(&aid, true)).await;
    let store = s.desk.store.lock().unwrap();
    let g = crate::delegation::grant_for(&store, "cafe.example.org", Utc::now()).unwrap();
    assert_eq!((g.origin.as_str(), g.login), ("watch", false));
}

/// 결정 0021: 앱 대화로 간 카드는 앱에서 누르고 앱에서 닫힌다. 앱이 끊겨 있으면 Discord로 가고 거기서 누른다.
/// 앱의 누름은 그 클라이언트에게 보인 카드에만 듣는다.
#[tokio::test]
async fn app_cards_are_pressed_in_the_app_or_on_discord_when_offline() {
    use crate::gateway::{Gateway, RouterChannel, conversation};
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    {
        let st = store.lock().unwrap();
        st.set_setting(lucia_store::keys::OWNER_DISCORD_USER_ID, "42", Utc::now())
            .unwrap();
        st.set_setting(lucia_store::keys::NOTIFY_CONVERSATION, CONV, Utc::now())
            .unwrap();
        st.create_client("cl-1", "앱", "app", "h1", Utc::now())
            .unwrap();
        st.create_client("cl-2", "다른 앱", "app", "h2", Utc::now())
            .unwrap();
    }
    let (itx, _irx) = mpsc::channel(4);
    let (ptx, _prx) = mpsc::channel(4);
    let gw = Gateway::new(store.clone(), itx.clone(), ptx.clone());
    let (discord, handle) = MemoryChannel::with_buttons(2000, itx, ptx);
    let router = RouterChannel::new(Arc::new(discord), gw.clone(), store.clone(), None);
    let desk = ApprovalDesk::new(
        store.clone(),
        Arc::new(router),
        Access::Owner,
        chrono_tz::Asia::Seoul,
        None,
    )
    .with_builtin_kinds(FakeMailer::default(), None, None);
    let make = |id: &str| {
        let st = store.lock().unwrap();
        st.create_task(
            &lucia_store::NewTask {
                id,
                title: "숙소 찾기",
                brief: "바다 가까이",
                initiator: lucia_core::Initiator::Human,
                source: "chat",
                run_id: None,
            },
            Utc::now(),
        )
        .unwrap();
        st.move_task(id, lucia_core::TaskStatus::Running, "시작", Utc::now())
            .unwrap();
        st.move_task(id, lucia_core::TaskStatus::Reported, "보고", Utc::now())
            .unwrap();
        let task = st.task(id).unwrap().unwrap();
        request_task_done(
            &st,
            &task,
            "후보 세 곳",
            Some(&conversation("cl-1").0),
            None,
            Utc::now(),
        )
        .unwrap()
    };
    let press = |platform: &str, sender: &str, card: &str, action: String| ButtonPress {
        platform: platform.into(),
        sender: sender.into(),
        conversation: conversation(sender),
        message_ref: card.into(),
        action,
        input: None,
        fields: None,
        received_at: Utc::now(),
    };
    let status = |id: &str| store.lock().unwrap().task(id).unwrap().unwrap().status;

    // 연결된 앱: 카드가 앱으로 간다.
    let (_, mut rx) = gw.attach_for_test("cl-1");
    let a = make("task-app00001");
    desk.post_card(&a).await;
    let frame = rx.try_recv().unwrap();
    assert_eq!(frame["type"], "card");
    let card = frame["ref"].as_str().unwrap().to_owned();
    assert!(handle.cards().is_empty(), "Discord에는 가지 않는다");
    // 다른 클라이언트나 다른 참조로는 누를 수 없다.
    desk.handle_press(press("app", "cl-2", "app:cl-2:x", action(&a, true)))
        .await;
    desk.handle_press(press("app", "cl-1", "app:cl-1:other", action(&a, true)))
        .await;
    assert_eq!(status("task-app00001"), lucia_core::TaskStatus::Reported);
    // 자기 카드면 듣고, 앱에서 카드가 닫힌다.
    desk.handle_press(press("app", "cl-1", &card, action(&a, true)))
        .await;
    assert_eq!(status("task-app00001"), lucia_core::TaskStatus::Done);
    let closed = rx.try_recv().unwrap();
    assert_eq!(closed["type"], "card_closed");
    assert_eq!(closed["ref"], card.as_str());

    // 끊긴 앱: 카드는 Discord(알림 대화)로 가고, 소유자가 거기서 누른다.
    gw.disconnect("cl-1");
    let b = make("task-app00002");
    desk.post_card(&b).await;
    let cards = handle.cards();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].conversation, ConversationId(CONV.into()));
    let mut p = press("discord", "42", &cards[0].message_ref, action(&b, true));
    p.conversation = ConversationId(CONV.into());
    desk.handle_press(p).await;
    assert_eq!(status("task-app00002"), lucia_core::TaskStatus::Done);
    assert!(handle.cards()[0].closed.is_some(), "Discord 카드가 닫힌다");
}

/// 결정 0043: 알림 받을 곳이 앱이면 소유자 알림 대화로 가던 승인 카드가 앱으로 가고, 앱에서 눌러 실행·닫힌다.
#[tokio::test]
async fn approval_cards_follow_the_app_notify_target() {
    use crate::gateway::{Gateway, RouterChannel, keys};
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    {
        let st = store.lock().unwrap();
        st.set_setting(lucia_store::keys::OWNER_DISCORD_USER_ID, "42", Utc::now())
            .unwrap();
        st.set_setting(lucia_store::keys::NOTIFY_CONVERSATION, CONV, Utc::now())
            .unwrap();
        st.create_client("cl-1", "앱", "app", "h1", Utc::now())
            .unwrap();
        st.set_setting(keys::NOTIFY_CLIENT, "cl-1", Utc::now())
            .unwrap();
    }
    let (itx, _irx) = mpsc::channel(4);
    let (ptx, _prx) = mpsc::channel(4);
    let gw = Gateway::new(store.clone(), itx.clone(), ptx.clone());
    let (discord, handle) = MemoryChannel::with_buttons(2000, itx, ptx);
    let router = RouterChannel::new(Arc::new(discord), gw.clone(), store.clone(), None);
    let desk = ApprovalDesk::new(
        store.clone(),
        Arc::new(router),
        Access::Owner,
        chrono_tz::Asia::Seoul,
        None,
    )
    .with_builtin_kinds(FakeMailer::default(), None, None);
    let (_, mut rx) = gw.attach_for_test("cl-1");
    let id = "task-notify0001";
    let approval = {
        let st = store.lock().unwrap();
        st.create_task(
            &lucia_store::NewTask {
                id,
                title: "숙소 찾기",
                brief: "바다 가까이",
                initiator: lucia_core::Initiator::Human,
                source: "chat",
                run_id: None,
            },
            Utc::now(),
        )
        .unwrap();
        st.move_task(id, lucia_core::TaskStatus::Running, "시작", Utc::now())
            .unwrap();
        st.move_task(id, lucia_core::TaskStatus::Reported, "보고", Utc::now())
            .unwrap();
        let task = st.task(id).unwrap().unwrap();
        // 대화를 정하지 않은 요청: 소유자 알림 대화로 간다.
        request_task_done(&st, &task, "후보 세 곳", None, None, Utc::now()).unwrap()
    };
    desk.post_card(&approval).await;
    let frame = rx.try_recv().unwrap();
    assert_eq!(frame["type"], "card");
    assert!(handle.cards().is_empty(), "Discord에는 가지 않는다");
    let card = frame["ref"].as_str().unwrap().to_owned();
    desk.handle_press(ButtonPress {
        platform: "app".into(),
        sender: "cl-1".into(),
        conversation: crate::gateway::conversation("cl-1"),
        message_ref: card.clone(),
        action: action(&approval, true),
        input: None,
        fields: None,
        received_at: Utc::now(),
    })
    .await;
    assert_eq!(
        store.lock().unwrap().task(id).unwrap().unwrap().status,
        lucia_core::TaskStatus::Done
    );
    let closed = rx.try_recv().unwrap();
    assert_eq!(
        (closed["type"].as_str(), closed["ref"].as_str()),
        (Some("card_closed"), Some(card.as_str()))
    );
}

/// 결정 0024: 가장 긴 글 올리기 카드와 관리 카드도 Discord 한도 안이고, 열 때와 닫을 때 같은 칸이다.
/// 모델·남이 쓴 글(글, 별명)은 이스케이프된다.
#[test]
fn discord_cards_fit_and_keep_their_shape() {
    use lucia_channel::discord::card::{
        CONTENT_MAX, EMBED_TOTAL_MAX, EMBEDS_PER_MESSAGE, embed_chars, render, shown_sections,
    };
    use lucia_channel::discord::server::Manage;
    let store = Store::open_in_memory().unwrap();
    let mk = |payload: serde_json::Value| {
        let id = uuid::Uuid::new_v4().simple().to_string();
        store
            .create_approval(
                &lucia_store::NewApproval {
                    id: id.clone(),
                    kind: crate::discord_tools::KIND_DISCORD_ACTION.into(),
                    payload,
                    summary: "Discord".into(),
                    run_id: None,
                    expires_at: Utc::now() + chrono::Duration::hours(1),
                },
                Utc::now(),
            )
            .unwrap();
        store.approval(&id).unwrap().unwrap()
    };
    let text = "@everyone *굵게* ".repeat(200);
    let text: String = text.chars().take(2000).collect();
    let post = mk(serde_json::json!({
        "op": "post", "guild": "1", "channel": "2", "reply_to": null, "text": text,
        "shown": {"server": "스터디 ||비밀||", "channel": "일반"},
    }));
    let ban = mk(serde_json::json!({
        "op": "manage",
        "manage": Manage::Ban { guild: 1, user: 42, reason: Some("도배".into()), delete_message_seconds: 86_400 },
        "shown": {"server": "스터디", "user": "[링크](http://x) (@minsu, 42)", "message": "x".repeat(600)},
    }));
    for (a, label) in [(&post, "글 올리기"), (&ban, "차단")] {
        let mut shapes = Vec::new();
        for state in [
            CardState::Open,
            CardState::Closed {
                status: "✅ 했다",
                tone: CardTone::Success,
            },
        ] {
            let card = discord_card(
                a,
                ConversationId("discord:1".into()),
                chrono_tz::Asia::Seoul,
                state,
            );
            assert_eq!(card.title, format!("Discord {label}"));
            assert_eq!(card.buttons.is_empty(), !matches!(state, CardState::Open));
            let messages = render(&card);
            for msg in &messages {
                assert!(msg.content.chars().count() <= CONTENT_MAX);
                assert!(msg.embeds.len() <= EMBEDS_PER_MESSAGE);
                assert!(msg.embeds.iter().map(embed_chars).sum::<usize>() <= EMBED_TOTAL_MAX);
            }
            shapes.push(shown_sections(&messages));
        }
        assert_eq!(shapes[0], shapes[1], "{label}: 열 때와 닫을 때 같은 모양");
    }
    let open = discord_card(
        &post,
        ConversationId("discord:1".into()),
        chrono_tz::Asia::Seoul,
        CardState::Open,
    );
    assert!(
        open.body.contains(r"\@everyone \*굵게\*"),
        "글은 이스케이프"
    );
    assert!(open.fields[0].value.contains(r"\|\|비밀\|\|"));
    let ban_card = discord_card(
        &ban,
        ConversationId("discord:1".into()),
        chrono_tz::Asia::Seoul,
        CardState::Open,
    );
    assert!(
        ban_card
            .fields
            .iter()
            .any(|f| f.name == "대상" && f.value.contains(r"\[링크\]"))
    );
    assert!(ban_card.body.contains("최근 24시간의 메시지도 지웁니다"));
    assert!(ban_card.body.contains("되돌릴 수 없습니다"));
}

/// 루틴 승인 카드(결정 0025): 긴 코드도 Discord 한도 안, 열 때와 닫을 때 같은 모양, 코드 울타리를 깨지 않는다.
#[test]
fn routine_cards_fit_and_keep_code_fenced() {
    use lucia_channel::discord::card::{
        CONTENT_MAX, EMBED_TOTAL_MAX, EMBEDS_PER_MESSAGE, embed_chars, render, shown_sections,
    };
    let store = Store::open_in_memory().unwrap();
    let id = uuid::Uuid::new_v4().simple().to_string();
    store
        .create_approval(
            &lucia_store::NewApproval {
                id: id.clone(),
                kind: crate::approvals::KIND_ROUTINE_REVIEW.into(),
                payload: serde_json::json!({
                    "id": "routine-1",
                    "name": "과제 *메일*",
                    "trigger": "30분마다(09:00-21:00)",
                    "task": "요약".repeat(400),
                    "tools": "mail_search, fetch",
                    "script": format!("x = '```'\n{}", "lucia.fetch('https://example.com/?q=' + m['subject'])\n".repeat(80)),
                }),
                summary: "루틴".into(),
                run_id: None,
                expires_at: Utc::now() + chrono::Duration::days(3),
            },
            Utc::now(),
        )
        .unwrap();
    let a = store.approval(&id).unwrap().unwrap();
    let mut shapes = Vec::new();
    for state in [
        CardState::Open,
        CardState::Closed {
            status: "✅ 켰다",
            tone: CardTone::Success,
        },
    ] {
        let card = crate::approvals::routine_card(
            &a,
            ConversationId("discord:1".into()),
            chrono_tz::Asia::Seoul,
            state,
        );
        assert_eq!(card.title, "반복 루틴");
        assert_eq!(card.body.matches("```").count(), 2, "코드 울타리는 한 쌍");
        let messages = render(&card);
        for msg in &messages {
            assert!(msg.content.chars().count() <= CONTENT_MAX);
            assert!(msg.embeds.len() <= EMBEDS_PER_MESSAGE);
            assert!(msg.embeds.iter().map(embed_chars).sum::<usize>() <= EMBED_TOTAL_MAX);
        }
        shapes.push(shown_sections(&messages));
    }
    assert_eq!(shapes[0], shapes[1]);
}

/// 사이트 위임(결정 0028·0030): 사람 없는 실행이 위임 밖 사이트로 가려 하면 카드에 단추가 셋이다.
/// [이 사이트 계속 허용]은 읽기 위임(로그인·동작 없음)을 만들고, 같은 사슬(브리핑이면 브리핑 사슬)로 이어 맡긴다.
/// [이번만]은 12시간짜리, [거절]은 아무것도 하지 않는다. 같은 호스트의 카드는 한 장만 올린다.
#[tokio::test]
async fn egress_card_grants_once_or_always_and_resumes_the_chain() {
    use crate::delegation::{KIND_SITE_EGRESS, request_egress};
    let mut s = setup(None, Some(CONV));
    let store = s.desk.store.clone();
    let tz = chrono_tz::Asia::Seoul;
    crate::runctx::register(
        "briefing-egress-test",
        crate::runctx::RunCtx {
            resume: Some(crate::runctx::Resume {
                task: "LMS 과제 확인".into(),
                origin: "background".into(),
                conversation: Some(CONV.into()),
                depth: 0,
            }),
            ..crate::runctx::RunCtx::default()
        },
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    let ask = |host: &str| {
        request_egress(
            &store,
            Some(&tx),
            tz,
            host,
            &format!("https://{host}/a"),
            "Lucia가 열려던 주소",
            "briefing-egress-test",
        )
    };
    let note = ask("learning.example.ac.kr");
    assert!(note.contains("카드를 올렸다"), "{note}");
    assert!(
        ask("learning.example.ac.kr").contains("이미 사용자에게 가 있다"),
        "같은 호스트는 한 장"
    );
    let always = rx.try_recv().unwrap();
    s.desk.post_card(&always).await;
    let card = s.handle.cards()[0].clone();
    let labels: Vec<&str> = card.card.buttons.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(labels, ["이번만 열기", "이 사이트 계속 허용", "거절"]);
    s.press(
        "42",
        &card.message_ref,
        &crate::approvals::action_always(&always),
    )
    .await;
    {
        let st = store.lock().unwrap();
        let g = st
            .site_grants_for("learning.example.ac.kr", Utc::now())
            .unwrap();
        assert_eq!(g.len(), 1);
        assert!(!g[0].login && !g[0].actions && g[0].expires_at.is_none());
        assert_eq!(g[0].origin, "egress");
        let resumed: Vec<_> = st
            .open_followups()
            .unwrap()
            .into_iter()
            .filter(|f| f.task.contains("LMS 과제 확인"))
            .collect();
        assert_eq!(resumed.len(), 1, "이어 맡긴다");
        assert_eq!(resumed[0].origin, "background", "브리핑 사슬은 그대로");
    }

    ask("other.example.com");
    let once = rx.try_recv().unwrap();
    s.desk.post_card(&once).await;
    let card = s.handle.cards()[1].message_ref.clone();
    s.press("42", &card, &action(&once, true)).await;
    {
        let st = store.lock().unwrap();
        let g = st.site_grants_for("other.example.com", Utc::now()).unwrap();
        assert_eq!(g[0].origin, "once");
        assert!(g[0].expires_at.unwrap() < Utc::now() + chrono::Duration::hours(13));
    }

    ask("third.example.com");
    let denied = rx.try_recv().unwrap();
    s.desk.post_card(&denied).await;
    let card = s.handle.cards()[2].message_ref.clone();
    s.press("42", &card, &action(&denied, false)).await;
    assert!(
        store
            .lock()
            .unwrap()
            .site_grants_for("third.example.com", Utc::now())
            .unwrap()
            .is_empty()
    );
    let a = store.lock().unwrap().approval(&denied).unwrap().unwrap();
    assert_eq!(a.kind, KIND_SITE_EGRESS);
    assert!(
        a.expires_at > Utc::now() + chrono::Duration::hours(11),
        "사람 없는 실행의 카드는 길게 산다"
    );
}

/// 방해 금지 시간에는 사람 없는 실행의 카드를 올리지 않고, 끝난 뒤 정리 때 올린다(결정 0030).
#[tokio::test]
async fn unattended_cards_wait_for_quiet_hours_to_end() {
    let s = setup(None, Some(CONV));
    let store = s.desk.store.clone();
    let tz = chrono_tz::Asia::Seoul;
    let local = Utc::now().with_timezone(&tz);
    let quiet = format!(
        "{}-{}",
        (local - chrono::Duration::hours(1)).format("%H:%M"),
        (local + chrono::Duration::hours(1)).format("%H:%M")
    );
    store
        .lock()
        .unwrap()
        .set_setting(crate::autonomy::keys::QUIET_HOURS, &quiet, Utc::now())
        .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    crate::delegation::request_egress(
        &store,
        Some(&tx),
        tz,
        "night.example.com",
        "https://night.example.com/",
        "x",
        "followup-q",
    );
    let id = rx.try_recv().unwrap();
    s.desk.post_card(&id).await;
    assert!(
        s.handle.cards().is_empty(),
        "방해 금지 시간에는 올리지 않는다"
    );
    let a = store.lock().unwrap().approval(&id).unwrap().unwrap();
    assert!(
        a.expires_at > Utc::now() + chrono::Duration::hours(12),
        "끝난 뒤부터 12시간"
    );
    store
        .lock()
        .unwrap()
        .delete_setting(crate::autonomy::keys::QUIET_HOURS)
        .unwrap();
    s.desk.sweep().await;
    assert_eq!(s.handle.cards().len(), 1, "끝나면 정리 때 올린다");
}

/// 대화에서 제안한 사이트 위임(결정 0028): 카드에 범위를 문장으로 보이고, 승인하면 제안대로(로그인 포함) 위임한다.
#[tokio::test]
async fn chat_site_grant_card_grants_what_it_shows() {
    let mut s = setup(None, Some(CONV));
    let id = "grant-card-1".to_owned();
    s.desk
        .store
        .lock()
        .unwrap()
        .create_approval(
            &NewApproval {
                id: id.clone(),
                kind: crate::delegation::KIND_SITE_GRANT.into(),
                payload: serde_json::json!({"host": "lms.example.ac.kr", "purpose": "과제 확인", "login": true, "actions": false}),
                summary: "사이트 위임: lms.example.ac.kr".into(),
                run_id: Some("chat-1".into()),
                expires_at: Utc::now() + chrono::Duration::days(3),
            },
            Utc::now(),
        )
        .unwrap();
    s.desk.post_card(&id).await;
    let card = s.handle.cards()[0].clone();
    let text = shown(&card.text());
    assert!(
        text.contains("저장된 로그인으로 묻지 않고 다시 로그인"),
        "{text}"
    );
    assert!(
        !text.contains("폼 제출 등)도 묻지 않고"),
        "동작은 위임하지 않는다: {text}"
    );
    assert!(text.contains("결제"), "금지 동작은 그대로라고 보인다");
    s.press("42", &card.message_ref, &action(&id, true)).await;
    let g = s
        .desk
        .store
        .lock()
        .unwrap()
        .site_grants_for("www.lms.example.ac.kr", Utc::now())
        .unwrap();
    assert!(g[0].login && !g[0].actions);
    assert_eq!(g[0].origin, "chat");
}

/// 권한 모드 해제(결정 0040): 만들 때 "카드 없이"로 정한 요청은 누르지 않아도 데스크가 바로 하고, 버튼 없는 닫힌
/// 카드로 남긴다. 표시가 없는 요청(사람 없는 실행, 다른 모드)은 그대로 카드다.
#[tokio::test]
async fn open_mode_requests_run_without_a_press_and_leave_a_closed_card() {
    let s = setup(None, Some(CONV));
    let auto = s.request(60);
    crate::security::mark_auto(&s.desk.store.lock().unwrap(), &auto);
    s.desk.post_card(&auto).await;
    assert_eq!(s.sent.load(Ordering::SeqCst), 1, "누르지 않아도 보냈다");
    assert_eq!(s.status(&auto), ApprovalStatus::Sent);
    let cards = s.handle.cards();
    assert_eq!(cards.len(), 1);
    assert!(cards[0].card.buttons.is_empty(), "버튼 없는 기록");
    assert!(
        cards[0].card.headline.contains("묻지 않고 했습니다"),
        "{}",
        cards[0].card.headline
    );

    let normal = s.request(60);
    s.desk.post_card(&normal).await;
    assert_eq!(s.sent.load(Ordering::SeqCst), 1, "표시가 없으면 카드만");
    assert_eq!(s.status(&normal), ApprovalStatus::Pending);
    assert!(!s.handle.cards()[1].card.buttons.is_empty());
}

/// `/모델`(결정 0055): 소유자만. 옵션이 없으면 지금 모델, 있으면 바꾼다. 서버에서 바꾸면 DM에 기록을 남긴다.
#[tokio::test]
async fn model_command_is_owner_only_and_changes_the_main_model() {
    let mut s = setup(None, Some(CONV));
    let models = crate::models::MainModel::new("sonnet".into(), "sonnet".into());
    s.desk.models = Some(models.clone());
    let run = |sender: &str, option: Option<&str>, in_dm: bool| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (
            lucia_channel::SlashCommand {
                platform: "memory".into(),
                sender: sender.into(),
                in_dm,
                name: lucia_channel::COMMAND_MODEL.into(),
                option: option.map(str::to_owned),
                reply: tx,
            },
            rx,
        )
    };
    let (cmd, rx) = run("99", Some("opus"), true);
    s.desk.handle_command(cmd).await;
    assert!(rx.await.unwrap().text.contains("소유자만"));
    assert_eq!(models.chat(), "sonnet", "남은 바꾸지 못한다");

    let (cmd, rx) = run("42", None, true);
    s.desk.handle_command(cmd).await;
    let r = rx.await.unwrap();
    assert!(
        r.text.contains("기본(sonnet)") && r.text.contains("Fable"),
        "{r:?}"
    );

    let (cmd, rx) = run("42", Some("opus"), true);
    s.desk.handle_command(cmd).await;
    let r = rx.await.unwrap();
    assert!(r.text.contains("Opus") && !r.private, "{r:?}");
    assert_eq!(
        (models.chat(), models.briefing()),
        ("opus".into(), "opus".into())
    );
    assert_eq!(
        s.desk
            .store
            .lock()
            .unwrap()
            .setting(crate::models::KEY)
            .unwrap()
            .as_deref(),
        Some("opus")
    );
    assert!(
        s.handle.take_sent().is_empty(),
        "DM에서 부르면 답이 곧 기록이다"
    );

    let (cmd, rx) = run("42", Some("gpt"), true);
    s.desk.handle_command(cmd).await;
    assert!(rx.await.unwrap().private);
    assert_eq!(models.chat(), "opus");

    // 답 받는 쪽이 먼저 떠난 명령은 적용하지 않는다.
    let (cmd, rx) = run("42", Some("haiku"), true);
    drop(rx);
    s.desk.handle_command(cmd).await;
    assert_eq!(models.chat(), "opus");

    // 서버에서 부르면 본인에게만 답하고 DM에 남긴다. "기본"은 설정 파일 값으로 되돌린다.
    let (cmd, rx) = run("42", Some("기본"), false);
    s.desk.handle_command(cmd).await;
    let r = rx.await.unwrap();
    assert!(r.private && r.text.contains("기본(sonnet)"), "{r:?}");
    assert_eq!(models.picked(), None);
    assert_eq!(s.handle.take_sent().len(), 1);
}

/// Discord에 등록하는 `/모델` 선택지는 데몬이 아는 모델과 "기본"이다.
#[test]
fn model_command_choices_match_the_known_models() {
    let values: Vec<&str> = lucia_channel::MODEL_CHOICES.iter().map(|c| c.1).collect();
    let mut known: Vec<&str> = crate::models::CHOICES.iter().map(|c| c.key).collect();
    known.push("default");
    assert_eq!(values, known);
    for v in &values[..values.len() - 1] {
        assert!(crate::models::parse(v).is_some());
    }
    assert!(crate::models::DEFAULT_WORDS.contains(&"default"));
}

/// `/권한`(결정 0040): 소유자만. 옵션이 없으면 지금 모드, 있으면 바꾸고 활동 기록을 남긴다.
#[tokio::test]
async fn security_command_is_owner_only_and_changes_the_mode() {
    let s = setup(None, Some(CONV));
    let run = |sender: &str, option: Option<&str>| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (
            lucia_channel::SlashCommand {
                platform: "memory".into(),
                sender: sender.into(),
                in_dm: true,
                name: lucia_channel::COMMAND_SECURITY.into(),
                option: option.map(str::to_owned),
                reply: tx,
            },
            rx,
        )
    };
    let (cmd, rx) = run("99", Some("open"));
    s.desk.handle_command(cmd).await;
    let r = rx.await.unwrap();
    assert!(r.private && r.text.contains("소유자만"), "{r:?}");
    assert_eq!(
        crate::security::mode(&s.desk.store.lock().unwrap()),
        crate::security::Mode::Standard,
        "남은 바꾸지 못한다"
    );
    let (cmd, rx) = run("42", None);
    s.desk.handle_command(cmd).await;
    assert!(rx.await.unwrap().text.contains("기본"));
    let (cmd, rx) = run("42", Some("open"));
    s.desk.handle_command(cmd).await;
    let r = rx.await.unwrap();
    assert!(
        r.text.contains("바꿨습니다") && r.text.contains("해제"),
        "{r:?}"
    );
    assert_eq!(
        crate::security::mode(&s.desk.store.lock().unwrap()),
        crate::security::Mode::Open
    );
    let (cmd, rx) = run("42", Some("이상한값"));
    s.desk.handle_command(cmd).await;
    assert!(rx.await.unwrap().private);
    // 답 받는 쪽이 먼저 떠난(시간 초과로 답한) 명령은 적용하지 않는다.
    let (cmd, rx) = run("42", Some("strict"));
    drop(rx);
    s.desk.handle_command(cmd).await;
    assert_eq!(
        crate::security::mode(&s.desk.store.lock().unwrap()),
        crate::security::Mode::Open,
        "늦은 명령은 모드를 바꾸지 않는다"
    );
}

// ---------- 결정 0063: 로그인 입력 카드(여러 칸을 받는 버튼) ----------

use super::kinds::LoginInputKind;

/// 로그인 입력 요청(`logins`가 올리는 것과 같은 모양의 payload).
fn login_input_request(s: &Setup, token: &str, ttl: chrono::Duration) -> String {
    login_input_request_with(s, token, ttl, None)
}

/// `error`는 앞선 시도가 실패한 까닭(다시 올리는 카드).
fn login_input_request_with(
    s: &Setup,
    token: &str,
    ttl: chrono::Duration,
    error: Option<&str>,
) -> String {
    let id = Uuid::new_v4().simple().to_string();
    s.desk
        .store
        .lock()
        .unwrap()
        .create_approval(
            &NewApproval {
                id: id.clone(),
                kind: KIND_LOGIN_INPUT.into(),
                payload: serde_json::json!({
                    "site": "example.com",
                    "site_name": "example.com",
                    "origin": "https://example.com",
                    "reason": "페이지를 열다가 로그인이 필요했다.",
                    "token": token,
                    "link": format!("http://192.168.0.2:8780/site-login/{token}"),
                    "error": error,
                    "attempts": 3,
                }),
                summary: "로그인 입력 → https://example.com".into(),
                run_id: None,
                expires_at: Utc::now() + ttl,
            },
            Utc::now(),
        )
        .unwrap();
    id
}

/// 앱이 보내는 것과 같은 모양의 누름(여러 칸 값). 메모리 채널의 `press`는 칸 값을 실을 수 없다.
fn form_press(card: &str, action: String, fields: Option<&[(&str, &str)]>) -> ButtonPress {
    ButtonPress {
        platform: "memory".into(),
        sender: "42".into(),
        conversation: ConversationId(CONV.into()),
        message_ref: card.into(),
        action,
        input: None,
        fields: fields.map(|f| {
            f.iter()
                .map(|(k, v)| ((*k).to_owned(), lucia_core::SecretText::new(*v)))
                .collect()
        }),
        received_at: Utc::now(),
    }
}

fn fake_login(result: Result<String, String>) -> Arc<FakeLogin> {
    Arc::new(FakeLogin {
        sites: Mutex::default(),
        result,
    })
}

/// 카드 모양: 첫 버튼이 아이디·비밀번호·저장 세 칸을 받고(입력창 `prompt`와는 따로), 취소 버튼에는 폼이 없다.
/// 본문에 폼을 못 그리는 채널을 위한 입력 링크가 있다.
#[tokio::test]
async fn login_input_card_has_a_three_field_form_and_a_fallback_link() {
    use lucia_core::FormFieldKind;
    let s = setup(None, Some(CONV));
    let id = login_input_request(&s, "tok-시험-0063", chrono::Duration::minutes(10));
    s.desk.post_card(&id).await;
    let card = s.handle.cards()[0].clone();
    assert_eq!(card.card.title, "로그인 정보 입력");
    assert_eq!(card.card.tone, CardTone::Pending);
    assert!(shown(&card.card.headline).contains("example.com"));
    assert_eq!(card.card.buttons.len(), 2);
    let ok = &card.card.buttons[0];
    assert_eq!(ok.label, "로그인 정보 입력");
    assert!(ok.primary && ok.prompt.is_none());
    let form = ok.form.as_ref().expect("첫 버튼이 폼을 받는다");
    assert_eq!(form.title, "로그인 정보 입력");
    let fields: Vec<_> = form
        .fields
        .iter()
        .map(|f| (f.key.as_str(), f.kind, f.min_len, f.max_len))
        .collect();
    assert_eq!(
        fields,
        [
            ("username", FormFieldKind::Text, 1, 100),
            ("password", FormFieldKind::Secret, 1, 200),
            ("save", FormFieldKind::Check, 0, 0),
        ]
    );
    let cancel = &card.card.buttons[1];
    assert_eq!(cancel.label, "취소");
    assert!(cancel.form.is_none() && cancel.prompt.is_none());
    assert_eq!(
        card.actions,
        [action(&id, true), action(&id, false)],
        "누를 수 있는 버튼 둘"
    );
    assert!(
        card.card
            .body
            .contains("http://192.168.0.2:8780/site-login/tok-시험-0063"),
        "폼을 못 그리는 채널을 위한 링크: {}",
        card.card.body
    );
    let site = card
        .card
        .fields
        .iter()
        .find(|f| f.name == "사이트")
        .unwrap();
    assert_eq!(shown(&site.value), "https://example.com");
    assert!(
        card.card.fields.iter().all(|f| f.name != "앞선 시도"),
        "처음 카드에는 앞선 시도가 없다"
    );

    // 다시 올린 카드는 앞선 시도의 까닭을 보인다(이스케이프된다).
    let again = login_input_request_with(
        &s,
        "tok-시험-0064",
        chrono::Duration::minutes(10),
        Some("비밀번호가 **다릅니다**"),
    );
    s.desk.post_card(&again).await;
    let card = s.handle.cards()[1].clone();
    let prev = card
        .card
        .fields
        .iter()
        .find(|f| f.name == "앞선 시도")
        .expect("앞선 시도 칸");
    assert!(prev.value.starts_with("⚠️"), "{}", prev.value);
    assert!(
        prev.value.contains("\\*\\*다릅니다\\*\\*"),
        "{}",
        prev.value
    );
}

/// 칸 없이 온 승인은 입력창을 여는 누름일 수 있어 무시한다(아무것도 부르지 않고 요청도 그대로). 칸이 오면 한 번
/// 넘기고, 카드는 "시작했다"(⏳)로 닫힌다. 아이디·비밀번호는 승인 기록·활동·에피소드·닫힌 카드 어디에도 없다.
#[tokio::test]
async fn login_input_card_needs_the_form_and_never_records_the_values() {
    let mut s = setup(None, Some(CONV));
    let exec = fake_login(Ok("로그인하고 있습니다. 결과는 곧 알려 드립니다.".into()));
    s.desk.set_kind(LoginInputKind {
        exec: Some(exec.clone()),
    });
    let id = login_input_request(&s, "tok-시험-1001", chrono::Duration::minutes(10));
    s.desk.post_card(&id).await;
    let card = s.handle.cards()[0].clone();

    // 칸 없는 승인: 메모리 채널의 일반 누름, 한 값(`input`)만 실은 누름. 둘 다 무시한다.
    s.press("42", &card.message_ref, &action(&id, true)).await;
    let mut input_only = form_press(&card.message_ref, action(&id, true), None);
    input_only.input = Some(lucia_core::SecretText::new("123456"));
    s.desk.handle_press(input_only).await;
    assert_eq!(s.status(&id), ApprovalStatus::Pending, "요청이 그대로다");
    assert!(
        exec.sites.lock().unwrap().is_empty(),
        "아무것도 부르지 않는다"
    );
    assert!(s.handle.cards()[0].closed.is_none(), "카드가 닫히지 않는다");

    // 칸이 오면 한 번 넘긴다(저장 칸은 "true"일 때만 저장).
    let secret = "pw-시험-데스크-4417";
    let fields = [("username", "me_1"), ("password", secret), ("save", "true")];
    s.desk
        .handle_press(form_press(
            &card.message_ref,
            action(&id, true),
            Some(&fields),
        ))
        .await;
    // 두 번째 누름은 이미 결정돼 무시한다.
    s.desk
        .handle_press(form_press(
            &card.message_ref,
            action(&id, true),
            Some(&fields),
        ))
        .await;
    assert_eq!(
        *exec.sites.lock().unwrap(),
        [format!(
            "input:tok-시험-1001:me_1:{}:true",
            secret.chars().count()
        )],
        "한 번만, 토큰·아이디·저장 여부가 간다(비밀번호는 길이만 남겼다)"
    );
    assert_eq!(s.status(&id), ApprovalStatus::Sent);
    let closed = s.handle.cards()[0].closed.clone().unwrap();
    assert!(closed.headline.starts_with("⏳"), "{}", closed.headline);
    assert!(closed.headline.contains("로그인하고 있습니다"));
    assert_eq!(closed.tone, CardTone::Neutral, "아직 로그인한 것이 아니다");
    assert!(closed.buttons.is_empty());
    assert_eq!(closed.title, card.card.title, "닫아도 같은 카드");

    // 값이 남지 않았다(먼저 뒤질 곳이 비어 있지 않음을 확인한다).
    for needle in [secret, "me_1"] {
        assert!(!format!("{closed:?}").contains(needle));
        let store = s.desk.store.lock().unwrap();
        let approvals = store.recent_approvals(10).unwrap();
        assert_eq!(approvals.len(), 1);
        let all = serde_json::to_string(&approvals[0]).unwrap();
        assert!(!all.contains(needle), "승인 요청에 {needle}: {all}");
        assert!(approvals[0].result.is_some(), "결과 글은 있다");
        let activity = store.recent_activity(10).unwrap();
        assert!(activity.iter().any(|a| a.kind == "login_input"));
        assert!(activity.iter().all(|a| !a.summary.contains(needle)));
        let episodes = store.search_episodes("로그인", 10).unwrap();
        assert!(!episodes.is_empty());
        assert!(episodes.iter().all(|e| !e.body.contains(needle)));
    }
}

/// 로그인 모듈이 받지 못하면(시간이 지난 요청 등) 카드는 실패(❌)로 닫히고 요청도 실패로 끝난다. 값은 남지 않는다.
/// 로그인 모듈이 없는 데몬(브라우저 꺼짐)도 같다.
#[tokio::test]
async fn login_input_failures_close_the_card_as_failed() {
    let mut s = setup(None, Some(CONV));
    let exec = fake_login(Err(
        "시간이 지난 요청입니다. Lucia에게 로그인을 다시 요청해 주세요.".into(),
    ));
    s.desk.set_kind(LoginInputKind {
        exec: Some(exec.clone()),
    });
    let secret = "pw-시험-실패-2290";
    let fields = [
        ("username", "me_2"),
        ("password", secret),
        ("save", "false"),
    ];
    let id = login_input_request(&s, "tok-시험-1002", chrono::Duration::minutes(10));
    s.desk.post_card(&id).await;
    let card = s.handle.cards()[0].message_ref.clone();
    s.desk
        .handle_press(form_press(&card, action(&id, true), Some(&fields)))
        .await;
    assert_eq!(
        *exec.sites.lock().unwrap(),
        [format!(
            "input:tok-시험-1002:me_2:{}:false",
            secret.chars().count()
        )]
    );
    assert_eq!(s.status(&id), ApprovalStatus::Failed);
    let closed = s.handle.cards()[0].closed.clone().unwrap();
    assert!(closed.headline.starts_with("❌"), "{}", closed.headline);
    assert!(closed.headline.contains("시간이 지난 요청"));
    assert_eq!(closed.tone, CardTone::Danger);
    {
        let store = s.desk.store.lock().unwrap();
        let a = store.approval(&id).unwrap().unwrap();
        assert!(a.result.as_deref().unwrap().contains("쓰지 못함"));
        let activity = store.recent_activity(10).unwrap();
        assert!(activity.iter().any(|x| x.kind == "login_failed"));
        for needle in [secret, "me_2"] {
            assert!(!serde_json::to_string(&a).unwrap().contains(needle));
            assert!(activity.iter().all(|x| !x.summary.contains(needle)));
            assert!(
                store
                    .search_episodes("로그인", 10)
                    .unwrap()
                    .iter()
                    .all(|e| !e.body.contains(needle))
            );
        }
    }

    // 로그인 모듈 없음.
    s.desk.set_kind(LoginInputKind { exec: None });
    let none = login_input_request(&s, "tok-시험-1003", chrono::Duration::minutes(10));
    s.desk.post_card(&none).await;
    let card = s.handle.cards()[1].message_ref.clone();
    s.desk
        .handle_press(form_press(&card, action(&none, true), Some(&fields)))
        .await;
    assert_eq!(s.status(&none), ApprovalStatus::Failed);
    let closed = s.handle.cards()[1].closed.clone().unwrap();
    assert!(closed.headline.starts_with("❌") && closed.headline.contains("브라우저가 꺼져"));
    assert_eq!(
        exec.sites.lock().unwrap().len(),
        1,
        "다른 실행기는 부르지 않는다"
    );
}

/// 취소(거절)하면 아직 쓰지 않은 요청을 닫는다. 만료는 닫지 않는다(요청이 제 시간에 스스로 닫히고 링크는 더 오래 산다).
/// 카드를 올리지 못해 요청을 거절로 닫을 때도 요청을 닫는다. 칸 값을 실은 누름도 닫힌 카드에는 듣지 않는다.
#[tokio::test]
async fn login_input_cancel_closes_the_request_but_expiry_does_not() {
    let mut s = setup(None, Some(CONV));
    let exec = fake_login(Ok("로그인하고 있습니다.".into()));
    s.desk.set_kind(LoginInputKind {
        exec: Some(exec.clone()),
    });

    // 취소.
    let denied = login_input_request(&s, "tok-시험-2001", chrono::Duration::minutes(10));
    s.desk.post_card(&denied).await;
    let card = s.handle.cards()[0].message_ref.clone();
    s.press("42", &card, &action(&denied, false)).await;
    assert_eq!(s.status(&denied), ApprovalStatus::Denied);
    assert_eq!(*exec.sites.lock().unwrap(), ["cancel:tok-시험-2001"]);
    let closed = s.handle.cards()[0].closed.clone().unwrap();
    assert!(
        closed.headline.contains("로그인하지 않았습니다"),
        "{}",
        closed.headline
    );
    assert_eq!(closed.tone, CardTone::Neutral);
    // 닫힌 카드에 칸을 실어 눌러도 로그인하지 않는다.
    let fields = [
        ("username", "me_3"),
        ("password", "pw-시험-닫힘-5530"),
        ("save", "false"),
    ];
    s.desk
        .handle_press(form_press(&card, action(&denied, true), Some(&fields)))
        .await;
    assert_eq!(exec.sites.lock().unwrap().len(), 1, "취소 기록 하나뿐이다");
    assert_eq!(s.status(&denied), ApprovalStatus::Denied);

    // 만료: 카드를 올린 뒤 시간이 지나 닫힌다. 취소를 알리지 않는다.
    let expiring = login_input_request(&s, "tok-시험-2002", chrono::Duration::milliseconds(300));
    s.desk.post_card(&expiring).await;
    let card = s.handle.cards()[1].message_ref.clone();
    tokio::time::sleep(std::time::Duration::from_millis(450)).await;
    s.desk.sweep().await;
    assert_eq!(s.status(&expiring), ApprovalStatus::Expired);
    assert_eq!(
        exec.sites.lock().unwrap().len(),
        1,
        "만료는 요청을 닫지 않는다: {:?}",
        exec.sites.lock().unwrap()
    );
    let closed = s.handle.cards()[1].closed.clone().unwrap();
    assert!(closed.headline.contains("만료"), "{}", closed.headline);
    // 만료된 카드에 칸을 실어 눌러도 로그인하지 않는다.
    s.desk
        .handle_press(form_press(&card, action(&expiring, true), Some(&fields)))
        .await;
    assert_eq!(exec.sites.lock().unwrap().len(), 1);

    // 카드를 올리지 못해(소유자 대화 없음) 요청을 거절로 닫으면 요청도 닫는다.
    let mut lonely = setup(None, None);
    let exec = fake_login(Ok("x".into()));
    lonely.desk.set_kind(LoginInputKind {
        exec: Some(exec.clone()),
    });
    let id = login_input_request(&lonely, "tok-시험-2003", chrono::Duration::minutes(10));
    lonely.desk.post_card(&id).await;
    assert!(lonely.handle.cards().is_empty());
    assert_eq!(lonely.status(&id), ApprovalStatus::Denied);
    assert_eq!(*exec.sites.lock().unwrap(), ["cancel:tok-시험-2003"]);
    let _ = lonely.presses.try_recv();
}

/// 결정 0063, 끝에서 끝까지: 알림 받을 곳이 앱이면 로그인 입력 카드가 앱으로 가고(폼 버튼), 앱이 칸을 채워 누르면
/// 로그인 모듈이 로그인한다. 사이트가 거절하면 앱으로 새 카드가 간다. 비밀번호는 앱에 간 프레임·상자·저장소에 없다.
#[tokio::test]
async fn login_input_card_round_trips_through_the_app_gateway() {
    use crate::browser::LoginResult;
    use crate::browser::fake::{FakeBrowser, FakeNotifier, FakeSecrets};
    use crate::browser::logins::{FormOutcome, FormStatus, LoginsConfig, Requested, SiteLogins};
    use crate::gateway::{Gateway, RouterChannel, keys};
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    {
        let st = store.lock().unwrap();
        st.set_setting(lucia_store::keys::OWNER_DISCORD_USER_ID, "42", Utc::now())
            .unwrap();
        st.set_setting(lucia_store::keys::NOTIFY_CONVERSATION, CONV, Utc::now())
            .unwrap();
        st.create_client("cl-1", "앱", "app", "h1", Utc::now())
            .unwrap();
        st.set_setting(keys::NOTIFY_CLIENT, "cl-1", Utc::now())
            .unwrap();
    }
    let (itx, _irx) = mpsc::channel(4);
    let (ptx, _prx) = mpsc::channel(4);
    let gw = Gateway::new(store.clone(), itx.clone(), ptx.clone());
    let (discord, handle) = MemoryChannel::with_buttons(2000, itx, ptx);
    let router = RouterChannel::new(Arc::new(discord), gw.clone(), store.clone(), None);
    let (posts_tx, mut posts_rx) = mpsc::unbounded_channel();
    let browser = Arc::new(FakeBrowser::default());
    let notes = Arc::new(FakeNotifier::default());
    let logins = Arc::new(SiteLogins::new(
        browser.clone(),
        Arc::new(FakeSecrets::default()),
        store.clone(),
        notes.clone(),
        Some(posts_tx),
        crate::browser::sites::builtin(),
        LoginsConfig {
            form_base: "http://192.168.0.2:8780".into(),
            input_ttl: chrono::Duration::minutes(10),
            approval_ttl: chrono::Duration::minutes(30),
        },
    ));
    let exec: Arc<dyn SiteLoginExec> = logins.clone();
    let desk = ApprovalDesk::new(
        store.clone(),
        Arc::new(router),
        Access::Owner,
        chrono_tz::Asia::Seoul,
        None,
    )
    .with_builtin_kinds(FakeMailer::default(), Some(exec), None);
    let (_, mut rx) = gw.attach_for_test("cl-1");
    let mut frames: Vec<serde_json::Value> = Vec::new();
    let mut next_frame = |rx: &mut mpsc::UnboundedReceiver<serde_json::Value>| {
        let f = rx.try_recv().expect("앱으로 프레임이 가야 한다");
        frames.push(f.clone());
        f
    };
    let wait_idle = |token: String| {
        let logins = logins.clone();
        async move {
            for _ in 0..400 {
                let status = logins.form_status(&token);
                if !matches!(status, FormStatus::Busy { .. }) {
                    return status;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            panic!("로그인이 시간 안에 끝나지 않았다");
        }
    };

    // 1) 요청 → 카드가 앱으로(Discord에는 가지 않는다).
    assert_eq!(
        logins.request("quasarzone", "시험", None).await.unwrap(),
        Requested::InputCard
    );
    let id = posts_rx.recv().await.unwrap();
    desk.post_card(&id).await;
    assert!(handle.cards().is_empty(), "Discord에는 가지 않는다");
    assert!(notes.0.lock().unwrap().is_empty(), "링크 글도 없다");
    let frame = next_frame(&mut rx);
    assert_eq!(frame["type"], "card");
    let card_ref = frame["ref"].as_str().unwrap().to_owned();
    let form = &frame["card"]["buttons"][0]["form"];
    assert_eq!(form["fields"].as_array().unwrap().len(), 3);
    let token = store
        .lock()
        .unwrap()
        .approval(&id)
        .unwrap()
        .unwrap()
        .payload["token"]
        .as_str()
        .unwrap()
        .to_owned();

    // 2) 앱이 거절당할 값을 채워 누른다 → 카드는 ⏳로 닫히고, 사이트가 거절하면 같은 토큰의 새 카드가 앱으로 간다.
    let press = |card: &str, action: String, password: &str| ButtonPress {
        platform: "app".into(),
        sender: "cl-1".into(),
        conversation: crate::gateway::conversation("cl-1"),
        message_ref: card.to_owned(),
        action,
        input: None,
        fields: Some(
            [
                ("username".to_owned(), lucia_core::SecretText::new("me_1")),
                ("password".to_owned(), lucia_core::SecretText::new(password)),
                ("save".to_owned(), lucia_core::SecretText::new("false")),
            ]
            .into_iter()
            .collect(),
        ),
        received_at: Utc::now(),
    };
    let wrong = "pw-시험-틀림-종단-1101";
    browser
        .logins
        .lock()
        .unwrap()
        .push_back(Ok(LoginResult::Rejected("비밀번호가 다릅니다".into())));
    desk.handle_press(press(&card_ref, action(&id, true), wrong))
        .await;
    let closed = next_frame(&mut rx);
    assert_eq!(
        (closed["type"].as_str(), closed["ref"].as_str()),
        (Some("card_closed"), Some(card_ref.as_str()))
    );
    assert!(
        closed["card"]["headline"]
            .as_str()
            .unwrap()
            .starts_with("⏳")
    );
    let again = tokio::time::timeout(std::time::Duration::from_secs(5), posts_rx.recv())
        .await
        .expect("새 카드가 올라와야 한다")
        .unwrap();
    assert_ne!(again, id);
    desk.post_card(&again).await;
    let frame = next_frame(&mut rx);
    assert_eq!(frame["type"], "card");
    let retry_ref = frame["ref"].as_str().unwrap().to_owned();
    assert_ne!(retry_ref, card_ref);
    let fields = frame["card"]["fields"].as_array().unwrap();
    let prev = fields
        .iter()
        .find(|f| f["name"] == "앞선 시도")
        .expect("앞선 시도의 까닭이 보인다");
    assert!(
        prev["value"]
            .as_str()
            .unwrap()
            .contains("비밀번호가 다릅니다")
    );
    assert_eq!(
        store
            .lock()
            .unwrap()
            .approval(&again)
            .unwrap()
            .unwrap()
            .payload["token"],
        token.as_str(),
        "같은 요청(같은 토큰)이다"
    );
    assert!(
        notes.0.lock().unwrap()[0].contains("새로 올린 카드에서 다시 넣을 수 있습니다"),
        "{:?}",
        notes.0.lock().unwrap()
    );

    // 3) 낡은 카드를 다시 눌러도 이미 결정됐다. 새 카드에 맞는 값을 채워 누르면 로그인된다.
    desk.handle_press(press(&card_ref, action(&id, true), wrong))
        .await;
    assert_eq!(
        browser.passwords.lock().unwrap().len(),
        1,
        "다시 시도하지 않는다"
    );
    let right = "pw-시험-맞음-종단-1102";
    desk.handle_press(press(&retry_ref, action(&again, true), right))
        .await;
    let closed = next_frame(&mut rx);
    assert_eq!(closed["ref"], retry_ref.as_str());
    let status = wait_idle(token.clone()).await;
    assert!(
        matches!(&status, FormStatus::Finished(FormOutcome::Done { .. })),
        "{status:?}"
    );
    assert_eq!(
        *browser.passwords.lock().unwrap(),
        [wrong, right],
        "데몬이 브라우저에 곧장 넣었다"
    );
    assert!(
        notes.0.lock().unwrap().last().unwrap().starts_with("✅"),
        "{:?}",
        notes.0.lock().unwrap()
    );

    // 4) 비밀번호는 앱으로 간 프레임, 상자, 저장소 어디에도 없다.
    let outbox = store.lock().unwrap().outbox_after("cl-1", 0, 100).unwrap();
    assert!(outbox.len() >= 4, "카드와 닫힘이 상자에 있다");
    for secret in [wrong, right] {
        assert!(frames.iter().all(|f| !f.to_string().contains(secret)));
        assert!(
            outbox
                .iter()
                .all(|row| !format!("{row:?}").contains(secret))
        );
        assert!(notes.0.lock().unwrap().iter().all(|n| !n.contains(secret)));
        let st = store.lock().unwrap();
        for a in st.recent_approvals(20).unwrap() {
            assert!(!serde_json::to_string(&a).unwrap().contains(secret));
        }
        let activity = st.recent_activity(50).unwrap();
        assert!(!activity.is_empty());
        assert!(activity.iter().all(|a| !a.summary.contains(secret)));
        let episodes = st.search_episodes("로그인", 50).unwrap();
        assert!(!episodes.is_empty());
        assert!(episodes.iter().all(|e| !e.body.contains(secret)));
    }
}
