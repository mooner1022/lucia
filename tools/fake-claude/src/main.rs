//! 가짜 claude CLI.
//!
//! 기록된 stream-json 픽스처를 stdout으로 재생해, 실행 계층을 모델 호출 없이 테스트한다
//! (명세 §운영 테스트). 진짜 `claude` 자리에 그대로 넣을 수 있게 인자는 모두 받아 무시한다.
//!
//! 사용법: `FAKE_CLAUDE_FIXTURE=<파일> fake-claude [아무 인자...]`
//! - stdin은 끝까지 읽고 버린다(요청 메시지).
//! - 받은 인자는 `FAKE_CLAUDE_ARGS_OUT`이 있으면 그 파일에 한 줄에 하나씩 쓴다(호출 조합 검증용).
//! - 받은 환경 변수 이름은 `FAKE_CLAUDE_ENV_OUT`이 있으면 그 파일에 정렬해 쓴다(환경 비우기 검증용).
//!   값은 쓰지 않는다.
//! - `FAKE_CLAUDE_EXIT`로 종료 코드를 정할 수 있다(기본 0).

use std::io::{Read, Write};

fn main() {
    if let Ok(path) = std::env::var("FAKE_CLAUDE_ENV_OUT") {
        let mut names: Vec<String> = std::env::vars_os()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        names.sort();
        if let Err(err) = std::fs::write(&path, names.join("\n")) {
            eprintln!("fake-claude: 환경 변수 이름을 기록하지 못했다: {err}");
            std::process::exit(2);
        }
    }

    let fixture = match std::env::var("FAKE_CLAUDE_FIXTURE") {
        Ok(path) => path,
        Err(_) => {
            eprintln!("fake-claude: FAKE_CLAUDE_FIXTURE가 없다");
            std::process::exit(2);
        }
    };

    if let Ok(path) = std::env::var("FAKE_CLAUDE_ARGS_OUT") {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if let Err(err) = std::fs::write(&path, args.join("\n")) {
            eprintln!("fake-claude: 인자를 기록하지 못했다: {err}");
            std::process::exit(2);
        }
    }

    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink);

    let text = match std::fs::read_to_string(&fixture) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("fake-claude: 픽스처를 읽지 못했다 {fixture}: {err}");
            std::process::exit(2);
        }
    };
    let mut out = std::io::stdout().lock();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if writeln!(out, "{line}").is_err() {
            std::process::exit(1);
        }
    }

    let code = std::env::var("FAKE_CLAUDE_EXIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    std::process::exit(code);
}
