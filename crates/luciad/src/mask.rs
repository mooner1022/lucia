//! 비밀 마스킹(명세 §인증과 비밀 관리: 로그 위생).
//!
//! 로그, 원장, 메신저로 가는 문자열은 모두 여기를 거친다. 두 가지를 가린다.
//! - 데몬이 실제로 쓰는 비밀 값(자격 증명을 읽을 때 [`Masker::register`]로 등록)
//! - 형태로 알아볼 수 있는 토큰(Anthropic, GitHub, Google, Discord 봇 토큰, Bearer 헤더 등)
//!
//! 가린 자리는 `[가림]`으로 바꾼다. 앞부분 몇 글자도 남기지 않는다.

use std::sync::{Arc, OnceLock, RwLock};

use regex::Regex;

pub const MASK: &str = "[가림]";

fn patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            // Anthropic API 키, OAuth 토큰.
            r"sk-ant-[A-Za-z0-9_\-]{8,}",
            // GitHub 토큰.
            r"\b(?:gh[pousr]|github_pat)_[A-Za-z0-9_]{16,}",
            // Google OAuth 액세스·리프레시 토큰, 클라이언트 시크릿.
            r"\bya29\.[A-Za-z0-9_\-\.]{16,}",
            r"\b1//[A-Za-z0-9_\-]{16,}",
            r"\bGOCSPX-[A-Za-z0-9_\-]{8,}",
            // Discord 봇 토큰(점으로 나뉜 세 덩어리).
            r"\b[A-Za-z0-9_\-]{23,28}\.[A-Za-z0-9_\-]{6,7}\.[A-Za-z0-9_\-]{27,}",
            // Authorization 헤더 값.
            r"(?i)\bbearer\s+[A-Za-z0-9_\-\.=+/]{8,}",
            // JSON·설정의 비밀 필드 값.
            r#"(?i)"(?:access_token|refresh_token|client_secret|id_token|password|token)"\s*:\s*"[^"]+""#,
        ]
        .iter()
        .map(|p| Regex::new(p).expect("마스킹 정규식"))
        .collect()
    })
}

/// 등록된 비밀 값과 토큰 형태를 가린다. 복제해 여러 곳에서 쓴다.
#[derive(Clone, Default)]
pub struct Masker {
    secrets: Arc<RwLock<Vec<String>>>,
}

impl Masker {
    /// 데몬이 쓰는 비밀 값을 등록한다. 너무 짧은 값(8자 미만)은 오탐이 많아 등록하지 않는다.
    pub fn register(&self, secret: &str) {
        let secret = secret.trim();
        if secret.chars().count() < 8 {
            return;
        }
        let mut secrets = self.secrets.write().expect("masker lock");
        if !secrets.iter().any(|s| s == secret) {
            secrets.push(secret.to_owned());
            // 긴 것부터 바꿔야 겹치는 값이 덜 새어 나간다.
            secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        }
    }

    pub fn mask(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in self.secrets.read().expect("masker lock").iter() {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), MASK);
            }
        }
        for re in patterns() {
            if re.is_match(&out) {
                out = re.replace_all(&out, MASK).into_owned();
            }
        }
        out
    }
}

/// 프로세스 전체에서 쓰는 마스커(tracing 출력 등 손이 닿기 어려운 곳용).
pub fn global() -> &'static Masker {
    static GLOBAL: OnceLock<Masker> = OnceLock::new();
    GLOBAL.get_or_init(Masker::default)
}

/// tracing 출력용 writer: 줄마다 마스킹한다.
pub struct MaskingWriter<W>(pub W);

impl<W: std::io::Write> std::io::Write for MaskingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        self.0.write_all(global().mask(&text).as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_registered_secrets_and_known_shapes() {
        let m = Masker::default();
        m.register("my-very-own-secret-value");
        m.register("short"); // 너무 짧아 무시
        let text = "값=my-very-own-secret-value, short는 남는다";
        assert_eq!(m.mask(text), format!("값={MASK}, short는 남는다"));

        // Discord 봇 토큰 모양의 가짜 값. 통째로 적어 두면 저장소의 비밀 검사가 진짜 토큰으로 보므로 조각을 이어 만든다.
        let discord = [
            "discord MTA5ODc2NTQz",
            "MjEwOTg3NjU0.",
            "GhIjKl.",
            "abcdefghijklmno",
            "pqrstuvwxyz0123",
        ]
        .concat();
        let cases = [
            "key sk-ant-api03-abcdefghijklmnop",
            "gh token gho_abcdefghijklmnopqrstu",
            "Authorization: Bearer abc.def.ghijklmnop",
            "google ya29.a0AfH6SMBabcdefghijklmn",
            "refresh 1//0gabcdefghijklmnopqrstuv",
            "secret GOCSPX-abcdefghijkl",
            r#"{"refresh_token": "abc123xyz", "expires_in": 3599}"#,
            &discord,
        ];
        for case in cases {
            let masked = m.mask(case);
            assert!(masked.contains(MASK), "가려야 한다: {case} → {masked}");
        }
        assert!(
            m.mask(r#"{"refresh_token": "abc123xyz", "expires_in": 3599}"#)
                .contains("expires_in")
        );
    }

    #[test]
    fn ordinary_text_is_untouched() {
        let m = Masker::default();
        for text in [
            "아침 브리핑을 만들지 못했습니다: error_max_budget_usd",
            "세션 3cdf3603-c5de-464c-9061-4b39b520a174 재개",
            "https://docs.example.com/path?x=1",
            "토큰 0개, 비용 $0.0012",
        ] {
            assert_eq!(m.mask(text), text);
        }
    }
}
