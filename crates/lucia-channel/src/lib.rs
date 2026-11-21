//! 채널 어댑터.
//!
//! - 어댑터 트레이트와 능력 선언(버튼, 메시지 수정, 스레드, 길이 제한)
//! - 모든 어댑터가 통과해야 하는 공통 계약([`contract`])
//! - 메모리 안 어댑터([`memory`]): 시험용, 그리고 Discord 없이 데몬 전체를 돌릴 때
//! - Discord 어댑터(twilight, D3): 다음 단계
//!
//! 어댑터는 플랫폼 ID를 그대로 넘긴다. 허용 여부는 코어(`lucia_core::Allowlist`)가 판단하고, 기본은 거부다.
//! 길이 분할 같은 플랫폼 세부는 어댑터 안에서 처리한다.

pub mod contract;
pub mod discord;
pub mod memory;

use std::future::Future;

use lucia_core::{Capabilities, Card, ConversationId, Outbound};

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("빈 메시지는 보내지 않는다")]
    Empty,
    #[error("전송 실패: {0}")]
    Send(String),
    #[error("어댑터가 닫혔다")]
    Closed,
    /// 플랫폼이 자격 증명을 거부했다(토큰이 틀렸거나 취소됨). 다시 시도해도 소용없다.
    #[error("자격 증명이 거부됐다: {0}")]
    Unauthorized(String),
    /// 그 채널·동작의 권한이 없다(서버 권한·역할 순서). 토큰은 멀쩡하다.
    #[error("권한이 없다: {0}")]
    Forbidden(String),
    #[error("이 채널은 {0}을(를) 지원하지 않는다")]
    Unsupported(&'static str),
    #[error("보낼 수 없는 파일: {0}")]
    File(String),
}

/// 한 메시지에 붙일 수 있는 파일 수와 크기(Discord 문서: 파일당 기본 20 MiB, 요청 25 MiB. 개수는 문서에 없어 10개로).
pub const MAX_FILES_PER_MESSAGE: usize = 10;
pub const MAX_FILE_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_FILES_TOTAL_BYTES: usize = 24 * 1024 * 1024;

/// 보낼 파일 목록 검사.
pub fn check_files(files: &[lucia_core::OutFile]) -> Result<(), ChannelError> {
    if files.len() > MAX_FILES_PER_MESSAGE {
        return Err(ChannelError::File(format!(
            "한 메시지에 {MAX_FILES_PER_MESSAGE}개까지"
        )));
    }
    if let Some(f) = files.iter().find(|f| f.bytes.len() > MAX_FILE_BYTES) {
        return Err(ChannelError::File(format!("{}: 20 MiB를 넘는다", f.name)));
    }
    if files.iter().map(|f| f.bytes.len()).sum::<usize>() > MAX_FILES_TOTAL_BYTES {
        return Err(ChannelError::File("합계가 24 MiB를 넘는다".into()));
    }
    Ok(())
}

/// 구조를 그릴 수 없는 채널용: 카드를 마크다운 글 하나로 바꾼다.
pub fn card_text(card: &Card) -> String {
    let mut out = String::new();
    if !card.headline.trim().is_empty() {
        out.push_str(card.headline.trim_end());
        out.push('\n');
    }
    out.push_str(&format!("**{}**\n", card.title));
    for f in &card.fields {
        out.push_str(&format!("**{}**: {}\n", f.name, f.value));
    }
    if !card.body.is_empty() {
        out.push_str("───\n");
        out.push_str(card.body.trim_end());
        out.push('\n');
    }
    if let Some(footer) = &card.footer {
        out.push_str(&format!("-# {footer}\n"));
    }
    out.trim_end().to_owned()
}

/// 권한 모드 슬래시 명령의 이름(결정 0040).
pub const COMMAND_SECURITY: &str = "권한";

/// 주 모델 슬래시 명령의 이름(결정 0055).
pub const COMMAND_MODEL: &str = "모델";

/// `/모델`이 받는 값(보이는 이름, 값). 받는 쪽(데몬)이 아는 모델과 같아야 한다(데몬의 테스트가 맞춰 본다).
pub const MODEL_CHOICES: [(&str, &str); 5] = [
    ("Fable", "fable"),
    ("Opus", "opus"),
    ("Sonnet", "sonnet"),
    ("Haiku", "haiku"),
    ("기본(설정 파일 값)", "default"),
];

/// 메신저의 슬래시 명령(결정 0040). 버튼 누름과 따로 어댑터의 명령 분기만 만든다(다른 통로가 흉내 내지 못하게).
/// 받는 쪽이 소유자인지 확인하고 `reply`로 답한다(늦으면 어댑터가 시간 초과로 답한다).
#[derive(Debug)]
pub struct SlashCommand {
    pub platform: String,
    pub sender: String,
    /// DM에서 불렀다.
    pub in_dm: bool,
    pub name: String,
    /// 첫 옵션의 글 값.
    pub option: Option<String>,
    pub reply: tokio::sync::oneshot::Sender<CommandReply>,
}

/// 명령의 답.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandReply {
    pub text: String,
    /// 부른 사람에게만 보인다.
    pub private: bool,
}

impl CommandReply {
    pub fn private(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            private: true,
        }
    }
}

/// 채널 어댑터. 들어오는 메시지는 만들 때 받은 `mpsc::Sender<Inbound>`로 보낸다.
pub trait ChannelAdapter: Send + Sync + 'static {
    /// 어댑터 이름. `Inbound::platform`과 같아야 한다.
    fn platform(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    /// 보낸다. 길이 제한을 넘으면 나눠 보내고, 나눈 개수를 돌려준다.
    fn send(&self, message: Outbound) -> impl Future<Output = Result<usize, ChannelError>> + Send;

    /// 지금 보낼 수 있는가(연결됨). 엔진은 모델을 부르기 전에 이것을 확인한다.
    fn ready(&self) -> bool {
        true
    }

    /// "입력 중" 표시를 잠깐 켠다(Discord는 10초). 기본 구현을 두지 않는다: 다른 어댑터를 감싸는 어댑터가
    /// 넘겨주기를 잊으면 컴파일이 실패하게(감싼 어댑터에서 조용히 아무 일도 안 하던 일이 있었다).
    fn typing(
        &self,
        conversation: &ConversationId,
    ) -> impl Future<Output = Result<(), ChannelError>> + Send;

    /// 버튼 카드를 보낸다. 길면 나눠 보내고 버튼은 마지막 메시지에 단다.
    /// 버튼이 달린 메시지의 참조를 돌려준다. 버튼이 없는 채널은 `Unsupported`.
    fn send_card(&self, card: Card) -> impl Future<Output = Result<String, ChannelError>> + Send {
        let _ = card;
        async { Err(ChannelError::Unsupported("버튼")) }
    }

    /// 작업 스레드를 연다(결정 0014). `parent`는 스레드를 열 수 있는 대화(예: 서버의 작업 채널)다.
    /// 새 스레드의 대화 ID를 돌려준다. 스레드가 없는 채널은 `Unsupported`(코어는 DM으로 낮춘다).
    fn open_thread(
        &self,
        parent: &ConversationId,
        name: &str,
    ) -> impl Future<Output = Result<ConversationId, ChannelError>> + Send {
        let _ = (parent, name);
        async { Err(ChannelError::Unsupported("스레드")) }
    }

    /// 카드를 닫는다: 버튼이 달린 메시지(`message_ref`)를 닫힌 카드(`card`, 버튼 없음)로 다시 그린다.
    /// 제목·칸·본문은 보낼 때와 같고 머리줄·꼬리말·색만 다르다.
    fn close_card(
        &self,
        message_ref: &str,
        card: &Card,
    ) -> impl Future<Output = Result<(), ChannelError>> + Send {
        let _ = (message_ref, card);
        async { Err(ChannelError::Unsupported("메시지 수정")) }
    }

    /// 스티커 하나만 있는 메시지를 보낸다(결정 0044). `sticker_id`는 그 플랫폼의 스티커 ID다.
    /// 스티커가 없는 채널은 지원하지 않는다고 답한다.
    fn send_sticker(
        &self,
        conversation: &ConversationId,
        sticker_id: &str,
    ) -> impl Future<Output = Result<(), ChannelError>> + Send {
        let _ = (conversation, sticker_id);
        async { Err(ChannelError::Unsupported("스티커")) }
    }
}

/// 글 속 주소가 링크 미리보기를 만들지 않게 `<주소>`로 감싼다(결정 0028).
///
/// 미리보기를 만들려고 메신저 서버가 그 주소를 가져가므로, 주입된 글이 답에 `https://공격자/?d=<비공개 자료>`를
/// 붙이게 하면 사용자가 누르지 않아도 자료가 나간다. 감싼 주소도 누르면 그대로 열린다.
/// - 코드 블록(```)과 인라인 코드(`) 안은 건드리지 않는다.
/// - `[글](주소)`는 `[글](<주소>)`로, 이미 `<주소>`인 것은 그대로 둔다.
/// - 주소는 공백이나 ASCII 밖 문자(한국어 조사 등)에서 끝나고, 끝의 문장부호는 주소에 넣지 않는다.
pub fn quiet_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    let mut in_fence = false;
    let mut in_code = false;
    while !rest.is_empty() {
        if rest.starts_with("```") {
            in_fence = !in_fence;
            out.push_str("```");
            rest = &rest[3..];
            continue;
        }
        let c = rest.chars().next().expect("비어 있지 않다");
        if !in_fence && c == '`' {
            in_code = !in_code;
            out.push(c);
            rest = &rest[1..];
            continue;
        }
        if !in_fence && !in_code && (rest.starts_with("https://") || rest.starts_with("http://")) {
            let end = rest
                .char_indices()
                .find(|&(_, ch)| ch.is_whitespace() || !ch.is_ascii() || ch == '<' || ch == '>')
                .map(|(i, _)| i)
                .unwrap_or(rest.len());
            let mut url = &rest[..end];
            // 끝의 문장부호(마침표, 쉼표, 닫는 괄호 등)는 주소가 아니다. 괄호는 짝이 맞으면 남긴다.
            loop {
                let Some(last) = url.chars().last() else {
                    break;
                };
                let trim = match last {
                    '.' | ',' | '!' | '?' | ';' | ':' | '\'' | '"' | '*' | '_' | '~' | ']'
                    | '}' => true,
                    ')' => url.matches('(').count() < url.matches(')').count(),
                    _ => false,
                };
                if !trim {
                    break;
                }
                url = &url[..url.len() - last.len_utf8()];
            }
            let already = out.ends_with('<');
            if already || url.len() <= "https://".len() {
                out.push_str(url);
            } else {
                out.push('<');
                out.push_str(url);
                out.push('>');
            }
            rest = &rest[url.len()..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// `max_chars`(문자 수) 이하로 나눈다. 가능하면 줄바꿈에서, 안 되면 공백에서, 그래도 안 되면
/// 문자 경계에서 자른다. 조각을 이어 붙이면 원문과 같다.
///
/// 이스케이프(`\` + 문자)는 둘로 가르지 않는다(`max_chars`가 2 이상일 때). 갈리면 다음 조각 맨 앞의
/// 문자가 마크다운으로 살아나, 이스케이프해 둔 글(승인 카드)이 다르게 보인다.
pub fn split_message(text: &str, max_chars: usize) -> Vec<String> {
    assert!(max_chars > 0, "max_chars는 0보다 커야 한다");
    let mut parts = Vec::new();
    let mut rest = text;
    while rest.chars().count() > max_chars {
        // max_chars번째 문자까지의 바이트 위치.
        let limit = rest
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        let window = &rest[..limit];
        let mut cut = window
            .rfind('\n')
            .map(|i| i + 1)
            .or_else(|| window.rfind(' ').map(|i| i + 1))
            .filter(|&i| i > 0)
            .unwrap_or(limit);
        // 끝의 `\`가 홀수 개면 마지막 `\`는 다음 문자를 이스케이프하는 것이다. 다음 조각으로 넘긴다.
        let trailing = rest[..cut]
            .bytes()
            .rev()
            .take_while(|&b| b == b'\\')
            .count();
        if trailing % 2 == 1 && cut > 1 {
            cut -= 1;
        }
        parts.push(rest[..cut].to_owned());
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        parts.push(rest.to_owned());
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_quieted_outside_code() {
        assert_eq!(
            quiet_links("보세요 https://a.com/x?q=1. 끝"),
            "보세요 <https://a.com/x?q=1>. 끝"
        );
        assert_eq!(
            quiet_links("https://a.com에서 봤어요"),
            "<https://a.com>에서 봤어요"
        );
        assert_eq!(
            quiet_links("[공지](https://a.com/n)"),
            "[공지](<https://a.com/n>)"
        );
        assert_eq!(
            quiet_links("이미 <https://a.com> 감쌈"),
            "이미 <https://a.com> 감쌈"
        );
        assert_eq!(
            quiet_links("(https://en.wikipedia.org/wiki/Rust_(language))"),
            "(<https://en.wikipedia.org/wiki/Rust_(language)>)"
        );
        assert_eq!(quiet_links("`https://a.com` 코드"), "`https://a.com` 코드");
        assert_eq!(
            quiet_links("```\ncurl https://a.com\n```\nhttps://b.com"),
            "```\ncurl https://a.com\n```\n<https://b.com>"
        );
        assert_eq!(quiet_links("http:// 만"), "http:// 만");
        assert_eq!(quiet_links("주소 없음"), "주소 없음");
    }

    #[test]
    fn split_keeps_content_and_limit() {
        let text = "첫 줄입니다\n둘째 줄은 조금 더 깁니다\n셋째";
        for max in [1, 3, 5, 8, 100] {
            let parts = split_message(text, max);
            assert_eq!(parts.concat(), text, "max={max}");
            assert!(
                parts.iter().all(|p| p.chars().count() <= max),
                "max={max}: {parts:?}"
            );
            assert!(parts.iter().all(|p| !p.is_empty()));
        }
    }

    #[test]
    fn split_prefers_newlines() {
        let parts = split_message("aaaa\nbbbb\ncccc", 10);
        assert_eq!(parts, vec!["aaaa\nbbbb\n", "cccc"]);
    }

    /// 이스케이프 쌍은 조각 경계에서 갈리지 않는다(공백 없는 글이라 문자 경계에서 잘릴 때도).
    #[test]
    fn split_never_separates_an_escape() {
        let text = r"\*\*\|\|\@\.\_".repeat(30);
        for max in [2, 3, 7, 10, 33] {
            let parts = split_message(&text, max);
            assert_eq!(parts.concat(), text, "max={max}");
            for p in &parts {
                assert!(p.chars().count() <= max, "max={max}: {p}");
                let trailing = p.bytes().rev().take_while(|&b| b == b'\\').count();
                assert_eq!(trailing % 2, 0, "max={max}: 조각이 `\\`로 끝난다: {p}");
            }
        }
        // `\\`(이스케이프된 백슬래시) 뒤에서 자르는 것은 괜찮다.
        assert_eq!(split_message(r"ab\\cd", 4), [r"ab\\", "cd"]);
    }

    #[test]
    fn card_text_has_every_part() {
        let card = Card {
            conversation: ConversationId("c".into()),
            headline: "머리줄".into(),
            title: "제목칸".into(),
            fields: vec![lucia_core::CardField {
                name: "받는 사람".into(),
                value: r"a\@b\.c".into(),
                inline: true,
            }],
            body: "본문".into(),
            footer: Some("꼬리".into()),
            tone: lucia_core::CardTone::Pending,
            buttons: vec![],
        };
        assert_eq!(
            card_text(&card),
            "머리줄\n**제목칸**\n**받는 사람**: a\\@b\\.c\n───\n본문\n-# 꼬리"
        );
    }

    #[test]
    fn split_empty_is_empty() {
        assert!(split_message("", 10).is_empty());
    }
}
