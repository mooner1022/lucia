//! 플랫폼 중립 타입.
//!
//! - 메시지 모델: 채널 어댑터와 코어 사이에서 오가는 들어오는·나가는 메시지(명세 §메신저 추상화)
//! - 허용 사용자: 어댑터는 플랫폼 ID만 넘기고, 허용 여부는 여기서 판단한다. 기본은 거부.
//!
//! Discord 같은 특정 플랫폼 개념은 이 크레이트에 들어오지 않는다.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 내부 사용자 ID. 1인용이지만 플랫폼 ID와 섞이지 않게 따로 둔다.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(pub String);

/// 플랫폼 중립 대화 ID(어댑터가 만든다. 예: 어댑터 이름 + 채널 ID).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationId(pub String);

/// 들어온 첨부. 어댑터는 주소와 정보만 넘기고, 내려받기는 코어가 소유자 확인 뒤에 한다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundAttachment {
    pub url: String,
    pub filename: String,
    /// 플랫폼이 알려 준 MIME(없을 수 있고, 믿지 않고 다시 확인한다).
    pub content_type: Option<String>,
    /// 플랫폼이 알려 준 크기(바이트).
    pub size: u64,
}

/// 들어오는 메시지.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inbound {
    /// 어댑터 이름(예: "discord", "memory").
    pub platform: String,
    /// 플랫폼의 사용자 ID. 허용 여부는 [`Allowlist`]가 판단한다.
    pub sender: String,
    pub conversation: ConversationId,
    pub body: String,
    /// 첨부 파일(없으면 빈 목록). 글 없이 파일만 올 수도 있다.
    pub attachments: Vec<InboundAttachment>,
    pub received_at: DateTime<Utc>,
    /// 답장한 원문(결정 0024). 다른 사람이 쓴 글일 수 있다(외부 데이터).
    pub reply_to: Option<Quoted>,
    /// 여러 사람이 보는 곳(서버 채널)에서 왔으면 그 곳(결정 0024). 1:1 대화면 `None`.
    pub venue: Option<Venue>,
}

impl Inbound {
    /// 1:1 대화의 글 하나(답장·장소 없음).
    pub fn direct(
        platform: &str,
        sender: &str,
        conversation: ConversationId,
        body: &str,
        received_at: DateTime<Utc>,
    ) -> Self {
        Self {
            platform: platform.to_owned(),
            sender: sender.to_owned(),
            conversation,
            body: body.to_owned(),
            attachments: Vec::new(),
            received_at,
            reply_to: None,
            venue: None,
        }
    }
}

/// 답장한 원문(결정 0024).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quoted {
    /// 플랫폼의 메시지 참조.
    pub id: String,
    /// 쓴 사람의 표시 이름.
    pub author: String,
    /// 쓴 사람의 플랫폼 ID.
    pub author_id: String,
    /// Lucia(봇) 자신이 쓴 메시지.
    pub from_self: bool,
    pub body: String,
    /// 원문의 첨부(받을지는 코어가 정한다).
    pub attachments: Vec<InboundAttachment>,
    pub sent_at: Option<DateTime<Utc>>,
}

/// 여러 사람이 보는 곳(결정 0024). 이름은 어댑터가 알아낸 만큼(모르면 `None`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Venue {
    /// 서버(공간) ID.
    pub space_id: String,
    pub space_name: Option<String>,
    /// 채널(방) ID.
    pub room_id: String,
    pub room_name: Option<String>,
    /// 사용자가 Lucia를 불렀다(멘션, 정한 머리말, Lucia 메시지에 답장). 아니면 그곳의 보통 말(작업 스레드 등).
    pub addressed: bool,
}

/// 보낼 파일.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// 나가는 메시지. 데몬만 만든다(에이전트의 자유 텍스트를 그대로 흘리지 않는다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outbound {
    /// 마크다운 하위 집합 텍스트.
    Text {
        conversation: ConversationId,
        body: String,
    },
    /// 진행 상황 같은 작은 안내(예: "웹 검색: …"). 어댑터가 덜 눈에 띄게 그린다.
    Note {
        conversation: ConversationId,
        body: String,
    },
    /// 파일을 붙인 메시지. 본문은 비어도 된다. 본문이 길면 나눠 보내고 파일은 마지막 조각에 붙인다.
    Files {
        conversation: ConversationId,
        body: String,
        files: Vec<OutFile>,
    },
}

impl Outbound {
    pub fn conversation(&self) -> &ConversationId {
        match self {
            Self::Text { conversation, .. }
            | Self::Note { conversation, .. }
            | Self::Files { conversation, .. } => conversation,
        }
    }
}

/// 카드의 버튼 하나. `action`은 누르면 [`ButtonPress::action`]으로 돌아온다(어댑터는 해석하지 않는다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardButton {
    pub action: String,
    pub label: String,
    /// 되돌릴 수 없는 일(보내기 등)은 강조한다.
    pub primary: bool,
    /// 누르면 짧은 값을 하나 받는다(예: 인증 코드, 결정 0017). 받은 값은 [`ButtonPress::input`]으로 온다.
    /// 능력 `prompts`가 없는 채널은 이 버튼을 그리지 않는다(만든 쪽이 다른 길을 함께 준다).
    pub prompt: Option<ButtonPrompt>,
    /// 누르면 여러 칸을 받는다(아이디·비밀번호 등, 결정 0063). 받은 값은 [`ButtonPress::fields`]로 온다.
    /// 능력 `forms`가 없는 채널(Discord 등 값이 남의 서버를 거치는 곳)은 이 버튼을 **그리지 않는다**
    /// (만든 쪽이 다른 길을 함께 준다).
    pub form: Option<ButtonForm>,
}

/// 버튼이 받는 여러 칸(결정 0063).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonForm {
    /// 입력창 제목(고정 문구).
    pub title: String,
    pub fields: Vec<FormField>,
}

/// 입력 칸 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    /// 값의 이름([`ButtonPress::fields`]의 키).
    pub key: String,
    /// 칸 이름(고정 문구).
    pub label: String,
    pub kind: FormFieldKind,
    /// 글 칸의 길이 범위(문자 수). 고르는 칸에는 뜻이 없다.
    pub min_len: u16,
    pub max_len: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormFieldKind {
    /// 보이는 글(아이디).
    Text,
    /// 가려서 받는 글(비밀번호). 받는 쪽은 저장하거나 기록하지 않는다.
    Secret,
    /// 켜고 끄는 칸. 값은 `"true"`·`"false"`.
    Check,
}

impl FormFieldKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Secret => "secret",
            Self::Check => "check",
        }
    }
}

/// 버튼이 받는 값의 모양.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonPrompt {
    /// 입력창 제목(고정 문구, 45자까지).
    pub title: String,
    /// 칸 이름(고정 문구, 45자까지).
    pub label: String,
    pub placeholder: Option<String>,
    pub min_len: u16,
    pub max_len: u16,
}

/// 사람이 넣은 짧은 비밀 값(인증 코드 등). 로그·`Debug`에 값이 찍히지 않는다.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretText(String);

impl SecretText {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretText(…{}자)", self.0.chars().count())
    }
}

/// 카드의 상태 색. 어댑터가 색 띠 등으로 그린다(없으면 무시한다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardTone {
    /// 사람의 답을 기다린다.
    Pending,
    Success,
    /// 실패.
    Danger,
    /// 결과를 모른다 등 확인이 필요하다.
    Warning,
    /// 거절·만료처럼 아무 일도 하지 않고 끝났다.
    Neutral,
}

/// 카드의 이름-값 한 칸.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardField {
    /// 고정 문구만 쓴다(모델이 쓴 글을 넣지 않는다. 어댑터에 따라 이스케이프가 듣지 않을 수 있다).
    pub name: String,
    /// 메신저 마크다운 하위 집합(모델이 쓴 글은 만드는 쪽이 이스케이프해 넣는다).
    pub value: String,
    /// 옆 칸과 나란히 놓아도 되는 짧은 값.
    pub inline: bool,
}

/// 버튼이 달린 구조화된 메시지(승인 카드).
///
/// 어댑터는 구조대로 그리거나(Discord 임베드) 글로 바꿔 보낸다. 길면 나눠 보내고 버튼은 마지막 메시지에 단다.
/// 카드를 닫을 때는 같은 카드를 상태만 바꿔 다시 그린다(제목·칸·본문이 같으면 나뉘는 모양도 같아야 한다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub conversation: ConversationId,
    /// 맨 위 한 줄(알림 미리보기에 보인다). 닫을 때는 결과 문구.
    pub headline: String,
    /// 고정 문구.
    pub title: String,
    pub fields: Vec<CardField>,
    /// 긴 본문(마크다운 하위 집합, 모델이 쓴 글은 이스케이프된 상태).
    pub body: String,
    /// 고정 문구와 데몬이 만든 값(시각, ID)만.
    pub footer: Option<String>,
    pub tone: CardTone,
    pub buttons: Vec<CardButton>,
}

/// 버튼 누름. 누른 사람이 허용된 사람인지는 코어가 판단한다.
#[derive(Debug, Clone, PartialEq)]
pub struct ButtonPress {
    pub platform: String,
    pub sender: String,
    pub conversation: ConversationId,
    /// 버튼이 달린 메시지(어댑터의 메시지 참조).
    pub message_ref: String,
    pub action: String,
    /// 입력을 받는 버튼이면 사람이 넣은 값(결정 0017).
    pub input: Option<SecretText>,
    /// 여러 칸을 받는 버튼이면 칸마다의 값(결정 0063). 능력 `forms`가 있는 채널만 채운다.
    pub fields: Option<std::collections::BTreeMap<String, SecretText>>,
    pub received_at: DateTime<Utc>,
}

/// 어댑터가 선언하는 지원 기능. 코어는 여기에 맞춰 표현을 낮춘다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// 메시지 하나의 최대 길이(문자 수). 넘으면 어댑터가 나눠 보낸다.
    pub max_message_chars: usize,
    pub buttons: bool,
    pub edit: bool,
    pub threads: bool,
    /// 버튼을 눌러 짧은 값을 받을 수 있다(Discord modal, 결정 0017).
    pub prompts: bool,
    /// 버튼을 눌러 여러 칸(비밀번호 포함)을 받을 수 있다(결정 0063). 값이 데몬까지 곧장 오는 채널(자체 앱)만 켠다.
    pub forms: bool,
}

/// 행동의 권한 등급(명세 §작업 모델과 승인, 결정 0013). 순서가 있다: 위로 갈수록 무겁다.
///
/// 데몬 도구, 그리고 나중의 외부 기기 명령이 이 등급을 선언하고, 실행 종류가 연 등급까지만 쓴다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    /// 페이지 조회, 메일·캘린더 읽기, 검색: 자동.
    Read,
    /// 작업 공간 안 파일, 초안, 기억 쓰기: 자동.
    LocalWrite,
    /// 메일 발송, 폼 제출, 캘린더 수정, git push, 기기 조작: Discord 승인 필수.
    ExternalWrite,
    /// 결제, 계정 설정 변경: 도구 자체를 주지 않는다.
    Forbidden,
}

impl Grade {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::LocalWrite => "local_write",
            Self::ExternalWrite => "external_write",
            Self::Forbidden => "forbidden",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "read" => Self::Read,
            "local_write" => Self::LocalWrite,
            "external_write" => Self::ExternalWrite,
            "forbidden" => Self::Forbidden,
            _ => return None,
        })
    }
}

/// 일을 시작한 주체(결정 0011·0013). 쓸 수 있는 도구가 여기서 갈린다.
///
/// - `Human`: 사람이 지금 대화 중이다(채팅, 관리 페이지).
/// - `Delegated`: 사람이 맡긴 일을 나중에 사람 없이 한다(채팅에서 등록한 후속 조치, 그 사슬).
/// - `Autonomous`: 사람 없이 시작했다(루틴, 사전 체크, 브리핑, 나중의 외부 기기 이벤트). 외부 글이 섞였을 수
///   있으므로 가장 좁게 연다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Initiator {
    Human,
    Delegated,
    Autonomous,
}

impl Initiator {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Delegated => "delegated",
            Self::Autonomous => "autonomous",
        }
    }

    /// 모르는 값은 가장 좁은 `Autonomous`로 읽는다.
    pub fn parse(text: &str) -> Self {
        match text {
            "human" => Self::Human,
            "delegated" => Self::Delegated,
            _ => Self::Autonomous,
        }
    }
}

/// 작업 상태(명세 §작업 모델과 승인, 결정 0014).
///
/// 생성 → 실행 중 → (승인 대기 또는 입력 대기) → 결과 보고 → 완료. 실패·취소·만료는 따로 끝나는 상태다.
/// 완료 확인은 사람만 한다. 상태는 데몬만 바꾸고, 에이전트는 MCP로 요청만 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Created,
    Running,
    WaitingApproval,
    WaitingInput,
    Reported,
    Done,
    Failed,
    Cancelled,
    Expired,
}

impl TaskStatus {
    pub const ALL: [TaskStatus; 9] = [
        Self::Created,
        Self::Running,
        Self::WaitingApproval,
        Self::WaitingInput,
        Self::Reported,
        Self::Done,
        Self::Failed,
        Self::Cancelled,
        Self::Expired,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::WaitingInput => "waiting_input",
            Self::Reported => "reported",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == text)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Created => "대기",
            Self::Running => "하는 중",
            Self::WaitingApproval => "승인 대기",
            Self::WaitingInput => "답 기다림",
            Self::Reported => "확인 기다림",
            Self::Done => "완료",
            Self::Failed => "실패",
            Self::Cancelled => "취소",
            Self::Expired => "만료",
        }
    }

    /// 끝난 상태(다시 움직이지 않는다).
    pub fn is_final(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Cancelled | Self::Expired
        )
    }

    /// 데몬이 할 수 있는 상태 전이. 완료(`Done`)로는 결과를 보고한 뒤에만 간다(사람이 확인할 때).
    pub fn can_move_to(self, next: Self) -> bool {
        use TaskStatus::*;
        if self.is_final() {
            return false;
        }
        match next {
            Created => false,
            Running => matches!(self, Created | WaitingApproval | WaitingInput | Reported),
            WaitingApproval | WaitingInput => matches!(self, Running | Reported | WaitingApproval),
            Reported => matches!(self, Running | WaitingApproval),
            Done => self == Reported,
            Failed => matches!(self, Created | Running | WaitingApproval),
            Cancelled => true,
            Expired => matches!(self, WaitingApproval | WaitingInput | Reported),
        }
    }
}

/// 허용 사용자 목록. 목록에 없으면 거부한다(비워 두면 아무도 허용하지 않는다).
#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    entries: HashMap<(String, String), UserId>,
}

impl Allowlist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow(mut self, platform: &str, platform_user: &str, user: UserId) -> Self {
        self.entries
            .insert((platform.to_owned(), platform_user.to_owned()), user);
        self
    }

    /// 허용된 발신자면 내부 사용자 ID, 아니면 `None`.
    pub fn resolve(&self, platform: &str, platform_user: &str) -> Option<&UserId> {
        self.entries
            .get(&(platform.to_owned(), platform_user.to_owned()))
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_moves_only_forward_and_done_needs_a_report() {
        use TaskStatus::*;
        for s in TaskStatus::ALL {
            assert_eq!(TaskStatus::parse(s.as_str()), Some(s));
        }
        assert!(Created.can_move_to(Running));
        assert!(Running.can_move_to(Reported));
        assert!(Reported.can_move_to(Done));
        assert!(
            !Running.can_move_to(Done),
            "결과 보고 없이 완료로 가지 않는다"
        );
        assert!(
            Reported.can_move_to(WaitingInput),
            "[아직]이면 다시 답을 기다린다"
        );
        assert!(WaitingInput.can_move_to(Running));
        for end in [Done, Failed, Cancelled, Expired] {
            assert!(end.is_final());
            assert!(
                TaskStatus::ALL.iter().all(|n| !end.can_move_to(*n)),
                "{end:?}에서는 움직이지 않는다"
            );
        }
        assert!(!Running.can_move_to(Created));
    }

    #[test]
    fn secret_text_hides_its_value() {
        let s = SecretText::new("123456");
        assert_eq!(format!("{s:?}"), "SecretText(…6자)");
        assert_eq!(s.expose(), "123456");
    }

    /// 결정 0063: 여러 칸을 받은 누름의 `Debug`에도 값이 찍히지 않는다(칸 이름만 보인다).
    #[test]
    fn button_press_debug_hides_every_form_value() {
        let press = ButtonPress {
            platform: "app".into(),
            sender: "cl-1".into(),
            conversation: ConversationId("app:cl-1".into()),
            message_ref: "app:cl-1:1".into(),
            action: "approval:x:approve".into(),
            input: None,
            fields: Some(std::collections::BTreeMap::from([
                ("username".to_owned(), SecretText::new("아이디-시험-5678")),
                ("password".to_owned(), SecretText::new("pw-시험-1234")),
            ])),
            received_at: Utc::now(),
        };
        let shown = format!("{press:?}");
        assert!(!shown.contains("pw-시험-1234"), "{shown}");
        assert!(!shown.contains("아이디-시험-5678"), "{shown}");
        assert!(shown.contains("password"), "칸 이름은 보인다: {shown}");
    }

    #[test]
    fn form_field_kinds_have_stable_wire_names() {
        assert_eq!(FormFieldKind::Text.as_str(), "text");
        assert_eq!(FormFieldKind::Secret.as_str(), "secret");
        assert_eq!(FormFieldKind::Check.as_str(), "check");
    }

    #[test]
    fn allowlist_denies_by_default() {
        let empty = Allowlist::new();
        assert!(
            empty.resolve("discord", "123").is_none(),
            "비우면 아무도 허용하지 않는다"
        );

        let list = Allowlist::new().allow("discord", "123", UserId("me".into()));
        assert_eq!(list.resolve("discord", "123"), Some(&UserId("me".into())));
        assert!(list.resolve("discord", "456").is_none());
        assert!(
            list.resolve("memory", "123").is_none(),
            "플랫폼이 다르면 다른 사람"
        );
    }
}
