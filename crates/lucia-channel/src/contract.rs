//! 채널 어댑터 공통 계약(명세 §운영 테스트: "채널 어댑터는 공통 계약 테스트를 통과해야 한다").
//!
//! 어댑터마다 [`ContractHarness`]를 구현해 [`run_all`]을 부른다. Discord처럼 외부 서비스가 있는
//! 어댑터는 전송 계층을 가짜로 바꾼 하네스를 쓴다.

use std::future::Future;

use lucia_core::{ConversationId, Inbound, Outbound};

use crate::{ChannelAdapter, ChannelError};

pub trait ContractHarness {
    type Adapter: ChannelAdapter;

    fn adapter(&self) -> &Self::Adapter;

    /// 플랫폼 쪽에서 사용자가 메시지를 보낸 것처럼 만들고, 어댑터가 넘긴 `Inbound`를 돌려준다.
    fn deliver_inbound(
        &mut self,
        sender: &str,
        conversation: &str,
        body: &str,
    ) -> impl Future<Output = Inbound>;

    /// 어댑터가 플랫폼으로 실제로 보낸 조각(대화, 본문)을 꺼낸다.
    fn take_sent(&mut self) -> Vec<(ConversationId, String)>;
}

/// 모든 계약을 확인한다. 실패하면 panic한다(시험용). 대화 ID는 이름 그대로 쓴다.
pub async fn run_all<H: ContractHarness>(h: &mut H) {
    run_all_with(h, |name| ConversationId(name.to_owned())).await;
}

/// 대화 ID 형식이 정해진 어댑터(예: `discord:<채널 ID>`)용. `conversation`이 시험 이름을 그 어댑터의
/// 대화 ID로 바꾼다.
pub async fn run_all_with<H: ContractHarness>(
    h: &mut H,
    conversation: impl Fn(&str) -> ConversationId,
) {
    capabilities_are_sane(h);
    long_messages_are_split_without_loss(h, conversation("contract-long")).await;
    empty_messages_are_rejected(h, conversation("contract-empty")).await;
    inbound_keeps_platform_ids(h).await;
    notes_files_and_typing(h, conversation("contract-files")).await;
}

/// 작은 안내, 파일 메시지, 타이핑 표시를 받아들인다. 빈 파일 목록과 너무 많은 파일은 거부한다.
pub async fn notes_files_and_typing<H: ContractHarness>(h: &mut H, conversation: ConversationId) {
    let a = h.adapter();
    a.send(Outbound::Note {
        conversation: conversation.clone(),
        body: "웹 검색: 날씨".into(),
    })
    .await
    .expect("작은 안내");
    a.send(Outbound::Files {
        conversation: conversation.clone(),
        body: "파일을 붙였습니다".into(),
        files: vec![lucia_core::OutFile {
            name: "메모.md".into(),
            bytes: "# 메모".as_bytes().to_vec(),
            id: None,
        }],
    })
    .await
    .expect("파일 메시지");
    let empty = a
        .send(Outbound::Files {
            conversation: conversation.clone(),
            body: "x".into(),
            files: vec![],
        })
        .await;
    assert!(
        matches!(empty, Err(ChannelError::Empty)),
        "파일 없는 파일 메시지"
    );
    let many = a
        .send(Outbound::Files {
            conversation: conversation.clone(),
            body: String::new(),
            files: (0..crate::MAX_FILES_PER_MESSAGE + 1)
                .map(|i| lucia_core::OutFile {
                    name: format!("{i}.txt"),
                    bytes: vec![b'x'],
                    id: None,
                })
                .collect(),
        })
        .await;
    assert!(
        matches!(many, Err(ChannelError::File(_))),
        "파일이 너무 많다"
    );
    a.typing(&conversation).await.expect("타이핑 표시");
    h.take_sent();
}

pub fn capabilities_are_sane<H: ContractHarness>(h: &H) {
    let caps = h.adapter().capabilities();
    assert!(caps.max_message_chars > 0, "최대 길이는 0보다 커야 한다");
    assert!(
        !h.adapter().platform().is_empty(),
        "어댑터 이름이 있어야 한다"
    );
}

pub async fn long_messages_are_split_without_loss<H: ContractHarness>(
    h: &mut H,
    conversation: ConversationId,
) {
    let max = h.adapter().capabilities().max_message_chars;
    let line = "가나다라마바사 아자차카타파하\n";
    let body: String = line.repeat(max * 3 / line.chars().count() + 2);
    h.take_sent();
    let count = h
        .adapter()
        .send(Outbound::Text {
            conversation: conversation.clone(),
            body: body.clone(),
        })
        .await
        .expect("긴 메시지 전송");
    let sent = h.take_sent();
    assert_eq!(
        sent.len(),
        count,
        "돌려준 개수와 실제로 보낸 개수가 같아야 한다"
    );
    assert!(
        count >= 3,
        "최대 길이의 3배가 넘는 글은 3개 이상으로 나뉘어야 한다"
    );
    assert!(
        sent.iter().all(|(c, _)| *c == conversation),
        "같은 대화로 보내야 한다"
    );
    assert!(
        sent.iter().all(|(_, p)| p.chars().count() <= max),
        "조각마다 최대 길이 이하"
    );
    let joined: String = sent.into_iter().map(|(_, p)| p).collect();
    assert_eq!(joined, body, "이어 붙이면 원문과 같아야 한다(순서 유지)");
}

pub async fn empty_messages_are_rejected<H: ContractHarness>(
    h: &mut H,
    conversation: ConversationId,
) {
    h.take_sent();
    let result = h
        .adapter()
        .send(Outbound::Text {
            conversation,
            body: "  \n ".into(),
        })
        .await;
    assert!(
        matches!(result, Err(ChannelError::Empty)),
        "빈 메시지는 Empty 오류"
    );
    assert!(h.take_sent().is_empty(), "빈 메시지는 보내지 않는다");
}

async fn inbound_keeps_platform_ids<H: ContractHarness>(h: &mut H) {
    let platform = h.adapter().platform().to_owned();
    let inbound = h.deliver_inbound("42", "7", "안녕 루시아").await;
    assert_eq!(inbound.platform, platform);
    assert_eq!(
        inbound.sender, "42",
        "플랫폼 사용자 ID를 그대로 넘긴다(허용 판단은 코어)"
    );
    assert!(
        inbound.conversation.0.contains('7'),
        "대화 ID에 플랫폼 대화가 드러나야 한다"
    );
    assert_eq!(inbound.body, "안녕 루시아");
}
