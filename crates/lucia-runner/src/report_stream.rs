//! 도구 입력 델타(`input_json_delta`)에서 문자열 필드 하나를 받는 대로 풀어낸다(결정 0005).
//!
//! `--include-partial-messages`를 켜면 도구 입력 JSON이 조각(`partial_json`)으로 온다. 조각은 JSON
//! 문법의 아무 곳에서나 끊긴다(이스케이프 중간, 서로게이트 쌍 중간, 글자 중간은 아니다: 조각은 문자열).
//! [`FieldStream`]은 맨 위 객체에서 정한 키의 문자열 값을 따라가며, 완성된 글자만 내놓는다.
//! [`ReportStream`]은 stream_event를 먹고 정한 도구(`report_result`)의 `summary`가 늘 때마다 지금까지의
//! 값을 돌려준다.

use std::collections::HashMap;

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Esc {
    None,
    Backslash,
    /// `\u` 뒤 16진수 몇 자리를 받았는지.
    Unicode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// `{`를 기다린다.
    Start,
    /// 키 문자열(`"`)이나 `}`를 기다린다.
    KeyOrEnd,
    InKey,
    /// `:`를 기다린다.
    Colon,
    /// 값의 시작을 기다린다.
    Value,
    /// 따라가는 문자열 값 안.
    Target,
    /// 건너뛰는 문자열 값 안.
    SkipString,
    /// 건너뛰는 객체·배열(깊이, 그 안의 문자열 안인가).
    Nested {
        depth: u32,
        in_string: bool,
    },
    /// 숫자·true·null 등.
    Literal,
    /// `,`나 `}`를 기다린다.
    AfterValue,
    Done,
}

/// 맨 위 객체에서 키 하나의 문자열 값을 점점 풀어낸다.
#[derive(Debug, Clone)]
pub struct FieldStream {
    key: String,
    state: State,
    esc: Esc,
    hex: String,
    /// 짝을 기다리는 높은 서로게이트.
    high: Option<u16>,
    current_key: String,
    out: String,
}

impl FieldStream {
    pub fn new(key: &str) -> Self {
        Self {
            key: key.to_owned(),
            state: State::Start,
            esc: Esc::None,
            hex: String::new(),
            high: None,
            current_key: String::new(),
            out: String::new(),
        }
    }

    /// 지금까지 풀린 값(완성된 글자만).
    pub fn value(&self) -> &str {
        &self.out
    }

    /// 조각 하나를 먹는다. 값이 늘었으면 `true`.
    pub fn push(&mut self, chunk: &str) -> bool {
        let before = self.out.len();
        for c in chunk.chars() {
            self.step(c);
        }
        self.out.len() != before
    }

    fn emit_unit(&mut self, unit: u16) {
        match (self.high.take(), unit) {
            (Some(hi), 0xDC00..=0xDFFF) => {
                let code = 0x10000 + ((u32::from(hi) - 0xD800) << 10) + (u32::from(unit) - 0xDC00);
                self.out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
            }
            (Some(_), _) => {
                // 짝이 없는 서로게이트: 대체 문자로 두고 이번 단위를 다시 본다.
                self.out.push('\u{FFFD}');
                self.emit_unit(unit);
            }
            (None, 0xD800..=0xDBFF) => self.high = Some(unit),
            (None, 0xDC00..=0xDFFF) => self.out.push('\u{FFFD}'),
            (None, u) => self
                .out
                .push(char::from_u32(u32::from(u)).unwrap_or('\u{FFFD}')),
        }
    }

    /// 문자열 안의 글자 하나(따라가는 값이면 풀어서 내놓는다). 문자열이 끝나면 `true`.
    fn string_char(&mut self, c: char, keep: bool, key: bool) -> bool {
        match self.esc {
            Esc::Backslash => {
                self.esc = Esc::None;
                let decoded = match c {
                    'n' => Some('\n'),
                    't' => Some('\t'),
                    'r' => Some('\r'),
                    'b' => Some('\u{8}'),
                    'f' => Some('\u{c}'),
                    '"' | '\\' | '/' => Some(c),
                    'u' => {
                        self.esc = Esc::Unicode;
                        self.hex.clear();
                        None
                    }
                    other => Some(other),
                };
                if let Some(d) = decoded {
                    if keep {
                        if self.high.take().is_some() {
                            self.out.push('\u{FFFD}');
                        }
                        self.out.push(d);
                    } else if key {
                        self.current_key.push(d);
                    }
                }
                false
            }
            Esc::Unicode => {
                self.hex.push(c);
                if self.hex.len() == 4 {
                    self.esc = Esc::None;
                    let unit = u16::from_str_radix(&self.hex, 16).unwrap_or(0xFFFD);
                    if keep {
                        self.emit_unit(unit);
                    } else if key {
                        self.current_key
                            .push(char::from_u32(u32::from(unit)).unwrap_or('\u{FFFD}'));
                    }
                }
                false
            }
            Esc::None => match c {
                '\\' => {
                    self.esc = Esc::Backslash;
                    false
                }
                '"' => {
                    if keep && self.high.take().is_some() {
                        self.out.push('\u{FFFD}');
                    }
                    true
                }
                _ => {
                    if keep {
                        if self.high.take().is_some() {
                            self.out.push('\u{FFFD}');
                        }
                        self.out.push(c);
                    } else if key {
                        self.current_key.push(c);
                    }
                    false
                }
            },
        }
    }

    fn step(&mut self, c: char) {
        match self.state.clone() {
            State::Start => {
                if c == '{' {
                    self.state = State::KeyOrEnd;
                }
            }
            State::KeyOrEnd => match c {
                '"' => {
                    self.current_key.clear();
                    self.state = State::InKey;
                }
                '}' => self.state = State::Done,
                _ => {}
            },
            State::InKey => {
                if self.string_char(c, false, true) {
                    self.state = State::Colon;
                }
            }
            State::Colon => {
                if c == ':' {
                    self.state = State::Value;
                }
            }
            State::Value => match c {
                c if c.is_whitespace() => {}
                '"' if self.current_key == self.key => self.state = State::Target,
                '"' => self.state = State::SkipString,
                '{' | '[' => {
                    self.state = State::Nested {
                        depth: 1,
                        in_string: false,
                    }
                }
                _ => self.state = State::Literal,
            },
            State::Target => {
                if self.string_char(c, true, false) {
                    self.state = State::AfterValue;
                }
            }
            State::SkipString => {
                if self.string_char(c, false, false) {
                    self.state = State::AfterValue;
                }
            }
            State::Nested { depth, in_string } => {
                if in_string {
                    if self.string_char(c, false, false) {
                        self.state = State::Nested {
                            depth,
                            in_string: false,
                        };
                    }
                } else {
                    self.state = match c {
                        '"' => State::Nested {
                            depth,
                            in_string: true,
                        },
                        '{' | '[' => State::Nested {
                            depth: depth + 1,
                            in_string: false,
                        },
                        '}' | ']' if depth == 1 => State::AfterValue,
                        '}' | ']' => State::Nested {
                            depth: depth - 1,
                            in_string: false,
                        },
                        _ => State::Nested { depth, in_string },
                    };
                }
            }
            State::Literal => match c {
                ',' => self.state = State::KeyOrEnd,
                '}' => self.state = State::Done,
                _ => {}
            },
            State::AfterValue => match c {
                ',' => self.state = State::KeyOrEnd,
                '}' => self.state = State::Done,
                _ => {}
            },
            State::Done => {}
        }
    }
}

/// stream_event에서 정한 도구의 문자열 필드를 따라간다. 같은 도구를 여러 번 부르면 첫 것만 따른다(먼저 보낸 조각이
/// 첫 보고의 것이어야 엔진이 보고를 합칠 때 이어 붙일 수 있다).
#[derive(Debug, Clone)]
pub struct ReportStream {
    tool: String,
    field: String,
    /// 지금 메시지에서 따라가는 블록(번호 → 풀이기).
    blocks: HashMap<u64, FieldStream>,
    /// 도구 블록이 몇 번 시작됐나(두 번 넘으면 엔진이 보고를 합친다).
    pub calls: usize,
}

impl ReportStream {
    pub fn new(tool: &str, field: &str) -> Self {
        Self {
            tool: tool.to_owned(),
            field: field.to_owned(),
            blocks: HashMap::new(),
            calls: 0,
        }
    }

    /// stream_event의 `event` 하나를 먹는다. 값이 늘었으면 지금까지의 전체 값을 돌려준다.
    pub fn feed(&mut self, event: &Value) -> Option<String> {
        match event.get("type").and_then(Value::as_str)? {
            "message_start" => {
                self.blocks.clear();
                None
            }
            "content_block_start" => {
                let block = event.get("content_block")?;
                if block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && block.get("name").and_then(Value::as_str) == Some(self.tool.as_str())
                {
                    let index = event.get("index").and_then(Value::as_u64)?;
                    self.calls += 1;
                    if self.calls == 1 {
                        self.blocks.insert(index, FieldStream::new(&self.field));
                    }
                }
                None
            }
            "content_block_delta" => {
                let index = event.get("index").and_then(Value::as_u64)?;
                let delta = event.get("delta")?;
                if delta.get("type").and_then(Value::as_str) != Some("input_json_delta") {
                    return None;
                }
                let chunk = delta.get("partial_json").and_then(Value::as_str)?;
                let stream = self.blocks.get_mut(&index)?;
                stream.push(chunk).then(|| stream.value().to_owned())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whole(json: &str, key: &str) -> String {
        let v: Value = serde_json::from_str(json).unwrap();
        v[key].as_str().unwrap().to_owned()
    }

    /// 한 글자씩, 그리고 여러 크기로 나눠 먹여도 매번 내놓는 값은 최종값의 앞부분이고, 끝나면 같다.
    fn check(json: &str) {
        let expected = whole(json, "summary");
        let chars: Vec<char> = json.chars().collect();
        for size in [1usize, 2, 3, 5, 7, 64] {
            let mut f = FieldStream::new("summary");
            for chunk in chars.chunks(size) {
                let chunk: String = chunk.iter().collect();
                f.push(&chunk);
                assert!(
                    expected.starts_with(f.value()),
                    "앞부분이 아니다(size {size}): {:?} / {:?}",
                    f.value(),
                    expected
                );
            }
            assert_eq!(f.value(), expected, "size {size}");
        }
    }

    #[test]
    fn decodes_progressively_with_escapes_and_surrogates() {
        let summary = "첫 줄 \"인용\" \\ 슬래시/\n%%\n둘째 😀 é\t탭\u{1F680}";
        let json =
            serde_json::json!({"status": "done", "summary": summary, "details": null}).to_string();
        check(&json);
        // 이스케이프를 모두 \\u로 쓴 경우(서로게이트 쌍이 조각 사이에서 끊긴다).
        let ascii = format!(
            "{{\"status\":\"done\",\"summary\":\"{}\"}}",
            summary
                .encode_utf16()
                .map(|u| format!("\\u{u:04x}"))
                .collect::<String>()
        );
        check(&ascii);
    }

    #[test]
    fn skips_other_fields_even_when_they_mention_the_key() {
        let json = r#"{"details": "\"summary\": \"가짜\"", "nested": {"summary": "x", "a": [1, "]"]}, "n": 3, "summary": "진짜 답", "attachments": ["f_1"]}"#;
        check(json);
        let mut f = FieldStream::new("summary");
        f.push(json);
        assert_eq!(f.value(), "진짜 답");
    }

    #[test]
    fn report_stream_follows_the_named_tool_only() {
        let mut r = ReportStream::new("mcp__lucia__report_result", "summary");
        let start = |index: u64, name: &str| serde_json::json!({"type": "content_block_start", "index": index, "content_block": {"type": "tool_use", "name": name, "input": {}}});
        let delta = |index: u64, s: &str| serde_json::json!({"type": "content_block_delta", "index": index, "delta": {"type": "input_json_delta", "partial_json": s}});
        r.feed(&serde_json::json!({"type": "message_start"}));
        r.feed(&start(0, "WebSearch"));
        assert_eq!(r.feed(&delta(0, r#"{"query": "x"}"#)), None, "다른 도구");
        r.feed(&start(1, "mcp__lucia__report_result"));
        assert_eq!(r.feed(&delta(1, r#"{"status":"done","sum"#)), None);
        assert_eq!(
            r.feed(&delta(1, r#"mary":"안녕\n"#)).as_deref(),
            Some("안녕\n")
        );
        assert_eq!(
            r.feed(&delta(1, r#"%%\n다음"#)).as_deref(),
            Some("안녕\n%%\n다음")
        );
        assert_eq!(r.calls, 1);
        // 두 번째 보고는 따라가지 않는다(먼저 보낸 조각은 첫 보고의 것).
        r.feed(&serde_json::json!({"type": "message_start"}));
        r.feed(&start(0, "mcp__lucia__report_result"));
        assert_eq!(r.feed(&delta(0, r#"{"summary":"다시 쓴 답"#)), None);
        assert_eq!(r.calls, 2);
    }
}
