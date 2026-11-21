//! `claude -p` 한 번 실행.
//!
//! 입출력은 stream-json이다. 요청은 stdin에 user 메시지 한 줄로 쓰고 닫는다.
//! 자식 프로세스의 환경 변수는 비우고 허용한 것만 넘긴다. 데몬의 비밀이 환경으로 새지
//! 않게 하고, 데몬을 띄운 셸(예: 다른 Claude Code 세션)의 설정이 섞이지 않게 하기 위해서다.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

use crate::stream_json::{self, Event, Init, RunResult};

/// 세션을 새로 열지, 이어 붙일지.
#[derive(Debug, Clone, Copy)]
pub enum SessionMode {
    /// `--session-id <uuid>`
    New(Uuid),
    /// `-r <uuid>`
    Resume(Uuid),
}

/// 자식 프로세스로 넘기는 환경 변수 기본값.
pub const DEFAULT_ENV_PASSTHROUGH: &[&str] = &["HOME", "PATH", "LANG", "USER", "LOGNAME"];

#[derive(Debug, Clone)]
pub struct ClaudeCommand {
    /// 실행 파일. 호스트에서 직접 돌리면 `claude`, 샌드박스면 `sbx`.
    pub program: PathBuf,
    /// claude 인자 앞에 붙는 인자. 샌드박스 실행이면 [`sbx_exec_prefix`]로 만든다.
    pub prefix: Vec<String>,
    /// 호스트 프로세스의 작업 디렉터리. 샌드박스 안 작업 디렉터리는 `prefix`에서 정한다.
    pub cwd: PathBuf,
    pub session: SessionMode,
    pub model: Option<String>,
    /// `--mcp-config`: 파일 경로나 JSON 문자열. 넣으면 `--strict-mcp-config`도 붙는다.
    pub mcp_config: Option<String>,
    /// `--tools`: 내장 도구 목록. `Some(vec![])`이면 내장 도구를 모두 끈다.
    pub tools: Option<Vec<String>>,
    /// `--allowedTools`
    pub allowed_tools: Vec<String>,
    /// `--restricted`: 관리형 설정과 `--settings`만 읽는다. 작업 공간에 심은 프로젝트 설정을 막는다.
    pub restricted: bool,
    /// `--settings`: 파일 경로나 JSON 문자열.
    pub settings: Option<String>,
    /// `--plugin-dir`(여러 개 가능).
    pub plugin_dirs: Vec<String>,
    /// `--permission-prompts`: `host` 또는 `none`.
    pub permission_prompts: Option<String>,
    /// `--permission-prompt-tool`: 권한 확인을 맡길 MCP 도구(`mcp__<서버>__<도구>`).
    pub permission_prompt_tool: Option<String>,
    /// `--setting-sources`. `restricted`와 함께 쓰지 않는다.
    pub setting_sources: Option<String>,
    pub append_system_prompt_file: Option<PathBuf>,
    /// `--include-hook-events`
    pub include_hook_events: bool,
    /// `--include-partial-messages`: 텍스트·도구 입력 델타를 `stream_event`로 받는다.
    pub include_partial_messages: bool,
    pub extra_args: Vec<String>,
    /// 데몬 환경에서 그대로 넘길 변수 이름.
    pub env_passthrough: Vec<String>,
    /// 값을 지정해 넘길 변수. 비밀은 넣지 않는다.
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
}

impl ClaudeCommand {
    pub fn new(program: impl Into<PathBuf>, cwd: impl Into<PathBuf>, session: SessionMode) -> Self {
        Self {
            program: program.into(),
            prefix: Vec::new(),
            cwd: cwd.into(),
            session,
            model: None,
            mcp_config: None,
            tools: None,
            allowed_tools: Vec::new(),
            restricted: false,
            settings: None,
            plugin_dirs: Vec::new(),
            permission_prompts: None,
            permission_prompt_tool: None,
            setting_sources: None,
            append_system_prompt_file: None,
            include_hook_events: false,
            include_partial_messages: false,
            extra_args: Vec::new(),
            env_passthrough: DEFAULT_ENV_PASSTHROUGH
                .iter()
                .map(|s| s.to_string())
                .collect(),
            env: Vec::new(),
            timeout: Duration::from_secs(300),
        }
    }

    /// 프로그램 이름을 뺀 인자 목록(접두 인자 포함).
    pub fn args(&self) -> Vec<String> {
        let mut args = self.prefix.clone();
        args.extend(
            [
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        if self.restricted {
            args.push("--restricted".into());
        }
        if self.include_hook_events {
            args.push("--include-hook-events".into());
        }
        if self.include_partial_messages {
            args.push("--include-partial-messages".into());
        }

        match self.session {
            SessionMode::New(id) => args.extend(["--session-id".into(), id.to_string()]),
            SessionMode::Resume(id) => args.extend(["--resume".into(), id.to_string()]),
        }
        if let Some(model) = &self.model {
            args.extend(["--model".into(), model.clone()]);
        }
        if let Some(config) = &self.mcp_config {
            args.extend([
                "--mcp-config".into(),
                config.clone(),
                "--strict-mcp-config".into(),
            ]);
        }
        if let Some(settings) = &self.settings {
            args.extend(["--settings".into(), settings.clone()]);
        }
        for dir in &self.plugin_dirs {
            args.extend(["--plugin-dir".into(), dir.clone()]);
        }
        if let Some(target) = &self.permission_prompts {
            args.extend(["--permission-prompts".into(), target.clone()]);
        }
        if let Some(tool) = &self.permission_prompt_tool {
            args.extend(["--permission-prompt-tool".into(), tool.clone()]);
        }
        if let Some(tools) = &self.tools {
            args.push("--tools".into());
            args.push(tools.join(","));
        }
        if !self.allowed_tools.is_empty() {
            // 가변 인자 옵션이라 값 하나로 묶어 넘긴다(뒤 인자를 도구 이름으로 먹지 않게).
            args.push("--allowedTools".into());
            args.push(self.allowed_tools.join(","));
        }
        if let Some(sources) = &self.setting_sources {
            args.extend(["--setting-sources".into(), sources.clone()]);
        }
        if let Some(path) = &self.append_system_prompt_file {
            args.extend([
                "--append-system-prompt-file".into(),
                path.display().to_string(),
            ]);
        }
        args.extend(self.extra_args.iter().cloned());
        args
    }

    /// 요청 하나를 보내고 끝날 때까지 기다린다. `record`가 있으면 stdout 원본 줄을 그 파일에 쓴다.
    pub async fn run(&self, prompt: &str, record: Option<&Path>) -> Result<RunOutput, RunError> {
        self.run_streaming(prompt, &[], record, |_| {}).await
    }

    /// 요청 하나를 보낸다. `blocks`는 글 뒤에 붙는 내용 블록(이미지·PDF 등)이다. `on_event`는 이벤트가 올 때마다
    /// 바로 불린다(나눠 보내기, 도구 사용 알림). `stream_event`는 넘기기만 하고 결과에 모아 두지 않는다.
    pub async fn run_streaming(
        &self,
        prompt: &str,
        blocks: &[serde_json::Value],
        record: Option<&Path>,
        mut on_event: impl FnMut(&Event) + Send,
    ) -> Result<RunOutput, RunError> {
        let mut cmd = Command::new(&self.program);
        cmd.args(self.args())
            .current_dir(&self.cwd)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for key in &self.env_passthrough {
            if let Ok(value) = std::env::var(key) {
                cmd.env(key, value);
            }
        }
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)));

        let mut child = cmd.spawn().map_err(RunError::Spawn)?;

        let mut stdin = child.stdin.take().expect("stdin piped");
        let line = user_message_line(prompt, blocks);
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(RunError::Io)?;
        drop(stdin);

        let stdout = child.stdout.take().expect("stdout piped");
        let mut stderr = child.stderr.take().expect("stderr piped");
        let stderr_task = tokio::spawn(async move {
            let mut buf = String::new();
            let _ = stderr.read_to_string(&mut buf).await;
            buf
        });

        let mut recorder = match record {
            Some(path) => Some(tokio::fs::File::create(path).await.map_err(RunError::Io)?),
            None => None,
        };

        let read_all = async {
            let mut events = Vec::new();
            let mut parse_errors = Vec::new();
            let mut lines = BufReader::new(stdout).lines();
            while let Some(line) = lines.next_line().await.map_err(RunError::Io)? {
                if let Some(file) = recorder.as_mut() {
                    file.write_all(line.as_bytes())
                        .await
                        .map_err(RunError::Io)?;
                    file.write_all(b"\n").await.map_err(RunError::Io)?;
                }
                match stream_json::parse_line(&line) {
                    Ok(Some(event)) => {
                        on_event(&event);
                        if !matches!(event, Event::StreamEvent(_)) {
                            events.push(event);
                        }
                    }
                    Ok(None) => {}
                    Err(err) => parse_errors.push(format!("{err}: {}", truncate(&line, 200))),
                }
            }
            if let Some(file) = recorder.as_mut() {
                file.flush().await.map_err(RunError::Io)?;
            }
            let status = child.wait().await.map_err(RunError::Io)?;
            Ok::<_, RunError>((events, parse_errors, status))
        };

        let (events, parse_errors, status) = tokio::time::timeout(self.timeout, read_all)
            .await
            .map_err(|_| RunError::Timeout(self.timeout))??;
        let stderr = stderr_task.await.unwrap_or_default();

        Ok(RunOutput {
            events,
            parse_errors,
            exit_code: status.code(),
            stderr,
        })
    }
}

/// `sbx exec -i [-w 디렉터리] [-e KEY=VALUE]... <샌드박스> claude` 접두 인자.
///
/// 샌드박스 안 claude의 환경 변수는 호스트 환경이 아니라 `-e`로만 넘어간다. 비밀은 넣지 않는다.
pub fn sbx_exec_prefix(
    sandbox: &str,
    workdir: Option<&str>,
    env: &[(String, String)],
) -> Vec<String> {
    let mut prefix = vec!["exec".to_owned(), "-i".to_owned()];
    if let Some(dir) = workdir {
        prefix.extend(["-w".to_owned(), dir.to_owned()]);
    }
    for (key, value) in env {
        prefix.extend(["-e".to_owned(), format!("{key}={value}")]);
    }
    prefix.extend([sandbox.to_owned(), "claude".to_owned()]);
    prefix
}

fn user_message_line(prompt: &str, blocks: &[serde_json::Value]) -> String {
    let content = if blocks.is_empty() {
        serde_json::Value::String(prompt.to_owned())
    } else {
        let mut all = vec![serde_json::json!({ "type": "text", "text": prompt })];
        all.extend(blocks.iter().cloned());
        serde_json::Value::Array(all)
    };
    let value = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": content },
    });
    format!("{value}\n")
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

#[derive(Debug)]
pub struct RunOutput {
    pub events: Vec<Event>,
    pub parse_errors: Vec<String>,
    pub exit_code: Option<i32>,
    pub stderr: String,
}

impl RunOutput {
    pub fn init(&self) -> Option<&Init> {
        self.events.iter().find_map(|e| match e {
            Event::Init(init) => Some(init.as_ref()),
            _ => None,
        })
    }

    pub fn result(&self) -> Option<&RunResult> {
        self.events.iter().rev().find_map(|e| match e {
            Event::Result(result) => Some(result.as_ref()),
            _ => None,
        })
    }

    /// compact 결과(있으면 마지막 것).
    pub fn compact_boundary(&self) -> Option<&stream_json::CompactBoundary> {
        self.events.iter().rev().find_map(|e| match e {
            Event::CompactBoundary(b) => Some(b.as_ref()),
            _ => None,
        })
    }

    /// compact 요약 본문(앞뒤 안내 문구 포함, 있으면 마지막 것).
    pub fn compact_summary(&self) -> Option<&str> {
        self.events.iter().rev().find_map(|e| match e {
            Event::User(raw) => stream_json::compact_summary(raw),
            _ => None,
        })
    }

    /// 이 실행에서 모델이 부른 도구(이름, 입력)(순서대로).
    pub fn tool_calls(&self) -> Vec<(String, serde_json::Value)> {
        self.events
            .iter()
            .filter_map(|e| match e {
                Event::Assistant(raw) => Some(stream_json::tool_calls(raw)),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// 이 실행에서 모델이 부른 도구 이름(순서대로).
    pub fn tool_uses(&self) -> Vec<&str> {
        self.events
            .iter()
            .filter_map(|e| match e {
                Event::Assistant(raw) => Some(stream_json::tool_uses(raw)),
                _ => None,
            })
            .flatten()
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("claude 실행 실패: {0}")]
    Spawn(std::io::Error),
    #[error("입출력 오류: {0}")]
    Io(std::io::Error),
    #[error("{0:?} 안에 끝나지 않았다")]
    Timeout(Duration),
}
