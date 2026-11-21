//! 서버 조회와 관리(결정 0024). REST만 쓴다(게이트웨이 캐시 없이).
//!
//! - 조회: 봇이 들어간 서버(봇의 권한 포함), 채널, 메시지 기록, 역할, 멤버 찾기.
//! - 관리: 정해진 동작([`Manage`])만. 무엇을 할지는 코어가 승인 카드로 사람에게 받은 뒤 부른다.
//!   서버 지우기, 일괄 차단·정리(prune), 역할 권한 바꾸기는 여기에 없다.
//!
//! 메시지 내용은 특권 intent `MESSAGE_CONTENT`가 켜져 있어야 REST에서도 온다(공식 문서). 꺼져 있으면 다른
//! 사람의 글은 내용이 비어 있다.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use twilight_http::Client;
use twilight_http::request::AuditLogReason as _;
use twilight_model::guild::Permissions;

use super::{body_error, http_error, id};
use crate::ChannelError;

/// 봇이 들어간 서버.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Server {
    /// JSON으로는 글로 내보낸다(브라우저의 수는 2^53을 넘는 ID를 그대로 담지 못한다).
    #[serde(serialize_with = "id_text")]
    pub id: u64,
    pub name: String,
    /// 봇이 이 서버에서 관리자 권한(ADMINISTRATOR)을 가졌다.
    pub admin: bool,
    /// 봇이 서버 주인이다.
    pub owner: bool,
}

fn id_text<S: serde::Serializer>(id: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(id)
}

/// 봇이 들어간 서버(최대 200개, 이름 순).
pub async fn servers(c: &Client) -> Result<Vec<Server>, ChannelError> {
    let list = c
        .current_user_guilds()
        .limit(200)
        .await
        .map_err(http_error)?
        .models()
        .await
        .map_err(body_error)?;
    let mut out: Vec<Server> = list
        .into_iter()
        .map(|g| Server {
            id: g.id.get(),
            name: g.name,
            admin: g.owner || g.permissions.contains(Permissions::ADMINISTRATOR),
            owner: g.owner,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// 서버의 채널 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    pub id: u64,
    pub name: String,
    /// text, voice, category, announcement, forum, stage, thread, other
    pub kind: &'static str,
    pub parent: Option<u64>,
    pub position: i32,
    pub topic: Option<String>,
}

fn kind_name(k: twilight_model::channel::ChannelType) -> &'static str {
    use twilight_model::channel::ChannelType as T;
    match k {
        T::GuildText => "text",
        T::GuildVoice => "voice",
        T::GuildCategory => "category",
        T::GuildAnnouncement => "announcement",
        T::GuildForum | T::GuildMedia => "forum",
        T::GuildStageVoice => "stage",
        T::PublicThread | T::PrivateThread | T::AnnouncementThread => "thread",
        T::Private | T::Group => "dm",
        _ => "other",
    }
}

fn channel_info(ch: twilight_model::channel::Channel) -> ChannelInfo {
    ChannelInfo {
        id: ch.id.get(),
        name: ch.name.unwrap_or_default(),
        kind: kind_name(ch.kind),
        parent: ch.parent_id.map(|p| p.get()),
        position: ch.position.unwrap_or(0),
        topic: ch.topic.filter(|t| !t.trim().is_empty()),
    }
}

/// 서버의 채널(분류 → 위치 순).
pub async fn channels(c: &Client, guild: u64) -> Result<Vec<ChannelInfo>, ChannelError> {
    let list = c
        .guild_channels(id(guild)?)
        .await
        .map_err(http_error)?
        .models()
        .await
        .map_err(body_error)?;
    let mut out: Vec<ChannelInfo> = list.into_iter().map(channel_info).collect();
    out.sort_by_key(|c| (c.parent.unwrap_or(c.id), c.kind != "category", c.position));
    Ok(out)
}

/// 채널 하나와 그 서버(DM이면 `None`).
pub async fn channel(c: &Client, channel: u64) -> Result<(ChannelInfo, Option<u64>), ChannelError> {
    let ch = c
        .channel(id(channel)?)
        .await
        .map_err(http_error)?
        .model()
        .await
        .map_err(body_error)?;
    let guild = ch.guild_id.map(|g| g.get());
    Ok((channel_info(ch), guild))
}

/// 메시지 기록의 한 줄.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Msg {
    pub id: u64,
    pub author_id: u64,
    pub author: String,
    pub bot: bool,
    pub content: String,
    pub attachments: Vec<String>,
    pub embeds: usize,
    pub at: Option<DateTime<Utc>>,
    pub edited: bool,
    /// 답장한 메시지.
    pub reply_to: Option<u64>,
}

/// 어디서부터 읽을지.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Latest,
    Before(u64),
    After(u64),
    Around(u64),
}

/// 채널의 메시지(오래된 것부터, 1~100개).
pub async fn messages(
    c: &Client,
    channel: u64,
    limit: u16,
    cursor: Cursor,
) -> Result<Vec<Msg>, ChannelError> {
    let limit = limit.clamp(1, 100);
    let req = c.channel_messages(id(channel)?);
    let resp = match cursor {
        Cursor::Latest => req.limit(limit).await,
        Cursor::Before(m) => req.before(id(m)?).limit(limit).await,
        Cursor::After(m) => req.after(id(m)?).limit(limit).await,
        Cursor::Around(m) => req.around(id(m)?).limit(limit).await,
    }
    .map_err(http_error)?;
    let list = resp.models().await.map_err(body_error)?;
    let mut out: Vec<Msg> = list
        .into_iter()
        .map(|m| {
            let author = m
                .member
                .as_ref()
                .and_then(|mem| mem.nick.clone())
                .or_else(|| m.author.global_name.clone())
                .unwrap_or_else(|| m.author.name.clone());
            Msg {
                id: m.id.get(),
                author_id: m.author.id.get(),
                author,
                bot: m.author.bot,
                content: m.content,
                attachments: m.attachments.into_iter().map(|a| a.filename).collect(),
                embeds: m.embeds.len(),
                at: DateTime::from_timestamp_micros(m.timestamp.as_micros()),
                edited: m.edited_timestamp.is_some(),
                reply_to: m.reference.and_then(|r| r.message_id).map(|i| i.get()),
            }
        })
        .collect();
    out.sort_by_key(|m| m.id);
    Ok(out)
}

/// 메시지 하나.
pub async fn message(c: &Client, channel: u64, message: u64) -> Result<Msg, ChannelError> {
    let m = c
        .message(id(channel)?, id(message)?)
        .await
        .map_err(http_error)?
        .model()
        .await
        .map_err(body_error)?;
    let author = m
        .member
        .as_ref()
        .and_then(|mem| mem.nick.clone())
        .or_else(|| m.author.global_name.clone())
        .unwrap_or_else(|| m.author.name.clone());
    Ok(Msg {
        id: m.id.get(),
        author_id: m.author.id.get(),
        author,
        bot: m.author.bot,
        content: m.content,
        attachments: m.attachments.into_iter().map(|a| a.filename).collect(),
        embeds: m.embeds.len(),
        at: DateTime::from_timestamp_micros(m.timestamp.as_micros()),
        edited: m.edited_timestamp.is_some(),
        reply_to: m.reference.and_then(|r| r.message_id).map(|i| i.get()),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleInfo {
    pub id: u64,
    pub name: String,
    pub position: i64,
    /// 봇·연동이 관리하는 역할(사람에게 줄 수 없다).
    pub managed: bool,
    pub admin: bool,
}

/// 서버의 역할(높은 것부터).
pub async fn roles(c: &Client, guild: u64) -> Result<Vec<RoleInfo>, ChannelError> {
    let list = c
        .roles(id(guild)?)
        .await
        .map_err(http_error)?
        .models()
        .await
        .map_err(body_error)?;
    let mut out: Vec<RoleInfo> = list
        .into_iter()
        .map(|r| RoleInfo {
            id: r.id.get(),
            name: r.name,
            position: r.position,
            managed: r.managed,
            admin: r.permissions.contains(Permissions::ADMINISTRATOR),
        })
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.position));
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberInfo {
    pub id: u64,
    pub username: String,
    /// 서버 별명 → 전역 이름 → 사용자 이름.
    pub display: String,
    pub bot: bool,
    pub roles: Vec<u64>,
    pub joined_at: Option<DateTime<Utc>>,
    /// 타임아웃이 끝나는 때(걸려 있으면).
    pub timeout_until: Option<DateTime<Utc>>,
}

fn member_info(m: twilight_model::guild::Member) -> MemberInfo {
    let display = m
        .nick
        .clone()
        .or_else(|| m.user.global_name.clone())
        .unwrap_or_else(|| m.user.name.clone());
    MemberInfo {
        id: m.user.id.get(),
        username: m.user.name,
        display,
        bot: m.user.bot,
        roles: m.roles.iter().map(|r| r.get()).collect(),
        joined_at: m
            .joined_at
            .and_then(|t| DateTime::from_timestamp_micros(t.as_micros())),
        timeout_until: m
            .communication_disabled_until
            .and_then(|t| DateTime::from_timestamp_micros(t.as_micros()))
            .filter(|t| *t > Utc::now()),
    }
}

/// 사용자 이름·별명이 `query`로 시작하는 멤버(1~100명).
pub async fn search_members(
    c: &Client,
    guild: u64,
    query: &str,
    limit: u16,
) -> Result<Vec<MemberInfo>, ChannelError> {
    let list = c
        .search_guild_members(id(guild)?, query)
        .limit(limit.clamp(1, 100))
        .await
        .map_err(http_error)?
        .models()
        .await
        .map_err(body_error)?;
    Ok(list.into_iter().map(member_info).collect())
}

/// 멤버 하나.
pub async fn member(c: &Client, guild: u64, user: u64) -> Result<MemberInfo, ChannelError> {
    let m = c
        .guild_member(id(guild)?, id(user)?)
        .await
        .map_err(http_error)?
        .model()
        .await
        .map_err(body_error)?;
    Ok(member_info(m))
}

/// 채널에 글을 올린다(`reply_to`면 그 메시지에 답장, 알림은 보내지 않는다). 메시지 ID를 돌려준다.
pub async fn post(
    c: &Client,
    channel: u64,
    text: &str,
    reply_to: Option<u64>,
) -> Result<u64, ChannelError> {
    let mut req = c.create_message(id(channel)?).content(text);
    if let Some(m) = reply_to {
        req = req.reply(id(m)?).fail_if_not_exists(false);
    }
    let m = req
        .await
        .map_err(http_error)?
        .model()
        .await
        .map_err(body_error)?;
    Ok(m.id.get())
}

/// 서버에 등록된 스티커(결정 0044). 모델에게는 이름·설명·태그만 보인다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildSticker {
    pub id: String,
    pub name: String,
    pub description: String,
    /// 쉼표로 나눈 태그(자동 완성·관련 이모지).
    pub tags: String,
}

/// 서버의 스티커(`GET /guilds/{guild}/stickers`). 쓸 수 없는 것(`available: false`, 부스트가 빠져 잠긴 것)은 뺀다.
/// `available`은 선택 필드라 없으면 쓸 수 있는 것으로 본다(타입 라이브러리는 없으면 false로 읽어 직접 읽는다).
pub async fn stickers(c: &Client, guild: u64) -> Result<Vec<GuildSticker>, ChannelError> {
    let text = c
        .guild_stickers(id(guild)?)
        .await
        .map_err(http_error)?
        .text()
        .await
        .map_err(|e| ChannelError::Send(format!("스티커 목록을 읽지 못했다: {e}")))?;
    parse_stickers(&text)
}

fn parse_stickers(text: &str) -> Result<Vec<GuildSticker>, ChannelError> {
    let list: Vec<serde_json::Value> = serde_json::from_str(text)
        .map_err(|_| ChannelError::Send("스티커 목록 형식이 아니다".into()))?;
    Ok(list
        .into_iter()
        .filter(|v| v["available"].as_bool().unwrap_or(true))
        .filter_map(|v| {
            Some(GuildSticker {
                id: v["id"].as_str()?.to_owned(),
                name: v["name"].as_str()?.to_owned(),
                description: v["description"].as_str().unwrap_or("").to_owned(),
                tags: v["tags"].as_str().unwrap_or("").to_owned(),
            })
        })
        .collect())
}

#[cfg(test)]
#[test]
fn sticker_list_keeps_usable_ones() {
    let list = parse_stickers(
        r#"[{"id":"11","name":"축하","description":"꽃가루","tags":"🎉","available":true},
            {"id":"12","name":"잠김","description":null,"tags":"x","available":false},
            {"id":"13","name":"모름","tags":""}]"#,
    )
    .unwrap();
    assert_eq!(
        list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["축하", "모름"]
    );
    assert_eq!(list[1].description, "");
    assert!(parse_stickers("nope").is_err());
}

/// 채널 종류(만들 때).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewChannelKind {
    Text,
    Voice,
    Category,
}

/// 서버 관리 동작(결정 0024). 고정된 목록만 있고, 사람이 승인 카드에서 누른 뒤에만 실행한다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Manage {
    /// 내보내기(다시 들어올 수 있다).
    Kick {
        guild: u64,
        user: u64,
        reason: Option<String>,
    },
    /// 차단. `delete_message_seconds`만큼 최근 메시지도 지운다(0~604800).
    Ban {
        guild: u64,
        user: u64,
        reason: Option<String>,
        delete_message_seconds: u32,
    },
    Unban {
        guild: u64,
        user: u64,
    },
    /// 타임아웃(1분~28일).
    Timeout {
        guild: u64,
        user: u64,
        minutes: u32,
        reason: Option<String>,
    },
    Untimeout {
        guild: u64,
        user: u64,
    },
    /// 별명 바꾸기(`None`이면 지운다).
    Nick {
        guild: u64,
        user: u64,
        nick: Option<String>,
    },
    RoleAdd {
        guild: u64,
        user: u64,
        role: u64,
    },
    RoleRemove {
        guild: u64,
        user: u64,
        role: u64,
    },
    DeleteMessage {
        guild: u64,
        channel: u64,
        message: u64,
    },
    Pin {
        guild: u64,
        channel: u64,
        message: u64,
    },
    Unpin {
        guild: u64,
        channel: u64,
        message: u64,
    },
    ChannelCreate {
        guild: u64,
        name: String,
        kind: NewChannelKind,
        parent: Option<u64>,
        topic: Option<String>,
    },
    ChannelRename {
        guild: u64,
        channel: u64,
        name: String,
    },
    ChannelTopic {
        guild: u64,
        channel: u64,
        topic: String,
    },
    /// 느린 모드(0이면 끈다, 최대 21600초).
    Slowmode {
        guild: u64,
        channel: u64,
        seconds: u16,
    },
    ChannelDelete {
        guild: u64,
        channel: u64,
    },
}

/// 타임아웃 최대(28일, Discord 한도).
pub const TIMEOUT_MAX_MINUTES: u32 = 28 * 24 * 60;
/// 차단할 때 지울 수 있는 최근 메시지 기간(7일, Discord 한도).
pub const BAN_DELETE_MAX_SECONDS: u32 = 7 * 24 * 3600;
/// 느린 모드 최대(6시간).
pub const SLOWMODE_MAX_SECONDS: u16 = 21_600;

impl Manage {
    pub fn guild(&self) -> u64 {
        use Manage::*;
        match self {
            Kick { guild, .. }
            | Ban { guild, .. }
            | Unban { guild, .. }
            | Timeout { guild, .. }
            | Untimeout { guild, .. }
            | Nick { guild, .. }
            | RoleAdd { guild, .. }
            | RoleRemove { guild, .. }
            | DeleteMessage { guild, .. }
            | Pin { guild, .. }
            | Unpin { guild, .. }
            | ChannelCreate { guild, .. }
            | ChannelRename { guild, .. }
            | ChannelTopic { guild, .. }
            | Slowmode { guild, .. }
            | ChannelDelete { guild, .. } => *guild,
        }
    }

    /// 대상 사용자(있으면).
    pub fn user(&self) -> Option<u64> {
        use Manage::*;
        match self {
            Kick { user, .. }
            | Ban { user, .. }
            | Unban { user, .. }
            | Timeout { user, .. }
            | Untimeout { user, .. }
            | Nick { user, .. }
            | RoleAdd { user, .. }
            | RoleRemove { user, .. } => Some(*user),
            _ => None,
        }
    }

    /// 대상 채널(있으면).
    pub fn channel(&self) -> Option<u64> {
        use Manage::*;
        match self {
            DeleteMessage { channel, .. }
            | Pin { channel, .. }
            | Unpin { channel, .. }
            | ChannelRename { channel, .. }
            | ChannelTopic { channel, .. }
            | Slowmode { channel, .. }
            | ChannelDelete { channel, .. } => Some(*channel),
            ChannelCreate { parent, .. } => *parent,
            _ => None,
        }
    }

    pub fn role(&self) -> Option<u64> {
        match self {
            Manage::RoleAdd { role, .. } | Manage::RoleRemove { role, .. } => Some(*role),
            _ => None,
        }
    }

    /// 한국어 이름(카드 제목).
    pub fn label(&self) -> &'static str {
        use Manage::*;
        match self {
            Kick { .. } => "내보내기",
            Ban { .. } => "차단",
            Unban { .. } => "차단 풀기",
            Timeout { .. } => "타임아웃",
            Untimeout { .. } => "타임아웃 풀기",
            Nick { .. } => "별명 바꾸기",
            RoleAdd { .. } => "역할 주기",
            RoleRemove { .. } => "역할 빼기",
            DeleteMessage { .. } => "메시지 지우기",
            Pin { .. } => "메시지 고정",
            Unpin { .. } => "메시지 고정 풀기",
            ChannelCreate { .. } => "채널 만들기",
            ChannelRename { .. } => "채널 이름 바꾸기",
            ChannelTopic { .. } => "채널 주제 바꾸기",
            Slowmode { .. } => "느린 모드",
            ChannelDelete { .. } => "채널 지우기",
        }
    }

    /// 되돌리기 어려운 동작(카드에 경고를 붙인다).
    pub fn destructive(&self) -> bool {
        matches!(
            self,
            Manage::Ban { .. } | Manage::DeleteMessage { .. } | Manage::ChannelDelete { .. }
        )
    }

    /// 값 범위 확인(Discord 한도).
    pub fn validate(&self) -> Result<(), String> {
        let name_ok = |n: &str| (1..=100).contains(&n.trim().chars().count());
        match self {
            Manage::Ban {
                delete_message_seconds,
                ..
            } if *delete_message_seconds > BAN_DELETE_MAX_SECONDS => {
                Err("지울 메시지 기간은 7일(604800초)까지다".into())
            }
            Manage::Timeout { minutes, .. } if !(1..=TIMEOUT_MAX_MINUTES).contains(minutes) => {
                Err("타임아웃은 1분~28일(40320분)이다".into())
            }
            Manage::Nick { nick: Some(n), .. } if n.chars().count() > 32 => {
                Err("별명은 32자까지다".into())
            }
            Manage::ChannelCreate { name, .. } | Manage::ChannelRename { name, .. }
                if !name_ok(name) =>
            {
                Err("채널 이름은 1~100자다".into())
            }
            Manage::ChannelCreate { topic: Some(t), .. }
            | Manage::ChannelTopic { topic: t, .. }
                if t.chars().count() > 1024 =>
            {
                Err("채널 주제는 1024자까지다".into())
            }
            Manage::Slowmode { seconds, .. } if *seconds > SLOWMODE_MAX_SECONDS => {
                Err("느린 모드는 21600초(6시간)까지다".into())
            }
            Manage::Kick {
                reason: Some(r), ..
            }
            | Manage::Ban {
                reason: Some(r), ..
            }
            | Manage::Timeout {
                reason: Some(r), ..
            } if r.chars().count() > 400 => Err("이유는 400자까지다".into()),
            _ => Ok(()),
        }
    }
}

/// 관리 동작을 한 번 실행하고 결과 문구를 돌려준다(다시 시도하지 않는다).
pub async fn manage(c: &Client, m: &Manage) -> Result<String, ChannelError> {
    m.validate().map_err(ChannelError::Send)?;
    let reason = |r: &Option<String>| {
        r.as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_owned)
    };
    match m {
        Manage::Kick {
            guild,
            user,
            reason: r,
        } => {
            let mut req = c.remove_guild_member(id(*guild)?, id(*user)?);
            if let Some(r) = reason(r) {
                req = req.reason(&r);
                req.await.map_err(http_error)?;
            } else {
                req.await.map_err(http_error)?;
            }
            Ok("내보냈다".into())
        }
        Manage::Ban {
            guild,
            user,
            reason: r,
            delete_message_seconds,
        } => {
            let req = c
                .create_ban(id(*guild)?, id(*user)?)
                .delete_message_seconds(*delete_message_seconds);
            match reason(r) {
                Some(r) => req.reason(&r).await,
                None => req.await,
            }
            .map_err(http_error)?;
            Ok("차단했다".into())
        }
        Manage::Unban { guild, user } => {
            c.delete_ban(id(*guild)?, id(*user)?)
                .await
                .map_err(http_error)?;
            Ok("차단을 풀었다".into())
        }
        Manage::Timeout {
            guild,
            user,
            minutes,
            reason: r,
        } => {
            let until = Utc::now() + chrono::Duration::minutes(i64::from(*minutes));
            let ts = twilight_model::util::Timestamp::from_secs(until.timestamp())
                .map_err(|e| ChannelError::Send(e.to_string()))?;
            let req = c
                .update_guild_member(id(*guild)?, id(*user)?)
                .communication_disabled_until(Some(ts));
            match reason(r) {
                Some(r) => req.reason(&r).await,
                None => req.await,
            }
            .map_err(http_error)?;
            Ok(format!("{minutes}분 타임아웃을 걸었다"))
        }
        Manage::Untimeout { guild, user } => {
            c.update_guild_member(id(*guild)?, id(*user)?)
                .communication_disabled_until(None)
                .await
                .map_err(http_error)?;
            Ok("타임아웃을 풀었다".into())
        }
        Manage::Nick { guild, user, nick } => {
            let nick = nick.as_deref().map(str::trim).filter(|n| !n.is_empty());
            c.update_guild_member(id(*guild)?, id(*user)?)
                .nick(nick)
                .await
                .map_err(http_error)?;
            Ok(match nick {
                Some(n) => format!("별명을 {n}(으)로 바꿨다"),
                None => "별명을 지웠다".into(),
            })
        }
        Manage::RoleAdd { guild, user, role } => {
            c.add_guild_member_role(id(*guild)?, id(*user)?, id(*role)?)
                .await
                .map_err(http_error)?;
            Ok("역할을 줬다".into())
        }
        Manage::RoleRemove { guild, user, role } => {
            c.remove_guild_member_role(id(*guild)?, id(*user)?, id(*role)?)
                .await
                .map_err(http_error)?;
            Ok("역할을 뺐다".into())
        }
        Manage::DeleteMessage {
            channel, message, ..
        } => {
            c.delete_message(id(*channel)?, id(*message)?)
                .await
                .map_err(http_error)?;
            Ok("메시지를 지웠다".into())
        }
        Manage::Pin {
            channel, message, ..
        } => {
            c.create_pin(id(*channel)?, id(*message)?)
                .await
                .map_err(http_error)?;
            Ok("메시지를 고정했다".into())
        }
        Manage::Unpin {
            channel, message, ..
        } => {
            c.delete_pin(id(*channel)?, id(*message)?)
                .await
                .map_err(http_error)?;
            Ok("고정을 풀었다".into())
        }
        Manage::ChannelCreate {
            guild,
            name,
            kind,
            parent,
            topic,
        } => {
            use twilight_model::channel::ChannelType;
            let mut req = c
                .create_guild_channel(id(*guild)?, name.trim())
                .kind(match kind {
                    NewChannelKind::Text => ChannelType::GuildText,
                    NewChannelKind::Voice => ChannelType::GuildVoice,
                    NewChannelKind::Category => ChannelType::GuildCategory,
                });
            if let Some(p) = parent {
                req = req.parent_id(id(*p)?);
            }
            if let Some(t) = topic.as_deref().filter(|t| !t.trim().is_empty()) {
                req = req.topic(t);
            }
            let ch = req
                .await
                .map_err(http_error)?
                .model()
                .await
                .map_err(body_error)?;
            Ok(format!("채널을 만들었다(ID {})", ch.id))
        }
        Manage::ChannelRename { channel, name, .. } => {
            c.update_channel(id(*channel)?)
                .name(name.trim())
                .await
                .map_err(http_error)?;
            Ok("채널 이름을 바꿨다".into())
        }
        Manage::ChannelTopic { channel, topic, .. } => {
            c.update_channel(id(*channel)?)
                .topic(topic)
                .await
                .map_err(http_error)?;
            Ok("채널 주제를 바꿨다".into())
        }
        Manage::Slowmode {
            channel, seconds, ..
        } => {
            c.update_channel(id(*channel)?)
                .rate_limit_per_user(*seconds)
                .await
                .map_err(http_error)?;
            Ok(if *seconds == 0 {
                "느린 모드를 껐다".into()
            } else {
                format!("느린 모드를 {seconds}초로 했다")
            })
        }
        Manage::ChannelDelete { channel, .. } => {
            c.delete_channel(id(*channel)?).await.map_err(http_error)?;
            Ok("채널을 지웠다".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manage_round_trips_and_checks_limits() {
        let m = Manage::Timeout {
            guild: 1,
            user: 2,
            minutes: 30,
            reason: Some("도배".into()),
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["action"], "timeout");
        assert_eq!(serde_json::from_value::<Manage>(v).unwrap(), m);
        assert!(m.validate().is_ok());
        assert_eq!((m.guild(), m.user(), m.channel()), (1, Some(2), None));
        assert!(
            Manage::Timeout {
                guild: 1,
                user: 2,
                minutes: TIMEOUT_MAX_MINUTES + 1,
                reason: None
            }
            .validate()
            .is_err()
        );
        assert!(
            Manage::ChannelCreate {
                guild: 1,
                name: " ".into(),
                kind: NewChannelKind::Text,
                parent: None,
                topic: None
            }
            .validate()
            .is_err()
        );
        assert!(
            Manage::Slowmode {
                guild: 1,
                channel: 3,
                seconds: 21_601
            }
            .validate()
            .is_err()
        );
        assert!(
            Manage::ChannelDelete {
                guild: 1,
                channel: 3
            }
            .destructive()
        );
    }
}
