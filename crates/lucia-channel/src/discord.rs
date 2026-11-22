//! Discord 어댑터(twilight 0.18, D3).
//!
//! - 들어오는 것: 게이트웨이 `DIRECT_MESSAGES` intent의 DM. DM 내용은 특권 intent `MESSAGE_CONTENT` 없이도
//!   온다(공식 문서의 예외, `docs/PLAN.md` §4). 봇 메시지는 버리고, 같은 메시지 ID는 한 번만 넘긴다.
//!   허용 여부는 코어가 판단한다(여기서는 플랫폼 ID만 넘긴다).
//! - 서버 메시지(결정 0014·0024): 켰을 때만 받는다. 소유자가 봇을 부른 말(멘션, 정한 머리말, 봇 메시지에
//!   답장)과 듣는 채널(작업 스레드)의 말만 넘기고 나머지는 여기서 버린다(바쁜 서버가 큐를 막지 않게).
//!   답장한 원문과 서버·채널 이름을 함께 넘긴다.
//! - 나가는 것: 대화 ID `discord:<채널 ID>`로 보낸다. 2,000자를 넘으면 나눈다.
//! - 승인 카드(결정 0003, 모양은 결정 0005): 임베드로 그린다([`card`]). 버튼은 마지막 메시지에 단다. 버튼 누름(`INTERACTION_CREATE`, intent가 필요 없다)은
//!   받자마자 "처리 중"으로 응답하고(Discord 3초 제한, `DEFERRED_UPDATE_MESSAGE`) 코어로 넘긴다. 누른 사람이
//!   허용된 사람인지는 코어가 판단한다. 서버(길드)에서 온 누름은 서버 메시지를 받을 때만 넘긴다.
//! - 실패: 토큰이 틀리거나 intent가 거부되면 샤드가 `FatallyClosed`가 된다. 그때는 다시 붙지 않고 멈춘다
//!   (Discord는 하루 IDENTIFY 1,000회를 넘기면 토큰을 초기화한다).
//!
//! HTTP 호출은 [`DiscordApi`] 뒤에 둔다. 공통 계약 테스트는 가짜 구현으로 돈다.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use chrono::Utc;
use lucia_core::{
    ButtonPress, ButtonPrompt, Capabilities, Card, CardButton, ConversationId, Inbound,
    InboundAttachment, OutFile, Outbound, Quoted, Venue,
};
use tokio::sync::mpsc;
use twilight_gateway::{
    CloseFrame, Event, EventTypeFlags, Intents, Shard, ShardId, ShardState, StreamExt as _,
};
use twilight_model::channel::message::embed::Embed;
use twilight_model::id::Id;

use crate::{ChannelAdapter, ChannelError, check_files, split_message};

pub mod card;
pub mod server;

pub const PLATFORM: &str = "discord";
pub const MAX_MESSAGE_CHARS: usize = 2000;
const CONVERSATION_PREFIX: &str = "discord:";

/// 봇 계정 정보(토큰 검증 결과).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotUser {
    pub id: u64,
    pub name: String,
}

/// Discord REST 호출. 실제 구현은 [`TwilightApi`].
pub trait DiscordApi: Send + Sync + 'static {
    fn create_message(
        &self,
        channel_id: u64,
        content: &str,
    ) -> impl Future<Output = Result<(), ChannelError>> + Send;
    /// 사용자와의 DM 채널을 연다(이미 있으면 그 채널).
    fn open_dm(&self, user_id: u64) -> impl Future<Output = Result<u64, ChannelError>> + Send;
    /// 토큰이 유효한지 확인한다(가장 가벼운 호출).
    fn current_user(&self) -> impl Future<Output = Result<BotUser, ChannelError>> + Send;
    /// 봇의 아바타를 바꾼다(결정 0060). `data_uri`는 `data:image/png;base64,…` 모양이다(공식 문서의 Image Data:
    /// JPG·GIF·PNG). `None`이면 아바타를 없앤다.
    fn set_avatar(
        &self,
        _data_uri: Option<&str>,
    ) -> impl Future<Output = Result<(), ChannelError>> + Send {
        async {
            Err(ChannelError::Send(
                "이 연결은 아바타를 바꾸지 못한다".into(),
            ))
        }
    }
    /// 봇의 지금 아바타 주소(Discord CDN, PNG). 아바타가 없으면 `None`.
    fn avatar_url(&self) -> impl Future<Output = Result<Option<String>, ChannelError>> + Send {
        async { Ok(None) }
    }
    /// 임베드(와 내용)만 있는 메시지를 보낸다(긴 카드의 앞부분).
    fn create_embeds(
        &self,
        channel_id: u64,
        content: &str,
        embeds: &[Embed],
    ) -> impl Future<Output = Result<(), ChannelError>> + Send;
    /// 임베드와 버튼이 달린 메시지를 보내고 메시지 ID를 돌려준다.
    fn create_card(
        &self,
        channel_id: u64,
        content: &str,
        embeds: &[Embed],
        buttons: &[CardButton],
    ) -> impl Future<Output = Result<u64, ChannelError>> + Send;
    /// 메시지의 내용과 임베드를 바꾸고 버튼을 없앤다.
    fn close_card(
        &self,
        channel_id: u64,
        message_id: u64,
        content: &str,
        embeds: &[Embed],
    ) -> impl Future<Output = Result<(), ChannelError>> + Send;
    /// 스티커 하나만 있는 메시지를 보낸다(결정 0044). 공식 문서: `sticker_ids`는 "그 서버의" 스티커 3개까지.
    fn create_sticker_message(
        &self,
        channel_id: u64,
        sticker_id: u64,
    ) -> impl Future<Output = Result<(), ChannelError>> + Send;
    /// 파일을 붙인 메시지를 보낸다(본문은 비어도 된다).
    fn create_message_with_files(
        &self,
        channel_id: u64,
        content: &str,
        files: &[OutFile],
    ) -> impl Future<Output = Result<(), ChannelError>> + Send;
    /// 버튼 누름에 입력창(modal)으로 응답한다(결정 0017). `custom_id`는 제출 때 돌아온다.
    fn respond_modal(
        &self,
        application_id: u64,
        interaction_id: u64,
        token: &str,
        custom_id: &str,
        prompt: &ButtonPrompt,
    ) -> impl Future<Output = Result<(), ChannelError>> + Send;
    /// 채널(서버의 텍스트 채널)에 공개 스레드를 열고 스레드 채널 ID를 돌려준다(결정 0014).
    fn create_thread(
        &self,
        channel_id: u64,
        name: &str,
    ) -> impl Future<Output = Result<u64, ChannelError>> + Send;
    /// 서버 이름(장소 표시용, 결정 0024).
    fn guild_name(
        &self,
        guild_id: u64,
    ) -> impl Future<Output = Result<String, ChannelError>> + Send;
    /// 채널 이름(장소 표시용).
    fn channel_name(
        &self,
        channel_id: u64,
    ) -> impl Future<Output = Result<String, ChannelError>> + Send;
    /// "입력 중" 표시(10초 동안).
    fn trigger_typing(
        &self,
        channel_id: u64,
    ) -> impl Future<Output = Result<(), ChannelError>> + Send;
    /// 버튼 누름에 "처리 중"으로 응답한다(메시지는 그대로).
    fn ack_interaction(
        &self,
        application_id: u64,
        interaction_id: u64,
        token: &str,
    ) -> impl Future<Output = Result<(), ChannelError>> + Send;
    /// 슬래시 명령에 글로 답한다(`private`이면 부른 사람에게만 보인다, 결정 0040).
    fn respond_message(
        &self,
        application_id: u64,
        interaction_id: u64,
        token: &str,
        content: &str,
        private: bool,
    ) -> impl Future<Output = Result<(), ChannelError>> + Send;
    /// 전역 슬래시 명령을 통째로 덮어써 등록한다(결정 0040).
    fn set_commands(
        &self,
        application_id: u64,
        commands: &[twilight_model::application::command::Command],
    ) -> impl Future<Output = Result<(), ChannelError>> + Send;
}

pub struct TwilightApi {
    client: twilight_http::Client,
}

impl TwilightApi {
    /// 보내는 글의 멘션은 아무도 부르지 않는다(결정 0024): 서버 채널에서 모델이 쓴 `@everyone`이나 사람 멘션이
    /// 실제 알림으로 가지 않게. 멘션은 글로만 보인다.
    pub fn new(token: String) -> Self {
        use twilight_model::channel::message::AllowedMentions;
        Self {
            client: twilight_http::Client::builder()
                .token(token)
                .default_allowed_mentions(AllowedMentions::default())
                .build(),
        }
    }

    pub fn client(&self) -> &twilight_http::Client {
        &self.client
    }
}

/// 토큰 거부(401·403)와 그 밖의 오류(네트워크 등, 다시 시도할 수 있음)를 나눈다.
fn http_error(e: twilight_http::Error) -> ChannelError {
    use twilight_http::error::ErrorType;
    match e.kind() {
        ErrorType::Unauthorized => ChannelError::Unauthorized(e.to_string()),
        ErrorType::Response { status, .. } if status.get() == 401 => {
            ChannelError::Unauthorized(e.to_string())
        }
        // 서버에서의 403은 대개 그 채널·동작의 권한이 없다는 뜻이다(토큰 문제가 아니다).
        ErrorType::Response { status, .. } if status.get() == 403 => {
            ChannelError::Forbidden(e.to_string())
        }
        _ => ChannelError::Send(e.to_string()),
    }
}

fn body_error(e: twilight_http::response::DeserializeBodyError) -> ChannelError {
    ChannelError::Send(e.to_string())
}

fn id<T>(n: u64) -> Result<Id<T>, ChannelError> {
    Id::new_checked(n).ok_or_else(|| ChannelError::Send("ID가 0이다".into()))
}

impl DiscordApi for TwilightApi {
    async fn set_avatar(&self, data_uri: Option<&str>) -> Result<(), ChannelError> {
        self.client
            .update_current_user()
            .avatar(data_uri)
            .await
            .map_err(http_error)?;
        Ok(())
    }

    async fn avatar_url(&self) -> Result<Option<String>, ChannelError> {
        let user = self
            .client
            .current_user()
            .await
            .map_err(http_error)?
            .model()
            .await
            .map_err(body_error)?;
        // 공식 문서의 CDN 주소: avatars/<사용자 ID>/<아바타 해시>.png, size는 2의 거듭제곱.
        Ok(user.avatar.map(|hash| {
            format!(
                "https://cdn.discordapp.com/avatars/{}/{hash}.png?size=1024",
                user.id
            )
        }))
    }

    async fn create_sticker_message(
        &self,
        channel_id: u64,
        sticker_id: u64,
    ) -> Result<(), ChannelError> {
        let sticker = twilight_model::id::Id::new_checked(sticker_id)
            .ok_or_else(|| ChannelError::Send("스티커 ID가 0이다".into()))?;
        self.client
            .create_message(id(channel_id)?)
            .sticker_ids(&[sticker])
            .await
            .map_err(http_error)?;
        Ok(())
    }

    async fn create_message(&self, channel_id: u64, content: &str) -> Result<(), ChannelError> {
        // 글 메시지는 링크 미리보기를 끈다(결정 0028). 카드(임베드)에는 쓰지 않는다(카드까지 숨겨진다).
        self.client
            .create_message(id(channel_id)?)
            .content(content)
            .flags(twilight_model::channel::message::MessageFlags::SUPPRESS_EMBEDS)
            .await
            .map_err(http_error)?;
        Ok(())
    }

    async fn open_dm(&self, user_id: u64) -> Result<u64, ChannelError> {
        let channel = self
            .client
            .create_private_channel(id(user_id)?)
            .await
            .map_err(http_error)?
            .model()
            .await
            .map_err(body_error)?;
        Ok(channel.id.get())
    }

    async fn current_user(&self) -> Result<BotUser, ChannelError> {
        let user = self
            .client
            .current_user()
            .await
            .map_err(http_error)?
            .model()
            .await
            .map_err(body_error)?;
        Ok(BotUser {
            id: user.id.get(),
            name: user.name,
        })
    }

    async fn create_embeds(
        &self,
        channel_id: u64,
        content: &str,
        embeds: &[Embed],
    ) -> Result<(), ChannelError> {
        let mut request = self.client.create_message(id(channel_id)?).embeds(embeds);
        if !content.trim().is_empty() {
            request = request.content(content);
        }
        request.await.map_err(http_error)?;
        Ok(())
    }

    async fn create_card(
        &self,
        channel_id: u64,
        content: &str,
        embeds: &[Embed],
        buttons: &[CardButton],
    ) -> Result<u64, ChannelError> {
        use twilight_model::channel::message::component::{
            ActionRow, Button, ButtonStyle, Component,
        };
        let row = Component::ActionRow(ActionRow {
            id: None,
            components: buttons
                .iter()
                .map(|b| {
                    Component::Button(Button {
                        custom_id: Some(button_id(b)),
                        disabled: false,
                        emoji: None,
                        id: None,
                        label: Some(b.label.clone()),
                        sku_id: None,
                        style: if b.primary {
                            ButtonStyle::Success
                        } else {
                            ButtonStyle::Secondary
                        },
                        url: None,
                    })
                })
                .collect(),
        });
        let rows = [row];
        let mut request = self
            .client
            .create_message(id(channel_id)?)
            .embeds(embeds)
            .components(if buttons.is_empty() { &[] } else { &rows });
        if !content.trim().is_empty() {
            request = request.content(content);
        }
        let message = request
            .await
            .map_err(http_error)?
            .model()
            .await
            .map_err(body_error)?;
        Ok(message.id.get())
    }

    async fn close_card(
        &self,
        channel_id: u64,
        message_id: u64,
        content: &str,
        embeds: &[Embed],
    ) -> Result<(), ChannelError> {
        let content = (!content.trim().is_empty()).then_some(content);
        self.client
            .update_message(id(channel_id)?, id(message_id)?)
            .content(content)
            .embeds(Some(embeds))
            .components(Some(&[]))
            .await
            .map_err(http_error)?;
        Ok(())
    }

    async fn create_message_with_files(
        &self,
        channel_id: u64,
        content: &str,
        files: &[OutFile],
    ) -> Result<(), ChannelError> {
        use twilight_model::http::attachment::Attachment;
        let attachments: Vec<Attachment> = files
            .iter()
            .enumerate()
            .map(|(i, f)| Attachment::from_bytes(f.name.clone(), f.bytes.clone(), i as u64))
            .collect();
        let mut request = self
            .client
            .create_message(id(channel_id)?)
            .attachments(&attachments)
            .flags(twilight_model::channel::message::MessageFlags::SUPPRESS_EMBEDS);
        if !content.trim().is_empty() {
            request = request.content(content);
        }
        request.await.map_err(http_error)?;
        Ok(())
    }

    async fn guild_name(&self, guild_id: u64) -> Result<String, ChannelError> {
        let guild = self
            .client
            .guild(id(guild_id)?)
            .await
            .map_err(http_error)?
            .model()
            .await
            .map_err(body_error)?;
        Ok(guild.name)
    }

    async fn channel_name(&self, channel_id: u64) -> Result<String, ChannelError> {
        let channel = self
            .client
            .channel(id(channel_id)?)
            .await
            .map_err(http_error)?
            .model()
            .await
            .map_err(body_error)?;
        Ok(channel.name.unwrap_or_default())
    }

    async fn trigger_typing(&self, channel_id: u64) -> Result<(), ChannelError> {
        self.client
            .create_typing_trigger(id(channel_id)?)
            .await
            .map_err(http_error)?;
        Ok(())
    }

    async fn create_thread(&self, channel_id: u64, name: &str) -> Result<u64, ChannelError> {
        use twilight_model::channel::ChannelType;
        use twilight_model::channel::thread::AutoArchiveDuration;
        let thread = self
            .client
            .create_thread(id(channel_id)?, name, ChannelType::PublicThread)
            .auto_archive_duration(AutoArchiveDuration::Week)
            .await
            .map_err(http_error)?
            .model()
            .await
            .map_err(body_error)?;
        Ok(thread.id.get())
    }

    async fn respond_modal(
        &self,
        application_id: u64,
        interaction_id: u64,
        token: &str,
        custom_id: &str,
        prompt: &ButtonPrompt,
    ) -> Result<(), ChannelError> {
        use twilight_model::channel::message::component::{
            Component, Label, TextInput, TextInputStyle,
        };
        use twilight_model::http::interaction::{
            InteractionResponse, InteractionResponseData, InteractionResponseType,
        };
        #[allow(deprecated)]
        let input = TextInput {
            custom_id: MODAL_INPUT_ID.to_owned(),
            id: None,
            label: None,
            max_length: Some(prompt.max_len),
            min_length: Some(prompt.min_len),
            placeholder: prompt.placeholder.clone(),
            required: Some(true),
            style: TextInputStyle::Short,
            value: None,
        };
        let response = InteractionResponse {
            kind: InteractionResponseType::Modal,
            data: Some(InteractionResponseData {
                custom_id: Some(custom_id.to_owned()),
                title: Some(prompt.title.chars().take(45).collect()),
                components: Some(vec![Component::Label(Label {
                    component: Box::new(Component::TextInput(input)),
                    description: None,
                    id: None,
                    label: prompt.label.chars().take(45).collect(),
                })]),
                ..Default::default()
            }),
        };
        self.client
            .interaction(id(application_id)?)
            .create_response(id(interaction_id)?, token, &response)
            .await
            .map_err(http_error)?;
        Ok(())
    }

    async fn ack_interaction(
        &self,
        application_id: u64,
        interaction_id: u64,
        token: &str,
    ) -> Result<(), ChannelError> {
        use twilight_model::http::interaction::{InteractionResponse, InteractionResponseType};
        let response = InteractionResponse {
            kind: InteractionResponseType::DeferredUpdateMessage,
            data: None,
        };
        self.client
            .interaction(id(application_id)?)
            .create_response(id(interaction_id)?, token, &response)
            .await
            .map_err(http_error)?;
        Ok(())
    }

    async fn respond_message(
        &self,
        application_id: u64,
        interaction_id: u64,
        token: &str,
        content: &str,
        private: bool,
    ) -> Result<(), ChannelError> {
        use twilight_model::channel::message::MessageFlags;
        use twilight_model::http::interaction::{
            InteractionResponse, InteractionResponseData, InteractionResponseType,
        };
        let response = InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(InteractionResponseData {
                content: Some(content.chars().take(MAX_MESSAGE_CHARS).collect()),
                flags: private.then_some(MessageFlags::EPHEMERAL),
                ..Default::default()
            }),
        };
        self.client
            .interaction(id(application_id)?)
            .create_response(id(interaction_id)?, token, &response)
            .await
            .map_err(http_error)?;
        Ok(())
    }

    async fn set_commands(
        &self,
        application_id: u64,
        commands: &[twilight_model::application::command::Command],
    ) -> Result<(), ChannelError> {
        self.client
            .interaction(id(application_id)?)
            .set_global_commands(commands)
            .await
            .map_err(http_error)?;
        Ok(())
    }
}

/// 값을 받는 버튼의 custom_id 앞머리(결정 0017). 누르면 입력창을 띄우고, 제출된 값만 코어로 넘긴다.
pub const PROMPT_PREFIX: &str = "p|";
/// 입력창 안 칸의 ID.
const MODAL_INPUT_ID: &str = "value";

/// 버튼의 Discord custom_id(값을 받는 버튼이면 앞머리를 붙인다).
pub fn button_id(b: &CardButton) -> String {
    if b.prompt.is_some() {
        format!("{PROMPT_PREFIX}{}", b.action)
    } else {
        b.action.clone()
    }
}

/// 재시작 등으로 입력창 모양을 모를 때.
fn default_prompt() -> ButtonPrompt {
    ButtonPrompt {
        title: "입력".into(),
        label: "값".into(),
        placeholder: None,
        min_len: 1,
        max_len: 100,
    }
}

pub fn conversation_for(channel_id: u64) -> ConversationId {
    ConversationId(format!("{CONVERSATION_PREFIX}{channel_id}"))
}

pub fn channel_of(conversation: &ConversationId) -> Option<u64> {
    conversation
        .0
        .strip_prefix(CONVERSATION_PREFIX)?
        .parse()
        .ok()
}

/// 게이트웨이 메시지에서 필요한 것만.
#[derive(Debug, Clone, Default)]
pub struct MessageInfo {
    pub id: u64,
    pub author_id: u64,
    /// 쓴 사람의 표시 이름(서버 별명 → 전역 이름 → 사용자 이름).
    pub author_name: String,
    pub author_is_bot: bool,
    /// 보통 글·답장이 아닌 메시지(스레드 시작 알림, 고정 알림, 입장 등 시스템 메시지).
    pub system: bool,
    /// 서버 메시지면 그 서버.
    pub guild_id: Option<u64>,
    pub channel_id: u64,
    pub content: String,
    /// 첨부(DM에서는 MESSAGE_CONTENT intent 없이도 온다: 문서의 "DM의 내용" 예외).
    pub attachments: Vec<InboundAttachment>,
    /// 멘션된 사용자: (ID, 표시 이름).
    pub mentions: Vec<(u64, String)>,
    /// 답장한 원문(결정 0024).
    pub referenced: Option<Box<MessageInfo>>,
    pub sent_at: Option<chrono::DateTime<Utc>>,
}

impl MessageInfo {
    /// twilight 메시지에서. 답장 원문은 한 단계만 따라간다.
    pub fn from_model(m: &twilight_model::channel::Message) -> Self {
        Self::from_model_depth(m, true)
    }

    fn from_model_depth(m: &twilight_model::channel::Message, follow: bool) -> Self {
        let author_name = m
            .member
            .as_ref()
            .and_then(|mem| mem.nick.clone())
            .or_else(|| m.author.global_name.clone())
            .unwrap_or_else(|| m.author.name.clone());
        Self {
            id: m.id.get(),
            author_id: m.author.id.get(),
            author_name,
            author_is_bot: m.author.bot,
            system: !matches!(
                m.kind,
                twilight_model::channel::message::MessageType::Regular
                    | twilight_model::channel::message::MessageType::Reply
            ),
            guild_id: m.guild_id.map(|g| g.get()),
            channel_id: m.channel_id.get(),
            content: m.content.clone(),
            attachments: m
                .attachments
                .iter()
                .map(|a| InboundAttachment {
                    url: a.url.clone(),
                    filename: a.filename.clone(),
                    content_type: a.content_type.clone(),
                    size: a.size,
                })
                .collect(),
            mentions: m
                .mentions
                .iter()
                .map(|u| {
                    let name = u
                        .member
                        .as_ref()
                        .and_then(|mem| mem.nick.clone())
                        .unwrap_or_else(|| u.name.clone());
                    (u.id.get(), name)
                })
                .collect(),
            referenced: if follow {
                m.referenced_message
                    .as_deref()
                    .map(|r| Box::new(Self::from_model_depth(r, false)))
            } else {
                None
            },
            sent_at: chrono::DateTime::from_timestamp_micros(m.timestamp.as_micros()),
        }
    }
}

/// 머리말이 될 수 있는 글인가(결정 0024): 1~20자, 앞뒤 공백 없음, 줄바꿈·백틱 없음.
pub fn valid_prefix(prefix: &str) -> bool {
    let n = prefix.chars().count();
    (1..=20).contains(&n)
        && prefix.trim() == prefix
        && !prefix.chars().any(|c| c.is_control() || c == '`')
}

/// 글 맨 앞의 머리말을 떼고 남은 글. 대소문자는 가리지 않는다. 머리말이 글자(숫자)로 끝나면 바로 뒤에
/// 글자가 붙은 경우는 아니다("!lucia"가 "!luciaX"에 맞지 않게).
pub fn strip_prefix<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if !valid_prefix(prefix) {
        return None;
    }
    let text = text.trim_start();
    let mut rest = text.char_indices();
    let mut want = prefix.chars();
    loop {
        match (want.next(), rest.next()) {
            (None, next) => {
                let cut = next.map(|(i, _)| i).unwrap_or(text.len());
                let after = &text[cut..];
                let glued = prefix.chars().last().is_some_and(char::is_alphanumeric)
                    && after.chars().next().is_some_and(char::is_alphanumeric);
                return (!glued).then(|| after.trim_start());
            }
            (Some(w), Some((_, c))) if w.to_lowercase().eq(c.to_lowercase()) => {}
            _ => return None,
        }
    }
}

/// 멘션 표기를 사람이 읽는 이름으로 바꾸고 봇 멘션은 지운다.
fn humanize_mentions(text: &str, bot_id: u64, names: &[(u64, String)]) -> String {
    let mut out = text.to_owned();
    if bot_id != 0 {
        for form in [format!("<@{bot_id}>"), format!("<@!{bot_id}>")] {
            out = out.replace(&form, "");
        }
    }
    for (uid, name) in names {
        if *uid == bot_id {
            continue;
        }
        for form in [format!("<@{uid}>"), format!("<@!{uid}>")] {
            out = out.replace(&form, &format!("@{name}"));
        }
    }
    out.trim().to_owned()
}

/// 듣는 채널(봇을 부르지 않아도 받는 서버 채널, 예: 작업 스레드)인지 알려 주는 함수.
pub type ListenFn = Arc<dyn Fn(u64) -> bool + Send + Sync>;

/// 사람만, 한 번만 넘긴다. 서버 채널은 켰을 때만(결정 0014·0024). 누가 허용된 사람인지는 코어가 본다.
#[derive(Default)]
pub struct InboundFilter {
    seen: Mutex<VecDeque<u64>>,
    /// 서버 채널의 말도 받는다. 켜면 게이트웨이가 서버 메시지 intent를 요청한다.
    guild: AtomicBool,
    /// 서버 메시지 내용 intent(특권 `MESSAGE_CONTENT`)도 요청한다. 꺼지면 멘션한 말만 내용이 온다.
    content: AtomicBool,
    /// 봇 자신의 사용자 ID(멘션·답장 판정). 0이면 모른다.
    bot_id: AtomicU64,
    /// 서버에서 받을 사람(소유자). 0이면 거르지 않는다(코어가 다시 본다). 바쁜 서버의 다른 사람 말이
    /// 큐에 들어가지 않게 하는 거름이지, 허용 판단이 아니다.
    owner_id: AtomicU64,
    /// 다른 서버에서 부르기(결정 0024). 꺼져 있으면 서버에서는 듣는 채널(작업 스레드)의 말만 받는다.
    calls: AtomicBool,
    /// 정한 머리말(결정 0024). 없으면 멘션·답장으로만 부른다.
    prefix: RwLock<Option<String>>,
    /// 듣는 채널(작업 스레드 등).
    listen: RwLock<Option<ListenFn>>,
    /// 서버·채널 이름(장소 표시). 바뀌는 일이 드물어 오래 둔다.
    names: Mutex<HashMap<u64, String>>,
}

impl InboundFilter {
    const REMEMBER: usize = 512;
    const NAMES_MAX: usize = 1000;

    /// 서버 채널의 말을 받을지. 게이트웨이를 다시 붙여야 intent가 바뀐다.
    pub fn set_guild(&self, on: bool) {
        self.guild.store(on, Ordering::SeqCst);
    }

    pub fn guild(&self) -> bool {
        self.guild.load(Ordering::SeqCst)
    }

    /// 서버 메시지 내용 intent를 요청할지(Discord가 거절하면 끈다).
    pub fn set_content(&self, on: bool) {
        self.content.store(on, Ordering::SeqCst);
    }

    pub fn content(&self) -> bool {
        self.content.load(Ordering::SeqCst)
    }

    pub fn set_calls(&self, on: bool) {
        self.calls.store(on, Ordering::SeqCst);
    }

    pub fn set_bot(&self, id: u64) {
        self.bot_id.store(id, Ordering::SeqCst);
    }

    pub fn set_owner(&self, id: Option<u64>) {
        self.owner_id.store(id.unwrap_or(0), Ordering::SeqCst);
    }

    /// 머리말. 머리말이 될 수 없는 글이면 없는 것으로 둔다(빈 머리말은 모든 말에 맞는다).
    pub fn set_prefix(&self, prefix: Option<String>) {
        *self.prefix.write().expect("prefix lock") = prefix.filter(|p| valid_prefix(p));
    }

    pub fn set_listen(&self, listen: Option<ListenFn>) {
        *self.listen.write().expect("listen lock") = listen;
    }

    fn listening(&self, channel: u64) -> bool {
        self.listen
            .read()
            .expect("listen lock")
            .as_ref()
            .is_some_and(|f| f(channel))
    }

    /// 봇을 불렀는가, 그리고 부른 표시를 뗀 글.
    fn address(&self, m: &MessageInfo) -> (bool, String) {
        let bot = self.bot_id.load(Ordering::SeqCst);
        let mentioned = bot != 0
            && (m.mentions.iter().any(|(id, _)| *id == bot)
                || m.content.contains(&format!("<@{bot}>"))
                || m.content.contains(&format!("<@!{bot}>")));
        let replied = bot != 0 && m.referenced.as_ref().is_some_and(|r| r.author_id == bot);
        let prefix = self.prefix.read().expect("prefix lock").clone();
        let after_prefix = prefix.as_deref().and_then(|p| strip_prefix(&m.content, p));
        let text = after_prefix.unwrap_or(&m.content);
        (
            mentioned || replied || after_prefix.is_some(),
            humanize_mentions(text, bot, &m.mentions),
        )
    }

    pub fn accept(&self, m: &MessageInfo) -> Option<Inbound> {
        if m.author_is_bot
            || m.system
            || (m.content.trim().is_empty() && m.attachments.is_empty() && m.referenced.is_none())
        {
            return None;
        }
        let (addressed, body) = self.address(m);
        let venue = match m.guild_id {
            None => None,
            Some(guild) => {
                if !self.guild() {
                    return None;
                }
                let owner = self.owner_id.load(Ordering::SeqCst);
                if owner != 0 && m.author_id != owner {
                    return None;
                }
                let called = addressed && self.calls.load(Ordering::SeqCst);
                if !called && !self.listening(m.channel_id) {
                    return None;
                }
                Some(Venue {
                    space_id: guild.to_string(),
                    space_name: None,
                    room_id: m.channel_id.to_string(),
                    room_name: None,
                    addressed,
                })
            }
        };
        // 부르기만 하고 글·첨부·답장이 없는 말도 넘긴다(코어가 "이름만 불렀다"로 다룬다). 그 밖의 빈 말은 버린다.
        if body.is_empty() && m.attachments.is_empty() && m.referenced.is_none() && !addressed {
            return None;
        }
        let mut seen = self.seen.lock().expect("seen lock");
        if seen.contains(&m.id) {
            return None;
        }
        seen.push_back(m.id);
        if seen.len() > Self::REMEMBER {
            seen.pop_front();
        }
        let bot = self.bot_id.load(Ordering::SeqCst);
        Some(Inbound {
            platform: PLATFORM.to_owned(),
            sender: m.author_id.to_string(),
            conversation: conversation_for(m.channel_id),
            body,
            attachments: m.attachments.clone(),
            received_at: Utc::now(),
            reply_to: m.referenced.as_deref().map(|r| Quoted {
                id: r.id.to_string(),
                author: r.author_name.clone(),
                author_id: r.author_id.to_string(),
                from_self: bot != 0 && r.author_id == bot,
                body: humanize_mentions(&r.content, 0, &r.mentions),
                attachments: r.attachments.clone(),
                sent_at: r.sent_at,
            }),
            venue,
            context: None,
        })
    }

    /// 장소의 서버·채널 이름을 채운다(모르면 REST로 한 번 묻고 기억한다). 늦어도 2초 안에 끝낸다.
    pub async fn fill_names<A: DiscordApi>(&self, api: &A, venue: &mut Venue) {
        let cached = |id: &str| -> Option<String> {
            let id: u64 = id.parse().ok()?;
            self.names.lock().expect("names lock").get(&id).cloned()
        };
        let remember = |id: &str, name: &str| {
            if let Ok(id) = id.parse::<u64>() {
                let mut names = self.names.lock().expect("names lock");
                if names.len() > Self::NAMES_MAX {
                    names.clear();
                }
                names.insert(id, name.to_owned());
            }
        };
        let wait = std::time::Duration::from_secs(2);
        venue.space_name = match cached(&venue.space_id) {
            Some(n) => Some(n),
            None => match venue.space_id.parse() {
                Ok(gid) => match tokio::time::timeout(wait, api.guild_name(gid)).await {
                    Ok(Ok(n)) => {
                        remember(&venue.space_id, &n);
                        Some(n)
                    }
                    _ => None,
                },
                Err(_) => None,
            },
        };
        venue.room_name = match cached(&venue.room_id) {
            Some(n) => Some(n),
            None => match venue.room_id.parse() {
                Ok(cid) => match tokio::time::timeout(wait, api.channel_name(cid)).await {
                    Ok(Ok(n)) => {
                        remember(&venue.room_id, &n);
                        Some(n)
                    }
                    _ => None,
                },
                Err(_) => None,
            },
        };
    }
}

pub struct DiscordAdapter<A> {
    api: Arc<A>,
    connected: Arc<AtomicBool>,
    /// 값을 받는 버튼의 입력창 모양(결정 0017). 버튼 누름 처리([`PressSink`])와 함께 쓴다.
    prompts: Arc<Mutex<HashMap<String, ButtonPrompt>>>,
}

impl<A: DiscordApi> DiscordAdapter<A> {
    /// `connected`는 게이트웨이 작업([`run_gateway`])이 갱신한다.
    pub fn new(api: A, connected: Arc<AtomicBool>) -> Self {
        Self {
            api: Arc::new(api),
            connected,
            prompts: Arc::default(),
        }
    }

    /// 값을 받는 버튼의 입력창 모양(버튼 누름 처리와 함께 쓴다).
    pub fn prompts_handle(&self) -> Arc<Mutex<HashMap<String, ButtonPrompt>>> {
        self.prompts.clone()
    }

    pub fn api(&self) -> &A {
        &self.api
    }

    /// 게이트웨이가 버튼 누름에 응답할 때 쓴다.
    pub fn api_handle(&self) -> Arc<A> {
        self.api.clone()
    }
}

/// 작은 글씨(Discord `-# `). 줄마다 붙이고, 빈 줄은 버린다. 한 메시지 길이 안으로 자른다.
pub fn subtext(body: &str) -> String {
    let text = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| format!("-# {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    match text.char_indices().nth(MAX_MESSAGE_CHARS) {
        Some((i, _)) => text[..i].to_owned(),
        None => text,
    }
}

fn message_ref(r: &str) -> Result<u64, ChannelError> {
    r.parse()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| ChannelError::Send(format!("Discord 메시지 참조가 아니다: {r}")))
}

impl<A: DiscordApi> ChannelAdapter for DiscordAdapter<A> {
    fn platform(&self) -> &str {
        PLATFORM
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            max_message_chars: MAX_MESSAGE_CHARS,
            buttons: true,
            edit: true,
            // 작업 스레드(결정 0014): 서버의 작업 채널에서만 연다. DM에는 스레드가 없다.
            threads: true,
            // 값을 받는 버튼(입력창, 결정 0017).
            prompts: true,
            forms: false,
        }
    }

    async fn send(&self, message: Outbound) -> Result<usize, ChannelError> {
        let channel = channel_of(message.conversation()).ok_or_else(|| {
            ChannelError::Send(format!(
                "Discord 대화가 아니다: {}",
                message.conversation().0
            ))
        })?;
        match message {
            Outbound::Text { body, .. } => {
                if body.trim().is_empty() {
                    return Err(ChannelError::Empty);
                }
                // 나누기 전에 감싼다(주소가 조각 경계에서 갈리지 않게, 결정 0028).
                let parts = split_message(&crate::quiet_links(&body), MAX_MESSAGE_CHARS);
                for part in &parts {
                    self.api.create_message(channel, part).await?;
                }
                Ok(parts.len())
            }
            Outbound::Note { body, .. } => {
                let text = subtext(&crate::quiet_links(&body));
                if text.is_empty() {
                    return Err(ChannelError::Empty);
                }
                self.api.create_message(channel, &text).await?;
                Ok(1)
            }
            Outbound::Files { body, files, .. } => {
                if files.is_empty() {
                    return Err(ChannelError::Empty);
                }
                check_files(&files)?;
                let mut parts = split_message(&crate::quiet_links(&body), MAX_MESSAGE_CHARS);
                let last = parts.pop().unwrap_or_default();
                for part in &parts {
                    self.api.create_message(channel, part).await?;
                }
                self.api
                    .create_message_with_files(channel, &last, &files)
                    .await?;
                Ok(parts.len() + 1)
            }
        }
    }

    async fn typing(&self, conversation: &ConversationId) -> Result<(), ChannelError> {
        let channel = channel_of(conversation).ok_or_else(|| {
            ChannelError::Send(format!("Discord 대화가 아니다: {}", conversation.0))
        })?;
        self.api.trigger_typing(channel).await
    }

    async fn open_thread(
        &self,
        parent: &ConversationId,
        name: &str,
    ) -> Result<ConversationId, ChannelError> {
        let channel = channel_of(parent)
            .ok_or_else(|| ChannelError::Send(format!("Discord 대화가 아니다: {}", parent.0)))?;
        // Discord 스레드 이름은 1~100자.
        let name: String = name.trim().chars().take(100).collect();
        let name = if name.is_empty() {
            "작업".to_owned()
        } else {
            name
        };
        let thread = self.api.create_thread(channel, &name).await?;
        Ok(conversation_for(thread))
    }

    fn ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn send_card(&self, card: Card) -> Result<String, ChannelError> {
        if card.title.trim().is_empty() && card.body.trim().is_empty() {
            return Err(ChannelError::Empty);
        }
        let channel = channel_of(&card.conversation).ok_or_else(|| {
            ChannelError::Send(format!("Discord 대화가 아니다: {}", card.conversation.0))
        })?;
        {
            let mut prompts = self.prompts.lock().expect("prompts lock");
            // 오래된 것이 쌓이지 않게(입력창은 몇 분짜리다).
            if prompts.len() > 200 {
                prompts.clear();
            }
            for b in &card.buttons {
                if let Some(p) = &b.prompt {
                    prompts.insert(button_id(b), p.clone());
                }
            }
        }
        let mut messages = card::render(&card);
        let last = messages.pop().expect("메시지 하나 이상");
        for m in &messages {
            self.api
                .create_embeds(channel, &m.content, &m.embeds)
                .await?;
        }
        // 여러 칸을 받는 버튼(`form`, 결정 0063)은 그리지 않는다: 값(비밀번호)이 Discord 서버를 거치게 된다.
        // 그 카드를 만든 쪽이 본문에 다른 길(입력 링크)을 함께 준다.
        let buttons: Vec<CardButton> = card
            .buttons
            .iter()
            .filter(|b| b.form.is_none())
            .cloned()
            .collect();
        let id = self
            .api
            .create_card(channel, &last.content, &last.embeds, &buttons)
            .await?;
        Ok(id.to_string())
    }

    async fn send_sticker(
        &self,
        conversation: &ConversationId,
        sticker_id: &str,
    ) -> Result<(), ChannelError> {
        let channel = channel_of(conversation).ok_or_else(|| {
            ChannelError::Send(format!("Discord 대화가 아니다: {}", conversation.0))
        })?;
        let sticker: u64 = sticker_id
            .parse()
            .map_err(|_| ChannelError::Send("스티커 ID 형식이 아니다".into()))?;
        self.api.create_sticker_message(channel, sticker).await
    }

    async fn close_card(&self, message_ref_text: &str, card: &Card) -> Result<(), ChannelError> {
        let channel = channel_of(&card.conversation).ok_or_else(|| {
            ChannelError::Send(format!("Discord 대화가 아니다: {}", card.conversation.0))
        })?;
        let message = message_ref(message_ref_text)?;
        // 보낼 때와 같은 모양으로 다시 그려 버튼이 달린 마지막 메시지만 고친다.
        let last = card::render(card).pop().expect("메시지 하나 이상");
        self.api
            .close_card(channel, message, &last.content, &last.embeds)
            .await
    }
}

/// 게이트웨이 버튼 누름에서 필요한 것만.
#[derive(Debug, Clone)]
pub struct PressInfo {
    pub application_id: u64,
    pub interaction_id: u64,
    pub token: String,
    pub user_id: Option<u64>,
    pub in_guild: bool,
    pub channel_id: Option<u64>,
    pub message_id: Option<u64>,
    pub custom_id: String,
}

impl PressInfo {
    /// DM에서 누른 버튼만 코어로 넘긴다. `allow_guild`면 서버 채널(작업 스레드)의 버튼도(결정 0014).
    pub fn to_press(&self, allow_guild: bool) -> Option<ButtonPress> {
        if self.in_guild && !allow_guild {
            return None;
        }
        Some(ButtonPress {
            platform: PLATFORM.to_owned(),
            sender: self.user_id?.to_string(),
            conversation: conversation_for(self.channel_id?),
            message_ref: self.message_id?.to_string(),
            action: self.custom_id.clone(),
            input: None,
            fields: None,
            received_at: Utc::now(),
        })
    }
}

/// 버튼 누름을 받는 곳과 응답에 쓸 API.
pub struct PressSink<A> {
    pub api: Arc<A>,
    pub presses: mpsc::Sender<ButtonPress>,
    /// 서버 채널(작업 스레드)의 버튼도 받는다(결정 0014).
    pub allow_guild: bool,
    /// 값을 받는 버튼의 입력창 모양(custom_id → 모양, 어댑터가 카드를 보낼 때 채운다).
    pub prompts: Arc<Mutex<HashMap<String, ButtonPrompt>>>,
    /// 슬래시 명령을 받는 곳(결정 0040). 없으면 명령을 등록하지 않는다.
    pub commands: Option<mpsc::Sender<crate::SlashCommand>>,
}

/// 등록하는 슬래시 명령: `/권한 [모드]`(결정 0040), `/모델 [모델]`(결정 0055). 옵션은 하나이고 비워도 된다.
pub fn slash_commands() -> Vec<twilight_model::application::command::Command> {
    use twilight_model::application::command::{
        Command, CommandOption, CommandOptionChoice, CommandOptionChoiceValue, CommandOptionType,
        CommandType,
    };
    let choice = |name: &str, value: &str| CommandOptionChoice {
        name: name.to_owned(),
        name_localizations: None,
        value: CommandOptionChoiceValue::String(value.to_owned()),
    };
    #[allow(deprecated)]
    let command = |name: &str,
                   description: &str,
                   option: &str,
                   option_description: &str,
                   choices: Vec<CommandOptionChoice>| Command {
        application_id: None,
        contexts: None,
        default_member_permissions: None,
        description: description.to_owned(),
        description_localizations: None,
        dm_permission: None,
        guild_id: None,
        id: None,
        integration_types: None,
        kind: CommandType::ChatInput,
        name: name.to_owned(),
        name_localizations: None,
        nsfw: None,
        options: vec![CommandOption {
            autocomplete: None,
            channel_types: None,
            choices: Some(choices),
            description: option_description.to_owned(),
            description_localizations: None,
            kind: CommandOptionType::String,
            max_length: None,
            max_value: None,
            min_length: None,
            min_value: None,
            name: option.to_owned(),
            name_localizations: None,
            options: None,
            file_types: None,
            required: Some(false),
        }],
        version: twilight_model::id::Id::new(1),
    };
    vec![
        command(
            crate::COMMAND_SECURITY,
            "Lucia의 권한 모드를 보거나 바꿉니다(소유자만)",
            "모드",
            "바꿀 모드(비우면 지금 모드를 보인다)",
            vec![
                choice("엄격", "strict"),
                choice("기본", "standard"),
                choice("해제", "open"),
            ],
        ),
        command(
            crate::COMMAND_MODEL,
            "Lucia의 주 모델을 보거나 바꿉니다(소유자만)",
            "모델",
            "바꿀 모델(비우면 지금 모델을 보인다)",
            crate::MODEL_CHOICES
                .iter()
                .map(|(name, value)| choice(name, value))
                .collect(),
        ),
    ]
}

/// 슬래시 명령: 받을 곳에 넘기고, 2.5초 안에 온 답으로 응답한다(Discord 응답 제한 3초). 받을 곳이 없거나 늦으면
/// 그렇다고 답한다. 소유자 확인은 받는 쪽(데몬)이 한다.
pub async fn handle_command<A: DiscordApi>(
    sink: &PressSink<A>,
    info: PressInfo,
    name: String,
    option: Option<String>,
) {
    let reply = match (&sink.commands, info.user_id) {
        (Some(tx), Some(user)) => {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let cmd = crate::SlashCommand {
                platform: PLATFORM.to_owned(),
                sender: user.to_string(),
                in_dm: !info.in_guild,
                name,
                option,
                reply: reply_tx,
            };
            if tx.send(cmd).await.is_err() {
                crate::CommandReply::private("명령을 받을 곳이 없습니다.")
            } else {
                match tokio::time::timeout(std::time::Duration::from_millis(2500), reply_rx).await {
                    Ok(Ok(r)) => r,
                    _ => crate::CommandReply::private(
                        "처리하지 못했습니다(시간 초과). 다시 해 주세요.",
                    ),
                }
            }
        }
        _ => crate::CommandReply::private("이 명령을 받을 곳이 없습니다."),
    };
    if let Err(err) = sink
        .api
        .respond_message(
            info.application_id,
            info.interaction_id,
            &info.token,
            &reply.text,
            reply.private,
        )
        .await
    {
        tracing::warn!(%err, "슬래시 명령 응답 실패");
    }
}

/// 버튼 누름 처리: 먼저 응답하고(3초 제한), DM 누름이면 코어로 넘긴다. 게이트웨이를 막지 않게 따로 돈다.
pub async fn handle_press<A: DiscordApi>(sink: &PressSink<A>, info: PressInfo) {
    // 값을 받는 버튼: 먼저 입력창으로 응답한다(입력창은 그 상호작용의 첫 응답이어야 한다). 코어에는 넘기지 않고,
    // 제출된 값만 넘긴다([`handle_modal`]).
    if let Some(action) = info.custom_id.strip_prefix(PROMPT_PREFIX) {
        if info.in_guild && !sink.allow_guild {
            return;
        }
        let prompt = sink
            .prompts
            .lock()
            .expect("prompts lock")
            .get(&info.custom_id)
            .cloned()
            .unwrap_or_else(default_prompt);
        if let Err(err) = sink
            .api
            .respond_modal(
                info.application_id,
                info.interaction_id,
                &info.token,
                action,
                &prompt,
            )
            .await
        {
            tracing::warn!(%err, "입력창을 띄우지 못했다");
        }
        return;
    }
    if let Err(err) = sink
        .api
        .ack_interaction(info.application_id, info.interaction_id, &info.token)
        .await
    {
        tracing::warn!(%err, "버튼 누름 응답 실패");
    }
    match info.to_press(sink.allow_guild) {
        Some(press) => {
            if sink.presses.send(press).await.is_err() {
                tracing::warn!("버튼 누름을 받을 곳이 닫혔다");
            }
        }
        None => tracing::warn!("받지 않는 곳(서버 채널)의 버튼 누름을 버렸다"),
    }
}

/// 입력창 제출(결정 0017).
#[derive(Clone)]
pub struct ModalInfo {
    pub press: PressInfo,
    pub value: lucia_core::SecretText,
}

/// 입력창 제출: 응답하고(3초 제한), 받을 곳이면 값과 함께 코어로 넘긴다.
pub async fn handle_modal<A: DiscordApi>(sink: &PressSink<A>, info: ModalInfo) {
    let p = &info.press;
    if let Err(err) = sink
        .api
        .ack_interaction(p.application_id, p.interaction_id, &p.token)
        .await
    {
        tracing::warn!(%err, "입력창 제출 응답 실패");
    }
    match p.to_press(sink.allow_guild) {
        Some(mut press) => {
            press.input = Some(info.value.clone());
            if sink.presses.send(press).await.is_err() {
                tracing::warn!("버튼 누름을 받을 곳이 닫혔다");
            }
        }
        None => tracing::warn!("받지 않는 곳의 입력창 제출을 버렸다"),
    }
}

/// 입력창 제출 데이터에서 칸의 값을 꺼낸다(Label·ActionRow 안을 찾는다).
pub fn modal_value(
    components: &[twilight_model::application::interaction::modal::ModalInteractionComponent],
) -> Option<String> {
    use twilight_model::application::interaction::modal::ModalInteractionComponent as C;
    for c in components {
        let found = match c {
            C::TextInput(t) if t.custom_id == MODAL_INPUT_ID => Some(t.value.clone()),
            C::Label(l) => modal_value(std::slice::from_ref(&*l.component)),
            C::ActionRow(r) => modal_value(&r.components),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// 요청할 intent. 서버 채널(작업 스레드, 다른 서버에서 부르기)을 켰으면 서버 메시지도, `content`면 그 내용
/// intent도 요청한다. MESSAGE_CONTENT는 특권 intent라 개발자 포털에서 켜 두지 않으면 게이트웨이가 4014로
/// 닫힌다. 그때 데몬은 내용 intent만 빼고 다시 붙는다(멘션한 말은 내용 없이도 온다: 공식 문서의 예외).
pub fn intents_for(guild: bool, content: bool) -> Intents {
    match (guild, content) {
        (false, _) => Intents::DIRECT_MESSAGES,
        (true, false) => Intents::DIRECT_MESSAGES | Intents::GUILD_MESSAGES,
        (true, true) => {
            Intents::DIRECT_MESSAGES | Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT
        }
    }
}

/// 게이트웨이 작업이 끝난 이유.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayExit {
    /// 멈추라고 했다.
    Cancelled,
    /// 다시 붙을 수 없는 종료(토큰 오류, intent 거부 등). 사람이 토큰을 고쳐야 한다.
    Fatal(String),
}

/// 게이트웨이에 붙어 DM을 받는다. 다시 붙을 수 있는 끊김은 twilight가 처리하고(세션 재개),
/// 치명적인 종료면 돌아온다. 같은 토큰으로 다시 부르지 않는다.
#[allow(clippy::too_many_arguments)]
pub async fn run_gateway<A: DiscordApi>(
    token: String,
    api: Arc<A>,
    inbound: mpsc::Sender<Inbound>,
    presses: Option<PressSink<A>>,
    filter: Arc<InboundFilter>,
    connected: Arc<AtomicBool>,
    cancel: tokio_util::sync::CancellationToken,
) -> GatewayExit {
    let intents = intents_for(filter.guild(), filter.content());
    let mut shard = Shard::new(ShardId::ONE, token, intents);
    let wanted = EventTypeFlags::READY
        | EventTypeFlags::RESUMED
        | EventTypeFlags::MESSAGE_CREATE
        | EventTypeFlags::INTERACTION_CREATE;
    // 닫힘(Close) 메시지는 늘 `Event::GatewayClose`로 온다(twilight `next_event` 문서).
    let presses = presses.map(Arc::new);
    // Discord가 닫은 마지막 코드(예: 4014 허용되지 않은 intent). 치명적 종료의 까닭으로 넘긴다.
    let mut last_close: Option<u16> = None;
    let exit = loop {
        let item = tokio::select! {
            _ = cancel.cancelled() => {
                shard.close(CloseFrame::NORMAL);
                break GatewayExit::Cancelled;
            }
            item = shard.next_event(wanted) => item,
        };
        match item {
            Some(Ok(Event::Ready(ready))) => {
                connected.store(true, Ordering::SeqCst);
                tracing::info!("Discord 게이트웨이 연결됨");
                // 슬래시 명령(결정 0040): 받을 곳이 있으면 연결될 때마다 통째로 덮어써 등록한다.
                if let Some(sink) = presses.clone().filter(|s| s.commands.is_some()) {
                    let app = ready.application.id.get();
                    tokio::spawn(async move {
                        match sink.api.set_commands(app, &slash_commands()).await {
                            Ok(()) => tracing::info!("슬래시 명령을 등록했다"),
                            Err(err) => tracing::warn!(%err, "슬래시 명령을 등록하지 못했다"),
                        }
                    });
                }
            }
            Some(Ok(Event::Resumed)) => {
                connected.store(true, Ordering::SeqCst);
                tracing::info!("Discord 게이트웨이 세션 재개");
            }
            Some(Ok(Event::MessageCreate(m))) => {
                let info = MessageInfo::from_model(&m);
                if let Some(mut msg) = filter.accept(&info) {
                    if let Some(venue) = msg.venue.as_mut() {
                        filter.fill_names(api.as_ref(), venue).await;
                    }
                    if inbound.send(msg).await.is_err() {
                        break GatewayExit::Cancelled;
                    }
                }
            }
            Some(Ok(Event::InteractionCreate(i))) => {
                use twilight_model::application::interaction::{InteractionData, InteractionType};
                let Some(sink) = presses.clone() else {
                    continue;
                };
                let channel_id = i
                    .channel
                    .as_ref()
                    .map(|c| c.id.get())
                    .or(i.message.as_ref().map(|m| m.channel_id.get()));
                if i.kind == InteractionType::ModalSubmit {
                    let Some(InteractionData::ModalSubmit(data)) = i.data.as_ref() else {
                        continue;
                    };
                    let Some(value) = modal_value(&data.components) else {
                        continue;
                    };
                    let info = ModalInfo {
                        press: PressInfo {
                            application_id: i.application_id.get(),
                            interaction_id: i.id.get(),
                            token: i.token.clone(),
                            user_id: i.author_id().map(|u| u.get()),
                            in_guild: i.guild_id.is_some(),
                            channel_id,
                            message_id: i.message.as_ref().map(|m| m.id.get()),
                            custom_id: data.custom_id.clone(),
                        },
                        value: lucia_core::SecretText::new(value),
                    };
                    tokio::spawn(async move { handle_modal(&sink, info).await });
                    continue;
                }
                if i.kind == InteractionType::ApplicationCommand {
                    let Some(InteractionData::ApplicationCommand(data)) = i.data.as_ref() else {
                        continue;
                    };
                    let option = data.options.first().and_then(|o| match &o.value {
                        twilight_model::application::interaction::application_command::CommandOptionValue::String(v) => Some(v.clone()),
                        _ => None,
                    });
                    let info = PressInfo {
                        application_id: i.application_id.get(),
                        interaction_id: i.id.get(),
                        token: i.token.clone(),
                        user_id: i.author_id().map(|u| u.get()),
                        in_guild: i.guild_id.is_some(),
                        channel_id,
                        message_id: None,
                        custom_id: String::new(),
                    };
                    let name = data.name.clone();
                    tokio::spawn(async move { handle_command(&sink, info, name, option).await });
                    continue;
                }
                if i.kind != InteractionType::MessageComponent {
                    continue;
                }
                let Some(InteractionData::MessageComponent(data)) = i.data.as_ref() else {
                    continue;
                };
                let info = PressInfo {
                    application_id: i.application_id.get(),
                    interaction_id: i.id.get(),
                    token: i.token.clone(),
                    user_id: i.author_id().map(|u| u.get()),
                    in_guild: i.guild_id.is_some(),
                    channel_id: i
                        .channel
                        .as_ref()
                        .map(|c| c.id.get())
                        .or(i.message.as_ref().map(|m| m.channel_id.get())),
                    message_id: i.message.as_ref().map(|m| m.id.get()),
                    custom_id: data.custom_id.clone(),
                };
                tokio::spawn(async move { handle_press(&sink, info).await });
            }
            Some(Ok(Event::GatewayClose(frame))) => {
                connected.store(false, Ordering::SeqCst);
                last_close = frame.map(|f| f.code);
                tracing::warn!(code = ?last_close, "Discord 게이트웨이가 연결을 닫았다");
            }
            Some(Ok(_)) => {}
            Some(Err(err)) => {
                connected.store(false, Ordering::SeqCst);
                tracing::warn!(%err, "Discord 게이트웨이 수신 오류");
                if shard.state() == ShardState::FatallyClosed {
                    break GatewayExit::Fatal(err.to_string());
                }
            }
            None => {
                break GatewayExit::Fatal(match last_close {
                    Some(code) => format!("게이트웨이가 닫혔다(코드 {code})"),
                    None => format!("게이트웨이가 닫혔다(상태: {:?})", shard.state()),
                });
            }
        }
    };
    connected.store(false, Ordering::SeqCst);
    exit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{self, ContractHarness};

    /// 보낸 파일 메시지: (채널, 내용, 파일 이름들).
    type SentCards = Arc<Mutex<Vec<(u64, String, Vec<String>)>>>;
    /// 보낸 카드 메시지: (채널, 내용, 임베드, 버튼 값들). 버튼 없는 앞부분도 담는다.
    type CardMessages = Arc<Mutex<Vec<(u64, String, Vec<Embed>, Vec<String>)>>>;
    /// 닫은 카드: (채널, 메시지, 내용, 임베드).
    type ClosedCards = Arc<Mutex<Vec<(u64, u64, String, Vec<Embed>)>>>;

    #[derive(Clone, Default)]
    struct FakeApi {
        sent: Arc<Mutex<Vec<(u64, String)>>>,
        cards: CardMessages,
        closed: ClosedCards,
        acks: Arc<Mutex<Vec<u64>>>,
        uploads: SentCards,
        typing: Arc<Mutex<Vec<u64>>>,
        /// 띄운 입력창: (상호작용, custom_id, 제목).
        modals: Arc<Mutex<Vec<(u64, String, String)>>>,
        /// 슬래시 명령 답: (상호작용, 글, 본인만).
        replies: Arc<Mutex<Vec<(u64, String, bool)>>>,
        /// 등록한 명령 이름.
        commands: Arc<Mutex<Vec<String>>>,
        /// 보낸 스티커: (채널, 스티커).
        stickers: Arc<Mutex<Vec<(u64, u64)>>>,
    }

    impl DiscordApi for FakeApi {
        async fn create_sticker_message(
            &self,
            channel_id: u64,
            sticker_id: u64,
        ) -> Result<(), ChannelError> {
            self.stickers.lock().unwrap().push((channel_id, sticker_id));
            Ok(())
        }

        async fn respond_modal(
            &self,
            _application_id: u64,
            interaction_id: u64,
            _token: &str,
            custom_id: &str,
            prompt: &ButtonPrompt,
        ) -> Result<(), ChannelError> {
            self.modals.lock().unwrap().push((
                interaction_id,
                custom_id.to_owned(),
                prompt.title.clone(),
            ));
            Ok(())
        }

        async fn create_message(&self, channel_id: u64, content: &str) -> Result<(), ChannelError> {
            assert!(
                content.chars().count() <= MAX_MESSAGE_CHARS,
                "Discord 길이 제한"
            );
            self.sent
                .lock()
                .unwrap()
                .push((channel_id, content.to_owned()));
            Ok(())
        }

        async fn open_dm(&self, user_id: u64) -> Result<u64, ChannelError> {
            Ok(user_id + 1)
        }

        async fn current_user(&self) -> Result<BotUser, ChannelError> {
            Ok(BotUser {
                id: 1,
                name: "lucia".into(),
            })
        }

        async fn create_embeds(
            &self,
            channel_id: u64,
            content: &str,
            embeds: &[Embed],
        ) -> Result<(), ChannelError> {
            assert!(content.chars().count() <= MAX_MESSAGE_CHARS);
            twilight_validate::message::embeds(embeds).expect("Discord 임베드 한도");
            self.cards.lock().unwrap().push((
                channel_id,
                content.to_owned(),
                embeds.to_vec(),
                Vec::new(),
            ));
            Ok(())
        }

        async fn create_card(
            &self,
            channel_id: u64,
            content: &str,
            embeds: &[Embed],
            buttons: &[CardButton],
        ) -> Result<u64, ChannelError> {
            assert!(content.chars().count() <= MAX_MESSAGE_CHARS);
            twilight_validate::message::embeds(embeds).expect("Discord 임베드 한도");
            let mut cards = self.cards.lock().unwrap();
            cards.push((
                channel_id,
                content.to_owned(),
                embeds.to_vec(),
                buttons.iter().map(|b| b.action.clone()).collect(),
            ));
            Ok(5000 + cards.len() as u64)
        }

        async fn close_card(
            &self,
            channel_id: u64,
            message_id: u64,
            content: &str,
            embeds: &[Embed],
        ) -> Result<(), ChannelError> {
            assert!(content.chars().count() <= MAX_MESSAGE_CHARS);
            twilight_validate::message::embeds(embeds).expect("Discord 임베드 한도");
            self.closed.lock().unwrap().push((
                channel_id,
                message_id,
                content.to_owned(),
                embeds.to_vec(),
            ));
            Ok(())
        }

        async fn ack_interaction(
            &self,
            _application_id: u64,
            interaction_id: u64,
            _token: &str,
        ) -> Result<(), ChannelError> {
            self.acks.lock().unwrap().push(interaction_id);
            Ok(())
        }

        async fn respond_message(
            &self,
            _application_id: u64,
            interaction_id: u64,
            _token: &str,
            content: &str,
            private: bool,
        ) -> Result<(), ChannelError> {
            self.replies
                .lock()
                .unwrap()
                .push((interaction_id, content.to_owned(), private));
            Ok(())
        }

        async fn set_commands(
            &self,
            _application_id: u64,
            commands: &[twilight_model::application::command::Command],
        ) -> Result<(), ChannelError> {
            self.commands
                .lock()
                .unwrap()
                .extend(commands.iter().map(|c| c.name.clone()));
            Ok(())
        }

        async fn create_message_with_files(
            &self,
            channel_id: u64,
            content: &str,
            files: &[OutFile],
        ) -> Result<(), ChannelError> {
            assert!(content.chars().count() <= MAX_MESSAGE_CHARS);
            self.uploads.lock().unwrap().push((
                channel_id,
                content.to_owned(),
                files.iter().map(|f| f.name.clone()).collect(),
            ));
            Ok(())
        }

        async fn guild_name(&self, guild_id: u64) -> Result<String, ChannelError> {
            Ok(format!("서버{guild_id}"))
        }

        async fn channel_name(&self, channel_id: u64) -> Result<String, ChannelError> {
            Ok(format!("채널{channel_id}"))
        }

        async fn trigger_typing(&self, channel_id: u64) -> Result<(), ChannelError> {
            self.typing.lock().unwrap().push(channel_id);
            Ok(())
        }

        async fn create_thread(&self, channel_id: u64, name: &str) -> Result<u64, ChannelError> {
            assert!((1..=100).contains(&name.chars().count()));
            Ok(channel_id * 10 + 1)
        }
    }

    struct Harness {
        adapter: DiscordAdapter<FakeApi>,
        api: FakeApi,
        filter: InboundFilter,
        next_id: u64,
    }

    impl ContractHarness for Harness {
        type Adapter = DiscordAdapter<FakeApi>;

        fn adapter(&self) -> &Self::Adapter {
            &self.adapter
        }

        /// 게이트웨이에서 DM이 온 것처럼 만든다(사용자·채널 ID는 숫자).
        async fn deliver_inbound(
            &mut self,
            sender: &str,
            conversation: &str,
            body: &str,
        ) -> Inbound {
            self.next_id += 1;
            self.filter
                .accept(&MessageInfo {
                    id: self.next_id,
                    author_id: sender.parse().expect("숫자 사용자 ID"),
                    author_is_bot: false,
                    guild_id: None,
                    channel_id: conversation.parse().expect("숫자 채널 ID"),
                    content: body.to_owned(),
                    attachments: Vec::new(),
                    ..Default::default()
                })
                .expect("DM은 받아야 한다")
        }

        fn take_sent(&mut self) -> Vec<(ConversationId, String)> {
            std::mem::take(&mut *self.api.sent.lock().unwrap())
                .into_iter()
                .map(|(channel, body)| (conversation_for(channel), body))
                .collect()
        }
    }

    #[tokio::test]
    async fn discord_adapter_passes_contract() {
        let api = FakeApi::default();
        let mut harness = Harness {
            adapter: DiscordAdapter::new(api.clone(), Arc::new(AtomicBool::new(true))),
            api,
            filter: InboundFilter::default(),
            next_id: 0,
        };
        contract::run_all_with(&mut harness, |name| {
            conversation_for(1000 + name.len() as u64)
        })
        .await;
    }

    /// 결정 0017: 값을 받는 버튼은 누르면 입력창으로 응답하고 코어에 넘기지 않는다. 제출하면 값과 함께 넘긴다.
    #[tokio::test]
    async fn prompt_buttons_open_a_modal_and_forward_only_the_submitted_value() {
        let api = FakeApi::default();
        let (tx, mut rx) = mpsc::channel(4);
        let prompt = ButtonPrompt {
            title: "인증 코드 입력".into(),
            label: "인증 코드".into(),
            placeholder: None,
            min_len: 1,
            max_len: 12,
        };
        let button = CardButton {
            action: "approval:abc:approve".into(),
            label: "코드 입력".into(),
            primary: true,
            prompt: Some(prompt.clone()),
            form: None,
        };
        let id = button_id(&button);
        assert_eq!(id, "p|approval:abc:approve");
        let sink = PressSink {
            api: Arc::new(api.clone()),
            presses: tx,
            allow_guild: false,
            prompts: Arc::new(Mutex::new(HashMap::from([(id.clone(), prompt)]))),
            commands: None,
        };
        let press = PressInfo {
            application_id: 1,
            interaction_id: 70,
            token: "t".into(),
            user_id: Some(42),
            in_guild: false,
            channel_id: Some(900),
            message_id: Some(5),
            custom_id: id,
        };
        handle_press(&sink, press.clone()).await;
        assert_eq!(
            api.modals.lock().unwrap().clone(),
            [(
                70,
                "approval:abc:approve".to_owned(),
                "인증 코드 입력".to_owned()
            )]
        );
        assert!(rx.try_recv().is_err(), "입력창을 연 누름은 넘기지 않는다");
        assert!(api.acks.lock().unwrap().is_empty(), "입력창이 첫 응답이다");

        handle_modal(
            &sink,
            ModalInfo {
                press: PressInfo {
                    interaction_id: 71,
                    custom_id: "approval:abc:approve".into(),
                    ..press
                },
                value: lucia_core::SecretText::new("123456"),
            },
        )
        .await;
        let got = rx.try_recv().unwrap();
        assert_eq!(got.action, "approval:abc:approve");
        assert_eq!(got.input.as_ref().map(|v| v.expose()), Some("123456"));
        assert!(
            !format!("{got:?}").contains("123456"),
            "값은 로그에 찍히지 않는다"
        );
        assert_eq!(*api.acks.lock().unwrap(), [71]);
    }

    /// 결정 0014: 작업 채널을 켜면 서버 채널(작업 스레드)의 말과 버튼도 받는다. 끄면 DM만.
    #[tokio::test]
    async fn guild_threads_are_opt_in() {
        let filter = InboundFilter::default();
        let msg = MessageInfo {
            id: 10,
            author_id: 42,
            author_is_bot: false,
            guild_id: Some(5),
            channel_id: 77,
            content: "바다 가까운 곳".into(),
            attachments: Vec::new(),
            ..Default::default()
        };
        assert!(filter.accept(&msg).is_none());
        filter.set_guild(true);
        assert!(
            filter.accept(&msg).is_none(),
            "부르지 않은 말은 듣는 채널에서만"
        );
        filter.set_listen(Some(Arc::new(|c| c == 77)));
        let got = filter
            .accept(&MessageInfo {
                id: 11,
                ..msg.clone()
            })
            .unwrap();
        assert_eq!(got.conversation, conversation_for(77));
        let press = PressInfo {
            application_id: 1,
            interaction_id: 2,
            token: "t".into(),
            user_id: Some(42),
            in_guild: true,
            channel_id: Some(77),
            message_id: Some(5),
            custom_id: "approval:x:approve".into(),
        };
        assert!(press.to_press(false).is_none());
        assert_eq!(
            press.to_press(true).unwrap().conversation,
            conversation_for(77)
        );

        let adapter = DiscordAdapter::new(FakeApi::default(), Arc::new(AtomicBool::new(true)));
        assert!(adapter.capabilities().threads);
        let thread = adapter
            .open_thread(&conversation_for(500), &"긴 이름".repeat(60))
            .await
            .unwrap();
        assert_eq!(thread, conversation_for(5001));
    }

    #[test]
    fn filter_drops_bots_guilds_empty_and_duplicates() {
        let filter = InboundFilter::default();
        let base = MessageInfo {
            id: 1,
            author_id: 42,
            author_is_bot: false,
            guild_id: None,
            channel_id: 900,
            content: "안녕".into(),
            attachments: Vec::new(),
            ..Default::default()
        };
        let first = filter.accept(&base).unwrap();
        assert_eq!(first.sender, "42");
        assert_eq!(first.conversation, conversation_for(900));
        assert!(filter.accept(&base).is_none(), "같은 메시지는 한 번만");
        assert!(
            filter
                .accept(&MessageInfo {
                    id: 2,
                    author_is_bot: true,
                    ..base.clone()
                })
                .is_none()
        );
        assert!(
            filter
                .accept(&MessageInfo {
                    id: 3,
                    guild_id: Some(5),
                    ..base.clone()
                })
                .is_none()
        );
        assert!(
            filter
                .accept(&MessageInfo {
                    id: 4,
                    content: "  ".into(),
                    ..base.clone()
                })
                .is_none()
        );
    }

    #[tokio::test]
    async fn send_rejects_non_discord_conversations_and_reports_readiness() {
        let connected = Arc::new(AtomicBool::new(false));
        let adapter = DiscordAdapter::new(FakeApi::default(), connected.clone());
        assert!(!adapter.ready());
        connected.store(true, Ordering::SeqCst);
        assert!(adapter.ready());
        let err = adapter
            .send(Outbound::Text {
                conversation: ConversationId("memory:dm".into()),
                body: "x".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::Send(_)));
        assert_eq!(channel_of(&conversation_for(123)), Some(123));
    }

    fn approval_card(body: &str, tone: lucia_core::CardTone, headline: &str) -> Card {
        Card {
            conversation: conversation_for(77),
            headline: headline.into(),
            title: "메일 발송 승인 요청".into(),
            fields: vec![lucia_core::CardField {
                name: "받는 사람".into(),
                value: r"a\@example\.com".into(),
                inline: true,
            }],
            body: body.into(),
            footer: Some("요청 r1".into()),
            tone,
            buttons: if tone == lucia_core::CardTone::Pending {
                vec![
                    CardButton {
                        action: "approval:r1:approve".into(),
                        label: "보내기".into(),
                        primary: true,
                        prompt: None,
                        form: None,
                    },
                    CardButton {
                        action: "approval:r1:deny".into(),
                        label: "거절".into(),
                        primary: false,
                        prompt: None,
                        form: None,
                    },
                ]
            } else {
                vec![]
            },
        }
    }

    /// 긴 카드: 앞부분은 임베드만, 버튼은 마지막 메시지에 한 번. 닫을 때는 그 메시지를 같은 조각으로 고친다.
    #[tokio::test]
    async fn long_cards_put_buttons_on_the_last_message_and_close_in_place() {
        use lucia_core::CardTone;
        let api = FakeApi::default();
        let adapter = DiscordAdapter::new(api.clone(), Arc::new(AtomicBool::new(true)));
        let body = "본문 ".repeat(4000);
        let r = adapter
            .send_card(approval_card(&body, CardTone::Pending, "📨 보내도 될까요?"))
            .await
            .unwrap();
        let sent = api.cards.lock().unwrap().clone();
        assert!(sent.len() > 1, "길면 여러 메시지");
        let with_buttons: Vec<_> = sent.iter().filter(|c| !c.3.is_empty()).collect();
        assert_eq!(with_buttons.len(), 1, "버튼은 한 번");
        assert!(!sent.last().unwrap().3.is_empty(), "버튼은 마지막 메시지에");
        assert_eq!(
            sent.last().unwrap().3,
            ["approval:r1:approve", "approval:r1:deny"]
        );
        let messages: Vec<card::CardMessage> = sent
            .iter()
            .map(|(_, content, embeds, _)| card::CardMessage {
                content: content.clone(),
                embeds: embeds.clone(),
            })
            .collect();
        let sections = card::shown_sections(&messages);
        assert_eq!(sections, [("본문".to_owned(), body.clone())], "빠짐없이");
        assert!(
            api.sent.lock().unwrap().is_empty(),
            "글 메시지로 새지 않는다"
        );

        adapter
            .close_card(
                &r,
                &approval_card(&body, CardTone::Success, "✅ 보냈습니다."),
            )
            .await
            .unwrap();
        let closed = api.closed.lock().unwrap().clone();
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].1.to_string(), r);
        assert_eq!(closed[0].2, "✅ 보냈습니다.");
        let before: Vec<_> = sent
            .last()
            .unwrap()
            .2
            .iter()
            .map(|e| &e.description)
            .collect();
        let after: Vec<_> = closed[0].3.iter().map(|e| &e.description).collect();
        assert_eq!(before, after, "같은 조각으로 고친다");
        assert!(
            closed[0]
                .3
                .iter()
                .all(|e| e.color == Some(card::color(CardTone::Success)))
        );
    }

    /// 결정 0063: 여러 칸을 받는 버튼(`form`)은 Discord에 그리지 않는다(비밀번호가 Discord 서버를 거치면 안 된다).
    /// 다른 버튼(일반, 입력창 `prompt`)은 그대로 그린다.
    #[tokio::test]
    async fn form_buttons_are_never_drawn_on_discord() {
        use lucia_core::{ButtonForm, CardTone, FormField, FormFieldKind};
        let form_button = |action: &str| CardButton {
            action: action.into(),
            label: "로그인 정보 입력".into(),
            primary: true,
            prompt: None,
            form: Some(ButtonForm {
                title: "로그인 정보 입력".into(),
                fields: vec![FormField {
                    key: "password".into(),
                    label: "비밀번호".into(),
                    kind: FormFieldKind::Secret,
                    min_len: 1,
                    max_len: 200,
                }],
            }),
        };
        let plain_button = |action: &str| CardButton {
            action: action.into(),
            label: "취소".into(),
            primary: false,
            prompt: None,
            form: None,
        };
        let api = FakeApi::default();
        let adapter = DiscordAdapter::new(api.clone(), Arc::new(AtomicBool::new(true)));

        // 폼 버튼 하나와 일반 버튼 하나: 일반 버튼만 그려진다.
        let mut card = approval_card("본문", CardTone::Pending, "🔐 로그인이 필요합니다");
        card.buttons = vec![
            form_button("approval:r1:approve"),
            plain_button("approval:r1:deny"),
        ];
        adapter.send_card(card.clone()).await.unwrap();
        let sent = api.cards.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].3,
            ["approval:r1:deny"],
            "폼 버튼은 그리지 않고 나머지는 그린다"
        );
        assert!(
            adapter.prompts.lock().unwrap().is_empty(),
            "폼 버튼은 입력창 표에도 들어가지 않는다"
        );

        // 폼 버튼만 있는 카드도 보내진다(버튼이 없을 뿐). 본문은 그대로라 만든 쪽이 준 다른 길(링크)이 보인다.
        card.body = "버튼이 없거나 안 되면: http://192.168.0.2:8780/site-login/abc".into();
        card.buttons = vec![form_button("approval:r2:approve")];
        adapter.send_card(card).await.unwrap();
        let sent = api.cards.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert!(sent[1].3.is_empty(), "{:?}", sent[1].3);
        assert!(
            format!("{:?}", sent[1].2).contains("/site-login/abc"),
            "본문의 다른 길은 그대로 간다"
        );

        // 입력창 버튼은 여전히 그려지고 표에 등록된다(폼 버튼과 섞여도).
        let prompt_button = CardButton {
            action: "approval:r3:approve".into(),
            label: "코드 입력".into(),
            primary: true,
            prompt: Some(ButtonPrompt {
                title: "인증 코드 입력".into(),
                label: "인증 코드".into(),
                placeholder: None,
                min_len: 1,
                max_len: 12,
            }),
            form: None,
        };
        let mut card = approval_card("본문", CardTone::Pending, "🔐 코드가 필요합니다");
        card.buttons = vec![
            prompt_button.clone(),
            form_button("approval:r3:other"),
            plain_button("approval:r3:deny"),
        ];
        adapter.send_card(card).await.unwrap();
        let sent = api.cards.lock().unwrap().clone();
        assert_eq!(
            sent[2].3,
            ["approval:r3:approve", "approval:r3:deny"],
            "입력창 버튼과 일반 버튼은 그려진다"
        );
        assert!(
            adapter
                .prompts
                .lock()
                .unwrap()
                .contains_key(&button_id(&prompt_button))
        );
        assert!(!adapter.capabilities().forms, "Discord는 폼 능력이 없다");
    }

    #[tokio::test]
    async fn presses_are_acked_then_forwarded_only_from_dms() {
        let api = FakeApi::default();
        let (tx, mut rx) = mpsc::channel(4);
        let sink = PressSink {
            api: Arc::new(api.clone()),
            presses: tx,
            allow_guild: false,
            prompts: Arc::default(),
            commands: None,
        };
        let info = PressInfo {
            application_id: 1,
            interaction_id: 900,
            token: "tok".into(),
            user_id: Some(42),
            in_guild: false,
            channel_id: Some(77),
            message_id: Some(5001),
            custom_id: "approval:r1:approve".into(),
        };
        handle_press(&sink, info.clone()).await;
        let press = rx.recv().await.unwrap();
        assert_eq!(press.sender, "42");
        assert_eq!(press.conversation, conversation_for(77));
        assert_eq!(press.message_ref, "5001");
        assert_eq!(press.action, "approval:r1:approve");

        handle_press(
            &sink,
            PressInfo {
                interaction_id: 901,
                in_guild: true,
                ..info
            },
        )
        .await;
        assert!(rx.try_recv().is_err(), "서버에서 누른 것은 넘기지 않는다");
        assert_eq!(
            *api.acks.lock().unwrap(),
            vec![900, 901],
            "응답은 모두 한다"
        );
    }

    #[tokio::test]
    async fn notes_files_and_typing_reach_the_api() {
        let api = FakeApi::default();
        let adapter = DiscordAdapter::new(api.clone(), Arc::new(AtomicBool::new(true)));
        let conv = conversation_for(77);
        adapter
            .send(Outbound::Note {
                conversation: conv.clone(),
                body: "웹 검색: tokio\n\n메일 검색: from:x".into(),
            })
            .await
            .unwrap();
        assert_eq!(
            api.sent.lock().unwrap().last().unwrap().1,
            "-# 웹 검색: tokio\n-# 메일 검색: from:x"
        );
        let n = adapter
            .send(Outbound::Files {
                conversation: conv.clone(),
                body: "가".repeat(2500),
                files: vec![OutFile {
                    name: "a.md".into(),
                    bytes: b"# hi".to_vec(),
                    id: None,
                }],
            })
            .await
            .unwrap();
        assert_eq!(n, 2, "긴 본문은 나누고 파일은 마지막에");
        let uploads = api.uploads.lock().unwrap().clone();
        assert_eq!(uploads.len(), 1);
        assert_eq!(uploads[0].2, ["a.md"]);
        assert_eq!(uploads[0].1.chars().count(), 500);
        let too_many = adapter
            .send(Outbound::Files {
                conversation: conv.clone(),
                body: String::new(),
                files: (0..11)
                    .map(|i| OutFile {
                        name: format!("{i}.txt"),
                        bytes: vec![b'x'],
                        id: None,
                    })
                    .collect(),
            })
            .await;
        assert!(matches!(too_many, Err(ChannelError::File(_))));
        adapter.typing(&conv).await.unwrap();
        assert_eq!(*api.typing.lock().unwrap(), vec![77]);
    }

    #[test]
    fn attachment_only_messages_pass_the_filter() {
        let filter = InboundFilter::default();
        let m = MessageInfo {
            id: 10,
            author_id: 42,
            author_is_bot: false,
            guild_id: None,
            channel_id: 900,
            content: String::new(),
            attachments: vec![InboundAttachment {
                url: "https://cdn.discordapp.com/attachments/1/2/a.png".into(),
                filename: "a.png".into(),
                content_type: Some("image/png".into()),
                size: 10,
            }],
            ..Default::default()
        };
        let got = filter.accept(&m).expect("파일만 와도 받는다");
        assert_eq!(got.attachments.len(), 1);
        assert_eq!(subtext("  \n"), "");
    }

    fn guild_msg(id: u64, author: u64, content: &str) -> MessageInfo {
        MessageInfo {
            id,
            author_id: author,
            author_name: "주인".into(),
            guild_id: Some(5),
            channel_id: 300,
            content: content.into(),
            ..Default::default()
        }
    }

    /// 결정 0024: 다른 서버에서는 소유자가 부른 말(멘션, 머리말, 봇 메시지에 답장)만 받는다.
    #[tokio::test]
    async fn guild_calls_need_a_mention_prefix_or_reply_from_the_owner() {
        let filter = InboundFilter::default();
        filter.set_guild(true);
        filter.set_bot(9);
        assert!(
            filter.accept(&guild_msg(90, 42, "<@9> 안녕")).is_none(),
            "부르기를 켜지 않았다"
        );
        filter.set_calls(true);
        filter.set_owner(Some(42));
        assert!(filter.accept(&guild_msg(1, 42, "그냥 잡담")).is_none());
        assert!(
            filter.accept(&guild_msg(2, 77, "<@9> 안녕")).is_none(),
            "다른 사람은 여기서 거른다"
        );
        let mut m = guild_msg(3, 42, "<@9> 오늘 <@77> 일정 알려 줘");
        m.mentions = vec![(9, "lucia".into()), (77, "민수".into())];
        let got = filter.accept(&m).expect("멘션");
        assert_eq!(got.body, "오늘 @민수 일정 알려 줘");
        let venue = got.venue.clone().unwrap();
        assert!(venue.addressed);
        assert_eq!(
            (venue.space_id.as_str(), venue.room_id.as_str()),
            ("5", "300")
        );

        // 머리말: 없으면 맞지 않고, 정하면 맞는다. 빈 머리말은 받지 않는다.
        filter.set_prefix(Some(String::new()));
        assert!(filter.accept(&guild_msg(4, 42, "아무 말")).is_none());
        filter.set_prefix(Some("!루시아".into()));
        assert_eq!(
            filter
                .accept(&guild_msg(5, 42, "  !루시아 날씨"))
                .unwrap()
                .body,
            "날씨"
        );
        assert!(
            filter.accept(&guild_msg(6, 42, "!루시아야 날씨")).is_none(),
            "붙은 글자"
        );
        filter.set_prefix(Some("L!".into()));
        assert_eq!(
            filter.accept(&guild_msg(7, 42, "l!help")).unwrap().body,
            "help"
        );

        // 봇 메시지에 답장(멘션 없이)도 부른 것이다. 원문이 함께 온다.
        let mut r = guild_msg(8, 42, "이거 다시 설명해");
        r.referenced = Some(Box::new(MessageInfo {
            id: 100,
            author_id: 9,
            author_name: "Lucia".into(),
            content: "앞선 답".into(),
            ..Default::default()
        }));
        let got = filter.accept(&r).expect("봇에게 답장");
        let q = got.reply_to.unwrap();
        assert!(q.from_self);
        assert_eq!((q.id.as_str(), q.body.as_str()), ("100", "앞선 답"));

        // 다른 사람 메시지에 답장하며 멘션만 했다: 글이 없어도 넘긴다.
        let mut bare = guild_msg(10, 42, "<@9>");
        bare.mentions = vec![(9, "lucia".into())];
        bare.referenced = Some(Box::new(MessageInfo {
            id: 101,
            author_id: 77,
            author_name: "민수".into(),
            content: "이번 주 회의 언제?".into(),
            ..Default::default()
        }));
        let got = filter.accept(&bare).expect("원문을 보라는 부름");
        assert_eq!(got.body, "");
        assert!(!got.reply_to.as_ref().unwrap().from_self);

        // 서버 이름은 한 번 묻고 기억한다.
        let mut venue = got.venue.unwrap();
        filter.fill_names(&FakeApi::default(), &mut venue).await;
        assert_eq!(venue.space_name.as_deref(), Some("서버5"));
        assert_eq!(venue.room_name.as_deref(), Some("채널300"));

        // 시스템 메시지(스레드 시작 알림 등)는 봇 메시지를 가리켜도 부름이 아니다.
        let mut sys = r.clone();
        sys.id = 13;
        sys.system = true;
        assert!(filter.accept(&sys).is_none());

        // 꺼져 있으면 서버 말은 받지 않는다. DM은 늘 받고 답장 원문도 붙는다.
        filter.set_guild(false);
        assert!(filter.accept(&guild_msg(11, 42, "<@9> 안녕")).is_none());
        let mut dm = guild_msg(12, 42, "이거 뭐였지");
        dm.guild_id = None;
        dm.referenced = r.referenced.clone();
        let got = filter.accept(&dm).unwrap();
        assert!(got.venue.is_none() && got.reply_to.is_some());
    }

    #[test]
    fn intents_follow_the_switches() {
        assert_eq!(intents_for(false, true), Intents::DIRECT_MESSAGES);
        assert!(!intents_for(true, false).contains(Intents::MESSAGE_CONTENT));
        assert!(intents_for(true, false).contains(Intents::GUILD_MESSAGES));
        assert!(intents_for(true, true).contains(Intents::MESSAGE_CONTENT));
    }

    /// 슬래시 명령(결정 0040): 등록하는 명령은 Discord 규칙을 지키고, 받는 쪽의 답으로 응답한다. 받을 곳이 없거나
    /// 늦으면 본인에게만 그렇다고 답한다.
    #[tokio::test]
    async fn slash_commands_are_valid_forwarded_and_answered() {
        for c in slash_commands() {
            twilight_validate::command::command(&c).expect("명령 모양");
            twilight_validate::command::chat_input_name(&c.name).expect("한글 이름");
        }
        let api = FakeApi::default();
        let (tx, _rx) = mpsc::channel(4);
        let (ctx, mut crx) = mpsc::channel(4);
        let sink = PressSink {
            api: Arc::new(api.clone()),
            presses: tx,
            allow_guild: false,
            prompts: Arc::default(),
            commands: Some(ctx),
        };
        let info = PressInfo {
            application_id: 1,
            interaction_id: 80,
            token: "t".into(),
            user_id: Some(42),
            in_guild: false,
            channel_id: Some(900),
            message_id: None,
            custom_id: String::new(),
        };
        let answer = tokio::spawn(async move {
            let cmd = crx.recv().await.unwrap();
            assert_eq!(
                (
                    cmd.name.as_str(),
                    cmd.option.as_deref(),
                    cmd.sender.as_str(),
                    cmd.in_dm
                ),
                ("권한", Some("open"), "42", true)
            );
            let _ = cmd.reply.send(crate::CommandReply {
                text: "🔓 해제".into(),
                private: false,
            });
            crx
        });
        handle_command(&sink, info.clone(), "권한".into(), Some("open".into())).await;
        let mut crx = answer.await.unwrap();
        assert_eq!(
            api.replies.lock().unwrap().clone(),
            [(80, "🔓 해제".to_owned(), false)]
        );
        // 받는 쪽이 답하지 않으면 시간 초과로 본인에게만.
        let late = tokio::spawn(async move {
            let cmd = crx.recv().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            drop(cmd);
        });
        handle_command(
            &sink,
            PressInfo {
                interaction_id: 81,
                ..info
            },
            "권한".into(),
            None,
        )
        .await;
        let r = api.replies.lock().unwrap().last().cloned().unwrap();
        assert!(r.0 == 81 && r.2 && r.1.contains("시간 초과"), "{r:?}");
        late.abort();
    }
}
