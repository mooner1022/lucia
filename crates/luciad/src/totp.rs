//! 관리 페이지 2단계 인증(결정 0043): RFC 6238 TOTP(HMAC-SHA1, 30초, 6자리). Google Authenticator 같은 앱과 맞는다.
//!
//! - 비밀(20바이트)은 자격 증명 파일 `admin-totp`(systemd-creds로 암호화)에 base32로 둔다. 화면에는 설정할 때 한 번만
//!   보인다.
//! - 앞뒤 한 칸(±30초)까지 받는다. 한 번 받은 칸과 그 앞의 칸은 다시 받지 않는다(가로챈 코드를 다시 쓰지 못하게).

use ring::hmac;

/// 자격 증명 이름.
pub const CREDENTIAL: &str = "admin-totp";
const STEP_SECS: i64 = 30;
const DIGITS: u32 = 6;
/// 비밀 길이(바이트, RFC 4226 권장 160비트).
const SECRET_BYTES: usize = 20;

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// 새 비밀(base32, 덧붙임 없음).
pub fn new_secret() -> String {
    use ring::rand::SecureRandom as _;
    let mut bytes = [0u8; SECRET_BYTES];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .expect("시스템 난수");
    base32_encode(&bytes)
}

pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for b in bytes {
        buffer = (buffer << 8) | u32::from(*b);
        bits += 8;
        while bits >= 5 {
            out.push(BASE32[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in text.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let v = BASE32
            .iter()
            .position(|b| *b as char == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push(((buffer >> (bits - 8)) & 0xff) as u8);
            bits -= 8;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// 시각(유닉스 초)이 속한 칸.
pub fn step_of(unix: i64) -> i64 {
    unix.div_euclid(STEP_SECS)
}

/// 한 칸의 코드.
pub fn code_at(secret: &[u8], step: i64) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &(step as u64).to_be_bytes());
    let h = tag.as_ref();
    let offset = (h[h.len() - 1] & 0x0f) as usize;
    let bin = (u32::from(h[offset]) & 0x7f) << 24
        | u32::from(h[offset + 1]) << 16
        | u32::from(h[offset + 2]) << 8
        | u32::from(h[offset + 3]);
    bin % 10u32.pow(DIGITS)
}

/// 코드를 확인한다. 맞으면 받은 칸을 돌려준다. `last_used` 이하의 칸은 받지 않는다.
pub fn verify(secret_b32: &str, code: &str, unix: i64, last_used: Option<i64>) -> Option<i64> {
    let code = code.trim().replace([' ', '-'], "");
    if code.len() != DIGITS as usize || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let want: u32 = code.parse().ok()?;
    let secret = base32_decode(secret_b32)?;
    let now = step_of(unix);
    // 앞뒤 한 칸. 같은 값이 둘 이상 맞을 일은 사실상 없지만, 늦은 칸부터 본다(재사용 막기가 덜 막히게).
    [now + 1, now, now - 1].into_iter().find(|step| {
        last_used.is_none_or(|last| *step > last) && constant_eq(code_at(&secret, *step), want)
    })
}

/// 값 전체를 한 번에 비교한다(앞자리부터 끊어 비교하지 않는다).
fn constant_eq(a: u32, b: u32) -> bool {
    std::hint::black_box(a ^ b) == 0
}

/// 인증 앱에 넣는 주소. 휴대폰에서 누르면 인증 앱이 열린다(QR 대신).
pub fn otpauth_uri(secret_b32: &str, account: &str) -> String {
    let enc = |s: &str| {
        percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
    };
    format!(
        "otpauth://totp/{}:{}?secret={secret_b32}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}",
        enc("Lucia"),
        enc(account),
        enc("Lucia")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 부록 B(SHA1, 비밀 "12345678901234567890")의 8자리 값의 끝 6자리.
    #[test]
    fn rfc6238_vectors() {
        let secret = b"12345678901234567890";
        for (t, want) in [
            (59, 94_287_082u32),
            (1_111_111_109, 7_081_804),
            (1_111_111_111, 14_050_471),
            (1_234_567_890, 89_005_924),
            (2_000_000_000, 69_279_037),
        ] {
            assert_eq!(code_at(secret, step_of(t)), want % 1_000_000, "t={t}");
        }
    }

    #[test]
    fn base32_roundtrip() {
        assert_eq!(
            base32_encode(b"12345678901234567890"),
            "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
        );
        assert_eq!(
            base32_decode("gezd gnbv gy3t qojq gezd gnbv gy3t qojq").unwrap(),
            b"12345678901234567890"
        );
        assert!(base32_decode("!!").is_none());
        let s = new_secret();
        assert_eq!(s.len(), 32);
        assert_eq!(base32_decode(&s).unwrap().len(), SECRET_BYTES);
    }

    #[test]
    fn verify_window_and_replay() {
        let secret = base32_encode(b"12345678901234567890");
        let t = 1_111_111_111;
        let code = format!("{:06}", code_at(b"12345678901234567890", step_of(t)));
        let step = verify(&secret, &code, t, None).expect("맞는 코드");
        assert_eq!(step, step_of(t));
        assert!(
            verify(&secret, &code, t + 30, None).is_some(),
            "30초 늦어도 받는다"
        );
        assert!(
            verify(&secret, &code, t + 90, None).is_none(),
            "90초 뒤는 받지 않는다"
        );
        assert!(
            verify(&secret, &code, t, Some(step)).is_none(),
            "같은 칸은 한 번만"
        );
        assert!(verify(&secret, "000000x", t, None).is_none());
        assert!(verify(&secret, "12345", t, None).is_none());
        let spaced = format!("{} {}", &code[..3], &code[3..]);
        assert!(
            verify(&secret, &spaced, t, None).is_some(),
            "띄어 써도 받는다"
        );
    }

    #[test]
    fn uri_has_secret_and_issuer() {
        let uri = otpauth_uri("ABC", "관리");
        assert!(uri.starts_with("otpauth://totp/Lucia:"));
        assert!(uri.contains("secret=ABC&issuer=Lucia"));
    }
}
