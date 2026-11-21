//! 초기 설정(결정 0064): 처음 쓰는 사람이 이름, 대화할 곳, 선택 연결을 차례로 정하는 화면의 데이터.
//!
//! - **필수는 대화할 곳 하나뿐이다**: Discord(봇 토큰과 소유자)나 알림 받을 곳으로 고른 앱. 나머지는 모두 선택이다.
//! - **계산은 데몬이 한다**: 단계마다 상태(`done`·`todo`·`optional`)와 한 줄 요약을 여기서 만들어 스냅숏의 `setup`에 싣는다.
//!   화면은 받은 글을 그대로 그린다(`web/admin/docs/derived.md` 14장).
//! - **이 화면은 읽기만 하는 것이 있다**: 샌드박스 만들기와 설정 파일 고치기는 데몬이 하지 않는다. 환경 단계는 상태와
//!   사용자가 터미널에서 돌릴 명령만 보여 준다.

use lucia_store::Store;
use serde_json::{Value, json};

use super::snapshot::{Page, Snapshot, Tone};
use crate::discord_ctl::DiscordState;

/// 홈의 초기 설정 안내를 껐을 때 저장소에 적는 설정(`1`).
pub const DISMISSED_KEY: &str = "setup.dismissed";

/// 홈의 안내를 껐나.
pub fn dismissed(store: &Store) -> bool {
    store
        .setting(DISMISSED_KEY)
        .ok()
        .flatten()
        .is_some_and(|v| v == "1")
}

/// 대화할 곳이 있나(초기 설정의 필수): Discord 토큰과 소유자가 있거나, 알림 받을 곳이 앱이다.
/// 로그인 뒤 첫 화면을 고를 때와 스냅숏이 같은 규칙을 쓴다.
pub fn channel_ready(discord_token: bool, discord_owner: bool, app_notify: bool) -> bool {
    (discord_token && discord_owner) || app_notify
}

/// 로그인한 뒤 처음 보여 줄 화면: 대화할 곳이 없고 안내를 끄지 않았으면 초기 설정, 아니면 홈.
pub fn landing(channel_ready: bool, dismissed: bool) -> &'static str {
    if channel_ready || dismissed {
        Page::Overview.path()
    } else {
        Page::Start.path()
    }
}

/// 단계 하나: 화면이 그대로 그린다. `state_label`·`tone`은 상태 배지의 글과 결이다.
fn step(key: &str, title: &str, icon: &str, state: &str, summary: String) -> Value {
    let (state_label, tone) = match state {
        "done" => ("완료", Tone::Ok),
        "todo" => ("필요", Tone::Warn),
        _ => ("선택", Tone::Muted),
    };
    json!({
        "key": key,
        "title": title,
        "icon": icon,
        "state": state,
        "state_label": state_label,
        "tone": tone_name(tone),
        "summary": summary,
    })
}

/// 끝났으면 `done`, 아니면 대화할 곳이 없을 때 이 단계가 길 중 하나면 `todo`, 그 밖은 `optional`.
fn state_of(done: bool, needed: bool) -> &'static str {
    if done {
        "done"
    } else if needed {
        "todo"
    } else {
        "optional"
    }
}

fn tone_name(tone: Tone) -> &'static str {
    match tone {
        Tone::Ok => "ok",
        Tone::Muted => "muted",
        Tone::Info => "info",
        Tone::Warn => "warn",
        Tone::Bad => "bad",
    }
}

fn usd(v: f64) -> String {
    format!("${v:.2}")
}

/// 점검 결과에서 이름으로 한 항목을 찾는다.
fn check<'a>(s: &'a Snapshot, name: &str) -> Option<&'a crate::check::Check> {
    s.checks.iter().find(|c| c.name == name)
}

/// 환경 한 줄: 이름, 결, 상태 글, 터미널에서 돌릴 명령.
fn env_item(key: &str, label: &str, tone: Tone, detail: String, command: Option<&str>) -> Value {
    json!({
        "key": key,
        "label": label,
        "tone": tone_name(tone),
        "detail": detail,
        "command": command,
    })
}

/// 점검 항목 하나를 환경 한 줄로: 아직 점검하지 않았으면 그렇게 말한다.
fn env_from_check(s: &Snapshot, key: &str, label: &str, check_name: &str, command: &str) -> Value {
    match check(s, check_name) {
        Some(c) => env_item(
            key,
            label,
            if c.ok { Tone::Ok } else { Tone::Bad },
            c.detail.clone(),
            Some(command),
        ),
        None => env_item(
            key,
            label,
            Tone::Muted,
            "아직 점검하지 않았습니다.".into(),
            Some(command),
        ),
    }
}

/// 환경 단계의 줄들(읽기 전용).
fn env_items(s: &Snapshot) -> Vec<Value> {
    let mut out = Vec::new();
    out.push(match &s.setup.config_path {
        Some(path) => env_item(
            "config",
            "설정 파일",
            Tone::Info,
            format!("{path} — 고친 뒤에는 데몬을 다시 시작합니다."),
            None,
        ),
        None => env_item(
            "config",
            "설정 파일",
            Tone::Info,
            "기본 위치는 ~/.config/lucia/lucia.toml입니다(예시: 저장소의 deploy/lucia.example.toml). 고친 뒤에는 데몬을 다시 시작합니다."
                .into(),
            None,
        ),
    });
    out.push(env_from_check(
        s,
        "sandbox",
        "상주 샌드박스",
        "상주 샌드박스",
        "deploy/create-sandbox.sh",
    ));
    out.push(if s.browser.is_some() {
        env_from_check(
            s,
            "browser",
            "브라우저 샌드박스",
            "브라우저 샌드박스",
            "deploy/create-browser-sandbox.sh",
        )
    } else {
        env_item(
            "browser",
            "브라우저 샌드박스",
            Tone::Muted,
            "꺼져 있습니다(선택). 샌드박스를 만든 뒤 설정 파일의 [browser] enabled = true로 켭니다.".into(),
            Some("deploy/create-browser-sandbox.sh"),
        )
    });
    out.push(match &s.scripts_sandbox {
        Some(name) => env_item(
            "scripts",
            "스크립트 샌드박스",
            Tone::Ok,
            format!("켜져 있습니다({name}). 실제 샌드박스에서 확인하려면 점검 명령을 돌립니다."),
            Some("luciad script-check"),
        ),
        None => env_item(
            "scripts",
            "스크립트 샌드박스",
            Tone::Muted,
            "꺼져 있습니다(선택). 샌드박스를 만든 뒤 설정 파일의 [scripts] enabled = true로 켜고 점검합니다.".into(),
            Some("deploy/create-script-sandbox.sh"),
        ),
    });
    out.push(if s.gateway_binds.is_empty() {
        env_item(
            "gateway",
            "접속 API(앱)",
            Tone::Muted,
            "꺼져 있습니다. 앱을 쓰려면 설정 파일의 [gateway] bind에 집 LAN·VPN 주소를 넣습니다."
                .into(),
            None,
        )
    } else {
        let binds: Vec<String> = s.gateway_binds.iter().map(|b| b.to_string()).collect();
        env_item(
            "gateway",
            "접속 API(앱)",
            Tone::Ok,
            format!("켜져 있습니다({}).", binds.join(", ")),
            None,
        )
    });
    out.push(env_item(
        "checks",
        "실행 환경 점검",
        checks_tone(s),
        checks_summary(s),
        Some("luciad check"),
    ));
    out
}

fn failed_checks(s: &Snapshot) -> Vec<&'static str> {
    s.checks.iter().filter(|c| !c.ok).map(|c| c.name).collect()
}

fn checks_tone(s: &Snapshot) -> Tone {
    if s.checks.is_empty() {
        Tone::Muted
    } else if failed_checks(s).is_empty() {
        Tone::Ok
    } else {
        Tone::Bad
    }
}

fn checks_summary(s: &Snapshot) -> String {
    let failed = failed_checks(s);
    if s.checks.is_empty() {
        "아직 점검하지 않았습니다.".into()
    } else if failed.is_empty() {
        format!("점검 {}개 모두 정상입니다.", s.checks.len())
    } else {
        format!(
            "점검 {}개 중 {}개가 실패했습니다: {}",
            s.checks.len(),
            failed.len(),
            failed.join(", ")
        )
    }
}

fn discord_summary(s: &Snapshot) -> String {
    let d = &s.discord;
    let link = match &d.state {
        DiscordState::Connected { bot } => format!("봇 {bot} 연결됨"),
        DiscordState::Connecting { bot } => format!("봇 {bot}에 연결하는 중"),
        DiscordState::Stopped { reason } => format!("연결이 멈췄습니다: {reason}"),
        DiscordState::NotConfigured if d.token_set => "봇 토큰은 있으나 아직 연결 전".into(),
        DiscordState::NotConfigured => "봇 토큰이 없습니다".into(),
    };
    let owner = if d.owner.is_some() {
        "소유자 설정됨"
    } else {
        "소유자 없음"
    };
    format!("{link} · {owner}")
}

fn app_summary(s: &Snapshot) -> String {
    if s.gateway_binds.is_empty() {
        return "접속 API가 꺼져 있습니다(설정 파일 [gateway] bind)".into();
    }
    let apps = s
        .clients
        .iter()
        .filter(|(c, _)| c.revoked_at.is_none() && c.role == crate::gateway::Role::App.as_str())
        .count();
    let notify = match &s.app.notify_client {
        Some(id) => s
            .clients
            .iter()
            .find(|(c, _)| &c.id == id)
            .map_or("앱".to_owned(), |(c, _)| format!("앱 {}", c.name)),
        None => "Discord DM".to_owned(),
    };
    let push = if s.app.fcm_ready { "켜짐" } else { "꺼짐" };
    format!("앱 토큰 {apps}개 · 알림 받는 곳 {notify} · 푸시 {push}")
}

fn google_summary(s: &Snapshot) -> String {
    let g = &s.google;
    let status = super::derive::google_status(g).0;
    match (&g.account, g.connected) {
        (Some(account), true) => format!("{status} · {account}"),
        (None, true) => format!("{status} · 계정 확인 안 됨"),
        _ => status.to_owned(),
    }
}

fn profile_summary(s: &Snapshot) -> String {
    let p = &s.profile;
    let name = if p.name_set {
        format!("이름 “{}”", p.name)
    } else {
        format!("이름 “{}”(기본값)", p.name)
    };
    let avatar = if p.avatar.is_some() {
        "프로필 그림 있음"
    } else {
        "프로필 그림 없음"
    };
    format!("{name} · {avatar}")
}

fn images_summary(s: &Snapshot) -> String {
    if s.images.key_set {
        format!("OpenAI 키 저장됨 · 하루 예산 {}", usd(s.images.daily_cap))
    } else {
        "OpenAI 키가 없습니다(그림 만들기가 꺼져 있습니다)".into()
    }
}

/// 프로필 그림의 이름표: 누가 정했나, 크기와 형식.
fn avatar_ui(a: &crate::profile::Avatar) -> Value {
    let by = match a.by.as_str() {
        "lucia" => "Lucia가 정함",
        "owner" | "user" => "내가 정함",
        other => other,
    };
    let kind = match a.mime.as_str() {
        "image/png" => "PNG",
        "image/jpeg" => "JPEG",
        other => other,
    };
    json!({
        "by_label": by,
        "facts": format!(
            "{}×{} · {kind} · {}",
            a.width,
            a.height,
            crate::files::human_size(a.size)
        ),
    })
}

/// 스냅숏(직렬화한 것)의 `setup`에 완료 여부, 단계, 환경 줄을 더하고, 프로필 그림에 이름표를 단다.
pub fn decorate(snap: &mut Value, s: &Snapshot) {
    let channel = channel_ready(
        s.discord.token_set,
        s.discord.owner.is_some(),
        s.app.notify_client.is_some(),
    );
    let discord_done = s.discord.token_set && s.discord.owner.is_some();
    let app_done = s.app.notify_client.is_some();
    let steps = vec![
        step(
            "profile",
            "이름과 모습",
            "smile",
            if s.profile.name_set || s.profile.avatar.is_some() {
                "done"
            } else {
                "optional"
            },
            profile_summary(s),
        ),
        step(
            "discord",
            "Discord",
            "chat",
            state_of(discord_done, !channel),
            discord_summary(s),
        ),
        step(
            "google",
            "Google 메일·일정",
            "mail",
            state_of(s.google.connected, false),
            google_summary(s),
        ),
        step(
            "app",
            "앱과 푸시",
            "bell",
            state_of(app_done, !channel),
            app_summary(s),
        ),
        step(
            "images",
            "이미지 생성",
            "spark",
            state_of(s.images.key_set, false),
            images_summary(s),
        ),
        step(
            "env",
            "환경",
            "server",
            if checks_tone(s) == Tone::Ok {
                "done"
            } else {
                "optional"
            },
            checks_summary(s),
        ),
    ];
    if let Some(setup) = snap.get_mut("setup").and_then(Value::as_object_mut) {
        setup.insert("complete".into(), json!(channel));
        setup.insert("steps".into(), json!(steps));
        setup.insert("env".into(), json!(env_items(s)));
    }
    if let (Some(avatar), Some(a)) = (snap.pointer_mut("/profile/avatar"), &s.profile.avatar)
        && let Some(map) = avatar.as_object_mut()
    {
        map.insert("ui".into(), avatar_ui(a));
    }
}
