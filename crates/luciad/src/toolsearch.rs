//! 도구 찾기(결정 0022): 모델의 도구 목록에는 자주 쓰는 도구만 두고, 나머지는 대분류 색인과 검색으로 찾는다.
//!
//! - `tool_search`의 설명에 대분류 색인(설명과 도구 이름)이 들어간다. 실행과 상관없이 같아서 프롬프트 캐시를
//!   깨지 않는다.
//! - 검색 결과에는 도구 설명과 파라미터(형, 필수 여부, 설명)가 함께 나와, 한 번 찾고 바로 `tool_call`로 부른다.
//! - 이 실행에서 쓸 수 없는 도구는 찾기에 나오지 않는다(부르기도 서버가 다시 막는다).

use rmcp::model::Tool;
use serde_json::{Map, Value};

use crate::tools::{self, CALL_TOOL, Category, Policy, SEARCH_TOOL};

/// 한 번에 돌려주는 도구 수(검색어로 찾을 때).
const MAX_RESULTS: usize = 5;

/// `tool_search`의 설명(색인 포함).
pub fn search_description() -> String {
    format!(
        "도구 목록에 없는 Lucia 도구를 찾는다. 지침이나 기억에 이름이 나온 도구(예: mail_send, browser_click, task_list)가 목록에 없으면 여기서 찾는다. category에 대분류 id를 주면 그 대분류의 도구를, query에 검색어를 주면 맞는 도구를 설명·파라미터와 함께 돌려준다. 찾은 도구는 {CALL_TOOL}(name, arguments)로 바로 부른다(다시 찾지 않는다).\n{}",
        tools::category_index()
    )
}

/// 찾는다. `tools`는 서버의 모든 도구 정의(파라미터 스키마), `policy`는 이 실행의 범위.
pub fn search(
    tools: &[Tool],
    policy: &Policy,
    query: Option<&str>,
    category: Option<&str>,
) -> String {
    let usable = |name: &str| {
        name != "report_result"
            && tools::spec(name).is_some_and(|s| s.category.is_some() && policy.allows_spec(s))
    };
    let defined = |name: &str| tools.iter().find(|t| t.name == name);
    let category = category.map(str::trim).filter(|c| !c.is_empty());
    let query = query.map(str::trim).filter(|q| !q.is_empty());

    let found: Vec<&'static tools::ToolSpec> = if let Some(c) = category {
        let Some(cat) = Category::parse(c) else {
            let ids: Vec<&str> = Category::ALL.iter().map(|c| c.id()).collect();
            return format!("모르는 대분류 {c:?}. 대분류 id: {}", ids.join(", "));
        };
        let words = query.map(words).unwrap_or_default();
        tools::TOOLS
            .iter()
            .filter(|t| t.category == Some(cat) && usable(t.name))
            .filter(|t| words.is_empty() || score(t, defined(t.name), &words) > 0)
            .collect()
    } else if let Some(q) = query {
        let words = words(q);
        let mut scored: Vec<(usize, usize, &'static tools::ToolSpec)> = tools::TOOLS
            .iter()
            .enumerate()
            .filter(|(_, t)| usable(t.name))
            .map(|(i, t)| (score(t, defined(t.name), &words), i, t))
            .filter(|(s, _, _)| *s > 0)
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored
            .into_iter()
            .take(MAX_RESULTS)
            .map(|(_, _, t)| t)
            .collect()
    } else {
        return format!(
            "query(검색어)나 category(대분류 id)를 준다.\n{}",
            tools::category_index()
        );
    };

    if found.is_empty() {
        return format!(
            "이 실행에서 쓸 수 있는 맞는 도구가 없다. 대분류를 보고 다시 찾거나, 도구 없이 할 수 있는지 본다.\n{}",
            tools::category_index()
        );
    }
    let mut out = format!(
        "찾은 도구 {}개. {CALL_TOOL}로 부른다: {{\"name\": \"<도구>\", \"arguments\": {{…}}}}\n",
        found.len()
    );
    for spec in found {
        let Some(tool) = defined(spec.name) else {
            continue;
        };
        out.push_str(&format!(
            "\n## {} ({})\n{}\n",
            spec.name,
            spec.category.map(Category::label).unwrap_or(""),
            tool.description.as_deref().unwrap_or("").trim()
        ));
        out.push_str(&render_params(&tool.input_schema));
    }
    out
}

fn words(query: &str) -> Vec<String> {
    query
        .split(|c: char| c.is_whitespace() || c == ',' || c == '·')
        .map(|w| w.trim().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// 맞는 정도: 이름 3, 대분류 2, 설명·파라미터 1(단어마다).
fn score(spec: &tools::ToolSpec, tool: Option<&Tool>, words: &[String]) -> usize {
    let desc = tool
        .and_then(|t| t.description.as_deref())
        .unwrap_or("")
        .to_lowercase();
    let params = tool
        .map(|t| {
            Value::Object((*t.input_schema).clone())
                .to_string()
                .to_lowercase()
        })
        .unwrap_or_default();
    let (cid, clabel) = spec
        .category
        .map(|c| (c.id(), c.label()))
        .unwrap_or(("", ""));
    words
        .iter()
        .map(|w| {
            let w = w.as_str();
            let mut s = 0;
            if spec.name.contains(w) || w.contains(spec.name) {
                s += 3;
            }
            if (!cid.is_empty() && (cid.contains(w) || w.contains(cid)))
                || (!clabel.is_empty() && (clabel.contains(w) || w.contains(clabel)))
            {
                s += 2;
            }
            if desc.contains(w) {
                s += 1;
            }
            if params.contains(w) {
                s += 1;
            }
            s
        })
        .sum()
}

/// 파라미터 목록: `- 이름 (형, 필수): 설명`.
pub fn render_params(schema: &Map<String, Value>) -> String {
    let defs = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .and_then(Value::as_object);
    let props = schema.get("properties").and_then(Value::as_object);
    let Some(props) = props.filter(|p| !p.is_empty()) else {
        return "파라미터: 없음({})\n".into();
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut names: Vec<&String> = props.keys().collect();
    // 필수를 먼저.
    names.sort_by_key(|n| !required.contains(&n.as_str()));
    let mut out = String::from("파라미터:\n");
    for name in names {
        let p = &props[name];
        let ty = type_of(p, defs, 0);
        let need = if required.contains(&name.as_str()) {
            ", 필수"
        } else {
            ""
        };
        let desc = describe(p, defs);
        if desc.is_empty() {
            out.push_str(&format!("- {name} ({ty}{need})\n"));
        } else {
            out.push_str(&format!("- {name} ({ty}{need}): {desc}\n"));
        }
    }
    out
}

fn resolve<'a>(v: &'a Value, defs: Option<&'a Map<String, Value>>) -> &'a Value {
    if let Some(r) = v.get("$ref").and_then(Value::as_str) {
        let name = r.rsplit('/').next().unwrap_or("");
        if let Some(d) = defs.and_then(|d| d.get(name)) {
            return d;
        }
    }
    v
}

fn describe(v: &Value, defs: Option<&Map<String, Value>>) -> String {
    let own = v.get("description").and_then(Value::as_str);
    let text = own.or_else(|| resolve(v, defs).get("description").and_then(Value::as_str));
    text.unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn type_of(v: &Value, defs: Option<&Map<String, Value>>, depth: usize) -> String {
    let v = resolve(v, defs);
    if let Some(e) = v.get("enum").and_then(Value::as_array) {
        return e
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join("|");
    }
    if let Some(c) = v.get("const") {
        return c.to_string();
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(alts) = v.get(key).and_then(Value::as_array) {
            let parts: Vec<String> = alts
                .iter()
                .filter(|a| a.get("type").and_then(Value::as_str) != Some("null"))
                .map(|a| type_of(a, defs, depth))
                .collect();
            return parts.join("|");
        }
    }
    let ty = match v.get("type") {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Array(ts)) => ts
            .iter()
            .filter_map(Value::as_str)
            .filter(|t| *t != "null")
            .collect::<Vec<_>>()
            .join("|"),
        _ => {
            if v.get("properties").is_some() {
                "object".into()
            } else {
                "any".into()
            }
        }
    };
    match ty.as_str() {
        "array" => {
            let item = v
                .get("items")
                .map(|i| type_of(i, defs, depth + 1))
                .unwrap_or_else(|| "any".into());
            format!("array<{item}>")
        }
        "object" if depth < 3 => match v.get("properties").and_then(Value::as_object) {
            Some(props) if !props.is_empty() => {
                let required: Vec<&str> = v
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let fields: Vec<String> = props
                    .iter()
                    .map(|(k, p)| {
                        let opt = if required.contains(&k.as_str()) {
                            ""
                        } else {
                            "?"
                        };
                        format!("{k}{opt}: {}", type_of(p, defs, depth + 1))
                    })
                    .collect();
                format!("{{{}}}", fields.join(", "))
            }
            _ => "object".into(),
        },
        _ => ty,
    }
}

/// `tool_call` 입력을 실제 도구 호출로 푼다: (도구 이름, 파라미터). 모델에게 돌려줄 거절 사유는 `Err`.
pub fn resolve_call(
    policy: &Policy,
    input: &Map<String, Value>,
) -> Result<(&'static str, Map<String, Value>), String> {
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .map(|n| n.trim().trim_start_matches("mcp__lucia__"))
        .filter(|n| !n.is_empty())
        .ok_or_else(|| format!("{CALL_TOOL}에는 name(도구 이름)이 필요하다."))?;
    if name == CALL_TOOL || name == SEARCH_TOOL {
        return Err(format!(
            "{CALL_TOOL}로 {name}을(를) 부르지 않는다. 직접 부른다."
        ));
    }
    let Some(spec) = tools::spec(name) else {
        return Err(format!(
            "모르는 도구 {name:?}. {SEARCH_TOOL}로 이름과 파라미터를 찾는다."
        ));
    };
    if !policy.allows_spec(spec) {
        return Err(format!("이 실행에서는 {name}을(를) 쓸 수 없다."));
    }
    let args = match input.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        // 모델이 파라미터를 JSON 글로 넣는 경우.
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Object(m)) => m,
            _ => return Err("arguments는 파라미터 객체여야 한다.".into()),
        },
        Some(_) => return Err("arguments는 파라미터 객체여야 한다.".into()),
    };
    Ok((spec.name, args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::RunProfile;
    use serde_json::json;
    use std::sync::Arc;

    fn tool(name: &str, desc: &str, schema: Value) -> Tool {
        let Value::Object(m) = schema else { panic!() };
        Tool::new(name.to_owned(), desc.to_owned(), Arc::new(m))
    }

    fn sample() -> Vec<Tool> {
        vec![
            tool(
                "mail_send",
                "메일 발송을 요청한다. 사용자가 Discord 카드에서 [보내기]를 눌러야 보낸다.",
                json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}, "description": "받는 사람 주소"},
                        "subject": {"type": "string", "description": "제목"},
                        "from": {"type": ["string", "null"], "description": "보내는 주소"},
                        "mode": {"$ref": "#/$defs/Mode"},
                    },
                    "required": ["to", "subject"],
                    "$defs": {"Mode": {"enum": ["plain", "html"], "description": "형식"}},
                }),
            ),
            tool(
                "browser_click",
                "요소를 누른다.",
                json!({"type": "object", "properties": {"ref": {"type": "string"}}, "required": ["ref"]}),
            ),
            tool(
                "browser_back",
                "앞 페이지로 돌아간다.",
                json!({"type": "object", "properties": {}}),
            ),
            tool(
                "followup_list",
                "열린 후속 조치 목록을 본다.",
                json!({"type": "object"}),
            ),
        ]
    }

    #[test]
    fn params_show_types_requirement_and_descriptions() {
        let tools = sample();
        let text = render_params(&tools[0].input_schema);
        assert!(
            text.contains("- to (array<string>, 필수): 받는 사람 주소"),
            "{text}"
        );
        assert!(text.contains("- subject (string, 필수): 제목"), "{text}");
        assert!(text.contains("- from (string): 보내는 주소"), "{text}");
        assert!(text.contains("- mode (\"plain\"|\"html\"): 형식"), "{text}");
        let first_optional = text.find("- from").unwrap();
        assert!(text.find("- to").unwrap() < first_optional, "필수가 먼저");
        assert!(render_params(&tools[2].input_schema).contains("없음"));
        assert!(render_params(&tools[3].input_schema).contains("없음"));
    }

    #[test]
    fn search_by_query_or_category_within_the_run_policy() {
        let tools = sample();
        let chat = RunProfile::Chat.grants();
        let found = search(&tools, &chat, Some("메일 보내기"), None);
        assert!(found.contains("## mail_send (메일·일정)"), "{found}");
        assert!(found.contains("- to (array<string>, 필수)"), "{found}");
        assert!(found.contains("tool_call"), "{found}");

        let browser = search(&tools, &chat, None, Some("browser"));
        assert!(browser.contains("## browser_click") && browser.contains("## browser_back"));
        assert!(!browser.contains("mail_send"));
        assert_eq!(
            search(&tools, &chat, None, Some("브라우저")),
            browser,
            "대분류 이름으로도"
        );

        // 브리핑(배경 실행)은 위임 사이트를 읽는 도구만 찾는다(결정 0028). 누르기·외부 쓰기는 나오지 않는다.
        let background = RunProfile::Background.grants();
        let read_only = search(&tools, &background, None, Some("browser"));
        assert!(read_only.contains("## browser_back"), "{read_only}");
        assert!(!read_only.contains("## browser_click"), "{read_only}");
        assert!(!search(&tools, &background, Some("메일 보내기"), None).contains("## mail_send"));

        assert!(search(&tools, &chat, None, Some("nope")).contains("모르는 대분류"));
        let index = search(&tools, &chat, None, None);
        assert!(index.contains("google, 메일·일정"), "{index}");
    }

    #[test]
    fn calls_resolve_to_allowed_tools_only() {
        let chat = RunProfile::Chat.grants();
        let m = |v: Value| v.as_object().unwrap().clone();
        let (name, args) = resolve_call(
            &chat,
            &m(json!({"name": "mail_send", "arguments": {"to": ["a@b.c"]}})),
        )
        .unwrap();
        assert_eq!(name, "mail_send");
        assert_eq!(args["to"][0], "a@b.c");
        let (name, args) = resolve_call(
            &chat,
            &m(json!({"name": "mcp__lucia__followup_list", "arguments": "{}"})),
        )
        .unwrap();
        assert_eq!((name, args.len()), ("followup_list", 0));
        assert!(resolve_call(&chat, &m(json!({"name": "tool_call"}))).is_err());
        assert!(
            resolve_call(&chat, &m(json!({"name": "rm_rf"})))
                .unwrap_err()
                .contains("모르는 도구")
        );
        assert!(resolve_call(&chat, &m(json!({"arguments": {}}))).is_err());
        assert!(resolve_call(&chat, &m(json!({"name": "mail_send", "arguments": 3}))).is_err());
        let background = RunProfile::Background.grants();
        assert!(
            resolve_call(&background, &m(json!({"name": "mail_send"})))
                .unwrap_err()
                .contains("쓸 수 없다")
        );
    }
}
