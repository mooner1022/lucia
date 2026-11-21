//! Discord 연결 관리: 토큰 검증·저장, 게이트웨이 시작·중지, 소유자 설정, 연결 상태.
//!
//! 엔진에는 [`HubChannel`]을 준다. 지금 연결된 Discord 어댑터로 보내고, 연결이 없으면 `ready() == false`라서
//! 엔진이 모델을 부르기 전에 멈추고 알림을 기록에만 남긴다.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context as _, bail};
use chrono::Utc;
use lucia_channel::discord::server::Server;
use lucia_channel::discord::{
    BotUser, DiscordAdapter, DiscordApi, GatewayExit, InboundFilter, PressSink, TwilightApi,
    conversation_for, run_gateway,
};
use lucia_channel::{ChannelAdapter, ChannelError};
use lucia_core::{ButtonPress, Capabilities, Card, ConversationId, Inbound, Outbound};
use lucia_store::{Store, keys};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::credentials::{Credentials, names};

/// 상태 화면에 보이는 Discord 상태.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum DiscordState {
    /// 토큰이 없다.
    NotConfigured,
    /// 게이트웨이에 붙는 중.
    Connecting {
        bot: String,
    },
    Connected {
        bot: String,
    },
    /// 멈췄다(토큰 오류 등). 사람이 토큰을 고쳐야 한다.
    Stopped {
        reason: String,
    },
}

struct Running {
    adapter: Arc<DiscordAdapter<TwilightApi>>,
    bot: BotUser,
    cancel: CancellationToken,
    /// 토큰을 바꿀 때마다 늘어난다. 끝난 게이트웨이가 새 연결을 지우지 않게 구분한다.
    generation: u64,
}

pub struct DiscordControl {
    store: Arc<Mutex<Store>>,
    creds: Arc<Credentials>,
    inbound: mpsc::Sender<Inbound>,
    /// 승인 버튼 누름을 받는 곳(승인 데스크).
    presses: mpsc::Sender<ButtonPress>,
    /// 슬래시 명령을 받는 곳(결정 0040, 승인 데스크). 없으면 등록하지 않는다.
    commands: Mutex<Option<mpsc::Sender<lucia_channel::SlashCommand>>>,
    filter: Arc<InboundFilter>,
    connected: Arc<AtomicBool>,
    running: RwLock<Option<Running>>,
    stopped: Mutex<Option<String>>,
    generation: std::sync::atomic::AtomicU64,
    /// Discord가 메시지 내용 intent를 거절했다(4014). 관리 페이지에서 다시 저장할 때까지 그 intent 없이
    /// 붙는다(서버에서는 멘션한 말만 내용이 온다, 결정 0024).
    content_refused: AtomicBool,
    /// 봇이 들어간 서버 목록(잠깐 기억한다).
    servers: Mutex<Option<(std::time::Instant, Vec<Server>)>>,
}

impl DiscordControl {
    pub fn new(
        store: Arc<Mutex<Store>>,
        creds: Arc<Credentials>,
        inbound: mpsc::Sender<Inbound>,
        presses: mpsc::Sender<ButtonPress>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            creds,
            inbound,
            presses,
            commands: Mutex::new(None),
            filter: Arc::default(),
            connected: Arc::new(AtomicBool::new(false)),
            running: RwLock::new(None),
            stopped: Mutex::new(None),
            generation: std::sync::atomic::AtomicU64::new(0),
            content_refused: AtomicBool::new(false),
            servers: Mutex::new(None),
        })
    }

    /// 슬래시 명령을 받을 곳을 붙인다(결정 0040).
    pub fn with_commands(
        self: Arc<Self>,
        tx: mpsc::Sender<lucia_channel::SlashCommand>,
    ) -> Arc<Self> {
        *self.commands.lock().expect("commands lock") = Some(tx);
        self
    }

    /// 작업 채널이나 서버 설정을 다시 저장했다: 메시지 내용 intent를 다시 요청해 본다(결정 0014·0024).
    pub fn retry_guild(&self) {
        self.content_refused.store(false, Ordering::SeqCst);
    }

    /// Discord가 메시지 내용 intent를 거절해 그것 없이 붙어 있는가(관리 페이지 안내용).
    pub fn content_refused(&self) -> bool {
        self.content_refused.load(Ordering::SeqCst)
    }

    /// 서버 메시지를 받는가(작업 채널을 정했거나 다른 서버에서 부르기를 켰다).
    pub fn guild_wanted(&self) -> bool {
        let store = self.store.lock().expect("store lock");
        let set = |k: &str| {
            store
                .setting(k)
                .ok()
                .flatten()
                .is_some_and(|v| !v.trim().is_empty())
        };
        set(crate::tasks::CHANNEL_KEY) || guild_calls_on(&store)
    }

    /// 저장소의 설정(소유자, 머리말, 듣는 채널)을 수신 거름에 다시 싣는다. 게이트웨이를 다시 붙이지 않는다.
    pub fn apply_settings(&self) {
        let (owner, prefix, calls) = {
            let store = self.store.lock().expect("store lock");
            (
                store
                    .setting(keys::OWNER_DISCORD_USER_ID)
                    .ok()
                    .flatten()
                    .and_then(|o| o.parse().ok()),
                store.setting(keys::DISCORD_PREFIX).ok().flatten(),
                guild_calls_on(&store),
            )
        };
        self.filter.set_owner(owner);
        // 다른 서버에서 부르기를 껐으면(작업 스레드만) 머리말·멘션으로 부르는 것은 받지 않는다: 듣는 채널만.
        self.filter.set_calls(calls);
        self.filter.set_prefix(prefix);
        let store = self.store.clone();
        self.filter.set_listen(Some(Arc::new(move |channel: u64| {
            let conv = conversation_for(channel).0;
            let Ok(store) = store.lock() else {
                return false;
            };
            store
                .setting(crate::tasks::CHANNEL_KEY)
                .ok()
                .flatten()
                .is_some_and(|c| c == conv)
                || store.is_task_thread(&conv).unwrap_or(false)
        })));
    }

    pub fn state(&self) -> DiscordState {
        if let Some(r) = self.running.read().expect("running lock").as_ref() {
            let bot = r.bot.name.clone();
            return if self.connected.load(Ordering::SeqCst) {
                DiscordState::Connected { bot }
            } else {
                DiscordState::Connecting { bot }
            };
        }
        match self.stopped.lock().expect("stopped lock").clone() {
            Some(reason) => DiscordState::Stopped { reason },
            None => DiscordState::NotConfigured,
        }
    }

    /// 저장된 토큰이 있으면 시작한다(데몬 시작 때). 토큰이 거부되면 멈추고, 네트워크 오류면
    /// 백오프하며 다시 시도한다(부팅 직후 네트워크가 늦게 올라와도 혼자 회복하게).
    pub async fn start_from_credentials(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(token) = self.creds.read(names::DISCORD_BOT_TOKEN).await? else {
            return Ok(());
        };
        match TwilightApi::new(token.clone()).current_user().await {
            Ok(bot) => {
                self.start(token, bot);
                Ok(())
            }
            Err(ChannelError::Unauthorized(why)) => {
                let text = format!(
                    "저장된 Discord 토큰을 Discord가 거부했습니다(관리 페이지에서 다시 넣으세요): {why}"
                );
                *self.stopped.lock().expect("stopped lock") = Some(text.clone());
                anyhow::bail!(text)
            }
            Err(err) => {
                tracing::warn!(%err, "Discord에 닿지 않는다. 백오프하며 다시 시도한다");
                *self.stopped.lock().expect("stopped lock") =
                    Some(format!("Discord에 연결하지 못해 다시 시도하는 중: {err}"));
                let this = self.clone();
                tokio::spawn(async move { this.retry_start(token).await });
                Ok(())
            }
        }
    }

    async fn retry_start(self: Arc<Self>, token: String) {
        let mut delay = std::time::Duration::from_secs(30);
        loop {
            tokio::time::sleep(delay).await;
            // 그사이 관리 페이지에서 토큰을 바꿨거나 지웠으면 그만둔다.
            if self.running.read().expect("running lock").is_some()
                || self
                    .creds
                    .read(names::DISCORD_BOT_TOKEN)
                    .await
                    .ok()
                    .flatten()
                    .as_deref()
                    != Some(token.as_str())
            {
                return;
            }
            match TwilightApi::new(token.clone()).current_user().await {
                Ok(bot) => {
                    tracing::info!(bot = %bot.name, "Discord 연결 회복");
                    self.start(token, bot);
                    return;
                }
                Err(ChannelError::Unauthorized(why)) => {
                    *self.stopped.lock().expect("stopped lock") = Some(format!(
                        "저장된 Discord 토큰을 Discord가 거부했습니다: {why}"
                    ));
                    return;
                }
                Err(err) => {
                    tracing::warn!(%err, ?delay, "Discord 재시도 실패");
                    delay = (delay * 2).min(std::time::Duration::from_secs(600));
                }
            }
        }
    }

    /// 관리 페이지에서 받은 토큰: 검증하고, 저장하고, 게이트웨이를 다시 시작한다.
    pub async fn set_token(self: &Arc<Self>, token: &str) -> anyhow::Result<BotUser> {
        let token = token.trim().to_owned();
        anyhow::ensure!(!token.is_empty(), "토큰이 비어 있다");
        let bot = match TwilightApi::new(token.clone()).current_user().await {
            Ok(bot) => bot,
            Err(ChannelError::Unauthorized(_)) => {
                anyhow::bail!(
                    "Discord가 이 토큰을 거부했다(Bot 메뉴의 Reset Token으로 받은 값인지 확인)"
                )
            }
            Err(err) => anyhow::bail!("Discord에 연결할 수 없다(네트워크): {err}"),
        };
        self.creds
            .write(names::DISCORD_BOT_TOKEN, &token)
            .await
            .context("토큰 저장")?;
        self.start(token, bot.clone());
        Ok(bot)
    }

    pub async fn remove_token(&self) -> anyhow::Result<()> {
        self.stop();
        *self.stopped.lock().expect("stopped lock") = None;
        self.creds.remove(names::DISCORD_BOT_TOKEN).await
    }

    fn stop(&self) {
        if let Some(old) = self.running.write().expect("running lock").take() {
            old.cancel.cancel();
        }
        self.connected.store(false, Ordering::SeqCst);
    }

    /// 이전 게이트웨이를 닫고 새로 붙는다. 치명적으로 끝나면 다시 붙지 않고 멈춘 상태로 둔다.
    fn start(self: &Arc<Self>, token: String, bot: BotUser) {
        tracing::info!(bot = %bot.name, "Discord 게이트웨이 시작");
        let (token_for_retry, bot_for_retry) = (token.clone(), bot.clone());
        self.stop();
        *self.stopped.lock().expect("stopped lock") = None;
        let cancel = CancellationToken::new();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let adapter = Arc::new(DiscordAdapter::new(
            TwilightApi::new(token.clone()),
            self.connected.clone(),
        ));
        // 작업 채널을 정했거나 다른 서버에서 부르기를 켰으면 서버 채널의 말과 버튼도 받는다(결정 0014·0024).
        let guild = self.guild_wanted();
        let content = guild && !self.content_refused.load(Ordering::SeqCst);
        self.filter.set_guild(guild);
        self.filter.set_content(content);
        self.filter.set_bot(bot.id);
        self.apply_settings();
        let api = adapter.api_handle();
        let sink = PressSink {
            api: adapter.api_handle(),
            presses: self.presses.clone(),
            allow_guild: guild,
            prompts: adapter.prompts_handle(),
            commands: self.commands.lock().expect("commands lock").clone(),
        };
        *self.running.write().expect("running lock") = Some(Running {
            adapter,
            bot,
            cancel: cancel.clone(),
            generation,
        });
        let this = self.clone();
        tokio::spawn(async move {
            let exit = run_gateway(
                token,
                api,
                this.inbound.clone(),
                Some(sink),
                this.filter.clone(),
                this.connected.clone(),
                cancel.clone(),
            )
            .await;
            if let GatewayExit::Fatal(reason) = exit {
                {
                    // 이미 새 게이트웨이로 바뀌었으면(토큰 교체) 그 연결은 그대로 둔다.
                    let mut running = this.running.write().expect("running lock");
                    if running.as_ref().is_some_and(|r| r.generation == generation) {
                        *running = None;
                    } else {
                        return;
                    }
                }
                // 메시지 내용 intent를 Discord가 거절했으면(4014) 그것만 빼고 다시 붙고 알린다. 서버에서는
                // 멘션한 말만 내용이 온다(작업 스레드의 보통 말, 머리말, 다른 서버 기록 읽기는 안 된다).
                if content && reason.contains("코드 4014") {
                    this.content_refused.store(true, Ordering::SeqCst);
                    let text = "Discord가 메시지 내용 intent를 거절해(4014) 그것 없이 다시 붙었습니다. 서버에서는 Lucia를 멘션한 말만 받고, 작업 스레드의 보통 말·머리말로 부르기·다른 서버 대화 읽기는 되지 않습니다. 쓰려면 Discord 개발자 포털(봇 → Privileged Gateway Intents)에서 Message Content Intent를 켠 뒤, 관리 페이지 \"Discord 서버\"나 \"작업 스레드\"에서 다시 저장하세요.";
                    if let Ok(store) = this.store.lock() {
                        let _ = store.add_alert(text, false, Utc::now());
                    }
                    tracing::warn!("{text}");
                    this.start(token_for_retry, bot_for_retry);
                    return;
                }
                let hint = "";
                let text = crate::mask::global().mask(&format!(
                    "Discord 연결이 끊겨 다시 붙지 않습니다(토큰을 확인하세요): {reason}{hint}"
                ));
                tracing::error!(alert = %text);
                *this.stopped.lock().expect("stopped lock") = Some(text.clone());
                if let Ok(store) = this.store.lock() {
                    let _ = store.add_alert(&text, false, Utc::now());
                }
            }
        });
    }

    /// 봇 자신의 사용자 ID(연결됐을 때).
    pub fn bot_id(&self) -> Option<u64> {
        self.running
            .read()
            .expect("running lock")
            .as_ref()
            .map(|r| r.bot.id)
    }

    /// 서버 조회·관리(결정 0024)에 쓸 REST 클라이언트. 연결되지 않았으면 `None`.
    pub fn rest(&self) -> Option<Arc<DiscordAdapter<TwilightApi>>> {
        self.adapter()
    }

    /// 봇이 들어간 서버. `fresh`가 아니면 5분 동안 기억한 목록을 쓴다(관리 동작의 권한 확인은 늘 새로 묻는다).
    pub async fn servers(&self, fresh: bool) -> Result<Vec<Server>, String> {
        const KEEP: std::time::Duration = std::time::Duration::from_secs(300);
        if !fresh
            && let Some((at, list)) = self.servers.lock().expect("servers lock").as_ref()
            && at.elapsed() < KEEP
        {
            return Ok(list.clone());
        }
        let Some(rest) = self.adapter() else {
            return Err("Discord가 연결되어 있지 않다".into());
        };
        let list = lucia_channel::discord::server::servers(rest.api().client())
            .await
            .map_err(|e| e.to_string())?;
        *self.servers.lock().expect("servers lock") =
            Some((std::time::Instant::now(), list.clone()));
        Ok(list)
    }

    fn adapter(&self) -> Option<Arc<DiscordAdapter<TwilightApi>>> {
        self.running
            .read()
            .expect("running lock")
            .as_ref()
            .map(|r| r.adapter.clone())
    }

    /// 소유자 설정: DM을 열고 확인 메시지를 보낸 뒤, 보내기에 성공했을 때만 소유자와 알림 대상을 저장한다
    /// (저장해 놓고 보낼 수 없으면 다음 브리핑이 토큰만 쓰고 전송에 실패한다).
    pub async fn set_owner(&self, user_id: &str) -> anyhow::Result<()> {
        let id: u64 = user_id
            .trim()
            .parse()
            .ok()
            .filter(|n| *n > 0)
            .context("Discord 사용자 ID는 숫자다(개발자 모드 → 프로필 → ID 복사)")?;
        let Some(adapter) = self.adapter() else {
            bail!("먼저 봇 토큰을 넣어 Discord를 연결하세요");
        };
        let channel =
            adapter.api().open_dm(id).await.map_err(|e| {
                anyhow::anyhow!("DM을 열 수 없다(봇과 같은 서버에 있는지 확인): {e}")
            })?;
        let conversation = conversation_for(channel);
        adapter
            .send(Outbound::Text {
                conversation: conversation.clone(),
                body: "Lucia가 연결되었습니다. 이 대화로 브리핑과 알림을 보냅니다.".into(),
            })
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "확인 메시지를 보내지 못해 저장하지 않았다(봇과 같은 서버에 있는지 확인): {e}"
                )
            })?;
        let store = self.store.lock().expect("store lock");
        let now = Utc::now();
        store.set_setting(keys::OWNER_DISCORD_USER_ID, &id.to_string(), now)?;
        store.set_setting(keys::NOTIFY_CONVERSATION, &conversation.0, now)?;
        drop(store);
        self.apply_settings();
        Ok(())
    }

    pub fn clear_owner(&self) -> anyhow::Result<()> {
        let store = self.store.lock().expect("store lock");
        store.delete_setting(keys::OWNER_DISCORD_USER_ID)?;
        store.delete_setting(keys::NOTIFY_CONVERSATION)?;
        drop(store);
        self.apply_settings();
        Ok(())
    }
}

/// 다른 서버에서 부르기를 켰는가(결정 0024).
pub fn guild_calls_on(store: &Store) -> bool {
    store
        .setting(keys::DISCORD_GUILD_CALLS)
        .ok()
        .flatten()
        .is_some_and(|v| v == "on")
}

/// 엔진이 쓰는 채널: 지금 연결된 Discord로 보낸다.
pub struct HubChannel {
    control: Arc<DiscordControl>,
}

impl HubChannel {
    pub fn new(control: Arc<DiscordControl>) -> Self {
        Self { control }
    }
}

impl ChannelAdapter for HubChannel {
    fn platform(&self) -> &str {
        lucia_channel::discord::PLATFORM
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            max_message_chars: lucia_channel::discord::MAX_MESSAGE_CHARS,
            buttons: true,
            edit: true,
            // 작업 스레드(결정 0014): 작업 채널을 정했을 때만 엔진이 쓴다.
            threads: true,
            // 값을 받는 버튼(결정 0017).
            prompts: true,
            forms: false,
        }
    }

    async fn send(&self, message: Outbound) -> Result<usize, ChannelError> {
        match self.control.adapter() {
            Some(adapter) => adapter.send(message).await,
            None => Err(ChannelError::Closed),
        }
    }

    async fn send_card(&self, card: Card) -> Result<String, ChannelError> {
        match self.control.adapter() {
            Some(adapter) => adapter.send_card(card).await,
            None => Err(ChannelError::Closed),
        }
    }

    async fn typing(&self, conversation: &ConversationId) -> Result<(), ChannelError> {
        match self.control.adapter() {
            Some(adapter) => adapter.typing(conversation).await,
            None => Err(ChannelError::Closed),
        }
    }

    async fn close_card(&self, message_ref: &str, card: &Card) -> Result<(), ChannelError> {
        match self.control.adapter() {
            Some(adapter) => adapter.close_card(message_ref, card).await,
            None => Err(ChannelError::Closed),
        }
    }

    async fn send_sticker(
        &self,
        conversation: &ConversationId,
        sticker_id: &str,
    ) -> Result<(), ChannelError> {
        match self.control.adapter() {
            Some(adapter) => adapter.send_sticker(conversation, sticker_id).await,
            None => Err(ChannelError::Closed),
        }
    }

    async fn open_thread(
        &self,
        parent: &ConversationId,
        name: &str,
    ) -> Result<ConversationId, ChannelError> {
        match self.control.adapter() {
            Some(adapter) => adapter.open_thread(parent, name).await,
            None => Err(ChannelError::Closed),
        }
    }

    fn ready(&self) -> bool {
        self.control.adapter().is_some() && self.control.connected.load(Ordering::SeqCst)
    }
}
