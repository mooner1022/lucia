//! 주 모델 고르기(결정 0055): 상주 세션을 쓰는 실행(대화, 브리핑, 보고, 신호, 루틴, 작업 세션)이 쓰는 모델을
//! 사용자가 관리 페이지나 `/모델` 명령으로 고른다. 고르지 않았으면 설정 파일 `[models]`의 `chat`·`briefing`을
//! 그대로 쓴다. 회고·신호 분류의 작은 모델(`retro`, `classify`)은 여기서 바꾸지 않는다.
//!
//! 고른 값은 저장소 설정([`KEY`])에 두고, 실행 경로는 메모리의 사본만 읽는다(저장소 잠금을 쥔 채 모델 이름을
//! 읽는 자리가 있어, 읽을 때 저장소를 다시 잠그지 않는다). 바꾸는 길은 관리 페이지와 소유자의 명령뿐이고
//! 모델 도구는 없다. 설정이 깨졌거나 모르는 값이면 고르지 않은 것으로 본다.

use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use lucia_store::Store;

/// 설정 키. 값은 [`Choice::key`]. 없으면 설정 파일 값을 쓴다.
pub const KEY: &str = "models.main";

/// 고를 수 있는 모델 하나. `key`는 Claude Code의 모델 별칭이라, 그 계열의 최신 모델을 따라간다.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct Choice {
    pub key: &'static str,
    pub label: &'static str,
    pub about: &'static str,
}

/// 고를 수 있는 모델(유능한 것부터). `claude --model`이 받는 별칭만 둔다.
pub const CHOICES: [Choice; 4] = [
    Choice {
        key: "fable",
        label: "Fable",
        about: "가장 유능합니다. 어려운 판단과 긴 작업에 맞고, 사용량을 가장 많이 씁니다.",
    },
    Choice {
        key: "opus",
        label: "Opus",
        about: "복잡한 일에 맞습니다. Sonnet보다 사용량을 더 씁니다.",
    },
    Choice {
        key: "sonnet",
        label: "Sonnet",
        about: "빠르고 가볍습니다. 일상 대화와 보통 작업에 맞습니다.",
    },
    Choice {
        key: "haiku",
        label: "Haiku",
        about: "가장 빠르고 가볍습니다. 복잡한 일에는 덜 맞습니다.",
    },
];

/// "설정 파일 값으로"를 뜻하는 말.
pub const DEFAULT_WORDS: [&str; 3] = ["default", "기본", "기본값"];

/// 별칭이나 이름으로 찾는다(대소문자 무시).
pub fn parse(s: &str) -> Option<&'static Choice> {
    let s = s.trim();
    CHOICES
        .iter()
        .find(|c| c.key.eq_ignore_ascii_case(s) || c.label.eq_ignore_ascii_case(s))
}

/// 저장소에 남은 선택. 없거나 모르는 값이면 `None`(설정 파일 값을 쓴다).
pub fn stored(store: &Store) -> Option<&'static Choice> {
    match store.setting(KEY) {
        Ok(Some(v)) => {
            let found = parse(&v);
            if found.is_none() {
                tracing::warn!(value = %v, "모르는 주 모델 설정이라 설정 파일 값을 쓴다");
            }
            found
        }
        Ok(None) => None,
        Err(err) => {
            tracing::warn!(%err, "주 모델 설정을 읽지 못해 설정 파일 값을 쓴다");
            None
        }
    }
}

#[derive(serde::Serialize)]
struct Inner {
    /// 설정 파일의 대화 모델과 브리핑 모델(고르지 않았을 때 쓴다).
    chat: String,
    briefing: String,
    picked: RwLock<Option<&'static Choice>>,
}

/// 지금 쓰는 주 모델. 엔진, 관리 페이지, 명령 처리기가 한 벌을 나눠 쓴다.
#[derive(Clone, serde::Serialize)]
pub struct MainModel(#[serde(serialize_with = "arc_inner")] Arc<Inner>);

/// `Arc<T>`는 안의 값을 그대로 내보낸다(serde의 `rc` 기능을 켜지 않는다).
fn arc_inner<T: serde::Serialize, S: serde::Serializer>(
    v: &Arc<T>,
    s: S,
) -> Result<S::Ok, S::Error> {
    T::serialize(v, s)
}

impl MainModel {
    pub fn new(chat: String, briefing: String) -> Self {
        Self(Arc::new(Inner {
            chat,
            briefing,
            picked: RwLock::new(None),
        }))
    }

    /// 저장소의 선택을 읽어 들인다(데몬이 뜰 때 한 번).
    pub fn load(&self, store: &Store) {
        *self.0.picked.write().expect("model lock") = stored(store);
    }

    /// 사용자가 고른 모델. 고르지 않았으면 `None`.
    pub fn picked(&self) -> Option<&'static Choice> {
        *self.0.picked.read().expect("model lock")
    }

    /// 설정 파일의 대화 모델(고르지 않았을 때의 주 모델).
    pub fn config_default(&self) -> &str {
        &self.0.chat
    }

    /// 대화·작업·루틴 실행의 모델.
    pub fn chat(&self) -> String {
        self.picked()
            .map_or_else(|| self.0.chat.clone(), |c| c.key.to_owned())
    }

    /// 브리핑·보고·신호처럼 상주 세션을 함께 쓰는 정기 실행의 모델. 골랐으면 대화와 같은 모델을 쓴다
    /// (한 세션에서 모델이 번갈아 바뀌면 그때마다 캐시를 새로 쓴다).
    pub fn briefing(&self) -> String {
        self.picked()
            .map_or_else(|| self.0.briefing.clone(), |c| c.key.to_owned())
    }

    /// 지금 값을 사람이 읽을 말로.
    pub fn label(&self) -> String {
        match self.picked() {
            Some(c) => c.label.to_owned(),
            None => format!("기본({})", self.0.chat),
        }
    }

    /// 주 모델을 바꾸고 활동 기록에 남긴다(`to`가 `None`이면 설정 파일 값으로). 바뀌었으면 `true`.
    /// 다음 실행부터 쓴다. 도는 실행은 그대로 끝난다.
    pub fn set(
        &self,
        store: &Store,
        to: Option<&'static Choice>,
        by: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, String> {
        let before = self.label();
        match to {
            Some(c) => store.set_setting(KEY, c.key, now),
            None => store.delete_setting(KEY),
        }
        .map_err(|e| e.to_string())?;
        let changed = {
            let mut picked = self.0.picked.write().expect("model lock");
            let changed = *picked != to;
            *picked = to;
            changed
        };
        if changed {
            let after = self.label();
            tracing::info!(%before, %after, by, "주 모델을 바꿨다");
            let _ = store.add_activity(
                "main_model",
                &format!("주 모델: {before} → {after}({by})"),
                None,
                now,
            );
        }
        Ok(changed)
    }

    /// 명령 답에 쓰는 설명: 지금 모델과 고를 수 있는 것.
    pub fn describe(&self) -> String {
        let mut out = format!("주 모델: {}\n고를 수 있는 모델:", self.label());
        for c in &CHOICES {
            out.push_str(&format!("\n- {}: {}", c.label, c.about));
        }
        out.push_str(&format!(
            "\n- 기본: 설정 파일 값({})\n{CHANGE_NOTE}",
            self.0.chat
        ));
        out
    }
}

/// 바꿀 때 알아 둘 것(관리 페이지와 명령 답이 함께 쓴다).
pub const CHANGE_NOTE: &str = "바꾸면 다음 실행부터 적용됩니다. 대화는 이어지고, 바꾼 뒤 첫 실행은 새 모델이 지금까지의 대화를 처음부터 다시 읽어 평소보다 느리고 사용량이 더 듭니다.";

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> DateTime<Utc> {
        "2026-11-20T00:00:00Z".parse().unwrap()
    }

    #[test]
    fn unpicked_uses_the_config_models() {
        let m = MainModel::new("sonnet".into(), "haiku".into());
        assert_eq!(m.picked(), None);
        assert_eq!((m.chat(), m.briefing()), ("sonnet".into(), "haiku".into()));
        assert_eq!(m.label(), "기본(sonnet)");
    }

    #[test]
    fn a_pick_governs_chat_and_briefing_and_survives_a_restart() {
        let store = Store::open_in_memory().unwrap();
        let m = MainModel::new("sonnet".into(), "haiku".into());
        assert!(m.set(&store, parse("Opus"), "시험", at()).unwrap());
        assert!(!m.set(&store, parse("opus"), "시험", at()).unwrap());
        assert_eq!((m.chat(), m.briefing()), ("opus".into(), "opus".into()));
        assert_eq!(store.setting(KEY).unwrap().as_deref(), Some("opus"));

        // 다시 뜬 데몬은 저장소에서 읽는다.
        let again = MainModel::new("sonnet".into(), "haiku".into());
        again.load(&store);
        assert_eq!(again.chat(), "opus");

        // 설정 파일 값으로 되돌린다.
        assert!(again.set(&store, None, "시험", at()).unwrap());
        assert_eq!(store.setting(KEY).unwrap(), None);
        assert_eq!(again.briefing(), "haiku");
    }

    #[test]
    fn unknown_stored_value_falls_back_to_config() {
        let store = Store::open_in_memory().unwrap();
        store.set_setting(KEY, "--dangerously", at()).unwrap();
        let m = MainModel::new("sonnet".into(), "sonnet".into());
        m.load(&store);
        assert_eq!(m.picked(), None);
        assert_eq!(m.chat(), "sonnet");
    }

    #[test]
    fn parse_takes_alias_or_label_only() {
        assert_eq!(parse(" FABLE ").map(|c| c.key), Some("fable"));
        assert_eq!(parse("Haiku").map(|c| c.key), Some("haiku"));
        assert!(parse("claude-opus-5-5").is_none());
        assert!(parse("").is_none());
    }
}
