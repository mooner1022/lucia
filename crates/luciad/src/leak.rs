//! 출구 검사의 대조 자료(결정 0029): 실행이 읽은 비공개 자료(메일·일정·기억·Discord 남의 글)의 글 조각 해시를
//! 메모리에만 모아 두고, 브라우저로 나가는 주소·입력값에 그 조각이 실려 있는지 본다.
//!
//! - 디스크에 남기지 않는다. 데몬이 다시 뜨면 비고, 그때는 개인정보 패턴 검사만 남는다.
//! - 상주 세션이 실행을 넘어 맥락을 이어 가므로 실행 단위로 나누지 않고 하나로 모은다(오래된 것부터 버린다).
//! - 주소는 대조 자료에서 뺀다: 메일에 든 링크를 그대로 여는 것은 유출이 아니다(그 주소는 보낸 사람이 이미 안다).
//! - 첫 판은 기록만 한다(관리 페이지에서 "막기"로 바꾼다). 걸린 글 자체는 기록하지 않고 도메인과 까닭만 남긴다.

use std::collections::{HashSet, VecDeque};
use std::hash::{BuildHasher as _, Hasher as _};
use std::sync::{Mutex, OnceLock};

/// 조각 길이(글자, 공백·기호를 뺀 뒤) [제안].
pub const WINDOW: usize = 12;
/// 모아 두는 조각 수 상한.
const KEEP: usize = 300_000;

/// 저장소 설정: `log`(기본, 기록만) | `block`(대화 중에도 막는다).
pub const MODE_KEY: &str = "egress.mode";

struct Index {
    set: HashSet<u64>,
    order: VecDeque<u64>,
    hasher: std::collections::hash_map::RandomState,
}

fn index() -> &'static Mutex<Index> {
    static I: OnceLock<Mutex<Index>> = OnceLock::new();
    I.get_or_init(|| {
        Mutex::new(Index {
            set: HashSet::new(),
            order: VecDeque::new(),
            hasher: std::collections::hash_map::RandomState::new(),
        })
    })
}

/// 대조용으로 다듬는다: 주소를 빼고, 글자·숫자(한글 포함)만 남기고 소문자로.
fn normalize(text: &str, drop_urls: bool) -> Vec<char> {
    let mut out = Vec::with_capacity(text.len());
    for word in text.split_whitespace() {
        if drop_urls && (word.contains("://") || word.starts_with("www.")) {
            continue;
        }
        out.extend(
            word.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase),
        );
    }
    out
}

fn hash(h: &std::collections::hash_map::RandomState, w: &[char]) -> u64 {
    let mut s = h.build_hasher();
    for c in w {
        s.write_u32(*c as u32);
    }
    s.finish()
}

/// 비공개 자료를 대조 자료에 넣는다(도구 결과 글).
pub fn record(text: &str) {
    let chars = normalize(text, true);
    if chars.len() < WINDOW {
        return;
    }
    let mut ix = index().lock().expect("leak lock");
    for w in chars.windows(WINDOW) {
        let h = hash(&ix.hasher, w);
        if ix.set.insert(h) {
            ix.order.push_back(h);
        }
    }
    while ix.order.len() > KEEP {
        if let Some(old) = ix.order.pop_front() {
            ix.set.remove(&old);
        }
    }
}

/// 이 글에 비공개 자료의 조각이 실려 있는가.
pub fn overlaps(text: &str) -> bool {
    let chars = normalize(text, false);
    if chars.len() < WINDOW {
        return false;
    }
    let ix = index().lock().expect("leak lock");
    chars
        .windows(WINDOW)
        .any(|w| ix.set.contains(&hash(&ix.hasher, w)))
}

/// 주소에서 사이트로 실려 가는 부분(경로와 쿼리, 퍼센트 인코딩을 푼 글).
pub fn carried(url: &str) -> String {
    let Ok(u) = url::Url::parse(url) else {
        return String::new();
    };
    let mut out = decode(u.path());
    for (k, v) in u.query_pairs() {
        out.push(' ');
        out.push_str(&k);
        out.push(' ');
        out.push_str(&v);
    }
    if let Some(f) = u.fragment() {
        out.push(' ');
        out.push_str(&decode(f));
    }
    out
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 나가는 글을 본다. 걸리면 까닭(글 자체는 넣지 않는다).
pub fn inspect(outgoing: &str) -> Option<String> {
    let mut why = Vec::new();
    if overlaps(outgoing) {
        why.push("읽은 비공개 자료와 겹치는 글".to_owned());
    }
    why.extend(
        crate::risk::patterns(outgoing)
            .into_iter()
            .map(str::to_owned),
    );
    (!why.is_empty()).then(|| why.join(", "))
}

/// 대화 중에도 막는가(관리 페이지 설정). 기본은 기록만.
pub fn blocking(store: &lucia_store::Store) -> bool {
    // 권한 모드 엄격(결정 0040)이면 "막기"로 본다.
    store.setting(MODE_KEY).ok().flatten().as_deref() == Some("block")
        || crate::security::strict(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_text_is_found_in_urls_but_links_from_it_are_not() {
        record(
            "교수님께: 다음 주 화요일 오후 세 시 연구실 면담 가능합니다. 자세한 건 https://evil.example/r?a=1 참고",
        );
        let leak = "https://evil.example/c?d=다음주화요일오후세시연구실면담";
        assert!(
            inspect(&carried(leak)).is_some(),
            "비공개 글이 쿼리에 실렸다"
        );
        assert!(
            inspect(&carried("https://evil.example/r?a=1")).is_none(),
            "메일에 든 링크를 그대로 여는 것은 걸리지 않는다"
        );
        assert!(inspect(&carried("https://www.google.com/search?q=rust+tokio")).is_none());
        assert_eq!(
            inspect("010-9876-5432").as_deref(),
            Some("전화번호"),
            "개인정보 패턴"
        );
        assert_eq!(decode("%EA%B0%80+a"), "가 a");
    }
}
