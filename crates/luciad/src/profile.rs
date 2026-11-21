//! Lucia의 프로필 이미지(결정 0060): Lucia가 자기 프로필 그림을 직접 보고 정한다.
//!
//! - **그림 하나를 데몬이 보관한다.** 파일은 `<data_dir>/profile/<id>`에, 지금 쓰는 것은 저장소 설정([`KEY`])에 둔다.
//!   id는 내용 해시라 앱이 영구 캐시한다(그림이 바뀌면 id가 바뀐다). 예전 그림도 한동안 내준다.
//! - **고치지 않고 검사만 한다**(이모지·스티커와 같다, 결정 0045): 정적인 PNG·JPEG, 정사각형, 한 변
//!   [`MIN_SIDE`]~[`MAX_SIDE`]px, [`MAX_BYTES`]까지. 맞지 않으면 어긴 것을 **모두** 한 번에 알려 준다(자르고 줄이는
//!   것은 Lucia가 쉘이나 `code_run`에서 한다). 크기 상한은 Lucia가 도구로 직접 볼 수 있는 그림 크기와 같다.
//! - **어디에 쓰이나**: 전용 앱(접속 API의 `profile`, 바뀌면 연결된 앱에 알린다)과 Discord 봇의 아바타
//!   (연결되어 있을 때, 도구가 따로 적용한다).
//! - **이름(결정 0064)**: 관리 화면과 앱에 보이는 이름. 저장소 설정([`NAME_KEY`])에 두고, 없으면 [`DEFAULT_NAME`]이다.
//!   Lucia가 자기를 부르는 이름과 말투는 캐릭터 파일의 일이고, 여기와 따로다.
//! - **누가 정하나**: 그림은 Lucia(도구)나 소유자(관리 페이지, 결정 0064)가 정한다. 소유자는 `owner`로 남는다.

use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use lucia_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

/// 지금 쓰는 프로필 그림(설정 값은 [`Avatar`]의 JSON).
pub const KEY: &str = "profile.avatar";
/// Discord에 적용한 상태(설정 값은 [`DiscordState`]의 JSON).
pub const DISCORD_KEY: &str = "profile.discord";
/// 관리 화면과 앱에 보이는 이름(결정 0064). 없으면 [`DEFAULT_NAME`].
pub const NAME_KEY: &str = "profile.name";
/// 이름을 정하지 않았을 때의 이름.
pub const DEFAULT_NAME: &str = "Lucia";
/// 이름의 글자 수 상한(유니코드 글자 단위) [제안].
pub const NAME_MAX_CHARS: usize = 24;
pub const MIN_SIDE: u32 = 128;
pub const MAX_SIDE: u32 = 2048;
/// 파일 크기 상한. Lucia가 도구 결과로 직접 볼 수 있는 크기([`crate::fileview::MAX_INLINE_IMAGE`])와 같다.
pub const MAX_BYTES: u64 = crate::fileview::MAX_INLINE_IMAGE;
/// 예전 그림을 이만큼 남겨 둔다(앱이 캐시한 id로 다시 청할 수 있다).
const KEEP_FILES: usize = 20;

/// 보이는 이름: 정한 이름, 없으면 [`DEFAULT_NAME`].
pub fn display_name(store: &Store) -> String {
    store
        .setting(NAME_KEY)
        .ok()
        .flatten()
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| DEFAULT_NAME.to_owned())
}

/// 이름 입력을 검사한다: 앞뒤 공백을 자르고, 1~[`NAME_MAX_CHARS`]자에 제어 문자(줄바꿈 포함)가 없어야 한다.
/// 빈 입력은 `Ok(None)`(기본 이름으로 되돌린다).
pub fn validate_name(input: &str) -> Result<Option<String>, String> {
    let name = input.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().any(char::is_control) {
        return Err("이름에는 줄바꿈이나 제어 문자를 넣을 수 없습니다.".into());
    }
    if name.chars().count() > NAME_MAX_CHARS {
        return Err(format!("이름은 {NAME_MAX_CHARS}자까지입니다."));
    }
    Ok(Some(name.to_owned()))
}

/// 이름을 저장소에 적는다(빈 입력이면 정한 이름을 지워 기본 이름으로). 바뀐 뒤의 보이는 이름을 돌려준다.
pub fn save_name(store: &Store, input: &str, now: DateTime<Utc>) -> Result<String, String> {
    match validate_name(input)? {
        Some(name) => {
            store
                .set_setting(NAME_KEY, &name, now)
                .map_err(|e| format!("저장: {e}"))?;
            Ok(name)
        }
        None => {
            store
                .delete_setting(NAME_KEY)
                .map_err(|e| format!("저장: {e}"))?;
            Ok(DEFAULT_NAME.to_owned())
        }
    }
}

/// 저장소에 정해 둔 프로필 그림(없으면 `None`).
pub fn stored_avatar(store: &Store) -> Option<Avatar> {
    serde_json::from_str(&store.setting(KEY).ok()??).ok()
}

/// 프로필 그림 하나.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Avatar {
    /// 내용 해시로 만든 id(`a_…`).
    pub id: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub size: u64,
    pub set_at: DateTime<Utc>,
    /// 누가 정했나(`lucia`: 도구, `owner`: 관리 페이지, `user`: 명령).
    pub by: String,
}

/// Discord 봇 아바타에 무엇을 했는지.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordState {
    /// 데몬이 마지막으로 Discord에 올린 그림의 id. 없으면 데몬은 Discord 아바타를 건드린 적이 없다.
    #[serde(default)]
    pub applied: Option<String>,
    /// 바꾸기 전에 받아 둔 원래 아바타의 파일 id(보관함). 원래 아바타가 없었으면 빈 글.
    #[serde(default)]
    pub original: Option<String>,
    /// 데몬이 마지막으로 바꾼 뒤 Discord가 알려 준 아바타의 표지(주소). 다음에 바꿀 때 이것과 다르면 그 사이에
    /// 사용자가 개발자 포털에서 바꾼 것이므로 그 그림을 다시 보관한다.
    #[serde(default)]
    pub seen: Option<String>,
}

/// 그림에서 읽은 것: (MIME, 너비, 높이).
pub type Shape = (&'static str, u32, u32);

fn be16(b: &[u8], at: usize) -> Option<u32> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?) as u32)
}

/// JPEG의 크기: 프레임 머리(SOF)를 찾아 읽는다.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    loop {
        // 조각은 0xFF로 시작한다(채움 바이트 0xFF가 더 붙을 수 있다).
        while *bytes.get(at)? != 0xFF {
            at += 1;
        }
        while *bytes.get(at)? == 0xFF {
            at += 1;
        }
        let marker = *bytes.get(at)?;
        at += 1;
        match marker {
            // 길이가 없는 조각.
            0x01 | 0xD0..=0xD9 => continue,
            // 프레임 머리(허프만 표 C4, JPG C8, 산술 부호 조건 CC는 아니다).
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                let (height, width) = (be16(bytes, at + 3)?, be16(bytes, at + 5)?);
                return Some((width, height));
            }
            _ => at += be16(bytes, at)? as usize,
        }
    }
}

/// 파일 앞부분으로 형식과 크기를 읽는다. 확장자와 MIME은 보지 않는다.
pub fn inspect(bytes: &[u8]) -> Result<Shape, String> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        let (width, height) = jpeg_size(bytes)
            .filter(|(w, h)| *w > 0 && *h > 0)
            .ok_or("JPEG 파일이 깨졌거나 끝까지 오지 않았다")?;
        return Ok(("image/jpeg", width, height));
    }
    match crate::expressions::inspect(bytes) {
        Ok(image) if image.mime == "image/png" => Ok(("image/png", image.width, image.height)),
        Ok(image) => Err(format!(
            "받지 않는 형식이다({}{}). 움직이지 않는 PNG나 JPEG로 바꾼다",
            image.mime,
            if image.animated {
                ", 움직이는 그림"
            } else {
                ""
            }
        )),
        Err(_) => Err("그림 파일이 아니거나 받지 않는 형식이다. PNG나 JPEG로 바꾼다".into()),
    }
}

/// 크기와 모양이 규칙을 어긴 것들(없으면 빈 목록).
fn broken_rules(width: u32, height: u32, len: usize) -> Vec<String> {
    let mut wrong = Vec::new();
    if width != height {
        wrong.push(format!(
            "정사각형이 아니다({width}×{height}): 가운데나 얼굴 쪽을 정사각형으로 자른다"
        ));
    }
    let (short, long) = (width.min(height), width.max(height));
    if short < MIN_SIDE {
        wrong.push(format!(
            "너무 작다({width}×{height}): 한 변이 {MIN_SIDE}px 이상이어야 한다"
        ));
    }
    if long > MAX_SIDE {
        wrong.push(format!(
            "너무 크다({width}×{height}): 한 변을 {MAX_SIDE}px 이하로 줄인다(512~1024px이면 충분하다)"
        ));
    }
    if len as u64 > MAX_BYTES {
        wrong.push(format!(
            "파일이 크다({}): {}까지",
            crate::files::human_size(len as u64),
            crate::files::human_size(MAX_BYTES)
        ));
    }
    wrong
}

/// 규칙을 한 줄로(관리 화면의 안내 글).
pub fn rules_text() -> String {
    format!(
        "움직이지 않는 PNG·JPEG, 정사각형, 한 변 {MIN_SIDE}~{MAX_SIDE}px, {}까지",
        crate::files::human_size(MAX_BYTES)
    )
}

/// 프로필 그림의 규칙을 본다. 어긴 것을 모두 모아 한 번에 알려 준다.
pub fn check(bytes: &[u8]) -> Result<Shape, String> {
    if bytes.is_empty() {
        return Err("빈 파일이다".into());
    }
    let (mime, width, height) = inspect(bytes)?;
    let wrong = broken_rules(width, height, bytes.len());
    if wrong.is_empty() {
        return Ok((mime, width, height));
    }
    Err(format!(
        "프로필 그림 규칙에 맞지 않는다: {}. 쉘이나 code_run에서 pillow로 자르고 줄인 사본을 만들어(file_import나 lucia.save_file) 그 파일로 다시 부른다. 규칙: {}.",
        wrong.join(" / "),
        rules_text()
    ))
}

/// 소유자가 관리 페이지에서 올린 그림을 본다(결정 0064). [`check`]와 같은 규칙이지만 안내가 사람(소유자)에게 한다:
/// 어긴 것을 모두 알리고, Lucia에게 하라는 말(pillow, `code_run`)은 뺀다.
pub fn check_for_owner(bytes: &[u8]) -> Result<Shape, String> {
    if bytes.is_empty() {
        return Err("빈 파일입니다.".into());
    }
    let (mime, width, height) = inspect(bytes).map_err(|e| format!("{e}."))?;
    let wrong = broken_rules(width, height, bytes.len());
    if wrong.is_empty() {
        return Ok((mime, width, height));
    }
    Err(format!(
        "프로필 그림 규칙에 맞지 않습니다: {}. 규칙: {}.",
        wrong.join(" / "),
        rules_text()
    ))
}

fn valid_id(id: &str) -> bool {
    id.strip_prefix("a_")
        .is_some_and(|hex| hex.len() == 24 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

pub struct Profile {
    store: Arc<Mutex<Store>>,
    dir: PathBuf,
    changes: tokio::sync::watch::Sender<Option<Avatar>>,
    /// 이름이 바뀔 때마다 새 이름을 보낸다(결정 0064). 이름 자체는 저장소가 가진다.
    names: tokio::sync::watch::Sender<String>,
}

impl Profile {
    pub fn new(store: Arc<Mutex<Store>>, dir: PathBuf) -> Arc<Self> {
        let (now, name) = {
            let guard = store.lock().expect("store lock");
            (stored_avatar(&guard), display_name(&guard))
        };
        let (changes, _) = tokio::sync::watch::channel(now);
        let (names, _) = tokio::sync::watch::channel(name);
        Arc::new(Self {
            store,
            dir,
            changes,
            names,
        })
    }

    /// 지금 프로필 그림. 정한 적이 없으면 `None`.
    pub fn avatar(&self) -> Option<Avatar> {
        self.changes.borrow().clone()
    }

    /// 바뀔 때마다 새 값을 받는다(접속 API가 연결된 앱에 알린다).
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<Option<Avatar>> {
        self.changes.subscribe()
    }

    /// 지금 보이는 이름(결정 0064). 저장소에서 읽으므로 늘 맞다.
    pub fn name(&self) -> String {
        display_name(&self.store.lock().expect("store lock"))
    }

    /// 이름이 바뀔 때마다 새 이름을 받는다(접속 API가 연결된 앱에 알린다).
    pub fn subscribe_name(&self) -> tokio::sync::watch::Receiver<String> {
        self.names.subscribe()
    }

    /// 이름을 정한다(빈 입력이면 기본 이름으로). 규칙에 맞지 않으면 까닭.
    pub fn set_name(&self, input: &str, now: DateTime<Utc>) -> Result<String, String> {
        // 저장소 잠금은 쓰고 바로 놓는다(알림을 받는 쪽이 이름을 읽으려고 같은 잠금을 잡는다).
        let name = save_name(&self.store.lock().expect("store lock"), input, now)?;
        self.names.send_replace(name.clone());
        Ok(name)
    }

    /// 앱에 주는 지금 모양(`profile`): 이름과 그림. 그림이 없으면 `avatar`가 `null`이다.
    pub fn view(&self) -> Value {
        Self::json(&self.name(), self.avatar().as_ref())
    }

    /// 앱에 주는 모양(`profile`): 그림이 없으면 `avatar`가 `null`이다.
    pub fn json(name: &str, avatar: Option<&Avatar>) -> Value {
        json!({
            "name": name,
            "avatar": avatar.map(|a| json!({
                "id": a.id,
                "mime": a.mime,
                "width": a.width,
                "height": a.height,
                "size": a.size,
                "updated_at": a.set_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            })),
        })
    }

    /// 그림을 프로필로 정한다. 규칙에 맞지 않으면 까닭(어긴 것 모두).
    pub fn set(&self, bytes: &[u8], by: &str, now: DateTime<Utc>) -> Result<Avatar, String> {
        let (mime, width, height) = check(bytes)?;
        let digest: String = Sha256::digest(bytes)
            .iter()
            .take(12)
            .map(|b| format!("{b:02x}"))
            .collect();
        let avatar = Avatar {
            id: format!("a_{digest}"),
            mime: mime.to_owned(),
            width,
            height,
            size: bytes.len() as u64,
            set_at: now,
            by: by.to_owned(),
        };
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .map_err(|e| format!("프로필 디렉터리: {e}"))?;
        let path = self.dir.join(&avatar.id);
        if !path.exists() {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(|e| format!("그림을 남기지 못했다: {e}"))?;
            file.write_all(bytes)
                .map_err(|e| format!("그림을 남기지 못했다: {e}"))?;
        }
        let raw = serde_json::to_string(&avatar).map_err(|e| e.to_string())?;
        self.store
            .lock()
            .expect("store lock")
            .set_setting(KEY, &raw, now)
            .map_err(|e| format!("저장: {e}"))?;
        self.prune(&avatar.id);
        self.changes.send_replace(Some(avatar.clone()));
        Ok(avatar)
    }

    /// 프로필 그림을 없앤다. 있던 것을 없앴으면 `true`.
    pub fn clear(&self) -> Result<bool, String> {
        let had = self.avatar().is_some();
        self.store
            .lock()
            .expect("store lock")
            .delete_setting(KEY)
            .map_err(|e| format!("저장: {e}"))?;
        self.changes.send_replace(None);
        Ok(had)
    }

    /// id로 그림을 준다(지금 것과, 아직 남아 있는 예전 것): (MIME, 내용).
    pub fn file(&self, id: &str) -> Option<(&'static str, Vec<u8>)> {
        if !valid_id(id) {
            return None;
        }
        let bytes = std::fs::read(self.dir.join(id)).ok()?;
        let (mime, _, _) = inspect(&bytes).ok()?;
        Some((mime, bytes))
    }

    /// 지금 그림의 내용.
    pub fn current(&self) -> Option<(Avatar, Vec<u8>)> {
        let avatar = self.avatar()?;
        let (_, bytes) = self.file(&avatar.id)?;
        Some((avatar, bytes))
    }

    /// 예전 그림은 새것부터 [`KEEP_FILES`]개만 남긴다(지금 것은 늘 남긴다).
    fn prune(&self, current: &str) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                valid_id(&name) && name != current
            })
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        files.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, path) in files.into_iter().skip(KEEP_FILES.saturating_sub(1)) {
            let _ = std::fs::remove_file(path);
        }
    }

    pub fn discord_state(&self) -> DiscordState {
        self.store
            .lock()
            .expect("store lock")
            .setting(DISCORD_KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn set_discord_state(&self, state: &DiscordState, now: DateTime<Utc>) {
        if let Ok(raw) = serde_json::to_string(state) {
            let _ = self
                .store
                .lock()
                .expect("store lock")
                .set_setting(DISCORD_KEY, &raw, now);
        }
    }
}

/// Discord 봇의 아바타를 다루는 곳(실제는 [`DiscordAvatar`], 시험은 가짜).
pub trait AvatarHost: Send + Sync {
    fn connected(&self) -> bool;
    /// 지금 아바타의 표지(그림이 바뀌면 달라지는 값, 실제로는 CDN 주소). 아바타가 없으면 `None`.
    fn identity(&self) -> crate::browser::BoxFut<'_, Result<Option<String>, String>>;
    /// 지금 아바타의 내용. 아바타가 없으면 `None`.
    fn current(&self) -> crate::browser::BoxFut<'_, Result<Option<Vec<u8>>, String>>;
    /// 아바타를 바꾼다(`None`이면 없앤다).
    fn set<'a>(
        &'a self,
        data_uri: Option<&'a str>,
    ) -> crate::browser::BoxFut<'a, Result<(), String>>;
}

/// 연결된 Discord 봇.
pub struct DiscordAvatar {
    pub control: Arc<crate::discord_ctl::DiscordControl>,
    pub files: Arc<crate::files::FileStore>,
}

impl AvatarHost for DiscordAvatar {
    fn connected(&self) -> bool {
        self.control.rest().is_some()
    }

    fn identity(&self) -> crate::browser::BoxFut<'_, Result<Option<String>, String>> {
        use lucia_channel::discord::DiscordApi as _;
        Box::pin(async move {
            let rest = self.control.rest().ok_or("Discord가 연결되어 있지 않다")?;
            rest.api().avatar_url().await.map_err(|e| e.to_string())
        })
    }

    fn current(&self) -> crate::browser::BoxFut<'_, Result<Option<Vec<u8>>, String>> {
        Box::pin(async move {
            let Some(url) = self.identity().await? else {
                return Ok(None);
            };
            self.files.fetch_cdn(&url).await.map(Some)
        })
    }

    fn set<'a>(
        &'a self,
        data_uri: Option<&'a str>,
    ) -> crate::browser::BoxFut<'a, Result<(), String>> {
        use lucia_channel::discord::DiscordApi as _;
        Box::pin(async move {
            let rest = self.control.rest().ok_or("Discord가 연결되어 있지 않다")?;
            rest.api()
                .set_avatar(data_uri)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

/// 도구가 쓰는 묶음.
#[derive(Clone)]
pub struct ProfileAccess {
    pub profile: Arc<Profile>,
    /// Discord 봇(없거나 연결되어 있지 않으면 앱에만 적용한다).
    pub discord: Option<Arc<dyn AvatarHost>>,
    pub files: Arc<crate::files::FileStore>,
}

impl ProfileAccess {
    fn host(&self) -> Option<&Arc<dyn AvatarHost>> {
        self.discord.as_ref().filter(|h| h.connected())
    }

    /// 새 프로필 그림을 Discord 봇에도 올린다. 한 일을 알리는 말.
    ///
    /// 덮어쓰기 전에, 지금 Discord에 있는 아바타가 데몬이 올린 것이 아니면(처음 바꿀 때, 또는 그 사이 사용자가
    /// 개발자 포털에서 바꿨을 때) 먼저 받아 보관함에 넣는다. 덮어쓰면 되찾을 길이 없다. 확인하거나 받아 두지 못하면
    /// Discord는 바꾸지 않는다.
    pub async fn apply_discord(&self, avatar: &Avatar, bytes: &[u8], now: DateTime<Utc>) -> String {
        let Some(host) = self.host() else {
            return "Discord는 연결되어 있지 않아 바꾸지 않았다.".into();
        };
        let mut state = self.profile.discord_state();
        let on_discord = match host.identity().await {
            Ok(id) => id,
            Err(why) => {
                return format!(
                    "Discord는 바꾸지 않았다: 지금 아바타를 확인하지 못했다({why}). 덮어쓰면 되찾을 수 없어 그대로 두었다."
                );
            }
        };
        // 올린 뒤의 표식을 읽지 못했던 때(`seen` 없음)는 모르는 것이다. 모르는 것을 "사용자가 바꿨다"로 보면
        // Lucia가 올린 그림이 원래 것 자리에 들어가므로, 그때는 보관해 둔 원래 것을 그대로 둔다.
        let ours = state.applied.is_some() && (state.seen.is_none() || state.seen == on_discord);
        if !ours {
            match (on_discord.is_some(), host.current().await) {
                (true, Ok(Some(original))) => {
                    let saved = self
                        .files
                        .save_made("discord-원래-아바타.png", &original, None, now)
                        .and_then(|f| {
                            self.files.keep(
                                &f.id,
                                "Discord 원래 프로필",
                                "Lucia가 프로필을 바꾸기 전의 Discord 봇 아바타",
                                now,
                            )
                        });
                    match saved {
                        Ok(f) => state.original = Some(f.id),
                        Err(why) => {
                            return format!(
                                "Discord는 바꾸지 않았다: 원래 아바타를 보관하지 못했다({why})."
                            );
                        }
                    }
                }
                (false, _) | (true, Ok(None)) => state.original = Some(String::new()),
                (true, Err(why)) => {
                    return format!(
                        "Discord는 바꾸지 않았다: 원래 아바타를 받아 두지 못했다({why}). 덮어쓰면 되찾을 수 없어 그대로 두었다."
                    );
                }
            }
            self.profile.set_discord_state(&state, now);
        }
        match host.set(Some(&data_uri(&avatar.mime, bytes))).await {
            Ok(()) => {
                state.applied = Some(avatar.id.clone());
                state.seen = host.identity().await.ok().flatten();
                self.profile.set_discord_state(&state, now);
                match state.original.as_deref() {
                    Some(id) if !id.is_empty() => {
                        format!("Discord 봇의 아바타도 바꿨다(원래 아바타는 보관함에 있다: {id}).")
                    }
                    _ => "Discord 봇의 아바타도 바꿨다.".into(),
                }
            }
            Err(why) => format!(
                "Discord 봇의 아바타는 바꾸지 못했다: {why}. Discord는 아바타를 자주 바꾸면 한동안 거절한다. 나중에 같은 파일로 다시 부르면 된다."
            ),
        }
    }

    /// 프로필 그림을 없앨 때 Discord 쪽: 데몬이 바꾼 적이 있으면 원래 아바타로 되돌린다(원래 것이 없었으면 없앤다).
    /// 데몬이 바꾼 적이 없으면 건드리지 않는다.
    pub async fn clear_discord(&self, now: DateTime<Utc>) -> String {
        let mut state = self.profile.discord_state();
        if state.applied.is_none() {
            return "Discord 봇의 아바타는 Lucia가 바꾼 적이 없어 그대로 두었다.".into();
        }
        let Some(host) = self.host() else {
            return "Discord는 연결되어 있지 않아 그대로 두었다(연결되면 다시 부른다).".into();
        };
        let original = state
            .original
            .as_deref()
            .filter(|id| !id.is_empty())
            .and_then(|id| self.files.get(id))
            .and_then(|f| self.files.read(&f).ok());
        let done = match &original {
            Some(bytes) => {
                let mime = inspect(bytes).map_or("image/png", |(mime, _, _)| mime);
                host.set(Some(&data_uri(mime, bytes))).await
            }
            None => host.set(None).await,
        };
        match done {
            Ok(()) => {
                state.applied = None;
                state.seen = None;
                self.profile.set_discord_state(&state, now);
                if original.is_some() {
                    "Discord 봇의 아바타를 원래 것으로 되돌렸다.".into()
                } else {
                    "Discord 봇의 아바타를 없앴다(원래 아바타가 없었다).".into()
                }
            }
            Err(why) => format!("Discord 봇의 아바타는 되돌리지 못했다: {why}."),
        }
    }
}

/// Discord에 보낼 데이터 URI(공식 문서의 Image Data: JPG·GIF·PNG).
pub fn data_uri(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// 정해 둔 크기의 JPEG 머리(SOI, APP0, 주석, SOF0)와 약간의 꼬리. 디코더 없이 크기만 읽는지 본다.
    pub fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        b.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        // 주석 조각(안에 SOF처럼 보이는 바이트가 있어도 건너뛴다).
        b.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x06, 0xFF, 0xC0, 0x00, 0x01]);
        b.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        b.extend_from_slice(&height.to_be_bytes());
        b.extend_from_slice(&width.to_be_bytes());
        b.extend_from_slice(&[0x03, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        b.extend_from_slice(&[0xFF, 0xD9]);
        b
    }

    pub fn profile() -> (Arc<Profile>, Arc<Mutex<Store>>, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("lucia-profile-{}", uuid::Uuid::new_v4().simple()));
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        (Profile::new(store.clone(), dir.clone()), store, dir)
    }

    #[test]
    fn shapes_are_read_from_png_and_jpeg_headers() {
        assert_eq!(inspect(&jpeg(640, 480)).unwrap(), ("image/jpeg", 640, 480));
        assert_eq!(
            inspect(&crate::expressions::testing::png(512)).unwrap(),
            ("image/png", 512, 512)
        );
        // 실제 인코더가 만든 JPEG(점진식, EXIF 포함)도 읽는다.
        let real = include_bytes!("../tests/fixtures/avatar-real.jpg");
        assert_eq!(inspect(real).unwrap(), ("image/jpeg", 300, 200));
        // 받지 않는 것: GIF·WebP, 깨진 JPEG, 그림이 아닌 것.
        assert!(
            inspect(b"GIF89a\x10\0\x10\0\0\0\0")
                .unwrap_err()
                .contains("PNG나 JPEG")
        );
        assert!(
            inspect(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00])
                .unwrap_err()
                .contains("깨졌")
        );
        assert!(inspect(b"hello").is_err());
    }

    #[test]
    fn every_broken_rule_is_reported_at_once() {
        assert!(check(&jpeg(512, 512)).is_ok());
        assert!(check(&crate::expressions::testing::png(128)).is_ok());
        let err = check(&jpeg(4000, 3000)).unwrap_err();
        assert!(
            err.contains("정사각형이 아니다(4000×3000)")
                && err.contains("너무 크다")
                && err.contains("pillow"),
            "{err}"
        );
        let err = check(&jpeg(64, 64)).unwrap_err();
        assert!(
            err.contains("너무 작다") && !err.contains("정사각형이 아니다"),
            "{err}"
        );
        let mut big = jpeg(512, 512);
        big.resize(MAX_BYTES as usize + 1, 0);
        assert!(check(&big).unwrap_err().contains("파일이 크다"));
        assert!(check(b"").is_err());
    }

    /// 소유자에게는 Lucia에게 하라는 말(pillow, `code_run`) 없이 어긴 것을 모두 알린다.
    #[test]
    fn the_owner_hears_every_broken_rule_without_instructions_for_lucia() {
        assert!(check_for_owner(&jpeg(512, 512)).is_ok());
        let err = check_for_owner(&jpeg(4000, 3000)).unwrap_err();
        assert!(
            err.contains("정사각형이 아니다(4000×3000)")
                && err.contains("너무 크다")
                && err.contains(&rules_text()),
            "{err}"
        );
        assert!(
            !err.contains("pillow") && !err.contains("code_run"),
            "{err}"
        );
        let mut big = jpeg(512, 512);
        big.resize(MAX_BYTES as usize + 1, 0);
        assert!(check_for_owner(&big).unwrap_err().contains("파일이 크다"));
        assert!(check_for_owner(b"").unwrap_err().contains("빈 파일"));
        assert!(
            check_for_owner(b"hello")
                .unwrap_err()
                .contains("그림 파일이 아니")
        );
        // Lucia에게 가는 안내는 그대로다.
        assert!(check(&jpeg(4000, 3000)).unwrap_err().contains("pillow"));
        assert_eq!(
            rules_text(),
            format!(
                "움직이지 않는 PNG·JPEG, 정사각형, 한 변 128~2048px, {}까지",
                crate::files::human_size(MAX_BYTES)
            )
        );
    }

    /// 결정 0064: 이름은 다듬어 저장하고, 규칙(1~24자, 제어 문자 없음)을 어기면 바꾸지 않고, 비우면 기본 이름으로 돌아간다.
    #[test]
    fn the_name_is_trimmed_validated_and_reset_to_the_default() {
        let (p, store, dir) = profile();
        let now: DateTime<Utc> = "2026-11-21T03:00:00Z".parse().unwrap();
        let stored = |store: &Arc<Mutex<Store>>| store.lock().unwrap().setting(NAME_KEY).unwrap();
        assert_eq!(p.name(), DEFAULT_NAME);
        assert_eq!(stored(&store), None);
        let mut names = p.subscribe_name();
        assert_eq!(p.set_name("  루시아  ", now).unwrap(), "루시아");
        assert_eq!(p.name(), "루시아");
        assert_eq!(stored(&store).as_deref(), Some("루시아"));
        assert!(names.has_changed().unwrap());
        assert_eq!(*names.borrow_and_update(), "루시아");
        // 24자까지(글자 단위: 한글도 한 글자).
        let longest = "가".repeat(NAME_MAX_CHARS);
        assert_eq!(p.set_name(&longest, now).unwrap(), longest);
        // 어기면 바꾸지 않고, 알리지도 않는다.
        for bad in [
            "가".repeat(NAME_MAX_CHARS + 1),
            "한\n줄".to_owned(),
            "탭\t이름".to_owned(),
            "\u{7}벨".to_owned(),
        ] {
            assert!(p.set_name(&bad, now).is_err(), "{bad:?}");
        }
        assert_eq!(p.name(), longest);
        names.borrow_and_update();
        assert!(!names.has_changed().unwrap());
        // 빈 입력과 공백만은 정한 이름을 지워 기본으로 돌아간다.
        assert_eq!(p.set_name("   ", now).unwrap(), DEFAULT_NAME);
        assert_eq!(stored(&store), None);
        assert_eq!(*names.borrow_and_update(), DEFAULT_NAME);
        // 다시 뜬 데몬은 저장소에서 읽는다. 기본 이름과 같은 글을 정해도 정한 것으로 남는다.
        assert_eq!(p.set_name("Lucia", now).unwrap(), "Lucia");
        assert_eq!(stored(&store).as_deref(), Some("Lucia"));
        p.set_name("달빛", now).unwrap();
        assert_eq!(Profile::new(store, dir.clone()).name(), "달빛");
        assert_eq!(validate_name("  "), Ok(None));
        assert_eq!(validate_name(" a "), Ok(Some("a".into())));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn set_clear_and_old_files() {
        let (p, store, dir) = profile();
        let mut changes = p.subscribe();
        assert_eq!(p.avatar(), None);
        assert_eq!(
            Profile::json("Lucia", None),
            json!({"name": "Lucia", "avatar": null})
        );
        assert_eq!(p.view(), json!({"name": "Lucia", "avatar": null}));
        let now: DateTime<Utc> = "2026-11-20T03:00:00Z".parse().unwrap();
        let first = p.set(&jpeg(512, 512), "lucia", now).unwrap();
        assert!(valid_id(&first.id) && first.mime == "image/jpeg" && first.width == 512);
        assert!(changes.has_changed().unwrap());
        assert_eq!(changes.borrow_and_update().as_ref(), Some(&first));
        let shown = p.view();
        assert_eq!(shown["avatar"]["id"], first.id.as_str());
        assert_eq!(shown["avatar"]["updated_at"], "2026-11-20T03:00:00Z");
        assert_eq!(p.current().unwrap().1, jpeg(512, 512));
        // 규칙에 맞지 않으면 바꾸지 않는다.
        assert!(p.set(&jpeg(4000, 3000), "lucia", now).is_err());
        assert_eq!(p.avatar().unwrap().id, first.id);
        // 바꾸면 id가 바뀌고, 예전 그림도 id로 받을 수 있다. 다시 뜬 데몬은 저장소에서 읽는다.
        let second = p
            .set(&crate::expressions::testing::png(256), "lucia", now)
            .unwrap();
        assert_ne!(second.id, first.id);
        assert_eq!(p.file(&first.id).unwrap().0, "image/jpeg");
        assert_eq!(p.file(&second.id).unwrap().0, "image/png");
        assert!(p.file("a_nothex").is_none() && p.file("../x").is_none());
        let again = Profile::new(store.clone(), dir.clone());
        assert_eq!(again.avatar().unwrap().id, second.id);
        // 없애면 null. 한 번 더 없애도 된다.
        assert!(p.clear().unwrap());
        assert!(!p.clear().unwrap());
        assert_eq!(p.avatar(), None);
        assert_eq!(changes.borrow_and_update().as_ref(), None);
        assert_eq!(Profile::new(store, dir.clone()).avatar(), None);
        // Discord 상태는 따로 남는다.
        assert_eq!(p.discord_state(), DiscordState::default());
        p.set_discord_state(
            &DiscordState {
                applied: Some(second.id.clone()),
                original: Some(String::new()),
                seen: None,
            },
            now,
        );
        assert_eq!(
            p.discord_state().applied.as_deref(),
            Some(second.id.as_str())
        );
        assert!(data_uri("image/png", b"ab").starts_with("data:image/png;base64,YWI="));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 가짜 Discord 봇: 지금 아바타와, 받은 `set` 호출을 적어 둔다(관리 페이지 시험도 쓴다).
    #[derive(Default)]
    pub(crate) struct FakeHost {
        pub(crate) online: Mutex<bool>,
        pub(crate) avatar: Mutex<Option<Vec<u8>>>,
        pub(crate) sets: Mutex<Vec<Option<String>>>,
        refuse: Mutex<Option<String>>,
        fetch_fails: Mutex<bool>,
        /// 지금 아바타가 무엇인지 답하지 못한다(Discord 오류).
        blind: Mutex<bool>,
        /// 올린 바로 뒤부터 답하지 못한다.
        blind_after_set: Mutex<bool>,
    }

    impl AvatarHost for FakeHost {
        fn connected(&self) -> bool {
            *self.online.lock().unwrap()
        }
        fn identity(&self) -> crate::browser::BoxFut<'_, Result<Option<String>, String>> {
            // 실제 Discord처럼 그림이 바뀌면 달라지는 값.
            let id = self.avatar.lock().unwrap().as_ref().map(|b| {
                format!(
                    "https://cdn.discordapp.com/avatars/1/{:x}.png",
                    b.len() * 31 + b[b.len() / 2] as usize
                )
            });
            let blind = *self.blind.lock().unwrap();
            Box::pin(async move {
                if blind {
                    return Err("확인 실패(HTTP 500)".into());
                }
                Ok(id)
            })
        }
        fn current(&self) -> crate::browser::BoxFut<'_, Result<Option<Vec<u8>>, String>> {
            Box::pin(async move {
                if *self.fetch_fails.lock().unwrap() {
                    return Err("내려받기 실패(HTTP 503)".into());
                }
                Ok(self.avatar.lock().unwrap().clone())
            })
        }
        fn set<'a>(
            &'a self,
            data_uri: Option<&'a str>,
        ) -> crate::browser::BoxFut<'a, Result<(), String>> {
            Box::pin(async move {
                if let Some(why) = self.refuse.lock().unwrap().clone() {
                    return Err(why);
                }
                self.sets.lock().unwrap().push(data_uri.map(str::to_owned));
                // 올린 그림이 지금 아바타가 된다(Discord는 다시 인코딩하지만 여기서는 받은 글 그대로 둔다).
                *self.avatar.lock().unwrap() = data_uri.map(|d| d.as_bytes().to_vec());
                if *self.blind_after_set.lock().unwrap() {
                    *self.blind.lock().unwrap() = true;
                }
                Ok(())
            })
        }
    }

    fn access(host: Arc<FakeHost>) -> (ProfileAccess, PathBuf) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (profile, store, dir) = profile();
        let files = Arc::new(crate::files::FileStore::new(
            dir.join("files"),
            reqwest::Client::new(),
            store,
        ));
        (
            ProfileAccess {
                profile,
                discord: Some(host),
                files,
            },
            dir,
        )
    }

    /// Discord 봇의 아바타: 처음 바꾸기 전에 원래 것을 보관함에 넣고, 없앨 때 원래 것으로 되돌린다.
    /// 데몬이 바꾼 적이 없는 아바타는 건드리지 않는다.
    #[tokio::test]
    async fn the_discord_avatar_is_saved_before_the_first_overwrite_and_restored_on_clear() {
        let now: DateTime<Utc> = "2026-11-20T03:00:00Z".parse().unwrap();
        let host = Arc::new(FakeHost::default());
        let (a, dir) = access(host.clone());
        let new = jpeg(512, 512);
        let avatar = a.profile.set(&new, "lucia", now).unwrap();
        // 연결되어 있지 않으면 앱에만 적용된다. 없앨 때도 건드리지 않는다.
        assert!(
            a.apply_discord(&avatar, &new, now)
                .await
                .contains("연결되어 있지 않아")
        );
        assert!(a.clear_discord(now).await.contains("바꾼 적이 없어"));
        assert!(host.sets.lock().unwrap().is_empty());

        // 원래 아바타를 받아 두지 못하면 덮어쓰지 않는다.
        *host.online.lock().unwrap() = true;
        let original = crate::expressions::testing::png(256);
        *host.avatar.lock().unwrap() = Some(original.clone());
        *host.fetch_fails.lock().unwrap() = true;
        let said = a.apply_discord(&avatar, &new, now).await;
        assert!(
            said.contains("바꾸지 않았다") && said.contains("503"),
            "{said}"
        );
        assert!(host.sets.lock().unwrap().is_empty());
        assert_eq!(a.profile.discord_state(), DiscordState::default());

        // 처음 바꿀 때: 원래 것을 보관함에 넣은 뒤 올린다.
        *host.fetch_fails.lock().unwrap() = false;
        let said = a.apply_discord(&avatar, &new, now).await;
        assert!(
            said.contains("아바타도 바꿨다") && said.contains("보관함"),
            "{said}"
        );
        let state = a.profile.discord_state();
        assert_eq!(state.applied.as_deref(), Some(avatar.id.as_str()));
        let kept = a.files.kept();
        assert_eq!(kept.len(), 1);
        assert_eq!(Some(kept[0].file.id.as_str()), state.original.as_deref());
        assert_eq!(a.files.read(&kept[0].file).unwrap(), original);
        assert_eq!(
            host.sets.lock().unwrap().last().unwrap().as_deref(),
            Some(data_uri("image/jpeg", &new).as_str())
        );
        // 다시 바꿀 때는 원래 것을 또 받지 않는다(지금 Discord에 있는 것은 Lucia가 올린 그림이다).
        let next = crate::expressions::testing::png(512);
        let avatar2 = a.profile.set(&next, "lucia", now).unwrap();
        a.apply_discord(&avatar2, &next, now).await;
        assert_eq!(a.files.kept().len(), 1);
        assert_eq!(a.profile.discord_state().original, state.original);
        // 그 사이 사용자가 개발자 포털에서 아바타를 바꿨으면, 덮어쓰기 전에 그 그림을 다시 보관한다.
        let by_user = jpeg(640, 640);
        *host.avatar.lock().unwrap() = Some(by_user.clone());
        a.apply_discord(&avatar2, &next, now).await;
        let kept = a.files.kept();
        assert_eq!(kept.len(), 2);
        let newest = a.profile.discord_state().original.unwrap();
        assert_ne!(Some(newest.as_str()), state.original.as_deref());
        assert_eq!(
            a.files.read(&a.files.get(&newest).unwrap()).unwrap(),
            by_user
        );
        let original = by_user;
        // Discord가 거절하면(자주 바꿈 등) 까닭을 그대로 알리고, 앱 쪽 설정은 그대로다.
        *host.refuse.lock().unwrap() =
            Some("Response error: status code 400, error: AVATAR_RATE_LIMIT".into());
        let said = a.apply_discord(&avatar2, &next, now).await;
        assert!(
            said.contains("바꾸지 못했다") && said.contains("AVATAR_RATE_LIMIT"),
            "{said}"
        );
        assert_eq!(a.profile.avatar().unwrap().id, avatar2.id);
        *host.refuse.lock().unwrap() = None;
        // 없애면 원래 아바타로 되돌린다.
        a.profile.clear().unwrap();
        let said = a.clear_discord(now).await;
        assert!(said.contains("원래 것으로 되돌렸다"), "{said}");
        assert_eq!(
            host.sets.lock().unwrap().last().unwrap().as_deref(),
            Some(data_uri("image/jpeg", &original).as_str())
        );
        assert_eq!(a.profile.discord_state().applied, None);
        assert!(a.clear_discord(now).await.contains("바꾼 적이 없어"));
        let _ = std::fs::remove_dir_all(dir);

        // 원래 아바타가 없던 봇: 없앨 때 아바타를 없앤다.
        let host = Arc::new(FakeHost::default());
        *host.online.lock().unwrap() = true;
        let (a, dir) = access(host.clone());
        let avatar = a.profile.set(&new, "lucia", now).unwrap();
        assert_eq!(
            a.apply_discord(&avatar, &new, now).await,
            "Discord 봇의 아바타도 바꿨다."
        );
        assert!(a.files.kept().is_empty());
        a.profile.clear().unwrap();
        assert!(a.clear_discord(now).await.contains("없앴다"));
        assert_eq!(host.sets.lock().unwrap().last().unwrap(), &None);
        let _ = std::fs::remove_dir_all(dir);

        // 지금 아바타를 확인하지 못하면 바꾸지 않는다.
        let host = Arc::new(FakeHost::default());
        *host.online.lock().unwrap() = true;
        let original = crate::expressions::testing::png(256);
        *host.avatar.lock().unwrap() = Some(original.clone());
        *host.blind.lock().unwrap() = true;
        let (a, dir) = access(host.clone());
        let avatar = a.profile.set(&new, "lucia", now).unwrap();
        let said = a.apply_discord(&avatar, &new, now).await;
        assert!(
            said.contains("바꾸지 않았다") && said.contains("500"),
            "{said}"
        );
        assert!(host.sets.lock().unwrap().is_empty());
        // 올린 바로 뒤에 확인이 실패했으면(무엇이 걸렸는지 모름), 다음에 바꿀 때 Lucia가 올린 그림을 원래 것으로
        // 잘못 보관하지 않는다. 없애면 처음의 원래 것으로 돌아간다.
        *host.blind.lock().unwrap() = false;
        *host.blind_after_set.lock().unwrap() = true;
        a.apply_discord(&avatar, &new, now).await;
        let state = a.profile.discord_state();
        assert_eq!((state.applied.is_some(), state.seen.clone()), (true, None));
        *host.blind.lock().unwrap() = false;
        *host.blind_after_set.lock().unwrap() = false;
        let next = crate::expressions::testing::png(512);
        let avatar2 = a.profile.set(&next, "lucia", now).unwrap();
        a.apply_discord(&avatar2, &next, now).await;
        assert_eq!(a.files.kept().len(), 1);
        let after = a.profile.discord_state();
        assert_eq!(after.original, state.original);
        assert!(after.seen.is_some());
        a.profile.clear().unwrap();
        a.clear_discord(now).await;
        assert_eq!(
            host.sets.lock().unwrap().last().unwrap().as_deref(),
            Some(data_uri("image/png", &original).as_str())
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
