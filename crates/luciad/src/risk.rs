//! 외부 쓰기 카드의 위험 줄(결정 0028): 사람이 승인 전에 눈여겨볼 점을 데몬이 결정적으로 찾는다.
//!
//! - 요청을 만들 때 한 번 계산해 카드 내용(payload `risks`)에 고정한다(보이는 것과 실행하는 것이 같다).
//! - 카드 수를 늘리지 않는다. 판단을 돕기만 한다. 같은 패턴 검사를 출구 검사(결정 0029)가 다시 쓴다.

use std::sync::OnceLock;

use regex::Regex;

/// 글에서 개인정보·약속으로 보이는 것(중복 없이, 정해진 순서).
pub fn patterns(text: &str) -> Vec<&'static str> {
    static RULES: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        [
            (
                "주민등록번호로 보이는 숫자",
                r"(?:^|[^0-9])\d{6}-?[1-4]\d{6}(?:[^0-9]|$)",
            ),
            (
                "전화번호",
                r"(?:^|[^0-9])(?:\+82[- ]?)?0\d{1,2}[- .]?\d{3,4}[- .]?\d{4}(?:[^0-9]|$)",
            ),
            (
                "메일 주소",
                r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+",
            ),
            (
                "계좌·카드 번호로 보이는 숫자",
                r"(?:^|[^0-9])(?:\d{2,6}-\d{2,6}-\d{2,8}(?:-\d{1,8})?|\d{12,16})(?:[^0-9]|$)",
            ),
            (
                "집 주소로 보이는 글",
                r"[가-힣0-9]+(?:로|길)\s?\d+(?:-\d+)?(?:번길)?",
            ),
            (
                "금액",
                r"\d[\d,]*(?:\.\d+)?\s?(?:원|만\s?원|억|달러|USD|KRW)\b?|[₩$]\s?\d",
            ),
        ]
        .into_iter()
        .map(|(name, re)| (name, Regex::new(re).expect("정해진 식")))
        .collect()
    });
    // 전화번호를 지운 글에서 계좌를 찾고, 숫자가 10개 이상인 묶음만 계좌로 본다(날짜·전화번호와 가르려고).
    let phone = &rules[1].1;
    let without_phones = phone.replace_all(text, " ");
    rules
        .iter()
        .filter(|(name, re)| {
            if name.starts_with("계좌") {
                re.find_iter(&without_phones)
                    .any(|m| m.as_str().chars().filter(char::is_ascii_digit).count() >= 10)
            } else {
                re.is_match(text)
            }
        })
        .map(|(name, _)| *name)
        .collect()
}

/// 위험 줄 글(카드 칸 값). 없으면 `None`.
pub fn line(risks: &[String]) -> Option<String> {
    (!risks.is_empty()).then(|| format!("⚠ {}", risks.join(" · ")))
}

/// 저장된 카드 내용에서 위험 목록.
pub fn of_payload(payload: &serde_json::Value) -> Vec<String> {
    payload["risks"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// 같은 종류의 요청 가운데 실제로 실행된(보낸) 것에 이 대상이 있었나.
fn seen(store: &lucia_store::Store, kind: &str, hit: impl Fn(&serde_json::Value) -> bool) -> bool {
    store
        .recent_approvals(500)
        .unwrap_or_default()
        .into_iter()
        .any(|a| a.kind == kind && a.status == lucia_store::ApprovalStatus::Sent && hit(&a.payload))
}

fn has_addr(v: &serde_json::Value, addr: &str) -> bool {
    ["to", "cc"].iter().any(|k| {
        v[*k].as_array().is_some_and(|l| {
            l.iter()
                .filter_map(serde_json::Value::as_str)
                .any(|a| a.eq_ignore_ascii_case(addr))
        })
    })
}

/// 메일 발송 요청의 위험 줄.
pub fn assess_mail(
    store: &lucia_store::Store,
    mail: &lucia_precheck::google::assist::OutgoingMail,
) -> Vec<String> {
    let mut out = Vec::new();
    let new: Vec<&String> = mail
        .to
        .iter()
        .chain(&mail.cc)
        .filter(|a| !seen(store, crate::approvals::KIND_MAIL_SEND, |p| has_addr(p, a)))
        .collect();
    if !new.is_empty() {
        out.push("처음 보내는 주소".to_owned());
    }
    out.extend(
        patterns(&format!("{}\n{}", mail.subject, mail.body))
            .into_iter()
            .map(str::to_owned),
    );
    out
}

/// Discord 글 올리기 요청의 위험 줄.
pub fn assess_post(store: &lucia_store::Store, channel: &str, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let known = seen(store, crate::discord_tools::KIND_DISCORD_ACTION, |p| {
        p["op"] == "post" && p["channel"].as_str() == Some(channel)
    });
    if !known {
        out.push("처음 올리는 채널".to_owned());
    }
    out.extend(patterns(text).into_iter().map(str::to_owned));
    out
}

/// 브라우저 동작 요청의 위험 줄: 처음 다루는 사이트, 넣는 글의 개인정보.
pub fn assess_browser(store: &lucia_store::Store, host: &str, typed: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let known = !host.is_empty()
        && seen(store, crate::approvals::KIND_BROWSER_ACTION, |p| {
            p["host"].as_str() == Some(host)
        });
    if !known {
        out.push("처음 승인하는 사이트".to_owned());
    }
    if let Some(t) = typed {
        out.extend(patterns(t).into_iter().map(str::to_owned));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_find_personal_data_and_promises() {
        assert_eq!(patterns("내일 회의 자료 공유드립니다"), Vec::<&str>::new());
        assert_eq!(patterns("연락처 010-1234-5678"), ["전화번호"]);
        assert_eq!(patterns("me@example.com 으로"), ["메일 주소"]);
        assert_eq!(
            patterns("우리은행 1002-123-456789"),
            ["계좌·카드 번호로 보이는 숫자"]
        );
        assert_eq!(patterns("경기 부천시 길주로 123"), ["집 주소로 보이는 글"]);
        assert_eq!(patterns("총 35,000원 입금"), ["금액"]);
        assert_eq!(patterns("900101-1234567"), ["주민등록번호로 보이는 숫자"]);
        assert!(patterns("2026-11-18 회의").is_empty(), "날짜는 아니다");
        assert_eq!(line(&[]), None);
        assert_eq!(
            line(&["처음 보내는 주소".into(), "전화번호".into()]).as_deref(),
            Some("⚠ 처음 보내는 주소 · 전화번호")
        );
    }
}
