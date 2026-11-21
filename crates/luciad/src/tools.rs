//! 도구 선언과 실행별 허락(결정 0013).
//!
//! 도구마다 갈래([`Group`]), 등급([`Grade`], 명세 §권한 등급), 사람이 지금 대화 중이어야 하는지를 선언한다.
//! 실행 종류는 [`Policy`](열린 갈래, 최대 등급, 사람 있음)로 정하고, 허용 여부는 선언과 정책에서 계산한다.
//! claude 허용 목록([`crate::runner::allowed_tools`])과 MCP 서버의 확인이 같은 계산을 쓴다.
//!
//! 새 기능은 도구를 이 표에 더하고 갈래를 고른다. 나중의 외부 기기 명령도 등급과 사람 필요 여부를 선언해
//! 같은 계산으로 허용한다(기기 명령마다 새 허락 칸을 만들지 않는다).
//!
//! 도구 찾기(결정 0022): 도구마다 대분류([`Category`])를 두고, 자주 쓰는 도구만 모델의 도구 목록에 바로 보인다
//! ([`ToolSpec::featured`]와 사용 기록으로 고른 것). 나머지는 `tool_search`로 찾고(파라미터까지 받는다)
//! `tool_call`로 부른다. 새 도구는 표에 더하고 대분류를 고르면 찾기에 바로 나온다.

pub use lucia_core::Grade;

/// 도구 갈래. 실행 종류가 갈래 단위로 연다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Group {
    /// 결과 보고.
    Report = 1 << 0,
    /// 기억 항목.
    Memory = 1 << 1,
    /// Google 메일·일정 읽기, 메일 발송 요청.
    Google = 1 << 2,
    /// 사용자에게 보낼 파일 만들기.
    Files = 1 << 3,
    /// 후속 조치.
    Followups = 1 << 4,
    /// 전용 브라우저(외부 사이트로 나간다).
    Browser = 1 << 5,
    /// 작업 맡기기·목록·전달·취소(결정 0014). 맡기기·전달·취소는 사람이 대화 중일 때만.
    Tasks = 1 << 6,
    /// 작업 세션 안에서 진행 알리기(결정 0014).
    TaskRun = 1 << 7,
    /// 스킬 읽기(결정 0018).
    Skills = 1 << 8,
    /// 스킬 만들기·고치기, 고정·끄기(사람만).
    SkillWrite = 1 << 9,
    /// 페이지 감시 제안(결정 0019). 사람이 시작한 일에서만.
    Watches = 1 << 10,
    /// 개발 작업의 push 요청(결정 0020, 승인 카드).
    DevPush = 1 << 11,
    /// Discord 서버 조회(결정 0024): 서버·채널·기록·역할·멤버. 다른 사람의 글을 읽는다.
    Discord = 1 << 12,
    /// Discord 글 올리기·서버 관리 요청(결정 0024, 승인 카드).
    DiscordWrite = 1 << 13,
    /// 코드 실행과 스킬 스크립트 실행(결정 0025). 코드가 부르는 도구는 그 실행의 범위로 다시 본다.
    Code = 1 << 14,
    /// 반복 루틴 만들기·고치기·지우기(결정 0025). 사람 없는 실행은 서버가 좁힌다(결정 0028·0032).
    Routines = 1 << 15,
    /// 목표(결정 0032): 목록·만들기(사람만)·진행 기록과 고치기(사람 없는 실행은 맡은 목표만).
    Goals = 1 << 16,
    /// 이미지 생성(결정 0046): 바깥 API에 돈을 쓴다. 사람이 시작한 대화와 맡긴 작업만 연다.
    Images = 1 << 17,
    /// 전용 앱의 기능 부르기(결정 0053): 위치, 휴대폰 알림 등 앱이 내준 것. 사용자가 그 앱에 범위를 주어야 한다.
    App = 1 << 19,
    /// 스크립트 샌드박스에 패키지 설치(결정 0047). 설치한 것은 뒤의 모든 스크립트와 함께 돌므로, 사람이 시작한
    /// 대화와 맡긴 작업만 연다(사람 없는 실행이 뒤의 실행을 넓히지 않게, 결정 0028).
    Packages = 1 << 18,
    /// 되돌릴 수 없는 바깥 동작의 승인 요청(결정 0059). 사람이 대화 중인 실행만 연다(카드를 볼 사람이 있어야 한다).
    Approvals = 1 << 20,
}

impl Group {
    const fn bit(self) -> u32 {
        self as u32
    }
}

/// 도구 대분류(결정 0022). 모델이 도구를 찾을 때 보는 색인이다. 허락 단위인 [`Group`]과 따로 둔다
/// (허락은 더 잘게 나뉘어도 찾을 때는 한 묶음이 낫다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Memory,
    Google,
    Browser,
    Tasks,
    Followups,
    Skills,
    Files,
    Watches,
    Discord,
    Automation,
    Goals,
}

impl Category {
    pub const ALL: [Category; 11] = [
        Self::Memory,
        Self::Google,
        Self::Browser,
        Self::Tasks,
        Self::Followups,
        Self::Skills,
        Self::Files,
        Self::Watches,
        Self::Discord,
        Self::Automation,
        Self::Goals,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Google => "google",
            Self::Browser => "browser",
            Self::Tasks => "tasks",
            Self::Followups => "followups",
            Self::Skills => "skills",
            Self::Files => "files",
            Self::Watches => "watches",
            Self::Discord => "discord",
            Self::Automation => "automation",
            Self::Goals => "goals",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Memory => "기억",
            Self::Google => "메일·일정",
            Self::Browser => "브라우저",
            Self::Tasks => "작업",
            Self::Followups => "후속 조치",
            Self::Skills => "스킬",
            Self::Files => "파일",
            Self::Watches => "페이지 감시",
            Self::Discord => "Discord 서버",
            Self::Automation => "자동화",
            Self::Goals => "목표",
        }
    }

    /// 한 줄 설명(색인에 들어간다).
    pub fn about(self) -> &'static str {
        match self {
            Self::Memory => {
                "사용자에 대한 오래 갈 사실·선호·진행 중인 일 항목을 찾고 읽고 고치고 지운다"
            }
            Self::Google => {
                "Gmail 메일 검색·읽기, 메일 발송 요청(승인 카드), 보내는 주소 확인, Google 캘린더 일정"
            }
            Self::Browser => {
                "로그인 프로필이 있는 전용 브라우저: 페이지 열기·누르기·입력·스크롤·뒤로·화면 찍기, 사이트 로그인 요청, 사람 없이 도는 일이 쓸 사이트 위임 제안"
            }
            Self::Tasks => {
                "오래 걸리는 일을 작업 세션으로 맡기기·목록·전달·취소, 작업 안 진행 알림, 레포 개발 작업과 push 요청"
            }
            Self::Followups => {
                "나중에 할 약속(후속 조치) 등록·목록·취소, 사용자가 지킬 마감 챙기기(다가오면 다시 알림)"
            }
            Self::Skills => "성공한 절차(스킬) 읽기·저장·고정·끄기",
            Self::Files => {
                "전용 앱의 기능 부르기(app_tools로 보고 app_call로: 휴대폰 위치 등 앱이 내준 것), 웹 주소의 파일 받기(file_download: 압축 파일·PDF·그림 등), 파일을 직접 열어 보기(만든 것·받은 것, 그림·글·문서), 파일을 보관함에 넣어 지워지지 않게 하기(file_keep), 사용자에게 보낼 텍스트 파일 만들기, 그림(이미지) 생성·편집, 전용 앱의 이모지·스티커 등록(사용자가 준 그림으로)·목록·지우기"
            }
            Self::Watches => {
                "공개 페이지의 한 구간을 모델 없이 주기적으로 확인하자고 제안(승인 카드)"
            }
            Self::Discord => {
                "봇이 들어간 Discord 서버의 채널·대화 기록·역할·멤버 조회, 다른 채널에 글 올리기와 서버 관리(내보내기·차단·타임아웃·역할·채널·메시지, 승인 카드)"
            }
            Self::Goals => {
                "마감이 있거나 오래 가는 일을 목표로 맡기기, 진행 기록, 하위 후속 조치·루틴·감시 잇기, 완료·포기"
            }
            Self::Automation => {
                "Python 코드로 도구를 불러 많은 자료를 기계적으로 거르고 계산하기·파일(그림 등)을 읽어 고치고 새 파일로 남기기(code_run), 코드가 쓸 Python 패키지 설치·목록(lib_install, lib_list), 반복 루틴(정해진 때·간격·새 메일 같은 변화마다 스크립트를 먼저 돌리고 필요할 때만 알리거나 Lucia를 깨움) 만들기·목록·고치기·지우기"
            }
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim();
        Self::ALL
            .into_iter()
            .find(|c| c.id().eq_ignore_ascii_case(t) || c.label() == t)
    }
}

/// 도구 찾기·부르기 도구의 이름(표에 없다: 모든 실행이 쓴다. 찾은 도구와 부른 도구는 실행 범위로 다시 본다).
pub const SEARCH_TOOL: &str = "tool_search";
pub const CALL_TOOL: &str = "tool_call";

/// 도구 하나의 선언.
#[derive(Debug, Clone, Copy)]
pub struct ToolSpec {
    pub name: &'static str,
    pub group: Group,
    pub grade: Grade,
    /// 사람이 지금 대화 중인 실행에서만(되돌리기 어려운 지우기, 비밀번호 입력 링크 보내기 등).
    pub human_only: bool,
    /// 대분류. `None`이면 늘 바로 보인다(결과 보고).
    pub category: Option<Category>,
    /// 자주 쓰일 것으로 보고 늘 바로 보인다(찾지 않고 부른다).
    pub featured: bool,
}

const fn tool(
    name: &'static str,
    group: Group,
    grade: Grade,
    human_only: bool,
    category: Category,
) -> ToolSpec {
    ToolSpec {
        name,
        group,
        grade,
        human_only,
        category: Some(category),
        featured: false,
    }
}

/// 늘 바로 보이는 도구.
const fn featured(spec: ToolSpec) -> ToolSpec {
    ToolSpec {
        featured: true,
        ..spec
    }
}

use Grade::{ExternalWrite, LocalWrite, Read};
use Group::*;

const MEM: Category = Category::Memory;
const GGL: Category = Category::Google;
const BRW: Category = Category::Browser;
const TSK: Category = Category::Tasks;
const FUP: Category = Category::Followups;
const SKL: Category = Category::Skills;
const FIL: Category = Category::Files;
const WCH: Category = Category::Watches;
const DSC: Category = Category::Discord;
const AUT: Category = Category::Automation;
const GOL: Category = Category::Goals;

/// 데몬 MCP 도구 전체. 외부 쓰기(`ExternalWrite`)는 도구 안에서 Discord 승인을 받는다.
pub const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "report_result",
        group: Report,
        grade: Read,
        human_only: false,
        category: None,
        featured: true,
    },
    tool("memory_search", Memory, Read, false, MEM),
    featured(tool("memory_read", Memory, Read, false, MEM)),
    featured(tool("memory_write", Memory, LocalWrite, false, MEM)),
    tool("memory_delete", Memory, LocalWrite, true, MEM),
    featured(tool("mail_search", Google, Read, false, GGL)),
    tool("mail_read", Google, Read, false, GGL),
    tool("mail_aliases", Google, Read, false, GGL),
    featured(tool("calendar_events", Google, Read, false, GGL)),
    tool("mail_send", Google, ExternalWrite, false, GGL),
    // 캘린더 쓰기(결정 0033): 참석자 없는 내 일정은 로컬 쓰기, 참석자가 있으면 도구 안에서 카드(외부 쓰기까지
    // 열린 실행만). 지우기는 늘 카드라 외부 쓰기로 선언한다.
    tool("calendar_create", Google, LocalWrite, false, GGL),
    tool("calendar_update", Google, LocalWrite, false, GGL),
    tool("calendar_delete", Google, ExternalWrite, false, GGL),
    tool("file_create", Files, LocalWrite, false, FIL),
    // 만든 것과 받은 것을 직접 열어 본다(결정 0048). 늘 바로 보인다: 확인은 기본 동작이어야 한다.
    featured(tool("file_view", Files, Read, false, FIL)),
    // 자기 프로필 이미지 보기·정하기(결정 0060). 정하기는 도구 안에서 사람이 대화 중인지 본다. 늘 바로 보인다.
    featured(tool("profile_image", Files, LocalWrite, false, FIL)),
    // 작업 공간과 파일 id 잇기(결정 0059): 쉘에서 만든 파일을 사용자에게 보내고, 받은 파일을 쉘에서 다룬다.
    featured(tool("file_import", Files, LocalWrite, true, FIL)),
    featured(tool("file_export", Files, LocalWrite, true, FIL)),
    // 승인 요청(결정 0059): 바깥에 되돌릴 수 없는 영향을 주는 일을 하기 전에 사용자에게 묻는다. 늘 바로 보인다.
    featured(tool("approval_request", Approvals, Read, true, FIL)),
    // 웹에서 파일 받기(결정 0057): 바깥으로 나가므로 브라우저 갈래다(사람 없는 실행은 위임한 사이트만).
    // 늘 바로 보인다: 찾기 뒤에 두면 "파일은 못 받는다"고 답하게 된다(결정 0046의 경험).
    featured(tool("file_download", Browser, Read, false, FIL)),
    // 보관함(결정 0052): 지워지지 않게 남긴다.
    tool("file_keep", Files, LocalWrite, false, FIL),
    // 전용 앱의 기능(결정 0053): 앱이 내준 것을 보고 부른다. 무엇을 바꾸는 기능은 도구 안에서 사람이 있는지 본다.
    tool("app_tools", App, Read, false, FIL),
    tool("app_call", App, Read, false, FIL),
    // 앱 이모지·스티커(결정 0045): 이름·설명이 뒤의 요청에 다시 실리므로 등록·지우기는 사람이 대화 중일 때만.
    tool("expression_add", Files, LocalWrite, true, FIL),
    tool("expression_list", Files, Read, false, FIL),
    tool("expression_remove", Files, LocalWrite, true, FIL),
    // 이미지 생성(결정 0046): 프롬프트가 OpenAI로 나가고 돈이 든다. 갈래로 따로 열고, 하루 예산은 도구가 본다.
    // 늘 바로 보인다: 찾기 뒤에 두었더니 실제 모델이 "그림은 못 만든다"며 SVG 파일로 대신했다(2026-11-19).
    featured(tool("image_generate", Images, LocalWrite, false, FIL)),
    featured(tool("image_result", Images, Read, false, FIL)),
    featured(tool("followup_create", Followups, LocalWrite, false, FUP)),
    tool("followup_list", Followups, Read, false, FUP),
    // 줄이는 동작(결정 0028 I2): 사람 없는 실행은 자기가 발사한 것과 같은 실행이 만든 것만(서버가 본다).
    tool("followup_cancel", Followups, LocalWrite, false, FUP),
    // 단계적 독촉(결정 0034): 사용자가 지킬 마감을 챙긴다.
    tool("nudge_create", Followups, LocalWrite, false, FUP),
    tool("nudge_done", Followups, LocalWrite, false, FUP),
    // 브라우저: 읽기도 외부 사이트로 나가므로 갈래로 따로 연다. 누르기·입력은 도구 안에서 등급을 다시 매겨
    // POST 제출 등은 승인을 받는다(결정 0008).
    featured(tool("browser_open", Browser, Read, false, BRW)),
    tool("browser_snapshot", Browser, Read, false, BRW),
    featured(tool("browser_screenshot", Browser, Read, false, BRW)),
    tool("browser_click", Browser, ExternalWrite, false, BRW),
    tool("browser_type", Browser, ExternalWrite, false, BRW),
    tool("browser_scroll", Browser, Read, false, BRW),
    tool("browser_back", Browser, Read, false, BRW),
    // 좌표·초점 동작(결정 0023): 등급은 도구 안에서 그 점(초점)의 요소로 다시 매긴다.
    featured(tool("browser_click_at", Browser, ExternalWrite, false, BRW)),
    tool("browser_type_text", Browser, ExternalWrite, false, BRW),
    tool("browser_key", Browser, ExternalWrite, false, BRW),
    // 페이지에서 스크립트 돌리기와 파일 올리기(결정 0059): 사람이 대화 중일 때만. 승인은 도구 안에서 받는다.
    featured(tool("browser_eval", Browser, ExternalWrite, true, BRW)),
    tool("browser_upload", Browser, ExternalWrite, true, BRW),
    // 로그인 요청은 로컬 쓰기로 둔다: 사람 없는 실행은 사람이 로그인을 위임한 사이트만 서버가 받는다(결정 0028).
    tool("browser_login", Browser, LocalWrite, false, BRW),
    // 사이트 위임 제안(결정 0028): 만들기·넓히기는 사람이 대화 중일 때 카드로만(I1).
    tool("site_grant_propose", Browser, LocalWrite, true, BRW),
    featured(tool("task_start", Tasks, LocalWrite, true, TSK)),
    tool("task_list", Tasks, Read, false, TSK),
    tool("task_message", Tasks, LocalWrite, true, TSK),
    tool("task_cancel", Tasks, LocalWrite, true, TSK),
    featured(tool("task_update", TaskRun, LocalWrite, false, TSK)),
    featured(tool("skill_read", Skills, Read, false, SKL)),
    tool("skill_save", SkillWrite, LocalWrite, false, SKL),
    tool("skill_pin", SkillWrite, LocalWrite, true, SKL),
    tool("skill_disable", SkillWrite, LocalWrite, true, SKL),
    tool("watch_propose", Watches, LocalWrite, false, WCH),
    tool("dev_start", Tasks, LocalWrite, true, TSK),
    tool("dev_push", DevPush, ExternalWrite, false, TSK),
    // Discord 서버(결정 0024): 조회는 읽기, 글 올리기·관리는 승인 카드. 관리는 사람이 대화 중일 때만.
    tool("discord_servers", Discord, Read, false, DSC),
    tool("discord_channels", Discord, Read, false, DSC),
    tool("discord_messages", Discord, Read, false, DSC),
    tool("discord_roles", Discord, Read, false, DSC),
    tool("discord_members", Discord, Read, false, DSC),
    tool("discord_post", DiscordWrite, ExternalWrite, false, DSC),
    tool("discord_manage", DiscordWrite, ExternalWrite, true, DSC),
    // 자동화(결정 0025): 코드가 부르는 도구는 그 실행의 범위에서 사람 없음·자기 호출 갈래를 뺀 것으로 다시 본다.
    featured(tool("code_run", Code, Read, false, AUT)),
    // 패키지 설치(결정 0047): 스크립트 샌드박스(VM) 안에만. 대화와 맡긴 작업에서만, 스크립트 다리로는 못 부른다.
    featured(tool("lib_install", Packages, LocalWrite, false, AUT)),
    tool("lib_list", Code, Read, false, AUT),
    tool("skill_run", Code, Read, false, SKL),
    // 사람 없는 실행은 목표 점검만 자기 목표의 하위 루틴을 만든다(결정 0032, 서버가 본다).
    tool("routine_create", Routines, LocalWrite, false, AUT),
    tool("routine_list", Routines, Read, false, AUT),
    // 사람 없는 실행은 자기를 깨운 루틴을 멈추거나 간격을 늘리기만 한다(결정 0028 I2, 서버가 본다).
    tool("routine_update", Routines, LocalWrite, false, AUT),
    // 사람 없는 실행은 목표 점검이 자기 목표의 하위 루틴만 지운다(결정 0032, 서버가 본다).
    tool("routine_delete", Routines, LocalWrite, false, AUT),
    // 목표(결정 0032).
    tool("goal_list", Goals, Read, false, GOL),
    tool("goal_create", Goals, LocalWrite, true, GOL),
    tool("goal_update", Goals, LocalWrite, false, GOL),
];

/// 사용 기록으로 바로 보이게 올리는 도구 수와 문턱(결정 0022, [제안]): 최근 30일에 5번 넘게 쓴 것 가운데 많이 쓴 5개.
pub const FREQUENT_DAYS: i64 = 30;
pub const FREQUENT_MIN_USES: i64 = 5;
pub const FREQUENT_MAX: usize = 5;

/// 사용 기록(이름, 횟수, 많은 순)에서 바로 보일 도구를 고른다. 표에 있고 대분류가 있는 것만.
pub fn frequent(usage: &[(String, i64)]) -> Vec<&'static str> {
    usage
        .iter()
        .filter(|(_, n)| *n >= FREQUENT_MIN_USES)
        .filter_map(|(name, _)| spec(name))
        .filter(|s| s.category.is_some() && !s.featured)
        .map(|s| s.name)
        .take(FREQUENT_MAX)
        .collect()
}

/// 바로 보이는 도구인가(늘 보이는 것 + 사용 기록으로 올린 것). 표에 없는 도구는 여기서 판단하지 않는다.
pub fn is_direct(name: &str, frequent: &[&str]) -> bool {
    spec(name).is_some_and(|s| s.category.is_none() || s.featured || frequent.contains(&s.name))
}

/// `tool_search` 설명에 붙는 대분류 색인. 실행과 상관없이 같다(프롬프트 캐시가 깨지지 않게).
pub fn category_index() -> String {
    let mut out = String::from("대분류(id, 이름: 설명 [도구]):\n");
    for c in Category::ALL {
        let names: Vec<&str> = TOOLS
            .iter()
            .filter(|t| t.category == Some(c))
            .map(|t| t.name)
            .collect();
        out.push_str(&format!(
            "- {}, {}: {} [{}]\n",
            c.id(),
            c.label(),
            c.about(),
            names.join(", ")
        ));
    }
    out
}

pub fn spec(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|t| t.name == name)
}

/// 실행 하나가 쓸 수 있는 범위.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Policy {
    groups: u32,
    /// 이 등급까지 연다. `None`이면 아무것도 열지 않는다.
    max_grade: Option<Grade>,
    /// 사람이 지금 대화 중이다.
    pub human: bool,
    /// 스크립트가 다리로 부르는 호출(결정 0025): JSON을 줄 수 있는 도구는 글 대신 JSON을 준다.
    pub script: bool,
}

impl Policy {
    /// 아무것도 열지 않는다(토큰만 있는 실행).
    pub const NONE: Policy = Policy {
        groups: 0,
        max_grade: None,
        human: false,
        script: false,
    };

    pub const fn new(groups: &[Group], max_grade: Grade, human: bool) -> Self {
        let mut bits = 0;
        let mut i = 0;
        while i < groups.len() {
            bits |= groups[i].bit();
            i += 1;
        }
        Policy {
            groups: bits,
            max_grade: Some(max_grade),
            human,
            script: false,
        }
    }

    /// 스크립트에 넘길 범위(결정 0025): 이 실행의 범위에서 사람 없음으로 하고, 보고·스스로를 부르는 것·새 자동
    /// 실행을 거는 갈래(코드, 루틴, 작업, 후속 조치, 감시, 스킬 쓰기, push)와 돈을 쓰는 이미지 생성을 뺀다.
    /// 넓어지는 것은 없다.
    pub fn for_script(self) -> Policy {
        let drop = Report.bit()
            | SkillWrite.bit()
            | Code.bit()
            | Routines.bit()
            | Tasks.bit()
            | TaskRun.bit()
            | Followups.bit()
            | Watches.bit()
            | DevPush.bit()
            | Images.bit()
            | Packages.bit()
            | Approvals.bit()
            | App.bit();
        Policy {
            groups: self.groups & !drop,
            max_grade: self.max_grade,
            human: false,
            script: true,
        }
    }

    /// 선언된 도구를 쓸 수 있는가.
    pub fn allows_spec(&self, spec: &ToolSpec) -> bool {
        self.groups & spec.group.bit() != 0 && self.allows_grade(spec.grade, spec.human_only)
    }

    /// 등급과 사람 필요 여부만으로(나중의 기기 명령처럼 표에 없는 동작).
    pub fn allows_grade(&self, grade: Grade, human_only: bool) -> bool {
        grade != Grade::Forbidden
            && self.max_grade.is_some_and(|max| grade <= max)
            && (!human_only || self.human)
    }

    /// 이름으로. 표에 없는 도구는 열지 않는다.
    pub fn allows(&self, name: &str) -> bool {
        spec(name).is_some_and(|s| self.allows_spec(s))
    }

    pub fn has_group(&self, group: Group) -> bool {
        self.groups & group.bit() != 0
    }

    /// 열린 도구 이름(표 순서).
    pub fn tool_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        TOOLS.iter().filter(|t| self.allows_spec(t)).map(|t| t.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_category_and_few_are_direct() {
        for t in TOOLS {
            assert!(
                t.category.is_some() || t.name == "report_result",
                "{} 대분류 없음",
                t.name
            );
        }
        let direct = TOOLS.iter().filter(|t| is_direct(t.name, &[])).count();
        assert!(
            direct * 3 <= TOOLS.len(),
            "바로 보이는 도구는 3분의 1 이하: {direct}"
        );
        let index = category_index();
        for c in Category::ALL {
            assert!(index.contains(c.about()), "{}", c.id());
            assert_eq!(Category::parse(c.id()), Some(c));
            assert_eq!(Category::parse(c.label()), Some(c));
        }
        assert!(index.contains("mail_send"));
    }

    #[test]
    fn usage_promotes_only_catalogued_tools_over_the_threshold() {
        let usage = vec![
            ("browser_snapshot".to_owned(), 7),
            ("report_result".to_owned(), 50),
            ("browser_open".to_owned(), 12),
            ("tool_search".to_owned(), 9),
            ("mail_read".to_owned(), 5),
            ("followup_list".to_owned(), 4),
        ];
        let f = frequent(&usage);
        assert_eq!(f, vec!["browser_snapshot", "mail_read"]);
        assert!(is_direct("browser_snapshot", &f));
        assert!(!is_direct("followup_list", &f));
        assert!(is_direct("report_result", &[]));
        assert!(!is_direct("tool_search", &f), "표 밖은 따로 다룬다");
    }

    #[test]
    fn every_tool_is_declared_once() {
        for (i, t) in TOOLS.iter().enumerate() {
            assert!(
                TOOLS[i + 1..].iter().all(|o| o.name != t.name),
                "{} 두 번",
                t.name
            );
        }
    }

    #[test]
    fn policy_combines_group_grade_and_human() {
        let background = Policy::new(&[Report, Memory, Google], LocalWrite, false);
        assert!(background.allows("memory_write"));
        assert!(
            !background.allows("memory_delete"),
            "사람이 없으면 지우지 않는다"
        );
        assert!(!background.allows("mail_send"), "외부 쓰기는 등급 밖");
        assert!(!background.allows("browser_open"), "갈래가 닫혀 있다");
        assert!(!background.allows("nope"), "표에 없는 도구");

        let chat = Policy::new(&[Report, Memory, Google, Browser], ExternalWrite, true);
        assert!(chat.allows("memory_delete") && chat.allows("browser_login"));
        assert!(
            !chat.allows_grade(Grade::Forbidden, false),
            "금지는 늘 닫힌다"
        );
        assert!(!Policy::NONE.allows("report_result"));
        assert!(!Policy::NONE.allows_grade(Read, false));
    }
}
