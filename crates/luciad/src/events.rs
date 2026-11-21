//! 이벤트 입구(결정 0031): 바깥에서 "바뀐 것이 있다"는 소식이 들어오는 길을 하나로 모은다.
//!
//! - 증분 확인: 관리 페이지에서 정한 간격(기본 5분)마다 사전 체크를 작업 큐에 넣는다([`run_poller`]).
//! - 접속 API의 기기 이벤트: `device` 역할 토큰이 보내는 `event` 프레임(결정 0021의 자리, 밑작업).
//! - 다른 통로(Gmail Pub/Sub, 웹훅 중계 등)를 붙일 때는 여기에 [`Poke::Check`]나 [`Poke::Event`]를 넣는 어댑터만
//!   더한다. 들어온 것은 모두 같은 신호 처리(`crate::signals`)를 거친다.

use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 관리 페이지 설정: 증분 확인 간격(분).
pub const POLL_MINUTES_KEY: &str = "events.poll_minutes";
/// 기본 간격 [제안].
pub const POLL_MINUTES_DEFAULT: u64 = 5;
pub const POLL_MINUTES_RANGE: std::ops::RangeInclusive<u64> = 1..=60;
/// 외부 이벤트 글 길이 상한(글자).
pub const EVENT_TEXT_MAX: usize = 1000;

/// 입구로 들어오는 것.
#[derive(Debug, Clone, PartialEq)]
pub enum Poke {
    /// 소스를 지금 확인하라(증분 확인, 또는 "새 메일이 있다"는 알림).
    Check,
    /// 사전 체크를 거치지 않는 외부 이벤트(기기 등). 사람이 아니므로 외부 데이터로 다룬다.
    Event(ExternalEvent),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternalEvent {
    /// 보낸 곳(기기 이름 등). 신호 소스는 `event:<보낸 곳>`.
    pub source: String,
    /// 같은 이벤트를 두 번 넣지 않기 위한 키.
    pub key: String,
    pub text: String,
    pub at: DateTime<Utc>,
}

impl ExternalEvent {
    /// 받을 수 있게 다듬는다. 빈 글이면 `None`.
    pub fn new(source: &str, key: Option<&str>, text: &str, at: DateTime<Utc>) -> Option<Self> {
        let text: String = text.trim().chars().take(EVENT_TEXT_MAX).collect();
        if text.is_empty() {
            return None;
        }
        let key = key
            .map(|k| k.trim().chars().take(120).collect::<String>())
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        Some(Self {
            source: source.trim().chars().take(60).collect(),
            key,
            text,
            at,
        })
    }
}

/// 지금 정한 증분 확인 간격.
pub fn poll_minutes(store: &lucia_store::Store) -> u64 {
    store
        .setting(POLL_MINUTES_KEY)
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|m| POLL_MINUTES_RANGE.contains(m))
        .unwrap_or(POLL_MINUTES_DEFAULT)
}

/// 증분 확인: 정한 간격마다 확인을 요청한다. 간격은 매번 다시 읽는다(관리 페이지에서 바꾸면 다음부터).
pub async fn run_poller(
    store: std::sync::Arc<std::sync::Mutex<lucia_store::Store>>,
    pokes: mpsc::UnboundedSender<Poke>,
    cancel: CancellationToken,
) {
    loop {
        let minutes = poll_minutes(&store.lock().expect("store lock"));
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(minutes * 60)) => {}
        }
        if pokes.send(Poke::Check).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_trimmed_and_keyed() {
        let now = Utc::now();
        assert!(ExternalEvent::new("안경", None, "   ", now).is_none());
        let e = ExternalEvent::new("안경", Some("k1"), &"가".repeat(2000), now).unwrap();
        assert_eq!(e.text.chars().count(), EVENT_TEXT_MAX);
        assert_eq!(e.key, "k1");
        let a = ExternalEvent::new("안경", None, "버튼", now).unwrap();
        let b = ExternalEvent::new("안경", None, "버튼", now).unwrap();
        assert_ne!(a.key, b.key, "키가 없으면 매번 새 이벤트");
    }

    #[test]
    fn poll_interval_comes_from_settings() {
        let store = lucia_store::Store::open_in_memory().unwrap();
        assert_eq!(poll_minutes(&store), POLL_MINUTES_DEFAULT);
        store
            .set_setting(POLL_MINUTES_KEY, "2", Utc::now())
            .unwrap();
        assert_eq!(poll_minutes(&store), 2);
        store
            .set_setting(POLL_MINUTES_KEY, "0", Utc::now())
            .unwrap();
        assert_eq!(
            poll_minutes(&store),
            POLL_MINUTES_DEFAULT,
            "범위 밖은 기본값"
        );
    }
}
