//! Discord 서버 도구(결정 0024).
//!
//! - 조회(읽기 등급): 봇이 들어간 서버, 채널, 메시지 기록, 역할, 멤버 찾기. 다른 사람이 쓴 글이므로 결과는
//!   외부 데이터로 감싼다.
//! - 글 올리기·서버 관리(외부 쓰기): 요청만 만든다. 데몬이 서버·대상 이름을 직접 알아내 고정하고 소유자 DM에
//!   승인 카드를 올린다. 사람이 누르면 데몬이 실행한다([`DiscordExec`]).
//! - 서버 관리는 봇이 그 서버에서 관리자 권한을 가졌을 때만 받는다. 요청할 때와 실행할 때 두 번 확인한다.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use lucia_channel::discord::server::{self, Cursor, Manage, NewChannelKind};
use lucia_store::{NewApproval, Store};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::discord_ctl::DiscordControl;

pub const KIND_DISCORD_ACTION: &str = "discord_action";

/// 도구가 쓰는 데몬 상태.
#[derive(Clone)]
pub struct DiscordAccess {
    pub control: Arc<DiscordControl>,
    pub store: Arc<Mutex<Store>>,
    /// 새 승인 요청 ID를 데스크로 보낸다(카드 올리기).
    pub posts: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    pub approval_ttl: chrono::Duration,
    pub tz: chrono_tz::Tz,
}

/// `discord_channels`·`discord_roles` 인자.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DiscordServerArg {
    /// 서버 ID(discord_servers의 id).
    pub server: String,
}

/// `discord_messages` 인자.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DiscordMessages {
    /// 채널(또는 스레드) ID.
    pub channel: String,
    /// 개수(기본 30, 최대 100).
    #[serde(default)]
    pub limit: Option<u8>,
    /// 이 메시지 ID보다 앞(오래된 쪽)을 읽는다.
    #[serde(default)]
    pub before: Option<String>,
    /// 이 메시지 ID보다 뒤를 읽는다.
    #[serde(default)]
    pub after: Option<String>,
    /// 이 메시지 ID 앞뒤를 읽는다.
    #[serde(default)]
    pub around: Option<String>,
}

/// `discord_members` 인자.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DiscordMembers {
    /// 서버 ID.
    pub server: String,
    /// 사용자 이름·별명의 앞부분.
    pub query: String,
    /// 개수(기본 20, 최대 100).
    #[serde(default)]
    pub limit: Option<u8>,
}

/// `discord_post` 인자.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DiscordPost {
    /// 올릴 채널(또는 스레드) ID.
    pub channel: String,
    /// 올릴 글(2,000자까지). 멘션은 알림 없이 글로만 보인다.
    pub text: String,
    /// 이 메시지 ID에 답장으로 올린다.
    #[serde(default)]
    pub reply_to: Option<String>,
}

/// 서버 관리 동작.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ManageVerb {
    /// 내보내기(user)
    Kick,
    /// 차단(user, delete_message_hours로 최근 메시지도 지움)
    Ban,
    /// 차단 풀기(user)
    Unban,
    /// 타임아웃(user, minutes)
    Timeout,
    /// 타임아웃 풀기(user)
    Untimeout,
    /// 별명 바꾸기(user, nick; 비우면 지움)
    Nick,
    /// 역할 주기(user, role)
    RoleAdd,
    /// 역할 빼기(user, role)
    RoleRemove,
    /// 메시지 지우기(channel, message)
    DeleteMessage,
    /// 메시지 고정(channel, message)
    Pin,
    /// 고정 풀기(channel, message)
    Unpin,
    /// 채널 만들기(name, kind, parent, topic)
    ChannelCreate,
    /// 채널 이름 바꾸기(channel, name)
    ChannelRename,
    /// 채널 주제 바꾸기(channel, topic)
    ChannelTopic,
    /// 느린 모드(channel, seconds; 0이면 끔)
    Slowmode,
    /// 채널 지우기(channel)
    ChannelDelete,
}

/// 만들 채널 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKindArg {
    Text,
    Voice,
    Category,
}

/// `discord_manage` 인자. 동작마다 필요한 칸만 채운다.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DiscordManage {
    /// 서버 ID.
    pub server: String,
    /// 동작과 필요한 칸: kick·unban·untimeout(user), ban(user, delete_message_hours 선택), timeout(user, minutes),
    /// nick(user, nick; 비우면 지움), role_add·role_remove(user, role), delete_message·pin·unpin(channel, message),
    /// channel_create(name, kind·parent·topic 선택), channel_rename(channel, name), channel_topic(channel, topic),
    /// slowmode(channel, seconds), channel_delete(channel). reason은 kick·ban·timeout에.
    pub action: ManageVerb,
    /// 대상 사용자 ID.
    #[serde(default)]
    pub user: Option<String>,
    /// 역할 ID(discord_roles).
    #[serde(default)]
    pub role: Option<String>,
    /// 채널 ID.
    #[serde(default)]
    pub channel: Option<String>,
    /// 메시지 ID.
    #[serde(default)]
    pub message: Option<String>,
    /// 새 채널 이름.
    #[serde(default)]
    pub name: Option<String>,
    /// 채널 주제.
    #[serde(default)]
    pub topic: Option<String>,
    /// 새 별명(비우면 지움).
    #[serde(default)]
    pub nick: Option<String>,
    /// 타임아웃 분(1~40320).
    #[serde(default)]
    pub minutes: Option<u32>,
    /// 느린 모드 초(0~21600).
    #[serde(default)]
    pub seconds: Option<u16>,
    /// 차단하며 지울 최근 메시지 시간(0~168).
    #[serde(default)]
    pub delete_message_hours: Option<u32>,
    /// 채널 종류(channel_create). 기본 text.
    #[serde(default)]
    pub kind: Option<ChannelKindArg>,
    /// 만들 채널의 분류(category) ID.
    #[serde(default)]
    pub parent: Option<String>,
    /// 감사 로그에 남길 이유(내보내기·차단·타임아웃).
    #[serde(default)]
    pub reason: Option<String>,
}

fn snowflake(what: &str, v: Option<&str>) -> Result<u64, String> {
    let v = v.map(str::trim).filter(|v| !v.is_empty());
    let Some(v) = v else {
        return Err(format!("{what} ID가 필요하다"));
    };
    // 멘션 표기(<@1>, <@!1>, <@&1>, <#1>)도 받는다.
    v.trim_start_matches('<')
        .trim_start_matches(['@', '!', '&', '#'])
        .trim_end_matches('>')
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("{what} ID는 숫자다: {v}"))
}

fn opt_snowflake(what: &str, v: Option<&str>) -> Result<Option<u64>, String> {
    match v.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => snowflake(what, Some(v)).map(Some),
    }
}

impl DiscordManage {
    /// 고정된 관리 동작으로 바꾼다(필요한 칸이 없으면 거절).
    pub fn to_manage(&self) -> Result<Manage, String> {
        let guild = snowflake("서버", Some(&self.server))?;
        let user = || snowflake("사용자(user)", self.user.as_deref());
        let role = || snowflake("역할(role)", self.role.as_deref());
        let channel = || snowflake("채널(channel)", self.channel.as_deref());
        let message = || snowflake("메시지(message)", self.message.as_deref());
        let reason = self.reason.clone().filter(|r| !r.trim().is_empty());
        use ManageVerb as V;
        let m = match self.action {
            V::Kick => Manage::Kick {
                guild,
                user: user()?,
                reason,
            },
            V::Ban => Manage::Ban {
                guild,
                user: user()?,
                reason,
                delete_message_seconds: self.delete_message_hours.unwrap_or(0).min(10_000) * 3600,
            },
            V::Unban => Manage::Unban {
                guild,
                user: user()?,
            },
            V::Timeout => Manage::Timeout {
                guild,
                user: user()?,
                minutes: self.minutes.ok_or("타임아웃 분(minutes)이 필요하다")?,
                reason,
            },
            V::Untimeout => Manage::Untimeout {
                guild,
                user: user()?,
            },
            V::Nick => Manage::Nick {
                guild,
                user: user()?,
                nick: self.nick.clone().filter(|n| !n.trim().is_empty()),
            },
            V::RoleAdd => Manage::RoleAdd {
                guild,
                user: user()?,
                role: role()?,
            },
            V::RoleRemove => Manage::RoleRemove {
                guild,
                user: user()?,
                role: role()?,
            },
            V::DeleteMessage => Manage::DeleteMessage {
                guild,
                channel: channel()?,
                message: message()?,
            },
            V::Pin => Manage::Pin {
                guild,
                channel: channel()?,
                message: message()?,
            },
            V::Unpin => Manage::Unpin {
                guild,
                channel: channel()?,
                message: message()?,
            },
            V::ChannelCreate => Manage::ChannelCreate {
                guild,
                name: self.name.clone().ok_or("채널 이름(name)이 필요하다")?,
                kind: match self.kind.unwrap_or(ChannelKindArg::Text) {
                    ChannelKindArg::Text => NewChannelKind::Text,
                    ChannelKindArg::Voice => NewChannelKind::Voice,
                    ChannelKindArg::Category => NewChannelKind::Category,
                },
                parent: opt_snowflake("분류(parent)", self.parent.as_deref())?,
                topic: self.topic.clone().filter(|t| !t.trim().is_empty()),
            },
            V::ChannelRename => Manage::ChannelRename {
                guild,
                channel: channel()?,
                name: self.name.clone().ok_or("새 이름(name)이 필요하다")?,
            },
            V::ChannelTopic => Manage::ChannelTopic {
                guild,
                channel: channel()?,
                topic: self.topic.clone().unwrap_or_default(),
            },
            V::Slowmode => Manage::Slowmode {
                guild,
                channel: channel()?,
                seconds: self.seconds.ok_or("느린 모드 초(seconds)가 필요하다")?,
            },
            V::ChannelDelete => Manage::ChannelDelete {
                guild,
                channel: channel()?,
            },
        };
        m.validate()?;
        Ok(m)
    }
}

fn when(t: Option<DateTime<Utc>>, tz: chrono_tz::Tz) -> String {
    t.map(|t| t.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// 메시지 기록을 글로(오래된 것부터). 내용이 모두 비어 있으면 intent 안내를 붙인다.
pub fn messages_text(list: &[server::Msg], tz: chrono_tz::Tz, bot_id: Option<u64>) -> String {
    if list.is_empty() {
        return "(메시지가 없다)".into();
    }
    let mut out = String::new();
    for m in list {
        let mut line = format!(
            "[{}] {} {}{}",
            m.id,
            when(m.at, tz),
            m.author,
            if Some(m.author_id) == bot_id {
                "(Lucia)"
            } else if m.bot {
                "(봇)"
            } else {
                ""
            }
        );
        line.push_str(&format!(" <{}>: ", m.author_id));
        line.push_str(&clip(&m.content.replace('\n', " / "), 1500));
        if !m.attachments.is_empty() {
            line.push_str(&format!(" (첨부: {})", m.attachments.join(", ")));
        }
        if m.embeds > 0 {
            line.push_str(&format!(" (임베드 {}개)", m.embeds));
        }
        if let Some(r) = m.reply_to {
            line.push_str(&format!(" (↪ {r}에 답장)"));
        }
        if m.edited {
            line.push_str(" (수정됨)");
        }
        out.push_str(&line);
        out.push('\n');
    }
    let others: Vec<_> = list
        .iter()
        .filter(|m| Some(m.author_id) != bot_id)
        .collect();
    if !others.is_empty()
        && others
            .iter()
            .all(|m| m.content.is_empty() && m.attachments.is_empty() && m.embeds == 0)
    {
        out.push_str("(다른 사람 글의 내용이 모두 비어 있다: Discord 개발자 포털에서 Message Content Intent가 꺼져 있으면 내용을 읽을 수 없다. 사용자에게 알린다.)\n");
    }
    out
}

/// 채널 목록을 분류 아래로 묶어 글로.
pub fn channels_text(list: &[server::ChannelInfo]) -> String {
    let mut out = String::new();
    let line = |c: &server::ChannelInfo| {
        let mut l = format!("[{}] {} ({})", c.id, c.name, c.kind);
        if let Some(t) = &c.topic {
            l.push_str(&format!(" — {}", clip(t, 120)));
        }
        l
    };
    for c in list
        .iter()
        .filter(|c| c.parent.is_none() && c.kind != "category")
    {
        out.push_str(&format!("{}\n", line(c)));
    }
    for cat in list.iter().filter(|c| c.kind == "category") {
        out.push_str(&format!("분류 {}\n", line(cat)));
        for c in list.iter().filter(|c| c.parent == Some(cat.id)) {
            out.push_str(&format!("  {}\n", line(c)));
        }
    }
    if out.is_empty() {
        out.push_str("(채널이 없거나 볼 수 없다)\n");
    }
    out
}

fn rest_err(e: lucia_channel::ChannelError) -> String {
    crate::mask::global().mask(&e.to_string())
}

impl DiscordAccess {
    fn client(
        &self,
    ) -> Result<
        Arc<lucia_channel::discord::DiscordAdapter<lucia_channel::discord::TwilightApi>>,
        String,
    > {
        self.control
            .rest()
            .ok_or_else(|| "Discord가 연결되어 있지 않다.".to_owned())
    }

    fn bot_id(&self) -> Option<u64> {
        self.control.bot_id()
    }

    pub async fn servers_text(&self) -> String {
        match self.control.servers(false).await {
            Ok(list) if list.is_empty() => "봇이 들어간 서버가 없다.".into(),
            Ok(list) => {
                let mut out = String::from(
                    "봇이 들어간 서버(관리 = 봇이 관리자 권한을 가져 discord_manage를 쓸 수 있다):\n",
                );
                for s in list {
                    out.push_str(&format!(
                        "- [{}] {}{}\n",
                        s.id,
                        s.name,
                        if s.admin { " · 관리" } else { "" }
                    ));
                }
                crate::mcp::external("discord", &out)
            }
            Err(e) => format!("서버 목록을 읽지 못했다: {e}"),
        }
    }

    pub async fn channels_text(&self, args: DiscordServerArg) -> String {
        let guild = match snowflake("서버", Some(&args.server)) {
            Ok(g) => g,
            Err(e) => return e,
        };
        let rest = match self.client() {
            Ok(r) => r,
            Err(e) => return e,
        };
        match server::channels(rest.api().client(), guild).await {
            Ok(list) => crate::mcp::external("discord", &channels_text(&list)),
            Err(e) => format!("채널 목록을 읽지 못했다: {}", rest_err(e)),
        }
    }

    /// `json`이면 스크립트용 JSON 배열(결정 0025).
    pub async fn messages_text(&self, args: DiscordMessages, json: bool) -> String {
        let channel = match snowflake("채널", Some(&args.channel)) {
            Ok(c) => c,
            Err(e) => return e,
        };
        let cursor = match (
            opt_snowflake("before", args.before.as_deref()),
            opt_snowflake("after", args.after.as_deref()),
            opt_snowflake("around", args.around.as_deref()),
        ) {
            (Ok(Some(b)), Ok(None), Ok(None)) => Cursor::Before(b),
            (Ok(None), Ok(Some(a)), Ok(None)) => Cursor::After(a),
            (Ok(None), Ok(None), Ok(Some(a))) => Cursor::Around(a),
            (Ok(None), Ok(None), Ok(None)) => Cursor::Latest,
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
            _ => return "before·after·around 중 하나만 쓴다.".into(),
        };
        let rest = match self.client() {
            Ok(r) => r,
            Err(e) => return e,
        };
        let c = rest.api().client();
        let (info, guild) = match server::channel(c, channel).await {
            Ok(x) => x,
            Err(e) => return format!("채널을 읽지 못했다: {}", rest_err(e)),
        };
        let limit = u16::from(args.limit.unwrap_or(30).clamp(1, 100));
        let list = match server::messages(c, channel, limit, cursor).await {
            Ok(l) => l,
            Err(e) => return format!("메시지를 읽지 못했다: {}", rest_err(e)),
        };
        if let Ok(store) = self.store.lock() {
            let _ = store.add_activity(
                "discord_read",
                &format!("Discord 기록 읽기: #{} {}개", info.name, list.len()),
                None,
                Utc::now(),
            );
        }
        if json {
            let bot = self.bot_id();
            return serde_json::Value::Array(
                list.iter()
                    .map(|m| {
                        serde_json::json!({
                            "id": m.id.to_string(),
                            "at": m.at.map(|t| t.to_rfc3339()),
                            "author": m.author,
                            "author_id": m.author_id.to_string(),
                            "bot": m.bot,
                            "lucia": Some(m.author_id) == bot,
                            "content": m.content,
                            "attachments": m.attachments,
                            "embeds": m.embeds,
                            "reply_to": m.reply_to.map(|r| r.to_string()),
                            "edited": m.edited,
                        })
                    })
                    .collect(),
            )
            .to_string();
        }
        let head = format!(
            "채널 #{} ({}{}), 오래된 것부터. 줄 모양: [메시지 ID] 시각 이름 <사용자 ID>: 내용\n",
            info.name,
            info.kind,
            guild.map(|g| format!(", 서버 {g}")).unwrap_or_default()
        );
        crate::mcp::external(
            "discord",
            &format!("{head}{}", messages_text(&list, self.tz, self.bot_id())),
        )
    }

    pub async fn roles_text(&self, args: DiscordServerArg) -> String {
        let guild = match snowflake("서버", Some(&args.server)) {
            Ok(g) => g,
            Err(e) => return e,
        };
        let rest = match self.client() {
            Ok(r) => r,
            Err(e) => return e,
        };
        match server::roles(rest.api().client(), guild).await {
            Ok(list) => {
                let text = list
                    .iter()
                    .map(|r| {
                        format!(
                            "- [{}] {}{}{}",
                            r.id,
                            r.name,
                            if r.admin { " · 관리자" } else { "" },
                            if r.managed {
                                " · 봇·연동 전용"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                crate::mcp::external("discord", &format!("역할(높은 것부터):\n{text}"))
            }
            Err(e) => format!("역할을 읽지 못했다: {}", rest_err(e)),
        }
    }

    pub async fn members_text(&self, args: DiscordMembers) -> String {
        let guild = match snowflake("서버", Some(&args.server)) {
            Ok(g) => g,
            Err(e) => return e,
        };
        let query = args.query.trim();
        if query.is_empty() {
            return "찾을 이름(query)이 필요하다.".into();
        }
        let rest = match self.client() {
            Ok(r) => r,
            Err(e) => return e,
        };
        let limit = u16::from(args.limit.unwrap_or(20).clamp(1, 100));
        match server::search_members(rest.api().client(), guild, query, limit).await {
            Ok(list) if list.is_empty() => format!("'{query}'로 시작하는 멤버가 없다."),
            Ok(list) => {
                let text = list
                    .iter()
                    .map(|m| {
                        let mut l = format!("- [{}] {} (@{})", m.id, m.display, m.username);
                        if m.bot {
                            l.push_str(" · 봇");
                        }
                        if !m.roles.is_empty() {
                            l.push_str(&format!(
                                " · 역할 {}",
                                m.roles
                                    .iter()
                                    .map(u64::to_string)
                                    .collect::<Vec<_>>()
                                    .join(",")
                            ));
                        }
                        if let Some(t) = m.joined_at {
                            l.push_str(&format!(" · 들어온 날 {}", when(Some(t), self.tz)));
                        }
                        if let Some(t) = m.timeout_until {
                            l.push_str(&format!(" · 타임아웃 {}까지", when(Some(t), self.tz)));
                        }
                        l
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                crate::mcp::external("discord", &text)
            }
            Err(e) => format!("멤버를 찾지 못했다: {}", rest_err(e)),
        }
    }

    fn approve(
        &self,
        kind_summary: String,
        payload: serde_json::Value,
        run: Option<&str>,
        human: bool,
    ) -> String {
        let now = Utc::now();
        let id = Uuid::new_v4().simple().to_string();
        let auto;
        {
            let Ok(store) = self.store.lock() else {
                return "저장소를 쓸 수 없다.".into();
            };
            if let Err(e) = store.create_approval(
                &NewApproval {
                    id: id.clone(),
                    kind: KIND_DISCORD_ACTION.into(),
                    payload,
                    summary: kind_summary.clone(),
                    run_id: run.map(str::to_owned),
                    expires_at: now + self.approval_ttl,
                },
                now,
            ) {
                return format!("요청하지 않았다(저장 실패): {e}");
            }
            let _ = store.add_activity(
                "approval_requested",
                &crate::mask::global().mask(&kind_summary),
                run,
                now,
            );
            auto = crate::security::auto_approve(&store, KIND_DISCORD_ACTION, human);
            if auto {
                crate::security::mark_auto(&store, &id);
            }
        }
        match &self.posts {
            Some(p) if p.send(id.clone()).is_ok() => crate::security::requested_note(
                auto,
                &format!(
                    "승인 요청을 올렸다(요청 {id}). 아직 하지 않았다. 사용자가 Discord DM의 카드에서 승인하면 데몬이 실행하고 결과는 카드에 표시된다. 사용자에게 카드를 확인해 달라고 보고하라."
                ),
            ),
            _ => "승인 데스크가 멈춰 있어 카드를 올리지 못했다. 사용자에게 알리라.".into(),
        }
    }

    /// 글 올리기 요청: 채널과 서버 이름을 알아내 고정하고 카드를 올린다.
    pub async fn request_post(&self, args: DiscordPost, run: Option<&str>, human: bool) -> String {
        let channel = match snowflake("채널", Some(&args.channel)) {
            Ok(c) => c,
            Err(e) => return e,
        };
        let reply_to = match opt_snowflake("답장할 메시지", args.reply_to.as_deref()) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let text = args.text.trim();
        if text.is_empty() || text.chars().count() > lucia_channel::discord::MAX_MESSAGE_CHARS {
            return "글은 1~2,000자여야 한다.".into();
        }
        let rest = match self.client() {
            Ok(r) => r,
            Err(e) => return e,
        };
        let c = rest.api().client();
        let (info, guild) = match server::channel(c, channel).await {
            Ok(x) => x,
            Err(e) => return format!("채널을 읽지 못해 요청하지 않았다: {}", rest_err(e)),
        };
        let Some(guild) = guild else {
            return "서버 채널에만 올린다(DM은 보고로 답한다).".into();
        };
        let server_name = self.server_name(guild).await;
        // 요약(활동 기록·결과 에피소드에 남는다)에는 남이 정한 이름 대신 ID만 쓴다.
        let summary = format!("Discord 글 올리기 → 서버 {guild} 채널 {channel}");
        let risks = self
            .store
            .lock()
            .map(|s| crate::risk::assess_post(&s, &channel.to_string(), text))
            .unwrap_or_default();
        self.approve(
            summary,
            serde_json::json!({
                "risks": risks,
                "op": "post",
                "guild": guild.to_string(),
                "channel": channel.to_string(),
                "reply_to": reply_to.map(|r| r.to_string()),
                "text": text,
                "shown": {"server": server_name, "channel": info.name},
            }),
            run,
            human,
        )
    }

    async fn server_name(&self, guild: u64) -> String {
        self.control
            .servers(false)
            .await
            .ok()
            .and_then(|l| l.into_iter().find(|s| s.id == guild))
            .map(|s| s.name)
            .unwrap_or_else(|| format!("서버 {guild}"))
    }

    /// 관리 요청: 봇의 관리자 권한을 확인하고, 대상 이름을 데몬이 알아내 카드에 고정한다.
    pub async fn request_manage(
        &self,
        args: DiscordManage,
        run: Option<&str>,
        human: bool,
    ) -> String {
        let m = match args.to_manage() {
            Ok(m) => m,
            Err(e) => return format!("요청하지 않았다: {e}"),
        };
        let guild = m.guild();
        let server = match admin_server(&self.control, guild).await {
            Ok(s) => s,
            Err(e) => return format!("요청하지 않았다: {e}"),
        };
        let rest = match self.client() {
            Ok(r) => r,
            Err(e) => return e,
        };
        let c = rest.api().client();
        let mut shown = serde_json::Map::new();
        shown.insert("server".into(), server.name.clone().into());
        if let Some(user) = m.user() {
            if Some(user) == self.bot_id() {
                return "요청하지 않았다: Lucia(봇) 자신은 대상으로 하지 않는다.".into();
            }
            let name = match server::member(c, guild, user).await {
                Ok(mem) => format!("{} (@{}, {})", mem.display, mem.username, mem.id),
                // 차단 풀기는 멤버가 아니다.
                Err(_) if matches!(m, Manage::Unban { .. } | Manage::Ban { .. }) => {
                    format!("사용자 {user}")
                }
                Err(e) => {
                    return format!(
                        "요청하지 않았다: 이 서버에서 사용자 {user}를 찾지 못했다({})",
                        rest_err(e)
                    );
                }
            };
            shown.insert("user".into(), name.into());
        }
        if let Some(role) = m.role() {
            let roles = match server::roles(c, guild).await {
                Ok(r) => r,
                Err(e) => return format!("요청하지 않았다: 역할을 읽지 못했다({})", rest_err(e)),
            };
            let Some(r) = roles.iter().find(|r| r.id == role) else {
                return format!("요청하지 않았다: 이 서버에 역할 {role}이 없다.");
            };
            if r.managed || r.id == guild {
                return "요청하지 않았다: 봇·연동 전용 역할이나 @everyone은 주고 뺄 수 없다."
                    .into();
            }
            shown.insert(
                "role".into(),
                format!(
                    "{}{}",
                    r.name,
                    if r.admin {
                        " (관리자 권한 역할)"
                    } else {
                        ""
                    }
                )
                .into(),
            );
        }
        if let Some(channel) = m.channel() {
            let (info, ch_guild) = match server::channel(c, channel).await {
                Ok(x) => x,
                Err(e) => return format!("요청하지 않았다: 채널을 읽지 못했다({})", rest_err(e)),
            };
            if ch_guild != Some(guild) {
                return "요청하지 않았다: 그 채널은 이 서버의 채널이 아니다.".into();
            }
            shown.insert(
                "channel".into(),
                format!("#{} ({})", info.name, info.kind).into(),
            );
        }
        if let Manage::DeleteMessage {
            channel, message, ..
        }
        | Manage::Pin {
            channel, message, ..
        }
        | Manage::Unpin {
            channel, message, ..
        } = &m
        {
            match server::message(c, *channel, *message).await {
                Ok(msg) => {
                    shown.insert(
                        "message".into(),
                        format!(
                            "{} ({}): {}",
                            msg.author,
                            when(msg.at, self.tz),
                            clip(&msg.content, 600)
                        )
                        .into(),
                    );
                }
                Err(e) => {
                    return format!("요청하지 않았다: 메시지를 읽지 못했다({})", rest_err(e));
                }
            }
        }
        // 요약(활동 기록·결과 에피소드에 남는다)에는 남이 정한 이름 대신 ID만 쓴다.
        let target = m
            .user()
            .map(|u| format!(" 사용자 {u}"))
            .or_else(|| m.channel().map(|c| format!(" 채널 {c}")))
            .unwrap_or_default();
        let summary = format!("Discord {}:{target} (서버 {guild})", m.label());
        self.approve(
            summary,
            serde_json::json!({
                "op": "manage",
                "manage": m,
                "shown": shown,
            }),
            run,
            human,
        )
    }
}

/// 봇이 관리자 권한을 가진 서버인지(늘 새로 묻는다).
pub async fn admin_server(control: &DiscordControl, guild: u64) -> Result<server::Server, String> {
    let list = control.servers(true).await?;
    match list.into_iter().find(|s| s.id == guild) {
        None => Err("봇이 들어간 서버가 아니다".into()),
        Some(s) if !s.admin => Err(format!(
            "봇이 '{}'에서 관리자 권한이 없어 서버 관리를 하지 않는다",
            s.name
        )),
        Some(s) => Ok(s),
    }
}

/// 승인된 Discord 동작을 실행한다(데스크가 부른다).
pub struct DiscordExec {
    pub control: Arc<DiscordControl>,
}

impl crate::approvals::DiscordActionExec for DiscordExec {
    fn run_approved<'a>(
        &'a self,
        payload: &'a serde_json::Value,
    ) -> crate::browser::BoxFut<'a, Result<String, String>> {
        Box::pin(async move {
            let rest = self
                .control
                .rest()
                .ok_or_else(|| "Discord가 연결되어 있지 않다".to_owned())?;
            let c = rest.api().client();
            match payload["op"].as_str() {
                Some("post") => {
                    let num = |k: &str| payload[k].as_str().and_then(|v| v.parse::<u64>().ok());
                    let channel = num("channel").ok_or("저장된 채널이 없다")?;
                    let text = payload["text"].as_str().unwrap_or("");
                    let id = server::post(c, channel, text, num("reply_to"))
                        .await
                        .map_err(rest_err)?;
                    Ok(format!("올렸다(메시지 {id})"))
                }
                Some("manage") => {
                    let m: Manage = serde_json::from_value(payload["manage"].clone())
                        .map_err(|e| format!("저장된 동작: {e}"))?;
                    // 그사이 권한이 바뀌었을 수 있다.
                    admin_server(&self.control, m.guild()).await?;
                    server::manage(c, &m).await.map_err(rest_err)
                }
                _ => Err("모르는 Discord 동작".into()),
            }
        })
    }
}

/// `luciad discord-check`: REST로만 읽는 점검. 메시지 내용은 출력하지 않고 수만 센다.
pub async fn check(cfg: &crate::config::Config) -> anyhow::Result<()> {
    use lucia_channel::discord::{DiscordApi as _, TwilightApi};
    let creds = crate::credentials::Credentials::new(
        cfg.credentials_dir.clone(),
        crate::mask::global().clone(),
    );
    let token = creds
        .read(crate::credentials::names::DISCORD_BOT_TOKEN)
        .await?
        .ok_or_else(|| anyhow::anyhow!("저장된 Discord 봇 토큰이 없다"))?;
    let api = TwilightApi::new(token);
    let bot = api.current_user().await?;
    println!("봇: {} ({})", bot.name, bot.id);
    // 봇의 지금 아바타(결정 0060): Lucia가 프로필을 바꾸기 전에 이것을 받아 보관한다. 여기서는 읽기만 한다.
    match api.avatar_url().await {
        Ok(Some(url)) => {
            let got = async {
                let r = reqwest::Client::new()
                    .get(&url)
                    .timeout(std::time::Duration::from_secs(20))
                    .send()
                    .await
                    .map_err(|e| e.without_url().to_string())?;
                if !r.status().is_success() {
                    return Err(format!("HTTP {}", r.status().as_u16()));
                }
                r.bytes().await.map_err(|e| e.without_url().to_string())
            }
            .await;
            match got {
                Ok(bytes) => match crate::profile::inspect(&bytes) {
                    Ok((mime, w, h)) => println!(
                        "아바타: 있음 · CDN에서 받음 {mime} {w}×{h} {}바이트(바꾸기 전에 보관할 수 있다)",
                        bytes.len()
                    ),
                    Err(why) => println!("아바타: 있음 · 받았지만 읽지 못함({why})"),
                },
                Err(why) => println!("아바타: 있음 · CDN에서 받지 못함({why})"),
            }
        }
        Ok(None) => println!("아바타: 없음(기본 모습)"),
        Err(e) => println!("아바타: 확인하지 못함({e})"),
    }
    let c = api.client();
    let list = server::servers(c).await?;
    println!("서버 {}개", list.len());
    for s in &list {
        let channels = server::channels(c, s.id).await;
        let (count, text_channels) = match &channels {
            Ok(l) => (
                l.len().to_string(),
                l.iter()
                    .filter(|c| c.kind == "text")
                    .map(|c| c.id)
                    .collect::<Vec<_>>(),
            ),
            Err(e) => (format!("읽지 못함({e})"), vec![]),
        };
        let mut sample = "읽을 글 채널 없음".to_owned();
        for ch in text_channels.iter().take(5) {
            match server::messages(c, *ch, 20, Cursor::Latest).await {
                Ok(msgs) if !msgs.is_empty() => {
                    let others: Vec<_> = msgs.iter().filter(|m| m.author_id != bot.id).collect();
                    let visible = others
                        .iter()
                        .filter(|m| !m.content.is_empty() || !m.attachments.is_empty())
                        .count();
                    sample = format!(
                        "채널 {ch}: 최근 {}개 중 다른 사람 글 {}개, 내용이 보이는 것 {visible}개",
                        msgs.len(),
                        others.len()
                    );
                    break;
                }
                Ok(_) => continue,
                Err(e) => sample = format!("채널 {ch}: 읽지 못함({e})"),
            }
        }
        let roles = match server::roles(c, s.id).await {
            Ok(r) => format!("역할 {}개", r.len()),
            Err(e) => format!("역할 읽지 못함({e})"),
        };
        // 멤버 찾기(GUILD_MEMBERS 없이 되는지)와 멤버 하나 읽기: 봇 자신으로 시험한다.
        let prefix: String = bot.name.chars().take(2).collect();
        let search = match server::search_members(c, s.id, &prefix, 5).await {
            Ok(m) => format!("멤버 찾기 됨({}명)", m.len()),
            Err(e) => format!("멤버 찾기 안 됨({e})"),
        };
        let one = match server::member(c, s.id, bot.id).await {
            Ok(m) => format!("멤버 읽기 됨(역할 {}개)", m.roles.len()),
            Err(e) => format!("멤버 읽기 안 됨({e})"),
        };
        println!(
            "- {} ({}){} · 채널 {count} · {roles} · {search} · {one} · {sample}",
            s.name,
            s.id,
            if s.admin { " · 관리자 권한" } else { "" }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manage(action: ManageVerb) -> DiscordManage {
        DiscordManage {
            server: "100".into(),
            action,
            user: None,
            role: None,
            channel: None,
            message: None,
            name: None,
            topic: None,
            nick: None,
            minutes: None,
            seconds: None,
            delete_message_hours: None,
            kind: None,
            parent: None,
            reason: None,
        }
    }

    #[test]
    fn manage_args_need_their_fields() {
        assert!(manage(ManageVerb::Kick).to_manage().is_err(), "사용자 필요");
        let mut a = manage(ManageVerb::Ban);
        a.user = Some("<@!42>".into());
        a.delete_message_hours = Some(24);
        assert_eq!(
            a.to_manage().unwrap(),
            Manage::Ban {
                guild: 100,
                user: 42,
                reason: None,
                delete_message_seconds: 86_400
            }
        );
        a.delete_message_hours = Some(200);
        assert!(a.to_manage().is_err(), "7일 넘게는 지우지 않는다");
        let mut t = manage(ManageVerb::Timeout);
        t.user = Some("42".into());
        assert!(t.to_manage().is_err(), "분 필요");
        t.minutes = Some(10);
        assert!(t.to_manage().is_ok());
        let mut c = manage(ManageVerb::ChannelCreate);
        c.name = Some("공지".into());
        c.parent = Some("<#77>".into());
        assert_eq!(
            c.to_manage().unwrap(),
            Manage::ChannelCreate {
                guild: 100,
                name: "공지".into(),
                kind: NewChannelKind::Text,
                parent: Some(77),
                topic: None
            }
        );
        let mut bad = manage(ManageVerb::ChannelDelete);
        bad.channel = Some("abc".into());
        assert!(bad.to_manage().is_err());
    }

    #[test]
    fn history_is_readable_and_flags_missing_content() {
        let tz = chrono_tz::Asia::Seoul;
        let at = DateTime::parse_from_rfc3339("2026-11-17T05:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let m = |id: u64, author: u64, content: &str| server::Msg {
            id,
            author_id: author,
            author: format!("사람{author}"),
            bot: author == 9,
            content: content.into(),
            attachments: vec![],
            embeds: 0,
            at: Some(at),
            edited: false,
            reply_to: None,
        };
        let text = messages_text(&[m(1, 42, "회의는\n3시"), m(2, 9, "알겠어요")], tz, Some(9));
        assert!(text.contains("[1] 2026-11-17 14:00 사람42 <42>: 회의는 / 3시"));
        assert!(text.contains("사람9(Lucia)"));
        assert!(!text.contains("Message Content"));
        let empty = messages_text(&[m(1, 42, ""), m(2, 9, "제 글")], tz, Some(9));
        assert!(empty.contains("Message Content Intent"), "{empty}");
    }

    #[test]
    fn channels_group_under_categories() {
        let ch = |id, name: &str, kind, parent| server::ChannelInfo {
            id,
            name: name.into(),
            kind,
            parent,
            position: 0,
            topic: None,
        };
        let text = channels_text(&[
            ch(1, "일반", "category", None),
            ch(2, "잡담", "text", Some(1)),
            ch(3, "규칙", "text", None),
        ]);
        assert_eq!(
            text,
            "[3] 규칙 (text)\n분류 [1] 일반 (category)\n  [2] 잡담 (text)\n"
        );
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    /// 한 번의 tool_search로 부를 수 있게: 동작 값 16개와 동작별 필요한 칸이 파라미터 설명에 보인다.
    #[test]
    fn manage_params_show_every_action_and_its_fields() {
        let schema = serde_json::to_value(schemars::schema_for!(DiscordManage)).unwrap();
        let text = crate::toolsearch::render_params(schema.as_object().unwrap());
        for v in [
            "kick",
            "ban",
            "unban",
            "timeout",
            "untimeout",
            "nick",
            "role_add",
            "role_remove",
            "delete_message",
            "pin",
            "unpin",
            "channel_create",
            "channel_rename",
            "channel_topic",
            "slowmode",
            "channel_delete",
        ] {
            assert!(text.contains(&format!("\"{v}\"")), "{v}: {text}");
        }
        assert!(text.contains("timeout(user, minutes)"), "{text}");
        assert!(text.contains("role_add·role_remove(user, role)"), "{text}");
    }
}
