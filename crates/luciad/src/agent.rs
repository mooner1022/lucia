//! 에이전트 실행기: 요청 하나를 Claude Code 세션에 보내고 구조화된 보고를 받는다.
//!
//! 실제 구현은 `sbx exec … claude -p`와 데몬 MCP(`report_result`)를 쓴다(PoC P4에서 확인한 조합).
//! 엔진은 [`AgentRunner`] 트레이트만 알므로, 시험에서는 [`FakeRunner`]로 바꿔 모델 없이 돌린다.

use std::future::Future;
#[cfg(test)]
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::mcp::ReportResult;
#[cfg(test)]
use crate::mcp::ReportStatus;

/// 세션을 새로 열지 이어 붙일지.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStart {
    New(Uuid),
    Resume(Uuid),
}

impl SessionStart {
    pub fn id(self) -> Uuid {
        match self {
            Self::New(id) | Self::Resume(id) => id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentRequest {
    /// 실행 ID. 실제 실행기는 이 값으로 MCP 토큰과 보고를 짝짓는다(시험 실행기는 쓰지 않는다).
    #[cfg_attr(test, allow(dead_code))]
    pub run_id: String,
    pub session: SessionStart,
    pub model: String,
    pub prompt: String,
    /// 요청 글 뒤에 붙는 내용 블록(첨부 이미지·PDF, 결정 0005).
    pub blocks: Vec<serde_json::Value>,
    /// 실행 중 이벤트를 받을 곳(채팅: 나눠 보내기, 도구 사용 알림). 있으면 부분 메시지를 켠다.
    pub events: Option<tokio::sync::mpsc::UnboundedSender<RunEvent>>,
    /// 실행 종류. 쓸 수 있는 도구가 달라진다(결정 0003).
    pub profile: RunProfile,
    /// 상주 샌드박스가 아닌 곳에서 돌릴 때(개발 작업의 임시 샌드박스, 결정 0020).
    pub target: Option<RunTarget>,
    /// 이 실행의 비용 상한(`--max-budget-usd`, 결정 0020).
    pub budget_usd: Option<f64>,
}

/// 실행할 샌드박스와 그 안의 작업 디렉터리.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTarget {
    pub sandbox: String,
    pub workdir: String,
}

/// 실행 중에 오는 일(결정 0005).
#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    /// `report_result`의 `summary`가 지금까지 받은 만큼(누적).
    ReportSoFar(String),
    /// 모델이 도구를 불렀다(완성된 입력).
    ToolUse {
        name: String,
        input: serde_json::Value,
    },
    /// 모델 API 요청이 실패해 Claude Code가 다시 보낸다(응답 없음, 429, 5xx 등). 사람에게 늦어지는 까닭을 알린다.
    Retrying {
        attempt: u64,
        max_retries: u64,
        /// 응답 머리를 받지 못하고 기다린 시간(첫 바이트 시간 초과).
        waited_ms: Option<u64>,
        error: String,
    },
}

/// 실행 종류별 도구(결정 0003). 읽기(메일·일정·웹 검색)와 핵심 기억 쓰기는 모두 쓴다.
/// 외부 쓰기 요청(메일 발송 승인 요청)은 사람이 시작한 채팅에서만 준다(명세: 백그라운드는 쓰기 도구를 뺀다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunProfile {
    Chat,
    Background,
    /// 보고 없이 끝난 실행을 같은 세션에서 한 번 더 청할 때: `report_result`만 쓴다. 앞 실행에서 이미 한 일
    /// (발송 요청, 기억 쓰기)을 되풀이하지 못하게 다른 도구는 모두 뺀다.
    ReportOnly,
    /// 관리 페이지의 "기억해 줘"(결정 0007): 기억 도구만 쓴다. 사람이 시작했으므로 지우기도 된다.
    Remember,
    /// 사람과의 대화에서 등록한 후속 조치 실행(결정 0010·0011·0013): 읽기, 기억 쓰기, 후속 조치 등록에 더해
    /// 브라우저와 메일 발송 요청을 쓴다(외부 쓰기는 승인 카드로). 로그인 요청·기억 지우기·후속 조치 취소는 없다.
    Followup,
    /// 브리핑에서 시작한 후속 조치 실행: 읽기, 기억 쓰기, 후속 조치 등록만. 외부 메일이 예약한 일이 사람 없이
    /// 페이지를 열어 정보를 내보내지 못하게 브라우저도 없다.
    FollowupLimited,
    /// 답에 약속이 있는데 등록하지 않았을 때 되묻는 실행(결정 0010): 후속 조치 등록만 한다.
    PromiseCheck,
    /// 작업 세션의 실행(결정 0014): 사람이 맡긴 일을 따로 세션에서 한다. 채팅에서 맡긴 후속 조치와 같은 범위에
    /// 진행 알리기(`task_update`)를 더한다. 새 작업 맡기기·취소는 없다(사람이 대화 중이 아니다).
    Task,
    /// 사람 없이 시작한 작업(결정 0013·0014, 나중의 루틴·기기 이벤트): 브리핑 범위에 진행 알리기만.
    TaskLimited,
    /// 하루 정리(결정 0016): 대화 기록을 읽고 기억 항목만 고친다(지우기 없음).
    Wrapup,
    /// 회고(결정 0018): 사람이 완료로 확인한 작업의 세션에서, 다시 쓸 절차를 스킬로 남긴다(스킬 도구만).
    Retro,
    /// 개발 작업(결정 0020): 임시 샌드박스에서 코드 도구(Bash, 편집)를 쓴다. 데몬 도구는 보고, 진행 알림,
    /// 스킬 읽기, push 요청(승인 카드)뿐이다.
    Dev,
    /// 목표 점검(결정 0032, 사람 없음): 브리핑 등급(읽기, 로컬 쓰기, 위임 사이트 읽기)에 목표 도구와 자기 목표의
    /// 하위 루틴·후속 조치 관리, 감시 제안(카드)을 더한다. 사이트 위임은 만들거나 넓히지 못한다.
    GoalReview,
}

impl RunProfile {
    /// 실행 종류별 범위(결정 0013): 열린 갈래, 최대 등급, 사람 있음. 도구별 허용은 [`crate::tools::TOOLS`]의
    /// 선언과 이것으로 계산한다.
    pub fn grants(self) -> crate::mcp::RunGrants {
        use crate::tools::{Grade::*, Group::*, Policy};
        match self {
            // 사람이 시작한 대화: 모두. 외부 쓰기는 도구 안에서 승인을 받는다.
            Self::Chat => Policy::new(
                &[
                    Report,
                    Memory,
                    Google,
                    Files,
                    Followups,
                    Browser,
                    Tasks,
                    Skills,
                    SkillWrite,
                    Watches,
                    Discord,
                    DiscordWrite,
                    Code,
                    Routines,
                    Goals,
                    Images,
                    Packages,
                    App,
                    Approvals,
                ],
                ExternalWrite,
                true,
            ),
            Self::Task => Policy::new(
                &[
                    Report,
                    Memory,
                    Google,
                    Files,
                    Followups,
                    Browser,
                    Tasks,
                    TaskRun,
                    Skills,
                    Watches,
                    Discord,
                    DiscordWrite,
                    Code,
                    // 사용자가 맡긴 작업은 그림을 만들 수 있다(결정 0046, 하루 예산 안에서).
                    Images,
                    // 맡긴 작업은 필요한 패키지를 설치할 수 있다(결정 0047).
                    Packages,
                    App,
                ],
                ExternalWrite,
                false,
            ),
            // 브리핑: 읽기와 기억 쓰기, 후속 조치 등록. 외부 쓰기 없음(명세: 백그라운드는 쓰기 도구를 뺀다).
            // 브라우저는 읽기만이고, 사용자가 위임한 사이트로만 나간다(결정 0028·0029, 브라우저 도구가 본다).
            // 목표 도구는 맡은 목표의 진행 기록만 남긴다(목표의 하위 후속 조치, 서버가 본다).
            Self::Background | Self::FollowupLimited => Policy::new(
                &[
                    Report, Memory, Google, Files, Followups, Browser, Code, Goals, App,
                ],
                LocalWrite,
                false,
            ),
            Self::GoalReview => Policy::new(
                &[
                    Report, Memory, Google, Files, Followups, Browser, Code, Routines, Watches,
                    Goals,
                ],
                LocalWrite,
                false,
            ),
            Self::ReportOnly => Policy::new(&[Report], Read, false),
            // 관리 페이지 "기억해 줘": 사람이 시작했으므로 지우기도 된다.
            Self::Remember => Policy::new(&[Report, Memory], LocalWrite, true),
            // 채팅에서 등록한 후속 조치(결정 0011·0013): 브라우저와 메일 발송 요청까지. 외부 쓰기는 모두 승인
            // 카드를 거친다. 사람이 대화 중이 아니므로 지우기·취소·로그인 요청은 없다.
            // 루틴 갈래는 자기를 깨운 루틴을 줄이는 데만 쓴다(결정 0028 I2: 만들기·지우기는 사람만).
            Self::Followup => Policy::new(
                &[
                    Report, Memory, Google, Files, Followups, Browser, Skills, Discord, Code,
                    Routines, Goals,
                ],
                ExternalWrite,
                false,
            ),
            Self::PromiseCheck => Policy::new(&[Followups], LocalWrite, false),
            Self::Wrapup => Policy::new(&[Report, Memory], LocalWrite, false),
            Self::Retro => Policy::new(&[Skills, SkillWrite], LocalWrite, false),
            Self::Dev => Policy::new(&[Report, TaskRun, Skills, DevPush], ExternalWrite, false),
            Self::TaskLimited => Policy::new(
                &[
                    Report, Memory, Google, Files, Followups, Browser, Tasks, TaskRun, Skills, Code,
                ],
                LocalWrite,
                false,
            ),
        }
    }
}

/// 실행 하나의 사용량(원장용).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunUsage {
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub context_tokens: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
    /// 모델이 돈 턴 수(결정 0018).
    pub num_turns: u32,
}

#[derive(Debug, Clone)]
pub struct AgentResponse {
    /// 이 실행의 토큰으로 들어온 보고.
    pub reports: Vec<ReportResult>,
    pub usage: RunUsage,
    /// 실행이 실패했으면 사유(인증 만료, 한도 초과, 시간 초과 등).
    pub error: Option<String>,
    /// 재개하려던 세션이 VM에 없었다(`No conversation found with session ID`). 엔진이 새 세션으로 다시 한다.
    pub session_missing: bool,
    /// `/compact`를 보냈을 때: 줄인 결과(결정 0004).
    pub compaction: Option<Compaction>,
    /// 이 실행에서 부른 도구(이름, 입력). 활동 기록용.
    pub tool_calls: Vec<(String, serde_json::Value)>,
}

/// compact 결과(stream-json `compact_boundary`와 요약 메시지).
#[derive(Debug, Clone, PartialEq)]
pub struct Compaction {
    pub pre_tokens: u64,
    pub post_tokens: u64,
    pub summary: Option<String>,
}

/// 보고가 여러 번 왔을 때 하나로 합친다(2026-11-18 사용자 요청: 모델이 실수해도 관대하게 받는다).
///
/// 실측(운영 기록): 같은 답을 2초 간격으로 그대로 두 번, 또는 첫 보고 뒤 같은 답을 줄여 다시 보고했다. 예전에는
/// 실패로 처리해, 먼저 보낸 조각 뒤에 "답을 끝까지 만들지 못했습니다"만 갔다.
/// - 첫 보고가 기준이다. 채팅은 첫 보고의 조각을 먼저 보낸다([`lucia_runner::report_stream::ReportStream`]).
/// - 뒤 보고가 앞의 것과 같으면(공백 무시) 버린다.
/// - 뒤 보고 앞에 다른 도구를 더 불렀으면 새로 알아낸 것으로 보고 이어 붙인다(메시지를 나눠서). 상태는 그 보고의 것.
/// - 다른 도구 없이 다시 쓴 보고는 같은 답을 고쳐 쓴 것으로 보고 버린다(사용자에게 같은 말이 두 번 가지 않게).
/// - 첨부는 모두 모은다(같은 것은 한 번).
pub fn merge_reports(
    reports: Vec<ReportResult>,
    tool_calls: &[(String, serde_json::Value)],
) -> Vec<ReportResult> {
    let mut it = reports.into_iter();
    let Some(mut merged) = it.next() else {
        return Vec::new();
    };
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let is_report = |name: &str| name.ends_with("report_result");
    // 보고를 부른 자리(도구 호출 순서에서).
    let at: Vec<usize> = tool_calls
        .iter()
        .enumerate()
        .filter(|(_, (n, _))| is_report(n))
        .map(|(i, _)| i)
        .collect();
    let mut seen = vec![norm(&merged.summary)];
    for (k, r) in it.enumerate() {
        let k = k + 1;
        for a in &r.attachments {
            if !merged.attachments.contains(a) {
                merged.attachments.push(a.clone());
            }
        }
        let text = norm(&r.summary);
        if text.is_empty() || seen.contains(&text) {
            continue;
        }
        let new_work = match (at.first(), at.get(k)) {
            (Some(&first), Some(&this)) => tool_calls[first + 1..this]
                .iter()
                .any(|(n, _)| !is_report(n)),
            _ => false,
        };
        if !new_work {
            continue;
        }
        merged.summary = format!("{}\n%%\n{}", merged.summary.trim_end(), r.summary.trim());
        merged.details = match (merged.details.take(), r.details) {
            (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
            (a, b) => a.or(b),
        };
        merged.status = r.status;
        seen.push(text);
    }
    vec![merged]
}

impl AgentResponse {
    /// 보고가 정확히 하나면 그 보고. 아니면 계약 위반(명세 §출력 계약). 여러 번 온 보고는 실행기가 이미 합쳤다.
    pub fn single_report(&self) -> Option<&ReportResult> {
        match self.reports.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }
}

pub trait AgentRunner: Send + Sync {
    fn run(&self, request: AgentRequest) -> impl Future<Output = AgentResponse> + Send;

    /// 데몬이 다시 뜰 때: 앞선 데몬이 띄워 놓고 죽은 실행을 끝낸다(결정 0050). 기본은 아무것도 하지 않는다.
    fn kill_orphans(&self) -> impl Future<Output = ()> + Send {
        async {}
    }

    /// 도는 중인 실행을 끝낸다(결정 0051: 사용자가 멈춰 달라고 했다). 끝냈으면 `true`. 그 실행의 `run`은
    /// 오류로 돌아온다. 기본은 끝내지 못한다.
    fn stop(&self, run_id: &str) -> impl Future<Output = bool> + Send {
        let _ = run_id;
        async { false }
    }
}

/// 시험용 실행기: 받은 요청을 기록하고, 정해 둔 응답(없으면 기본 보고)을 돌려준다.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct FakeRunner {
    pub requests: Arc<Mutex<Vec<AgentRequest>>>,
    script: Arc<Mutex<std::collections::VecDeque<AgentResponse>>>,
    /// 실행마다 흘려보낼 이벤트(요청에 받을 곳이 있을 때).
    events: Arc<Mutex<std::collections::VecDeque<Vec<RunEvent>>>>,
    /// 멈춰 달라고 한 실행(시험용: 그 실행은 오류로 돌아온다).
    pub stopped: Arc<Mutex<Vec<String>>>,
    /// 실행마다 기다리는 시간(동시 실행 시험용).
    delay: Option<std::time::Duration>,
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
    /// 동시에 돌던 실행 수의 최댓값.
    pub max_in_flight: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl FakeRunner {
    pub fn with_delay(delay: std::time::Duration) -> Self {
        Self {
            delay: Some(delay),
            ..Self::default()
        }
    }

    pub fn then(&self, response: AgentResponse) -> &Self {
        self.script.lock().unwrap().push_back(response);
        self.events.lock().unwrap().push_back(Vec::new());
        self
    }

    /// 실행 중 이벤트를 흘리고 응답을 돌려준다.
    pub fn then_streaming(&self, events: Vec<RunEvent>, response: AgentResponse) -> &Self {
        self.script.lock().unwrap().push_back(response);
        self.events.lock().unwrap().push_back(events);
        self
    }

    pub fn calls(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    pub fn last_prompt(&self) -> String {
        self.requests
            .lock()
            .unwrap()
            .last()
            .map(|r| r.prompt.clone())
            .unwrap_or_default()
    }

    /// 보고 하나를 돌려주는 응답.
    pub fn reply(summary: &str) -> AgentResponse {
        AgentResponse {
            reports: vec![ReportResult {
                status: ReportStatus::Done,
                summary: summary.to_owned(),
                details: None,
                attachments: Vec::new(),
                blocked: None,
            }],
            usage: RunUsage {
                model: Some("fake".into()),
                input_tokens: 1000,
                output_tokens: 50,
                context_tokens: 1000,
                cost_usd: 0.001,
                ..RunUsage::default()
            },
            error: None,
            session_missing: false,
            compaction: None,
            tool_calls: Vec::new(),
        }
    }

    /// compact 응답(보고 없음, 턴 0).
    pub fn compacted(pre: u64, post: u64, summary: &str) -> AgentResponse {
        AgentResponse {
            reports: vec![],
            usage: RunUsage {
                model: Some("fake".into()),
                cost_usd: 0.0005,
                ..RunUsage::default()
            },
            error: None,
            session_missing: false,
            compaction: Some(Compaction {
                pre_tokens: pre,
                post_tokens: post,
                summary: Some(summary.to_owned()),
            }),
            tool_calls: Vec::new(),
        }
    }
}

#[cfg(test)]
impl AgentRunner for FakeRunner {
    async fn stop(&self, run_id: &str) -> bool {
        self.stopped.lock().unwrap().push(run_id.to_owned());
        true
    }

    async fn run(&self, request: AgentRequest) -> AgentResponse {
        use std::sync::atomic::Ordering;
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        let sink = request.events.clone();
        let run_id = request.run_id.clone();
        self.requests.lock().unwrap().push(request);
        if let Some(delay) = self.delay {
            // 기다리는 동안 멈춰 달라고 하면 바로 끊긴다.
            let step = std::time::Duration::from_millis(10);
            let mut waited = std::time::Duration::ZERO;
            while waited < delay && !self.stopped.lock().unwrap().contains(&run_id) {
                tokio::time::sleep(step).await;
                waited += step;
            }
        }
        if self.stopped.lock().unwrap().contains(&run_id) {
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            let mut killed = Self::reply("");
            killed.reports.clear();
            killed.error = Some("실행 실패: 프로세스가 끝났다".into());
            return killed;
        }
        let events = self.events.lock().unwrap().pop_front().unwrap_or_default();
        if let Some(sink) = sink {
            for e in events {
                let _ = sink.send(e);
            }
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Self::reply("알겠습니다."))
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    use crate::mcp::ReportStatus;

    fn report(summary: &str, status: ReportStatus, files: &[&str]) -> ReportResult {
        ReportResult {
            status,
            summary: summary.into(),
            details: None,
            attachments: files.iter().map(|f| (*f).to_owned()).collect(),
            blocked: None,
        }
    }

    fn call(name: &str) -> (String, serde_json::Value) {
        (format!("mcp__lucia__{name}"), serde_json::Value::Null)
    }

    /// 2026-11-18 운영 실측: 같은 답을 2초 간격으로 그대로 두 번 보고했다. 하나만 보낸다.
    #[test]
    fn identical_reports_collapse_to_one() {
        let a = "네, 좋은 방향이에요.\n%%\n이상한 동작이 있으면 말씀해 주세요.";
        let merged = merge_reports(
            vec![
                report(a, ReportStatus::Done, &[]),
                report(&format!("{a}\n"), ReportStatus::Done, &[]),
            ],
            &[call("report_result"), call("report_result")],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].summary, a);
    }

    /// 2026-11-17 운영 실측: 길게 보고한 뒤 다른 도구 없이 같은 답을 줄여 다시 보고했다. 첫 보고만 보낸다
    /// (채팅은 첫 보고의 조각을 이미 보냈다). 첨부는 모은다.
    #[test]
    fn rewording_without_new_work_keeps_the_first() {
        let merged = merge_reports(
            vec![
                report(
                    "시간표를 못 찾았어요. 확인한 곳은 이래요…",
                    ReportStatus::NeedsInput,
                    &["f1"],
                ),
                report(
                    "시간표를 찾아봤지만 못 찾았어요.",
                    ReportStatus::Done,
                    &["f1", "f2"],
                ),
            ],
            &[
                call("memory_read"),
                call("browser_open"),
                call("report_result"),
                call("report_result"),
            ],
        );
        assert_eq!(merged.len(), 1);
        assert!(merged[0].summary.starts_with("시간표를 못 찾았어요"));
        assert!(!merged[0].summary.contains("찾아봤지만"));
        assert_eq!(merged[0].status, ReportStatus::NeedsInput);
        assert_eq!(merged[0].attachments, ["f1", "f2"]);
    }

    /// 보고 뒤 도구를 더 써서 새로 알아낸 것을 다시 보고하면 이어 붙인다(메시지를 나눠서, 상태는 뒤의 것).
    #[test]
    fn a_later_report_after_more_work_is_appended() {
        let merged = merge_reports(
            vec![
                report("먼저 찾은 것: 과제 두 개.", ReportStatus::Done, &[]),
                report("더 보니 퀴즈도 하나 있어요.", ReportStatus::NeedsInput, &[]),
            ],
            &[
                call("browser_open"),
                call("report_result"),
                call("browser_open"),
                call("report_result"),
            ],
        );
        assert_eq!(
            merged[0].summary,
            "먼저 찾은 것: 과제 두 개.\n%%\n더 보니 퀴즈도 하나 있어요."
        );
        assert_eq!(merged[0].status, ReportStatus::NeedsInput);
        assert!(merge_reports(Vec::new(), &[]).is_empty());
    }
}
