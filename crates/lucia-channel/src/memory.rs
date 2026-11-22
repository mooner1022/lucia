//! 메모리 안 어댑터. 사람 대신 시험 코드가 메시지를 넣고, 보낸 메시지를 읽는다.
//! Discord 없이 데몬 전체를 돌려 볼 때도 쓴다.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use lucia_core::{
    ButtonPress, Capabilities, Card, ConversationId, Inbound, InboundAttachment, Outbound,
};
use tokio::sync::mpsc;

use crate::{ChannelAdapter, ChannelError, card_text, check_files, split_message};

pub const PLATFORM: &str = "memory";

/// 보낸 카드(시험에서 확인한다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentCard {
    pub message_ref: String,
    pub conversation: ConversationId,
    /// 처음 보낸 카드.
    pub card: Card,
    /// 아직 누를 수 있는 버튼.
    pub actions: Vec<String>,
    /// 닫혔으면 닫힌 카드.
    pub closed: Option<Card>,
}

impl SentCard {
    /// 지금 보이는 모습(닫혔으면 닫힌 카드)을 글로.
    pub fn text(&self) -> String {
        card_text(self.closed.as_ref().unwrap_or(&self.card))
    }
}

/// 보낸 파일 메시지(시험에서 확인한다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentFiles {
    pub conversation: ConversationId,
    pub body: String,
    pub names: Vec<String>,
    pub sizes: Vec<usize>,
}

/// 메모리 채널이 기록하는 것들.
#[derive(Default)]
struct Log {
    notes: Vec<(ConversationId, String)>,
    files: Vec<SentFiles>,
    typing: usize,
    /// 보낸 순서(종류: text·note·files).
    order: Vec<&'static str>,
    /// 연 스레드(부모, 이름, 새 대화).
    threads: Vec<(ConversationId, String, ConversationId)>,
    /// 보낸 스티커(대화, 스티커 ID).
    stickers: Vec<(ConversationId, String)>,
}

#[derive(Clone)]
pub struct MemoryChannel {
    caps: Capabilities,
    sent: Arc<Mutex<Vec<(ConversationId, String)>>>,
    cards: Arc<Mutex<Vec<SentCard>>>,
    log: Arc<Mutex<Log>>,
}

/// 시험 코드 쪽 손잡이.
#[derive(Clone)]
pub struct MemoryHandle {
    inbound: mpsc::Sender<Inbound>,
    presses: Option<mpsc::Sender<ButtonPress>>,
    sent: Arc<Mutex<Vec<(ConversationId, String)>>>,
    cards: Arc<Mutex<Vec<SentCard>>>,
    log: Arc<Mutex<Log>>,
}

impl MemoryChannel {
    /// 어댑터와 손잡이를 만든다. `inbound`는 데몬이 들어오는 메시지를 받는 곳이다.
    pub fn new(max_message_chars: usize, inbound: mpsc::Sender<Inbound>) -> (Self, MemoryHandle) {
        Self::build(max_message_chars, inbound, None)
    }

    /// 버튼 카드까지 쓰는 채널. 누름은 `presses`로 간다.
    pub fn with_buttons(
        max_message_chars: usize,
        inbound: mpsc::Sender<Inbound>,
        presses: mpsc::Sender<ButtonPress>,
    ) -> (Self, MemoryHandle) {
        Self::build(max_message_chars, inbound, Some(presses))
    }

    fn build(
        max_message_chars: usize,
        inbound: mpsc::Sender<Inbound>,
        presses: Option<mpsc::Sender<ButtonPress>>,
    ) -> (Self, MemoryHandle) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let cards = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::new(Mutex::new(Log::default()));
        let caps = Capabilities {
            max_message_chars,
            buttons: presses.is_some(),
            edit: presses.is_some(),
            threads: false,
            prompts: false,
            forms: false,
        };
        (
            Self {
                caps,
                sent: sent.clone(),
                cards: cards.clone(),
                log: log.clone(),
            },
            MemoryHandle {
                inbound,
                presses,
                sent,
                cards,
                log,
            },
        )
    }
}

impl MemoryChannel {
    /// 스레드를 지원하는 채널로(작업 스레드 시험용).
    pub fn with_threads(mut self) -> Self {
        self.caps.threads = true;
        self
    }
}

impl MemoryHandle {
    /// 연 스레드(부모, 이름, 새 대화).
    pub fn threads(&self) -> Vec<(ConversationId, String, ConversationId)> {
        self.log.lock().expect("log lock").threads.clone()
    }
}

impl ChannelAdapter for MemoryChannel {
    fn platform(&self) -> &str {
        PLATFORM
    }

    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    async fn send(&self, message: Outbound) -> Result<usize, ChannelError> {
        match message {
            Outbound::Text { conversation, body } => {
                if body.trim().is_empty() {
                    return Err(ChannelError::Empty);
                }
                let parts = split_message(&body, self.caps.max_message_chars);
                let count = parts.len();
                let mut sent = self.sent.lock().expect("sent lock");
                sent.extend(parts.into_iter().map(|p| (conversation.clone(), p)));
                self.log.lock().expect("log lock").order.push("text");
                Ok(count)
            }
            Outbound::Note { conversation, body } => {
                if body.trim().is_empty() {
                    return Err(ChannelError::Empty);
                }
                let mut log = self.log.lock().expect("log lock");
                log.notes.push((conversation, body));
                log.order.push("note");
                Ok(1)
            }
            Outbound::Files {
                conversation,
                body,
                files,
            } => {
                if files.is_empty() {
                    return Err(ChannelError::Empty);
                }
                check_files(&files)?;
                let mut log = self.log.lock().expect("log lock");
                log.files.push(SentFiles {
                    conversation,
                    body,
                    names: files.iter().map(|f| f.name.clone()).collect(),
                    sizes: files.iter().map(|f| f.bytes.len()).collect(),
                });
                log.order.push("files");
                Ok(1)
            }
        }
    }

    async fn typing(&self, _conversation: &ConversationId) -> Result<(), ChannelError> {
        self.log.lock().expect("log lock").typing += 1;
        Ok(())
    }

    async fn open_thread(
        &self,
        parent: &ConversationId,
        name: &str,
    ) -> Result<ConversationId, ChannelError> {
        if !self.caps.threads {
            return Err(ChannelError::Unsupported("스레드"));
        }
        let mut log = self.log.lock().expect("log lock");
        let thread = ConversationId(format!("{}/thread-{}", parent.0, log.threads.len() + 1));
        log.threads
            .push((parent.clone(), name.to_owned(), thread.clone()));
        Ok(thread)
    }

    async fn send_card(&self, card: Card) -> Result<String, ChannelError> {
        if !self.caps.buttons {
            return Err(ChannelError::Unsupported("버튼"));
        }
        if card.title.trim().is_empty() && card.body.trim().is_empty() {
            return Err(ChannelError::Empty);
        }
        let mut cards = self.cards.lock().expect("cards lock");
        let message_ref = format!("card-{}", cards.len() + 1);
        cards.push(SentCard {
            message_ref: message_ref.clone(),
            conversation: card.conversation.clone(),
            actions: card.buttons.iter().map(|b| b.action.clone()).collect(),
            card,
            closed: None,
        });
        Ok(message_ref)
    }

    async fn close_card(&self, message_ref: &str, card: &Card) -> Result<(), ChannelError> {
        let mut cards = self.cards.lock().expect("cards lock");
        let sent = cards
            .iter_mut()
            .find(|c| c.message_ref == message_ref && c.conversation == card.conversation)
            .ok_or_else(|| ChannelError::Send(format!("모르는 카드 {message_ref}")))?;
        sent.closed = Some(card.clone());
        sent.actions.clear();
        Ok(())
    }

    async fn send_sticker(
        &self,
        conversation: &ConversationId,
        sticker_id: &str,
    ) -> Result<(), ChannelError> {
        let mut log = self.log.lock().expect("log lock");
        log.stickers
            .push((conversation.clone(), sticker_id.to_owned()));
        log.order.push("sticker");
        Ok(())
    }
}

impl MemoryHandle {
    /// 사용자가 메시지를 보낸 것처럼 넣는다.
    pub async fn user_says(
        &self,
        sender: &str,
        conversation: &str,
        body: &str,
    ) -> Result<(), ChannelError> {
        self.inbound
            .send(Inbound {
                platform: PLATFORM.to_owned(),
                sender: sender.to_owned(),
                conversation: ConversationId(conversation.to_owned()),
                body: body.to_owned(),
                attachments: Vec::new(),
                received_at: Utc::now(),
                reply_to: None,
                venue: None,
                context: None,
            })
            .await
            .map_err(|_| ChannelError::Closed)
    }

    /// 사용자가 파일을 붙여 보낸 것처럼 넣는다.
    pub async fn user_sends_files(
        &self,
        sender: &str,
        conversation: &str,
        body: &str,
        attachments: Vec<InboundAttachment>,
    ) -> Result<(), ChannelError> {
        self.inbound
            .send(Inbound {
                platform: PLATFORM.to_owned(),
                sender: sender.to_owned(),
                conversation: ConversationId(conversation.to_owned()),
                body: body.to_owned(),
                attachments,
                received_at: Utc::now(),
                reply_to: None,
                venue: None,
                context: None,
            })
            .await
            .map_err(|_| ChannelError::Closed)
    }

    /// 보낸 작은 안내(도구 사용 등).
    pub fn take_notes(&self) -> Vec<(ConversationId, String)> {
        std::mem::take(&mut self.log.lock().expect("log lock").notes)
    }

    /// 보낸 스티커(대화, 스티커 ID).
    pub fn take_stickers(&self) -> Vec<(ConversationId, String)> {
        std::mem::take(&mut self.log.lock().expect("log lock").stickers)
    }

    /// 보낸 파일 메시지.
    pub fn take_files(&self) -> Vec<SentFiles> {
        std::mem::take(&mut self.log.lock().expect("log lock").files)
    }

    /// 타이핑 표시를 켠 횟수.
    pub fn typing_count(&self) -> usize {
        self.log.lock().expect("log lock").typing
    }

    /// 보낸 순서(text·note·files).
    pub fn take_order(&self) -> Vec<&'static str> {
        std::mem::take(&mut self.log.lock().expect("log lock").order)
    }

    /// 지금까지 보낸 메시지 조각(대화, 본문)을 꺼낸다.
    pub fn take_sent(&self) -> Vec<(ConversationId, String)> {
        std::mem::take(&mut *self.sent.lock().expect("sent lock"))
    }

    /// 보낸 카드(닫힌 것 포함).
    pub fn cards(&self) -> Vec<SentCard> {
        self.cards.lock().expect("cards lock").clone()
    }

    /// 사용자가 카드의 버튼을 누른 것처럼 넣는다.
    pub async fn press(
        &self,
        sender: &str,
        conversation: &str,
        message_ref: &str,
        action: &str,
    ) -> Result<(), ChannelError> {
        self.press_with(sender, conversation, message_ref, action, None)
            .await
    }

    /// 값을 받는 버튼(입력창)에 값을 넣어 누른다(결정 0017).
    pub async fn press_with(
        &self,
        sender: &str,
        conversation: &str,
        message_ref: &str,
        action: &str,
        input: Option<&str>,
    ) -> Result<(), ChannelError> {
        let presses = self
            .presses
            .as_ref()
            .ok_or(ChannelError::Unsupported("버튼"))?;
        presses
            .send(ButtonPress {
                platform: PLATFORM.to_owned(),
                sender: sender.to_owned(),
                conversation: ConversationId(conversation.to_owned()),
                message_ref: message_ref.to_owned(),
                action: action.to_owned(),
                input: input.map(lucia_core::SecretText::new),
                fields: None,
                received_at: Utc::now(),
            })
            .await
            .map_err(|_| ChannelError::Closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{self, ContractHarness};

    struct Harness {
        adapter: MemoryChannel,
        handle: MemoryHandle,
        inbound: mpsc::Receiver<Inbound>,
    }

    impl ContractHarness for Harness {
        type Adapter = MemoryChannel;

        fn adapter(&self) -> &MemoryChannel {
            &self.adapter
        }

        async fn deliver_inbound(
            &mut self,
            sender: &str,
            conversation: &str,
            body: &str,
        ) -> Inbound {
            self.handle
                .user_says(sender, conversation, body)
                .await
                .unwrap();
            self.inbound.recv().await.expect("inbound")
        }

        fn take_sent(&mut self) -> Vec<(ConversationId, String)> {
            self.handle.take_sent()
        }
    }

    #[tokio::test]
    async fn memory_channel_passes_contract() {
        let (tx, rx) = mpsc::channel(8);
        let (adapter, handle) = MemoryChannel::new(50, tx);
        let mut harness = Harness {
            adapter,
            handle,
            inbound: rx,
        };
        contract::run_all(&mut harness).await;
    }
}
