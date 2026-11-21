//! 카드를 Discord 임베드로 그린다(결정 0005 "승인 카드 모양").
//!
//! - 메시지 하나에 머리 임베드(제목, 이름-값 칸)와 본문 임베드를 차례로 싣는다. 색 띠로 상태를 보인다.
//! - 길면 여러 메시지로 나눈다. 버튼과 꼬리말은 마지막 메시지에 단다. 닫을 때는 버튼이 달린 마지막
//!   메시지만 고치므로, 앞 메시지는 처음부터 회색으로 둔다(색이 바뀌지 않고 남지 않게).
//! - 나누는 모양은 제목·칸·본문만으로 정한다(머리줄·꼬리말·색과 무관). 닫을 때 다시 그려도 같은 모양이다.
//! - 잘라 버리지 않는다(보이는 것 = 보내는 것). 한도를 넘는 칸은 본문처럼 따로 임베드로 뺀다.
//!   고정 문구(제목, 칸 이름, 꼬리말, 머리줄)만 만일을 위해 한도 안으로 자른다.

use lucia_core::{Card, CardTone};
use twilight_model::channel::message::embed::{Embed, EmbedAuthor, EmbedField, EmbedFooter};

use crate::split_message;

/// Discord 한도(twilight-validate 0.18의 상수와 같다).
pub const DESCRIPTION_MAX: usize = 4096;
pub const FIELD_VALUE_MAX: usize = 1024;
pub const FIELD_NAME_MAX: usize = 256;
pub const FIELD_COUNT_MAX: usize = 25;
pub const TITLE_MAX: usize = 256;
pub const AUTHOR_NAME_MAX: usize = 256;
pub const EMBEDS_PER_MESSAGE: usize = 10;
/// 한 메시지의 모든 임베드 글자 합(제목, 설명, 칸, 꼬리말, 작성자 이름).
pub const EMBED_TOTAL_MAX: usize = 6000;
pub const CONTENT_MAX: usize = 2000;

/// 본문 조각 하나의 크기(설명 한도 아래).
const CHUNK: usize = 3800;
/// 꼬리말은 이 안으로 자르고, 메시지마다 이만큼 비워 둔다.
const FOOTER_MAX: usize = 200;
/// 머리줄(메시지 내용)은 이 안으로 자른다.
const HEADLINE_MAX: usize = 1800;
/// 머리 임베드에 칸을 싣는 상한. 넘는 칸은 따로 뺀다.
const HEAD_BUDGET: usize = 3000;
/// 메시지 하나의 임베드 글자 합 상한(꼬리말 자리와 여유를 뺀다).
const MESSAGE_BUDGET: usize = EMBED_TOTAL_MAX - FOOTER_MAX - 100;

/// 메시지 하나로 보낼 것.
#[derive(Debug, Clone, PartialEq)]
pub struct CardMessage {
    /// 비면 내용 없이 임베드만.
    pub content: String,
    pub embeds: Vec<Embed>,
}

/// 상태 색.
pub fn color(tone: CardTone) -> u32 {
    match tone {
        CardTone::Pending => 0x00F0_B232,
        CardTone::Success => 0x0023_A55A,
        CardTone::Danger => 0x00DA_373C,
        CardTone::Warning => 0x00E6_7E22,
        CardTone::Neutral => 0x0080_848E,
    }
}

/// 고정 문구용: `max`자 안으로(넘으면 끝을 `…`로).
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn blank(color: u32) -> Embed {
    Embed {
        author: None,
        color: Some(color),
        description: None,
        fields: Vec::new(),
        flags: None,
        footer: None,
        image: None,
        kind: "rich".to_owned(),
        provider: None,
        thumbnail: None,
        timestamp: None,
        title: None,
        url: None,
        video: None,
    }
}

/// Discord가 세는 임베드 글자 수.
pub fn embed_chars(e: &Embed) -> usize {
    let count = |s: &Option<String>| s.as_deref().map_or(0, |s| s.chars().count());
    count(&e.title)
        + count(&e.description)
        + e.author.as_ref().map_or(0, |a| a.name.chars().count())
        + e.footer.as_ref().map_or(0, |f| f.text.chars().count())
        + e.fields
            .iter()
            .map(|f| f.name.chars().count() + f.value.chars().count())
            .sum::<usize>()
}

/// 카드를 메시지들로. 버튼은 마지막 메시지에 단다(여기서는 다루지 않는다).
pub fn render(card: &Card) -> Vec<CardMessage> {
    let tone = color(card.tone);

    // 머리 임베드: 제목과 짧은 칸. 칸 값이 한도를 넘거나 머리가 너무 커지면 그 칸은 따로 뺀다.
    let mut head = blank(tone);
    head.title = Some(clip(&card.title, TITLE_MAX));
    let mut sections: Vec<(String, &str)> = Vec::new();
    for f in &card.fields {
        let name = match clip(f.name.trim(), FIELD_NAME_MAX) {
            n if n.is_empty() => "\u{200b}".to_owned(),
            n => n,
        };
        let value = if f.value.trim().is_empty() {
            "—"
        } else {
            f.value.as_str()
        };
        let size = name.chars().count() + value.chars().count();
        if value.chars().count() <= FIELD_VALUE_MAX
            && head.fields.len() < FIELD_COUNT_MAX
            && embed_chars(&head) + size <= HEAD_BUDGET
        {
            head.fields.push(EmbedField {
                inline: f.inline,
                name,
                value: value.to_owned(),
            });
        } else {
            sections.push((name, value));
        }
    }

    // 본문 조각: 따로 뺀 칸, 그다음 본문. 조각마다 임베드 하나, 작은 이름표(작성자 칸)를 단다.
    let body = (!card.body.trim().is_empty()).then(|| ("본문".to_owned(), card.body.as_str()));
    let mut pieces = Vec::new();
    for (label, text) in sections.into_iter().chain(body) {
        for (i, chunk) in split_message(text, CHUNK).into_iter().enumerate() {
            let mut e = blank(tone);
            let name = if i == 0 {
                label.clone()
            } else {
                format!("{label} (이어서)")
            };
            e.author = Some(EmbedAuthor {
                icon_url: None,
                name: clip(&name, AUTHOR_NAME_MAX),
                proxy_icon_url: None,
                url: None,
            });
            e.description = Some(chunk);
            pieces.push(e);
        }
    }

    // 메시지로 묶는다(꼬리말 자리를 늘 비워 둔다: 닫을 때 꼬리말이 바뀌어도 같은 모양).
    let mut groups: Vec<Vec<Embed>> = vec![vec![head]];
    let mut used = embed_chars(&groups[0][0]);
    for e in pieces {
        let n = embed_chars(&e);
        let current = groups.last_mut().expect("하나 이상");
        if current.len() < EMBEDS_PER_MESSAGE && used + n <= MESSAGE_BUDGET {
            current.push(e);
            used += n;
        } else {
            groups.push(vec![e]);
            used = n;
        }
    }

    let last = groups.len() - 1;
    for group in &mut groups[..last] {
        for e in group {
            e.color = Some(color(CardTone::Neutral));
        }
    }
    if let Some(footer) = &card.footer {
        let e = groups[last].last_mut().expect("하나 이상");
        e.footer = Some(EmbedFooter {
            icon_url: None,
            proxy_icon_url: None,
            text: clip(footer, FOOTER_MAX),
        });
    }

    // 머리줄은 메시지 내용이라 주소가 있으면 미리보기를 만든다(결정 0028). 임베드 안 글은 미리보기를 만들지 않는다.
    let headline = crate::quiet_links(&clip(card.headline.trim(), HEADLINE_MAX));
    groups
        .into_iter()
        .enumerate()
        .map(|(i, embeds)| CardMessage {
            content: if i == 0 || i == last {
                headline.clone()
            } else {
                String::new()
            },
            embeds,
        })
        .collect()
}

/// 시험·점검용: 메시지들에 보이는 본문 조각을 이어 붙인다(머리 칸 제외).
pub fn shown_sections(messages: &[CardMessage]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for e in messages.iter().flat_map(|m| &m.embeds) {
        let (Some(author), Some(text)) = (&e.author, &e.description) else {
            continue;
        };
        let label = author
            .name
            .strip_suffix(" (이어서)")
            .unwrap_or(&author.name)
            .to_owned();
        match out.last_mut() {
            Some((l, t)) if *l == label && author.name.ends_with(" (이어서)") => {
                t.push_str(text)
            }
            _ => out.push((label, text.clone())),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use lucia_core::{CardButton, CardField, ConversationId};

    use super::*;

    fn card(fields: Vec<CardField>, body: &str, tone: CardTone, footer: &str) -> Card {
        Card {
            conversation: ConversationId("discord:1".into()),
            headline: "📨 메일을 보내도 될까요?".into(),
            title: "메일 발송 승인 요청".into(),
            fields,
            body: body.into(),
            footer: Some(footer.into()),
            tone,
            buttons: vec![CardButton {
                action: "a".into(),
                label: "보내기".into(),
                primary: true,
                prompt: None,
                form: None,
            }],
        }
    }

    fn field(name: &str, value: &str, inline: bool) -> CardField {
        CardField {
            name: name.into(),
            value: value.into(),
            inline,
        }
    }

    fn valid(messages: &[CardMessage]) {
        for m in messages {
            assert!(m.content.chars().count() <= CONTENT_MAX);
            twilight_validate::message::embeds(&m.embeds).expect("Discord 한도");
            for e in &m.embeds {
                for f in &e.fields {
                    assert!(!f.name.is_empty() && !f.value.is_empty(), "빈 칸");
                }
            }
        }
    }

    #[test]
    fn short_card_is_one_message_with_header_and_body() {
        let c = card(
            vec![
                field("보내는 주소", r"me\@example\.com", true),
                field("받는 사람", r"you\@example\.com", true),
                field("제목", "안녕하세요", false),
            ],
            "본문입니다.",
            CardTone::Pending,
            "요청 abc · 11-17 19:07까지",
        );
        let m = render(&c);
        valid(&m);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].content, "📨 메일을 보내도 될까요?");
        assert_eq!(m[0].embeds.len(), 2, "머리 + 본문");
        assert_eq!(m[0].embeds[0].fields.len(), 3);
        assert_eq!(m[0].embeds[0].color, Some(color(CardTone::Pending)));
        assert_eq!(m[0].embeds[1].description.as_deref(), Some("본문입니다."));
        assert!(m[0].embeds[1].footer.is_some(), "꼬리말은 마지막 임베드에");
    }

    /// 최악의 경우(이스케이프로 두 배가 된 8,000자 본문, 공백 없음, 긴 받는 사람 목록)도 한도 안이고,
    /// 잘린 글이 없으며, 열 때와 닫을 때 나뉘는 모양이 같다.
    #[test]
    fn worst_case_fits_limits_and_closes_with_the_same_shape() {
        let body = r"\*\_".repeat(4000); // 16,000자, 공백·줄바꿈 없음
        let long_list = (0..20)
            .map(|i| format!(r"{}\@{}\.example\.com", "a".repeat(60), i))
            .collect::<Vec<_>>()
            .join("\n");
        let fields = vec![
            field("보내는 주소", r"me\@example\.com", true),
            field("받는 사람", &long_list, true),
            field("참조", &long_list, true),
            field("제목", &"제".repeat(500), false),
        ];
        let open = render(&card(fields.clone(), &body, CardTone::Pending, "열림"));
        let closed = render(&card(
            fields,
            &body,
            CardTone::Success,
            &"닫힘 꼬리말이 훨씬 깁니다 ".repeat(30),
        ));
        valid(&open);
        valid(&closed);
        assert!(open.len() > 1, "여러 메시지로 나뉜다");
        assert_eq!(open.len(), closed.len(), "닫아도 같은 모양");
        for (a, b) in open.iter().zip(&closed) {
            let da: Vec<_> = a.embeds.iter().map(|e| &e.description).collect();
            let db: Vec<_> = b.embeds.iter().map(|e| &e.description).collect();
            assert_eq!(da, db, "같은 조각");
        }
        // 빠짐없이: 본문과 따로 뺀 칸이 그대로 다 보인다.
        let sections = shown_sections(&open);
        let get = |label: &str| {
            sections
                .iter()
                .find(|(l, _)| l == label)
                .map(|(_, t)| t.clone())
        };
        assert_eq!(get("본문").as_deref(), Some(body.as_str()));
        assert_eq!(get("받는 사람").as_deref(), Some(long_list.as_str()));
        assert_eq!(get("참조").as_deref(), Some(long_list.as_str()));
        // 마지막이 아닌 메시지는 회색, 마지막은 상태 색.
        assert!(open[..open.len() - 1].iter().all(|m| {
            m.embeds
                .iter()
                .all(|e| e.color == Some(color(CardTone::Neutral)))
        }));
        assert_eq!(
            closed.last().unwrap().embeds[0].color,
            Some(color(CardTone::Success))
        );
    }

    #[test]
    fn empty_values_and_names_are_filled() {
        let m = render(&card(
            vec![field("", "", true)],
            "",
            CardTone::Danger,
            "꼬리",
        ));
        valid(&m);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].embeds.len(), 1, "본문이 없으면 머리만");
        assert!(m[0].embeds[0].footer.is_some());
    }
}
