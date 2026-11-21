//! 권한 모드(결정 0040): 사용자가 대시보드나 Discord `/권한`으로 고르는 세 단계.
//!
//! - **기본**: 지금까지의 규칙(결정 0003·0008·0023·0028·0029·0039 등).
//! - **엄격**: 모든 실행에 더 묻는다([`STRICT_ITEMS`]).
//! - **해제**: 사람이 대화 중인 실행에서만, 가벼운 확인을 건너뛴다([`OPEN_ITEMS`]). 되돌릴 수 없이 바깥에 남는 일(발송·게시·결제·
//!   쉘의 push·릴리스 등, 결정 0059)은 해제에서도 승인 카드를 거친다. 사람 없는 실행(브리핑·후속
//!   조치·루틴·신호·작업 세션·코드 실행)은 기본 규칙 그대로다. 모드가 끝나도 남는 권한(사이트 위임, 루틴·감시·
//!   스킬 승인)은 해제에서도 카드로 묻는다.
//!
//! 바꾸는 길은 관리 페이지(로그인·CSRF), 소유자의 Discord 명령, 그리고 `security_mode` 범위를 준 앱 토큰의 명령
//! (결정 0043)뿐이다. 모델 도구는 없다. 설정이 없거나 깨졌거나
//! 읽지 못하면 기본이다(해제로 떨어지지 않는다). 이미 떠 있는 카드에는 소급하지 않는다.

use chrono::{DateTime, Utc};
use lucia_store::Store;

/// 설정 키.
pub const KEY: &str = "security.mode";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Mode {
    Strict,
    Standard,
    Open,
}

/// 엄격 모드가 더 묻는 것(관리 페이지·명령 답에 그대로 보인다) [제안].
pub const STRICT_ITEMS: &[&str] = &[
    "브라우저: 링크 이동·펼치기·스크롤 말고는 누르기·글 넣기·Enter를 모두 승인 카드로 묻는다",
    "사이트 위임의 \"로그인\"·\"동작\"을 쓰지 않는다(묻지 않고 다시 로그인하거나 폼을 내지 않는다. 로그인 게시판 감시의 자동 재로그인도 멈춘다). 읽기 위임은 그대로",
    "캘린더: 참석자가 없는 내 일정도 만들거나 고칠 때 카드로 묻는다",
    "나가는 글 검사: 대화 중에도 \"막기\"로 본다(사용자 자료가 실린 글·주소는 묻는다)",
    "반복 루틴: 새로 만들거나 고치면 모두 승인 카드를 거친다",
    "사람 없는 실행의 기억 쓰기: 갈래와 상관없이 보류 카드로 묻는다(하루 정리는 그대로)",
];

/// 해제 모드가 카드 없이 하는 것(대화 중인 실행만).
pub const OPEN_ITEMS: &[&str] = &[
    "내 캘린더 쓰기·지우기: 카드 없이 바로 하고 DM에 닫힌 카드로 남긴다",
    "브라우저: 무엇을 하는지 확인하지 못한 단추·자리 누르기 같은 가벼운 확인을 묻지 않는다. 나가는 글 검사의 \"막기\"로 생긴 승인도 건너뛴다",
    "저장된 로그인으로 다시 로그인할 때 묻지 않는다",
];

/// 해제에서도 그대로 두는 것.
pub const OPEN_KEEPS: &[&str] = &[
    "사람이 대화 중이 아닌 실행(아침 브리핑, 후속 조치, 반복 루틴, 메일 알림, 작업 세션, 코드 실행)은 기본 규칙",
    "되돌릴 수 없이 바깥에 남는 일은 해제에서도 승인 카드를 거친다(결정 0059): 메일 발송, Discord 글·서버 관리, 브라우저의 글 등록·폼 제출·파일 올리기, 결제·주문·송금·탈퇴·비밀번호 바꾸기, 쉘의 git push·릴리스·배포·게시",
    "사이트 위임·루틴·감시·스킬 승인처럼 모드가 끝나도 남는 권한은 카드로 묻는다",
    "인젝션 검사에 걸린 쓰기 보류, 비밀번호를 모델에 주지 않기, VM 격리와 네트워크 제한",
];

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Strict, Mode::Standard, Mode::Open];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "strict" | "엄격" => Self::Strict,
            "standard" | "기본" => Self::Standard,
            "open" | "해제" => Self::Open,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Standard => "standard",
            Self::Open => "open",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Strict => "엄격",
            Self::Standard => "기본",
            Self::Open => "해제",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Strict => "🔒",
            Self::Standard => "🛡️",
            Self::Open => "🔓",
        }
    }

    /// 한 줄 설명.
    pub fn summary(self) -> &'static str {
        match self {
            Self::Strict => {
                "더 묻는다: 브라우저 입력, 위임된 로그인·동작, 캘린더, 루틴, 사람 없는 기억 쓰기, 나가는 글"
            }
            Self::Standard => "지금까지의 규칙: 외부 쓰기·위험한 브라우저 동작은 카드로 묻는다",
            Self::Open => {
                "대화 중에는 가벼운 확인을 건너뛴다(되돌릴 수 없이 바깥에 남는 일과 사람 없는 실행은 그대로 묻는다)"
            }
        }
    }
}

/// 지금 모드. 설정이 없거나 깨졌거나 읽지 못하면 기본.
pub fn mode(store: &Store) -> Mode {
    store
        .setting(KEY)
        .ok()
        .flatten()
        .and_then(|v| Mode::parse(&v))
        .unwrap_or(Mode::Standard)
}

pub fn strict(store: &Store) -> bool {
    mode(store) == Mode::Strict
}

/// 모드를 바꾸고 활동 기록에 남긴다(`by`: 어디서 바꿨나). 바뀌었으면 `true`.
pub fn set_mode(store: &Store, to: Mode, by: &str, now: DateTime<Utc>) -> Result<bool, String> {
    let before = mode(store);
    store
        .set_setting(KEY, to.as_str(), now)
        .map_err(|e| e.to_string())?;
    let _ = store.add_activity(
        "security_mode",
        &format!("권한 모드: {} → {}({by})", before.label(), to.label()),
        None,
        now,
    );
    Ok(before != to)
}

/// 해제 모드에서 카드 없이 해도 되는 일회성 승인 종류(새 종류는 기본이 카드).
/// 메일 발송과 Discord 글·서버 관리는 뺐다(결정 0059, 사용자 지시 2026-11-20: 되돌릴 수 없이 바깥에 남는 일은 해제
/// 모드에서도 승인을 받는다). 남은 것은 내 캘린더 쓰기다.
pub const ONE_SHOT_KINDS: &[&str] = &[crate::calendar::KIND];

/// 승인 요청을 만들 때 한 번 정한다: 카드 없이 바로 할 것인가(해제, 사람이 대화 중, 일회성 종류).
pub fn auto_approve(store: &Store, kind: &str, human: bool) -> bool {
    human && mode(store) == Mode::Open && ONE_SHOT_KINDS.contains(&kind)
}

/// [`auto_approve`]로 정한 요청에 표시를 남긴다(데스크가 카드 대신 바로 한다).
pub fn mark_auto(store: &Store, approval: &str) {
    let _ = store.set_approval_auto(approval, Mode::Open.as_str());
}

/// 도구가 모델에게 돌려줄 말: 카드를 올렸는지, 해제 모드라 바로 하는지.
pub fn requested_note(auto: bool, card: &str) -> String {
    if auto {
        "권한 모드가 해제(대화 중)라 카드 없이 데몬이 바로 한다. 결과는 곧 [승인 결과]로 남는다. 사용자에게 카드를 누르라고 하지 않는다."
            .into()
    } else {
        card.to_owned()
    }
}

/// 모드 안내(명령 답·알림).
pub fn describe(m: Mode) -> String {
    let mut out = format!(
        "{} 권한 모드: **{}** — {}",
        m.icon(),
        m.label(),
        m.summary()
    );
    let items: &[&str] = match m {
        Mode::Strict => STRICT_ITEMS,
        Mode::Open => OPEN_ITEMS,
        Mode::Standard => &[],
    };
    for i in items {
        out.push_str("\n- ");
        out.push_str(i);
    }
    if m == Mode::Open {
        out.push_str("\n그대로 두는 것:");
        for i in OPEN_KEEPS {
            out.push_str("\n- ");
            out.push_str(i);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_defaults_to_standard_and_open_is_narrow() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        assert_eq!(mode(&store), Mode::Standard, "없으면 기본");
        store.set_setting(KEY, "nonsense", now).unwrap();
        assert_eq!(
            mode(&store),
            Mode::Standard,
            "깨졌으면 기본(해제로 떨어지지 않는다)"
        );
        assert!(set_mode(&store, Mode::Open, "시험", now).unwrap());
        assert!(!set_mode(&store, Mode::Open, "시험", now).unwrap());
        assert_eq!(mode(&store), Mode::Open);
        assert!(auto_approve(&store, crate::calendar::KIND, true));
        assert!(
            !auto_approve(&store, crate::calendar::KIND, false),
            "사람 없으면 카드"
        );
        // 되돌릴 수 없이 바깥에 남는 일은 해제에서도 카드다(결정 0059).
        for kind in [
            crate::approvals::KIND_MAIL_SEND,
            crate::discord_tools::KIND_DISCORD_ACTION,
            crate::actions::KIND_EXTERNAL_ACTION,
            crate::approvals::KIND_GIT_PUSH,
        ] {
            assert!(!auto_approve(&store, kind, true), "{kind}");
        }
        assert!(
            !auto_approve(&store, crate::approvals::KIND_WATCH_REVIEW, true),
            "남는 권한은 카드"
        );
        assert!(!auto_approve(
            &store,
            crate::delegation::KIND_SITE_GRANT,
            true
        ));
        set_mode(&store, Mode::Strict, "시험", now).unwrap();
        assert!(!auto_approve(
            &store,
            crate::approvals::KIND_MAIL_SEND,
            true
        ));
        assert!(
            store
                .recent_activity(5)
                .unwrap()
                .iter()
                .any(|a| a.kind == "security_mode" && a.summary.contains("해제 → 엄격"))
        );
        assert_eq!(Mode::parse("해제"), Some(Mode::Open));
    }
}
