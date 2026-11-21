//! 설정 파일(TOML). 비밀은 여기에 두지 않는다(자격 증명 파일은 [`crate::credentials`]).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::Deserialize;

use crate::scheduler::Routine;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_timezone")]
    pub timezone: String,
    /// SQLite 등 데이터 위치.
    pub data_dir: PathBuf,
    /// 암호화된 자격 증명 파일 위치(`<이름>.cred`).
    pub credentials_dir: PathBuf,
    pub sandbox: SandboxConfig,
    #[serde(default)]
    pub mcp: McpConfig,
    #[serde(default)]
    pub models: ModelConfig,
    /// 예전 핵심 기억 설정. 결정 0007 뒤로는 받기만 한다(기존 설정 파일 호환).
    #[serde(default)]
    #[allow(dead_code)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub routines: Vec<RoutineConfig>,
    #[serde(default)]
    pub admin: AdminConfig,
    #[serde(default)]
    pub backup: BackupConfig,
    #[serde(default)]
    pub browser: BrowserConfig,
    #[serde(default)]
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub scripts: ScriptsConfig,
    #[serde(default)]
    pub public: PublicConfig,
    /// 읽어 온 설정 파일의 경로(`load`가 채운다. 관리 화면의 초기 설정이 보여 준다). 파일에 적는 값이 아니다.
    #[serde(skip)]
    pub source: Option<PathBuf>,
}

/// 외부 노출(결정 0043): 리버스 프록시(TLS를 맡는다) 뒤의 공개 주소. 비우면 예전처럼 집 LAN·VPN에서만 열린다.
/// 프록시는 `admin.bind`·`gateway.bind`(LAN·루프백 주소)로 붙고, 데몬은 `trusted_proxies`에서 온 요청만 외부로 본다.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicConfig {
    /// 관리 페이지의 공개 주소(`https://<호스트>`). 외부 로그인에는 2단계 인증(TOTP)이 필요하다.
    #[serde(default)]
    pub admin_url: Option<String>,
    /// 접속 API의 공개 주소(`https://<호스트>`, 앱은 `wss://<호스트>/v1/ws`).
    #[serde(default)]
    pub gateway_url: Option<String>,
    /// 리버스 프록시의 주소(IP나 CIDR, 이 호스트의 루프백·사설 주소). 여기서 온 요청만 `X-Forwarded-For`를 믿는다.
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    /// 믿는 프록시가 실제 접속 주소를 싣는 머리(예: `"X-Real-Ip"`). 앞에 CDN(Cloudflare)이 있어 `X-Forwarded-For`
    /// 오른쪽에 CDN 주소가 붙을 때 쓴다. 비우면 `X-Forwarded-For`를 오른쪽부터 읽는다.
    #[serde(default)]
    pub real_ip_header: Option<String>,
}

impl PublicConfig {
    pub fn proxies(&self) -> crate::public::Proxies {
        crate::public::Proxies::parse(&self.trusted_proxies)
            .unwrap_or_default()
            .with_real_ip_header(self.real_ip_header.as_deref())
    }

    /// 관리 페이지의 공개 Host(`host[:port]`).
    pub fn admin_host(&self) -> Option<String> {
        self.admin_url
            .as_deref()
            .and_then(|u| crate::public::public_host(u).ok())
    }

    /// 접속 API를 외부에 여는가.
    pub fn gateway_open(&self) -> bool {
        self.gateway_url
            .as_deref()
            .is_some_and(|u| crate::public::public_host(u).is_ok())
    }
}

/// 스크립트(결정 0025): Lucia가 쓴 Python 코드를 돌리는 전용 샌드박스(`deploy/create-script-sandbox.sh`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptsConfig {
    /// 끄면 code_run·skill_run이 "꺼져 있다"고 답하고, 스크립트 루틴은 돌지 않는다(스크립트 없는 루틴은 돈다).
    #[serde(default)]
    pub enabled: bool,
    /// 스크립트 샌드박스 이름. 상주·브라우저 샌드박스와 달라야 한다.
    #[serde(default = "default_scripts_sandbox")]
    pub sandbox: String,
    /// 대화 중 실행(code_run, skill_run)의 상한(초). MCP 요청이 60초에 끊기므로 그보다 짧게.
    #[serde(default = "default_scripts_chat_timeout")]
    pub chat_timeout_secs: u64,
    /// 루틴 스크립트의 상한(초).
    #[serde(default = "default_scripts_routine_timeout")]
    pub routine_timeout_secs: u64,
    /// 사람이 대화 중인 실행의 코드가 바깥 인터넷에 직접 나가게 한다(결정 0058). 데몬의 출구 프록시를 거친다.
    /// 끄면 코드는 `lucia.fetch`·`lucia.download`로만 나간다.
    #[serde(default = "default_true")]
    pub net: bool,
    /// 스크립트 출구 프록시가 듣는 주소(loopback만). 스크립트 샌드박스는 이 포트 하나만 허용한다.
    #[serde(default = "default_scripts_proxy_bind")]
    pub proxy_bind: SocketAddr,
}

impl Default for ScriptsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sandbox: default_scripts_sandbox(),
            chat_timeout_secs: default_scripts_chat_timeout(),
            routine_timeout_secs: default_scripts_routine_timeout(),
            net: true,
            proxy_bind: default_scripts_proxy_bind(),
        }
    }
}

fn default_scripts_proxy_bind() -> SocketAddr {
    "127.0.0.1:47812"
        .parse()
        .expect("기본 스크립트 프록시 주소")
}

fn default_scripts_sandbox() -> String {
    "lucia-script".into()
}

fn default_scripts_chat_timeout() -> u64 {
    45
}

fn default_scripts_routine_timeout() -> u64 {
    120
}

/// 접속 API(결정 0021): 자체 앱이 붙는 WebSocket. 비우면 열지 않는다. 집 LAN·VPN 주소에만 바인드한다
/// (그 밖의 주소에서 온 연결은 바인드와 상관없이 거절한다). 관리 페이지·MCP와 다른 포트.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfig {
    #[serde(default)]
    pub bind: Vec<SocketAddr>,
}

/// 브라우저(결정 0006): 로그인 프로필을 유지하는 전용 샌드박스(`deploy/create-browser-sandbox.sh`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserConfig {
    /// 끄면 브라우저 도구가 "꺼져 있다"고 답한다(샌드박스를 만들기 전에는 끈다).
    #[serde(default)]
    pub enabled: bool,
    /// 브라우저 샌드박스 이름. 상주 샌드박스(`sandbox.name`)와 달라야 한다.
    #[serde(default = "default_browser_sandbox")]
    pub sandbox: String,
    /// 이만큼 쓰지 않으면 브라우저를 닫는다(샌드박스는 스스로 멈춰 메모리를 돌려준다).
    #[serde(default = "default_browser_idle")]
    pub idle_minutes: u64,
    /// 쓸 내장 로그인 프로필 ID(로그인 칸을 미리 정해 둔 사이트, 예: `"quasarzone"`). 기본은 없다. 열 수 있는 곳은
    /// 차단 목록 방식이라 이것과 상관없다(결정 0008).
    #[serde(default = "default_browser_sites")]
    pub sites: Vec<String>,
    /// 브라우저 출구 프록시(SOCKS5) 주소. 루프백만. 브라우저 샌드박스는 이 포트 하나만 허용한다
    /// (`deploy/create-browser-sandbox.sh`의 `LUCIA_BROWSER_PROXY_PORT`와 맞춘다).
    #[serde(default = "default_browser_proxy_bind")]
    pub proxy_bind: SocketAddr,
    /// 로그인 입력 링크의 앞부분. 비우면 관리 페이지의 첫 사설 주소(`http://<주소>:<포트>`).
    #[serde(default)]
    pub form_base_url: Option<String>,
    /// 입력 링크 유효 시간(분).
    #[serde(default = "default_login_input_minutes")]
    pub login_input_minutes: i64,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sandbox: default_browser_sandbox(),
            idle_minutes: default_browser_idle(),
            sites: default_browser_sites(),
            proxy_bind: default_browser_proxy_bind(),
            form_base_url: None,
            login_input_minutes: default_login_input_minutes(),
        }
    }
}

fn default_browser_sandbox() -> String {
    "lucia-browser".into()
}
fn default_browser_idle() -> u64 {
    10
}
fn default_browser_sites() -> Vec<String> {
    // 기본은 없다: 내장 프로필(`browser::sites`)은 쓰는 사람이 설정에 적어 켠다. 다른 https 사이트는 프로필 없이도
    // 로그인 칸을 찾아 로그인한다(결정 0008).
    Vec::new()
}
fn default_login_input_minutes() -> i64 {
    10
}
fn default_browser_proxy_bind() -> SocketAddr {
    "127.0.0.1:47811"
        .parse()
        .expect("기본 브라우저 프록시 주소")
}

impl Config {
    /// 로그인 입력 링크의 앞부분: 설정값, 관리 페이지의 공개 주소(결정 0043), 관리 페이지의 첫 사설 주소,
    /// 그것도 없으면 첫 주소.
    pub fn login_form_base(&self) -> String {
        if let Some(url) = &self.browser.form_base_url {
            return url.trim_end_matches('/').to_owned();
        }
        if let Some(url) = &self.public.admin_url {
            return crate::public::base(url);
        }
        let addr = self
            .admin
            .bind
            .iter()
            .find(|a| !a.ip().is_loopback())
            .or_else(|| self.admin.bind.first());
        match addr {
            Some(a) => format!("http://{a}"),
            None => "http://127.0.0.1:8780".into(),
        }
    }
}

/// 상주 세션 기록의 VM 밖 백업(명세 §샌드박스 수명, PLAN §8). 하루 한 번 `sbx cp`로 데이터 디렉터리에 복사한다.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_backup_cron")]
    pub cron: String,
    /// 이 날수보다 오래된 백업은 지운다.
    #[serde(default = "default_keep_days")]
    pub keep_days: u32,
    /// VM 안 Claude Code 세션 기록 위치(PoC에서 확인: `~/.claude/projects/<작업 공간>/<세션>.jsonl`).
    #[serde(default = "default_vm_projects")]
    pub vm_path: String,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cron: default_backup_cron(),
            keep_days: default_keep_days(),
            vm_path: default_vm_projects(),
        }
    }
}

/// 내장 루틴 ID(설정에 같은 ID가 있으면 그것을 쓴다).
pub const BACKUP_ROUTINE_ID: &str = "session-backup";
/// 하루 정리(결정 0016). 매일 20:30. 하루 기억 변경 확인(21:00)은 결정 0027에서 없앴다(설정에서 사라진
/// 내장 루틴의 스케줄은 `sync_routines`가 끈다).
pub const WRAPUP_ROUTINE_ID: &str = "daily-wrapup";
/// 주기 점검(결정 0015). 낮 동안 3시간마다, 훑을 것이 없으면 모델도 메시지도 없다.
pub const PERIODIC_CHECK_ROUTINE_ID: &str = "periodic-check";

fn default_backup_cron() -> String {
    "0 4 * * *".into()
}

fn default_keep_days() -> u32 {
    14
}

fn default_vm_projects() -> String {
    "/home/agent/.claude/projects".into()
}

/// 관리 페이지(D8). 집 LAN과 localhost에만 바인드한다. MCP와 다른 포트여야 한다(샌드박스는 MCP 포트에만 닿는다).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminConfig {
    #[serde(default = "default_admin_bind")]
    pub bind: Vec<SocketAddr>,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            bind: default_admin_bind(),
        }
    }
}

fn default_admin_bind() -> Vec<SocketAddr> {
    vec!["127.0.0.1:8780".parse().expect("기본 관리 주소")]
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxConfig {
    /// 상주 샌드박스 이름(`deploy/create-sandbox.sh`로 만든다).
    pub name: String,
    #[serde(default = "default_sbx")]
    pub sbx: PathBuf,
    /// 작업 공간. 호스트와 VM 안에서 같은 경로다.
    pub workdir: PathBuf,
    /// VM 안의 읽기 전용 plugin 경로.
    pub plugin_dir: Option<String>,
    /// VM 안의 읽기 전용 캐릭터 파일(`--append-system-prompt-file`).
    pub character_file: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// 작업 세션 실행 한 번의 시간 제한(결정 0014).
    #[serde(default = "default_task_timeout")]
    pub task_timeout_secs: u64,
    /// 사람이 대화 중인 실행 한 번의 시간 제한(결정 0059). 쉘로 하는 긴 일을 끊지 않게 길게 둔다. 멈추려면
    /// 대화에서 "그만"을 보낸다. 브리핑·루틴 같은 정기 실행은 `timeout_secs` 그대로다.
    #[serde(default = "default_chat_timeout")]
    pub chat_timeout_secs: u64,
    /// 사람이 대화 중인 실행에 Claude Code 내장 도구(Bash, Read, Write, Edit, Glob, Grep)를 연다(결정 0059).
    /// 쉘의 인터넷은 `[scripts] net`의 출구 프록시를 같이 쓴다(통행증).
    #[serde(default = "default_true")]
    pub native_tools: bool,
    /// 내장 웹 검색(`WebSearch`)을 열지(결정 0003). 검색은 Anthropic 서버가 한다.
    #[serde(default = "default_true")]
    pub web_search: bool,
    /// 외부 쓰기 승인 요청이 만료되기까지의 시간(무응답은 거부).
    #[serde(default = "default_approval_hours")]
    pub approval_ttl_hours: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpConfig {
    /// 데몬 MCP 서버 주소. 샌드박스 정책에서 이 포트를 허용해야 한다.
    #[serde(default = "default_mcp_bind")]
    pub bind: SocketAddr,
    /// VM 안에서 호스트를 부르는 이름.
    #[serde(default = "default_host_alias")]
    pub host_alias: String,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            bind: default_mcp_bind(),
            host_alias: default_host_alias(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    #[serde(default = "default_model")]
    pub chat: String,
    #[serde(default = "default_model")]
    pub briefing: String,
    /// 회고(결정 0018, 명세 "회고 단계(저렴한 모델)").
    #[serde(default = "default_retro_model")]
    pub retro: String,
    /// 신호 2단계 분류(결정 0031): 들어온 메일·일정 변화를 "지금/소식/버림"으로 나누는 작은 모델.
    #[serde(default = "default_retro_model")]
    pub classify: String,
}

fn default_retro_model() -> String {
    "haiku".into()
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            chat: default_model(),
            briefing: default_model(),
            retro: default_retro_model(),
            classify: default_retro_model(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryConfig {
    /// 예전 한 덩어리 핵심 기억의 상한. 결정 0007(기억 항목) 뒤로는 쓰지 않는다. 기존 설정 파일이 이 키를
    /// 갖고 있어도 뜨도록 받기만 한다(항목 크기 상한은 저장소가 정한다).
    #[serde(default = "default_core_max")]
    #[allow(dead_code)]
    pub core_max_bytes: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            core_max_bytes: default_core_max(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    /// compact를 켜면 이것은 compact가 듣지 않을 때의 안전 상한이다(결정 0004).
    #[serde(default = "default_rotate_tokens")]
    pub rotate_context_tokens: u64,
    /// compact를 끈 경우에만 쓴다.
    #[serde(default = "default_rotate_days")]
    pub rotate_after_days: i64,
    /// 상주 세션을 교대 대신 compact로 줄인다(결정 0004).
    #[serde(default = "default_true")]
    pub compaction: bool,
    /// 실행이 끝난 뒤 컨텍스트가 이 이상이면 compact.
    #[serde(default = "default_compact_tokens")]
    pub compact_context_tokens: u64,
    /// 마지막 실행 뒤 이만큼 조용하면(그리고 아래 크기 이상이면) compact.
    #[serde(default = "default_idle_compact_minutes")]
    pub idle_compact_minutes: i64,
    #[serde(default = "default_idle_compact_min_tokens")]
    pub idle_compact_min_tokens: u64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            rotate_context_tokens: default_rotate_tokens(),
            rotate_after_days: default_rotate_days(),
            compaction: true,
            compact_context_tokens: default_compact_tokens(),
            idle_compact_minutes: default_idle_compact_minutes(),
            idle_compact_min_tokens: default_idle_compact_min_tokens(),
        }
    }
}

/// 기본값의 근거(결정 0004 "기준값 조정"): 창은 100만이지만, 1시간 넘게 쉬면 캐시가 사라져 첫 메시지가
/// 컨텍스트 전체를 다시 쓴다(sonnet, API 환산 100만 토큰당 $4). 그 비용과 남길 맥락 사이에서 고른 값이다.
fn default_compact_tokens() -> u64 {
    200_000
}

fn default_idle_compact_minutes() -> i64 {
    60
}

fn default_idle_compact_min_tokens() -> u64 {
    100_000
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineConfig {
    pub id: String,
    pub cron: String,
    pub action: String,
    #[serde(default = "default_max_delay")]
    pub max_delay_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_approval_hours() -> i64 {
    24
}

fn default_timezone() -> String {
    "Asia/Seoul".into()
}
fn default_sbx() -> PathBuf {
    "sbx".into()
}
fn default_timeout() -> u64 {
    300
}
fn default_task_timeout() -> u64 {
    1800
}
fn default_chat_timeout() -> u64 {
    3600
}
fn default_mcp_bind() -> SocketAddr {
    "127.0.0.1:47810".parse().expect("기본 MCP 주소")
}
fn default_host_alias() -> String {
    "host.docker.internal".into()
}
fn default_model() -> String {
    "sonnet".into()
}
fn default_core_max() -> usize {
    8 * 1024
}
fn default_rotate_tokens() -> u64 {
    250_000
}
fn default_rotate_days() -> i64 {
    7
}
fn default_max_delay() -> u64 {
    3600
}

fn is_private(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_private(),
        std::net::IpAddr::V6(v6) => (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// `~/`로 시작하면 홈 디렉터리로 바꾼다.
fn expand_home(path: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(rest))
            .unwrap_or_else(|| path.to_path_buf()),
        Err(_) => path.to_path_buf(),
    }
}

impl Config {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let mut cfg: Config = toml::from_str(text).context("설정 파일 형식")?;
        cfg.data_dir = expand_home(&cfg.data_dir);
        cfg.credentials_dir = expand_home(&cfg.credentials_dir);
        cfg.sandbox.workdir = expand_home(&cfg.sandbox.workdir);
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("설정 파일을 읽을 수 없다: {}", path.display()))?;
        let mut cfg =
            Self::parse(&text).with_context(|| format!("설정 파일: {}", path.display()))?;
        cfg.source = Some(path.to_path_buf());
        Ok(cfg)
    }

    fn validate(&self) -> anyhow::Result<()> {
        self.tz()?;
        anyhow::ensure!(
            self.sandbox.workdir.is_absolute(),
            "sandbox.workdir는 절대 경로여야 한다"
        );
        anyhow::ensure!(
            self.mcp.bind.ip().is_loopback(),
            "mcp.bind는 loopback이어야 한다(VM은 host.docker.internal로 접근한다)"
        );
        anyhow::ensure!(
            self.mcp.bind.port() != 0,
            "mcp.bind에 고정 포트가 필요하다(샌드박스 정책에서 허용)"
        );
        for addr in &self.gateway.bind {
            // 접속 API(결정 0021): 집 LAN·VPN·루프백 주소만, 다른 서버와 포트가 겹치지 않게.
            anyhow::ensure!(
                !addr.ip().is_unspecified() && crate::gateway::local_peer(addr.ip()),
                "gateway.bind {addr}: 집 LAN·VPN·루프백 주소만 받는다(0.0.0.0·공인 주소 금지)"
            );
            anyhow::ensure!(
                addr.port() != self.mcp.bind.port()
                    && addr.port() != self.browser.proxy_bind.port()
                    && !self.admin.bind.iter().any(|a| a.port() == addr.port()),
                "gateway.bind {addr}: 관리 페이지·MCP·브라우저 프록시와 다른 포트여야 한다"
            );
        }
        if self.session.compaction {
            // 안전 상한이 compact 기준 이하면 compact보다 교대가 먼저 일어나 맥락이 통째로 사라진다.
            let s = &self.session;
            anyhow::ensure!(
                s.compact_context_tokens < s.rotate_context_tokens,
                "session.compact_context_tokens({})는 rotate_context_tokens({})보다 작아야 한다(교대가 compact보다 먼저 일어난다)",
                s.compact_context_tokens,
                s.rotate_context_tokens
            );
            anyhow::ensure!(
                s.idle_compact_min_tokens <= s.compact_context_tokens,
                "session.idle_compact_min_tokens({})는 compact_context_tokens({}) 이하여야 한다",
                s.idle_compact_min_tokens,
                s.compact_context_tokens
            );
        }
        if self.scripts.enabled {
            // 스크립트(결정 0025): 상주·브라우저 샌드박스와 섞지 않는다(훅의 이중 방어, 로그인 쿠키).
            let sc = &self.scripts;
            anyhow::ensure!(
                sc.sandbox != self.sandbox.name && sc.sandbox != self.browser.sandbox,
                "scripts.sandbox는 상주({})·브라우저({}) 샌드박스와 달라야 한다",
                self.sandbox.name,
                self.browser.sandbox
            );
            anyhow::ensure!(
                !sc.sandbox.is_empty()
                    && sc
                        .sandbox
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'),
                "scripts.sandbox 이름이 이상하다: {:?}",
                sc.sandbox
            );
            anyhow::ensure!(
                (5..=55).contains(&sc.chat_timeout_secs),
                "scripts.chat_timeout_secs는 5~55(MCP 요청이 60초에 끊긴다)"
            );
            anyhow::ensure!(
                (5..=600).contains(&sc.routine_timeout_secs),
                "scripts.routine_timeout_secs는 5~600"
            );
            if sc.net {
                let port = sc.proxy_bind.port();
                anyhow::ensure!(
                    sc.proxy_bind.ip().is_loopback(),
                    "scripts.proxy_bind는 loopback이어야 한다(스크립트 샌드박스는 host.docker.internal로 접근한다)"
                );
                anyhow::ensure!(
                    port != self.mcp.bind.port()
                        && !self.admin.bind.iter().any(|a| a.port() == port)
                        && !(self.browser.enabled && self.browser.proxy_bind.port() == port)
                        && self.gateway.bind.iter().all(|a| a.port() != port),
                    "scripts.proxy_bind 포트는 MCP·관리 페이지·브라우저 프록시·접속 API와 달라야 한다"
                );
            }
        }
        if self.browser.enabled {
            let b = &self.browser;
            anyhow::ensure!(
                b.sandbox != self.sandbox.name,
                "browser.sandbox는 상주 샌드박스({})와 달라야 한다(로그인된 브라우저와 에이전트를 섞지 않는다)",
                self.sandbox.name
            );
            anyhow::ensure!(
                !b.sandbox.is_empty()
                    && b.sandbox
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'),
                "browser.sandbox 이름이 이상하다: {:?}",
                b.sandbox
            );
            crate::browser::sites::select(&b.sites)
                .map_err(|e| anyhow::anyhow!("browser.sites: {e}"))?;
            anyhow::ensure!(b.idle_minutes >= 1, "browser.idle_minutes는 1 이상");
            anyhow::ensure!(
                b.proxy_bind.ip().is_loopback(),
                "browser.proxy_bind는 loopback이어야 한다(브라우저 샌드박스는 host.docker.internal로 접근한다)"
            );
            anyhow::ensure!(
                b.proxy_bind.port() != self.mcp.bind.port()
                    && !self
                        .admin
                        .bind
                        .iter()
                        .any(|a| a.port() == b.proxy_bind.port()),
                "browser.proxy_bind 포트는 MCP·관리 페이지와 달라야 한다"
            );
            anyhow::ensure!(
                (1..=60).contains(&b.login_input_minutes),
                "browser.login_input_minutes는 1~60"
            );
        }
        {
            // 외부 노출(결정 0043): 공개 주소는 https, 그때는 믿는 프록시가 있어야 한다.
            let p = &self.public;
            let proxies = crate::public::Proxies::parse(&p.trusted_proxies)
                .map_err(|e| anyhow::anyhow!("public.trusted_proxies: {e}"))?;
            for (key, url) in [("admin_url", &p.admin_url), ("gateway_url", &p.gateway_url)] {
                if let Some(url) = url {
                    crate::public::public_host(url)
                        .map_err(|e| anyhow::anyhow!("public.{key}: {e}"))?;
                    anyhow::ensure!(
                        !proxies.is_empty(),
                        "public.{key}를 쓰려면 public.trusted_proxies(리버스 프록시 주소)가 필요하다"
                    );
                }
            }
            if let Some(name) = &p.real_ip_header {
                anyhow::ensure!(
                    axum::http::HeaderName::from_bytes(name.trim().as_bytes()).is_ok(),
                    "public.real_ip_header가 머리 이름이 아니다: {name:?}"
                );
            }
            if p.gateway_url.is_some() {
                anyhow::ensure!(
                    !self.gateway.bind.is_empty(),
                    "public.gateway_url을 쓰려면 gateway.bind(프록시가 붙을 주소)가 필요하다"
                );
            }
        }
        anyhow::ensure!(!self.admin.bind.is_empty(), "admin.bind가 비어 있다");
        for addr in &self.admin.bind {
            anyhow::ensure!(
                addr.port() != self.mcp.bind.port(),
                "관리 페이지와 MCP는 다른 포트여야 한다(샌드박스는 MCP 포트에 닿는다)"
            );
            anyhow::ensure!(
                addr.ip().is_loopback() || is_private(addr.ip()),
                "admin.bind는 loopback이나 사설 LAN 주소만 된다(인터넷에 노출하지 않는다): {addr}"
            );
        }
        for r in &self.routines {
            crate::scheduler::next_after(
                lucia_store::ScheduleKind::Cron,
                &r.cron,
                &self.timezone,
                chrono::Utc::now(),
            )
            .with_context(|| format!("루틴 {}", r.id))?;
        }
        Ok(())
    }

    pub fn tz(&self) -> anyhow::Result<chrono_tz::Tz> {
        self.timezone
            .parse()
            .map_err(|e| anyhow::anyhow!("시간대 {:?}: {e}", self.timezone))
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("lucia.sqlite")
    }

    pub fn routines(&self) -> Vec<Routine> {
        let mut list: Vec<Routine> = self
            .routines
            .iter()
            .map(|r| Routine {
                id: r.id.clone(),
                cron: r.cron.clone(),
                timezone: self.timezone.clone(),
                action: r.action.clone(),
                max_delay_secs: r.max_delay_secs,
            })
            .collect();
        if self.backup.enabled && !list.iter().any(|r| r.id == BACKUP_ROUTINE_ID) {
            list.push(Routine {
                id: BACKUP_ROUTINE_ID.into(),
                cron: self.backup.cron.clone(),
                timezone: self.timezone.clone(),
                action: crate::worker::actions::SESSION_BACKUP.into(),
                max_delay_secs: 6 * 3600,
            });
        }
        if !list.iter().any(|r| r.id == WRAPUP_ROUTINE_ID) {
            list.push(Routine {
                id: WRAPUP_ROUTINE_ID.into(),
                cron: "30 20 * * *".into(),
                timezone: self.timezone.clone(),
                action: crate::worker::actions::DAILY_WRAPUP.into(),
                max_delay_secs: 6 * 3600,
            });
        }
        // 주간 자기 보고와 저녁 미리보기(결정 0035). 관리 페이지에서 끈다.
        for (id, cron, action) in [
            (
                crate::reports::WEEKLY_ID,
                crate::reports::WEEKLY_CRON,
                crate::reports::WEEKLY_ACTION,
            ),
            (
                crate::reports::EVENING_ID,
                crate::reports::EVENING_CRON,
                crate::reports::EVENING_ACTION,
            ),
        ] {
            if !list.iter().any(|r| r.id == id) {
                list.push(Routine {
                    id: id.into(),
                    cron: cron.into(),
                    timezone: self.timezone.clone(),
                    action: action.into(),
                    // 늦은 보고는 건너뛴다(때가 지난 저녁 미리보기는 쓸모가 없다).
                    max_delay_secs: 2 * 3600,
                });
            }
        }
        // 교차 정합성 검사(결정 0033): 하루 두 번, 볼 것이 없으면 모델 없이 끝난다.
        if !list.iter().any(|r| r.id == crate::consistency::ROUTINE_ID) {
            list.push(Routine {
                id: crate::consistency::ROUTINE_ID.into(),
                cron: crate::consistency::CRON.into(),
                timezone: self.timezone.clone(),
                action: crate::consistency::ACTION.into(),
                max_delay_secs: 3 * 3600,
            });
        }
        if !list.iter().any(|r| r.id == PERIODIC_CHECK_ROUTINE_ID) {
            list.push(Routine {
                id: PERIODIC_CHECK_ROUTINE_ID.into(),
                cron: "0 9-21/3 * * *".into(),
                timezone: self.timezone.clone(),
                action: crate::worker::actions::PERIODIC_CHECK.into(),
                // 늦은 점검은 건너뛴다(다음 점검이 곧 온다).
                max_delay_secs: 3600,
            });
        }
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
data_dir = "~/.local/share/lucia"
credentials_dir = "~/.config/lucia/credentials"

[sandbox]
name = "lucia"
workdir = "/home/u/.local/share/lucia/workspace"
plugin_dir = "/opt/lucia/plugin"
character_file = "/opt/lucia/character/character.md"

[models]
chat = "sonnet"
briefing = "haiku"

[[routines]]
id = "morning-briefing"
cron = "30 7 * * *"
action = "morning_briefing"
"#;

    #[test]
    fn parses_sample_with_defaults() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(cfg.timezone, "Asia/Seoul");
        assert!(!cfg.data_dir.starts_with("~"), "홈 경로를 펼친다");
        assert_eq!(cfg.mcp.bind.port(), 47810);
        assert_eq!(cfg.models.briefing, "haiku");
        assert_eq!(cfg.memory.core_max_bytes, 8192);
        assert_eq!(cfg.routines()[0].max_delay_secs, 3600);
        assert_eq!(cfg.routines()[0].timezone, "Asia/Seoul");
    }

    #[test]
    fn rejects_bad_values() {
        assert!(Config::parse(&SAMPLE.replace("30 7 * * *", "not cron")).is_err());
        assert!(Config::parse(&format!("timezone = \"Mars/Base\"\n{SAMPLE}")).is_err());
        assert!(Config::parse(&format!("{SAMPLE}\n[mcp]\nbind = \"0.0.0.0:47810\"\n")).is_err());
        assert!(
            Config::parse(&format!("unknown_key = 1\n{SAMPLE}")).is_err(),
            "오타를 잡는다"
        );
        let admin = |bind: &str| Config::parse(&format!("{SAMPLE}\n[admin]\nbind = [{bind}]\n"));
        assert!(
            admin("\"127.0.0.1:47810\"").is_err(),
            "MCP와 같은 포트는 거부"
        );
        assert!(admin("\"0.0.0.0:8780\"").is_err(), "모든 인터페이스는 거부");
        assert!(admin("\"8.8.8.8:8780\"").is_err(), "공인 주소는 거부");
        let ok = admin("\"192.168.0.10:8780\", \"127.0.0.1:8780\"").unwrap();
        assert_eq!(ok.admin.bind.len(), 2);
    }

    /// compact 기준값(결정 0004 "기준값 조정"): 기본값, 그리고 교대가 compact보다 먼저 일어나는 조합은 거부.
    #[test]
    fn compaction_thresholds_must_leave_room_for_compact() {
        let cfg = Config::parse(SAMPLE).unwrap();
        let s = &cfg.session;
        assert_eq!(
            (
                s.compact_context_tokens,
                s.rotate_context_tokens,
                s.idle_compact_min_tokens
            ),
            (200_000, 250_000, 100_000)
        );
        let session = |body: &str| Config::parse(&format!("{SAMPLE}\n[session]\n{body}\n"));
        assert!(
            session("rotate_context_tokens = 150000").is_err(),
            "상한이 compact 기준(20만)보다 낮다"
        );
        assert!(
            session("compact_context_tokens = 300000\nrotate_context_tokens = 300000").is_err()
        );
        assert!(
            session("idle_compact_min_tokens = 250000\nrotate_context_tokens = 300000").is_err()
        );
        assert!(
            session("compaction = false\nrotate_context_tokens = 150000").is_ok(),
            "compact를 끄면 예전 교대 규칙만 쓴다"
        );
        assert!(session("compact_context_tokens = 300000\nrotate_context_tokens = 400000").is_ok());
    }
    #[test]
    fn browser_config_is_off_by_default_and_checked_when_on() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert!(!cfg.browser.enabled);
        assert_eq!(cfg.browser.sandbox, "lucia-browser");
        let browser =
            |body: &str| Config::parse(&format!("{SAMPLE}\n[browser]\nenabled = true\n{body}\n"));
        assert!(browser("").is_ok());
        assert!(
            browser("sandbox = \"lucia\"").is_err(),
            "상주 샌드박스와 같은 이름은 거부"
        );
        assert!(browser("sites = [\"naver\"]").is_err(), "모르는 사이트");
        assert_eq!(cfg.browser.proxy_bind.port(), 47811);
        assert!(
            browser("proxy_bind = \"0.0.0.0:47811\"").is_err(),
            "루프백만"
        );
        assert!(
            browser("proxy_bind = \"127.0.0.1:47810\"").is_err(),
            "MCP 포트와 겹침"
        );
        assert!(browser("proxy_bind = \"127.0.0.1:47812\"").is_ok());
        assert!(browser("sandbox = \"x; rm\"").is_err());
        let with_admin = Config::parse(&format!(
            "{SAMPLE}\n[admin]\nbind = [\"127.0.0.1:8780\", \"192.168.0.10:8780\"]\n"
        ))
        .unwrap();
        assert_eq!(with_admin.login_form_base(), "http://192.168.0.10:8780");
    }

    /// 외부 노출(결정 0043): 비면 아무것도 바뀌지 않고, 쓰면 https와 믿는 프록시가 있어야 한다.
    #[test]
    fn public_section_is_off_by_default_and_checked() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert!(cfg.public.admin_host().is_none() && !cfg.public.gateway_open());
        let public = |body: &str| Config::parse(&format!("{SAMPLE}\n[public]\n{body}\n"));
        assert!(
            public("admin_url = \"https://lucia.example.com\"").is_err(),
            "프록시 없이"
        );
        assert!(
            public("admin_url = \"http://lucia.example.com\"\ntrusted_proxies = [\"172.18.0.11\"]")
                .is_err(),
            "http는 거부"
        );
        assert!(
            public("admin_url = \"https://lucia.example.com\"\ntrusted_proxies = [\"8.8.8.8\"]")
                .is_err(),
            "공인 주소를 프록시로 믿지 않는다"
        );
        assert!(
            public(
                "gateway_url = \"https://api.example.com\"\ntrusted_proxies = [\"172.18.0.0/16\"]"
            )
            .is_err(),
            "접속 API를 열려면 gateway.bind가 있어야 한다"
        );
        let ok = public(
            "admin_url = \"https://Lucia.example.com/\"\ntrusted_proxies = [\"172.18.0.0/16\"]",
        )
        .unwrap();
        assert_eq!(ok.public.admin_host().as_deref(), Some("lucia.example.com"));
        assert_eq!(
            ok.login_form_base(),
            "https://Lucia.example.com",
            "입력 링크는 공개 주소로"
        );
    }
}
