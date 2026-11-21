//! 게시판 감시의 규칙(결정 0036): 글 목록에서 글 링크를 찾고, 지난번에 본 것과 견줘 새 글을 고른다.
//!
//! - **링크 패턴**: 글 주소의 모양. 예: `/bbs/board.php?bo_table=free&wr_id=*`, `/article/*`.
//!   - 경로는 마디마다 같거나(`*`면 아무 마디), 쿼리는 패턴에 적힌 이름만 본다(값이 `*`면 아무 값, 아니면 같아야).
//!   - 패턴에 없는 쿼리(`page`, `sca` 등)는 무시한다. 같은 글이 목록 쪽수에 따라 다른 주소가 되지 않게.
//!   - `*`가 잡은 값들이 글의 키다. 제목 뒤 댓글 수가 바뀌어도 키가 같으니 새 글이 아니다.
//! - **본 글**: 키의 해시를 [`SEEN_MAX`]개까지 기억한다(커서). 첫 확인은 기준선만.
//! - 페이지 글은 공격자가 쓸 수 있으므로, 여기서 나온 제목은 모델에게 갈 때 외부 데이터로 감싼다(부르는 쪽).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use url::Url;

/// 기억하는 본 글 수.
pub const SEEN_MAX: usize = 500;
/// 한 번에 이보다 많이 새로 보이면 목록이 통째로 바뀐 것(패턴이 깨짐, 개편)으로 보고 알리지 않는다 [제안].
pub const BURST_MAX: usize = 30;
/// 제목 길이.
pub const TITLE_MAX: usize = 120;
/// 패턴 길이.
pub const PATTERN_MAX: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPattern {
    host: Option<String>,
    path: Vec<String>,
    query: Vec<(String, String)>,
}

impl LinkPattern {
    /// 패턴을 읽는다. 상대 주소면 감시 주소의 호스트를 쓴다.
    pub fn parse(pattern: &str, base: &Url) -> Result<Self, String> {
        let pattern = pattern.trim();
        if pattern.is_empty() || pattern.chars().count() > PATTERN_MAX {
            return Err(format!("링크 패턴은 1~{PATTERN_MAX}자"));
        }
        // `*`가 퍼센트 인코딩되지 않게 자리표로 바꿔 읽는다.
        const STAR: &str = "LUCIASTAR";
        let joined = base
            .join(&pattern.replace('*', STAR))
            .map_err(|e| format!("링크 패턴을 읽지 못했다: {e}"))?;
        let host = joined.host_str().map(str::to_ascii_lowercase);
        let path: Vec<String> = joined
            .path_segments()
            .map(|s| {
                s.filter(|p| !p.is_empty())
                    .map(|p| decode(p).replace(STAR, "*"))
                    .collect()
            })
            .unwrap_or_default();
        let query: Vec<(String, String)> = joined
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.replace(STAR, "*")))
            .collect();
        let wild = path.iter().filter(|p| p.contains('*')).count()
            + query.iter().filter(|(_, v)| v.contains('*')).count();
        if wild == 0 {
            return Err("글마다 달라지는 부분(글 번호 등)을 *로 적는다".into());
        }
        if path.iter().any(|p| p.contains('*') && p != "*")
            || query.iter().any(|(_, v)| v.contains('*') && v != "*")
        {
            return Err("*는 경로 한 마디 전체나 쿼리 값 전체에만 쓴다".into());
        }
        Ok(Self { host, path, query })
    }

    /// 주소가 맞으면 글의 키(`*`가 잡은 값들).
    pub fn key(&self, url: &Url) -> Option<String> {
        if let Some(h) = &self.host
            && url.host_str().map(str::to_ascii_lowercase).as_deref() != Some(h.as_str())
        {
            return None;
        }
        let segs: Vec<String> = url
            .path_segments()
            .map(|s| s.filter(|p| !p.is_empty()).map(decode).collect())
            .unwrap_or_default();
        if segs.len() != self.path.len() {
            return None;
        }
        let mut caps = Vec::new();
        for (want, got) in self.path.iter().zip(&segs) {
            if want == "*" {
                caps.push(got.clone());
            } else if want != got {
                return None;
            }
        }
        for (k, want) in &self.query {
            let got = url
                .query_pairs()
                .find(|(name, _)| name == k.as_str())
                .map(|(_, v)| v.into_owned())
                .filter(|v| !v.is_empty())?;
            if want == "*" {
                caps.push(got);
            } else if *want != got {
                return None;
            }
        }
        Some(caps.join("/"))
    }
}

fn decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

/// 목록에서 찾은 글 하나.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub key: String,
    pub title: String,
    pub url: String,
}

/// 페이지의 링크(주소, 보이는 글). 상대 주소는 `base`로 푼다. 스크립트 링크·조각 링크는 뺀다.
pub fn links(html: &str, base: &Url) -> Vec<(Url, String)> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = lower[i..].find("<a") {
        let start = i + p;
        let after = lower.as_bytes().get(start + 2).copied().unwrap_or(b'>');
        if !(after.is_ascii_whitespace() || after == b'>') {
            i = start + 2;
            continue;
        }
        let Some(tag_end) = lower[start..].find('>').map(|q| start + q) else {
            break;
        };
        let tag = &html[start..tag_end];
        let close = lower[tag_end..]
            .find("</a")
            .map_or(html.len(), |q| tag_end + q);
        let inner = &html[tag_end + 1..close.max(tag_end + 1)];
        i = close;
        let Some(href) = attr(tag, "href") else {
            continue;
        };
        let href = decode_entities(href.trim());
        if href.is_empty()
            || href.starts_with('#')
            || href.to_ascii_lowercase().starts_with("javascript:")
        {
            continue;
        }
        let Ok(mut url) = base.join(&href) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        url.set_fragment(None);
        let text = visible(inner);
        let text = if text.is_empty() {
            attr(tag, "title").map(|t| visible(&t)).unwrap_or_default()
        } else {
            text
        };
        out.push((url, text));
    }
    out
}

/// 태그 안 속성 값(따옴표 있거나 없거나).
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(p) = lower[from..].find(name) {
        let at = from + p;
        from = at + name.len();
        let before = lower
            .as_bytes()
            .get(at.wrapping_sub(1))
            .copied()
            .unwrap_or(b' ');
        if at > 0 && !before.is_ascii_whitespace() {
            continue;
        }
        let rest = tag[at + name.len()..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
            _ => rest
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()
                .unwrap_or(""),
        };
        return Some(value.to_owned());
    }
    None
}

/// 태그를 빼고 보이는 글만(공백 하나로).
pub fn visible(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    decode_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// HTML 문자 참조를 푼다(이름 몇 개와 숫자).
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        // `;`는 바이트로 찾는다(12바이트째가 한글 글자 중간일 수 있다).
        let Some(end) = rest.as_bytes()[..rest.len().min(12)]
            .iter()
            .position(|b| *b == b';')
        else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 패턴에 맞는 글(페이지 순서, 키마다 하나). 같은 글의 링크가 여럿이면(제목, 댓글 수, 그림) 가장 긴 글을 제목으로.
pub fn items(html: &str, base: &Url, pattern: &LinkPattern) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    for (url, text) in links(html, base) {
        let Some(key) = pattern.key(&url) else {
            continue;
        };
        let text: String = text.chars().take(TITLE_MAX).collect();
        let meaningful = text.chars().any(char::is_alphabetic);
        match out.iter_mut().find(|i| i.key == key) {
            Some(i) => {
                if meaningful && text.chars().count() > i.title.chars().count() {
                    i.title = text;
                }
            }
            None => out.push(Item {
                key,
                title: if meaningful { text } else { String::new() },
                url: url.to_string(),
            }),
        }
    }
    for i in &mut out {
        if i.title.is_empty() {
            i.title = "(제목 없음)".into();
        }
    }
    out
}

/// 패턴이 맞지 않을 때 모델에게 보일 링크 예(같은 모양은 하나만, `n`개까지).
pub fn link_samples(html: &str, base: &Url, n: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (url, text) in links(html, base) {
        let shape = format!(
            "{}{}",
            url.host_str().unwrap_or(""),
            url.path_segments()
                .map(|s| s
                    .map(|p| if p.chars().any(|c| c.is_ascii_digit()) {
                        "#"
                    } else {
                        p
                    })
                    .collect::<Vec<_>>()
                    .join("/"))
                .unwrap_or_default()
        );
        if text.is_empty() || !seen.insert(shape) {
            continue;
        }
        out.push(format!(
            "{} — {}",
            url,
            text.chars().take(40).collect::<String>()
        ));
        if out.len() >= n {
            break;
        }
    }
    out
}

fn hash_key(key: &str) -> String {
    crate::watch::digest(key)
}

/// 본 글 기록(커서).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub seen: Vec<String>,
}

impl Seen {
    /// 커서를 읽는다. 다른 방식(구간 해시)의 커서면 `None`(기준선을 다시 잡는다).
    pub fn parse(cursor: Option<&str>) -> Option<Self> {
        serde_json::from_str(cursor?).ok()
    }

    pub fn to_cursor(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// 확인 한 번의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    /// 처음 보는 글(페이지 순서).
    pub new: Vec<Item>,
    pub cursor: Seen,
    /// 기준선만 잡았다(첫 확인, 또는 한꺼번에 너무 많이 바뀜).
    pub baseline: bool,
    /// 한꺼번에 [`BURST_MAX`]개보다 많이 새로 보였다.
    pub burst: bool,
}

/// 지난 기록과 견준다.
pub fn diff(current: &[Item], before: Option<&Seen>) -> Diff {
    let now: Vec<String> = current.iter().map(|i| hash_key(&i.key)).collect();
    let mut cursor = Seen { seen: now.clone() };
    let Some(before) = before else {
        return Diff {
            new: Vec::new(),
            cursor,
            baseline: true,
            burst: false,
        };
    };
    let old: HashSet<&String> = before.seen.iter().collect();
    let new: Vec<Item> = current
        .iter()
        .zip(&now)
        .filter(|(_, h)| !old.contains(h))
        .map(|(i, _)| i.clone())
        .collect();
    let here: HashSet<&String> = now.iter().collect();
    cursor.seen.extend(
        before
            .seen
            .iter()
            .filter(|h| !here.contains(h))
            .take(SEEN_MAX.saturating_sub(now.len()))
            .cloned(),
    );
    cursor.seen.truncate(SEEN_MAX);
    if new.len() > BURST_MAX {
        return Diff {
            new: Vec::new(),
            cursor,
            baseline: true,
            burst: true,
        };
    }
    Diff {
        new,
        cursor,
        baseline: false,
        burst: false,
    }
}

/// 줄마다 하나인 말 목록(빈 줄 빼고, 20개·각 40자까지).
pub fn words(text: &str) -> Vec<String> {
    text.lines()
        .flat_map(|l| l.split(','))
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(|w| w.chars().take(40).collect::<String>().to_lowercase())
        .take(20)
        .collect()
}

/// 키워드로 거른다: 넣을 말이 있으면 하나라도 들어간 것만, 뺄 말이 들어간 것은 뺀다.
pub fn keep(title: &str, include: &[String], exclude: &[String]) -> bool {
    let t = title.to_lowercase();
    (include.is_empty() || include.iter().any(|w| t.contains(w.as_str())))
        && !exclude.iter().any(|w| t.contains(w.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://cafe.example.com/bbs/board.php?bo_table=free&page=1").unwrap()
    }

    const PAGE: &str = r#"<html><body>
      <a href="/bbs/board.php?bo_table=free&amp;wr_id=101&amp;page=1">그래픽카드 팝니다 &lt;급처&gt;</a>
      <a href="/bbs/board.php?bo_table=free&wr_id=101&page=1#c">[3]</a>
      <a href='board.php?bo_table=free&wr_id=100'>모니터 삽니다</a>
      <a href="/bbs/board.php?bo_table=notice&wr_id=7">공지</a>
      <a href="javascript:void(0)">글쓰기</a>
      <a href="https://other.example.com/bbs/board.php?bo_table=free&wr_id=5">남의 사이트</a>
      <abbr>아님</abbr>
    </body></html>"#;

    #[test]
    fn pattern_matches_board_posts_and_ignores_other_query() {
        let p = LinkPattern::parse("/bbs/board.php?bo_table=free&wr_id=*", &base()).unwrap();
        let found = items(PAGE, &base(), &p);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].key, "101");
        assert_eq!(
            found[0].title, "그래픽카드 팝니다 <급처>",
            "댓글 수 링크가 제목을 덮지 않는다"
        );
        assert_eq!(found[1].key, "100");
        assert!(
            found[1]
                .url
                .starts_with("https://cafe.example.com/bbs/board.php")
        );
    }

    #[test]
    fn path_wildcards_and_bad_patterns() {
        let b = Url::parse("https://x.example.com/board/list").unwrap();
        let p = LinkPattern::parse("/article/*", &b).unwrap();
        assert_eq!(
            p.key(&Url::parse("https://x.example.com/article/42?ref=list").unwrap()),
            Some("42".into())
        );
        assert_eq!(
            p.key(&Url::parse("https://x.example.com/article/42/edit").unwrap()),
            None
        );
        assert_eq!(
            p.key(&Url::parse("https://y.example.com/article/42").unwrap()),
            None
        );
        assert!(
            LinkPattern::parse("/article/1", &b).is_err(),
            "*가 없으면 키가 없다"
        );
        assert!(LinkPattern::parse("/article/post-*", &b).is_err());
    }

    #[test]
    fn diff_finds_new_items_and_guards_bursts() {
        let item = |k: &str| Item {
            key: k.into(),
            title: format!("글 {k}"),
            url: format!("https://x/{k}"),
        };
        let first = diff(&[item("2"), item("1")], None);
        assert!(first.baseline && first.new.is_empty());
        let next = diff(&[item("3"), item("2"), item("1")], Some(&first.cursor));
        assert_eq!(next.new, vec![item("3")]);
        // 목록 밖으로 밀려난 글은 기억에 남는다(다시 보여도 새 글이 아니다).
        let pushed = diff(&[item("4"), item("3")], Some(&next.cursor));
        assert_eq!(pushed.new, vec![item("4")]);
        let back = diff(&[item("4"), item("1")], Some(&pushed.cursor));
        assert!(back.new.is_empty());
        let many: Vec<Item> = (100..140).map(|n| item(&n.to_string())).collect();
        let burst = diff(&many, Some(&back.cursor));
        assert!(burst.burst && burst.baseline && burst.new.is_empty());
        let cur = Seen::parse(Some(&burst.cursor.to_cursor())).unwrap();
        assert!(diff(&many, Some(&cur)).new.is_empty(), "다시 잡은 기준선");
        assert!(
            Seen::parse(Some("abc123")).is_none(),
            "구간 해시 커서는 기준선부터"
        );
    }

    #[test]
    fn keywords_include_and_exclude() {
        let inc = words("그래픽카드, RTX\n");
        let exc = words("삽니다");
        assert!(keep("RTX 4070 팝니다", &inc, &exc));
        assert!(!keep("그래픽카드 삽니다", &inc, &exc));
        assert!(!keep("모니터 팝니다", &inc, &exc));
        assert!(keep("아무 글", &[], &[]));
    }

    #[test]
    fn samples_show_distinct_link_shapes() {
        let s = link_samples(PAGE, &base(), 10);
        assert!(s.iter().any(|l| l.contains("wr_id=101")));
        assert!(s.iter().all(|l| !l.contains("javascript")));
        assert_eq!(decode_entities("a&#44;b&#x41;&bogus;"), "a,bA&bogus;");
        // 한글이 뒤따르는 &(글자 중간에서 자르지 않는다).
        assert_eq!(decode_entities("교육&amp;세미나 안내"), "교육&세미나 안내");
        assert_eq!(decode_entities("&gt;가나다라"), ">가나다라");
        assert_eq!(
            decode_entities("주식&회사가나다라마"),
            "주식&회사가나다라마"
        );
        assert_eq!(visible("<b>A&B 가나</b>"), "A&B 가나");
    }
}
