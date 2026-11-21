//! `claude -p --output-format stream-json --verbose` 출력 한 줄을 이벤트로 바꾼다.
//!
//! 필드 이름은 Claude Code 2.1.325의 실제 출력에서 확인했다
//! (`tests/fixtures/host-2.1.325-pong.jsonl`). 모르는 이벤트와 필드는 버리지 않고
//! 원본 JSON으로 남겨, CLI 버전이 바뀌어도 파싱이 깨지지 않게 한다.

use serde::Deserialize;
use serde_json::Value;

/// stream-json 한 줄.
#[derive(Debug, Clone)]
pub enum Event {
    /// `type=system, subtype=init`: 세션 시작 정보.
    Init(Box<Init>),
    /// `type=assistant`: 모델 응답 메시지(텍스트, 도구 호출).
    Assistant(Value),
    /// `type=user`: 도구 결과 등 사용자 측 메시지.
    User(Value),
    /// `type=rate_limit_event`: 구독 사용량 한도 상태.
    RateLimit(Box<RateLimitInfo>),
    /// `type=result`: 실행 종료 요약.
    Result(Box<RunResult>),
    /// `type=stream_event`(`--include-partial-messages`): API 스트림 이벤트(`event`). 받는 대로 넘기고 모아 두지 않는다.
    StreamEvent(Value),
    /// `type=system, subtype=compact_boundary`: 대화를 요약해 줄였다(`/compact` 또는 자동).
    /// 공식 문서에 CLI 이벤트로는 없고 실측(2.1.325)으로 확인했다.
    CompactBoundary(Box<CompactBoundary>),
    /// `type=system, subtype=api_retry`: API 요청이 실패해 다시 보낸다(응답 없음, 429, 5xx 등).
    /// 공식 문서(headless "Handle API retries"): `attempt`, `max_retries`, `retry_delay_ms`, `error_status`(없으면 null),
    /// `error`(종류 문자열), 응답 머리를 못 받았을 때만 `no_response{waited_ms, retry_wait_ms}`.
    ApiRetry(Box<ApiRetry>),
    /// 위에 없는 이벤트. 종류 이름과 원본을 그대로 둔다.
    Other { kind: String, raw: Value },
}

/// API 다시 보내기(`api_retry`). 모르는 모양이어도 깨지지 않게 모두 선택 필드다.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApiRetry {
    pub attempt: u64,
    pub max_retries: u64,
    pub retry_delay_ms: u64,
    /// HTTP 상태(있으면).
    pub status: Option<u64>,
    /// 오류 종류 한 줄(예: `rate_limit`, `connection`).
    pub error: String,
    /// 응답 머리를 받지 못하고 기다린 시간(첫 바이트 시간 초과일 때).
    pub waited_ms: Option<u64>,
}

impl ApiRetry {
    fn from_raw(raw: &Value) -> Self {
        let num = |p: &str| raw.pointer(p).and_then(Value::as_u64);
        Self {
            attempt: num("/attempt").unwrap_or(0),
            max_retries: num("/max_retries").unwrap_or(0),
            retry_delay_ms: num("/retry_delay_ms").unwrap_or(0),
            status: num("/error_status"),
            error: match raw.get("error") {
                Some(Value::String(s)) => s.chars().take(200).collect(),
                Some(v @ Value::Object(_)) => v
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .chars()
                    .take(200)
                    .collect(),
                _ => String::new(),
            },
            waited_ms: num("/no_response/waited_ms"),
        }
    }
}

/// `compact_boundary.compact_metadata`에서 쓰는 것.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CompactBoundary {
    #[serde(default)]
    pub trigger: Option<String>,
    #[serde(default)]
    pub pre_tokens: u64,
    #[serde(default)]
    pub post_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Init {
    pub session_id: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub claude_code_version: Option<String>,
    #[serde(rename = "permissionMode", default)]
    pub permission_mode: Option<String>,
    #[serde(rename = "apiKeySource", default)]
    pub api_key_source: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerStatus>,
}

/// init 이벤트의 MCP 서버 연결 상태. 예: `{"name":"lucia","source":"dynamic","status":"connected"}`.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

impl Init {
    /// 이름이 `name`인 MCP 서버가 연결됐는지.
    pub fn mcp_connected(&self, name: &str) -> bool {
        self.mcp_servers
            .iter()
            .any(|s| s.name == name && s.status.as_deref() == Some("connected"))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitInfo {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(rename = "rateLimitType", default)]
    pub rate_limit_type: Option<String>,
    /// 유닉스 시각(초).
    #[serde(rename = "resetsAt", default)]
    pub resets_at: Option<i64>,
    /// `five_hour`, `seven_day` 같은 창별 사용률. 원본으로 둔다.
    #[serde(rename = "unifiedWindows", default)]
    pub unified_windows: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// 턴별 사용량. 2.1.325에서는 마지막 턴 하나만 들어 있었다.
    #[serde(default)]
    pub iterations: Vec<Iteration>,
}

/// 턴 하나의 사용량.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Iteration {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

impl Usage {
    /// 실행 전체에서 모델이 읽은 입력 토큰 합계(캐시 포함, 턴마다 다시 센다). 비용 원장용.
    pub fn total_input(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunResult {
    /// `success` 또는 오류 종류.
    pub subtype: String,
    #[serde(default)]
    pub is_error: bool,
    pub session_id: String,
    #[serde(default)]
    pub num_turns: u32,
    #[serde(default)]
    pub duration_ms: u64,
    /// 구독 인증에서도 값이 온다(API 환산 비용).
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    /// 모델의 최종 자유 텍스트. 메신저로 그대로 흘리지 않는다(명세 §출력 계약).
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub permission_denials: Vec<Value>,
    #[serde(default)]
    pub terminal_reason: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub api_error_status: Option<Value>,
    /// 인증 만료 시각(ISO 8601). 만료 감지에 쓴다(명세 원칙 8).
    #[serde(default)]
    pub credential_expires_at: Option<String>,
    /// 모델별 사용량. `contextWindow`가 들어 있어 세션 교대 기준에 쓸 수 있다.
    #[serde(rename = "modelUsage", default)]
    pub model_usage: Option<Value>,
}

impl RunResult {
    /// 마지막 턴에 모델이 읽은 입력 토큰 = 현재 컨텍스트 크기. 세션 교대 판단용.
    /// `usage`의 합계는 턴마다 같은 맥락을 다시 세므로 쓰지 않는다.
    pub fn context_tokens(&self) -> u64 {
        match self.usage.iterations.last() {
            Some(it) => {
                it.input_tokens + it.cache_creation_input_tokens + it.cache_read_input_tokens
            }
            None => self.usage.total_input(),
        }
    }

    /// 모델별 사용량에서 가장 큰 컨텍스트 창 크기.
    pub fn context_window(&self) -> Option<u64> {
        self.model_usage
            .as_ref()?
            .as_object()?
            .values()
            .filter_map(|m| m.get("contextWindow")?.as_u64())
            .max()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("JSON이 아니다: {0}")]
    Json(#[from] serde_json::Error),
    #[error("type 필드가 없다")]
    MissingType,
}

/// 한 줄을 파싱한다. 빈 줄은 `Ok(None)`.
pub fn parse_line(line: &str) -> Result<Option<Event>, ParseError> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let raw: Value = serde_json::from_str(line)?;
    let kind = raw
        .get("type")
        .and_then(Value::as_str)
        .ok_or(ParseError::MissingType)?
        .to_owned();
    let subtype = raw.get("subtype").and_then(Value::as_str);

    let event = match (kind.as_str(), subtype) {
        ("system", Some("init")) => Event::Init(Box::new(serde_json::from_value(raw)?)),
        ("system", Some("compact_boundary")) => {
            let meta = raw.get("compact_metadata").cloned().unwrap_or(Value::Null);
            Event::CompactBoundary(Box::new(serde_json::from_value(meta)?))
        }
        ("system", Some("api_retry")) => Event::ApiRetry(Box::new(ApiRetry::from_raw(&raw))),
        ("assistant", _) => Event::Assistant(raw),
        ("stream_event", _) => Event::StreamEvent(raw.get("event").cloned().unwrap_or(Value::Null)),
        ("user", _) => Event::User(raw),
        ("rate_limit_event", _) => {
            let info = raw.get("rate_limit_info").cloned().unwrap_or(Value::Null);
            Event::RateLimit(Box::new(serde_json::from_value(info)?))
        }
        ("result", _) => Event::Result(Box::new(serde_json::from_value(raw)?)),
        _ => Event::Other { kind, raw },
    };
    Ok(Some(event))
}

/// assistant 메시지에서 도구 호출 이름을 뽑는다.
pub fn tool_uses(message: &Value) -> Vec<&str> {
    message
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| block.get("name").and_then(Value::as_str))
        .collect()
}

/// compact 요약 메시지(`type=user`, `isSynthetic=true`, 내용은 문자열)의 본문.
pub fn compact_summary(raw: &Value) -> Option<&str> {
    if raw.get("isSynthetic").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let text = raw.pointer("/message/content").and_then(Value::as_str)?;
    text.contains("Summary:").then_some(text)
}

/// assistant 메시지의 도구 호출(이름, 입력).
///
/// MCP 서버의 부르기 도구(`mcp__<서버>__tool_call`, 입력 `{name, arguments}`)는 실제로 부른 도구
/// (`mcp__<서버>__<name>`, 입력 `arguments`)로 펼친다. 진행 알림·도구 흔적·사용 횟수가 실제 도구를 보게.
pub fn tool_calls(raw: &Value) -> Vec<(String, Value)> {
    raw.pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|c| {
            let name = c.get("name").and_then(Value::as_str)?.to_owned();
            Some(unwrap_call_through(
                name,
                c.get("input").cloned().unwrap_or(Value::Null),
            ))
        })
        .collect()
}

/// 부르기 도구의 이름 끝.
pub const CALL_THROUGH_SUFFIX: &str = "__tool_call";

fn unwrap_call_through(name: String, input: Value) -> (String, Value) {
    let Some(prefix) = name
        .strip_suffix(CALL_THROUGH_SUFFIX)
        .filter(|p| p.starts_with("mcp__"))
    else {
        return (name, input);
    };
    match input.get("name").and_then(Value::as_str) {
        Some(inner) if !inner.is_empty() && !inner.contains("__") => {
            let args = input
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            (format!("{prefix}__{inner}"), args)
        }
        _ => (name, input),
    }
}

#[cfg(test)]
mod call_through_tests {
    use super::*;

    #[test]
    fn call_through_is_unwrapped_to_the_real_tool() {
        let raw = serde_json::json!({"message": {"content": [
            {"type": "tool_use", "name": "mcp__lucia__tool_call",
             "input": {"name": "mail_send", "arguments": {"to": ["a@b.c"]}}},
            {"type": "tool_use", "name": "mcp__lucia__tool_call", "input": {"arguments": {}}},
            {"type": "tool_use", "name": "mcp__lucia__tool_search", "input": {"query": "메일"}},
            {"type": "tool_use", "name": "WebSearch", "input": {"query": "x"}},
        ]}});
        let calls = tool_calls(&raw);
        assert_eq!(calls[0].0, "mcp__lucia__mail_send");
        assert_eq!(calls[0].1, serde_json::json!({"to": ["a@b.c"]}));
        assert_eq!(calls[1].0, "mcp__lucia__tool_call", "이름이 없으면 그대로");
        assert_eq!(calls[2].0, "mcp__lucia__tool_search");
        assert_eq!(calls[3].0, "WebSearch");
    }
}

#[cfg(test)]
mod api_retry_tests {
    use super::*;

    /// 2.1.325가 내는 모양(바이너리에서 확인한 필드). 첫 바이트 시간 초과와 429.
    #[test]
    fn api_retry_lines_are_read() {
        let line = r#"{"type":"system","subtype":"api_retry","attempt":1,"max_retries":10,"retry_delay_ms":0,"error_status":null,"error":"connection","no_response":{"waited_ms":186000,"retry_wait_ms":599000},"session_id":"s"}"#;
        let Some(Event::ApiRetry(r)) = parse_line(line).unwrap() else {
            panic!("api_retry");
        };
        assert_eq!(
            (
                r.attempt,
                r.max_retries,
                r.status,
                r.waited_ms,
                r.error.as_str()
            ),
            (1, 10, None, Some(186000), "connection")
        );
        let line = r#"{"type":"system","subtype":"api_retry","attempt":2,"max_retries":10,"retry_delay_ms":4000,"error_status":429,"error":"rate_limit"}"#;
        let Some(Event::ApiRetry(r)) = parse_line(line).unwrap() else {
            panic!("api_retry");
        };
        assert_eq!(
            (r.status, r.retry_delay_ms, r.waited_ms),
            (Some(429), 4000, None)
        );
        // 모르는 모양도 깨지지 않는다.
        let Some(Event::ApiRetry(r)) =
            parse_line(r#"{"type":"system","subtype":"api_retry","error":{"message":"x"}}"#)
                .unwrap()
        else {
            panic!("api_retry");
        };
        assert_eq!((r.attempt, r.error.as_str()), (0, "x"));
    }
}
