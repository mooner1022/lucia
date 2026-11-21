//! 브라우저(결정 0006·0008): 로그인 프로필을 유지하는 전용 샌드박스의 Chrome을 데몬이 CDP로 다룬다.
//!
//! - Lucia(모델)는 페이지를 열고, 화면을 보고(요소 목록·스크린샷), 클릭·입력·스크롤·뒤로 가기를 한다
//!   ([`tools`]). 외부에 흔적을 남길 수 있는 동작(POST 폼 제출, 무엇을 할지 모르는 스크립트 버튼)은 Discord 승인을
//!   받고, 결제·계정 설정·로그아웃은 하지 않는다(명세 §권한 등급). JavaScript 실행 도구는 주지 않는다.
//! - 갈 수 있는 곳은 차단 목록 방식이다([`egress`]): 모든 연결이 데몬 프록시([`proxy`])를 지난다.
//! - 로그인은 데몬의 정해진 코드가 한다. 비밀번호는 모델 경로에 두지 않는다(명세 §브라우저와 웹사이트 로그인).
//!   사람은 관리 페이지의 일회용 입력 링크로 넣고, 저장된 로그인을 다시 쓸 때는 Discord 승인을 받는다([`logins`]).
//! - CDP는 포트를 열지 않고 붙잡아 두는 `sbx exec -i` 세션의 표준 입출력으로 잇는다(PoC P8, [`sandbox`]).

pub mod cdp;
pub mod devcheck;
pub mod egress;
pub mod logins;
pub mod proxy;
pub mod sandbox;
pub mod sites;
pub mod tools;

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

pub use sites::SiteProfile;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 요소가 속한 폼.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormInfo {
    /// `get`, `post`, `dialog`.
    pub method: String,
    /// 보낼 곳(절대 주소).
    pub action: String,
    /// 보이는 칸의 (이름, 값). 비밀번호 칸과 숨은 칸은 넣지 않는다.
    #[serde(default)]
    pub fields: Vec<(String, String)>,
    /// 숨은 칸 수.
    #[serde(default)]
    pub hidden: usize,
    /// 비밀번호 칸이 있다(로그인 폼).
    #[serde(default)]
    pub password: bool,
}

/// 화면의 조작할 수 있는 요소 하나(데몬 스크립트가 격리된 실행 공간에서 읽은 것).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Element {
    /// `e12` 같은 참조(그 화면에서만 유효).
    #[serde(rename = "ref")]
    pub r: String,
    /// link, button, submit, reset, text, textarea, select, checkbox, radio, password, file, tab, summary,
    /// editable, hidden.
    pub kind: String,
    pub tag: String,
    pub label: String,
    #[serde(default)]
    pub href: Option<String>,
    #[serde(default)]
    pub input_type: Option<String>,
    /// 지금 값(글 칸, 선택). 비밀번호 칸은 늘 비운다.
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub checked: Option<bool>,
    #[serde(default)]
    pub disabled: bool,
    /// `aria-expanded`가 있다(펼치기 단추).
    #[serde(default)]
    pub expanded: Option<bool>,
    /// `aria-haspopup`(메뉴를 여는 단추).
    #[serde(default)]
    pub popup: bool,
    /// 검색 칸으로 보인다.
    #[serde(default)]
    pub search: bool,
    /// 지금 화면(뷰포트) 안에 있다.
    #[serde(default)]
    pub in_view: bool,
    #[serde(default)]
    pub form: Option<FormInfo>,
    /// 선택 상자의 선택지(앞의 30개).
    #[serde(default)]
    pub options: Vec<String>,
    /// iframe 안의 요소면 그 프레임의 주소(결정 0023). 최상위 문서면 없다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
}

impl Element {
    /// 같은 대상인가(값은 보지 않는다): 사이에 페이지가 요소를 바꿔치기했는지 본다.
    pub fn same_target(&self, other: &Element) -> bool {
        self.kind == other.kind
            && self.tag == other.tag
            && self.label == other.label
            && self.href == other.href
            && self.input_type == other.input_type
            && self.form.as_ref().map(|f| (&f.method, &f.action))
                == other.form.as_ref().map(|f| (&f.method, &f.action))
            && self.frame == other.frame
    }
}

impl Element {
    /// 확인하지 못한 대상(승인 카드와 잠금 안 재확인에 쓴다).
    pub fn unknown(why: &str) -> Self {
        Self {
            r: String::new(),
            kind: "unknown".into(),
            tag: String::new(),
            label: why.chars().take(100).collect(),
            href: None,
            input_type: None,
            value: None,
            checked: None,
            disabled: false,
            expanded: None,
            popup: false,
            search: false,
            in_view: true,
            form: None,
            options: Vec::new(),
            frame: None,
        }
    }
}

impl Hit {
    /// 판정과 재확인의 대상(확인하지 못했으면 그 까닭을 단 "모르는 곳").
    pub fn target(&self) -> Element {
        self.element
            .clone()
            .unwrap_or_else(|| Element::unknown(self.why.as_deref().unwrap_or("확인하지 못한 곳")))
    }
}

impl Focus {
    /// 판정과 재확인의 대상.
    pub fn target(&self) -> Element {
        match self {
            Self::On { element } => element.clone(),
            Self::Nothing => Element {
                kind: "area".into(),
                ..Element::unknown("문서 본문(초점 없음)")
            },
            Self::Unknown { why } => Element::unknown(why),
        }
    }
}

/// 화면 좌표(CSS 픽셀, 지금 보이는 화면의 왼쪽 위가 0,0). 스크린샷 픽셀과 같다(결정 0023).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// iframe 하나의 화면(결정 0023): 본문 글과 요소(참조 대신 화면 좌표, `browser_click_at`으로 누른다).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FrameView {
    pub url: String,
    /// 화면에서 프레임 안쪽 영역(x, y, 너비, 높이).
    pub rect: (i32, i32, i32, i32),
    pub text: String,
    /// 지금 보이는 요소와 그 가운데 점(화면 좌표).
    pub elements: Vec<(Point, Element)>,
}

/// 화면 좌표의 히트 테스트 결과(결정 0023). 데몬이 그 점에서 직접 읽은 것이다(모델의 말이 아니다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// 최상위 페이지 주소.
    pub url: String,
    /// 화면 크기(너비, 높이).
    pub viewport: (i32, i32),
    /// 그 점의 요소(누를 수 있는 가장 가까운 조상, 없으면 그 점의 요소). `None`이면 확인하지 못했다.
    pub element: Option<Element>,
    /// 누를 수 있는 요소가 아니지만 모양(손가락 커서)이 누르는 곳이다.
    pub pointer: bool,
    /// 확인하지 못한 까닭.
    pub why: Option<String>,
}

/// 지금 초점(결정 0023). 잠깐 쓰고 버리는 값이라 크기 차이는 두었다.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "focus", rename_all = "snake_case")]
pub enum Focus {
    /// 초점이 있는 요소가 없다(문서 본문).
    Nothing,
    On {
        element: Element,
    },
    /// 확인하지 못했다.
    Unknown {
        why: String,
    },
}

/// 지금 화면. 입력 칸의 값 중 비밀번호는 담지 않는다.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageView {
    /// 실제로 도착한 주소(넘겨주기 뒤).
    pub url: String,
    pub title: String,
    pub text: String,
    pub elements: Vec<Element>,
    /// 요소가 더 있어 목록을 줄였으면 전체 수.
    pub element_total: usize,
    /// 보이는 비밀번호 칸이 있다(로그인 폼).
    pub login_form: bool,
    /// (스크롤 위치, 화면 높이, 문서 높이).
    pub scroll: (i64, i64, i64),
    /// 이 동작 동안 자동으로 닫은 대화상자.
    pub dialogs: Vec<String>,
    /// 동작 중 새 창이 열려 닫았으면 그 주소.
    pub popups: Vec<String>,
    /// 데몬 판단: 이 사이트(내장 프로필)의 로그인 페이지로 튕겼다.
    pub login_redirect: bool,
    /// 화면에 보이는 iframe들(결정 0023).
    pub frames: Vec<FrameView>,
    /// 화면 크기(너비, 높이). 모르면 (0, 0).
    pub viewport: (i32, i32),
}

/// 스크린샷(JPEG).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    pub url: String,
    pub title: String,
    pub jpeg: Vec<u8>,
    /// 화면 크기(너비, 높이, CSS 픽셀). 화면 스크린샷이면 이미지 픽셀과 같다(결정 0023).
    pub viewport: (i32, i32),
}

/// 스크롤 방향.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scroll {
    Down,
    Up,
    Top,
    Bottom,
}

/// 모델이 요청하는 화면 동작.
///
/// 승인 대기 중인 동작이 저장소에 JSON으로 남으므로, 모양을 바꿀 때는 예전 것도 읽히게 한다(새 칸은 기본값).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Act {
    Click {
        r: String,
    },
    /// 화면 좌표를 누른다(결정 0023). iframe 안도 된다.
    ClickAt {
        at: Point,
    },
    /// 지금 초점이 있는 칸의 커서 자리에 글을 넣는다(결정 0023). 기존 값은 지우지 않는다.
    TypeText {
        text: String,
        /// 넣은 뒤 Enter.
        #[serde(default)]
        submit: bool,
    },
    /// 키 하나(정해진 것만, [`KEYS`]).
    Key {
        key: String,
    },
    /// 글 칸에 넣는다(기존 값은 지운다). 선택 상자는 선택지 이름이나 값으로 고른다.
    Type {
        r: String,
        text: String,
        /// 넣은 뒤 Enter로 제출한다.
        submit: bool,
    },
    Scroll {
        to: Scroll,
        /// 이 점 아래의 스크롤 영역(iframe, 목록 상자)을 굴린다. 없으면 페이지.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<Point>,
    },
    Back,
}

/// `browser_key`가 받는 키(결정 0023). 글자 키는 받지 않는다(편집 칸 밖에서는 웹앱 단축키가 된다).
pub const KEYS: &[&str] = &[
    "enter",
    "space",
    "tab",
    "shift+tab",
    "escape",
    "backspace",
    "delete",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
    "select_all",
];

impl Act {
    pub fn target(&self) -> Option<&str> {
        match self {
            Self::Click { r } | Self::Type { r, .. } => Some(r),
            _ => None,
        }
    }
}

/// 동작 직전에 다시 확인할 것: 그 주소의 그 요소(승인 카드에 보인 것)인가.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expect {
    pub url: String,
    pub element: Element,
    /// 사람이 승인한 동작이다: 이 동안의 확인(confirm) 대화상자는 받아들인다. 아니면 취소한다.
    #[serde(default)]
    pub approved: bool,
}

/// 로그인 시도 결과(브라우저는 정상으로 돌았다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginResult {
    LoggedIn,
    /// 이미 로그인돼 있었다(로그인 페이지가 다른 곳으로 넘겼다).
    AlreadyLoggedIn,
    /// 사이트가 받아들이지 않았다(이유: 사이트가 보인 문구, 비밀번호는 가렸다).
    Rejected(String),
    /// 비밀번호 뒤 인증 코드(OTP) 칸이 나왔다(결정 0017). 페이지는 그 단계에 그대로 둔다. `hint`는 칸 이름.
    NeedsCode {
        hint: String,
    },
    /// 사람이 풀어야 하는 단계(캡차, 추가 보안 확인, 새 기기 확인 등)가 나왔다. 페이지는 그 단계에 그대로 둔다.
    /// 관리 페이지의 화면 넘겨받기(결정 0017)로 사람이 푼다. `why`는 화면의 문구.
    NeedsHuman {
        why: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserError {
    /// 열 수 없는 주소(막힌 곳으로 넘어감 등).
    NotAllowed(String),
    /// 브라우저를 띄우지 못했다(샌드박스 없음, 시간 초과 등).
    Unavailable(String),
    /// 화면이 바뀌어 그 요소가 없거나 달라졌다(다시 보고 해야 한다).
    Stale(String),
    /// 그 밖의 실패.
    Failed(String),
    /// 다른 조작이 쥐고 있다(감시 읽기는 기다리지 않는다).
    Busy,
}

/// 브라우저가 시작하려던 내려받기(결정 0057). 브라우저는 받지 않고, 데몬이 이 주소를 같은 검사로 받는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenDownload {
    pub url: String,
    /// 브라우저가 정한 파일 이름(응답의 이름이나 링크의 `download` 속성).
    pub name: String,
}

/// 한 동작에서 기억하는 내려받기 수.
pub const SEEN_DOWNLOADS_MAX: usize = 5;

/// 브라우저에서 빌린 세션(결정 0036).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SessionCookies {
    /// (이름, 값).
    pub cookies: Vec<(String, String)>,
    pub user_agent: String,
}

impl std::fmt::Debug for SessionCookies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 값은 보이지 않는다.
        f.debug_struct("SessionCookies")
            .field("count", &self.cookies.len())
            .finish()
    }
}

impl std::fmt::Display for BrowserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAllowed(m) => write!(f, "열 수 없는 주소: {m}"),
            Self::Busy => write!(f, "브라우저가 다른 일로 바쁘다"),
            Self::Unavailable(m) => write!(f, "브라우저를 쓸 수 없다: {m}"),
            Self::Stale(m) => write!(f, "화면이 바뀌었다: {m}"),
            Self::Failed(m) => write!(f, "브라우저 오류: {m}"),
        }
    }
}

impl std::error::Error for BrowserError {}

/// 브라우저 조작. 실제는 [`sandbox::SbxBrowser`], 시험은 가짜.
pub trait Browser: Send + Sync {
    /// 주소를 열고 화면을 읽는다. 호출하는 쪽이 주소를 먼저 검사한다(도착한 주소는 여기서 다시 본다).
    /// `site`가 있으면 그 사이트의 로그인 페이지로 튕겼는지도 본다.
    fn open<'a>(
        &'a self,
        url: &'a str,
        site: Option<&'a SiteProfile>,
    ) -> BoxFut<'a, Result<PageView, BrowserError>>;
    /// 이동 없이 지금 화면을 다시 읽는다(요소 참조를 새로 매긴다).
    fn view(&self) -> BoxFut<'_, Result<PageView, BrowserError>>;
    /// 지금 화면(또는 페이지 전체)을 찍는다.
    fn screenshot(&self, full_page: bool) -> BoxFut<'_, Result<Shot, BrowserError>>;
    /// 요소를 지금 상태로 다시 읽는다: (페이지 주소, 요소).
    fn inspect<'a>(&'a self, r: &'a str) -> BoxFut<'a, Result<(String, Element), BrowserError>>;
    /// 동작을 한다. `expect`가 있으면 같은 주소·같은 요소인지 잠금 안에서 다시 확인한 뒤에 한다.
    fn act<'a>(
        &'a self,
        act: &'a Act,
        expect: Option<&'a Expect>,
    ) -> BoxFut<'a, Result<PageView, BrowserError>>;
    /// 로그인 페이지에서 칸을 채우고 제출한다(한 번만 시도한다).
    fn login<'a>(
        &'a self,
        site: &'a SiteProfile,
        username: &'a str,
        password: &'a str,
    ) -> BoxFut<'a, Result<LoginResult, BrowserError>>;
    /// 인증 코드 단계(결정 0017): 같은 사이트의 그 칸에 코드를 넣고 제출한다(한 번만 시도한다).
    fn submit_code<'a>(
        &'a self,
        site: &'a SiteProfile,
        code: &'a str,
    ) -> BoxFut<'a, Result<LoginResult, BrowserError>>;
    /// 지금 브라우저가 떠 있는가(상태 화면용).
    fn running(&self) -> bool;

    /// 그 주소로 보낼 쿠키(이름, 값)와 브라우저의 사용자 에이전트(결정 0036, 로그인 세션 빌려 읽기).
    /// 브라우저가 꺼져 있으면 띄운다. 값은 로그에 남기지 않는다.
    fn cookies<'a>(&'a self, _url: &'a str) -> BoxFut<'a, Result<SessionCookies, BrowserError>> {
        Box::pin(async { Err(BrowserError::Unavailable("쿠키를 읽을 수 없다".into())) })
    }

    /// 감시 탭에서 주소를 열고 그려진 문서(HTML)와 도착한 주소를 돌려준다(결정 0036). 대화용 탭은 건드리지 않는다.
    /// 다른 조작이 브라우저를 쥐고 있으면 기다리지 않고 [`BrowserError::Busy`].
    fn read_page<'a>(
        &'a self,
        _url: &'a str,
    ) -> BoxFut<'a, Result<(String, String), BrowserError>> {
        Box::pin(async {
            Err(BrowserError::Unavailable("브라우저로 읽을 수 없다".into()))
        })
    }

    /// 브라우저 방식 감시가 있는 동안 쓰지 않아도 닫지 않는다.
    fn set_keep_alive(&self, _on: bool) {}

    /// 마지막으로 꺼낸 뒤 브라우저가 시작하려던 내려받기를 꺼낸다(결정 0057).
    fn take_downloads(&self) -> Vec<SeenDownload> {
        Vec::new()
    }

    /// 지금 페이지에서 스크립트를 돈다(결정 0059): (돌려준 값, 그 뒤의 화면).
    fn eval<'a>(&'a self, _code: &'a str) -> BoxFut<'a, Result<(String, PageView), BrowserError>> {
        Box::pin(async {
            Err(BrowserError::Unavailable(
                "이 브라우저에서는 스크립트를 돌릴 수 없다".into(),
            ))
        })
    }

    /// 파일 고르기 칸(`r`)에 파일을 넣는다(결정 0059).
    fn upload<'a>(
        &'a self,
        _r: &'a str,
        _name: &'a str,
        _bytes: &'a [u8],
    ) -> BoxFut<'a, Result<PageView, BrowserError>> {
        Box::pin(async {
            Err(BrowserError::Unavailable(
                "이 브라우저에서는 파일을 올릴 수 없다".into(),
            ))
        })
    }

    /// 화면 좌표의 요소를 읽는다(결정 0023, iframe 안까지).
    fn hit(&self, at: Point) -> BoxFut<'_, Result<Hit, BrowserError>>;
    /// 지금 초점이 있는 요소(iframe 안까지): (페이지 주소, 초점).
    fn focused(&self) -> BoxFut<'_, Result<(String, Focus), BrowserError>>;

    /// 화면 넘겨받기(결정 0017): 켜면 새 창(팝업)을 닫지 않고 탭으로 남긴다. 끄면 남은 탭을 닫는다.
    fn set_handover<'a>(&'a self, on: bool) -> BoxFut<'a, Result<(), BrowserError>> {
        let _ = on;
        Box::pin(async {
            Err(BrowserError::Unavailable(
                "이 브라우저는 넘겨받기를 못 한다".into(),
            ))
        })
    }
    /// 넘겨받은 화면의 탭들(첫째가 원래 페이지).
    fn handover_tabs(&self) -> BoxFut<'_, Result<Vec<Tab>, BrowserError>> {
        Box::pin(async {
            Err(BrowserError::Unavailable(
                "이 브라우저는 넘겨받기를 못 한다".into(),
            ))
        })
    }
    /// 탭 하나의 지금 화면(JPEG)과 크기(CSS 픽셀).
    fn handover_frame<'a>(&'a self, tab: &'a str) -> BoxFut<'a, Result<Frame, BrowserError>> {
        let _ = tab;
        Box::pin(async {
            Err(BrowserError::Unavailable(
                "이 브라우저는 넘겨받기를 못 한다".into(),
            ))
        })
    }
    /// 사람의 입력을 그 탭에 그대로 보낸다(기록하지 않는다).
    fn handover_input<'a>(
        &'a self,
        tab: &'a str,
        input: &'a HandInput,
    ) -> BoxFut<'a, Result<(), BrowserError>> {
        let _ = (tab, input);
        Box::pin(async {
            Err(BrowserError::Unavailable(
                "이 브라우저는 넘겨받기를 못 한다".into(),
            ))
        })
    }
}

/// 넘겨받은 화면의 탭.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Tab {
    pub id: String,
    pub url: String,
    pub title: String,
}

/// 넘겨받은 화면 한 장(JPEG, 브라우저 화면 크기 그대로라 좌표가 같다).
#[derive(Debug, Clone)]
pub struct Frame {
    pub jpeg: Vec<u8>,
}

/// 사람이 넘겨받아 보내는 입력(결정 0017). 글은 기록하지 않는다.
#[derive(Clone, PartialEq, Eq)]
pub enum HandInput {
    /// 화면 좌표(CSS 픽셀) 클릭.
    Click { x: u32, y: u32 },
    /// 초점이 있는 칸에 글을 넣는다.
    Text(String),
    /// 키 하나: enter, tab, backspace, escape, up, down.
    Key(String),
    /// 아래(양수)·위(음수)로 스크롤.
    Scroll(i32),
}

impl std::fmt::Debug for HandInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Click { x, y } => write!(f, "Click({x},{y})"),
            Self::Text(t) => write!(f, "Text(…{}자)", t.chars().count()),
            Self::Key(k) => write!(f, "Key({k})"),
            Self::Scroll(d) => write!(f, "Scroll({d})"),
        }
    }
}

/// 시험용 가짜 브라우저: 정해 둔 화면과 결과를 돌려주고 호출을 기록한다.
#[cfg(test)]
pub mod fake {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    use super::logins::{Notifier, SecretStore};
    use super::*;

    /// 비밀 저장소 가짜(실제처럼 앞뒤 공백을 다듬어 저장한다).
    #[derive(Default)]
    pub struct FakeSecrets(pub Mutex<HashMap<String, String>>);

    impl SecretStore for FakeSecrets {
        fn read<'a>(&'a self, name: &'a str) -> BoxFut<'a, anyhow::Result<Option<String>>> {
            let v = self.0.lock().unwrap().get(name).cloned();
            Box::pin(async move { Ok(v) })
        }
        fn write<'a>(&'a self, name: &'a str, value: &'a str) -> BoxFut<'a, anyhow::Result<()>> {
            self.0
                .lock()
                .unwrap()
                .insert(name.to_owned(), value.trim().to_owned());
            Box::pin(async { Ok(()) })
        }
        fn remove<'a>(&'a self, name: &'a str) -> BoxFut<'a, anyhow::Result<()>> {
            self.0.lock().unwrap().remove(name);
            Box::pin(async { Ok(()) })
        }
        fn exists(&self, name: &str) -> bool {
            self.0.lock().unwrap().contains_key(name)
        }
    }

    /// 알림 가짜: 보낸 글을 모은다.
    #[derive(Default)]
    pub struct FakeNotifier(pub Mutex<Vec<String>>);

    impl Notifier for FakeNotifier {
        fn notify(&self, text: String) -> BoxFut<'_, bool> {
            self.0.lock().unwrap().push(text);
            Box::pin(async { true })
        }
    }

    #[derive(Default)]
    pub struct FakeBrowser {
        pub pages: Mutex<VecDeque<Result<PageView, BrowserError>>>,
        pub logins: Mutex<VecDeque<Result<LoginResult, BrowserError>>>,
        /// `inspect`가 돌려줄 요소(참조별). 없으면 `Stale`.
        pub elements: Mutex<HashMap<String, Element>>,
        /// 지금 주소(`inspect`가 돌려준다).
        pub url: Mutex<String>,
        /// ("open", 주소) · ("login", 아이디) · ("act", 동작 JSON) · ("view", "") · ("shot", "").
        pub calls: Mutex<Vec<(String, String)>>,
        /// `act`가 받은 확인 값.
        pub expects: Mutex<Vec<Option<Expect>>>,
        /// 받은 비밀번호(시험에서 그대로 넘어갔는지 본다).
        pub passwords: Mutex<Vec<String>>,
        /// `hit`이 돌려줄 결과(차례로). 없으면 아무것도 확인하지 못함.
        pub hits: Mutex<VecDeque<Hit>>,
        /// `focused`가 돌려줄 초점(차례로). 없으면 `Nothing`.
        pub focuses: Mutex<VecDeque<Focus>>,
        /// `cookies`가 돌려줄 세션(차례로, 마지막 것은 남긴다).
        pub sessions: Mutex<VecDeque<SessionCookies>>,
        /// `read_page`가 돌려줄 (HTML, 도착 주소)(차례로).
        pub reads: Mutex<VecDeque<Result<(String, String), BrowserError>>>,
        pub keep_alive: std::sync::atomic::AtomicBool,
        /// `take_downloads`가 돌려줄 내려받기(한 번 꺼내면 빈다).
        pub downloads: Mutex<Vec<SeenDownload>>,
    }

    impl FakeBrowser {
        pub fn page(text: &str, login_form: bool) -> PageView {
            PageView {
                url: "https://quasarzone.com/bbs/x".into(),
                title: "제목".into(),
                text: text.into(),
                elements: vec![link("e1", "다음 글", "https://quasarzone.com/bbs/y")],
                element_total: 1,
                login_form,
                scroll: (0, 900, 3000),
                ..PageView::default()
            }
        }

        fn next_page(&self) -> Result<PageView, BrowserError> {
            self.pages
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(Self::page("본문", false)))
        }

        fn call(&self, kind: &str, arg: String) {
            self.calls.lock().unwrap().push((kind.to_owned(), arg));
        }
    }

    pub fn element(r: &str, kind: &str, label: &str) -> Element {
        Element {
            r: r.into(),
            kind: kind.into(),
            tag: match kind {
                "link" => "A",
                "text" | "password" | "checkbox" => "INPUT",
                "textarea" => "TEXTAREA",
                "select" => "SELECT",
                _ => "BUTTON",
            }
            .into(),
            label: label.into(),
            href: None,
            input_type: None,
            value: None,
            checked: None,
            disabled: false,
            expanded: None,
            popup: false,
            search: false,
            in_view: true,
            form: None,
            options: Vec::new(),
            frame: None,
        }
    }

    pub fn link(r: &str, label: &str, href: &str) -> Element {
        Element {
            href: Some(href.into()),
            ..element(r, "link", label)
        }
    }

    impl Browser for FakeBrowser {
        fn open<'a>(
            &'a self,
            url: &'a str,
            _site: Option<&'a SiteProfile>,
        ) -> BoxFut<'a, Result<PageView, BrowserError>> {
            self.call("open", url.to_owned());
            let next = self.next_page();
            Box::pin(async move { next })
        }

        fn view(&self) -> BoxFut<'_, Result<PageView, BrowserError>> {
            self.call("view", String::new());
            let next = self.next_page();
            Box::pin(async move { next })
        }

        fn screenshot(&self, _full_page: bool) -> BoxFut<'_, Result<Shot, BrowserError>> {
            self.call("shot", String::new());
            Box::pin(async {
                Ok(Shot {
                    url: "https://quasarzone.com/".into(),
                    title: "제목".into(),
                    jpeg: vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0],
                    viewport: (1365, 812),
                })
            })
        }

        fn inspect<'a>(
            &'a self,
            r: &'a str,
        ) -> BoxFut<'a, Result<(String, Element), BrowserError>> {
            let found = self.elements.lock().unwrap().get(r).cloned();
            let url = self.url.lock().unwrap().clone();
            Box::pin(async move {
                found
                    .map(|e| (url, e))
                    .ok_or_else(|| BrowserError::Stale(format!("{r} 요소가 없다")))
            })
        }

        fn act<'a>(
            &'a self,
            act: &'a Act,
            expect: Option<&'a Expect>,
        ) -> BoxFut<'a, Result<PageView, BrowserError>> {
            self.call("act", serde_json::to_string(act).unwrap());
            self.expects.lock().unwrap().push(expect.cloned());
            let next = self.next_page();
            Box::pin(async move { next })
        }

        fn login<'a>(
            &'a self,
            site: &'a SiteProfile,
            username: &'a str,
            password: &'a str,
        ) -> BoxFut<'a, Result<LoginResult, BrowserError>> {
            self.call("login", format!("{}:{username}", site.id));
            self.passwords.lock().unwrap().push(password.to_owned());
            let next = self
                .logins
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(LoginResult::LoggedIn));
            Box::pin(async move { next })
        }

        fn submit_code<'a>(
            &'a self,
            site: &'a SiteProfile,
            code: &'a str,
        ) -> BoxFut<'a, Result<LoginResult, BrowserError>> {
            self.call("code", site.id.clone());
            self.passwords.lock().unwrap().push(code.to_owned());
            let next = self
                .logins
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(LoginResult::LoggedIn));
            Box::pin(async move { next })
        }

        fn running(&self) -> bool {
            true
        }

        fn cookies<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<SessionCookies, BrowserError>> {
            self.call("cookies", url.to_owned());
            let mut q = self.sessions.lock().unwrap();
            let next = if q.len() > 1 {
                q.pop_front()
            } else {
                q.front().cloned()
            };
            Box::pin(async move {
                next.ok_or_else(|| BrowserError::Unavailable("세션 없음".into()))
            })
        }

        fn read_page<'a>(
            &'a self,
            url: &'a str,
        ) -> BoxFut<'a, Result<(String, String), BrowserError>> {
            self.call("read", url.to_owned());
            let next = self
                .reads
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(BrowserError::Failed("읽을 것 없음".into())));
            Box::pin(async move { next })
        }

        fn set_keep_alive(&self, on: bool) {
            self.keep_alive
                .store(on, std::sync::atomic::Ordering::SeqCst);
        }

        fn take_downloads(&self) -> Vec<SeenDownload> {
            std::mem::take(&mut *self.downloads.lock().unwrap())
        }

        fn eval<'a>(
            &'a self,
            code: &'a str,
        ) -> BoxFut<'a, Result<(String, PageView), BrowserError>> {
            self.call("eval", code.to_owned());
            let next = self.pages.lock().unwrap().pop_front();
            Box::pin(async move {
                match next {
                    Some(Ok(view)) => Ok(("\"값\"".to_owned(), view)),
                    Some(Err(e)) => Err(e),
                    None => Err(BrowserError::Failed("준비한 화면이 없다".into())),
                }
            })
        }

        fn upload<'a>(
            &'a self,
            r: &'a str,
            name: &'a str,
            bytes: &'a [u8],
        ) -> BoxFut<'a, Result<PageView, BrowserError>> {
            self.call("upload", format!("{r} {name} {}", bytes.len()));
            let next = self.pages.lock().unwrap().pop_front();
            Box::pin(async move {
                match next {
                    Some(view) => view,
                    None => Err(BrowserError::Failed("준비한 화면이 없다".into())),
                }
            })
        }

        fn hit(&self, at: Point) -> BoxFut<'_, Result<Hit, BrowserError>> {
            self.call("hit", format!("{},{}", at.x, at.y));
            let next = self.hits.lock().unwrap().pop_front().unwrap_or(Hit {
                url: self.url.lock().unwrap().clone(),
                viewport: (1365, 812),
                element: None,
                pointer: false,
                why: Some("가짜".into()),
            });
            Box::pin(async move { Ok(next) })
        }

        fn focused(&self) -> BoxFut<'_, Result<(String, Focus), BrowserError>> {
            self.call("focused", String::new());
            let next = self
                .focuses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Focus::Nothing);
            let url = self.url.lock().unwrap().clone();
            Box::pin(async move { Ok((url, next)) })
        }
    }
}
