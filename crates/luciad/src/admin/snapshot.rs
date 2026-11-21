//! 관리 화면이 그리는 데이터. 저장소와 연결 상태에서 모으고([`super::gather`]), 화면은 이것만 보고 그린다
//! (그래서 상태별 화면을 가짜 데이터로 시험할 수 있다). 숫자 표기와 "확인할 것" 판단도 여기 있다. 시각·달러·바이트
//! 표기는 화면(`web/admin/src/lib/format.ts`)이 맡고, 여기에는 같은 글을 내는지 맞춰 보는 시험용 구현만 남아 있다.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use lucia_store::{
    Activity, Alert, Approval, ApprovalStatus, MemoryEntry, Schedule, SessionRow, SourceState,
    SpendSummary,
};

use crate::check::Check;
use crate::discord_ctl::DiscordState;
use crate::google_auth;

/// 상태의 결. 색과 아이콘이 함께 바뀐다(색만으로 구분하지 않는다).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    Ok,
    Muted,
    Info,
    Warn,
    Bad,
}

/// 처리 결과 안내(한 번 보이고 사라진다).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Flash {
    pub tone: Tone,
    pub text: String,
}

/// 화면. 메뉴에는 묶음으로 나오고(`web/admin/src/lib/nav.ts`의 `NAV`), 설정은 보조 탭 셋이 한 메뉴 칸을 나눠 쓴다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Page {
    Overview,
    Work,
    Automation,
    Activity,
    Memory,
    Skills,
    Character,
    /// 앱 이모지·스티커(결정 0045).
    Expressions,
    Connections,
    Browser,
    /// 설정: 안전과 한도.
    Safety,
    /// 설정: 알림.
    Notify,
    /// 설정: 작업(작업 스레드, 개발 위임, 하루 정리).
    Tasks,
    System,
    /// 초기 설정(결정 0064): 이름·모습, Discord, Google, 앱·푸시, 이미지 생성, 환경.
    Start,
}

impl Page {
    pub const ALL: [Page; 15] = [
        Page::Overview,
        Page::Work,
        Page::Automation,
        Page::Activity,
        Page::Memory,
        Page::Skills,
        Page::Character,
        Page::Expressions,
        Page::Connections,
        Page::Browser,
        Page::Safety,
        Page::Notify,
        Page::Tasks,
        Page::System,
        Page::Start,
    ];

    /// 설정의 보조 탭.
    pub const SETTINGS: [Page; 3] = [Page::Safety, Page::Notify, Page::Tasks];

    pub fn path(self) -> &'static str {
        match self {
            Self::Overview => "/",
            Self::Work => "/work",
            Self::Automation => "/automation",
            Self::Activity => "/activity",
            Self::Memory => "/memory",
            Self::Skills => "/skills",
            Self::Character => "/character",
            Self::Expressions => "/expressions",
            Self::Connections => "/connections",
            Self::Browser => "/browser",
            Self::Safety => "/settings",
            Self::Notify => "/settings/notify",
            Self::Tasks => "/settings/work",
            Self::System => "/system",
            Self::Start => "/start",
        }
    }

    /// 화면 제목(탭 제목과 첫 제목).
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "홈",
            Self::Work => "할 일",
            Self::Automation => "자동화",
            Self::Activity => "활동",
            Self::Memory => "기억",
            Self::Skills => "스킬",
            Self::Character => "캐릭터",
            Self::Expressions => "이모지·스티커",
            Self::Connections => "연결",
            Self::Browser => "브라우저",
            Self::Safety => "설정 · 안전",
            Self::Notify => "설정 · 알림",
            Self::Tasks => "설정 · 작업",
            Self::System => "시스템",
            Self::Start => "초기 설정",
        }
    }

    /// 메뉴에 나오는 이름.
    pub fn nav_label(self) -> &'static str {
        match self {
            Self::Safety | Self::Notify | Self::Tasks => "설정",
            other => other.label(),
        }
    }

    /// 보조 탭에 나오는 이름.
    pub fn tab_label(self) -> &'static str {
        match self {
            Self::Safety => "안전과 한도",
            Self::Notify => "알림",
            Self::Tasks => "작업과 보관",
            other => other.label(),
        }
    }
}

/// 처리 뒤 돌아갈 화면. 정해진 목록만 받는다(받은 값을 그대로 쓰지 않는다).
pub fn back_path(back: Option<&str>) -> &'static str {
    Page::ALL
        .iter()
        .find(|p| Some(p.path()) == back)
        .map_or("/", |p| p.path())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscordView {
    pub state: DiscordState,
    pub token_set: bool,
    pub owner: Option<String>,
    pub notify: Option<String>,
    /// 다른 서버에서 부르기(결정 0024).
    pub guild_calls: bool,
    pub prefix: Option<String>,
    /// Discord가 메시지 내용 intent를 거절해 그것 없이 붙어 있다.
    pub content_refused: bool,
    /// 봇이 들어간 서버(연결됐을 때). 읽지 못했으면 그 까닭.
    pub servers: Option<Result<Vec<lucia_channel::discord::server::Server>, String>>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceView {
    pub label: &'static str,
    pub state: Option<SourceState>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GoogleView {
    pub has_client: bool,
    pub connected: bool,
    pub account: Option<String>,
    /// 동의에서 받은 범위(공백으로 나눈 목록). 모르면 `None`.
    pub scopes: Option<String>,
    /// 진행 중인 동의가 있으면 남은 분.
    pub consent_minutes_left: Option<i64>,
    pub sources: Vec<SourceView>,
}

impl GoogleView {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes
            .as_deref()
            .is_some_and(|g| g.split_whitespace().any(|x| x == scope))
    }

    /// (이름, 있는가). 범위를 모르면 빈 목록.
    pub fn scope_list(&self) -> Vec<(&'static str, bool)> {
        if self.scopes.is_none() {
            return Vec::new();
        }
        [
            (google_auth::SCOPE_GMAIL_READ, "메일 읽기"),
            (google_auth::SCOPE_CALENDAR_READ, "캘린더 읽기"),
            (google_auth::SCOPE_GMAIL_SEND, "메일 발송"),
            (google_auth::SCOPE_CALENDAR_WRITE, "캘린더 쓰기"),
        ]
        .into_iter()
        .map(|(scope, label)| (label, self.has_scope(scope)))
        .collect()
    }
}

/// 프로필(결정 0064): 관리 화면과 앱에 보이는 이름, 정한 그림. 그림의 이름표(`ui`)는 `derive.rs`가 붙인다.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProfileView {
    /// 보이는 이름(정하지 않았으면 기본 이름).
    pub name: String,
    /// 이름을 직접 정했나.
    pub name_set: bool,
    pub avatar: Option<crate::profile::Avatar>,
}

/// 초기 설정(결정 0064)의 입력. 완료 여부와 단계 목록은 `derive.rs`가 이것과 스냅숏의 다른 칸으로 계산해 같은 객체에 더한다.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SetupView {
    /// 홈의 안내를 껐나(`setup.dismissed`).
    pub dismissed: bool,
    /// 읽어 온 설정 파일의 경로(모르면 `None`). 이 화면은 설정 파일을 고치지 않고 보여 주기만 한다.
    pub config_path: Option<String>,
}

pub use crate::worker::LastRun;

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackupView {
    pub enabled: bool,
    pub cron: String,
    pub keep_days: u32,
    pub last: Option<String>,
}

/// JSON으로는 시간대 이름만 내보낸다(예: `Asia/Seoul`).
pub fn tz_name<S: serde::Serializer>(tz: &Tz, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(tz.name())
}

/// `Option<Arc<T>>`는 안의 값이나 `null`을 내보낸다(serde의 `rc` 기능을 켜지 않는다).
pub fn opt_arc_inner<T: serde::Serialize, S: serde::Serializer>(
    v: &Option<std::sync::Arc<T>>,
    s: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(&v.as_deref(), s)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Snapshot {
    pub now: DateTime<Utc>,
    #[serde(serialize_with = "tz_name")]
    pub tz: Tz,
    pub discord: DiscordView,
    pub google: GoogleView,
    pub checks: Vec<Check>,
    pub checks_at: Option<DateTime<Utc>>,
    pub today: SpendSummary,
    pub week: SpendSummary,
    pub schedules: Vec<Schedule>,
    pub session: Option<SessionRow>,
    /// 기억 항목(결정 0007): 나 → 주제 → 영역 순.
    pub memories: Vec<MemoryEntry>,
    /// 관리 페이지 "기억해 줘"의 마지막 결과.
    pub last_remember: Option<LastRun>,
    /// compact 기준(꺼져 있으면 `None`)과 안전 상한.
    pub compact_at_tokens: Option<u64>,
    pub rotate_tokens: u64,
    pub backup: BackupView,
    pub alerts: Vec<Alert>,
    pub approvals: Vec<Approval>,
    pub activity: Vec<Activity>,
    pub approval_ttl_hours: i64,
    pub last_precheck: Option<LastRun>,
    pub last_briefing: Option<LastRun>,
    pub last_backup: Option<LastRun>,
    pub last_compact: Option<LastRun>,
    /// 브라우저(결정 0006). 꺼져 있으면 `None`.
    pub browser: Option<BrowserView>,
    /// 후속 조치(결정 0010): 열린 것(이른 것부터)과 최근에 닫힌 것.
    pub followups_open: Vec<lucia_store::Followup>,
    /// 목표와 그 진행 기록·하위 항목(결정 0032). 열린 것 먼저.
    pub goals: Vec<GoalView>,
    pub followups_closed: Vec<lucia_store::Followup>,
    /// 작업(결정 0014).
    pub tasks_open: Vec<lucia_store::Task>,
    pub tasks_closed: Vec<lucia_store::Task>,
    /// 자동 실행 한도(결정 0010): 하루·주 비용 상한(달러), 방해 금지 시간대.
    pub daily_cap: Option<f64>,
    pub weekly_cap: Option<f64>,
    pub quiet_hours: Option<String>,
    /// 일정 기반 방해 금지(결정 0034): 켰는가, 예외 단어, 지금 읽어 둔 바쁜 구간 수.
    pub quiet_calendar: bool,
    pub quiet_except: String,
    pub quiet_busy: usize,
    /// 알림 규칙(결정 0034).
    pub notice_rules: Vec<lucia_store::NoticeRule>,
    /// 챙기는 마감(결정 0034).
    pub nudges: Vec<lucia_store::Nudge>,
    /// 반복 루틴의 하루 깨우기 상한(결정 0025). 없으면 기본값.
    pub routine_wakes: Option<String>,
    /// 작업 스레드를 열 Discord 채널 ID(결정 0014). 없으면 DM.
    pub task_channel: Option<String>,
    /// 대화 원본 보관 기간(일, 결정 0016). 없으면 지우지 않는다.
    pub keep_days: Option<i64>,
    /// 개발 위임 설정(결정 0020): 비용 상한, 열 주소, 허용한 로컬 경로.
    pub dev_cost_cap: Option<f64>,
    pub dev_allow_hosts: String,
    pub dev_local_roots: String,
    /// 페이지 감시와 그 확인 기록(결정 0019).
    pub watches: Vec<(lucia_store::Watch, Option<lucia_store::SourceState>)>,
    /// 감시 하나가 하루에 바로 보내는 새 글 알림 수(결정 0036).
    pub watch_now_daily: i64,
    /// 권한 모드(결정 0040).
    pub security_mode: crate::security::Mode,
    /// 반복 루틴, 다음 실행, 최근 기록(결정 0025).
    pub routines: Vec<RoutineView>,
    /// 스크립트 샌드박스(결정 0025): 켜져 있으면 그 이름.
    pub scripts_sandbox: Option<String>,
    /// 사이트 위임(결정 0028). 거둔 것·지난 것도 최근 것부터.
    pub site_grants: Vec<lucia_store::SiteGrant>,
    /// 출구 검사가 대화 중에도 막는가(결정 0029, 기본은 기록만).
    pub egress_block: bool,
    /// 이벤트 확인과 신호 처리(결정 0031).
    pub signals: SignalsView,
    /// 접속 API 클라이언트와 지금 연결 여부(결정 0021).
    pub clients: Vec<(lucia_store::Client, bool)>,
    /// 접속 API가 열린 주소. 비면 꺼져 있다.
    pub gateway_binds: Vec<std::net::SocketAddr>,
    /// 외부 노출과 2단계 인증(결정 0043).
    pub access: AccessView,
    /// 파일 보관함(결정 0052).
    pub kept_files: Vec<lucia_store::KeptFile>,
    /// 이미지 생성(결정 0046).
    pub images: ImagesView,
    /// 앱 이모지·스티커 목록(결정 0045). 없으면 이 데몬이 다루지 않는다.
    #[serde(serialize_with = "opt_arc_inner")]
    pub expressions: Option<std::sync::Arc<crate::expressions::Catalog>>,
    /// 앱 연동(결정 0043): 알림 받을 곳, 푸시.
    pub app: AppView,
    /// 프로필: 이름과 그림(결정 0064).
    pub profile: ProfileView,
    /// 초기 설정(결정 0064).
    pub setup: SetupView,
}

/// 이미지 생성(결정 0046).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ImagesView {
    /// OpenAI API 키가 들어 있나(값은 보이지 않는다).
    pub key_set: bool,
    /// 하루 예산(달러).
    pub daily_cap: f64,
    /// 오늘 쓴 돈.
    pub today_usd: f64,
    /// 최근 요청(새것부터).
    pub recent: Vec<lucia_store::GeneratedImage>,
}

/// 앱 연동(결정 0043).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AppView {
    /// 알림·승인 카드를 받을 앱(없으면 Discord DM).
    pub notify_client: Option<String>,
    /// 푸시에 글 미리보기를 싣나.
    pub push_preview: bool,
    /// FCM 자격 증명이 있어 푸시를 보낼 수 있나.
    pub fcm_ready: bool,
    /// FCM 프로젝트 ID.
    pub fcm_project: Option<String>,
}

/// 외부 노출과 관리 페이지 2단계 인증(결정 0043).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AccessView {
    /// 관리 페이지·접속 API의 공개 주소(설정 파일 `[public]`).
    pub admin_url: Option<String>,
    pub gateway_url: Option<String>,
    /// 2단계 인증이 켜져 있나.
    pub totp_enabled: bool,
    /// 이 세션이 설정 중인 비밀과 인증 앱 주소(설정하는 동안만).
    pub totp_pending: Option<(String, String)>,
    /// 이 세션이 밖에서 로그인했나(2단계 인증 바꾸기는 집 안에서만).
    pub session_external: bool,
    /// 외부 로그인이 실패 한도로 막혀 있나.
    pub external_blocked: bool,
}

/// 목표 하나(개요, 결정 0032).
#[derive(Debug, Clone, serde::Serialize)]
pub struct GoalView {
    pub goal: lucia_store::Goal,
    pub log: Vec<lucia_store::GoalLog>,
    pub children: Vec<lucia_store::GoalChild>,
}

/// 이벤트 확인과 신호 처리 설정·최근 기록(결정 0031).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SignalsView {
    pub enabled: bool,
    /// 주간 자기 보고·저녁 미리보기(결정 0035).
    pub weekly: bool,
    pub evening: bool,
    /// 교차 정합성 검사(결정 0033).
    pub consistency: bool,
    pub poll_minutes: u64,
    pub interests: String,
    pub classify_daily: i64,
    pub wake_daily: i64,
    pub classify_used: i64,
    pub wake_used: i64,
    pub recent: Vec<lucia_store::Signal>,
}

/// 루틴 하나(시스템 화면, 결정 0025).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RoutineView {
    pub routine: lucia_store::Routine,
    pub next: Option<chrono::DateTime<chrono::Utc>>,
    pub runs: Vec<lucia_store::RoutineRun>,
}

/// 브라우저 상태(시스템 화면).
#[derive(Debug, Clone, serde::Serialize)]
pub struct BrowserView {
    pub sandbox: String,
    /// 지금 브라우저가 떠 있는가(쓸 때 열고 한동안 안 쓰면 닫는다).
    pub running: bool,
    pub idle_minutes: u64,
    pub sites: Vec<crate::browser::logins::SiteStatus>,
    /// 사용자 차단 목록(결정 0008).
    pub blocked: Vec<String>,
    /// 프록시가 최근에 막은 연결(새것부터).
    pub recent_blocks: Vec<crate::browser::proxy::Decision>,
}

// ---------- 표기 ----------

/// 사람이 읽는 시각: 가까우면 "3분 전", 오늘·어제·내일은 그 말로, 나머지는 날짜.
/// 지금 화면의 구현은 `web/admin/src/lib/format.ts`이고, 이 함수는 `format-cases.json`으로 둘이 같은 글을 내는지
/// 맞춰 보는 시험에서만 쓴다.
#[cfg(test)]
pub fn when(at: DateTime<Utc>, now: DateTime<Utc>, tz: Tz) -> String {
    use chrono::Datelike;
    let local = at.with_timezone(&tz);
    let today = now.with_timezone(&tz).date_naive();
    let delta = now - at;
    let minutes = delta.num_minutes();
    if (0..1).contains(&minutes) && delta.num_seconds() >= 0 {
        return "방금".into();
    }
    if (1..60).contains(&minutes) {
        return format!("{minutes}분 전");
    }
    if (-59..0).contains(&minutes) {
        return format!("{}분 뒤", -minutes);
    }
    let day = local.date_naive();
    let hm = local.format("%H:%M");
    match (day - today).num_days() {
        0 => format!("오늘 {hm}"),
        -1 => format!("어제 {hm}"),
        1 => format!("내일 {hm}"),
        _ if day.year() == today.year() => local.format("%m월 %d일 %H:%M").to_string(),
        _ => local.format("%Y-%m-%d %H:%M").to_string(),
    }
}

/// 마우스를 올리면 보이는 정확한 시각. `format.ts`와 같은 글을 내는지 맞춰 보는 시험에서만 쓴다([`when`] 참고).
#[cfg(test)]
pub fn exact(at: DateTime<Utc>, tz: Tz) -> String {
    at.with_timezone(&tz)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// 천 단위 쉼표.
pub fn num(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 달러 표기. `format.ts`와 같은 글을 내는지 맞춰 보는 시험에서만 쓴다([`when`] 참고).
#[cfg(test)]
pub fn usd(v: f64) -> String {
    if v < 0.01 && v > 0.0 {
        format!("${v:.4}")
    } else {
        format!("${v:.2}")
    }
}

/// 바이트 표기. `format.ts`와 같은 글을 내는지 맞춰 보는 시험에서만 쓴다([`when`] 참고).
#[cfg(test)]
pub fn bytes(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{:.1} KB", n as f64 / 1024.0)
    }
}

/// 루틴 이름.
pub fn routine_label(id: &str) -> String {
    match id {
        "morning-briefing" => "아침 브리핑".into(),
        "hourly-precheck" => "사전 체크(설정 파일)".into(),
        crate::config::BACKUP_ROUTINE_ID => "세션 기록 백업".into(),
        crate::config::PERIODIC_CHECK_ROUTINE_ID => "주기 점검".into(),
        crate::config::WRAPUP_ROUTINE_ID => "하루 정리".into(),
        crate::consistency::ROUTINE_ID => "메일·일정 맞춰 보기".into(),
        crate::reports::WEEKLY_ID => "주간 자기 보고".into(),
        crate::reports::EVENING_ID => "저녁 미리보기".into(),
        other if other.starts_with("goal-review-") => "목표 점검".into(),
        other if other.starts_with("followup-") => "후속 조치".into(),
        other => other.into(),
    }
}

/// 흔한 cron 식을 말로. 모르는 꼴이면 `None`.
pub fn cron_text(spec: &str) -> Option<String> {
    let parts: Vec<&str> = spec.split_whitespace().collect();
    let [m, h, dom, mon, dow] = parts.as_slice() else {
        return None;
    };
    if (*dom, *mon, *dow) != ("*", "*", "*") {
        return None;
    }
    let minute: u32 = m.parse().ok()?;
    if *h == "*" {
        return Some(if minute == 0 {
            "매시 정각".into()
        } else {
            format!("매시 {minute}분")
        });
    }
    let hour: u32 = h.parse().ok()?;
    Some(format!("매일 {hour:02}:{minute:02}"))
}

pub fn activity_label(kind: &str) -> &'static str {
    match kind {
        "mail_search" => "메일 검색",
        "mail_read" => "메일 읽기",
        "mail_aliases" => "보내는 주소 조회",
        "calendar_read" => "일정 조회",
        "approval_requested" => "발송 요청",
        "approval_denied" => "발송 거절",
        "approval_expired" => "발송 만료",
        "approval_failed" => "요청 닫힘",
        "mail_sent" => "메일 보냄",
        "send_failed" => "발송 실패",
        "memory_write" => "기억 변경",
        "memory_delete" => "기억 지움",
        "memory_restore" => "기억 되돌림",
        "site_granted" => "사이트 위임",
        "signal" => "신호 분류",
        "goal_create" => "목표 만듦",
        "notice_rule" => "알림 규칙",
        "blocked" => "못 한 일",
        "suggestion_taken" => "제안 진행",
        "suggestion_failed" => "제안 실패",
        "nudge_create" => "마감 챙기기",
        "nudge_done" => "마감 처리함",
        "nudge_expired" => "마감 지남",
        "calendar_write" => "일정 바꿈",
        "calendar_approved" => "일정 승인",
        "calendar_failed" => "일정 실패",
        "goal_update" => "목표 진행",
        "goal_closed" => "목표 닫음",
        "goal_done" => "목표 완료",
        "site_revoked" => "사이트 위임 거둠",
        "site_grant_propose" => "사이트 위임 제안",
        "egress_blocked" => "위임 밖 사이트",
        "egress_flag" => "출구 검사",
        "browser_delegated" => "위임으로 한 동작",
        "login_delegated" => "위임으로 한 로그인",
        "memory_tell" => "기억해 줘",
        "character_edit" => "캐릭터 고침",
        "followup_create" => "후속 조치 등록",
        "followup_cancel" => "후속 조치 취소",
        "followup_done" => "후속 조치 함",
        "promise_check" => "약속 되묻기",
        "write_held" => "쓰기 보류",
        "write_applied" => "보류 처리",
        "write_failed" => "보류 처리 실패",
        "web_search" => "웹 검색",
        "file_create" => "파일 만들기",
        "file_sent" => "파일 보냄",
        "file_download" => "파일 받기",
        "action_requested" => "바깥 동작 승인 요청",
        "action_approved" => "바깥 동작 승인",
        "action_denied" => "바깥 동작 거절",
        "shell_net" => "쉘의 인터넷",
        "profile_image" => "프로필 그림",
        "profile_name" => "프로필 이름",
        "browser_open" => "페이지 열기",
        "browser_act" => "브라우저 동작",
        "browser_approval" => "브라우저 승인 요청",
        "browser_refused" => "브라우저 동작 거부",
        "browser_failed" => "브라우저 동작 실패",
        "login_requested" => "로그인 요청",
        "login_ok" => "로그인",
        "login_failed" => "로그인 실패",
        "login_input" => "로그인 입력",
        "followup_browser" => "후속 조치 브라우저 허용",
        "task_start" => "작업 맡김",
        "task_report" => "작업 보고",
        "task_done" => "작업 완료",
        "task_cancel" => "작업 취소",
        "task_failed" => "작업 처리 실패",
        "episodes_pruned" => "대화 원본 정리",
        _ => "기타",
    }
}

pub fn activity_tone(kind: &str) -> Tone {
    match kind {
        "mail_sent" | "login_ok" => Tone::Ok,
        "send_failed" | "approval_failed" | "login_failed" => Tone::Bad,
        "approval_requested" | "login_requested" => Tone::Info,
        "memory_write" | "memory_delete" | "memory_restore" => Tone::Warn,
        "egress_blocked" | "egress_flag" => Tone::Warn,
        _ => Tone::Muted,
    }
}

/// 활동 기록 거르개.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ActivityFilter {
    All,
    Mail,
    Calendar,
    Web,
    Approval,
    Files,
    Memory,
}

impl ActivityFilter {
    pub const ALL: [ActivityFilter; 7] = [
        Self::All,
        Self::Mail,
        Self::Calendar,
        Self::Web,
        Self::Approval,
        Self::Files,
        Self::Memory,
    ];

    pub fn parse(s: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|f| Some(f.key()) == s)
            .unwrap_or(Self::All)
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Mail => "mail",
            Self::Calendar => "calendar",
            Self::Web => "web",
            Self::Approval => "approval",
            Self::Files => "files",
            Self::Memory => "memory",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "전체",
            Self::Mail => "메일",
            Self::Calendar => "일정",
            Self::Web => "웹",
            Self::Approval => "발송·승인",
            Self::Files => "파일",
            Self::Memory => "기억",
        }
    }

    pub fn matches(self, kind: &str) -> bool {
        match self {
            Self::All => true,
            Self::Mail => kind.starts_with("mail_") && kind != "mail_sent",
            Self::Calendar => kind.starts_with("calendar"),
            Self::Web => {
                kind == "web_search"
                    || kind == "shell_net"
                    || kind.starts_with("browser_")
                    || kind.starts_with("login_")
            }
            Self::Files => kind.starts_with("file_"),
            Self::Approval => {
                kind.starts_with("approval_")
                    || kind.starts_with("action_")
                    || kind == "mail_sent"
                    || kind == "send_failed"
            }
            Self::Memory => kind.starts_with("memory_"),
        }
    }
}

pub fn approval_status(status: ApprovalStatus) -> (&'static str, Tone) {
    match status {
        ApprovalStatus::Pending => ("승인 대기", Tone::Info),
        ApprovalStatus::Approved => ("보내는 중", Tone::Info),
        ApprovalStatus::Sent => ("보냄", Tone::Ok),
        ApprovalStatus::Denied => ("거절", Tone::Muted),
        ApprovalStatus::Expired => ("만료", Tone::Muted),
        ApprovalStatus::Failed => ("실패", Tone::Bad),
    }
}

pub fn discord_status(state: &DiscordState) -> (String, Tone) {
    match state {
        DiscordState::NotConfigured => ("토큰 없음".into(), Tone::Muted),
        DiscordState::Connecting { .. } => ("연결 중".into(), Tone::Info),
        DiscordState::Connected { .. } => ("연결됨".into(), Tone::Ok),
        DiscordState::Stopped { .. } => ("멈춤".into(), Tone::Bad),
    }
}

pub fn discord_bot(state: &DiscordState) -> Option<&str> {
    match state {
        DiscordState::Connecting { bot } | DiscordState::Connected { bot } => Some(bot),
        _ => None,
    }
}

// ---------- 확인할 것 ----------

/// 개요의 "확인할 것" 한 줄.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Notice {
    pub tone: Tone,
    pub text: String,
    /// 해결할 화면.
    pub page: Page,
}

/// 사람이 손봐야 할 것들(심한 것부터).
pub fn notices(s: &Snapshot) -> Vec<Notice> {
    let mut out = Vec::new();
    let mut add = |tone, text: String, page| out.push(Notice { tone, text, page });
    match &s.discord.state {
        DiscordState::NotConfigured => add(
            Tone::Bad,
            "Discord 봇 토큰이 없어 메시지를 주고받을 수 없습니다.".into(),
            Page::Connections,
        ),
        DiscordState::Stopped { reason } => add(
            Tone::Bad,
            format!("Discord 연결이 멈췄습니다: {reason}"),
            Page::Connections,
        ),
        _ => {}
    }
    if s.discord.token_set && s.discord.owner.is_none() {
        add(
            Tone::Warn,
            "소유자가 없어 아무 메시지도 받지 않습니다.".into(),
            Page::Connections,
        );
    }
    if !s.google.connected {
        add(
            Tone::Warn,
            "Google이 연결되지 않아 메일·일정을 볼 수 없습니다.".into(),
            Page::Connections,
        );
    } else if s.google.scopes.is_none() || !s.google.has_scope(google_auth::SCOPE_GMAIL_SEND) {
        add(
            Tone::Warn,
            "Google 메일 발송 권한이 없습니다. 다시 연결하면 더할 수 있습니다.".into(),
            Page::Connections,
        );
    } else if !s.google.has_scope(google_auth::SCOPE_CALENDAR_WRITE) {
        add(
            Tone::Warn,
            "Google 캘린더 쓰기 권한이 없습니다(결정 0033). 연결 화면에서 다시 연결하면 Lucia가 메일 속 마감을 캘린더에 넣을 수 있습니다.".into(),
            Page::Connections,
        );
    }
    for src in &s.google.sources {
        if let Some(err) = src.state.as_ref().and_then(|st| st.last_error.as_ref()) {
            add(
                Tone::Bad,
                format!("{} 사전 체크 오류: {err}", src.label),
                Page::Connections,
            );
        }
    }
    let failed: Vec<&str> = s.checks.iter().filter(|c| !c.ok).map(|c| c.name).collect();
    if !failed.is_empty() {
        add(
            Tone::Bad,
            format!("점검 실패: {}", failed.join(", ")),
            Page::System,
        );
    }
    let pending = s
        .approvals
        .iter()
        .filter(|a| a.status == ApprovalStatus::Pending)
        .count();
    if pending > 0 {
        add(
            Tone::Info,
            format!("메일 발송 승인 대기 {pending}건이 있습니다. Discord 카드에서 답해 주세요."),
            Page::Activity,
        );
    }
    let undelivered = s
        .alerts
        .iter()
        .filter(|a| !a.delivered && s.now - a.at < chrono::Duration::hours(24))
        .count();
    if undelivered > 0 {
        add(
            Tone::Warn,
            format!("지난 24시간 동안 Discord로 보내지 못한 알림 {undelivered}건"),
            Page::Activity,
        );
    }
    if let Some(run) = &s.last_backup
        && !run.ok
    {
        add(
            Tone::Warn,
            format!("마지막 세션 기록 백업이 실패했습니다: {}", run.text),
            Page::System,
        );
    }
    out.sort_by_key(|n| std::cmp::Reverse(n.tone));
    out
}

/// 머리글의 전체 상태.
pub fn overall(notices: &[Notice]) -> (Tone, &'static str) {
    match notices.iter().map(|n| n.tone).max() {
        Some(Tone::Bad) => (Tone::Bad, "확인 필요"),
        Some(Tone::Warn) => (Tone::Warn, "주의"),
        _ => (Tone::Ok, "정상"),
    }
}

/// JSON 화면이 내보내는 값이 모두 `serde::Serialize`인지 컴파일로 확인한다.
#[allow(dead_code)]
fn _serialize_bounds() {
    fn ok<T: serde::Serialize>() {}
    ok::<Snapshot>();
    ok::<lucia_store::MemoryRevision>();
    ok::<lucia_store::Skill>();
    ok::<lucia_store::SkillRevision>();
    ok::<lucia_store::CharacterRevision>();
    ok::<crate::browser::Tab>();
    ok::<crate::segments::Pace>();
    ok::<crate::models::MainModel>();
    ok::<ActivityFilter>();
    ok::<Notice>();
    ok::<Page>();
    ok::<Tone>();
    ok::<Flash>();
    ok::<crate::browser::sites::SiteProfile>();
    ok::<crate::browser::logins::FormStatus>();
    ok::<crate::browser::logins::FormOutcome>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn kst(d: u32, h: u32, m: u32) -> DateTime<Utc> {
        chrono_tz::Asia::Seoul
            .with_ymd_and_hms(2026, 11, d, h, m, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn times_read_naturally() {
        let tz = chrono_tz::Asia::Seoul;
        let now = kst(16, 20, 0);
        assert_eq!(when(kst(16, 20, 0), now, tz), "방금");
        assert_eq!(when(kst(16, 19, 57), now, tz), "3분 전");
        assert_eq!(when(kst(16, 20, 30), now, tz), "30분 뒤");
        assert_eq!(when(kst(16, 7, 30), now, tz), "오늘 07:30");
        assert_eq!(when(kst(15, 23, 0), now, tz), "어제 23:00");
        assert_eq!(when(kst(17, 7, 30), now, tz), "내일 07:30");
        assert_eq!(when(kst(10, 9, 5), now, tz), "11월 10일 09:05");
    }

    #[test]
    fn numbers_and_crons() {
        assert_eq!(num(0), "0");
        assert_eq!(num(1234), "1,234");
        assert_eq!(num(1_234_567), "1,234,567");
        assert_eq!(usd(0.192), "$0.19");
        assert_eq!(usd(0.0064), "$0.0064");
        assert_eq!(cron_text("30 7 * * *").as_deref(), Some("매일 07:30"));
        assert_eq!(cron_text("0 * * * *").as_deref(), Some("매시 정각"));
        assert_eq!(cron_text("15 * * * *").as_deref(), Some("매시 15분"));
        assert_eq!(cron_text("0 9 * * 1"), None);
    }

    #[test]
    fn back_targets_are_whitelisted() {
        assert_eq!(back_path(Some("/system")), "/system");
        assert_eq!(back_path(Some("/connections")), "/connections");
        assert_eq!(back_path(Some("https://evil.example/")), "/");
        assert_eq!(back_path(Some("//evil.example")), "/");
        assert_eq!(back_path(None), "/");
    }

    #[test]
    fn activity_filters_group_kinds() {
        assert!(ActivityFilter::Mail.matches("mail_read"));
        assert!(!ActivityFilter::Mail.matches("mail_sent"));
        assert!(ActivityFilter::Approval.matches("mail_sent"));
        assert!(ActivityFilter::Approval.matches("approval_expired"));
        assert!(ActivityFilter::Memory.matches("memory_write"));
        assert!(ActivityFilter::Web.matches("web_search"));
        assert!(
            ActivityFilter::Files.matches("file_sent")
                && ActivityFilter::Files.matches("file_create")
        );
        assert_eq!(activity_label("web_search"), "웹 검색");
        assert_eq!(
            ActivityFilter::parse(Some("calendar")),
            ActivityFilter::Calendar
        );
        assert_eq!(ActivityFilter::parse(Some("<x>")), ActivityFilter::All);
    }
}
