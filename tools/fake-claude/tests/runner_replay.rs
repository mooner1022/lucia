//! lucia-runner의 `ClaudeCommand::run`을 fake-claude로 돌려 본다. 모델은 부르지 않는다.

use std::path::PathBuf;
use std::time::Duration;

use lucia_runner::{ClaudeCommand, SessionMode};
use uuid::Uuid;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/lucia-runner/tests/fixtures")
        .join(name)
}

fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fake-claude-test-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test(flavor = "current_thread")]
async fn run_replays_fixture_and_builds_args() {
    let dir = scratch_dir();
    let args_out = dir.join("args.txt");
    let record = dir.join("record.jsonl");
    let session = Uuid::new_v4();

    let mut cmd = ClaudeCommand::new(
        env!("CARGO_BIN_EXE_fake-claude"),
        &dir,
        SessionMode::Resume(session),
    );
    cmd.model = Some("haiku".into());
    cmd.mcp_config = Some(r#"{"mcpServers":{}}"#.into());
    cmd.restricted = true;
    cmd.permission_prompts = Some("none".into());
    cmd.plugin_dirs = vec!["/opt/lucia/plugin".into()];
    cmd.settings = Some("/opt/lucia/settings.json".into());
    cmd.tools = Some(Vec::new());
    cmd.allowed_tools = vec![
        "mcp__lucia__report_result".into(),
        "mcp__lucia__memory_search".into(),
    ];
    cmd.setting_sources = Some("project".into());
    cmd.timeout = Duration::from_secs(30);
    cmd.env = vec![
        (
            "FAKE_CLAUDE_FIXTURE".into(),
            fixture("host-2.1.325-poc-request2.jsonl")
                .display()
                .to_string(),
        ),
        (
            "FAKE_CLAUDE_ARGS_OUT".into(),
            args_out.display().to_string(),
        ),
    ];

    let out = cmd.run("요청", Some(&record)).await.expect("run");

    assert_eq!(out.exit_code, Some(0));
    assert!(out.parse_errors.is_empty(), "{:?}", out.parse_errors);
    assert!(out.init().is_some_and(|i| i.mcp_connected("lucia")));
    assert_eq!(out.tool_uses(), ["mcp__lucia__report_result"]);
    assert_eq!(out.result().map(|r| r.subtype.as_str()), Some("success"));

    // 원본 기록은 픽스처와 같아야 한다.
    let recorded = std::fs::read_to_string(&record).unwrap();
    let original = std::fs::read_to_string(fixture("host-2.1.325-poc-request2.jsonl")).unwrap();
    assert_eq!(recorded.trim_end(), original.trim_end());

    // 실제로 넘어간 인자 조합.
    let args: Vec<String> = std::fs::read_to_string(&args_out)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let pair = |flag: &str| {
        let i = args
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag} 없음: {args:?}"));
        args[i + 1].clone()
    };
    assert_eq!(args[0], "-p");
    assert_eq!(pair("--input-format"), "stream-json");
    assert_eq!(pair("--output-format"), "stream-json");
    assert!(args.contains(&"--verbose".to_owned()));
    assert_eq!(pair("--resume"), session.to_string());
    assert!(!args.contains(&"--session-id".to_owned()));
    assert_eq!(pair("--model"), "haiku");
    assert!(args.contains(&"--strict-mcp-config".to_owned()));
    assert_eq!(pair("--mcp-config"), r#"{"mcpServers":{}}"#);
    assert!(args.contains(&"--restricted".to_owned()));
    assert_eq!(pair("--permission-prompts"), "none");
    assert_eq!(pair("--plugin-dir"), "/opt/lucia/plugin");
    assert_eq!(pair("--settings"), "/opt/lucia/settings.json");
    assert_eq!(pair("--tools"), "");
    assert_eq!(
        pair("--allowedTools"),
        "mcp__lucia__report_result,mcp__lucia__memory_search"
    );
    assert_eq!(pair("--setting-sources"), "project");
    assert!(!args.contains(&"--bare".to_owned()));

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test(flavor = "current_thread")]
async fn child_env_is_cleared() {
    // 자식은 허용 목록(passthrough)과 명시한 변수(env)만 받아야 한다.
    // 이 테스트 프로세스에는 HOME, CARGO_* 같은 다른 변수가 있으므로, 환경을 비우지 않으면 실패한다.
    assert!(
        std::env::vars_os().count() > 1,
        "테스트 전제: 부모 환경에 PATH 말고도 변수가 있어야 한다"
    );

    let dir = scratch_dir();
    let env_out = dir.join("env.txt");
    let mut cmd = ClaudeCommand::new(
        env!("CARGO_BIN_EXE_fake-claude"),
        &dir,
        SessionMode::New(Uuid::new_v4()),
    );
    cmd.env_passthrough = vec!["PATH".into()];
    cmd.env = vec![
        (
            "FAKE_CLAUDE_FIXTURE".into(),
            fixture("host-2.1.325-pong.jsonl").display().to_string(),
        ),
        ("FAKE_CLAUDE_ENV_OUT".into(), env_out.display().to_string()),
    ];
    cmd.timeout = Duration::from_secs(30);

    let out = cmd.run("요청", None).await.expect("run");
    assert_eq!(out.exit_code, Some(0));

    let names: Vec<String> = std::fs::read_to_string(&env_out)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let mut expected = vec!["FAKE_CLAUDE_ENV_OUT", "FAKE_CLAUDE_FIXTURE"];
    if std::env::var_os("PATH").is_some() {
        expected.push("PATH");
    }
    expected.sort();
    assert_eq!(names, expected);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test(flavor = "current_thread")]
async fn sbx_prefix_comes_before_claude_args() {
    // 샌드박스 실행: `sbx exec -i -w <dir> -e K=V <샌드박스> claude -p ...` 순서여야 한다.
    let dir = scratch_dir();
    let args_out = dir.join("args.txt");
    let session = Uuid::new_v4();
    let mut cmd = ClaudeCommand::new(
        env!("CARGO_BIN_EXE_fake-claude"),
        &dir,
        SessionMode::New(session),
    );
    cmd.prefix = lucia_runner::sbx_exec_prefix(
        "lucia-poc",
        Some("/work"),
        &[("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "1".into())],
    );
    cmd.timeout = Duration::from_secs(30);
    cmd.env = vec![
        (
            "FAKE_CLAUDE_FIXTURE".into(),
            fixture("host-2.1.325-pong.jsonl").display().to_string(),
        ),
        (
            "FAKE_CLAUDE_ARGS_OUT".into(),
            args_out.display().to_string(),
        ),
    ];

    let out = cmd.run("요청", None).await.expect("run");
    assert_eq!(out.exit_code, Some(0));

    let args: Vec<String> = std::fs::read_to_string(&args_out)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        &args[..9],
        [
            "exec",
            "-i",
            "-w",
            "/work",
            "-e",
            "CLAUDE_CODE_DISABLE_AUTO_MEMORY=1",
            "lucia-poc",
            "claude",
            "-p"
        ]
    );
    let i = args.iter().position(|a| a == "--session-id").unwrap();
    assert_eq!(args[i + 1], session.to_string());

    std::fs::remove_dir_all(&dir).ok();
}
