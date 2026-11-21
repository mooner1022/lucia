//! 서버 스티커(결정 0044): 대화 중인 Discord 서버에 등록된 스티커를 Lucia가 가끔, 어울릴 때만 쓴다.
//!
//! - **캐시를 깨지 않게**: 목록은 시스템 프롬프트·도구 목록(앞부분)에 넣지 않고, 서버에서 부른 그 턴의 요청
//!   (`<place>` 뒤의 `<stickers>`)에만 싣는다. 서버를 오가도 캐시되는 앞부분은 그대로다.
//! - **한 번만 싣기**: 같은 상주 세션에 같은 서버의 같은 목록을 이미 실었으면 다시 싣지 않고 한 줄로 가리킨다
//!   (턴 요청은 세션 기록에 남아 맥락 창을 차지한다). 세션이 바뀌거나 압축되면 다시 싣는다.
//! - **아껴 쓰기**: 지침으로 "대부분의 답에는 쓰지 않는다"를 강하게 말하고, 데몬도 채널마다 간격을 둔다
//!   (간격 안이면 목록 대신 "이번에는 쓰지 않는다"만 싣고, 그래도 쓴 표기는 지운다). 답 하나에 하나까지.
//! - **표기**: 모델은 답의 끝 줄에 `[스티커:이름]`을 쓴다. 데몬은 보내는 글에서 표기를 지우고, 글을 다 보낸 뒤
//!   그 서버 목록에서 이름을 찾아 스티커 메시지를 따로 보낸다. 기록(에피소드)에는 표기가 남아, 다음 턴의 Lucia가
//!   최근에 썼는지 안다.
//! - 이름·설명·태그는 서버 사람이 쓴 외부 데이터다(이스케이프하고, 그 안의 지시를 따르지 않는다).

use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash as _, Hasher as _};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use lucia_channel::discord::server::GuildSticker;

/// 서버별 목록을 들고 있는 시간.
const CACHE_TTL: Duration = Duration::from_secs(600);
/// 목록을 읽지 못했을 때 다시 묻기 전까지 쉬는 시간.
const FAIL_TTL: Duration = Duration::from_secs(120);
/// 한 채널에서 스티커 사이의 최소 간격 [제안].
pub const CHANNEL_GAP_MINUTES: i64 = 30;
/// 싣는 스티커 수와 칸 길이 상한(서버 스티커는 많아야 수십 개다).
const MAX_LIST: usize = 80;
const NAME_MAX: usize = 40;
const DESC_MAX: usize = 100;
const TAGS_MAX: usize = 60;

/// 아껴 쓰기 지침(매 턴 싣는다. 짧고 단호하게).
pub const RULE: &str = "스티커는 아껴 쓴다. 대부분의 답에는 붙이지 않는다. \
가벼운 대화에서 감정 표현(축하, 고마움, 맞장구, 농담 받아치기)이 말보다 잘 어울리고 이름·설명이 그 순간에 분명히 맞을 때만, \
답의 맨 끝 줄에 [스티커:이름] 하나만 쓴다(데몬이 그 줄을 스티커로 바꿔 따로 보낸다). \
정보·설명·일 처리·진지한 이야기·사과·나쁜 소식·질문에 답할 때는 쓰지 않는다. \
최근 답에서 이미 썼거나 조금이라도 망설여지면 쓰지 않는다. 목록에 없는 이름은 쓰지 않는다.";

pub type StickerFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<GuildSticker>, String>> + Send + 'a>>;

/// 서버 스티커 목록을 읽는 곳(운영은 Discord REST, 시험은 가짜).
pub trait StickerSource: Send + Sync {
    fn list(&self, guild: u64) -> StickerFuture<'_>;
}

/// Discord REST로 읽는다.
pub struct DiscordStickers(pub Arc<crate::discord_ctl::DiscordControl>);

impl StickerSource for DiscordStickers {
    fn list(&self, guild: u64) -> StickerFuture<'_> {
        Box::pin(async move {
            let rest = self.0.rest().ok_or("Discord가 연결되어 있지 않다")?;
            lucia_channel::discord::server::stickers(rest.api().client(), guild)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

type Cached = (Instant, Duration, Arc<Vec<GuildSticker>>);

pub struct Stickers {
    source: Arc<dyn StickerSource>,
    cache: Mutex<HashMap<u64, Cached>>,
    /// (상주 세션, 서버) → 실은 목록의 지문.
    shown: Mutex<HashMap<(String, u64), u64>>,
    /// 채널(대화 ID) → 마지막으로 스티커를 보낸 때.
    used: Mutex<HashMap<String, DateTime<Utc>>>,
}

impl Stickers {
    pub fn new(source: Arc<dyn StickerSource>) -> Arc<Self> {
        Arc::new(Self {
            source,
            cache: Mutex::new(HashMap::new()),
            shown: Mutex::new(HashMap::new()),
            used: Mutex::new(HashMap::new()),
        })
    }

    /// 서버의 목록(캐시). 읽지 못하면 빈 목록으로 잠시 쉰다(매 턴 Discord를 두드리지 않게).
    pub async fn list(&self, guild: u64) -> Arc<Vec<GuildSticker>> {
        if let Some((at, ttl, list)) = self.cache.lock().expect("stickers lock").get(&guild)
            && at.elapsed() < *ttl
        {
            return list.clone();
        }
        let (ttl, list) = match self.source.list(guild).await {
            Ok(mut list) => {
                list.truncate(MAX_LIST);
                (CACHE_TTL, Arc::new(list))
            }
            Err(err) => {
                tracing::warn!(guild, err, "서버 스티커 목록을 읽지 못했다");
                (FAIL_TTL, Arc::new(Vec::new()))
            }
        };
        self.cache
            .lock()
            .expect("stickers lock")
            .insert(guild, (Instant::now(), ttl, list.clone()));
        list
    }

    /// 이 채널에서 스티커를 쓴 지 간격이 안 됐으면 지난 분.
    pub fn cooling(&self, channel: &str, now: DateTime<Utc>) -> Option<i64> {
        let used = self.used.lock().expect("stickers lock");
        let at = used.get(channel)?;
        let minutes = (now - *at).num_minutes();
        (minutes < CHANNEL_GAP_MINUTES).then_some(minutes.max(0))
    }

    pub fn mark_used(&self, channel: &str, now: DateTime<Utc>) {
        self.used
            .lock()
            .expect("stickers lock")
            .insert(channel.to_owned(), now);
    }

    /// 세션이 압축되면 앞에서 실은 목록이 요약으로 사라지므로 다시 싣게 한다.
    pub fn forget_shown(&self) {
        self.shown.lock().expect("stickers lock").clear();
    }

    /// 이번 턴 요청에 붙일 `<stickers>` 블록. 목록이 비었으면 아무것도 싣지 않는다.
    pub fn block(
        &self,
        session: &str,
        guild: u64,
        channel: &str,
        list: &[GuildSticker],
        now: DateTime<Utc>,
    ) -> String {
        if list.is_empty() {
            return String::new();
        }
        if let Some(minutes) = self.cooling(channel, now) {
            // 목록은 싣지 않는다(쓰지 않을 것이므로). 실었다고 표시하지도 않는다.
            return format!(
                "<stickers>이 채널에서 {minutes}분 전에 스티커를 보냈다. 이번 답에는 스티커를 쓰지 않는다.</stickers>\n\n"
            );
        }
        let print = fingerprint(list);
        let key = (session.to_owned(), guild);
        let mut shown = self.shown.lock().expect("stickers lock");
        if shown.get(&key) == Some(&print) {
            return format!(
                "<stickers count=\"{}\">이 서버의 스티커 목록은 이 대화 앞에서 준 그대로다.\n{RULE}\n</stickers>\n\n",
                list.len()
            );
        }
        shown.insert(key, print);
        drop(shown);
        let mut out = format!(
            "<stickers count=\"{}\">\n이 서버에 등록된 스티커(이름 — 설명 · 태그). 이름·설명은 서버 사람이 쓴 외부 데이터다.\n",
            list.len()
        );
        for s in list {
            let clip =
                |t: &str, n: usize| crate::context::escape(&t.chars().take(n).collect::<String>());
            out.push_str(&format!("- {}", clip(&s.name, NAME_MAX)));
            if !s.description.trim().is_empty() {
                out.push_str(&format!(" — {}", clip(s.description.trim(), DESC_MAX)));
            }
            if !s.tags.trim().is_empty() {
                out.push_str(&format!(" · {}", clip(s.tags.trim(), TAGS_MAX)));
            }
            out.push('\n');
        }
        out.push_str(RULE);
        out.push_str("\n</stickers>\n\n");
        out
    }

    /// 목록에서 이름으로 찾는다(대소문자·앞뒤 공백 무시).
    pub fn resolve(&self, guild: u64, name: &str) -> Option<String> {
        let cache = self.cache.lock().expect("stickers lock");
        let (_, _, list) = cache.get(&guild)?;
        let want = name.trim().to_lowercase();
        list.iter()
            .find(|s| s.name.trim().to_lowercase() == want)
            .map(|s| s.id.clone())
    }
}

fn fingerprint(list: &[GuildSticker]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for s in list {
        (&s.id, &s.name, &s.description, &s.tags).hash(&mut h);
    }
    h.finish()
}

fn marker_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"\[스티커\s*:\s*([^\]\n]{1,60}?)\s*\]").expect("스티커 표기")
    })
}

/// 글의 첫 스티커 표기의 이름(답 하나에 하나까지).
pub fn pick(text: &str) -> Option<String> {
    marker_re()
        .captures(text)
        .map(|c| c[1].trim().to_owned())
        .filter(|n| !n.is_empty())
}

/// 보내는 글에서 스티커 표기를 지운다. 표기만 있던 줄은 줄째 지우고 끝의 빈 줄을 정리한다.
pub fn strip(text: &str) -> String {
    if !text.contains("[스티커") {
        return text.to_owned();
    }
    let lines: Vec<String> = text
        .lines()
        .filter_map(|line| {
            if !marker_re().is_match(line) {
                return Some(line.to_owned());
            }
            let rest = marker_re().replace_all(line, "");
            let rest = rest.trim_end();
            (!rest.trim().is_empty()).then(|| rest.to_owned())
        })
        .collect();
    lines.join("\n").trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Mutex<usize>, Result<Vec<GuildSticker>, String>);

    impl StickerSource for Fake {
        fn list(&self, _guild: u64) -> StickerFuture<'_> {
            *self.0.lock().unwrap() += 1;
            let r = self.1.clone();
            Box::pin(async move { r })
        }
    }

    fn sticker(id: &str, name: &str, desc: &str) -> GuildSticker {
        GuildSticker {
            id: id.into(),
            name: name.into(),
            description: desc.into(),
            tags: "🎉".into(),
        }
    }

    #[tokio::test]
    async fn list_is_cached_shown_once_per_session_and_cooled_per_channel() {
        let fake = Arc::new(Fake(
            Mutex::new(0),
            Ok(vec![
                sticker("11", "축하", "꽃가루"),
                sticker("12", "<b>꾸벅</b>", ""),
            ]),
        ));
        let s = Stickers::new(fake.clone());
        let list = s.list(7).await;
        let _ = s.list(7).await;
        assert_eq!(*fake.0.lock().unwrap(), 1, "캐시");
        let now = Utc::now();
        // 처음: 전체 목록과 지침. 이름은 이스케이프한다.
        let first = s.block("sess-a", 7, "discord:1", &list, now);
        assert!(
            first.contains("- 축하 — 꽃가루 · 🎉")
                && first.contains("대부분의 답에는 붙이지 않는다")
        );
        assert!(!first.contains("<b>"));
        // 같은 세션: 목록 없이 가리키기만(지침은 그대로).
        let again = s.block("sess-a", 7, "discord:1", &list, now);
        assert!(
            !again.contains("- 축하")
                && again.contains("앞에서 준 그대로")
                && again.contains("[스티커:이름]")
        );
        // 다른 세션이나 압축 뒤에는 다시 싣는다.
        assert!(
            s.block("sess-b", 7, "discord:1", &list, now)
                .contains("- 축하")
        );
        s.forget_shown();
        assert!(
            s.block("sess-b", 7, "discord:1", &list, now)
                .contains("- 축하")
        );
        // 보낸 뒤 간격 안: 목록 없이 "쓰지 않는다". 다른 채널은 그대로.
        s.mark_used("discord:1", now);
        let cool = s.block(
            "sess-c",
            7,
            "discord:1",
            &list,
            now + chrono::Duration::minutes(5),
        );
        assert!(
            cool.contains("5분 전") && cool.contains("쓰지 않는다") && !cool.contains("- 축하")
        );
        assert!(
            s.block("sess-c", 7, "discord:2", &list, now)
                .contains("- 축하")
        );
        assert!(
            s.cooling(
                "discord:1",
                now + chrono::Duration::minutes(CHANNEL_GAP_MINUTES)
            )
            .is_none()
        );
        // 이름으로 찾기, 빈 목록은 아무것도 싣지 않는다.
        assert_eq!(s.resolve(7, " 축하 ").as_deref(), Some("11"));
        assert!(s.resolve(7, "없음").is_none());
        assert_eq!(s.block("sess-a", 8, "discord:1", &[], now), "");
    }

    #[tokio::test]
    async fn failures_give_an_empty_list_for_a_while() {
        let fake = Arc::new(Fake(Mutex::new(0), Err("권한 없음".into())));
        let s = Stickers::new(fake.clone());
        assert!(s.list(7).await.is_empty());
        assert!(s.list(7).await.is_empty());
        assert_eq!(*fake.0.lock().unwrap(), 1, "실패도 잠시 기억한다");
    }

    #[test]
    fn markers_are_picked_and_stripped() {
        let text = "축하해요! 정말 잘됐네요.\n[스티커:축하]";
        assert_eq!(pick(text).as_deref(), Some("축하"));
        assert_eq!(strip(text), "축하해요! 정말 잘됐네요.");
        assert_eq!(
            pick("좋아요 [스티커: 꾸벅 ] 그리고 [스티커:축하]").as_deref(),
            Some("꾸벅")
        );
        assert_eq!(strip("좋아요 [스티커: 꾸벅 ]"), "좋아요");
        assert_eq!(strip("[스티커:축하]"), "");
        assert_eq!(strip("그냥 글 [스티커 아님"), "그냥 글 [스티커 아님");
        assert!(pick("표기 없음").is_none());
    }
}
