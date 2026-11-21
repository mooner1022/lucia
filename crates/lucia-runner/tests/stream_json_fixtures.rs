//! 기록된 실제 출력으로 파서를 검증한다.

use lucia_runner::stream_json::{Event, parse_line};

fn events(fixture: &str) -> Vec<Event> {
    let path = format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).expect("fixture");
    text.lines()
        .filter_map(|line| parse_line(line).expect("파싱 실패"))
        .collect()
}

#[test]
fn host_pong_run_parses() {
    let events = events("host-2.1.325-pong.jsonl");
    assert_eq!(events.len(), 4);

    let Event::Init(init) = &events[0] else {
        panic!("첫 이벤트는 init이어야 한다: {:?}", events[0]);
    };
    assert_eq!(init.claude_code_version.as_deref(), Some("2.1.325"));
    assert_eq!(init.permission_mode.as_deref(), Some("default"));
    assert!(init.tools.is_empty(), "--tools \"\"로 내장 도구를 껐다");

    assert!(matches!(events[1], Event::Assistant(_)));

    let Event::RateLimit(rate) = &events[2] else {
        panic!("rate_limit_event가 있어야 한다");
    };
    assert_eq!(rate.status.as_deref(), Some("allowed"));
    assert!(rate.unified_windows.is_some());

    let Event::Result(result) = &events[3] else {
        panic!("마지막은 result여야 한다");
    };
    assert_eq!(result.subtype, "success");
    assert!(!result.is_error);
    assert_eq!(result.session_id, init.session_id);
    assert_eq!(result.num_turns, 1);
    assert_eq!(result.result.as_deref(), Some("pong"));
    assert!(result.total_cost_usd.unwrap_or(0.0) > 0.0);
    assert!(result.usage.total_input() > 0);
    assert!(result.credential_expires_at.is_some());
    assert_eq!(result.context_window(), Some(1_000_000));
}

/// `luciad poc`가 호스트 claude로 남긴 기록: MCP 연결, report_result 호출, 세션 재개.
#[test]
fn host_poc_runs_parse() {
    use lucia_runner::stream_json::tool_uses;

    let first = events("host-2.1.325-poc-request1.jsonl");
    let second = events("host-2.1.325-poc-request2.jsonl");

    for run in [&first, &second] {
        let Event::Init(init) = &run[0] else {
            panic!("첫 이벤트는 init이어야 한다");
        };
        assert!(init.mcp_connected("lucia"));
        assert!(!init.mcp_connected("other"));

        let calls: Vec<&str> = run
            .iter()
            .filter_map(|e| match e {
                Event::Assistant(raw) => Some(tool_uses(raw)),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(calls, ["mcp__lucia__report_result"]);

        let Some(Event::Result(result)) = run.last() else {
            panic!("마지막은 result여야 한다");
        };
        assert_eq!(result.subtype, "success");
        assert!(result.permission_denials.is_empty());
    }

    // usage는 턴 합계, 컨텍스트 크기는 마지막 턴(iterations) 기준이다.
    // 요청 1: 2턴, 합계 입력 9275 / 마지막 턴 2 + 205 + 4533 = 4740.
    let Some(Event::Result(r1)) = first.last() else {
        unreachable!()
    };
    assert_eq!(r1.num_turns, 2);
    assert_eq!(r1.usage.total_input(), 9275);
    assert_eq!(r1.context_tokens(), 4740);

    // 두 번째 요청은 첫 요청의 세션을 이어 붙였다.
    let session = |run: &[Event]| match run.last() {
        Some(Event::Result(r)) => r.session_id.clone(),
        _ => unreachable!(),
    };
    assert_eq!(session(&first), session(&second));

    // 첫 기록에는 아직 따로 다루지 않는 system 이벤트(thinking_tokens)가 있다. Other로 남아야 한다.
    assert!(
        first
            .iter()
            .any(|e| matches!(e, Event::Other { kind, .. } if kind == "system"))
    );
}

#[test]
fn unknown_event_is_kept() {
    let event = parse_line(r#"{"type":"brand_new","x":1}"#)
        .unwrap()
        .unwrap();
    match event {
        Event::Other { kind, raw } => {
            assert_eq!(kind, "brand_new");
            assert_eq!(raw["x"], 1);
        }
        other => panic!("Other여야 한다: {other:?}"),
    }
}

#[test]
fn blank_line_is_skipped() {
    assert!(parse_line("   ").unwrap().is_none());
}

#[test]
fn missing_type_is_error() {
    assert!(parse_line(r#"{"a":1}"#).is_err());
}

/// VM 안 claude 2.1.325에서 `/compact`를 stream-json 입력으로 보낸 기록(2026-11-16).
#[test]
fn vm_compact_run_parses() {
    let events = events("vm-2.1.325-compact.jsonl");
    let boundary = events
        .iter()
        .find_map(|e| match e {
            Event::CompactBoundary(b) => Some(b.clone()),
            _ => None,
        })
        .expect("compact_boundary");
    assert_eq!(boundary.trigger.as_deref(), Some("manual"));
    assert!(boundary.pre_tokens > boundary.post_tokens && boundary.post_tokens > 0);
    let summary = events
        .iter()
        .find_map(|e| match e {
            Event::User(raw) => lucia_runner::stream_json::compact_summary(raw).map(str::to_owned),
            _ => None,
        })
        .expect("요약 메시지");
    assert!(summary.contains("Summary:"));
    let Some(Event::Result(result)) = events.last() else {
        panic!("마지막은 result");
    };
    assert_eq!(result.num_turns, 0, "compact는 턴이 아니다");
    assert!(!result.is_error);
}

/// 실제 VM 실행(sonnet, 데몬 MCP)의 report_result 입력 델타(2026-11-16, 구분자 포함 답).
/// 조각을 차례로 풀면 매번 최종값의 앞부분이고, 끝나면 assistant 이벤트의 summary와 같다.
#[test]
fn vm_report_stream_reassembles_the_summary() {
    use lucia_runner::report_stream::ReportStream;
    let mut stream = ReportStream::new("mcp__lucia__report_result", "summary");
    let mut last = String::new();
    let mut expected = None;
    let mut updates = 0;
    for event in events("vm-2.1.325-report-stream.jsonl") {
        match event {
            Event::StreamEvent(ev) => {
                if let Some(so_far) = stream.feed(&ev) {
                    assert!(so_far.starts_with(&last), "늘기만 한다");
                    last = so_far;
                    updates += 1;
                }
            }
            Event::Assistant(raw) => {
                let calls = lucia_runner::stream_json::tool_calls(&raw);
                expected = calls[0].1["summary"].as_str().map(str::to_owned);
            }
            _ => {}
        }
    }
    let expected = expected.expect("report_result 호출");
    assert_eq!(last, expected);
    assert!(updates > 5, "조각으로 왔다: {updates}");
    assert!(expected.contains("\n%%\n"), "구분자가 있는 답");
    assert_eq!(stream.calls, 1);
}
