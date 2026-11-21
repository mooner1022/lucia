//! 화면 검사용 시험 데이터(결정 0061).
//!
//! 화면(Svelte)은 브라우저에서 돌므로, 데몬의 시험 데이터를 JSON으로 내려 주고 화면 쪽 시험이 그것으로 그린다.
//!
//! - [`samples`]: 시험 데이터의 상태들(스냅숏). `full`·`alt`·`open_security`는 모든 문자열 칸에 표지(`qz-…`)가 심겨 있다.
//! - [`pages`]: 한 상태(스냅숏)의 모든 화면과 하위 화면. 이름은 개편 전 화면 목록과 같다.
//! - `web/admin/fixtures/`: 위 데이터를 파일로 둔 것. 화면 쪽 시험(vitest)이 읽는다. 데몬이 바뀌어 어긋나면
//!   [`ui_fixtures_are_current`]가 실패한다. 다시 만들기:
//!   `cargo test -p luciad write_ui_fixtures -- --ignored`
//!   (`format-cases.json`은 데몬의 표기 함수가 낸 글로, 화면 쪽 `format.ts`가 같은 글을 내는지 맞춰 본다.)
//! - `web/admin/fixtures/baseline-by-page.json`은 데몬이 만들지 않는다. 개편 전(서버가 HTML을 그리던) 화면에서
//!   뽑아 둔 화면별 폼·입력·링크·표지 목록을 그대로 얼려 둔 기록이다(예전 화면 코드는 지웠다). 고치지 않는다.
//! - [`serve_ui_fixtures`]: 실제 라우터를 시험 데이터로 띄운다(브라우저 검사: 로그인, CSRF, CSP, 폼 왕복).
//!   `LUCIA_UI_FIXTURE_ADDR=127.0.0.1:47890 cargo test -p luciad serve_ui_fixtures -- --ignored --nocapture`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use lucia_store::{MemoryAction, MemoryAuthor};
use serde_json::{Value, json};

use super::snapshot::{ActivityFilter, Flash, Page, Snapshot};
use super::spa::{self, Standalone};
use super::{AdminState, current_session, extra, setup};

mod samples;

/// 상태 묶음의 이름. `stress`는 `full`을 길고 많은 데이터로 부풀린 것이다(배치 검사용).
pub const SETS: [&str; 6] = ["healthy", "fresh", "troubled", "full", "alt", "open"];

fn snapshot(set: &str) -> Snapshot {
    match set {
        "healthy" => samples::healthy(),
        "fresh" => samples::fresh(),
        "troubled" => samples::troubled(),
        "alt" => samples::alt(),
        "open" => samples::open_security(),
        _ => samples::full(),
    }
}

/// 화면 하나: 예전 목록의 이름, 화면 검사가 여는 주소, HTTP 상태, 데이터.
pub struct FixturePage {
    pub name: String,
    pub url: String,
    pub status: u16,
    pub data: Value,
}

/// 한 상태의 모든 화면(하위 화면 포함). 이름과 순서는 개편 전 화면 목록과 같다(그 코드는 지웠다).
pub fn pages(s: &Snapshot) -> Vec<FixturePage> {
    let csrf = samples::CSRF;
    let flash = samples::flash_sample();
    let mut out = Vec::new();
    let mut add =
        |name: &str, url: &str, status: u16, page: Page, flash: Option<&Flash>, extra: Value| {
            out.push(FixturePage {
                name: name.to_owned(),
                url: url.to_owned(),
                status,
                data: spa::page(page, s, csrf, flash, extra),
            });
        };
    let character = |loaded: &Result<(String, String), String>,
                     history: &[lucia_store::CharacterRevision],
                     picked: bool| {
        extra::character(
            loaded,
            history,
            crate::segments::Pace::Slow,
            &samples::sample_models(picked),
        )
    };
    let content = format!(
        "# {}\n\n- {}\n",
        samples::q("char-title"),
        samples::q("char-line")
    );
    let history = vec![
        lucia_store::CharacterRevision {
            id: 2,
            at: s.now,
            note: samples::q("char-hist-note-2"),
            content: content.clone(),
        },
        lucia_store::CharacterRevision {
            id: 1,
            at: s.now,
            note: samples::q("char-hist-note-1"),
            content: String::new(),
        },
    ];
    for page in Page::ALL {
        let extra = match page {
            Page::Activity => extra::activity(ActivityFilter::All),
            Page::Memory => extra::memory_list(""),
            Page::Skills => extra::skills(&samples::skill_samples()),
            Page::Character => character(
                &Ok((samples::q("char-path"), content.clone())),
                &history,
                true,
            ),
            Page::Browser => extra::browser_sites(),
            _ => Value::Null,
        };
        add(page.path(), page.path(), 200, page, Some(&flash), extra);
    }
    for f in ActivityFilter::ALL {
        add(
            &format!("activity?{}", f.key()),
            &format!("/activity?kind={}", f.key()),
            200,
            Page::Activity,
            None,
            extra::activity(f),
        );
    }
    add(
        "memory?q",
        "/memory?q=qz",
        200,
        Page::Memory,
        None,
        extra::memory_list("qz"),
    );
    let entry = s
        .memories
        .first()
        .cloned()
        .unwrap_or_else(|| samples::full().memories[0].clone());
    let revisions = vec![
        samples::memory_revision(
            1,
            MemoryAuthor::Agent,
            Some("chat-1"),
            MemoryAction::Write,
            s,
        ),
        samples::memory_revision(2, MemoryAuthor::User, None, MemoryAction::Write, s),
        samples::memory_revision(
            3,
            MemoryAuthor::Daemon,
            Some("routine-1"),
            MemoryAction::Delete,
            s,
        ),
    ];
    add(
        "memory/entry",
        &format!("/memory/{}", entry.slug),
        200,
        Page::Memory,
        None,
        extra::memory_entry(&entry, &revisions),
    );
    add(
        "memory/missing",
        "/memory/no-such-entry",
        404,
        Page::Memory,
        None,
        extra::memory_missing(),
    );
    let skill = samples::skill_samples().remove(0);
    let skill_revisions = vec![
        lucia_store::SkillRevision {
            revision: 2,
            at: s.now,
            author: "user".into(),
            reason: samples::q("skill-rev2-reason"),
            body: samples::q("skill-rev2-body"),
            script: Some(samples::q("skill-rev2-script")),
        },
        lucia_store::SkillRevision {
            revision: 1,
            at: s.now,
            author: "agent".into(),
            reason: samples::q("skill-rev1-reason"),
            body: samples::q("skill-rev1-body"),
            script: None,
        },
    ];
    add(
        "skills/detail",
        &format!("/skills/{}", skill.slug),
        200,
        Page::Skills,
        None,
        extra::skill_detail(&skill, &skill_revisions),
    );
    let rev = lucia_store::CharacterRevision {
        id: 4,
        at: s.now,
        note: samples::q("char-rev-note"),
        content: samples::q("char-rev-content"),
    };
    add(
        "character/rev-current",
        "/character/rev/4",
        200,
        Page::Character,
        None,
        extra::character_revision(&rev, true),
    );
    add(
        "character/rev-old",
        "/character/rev/3",
        200,
        Page::Character,
        None,
        extra::character_revision(&rev, false),
    );
    add(
        "character/rev-missing",
        "/character/rev/999",
        404,
        Page::Character,
        None,
        extra::character_revision_missing(),
    );
    add(
        "character/load-error",
        "/character?fixture=load-error",
        200,
        Page::Character,
        None,
        extra::character(
            &Err(samples::q("char-load-error")),
            &[],
            crate::segments::Pace::Normal,
            &samples::sample_models(false),
        ),
    );
    let tabs = vec![
        crate::browser::Tab {
            id: samples::q("tab-one"),
            url: format!("https://{}.example.test/", samples::q("tab-one")),
            title: samples::q("tab-one-title"),
        },
        crate::browser::Tab {
            id: samples::q("tab-two"),
            url: format!("https://{}.example.test/", samples::q("tab-two")),
            title: String::new(),
        },
    ];
    let one = samples::q("tab-one");
    for (name, url, available, left, tab, auto) in [
        (
            "handover/off",
            "/browser/handover?fixture=off".to_owned(),
            false,
            None,
            None,
            false,
        ),
        (
            "handover/idle",
            "/browser/handover".to_owned(),
            true,
            None,
            None,
            false,
        ),
        (
            "handover/active",
            format!("/browser/handover?tab={one}"),
            true,
            Some(StdDuration::from_secs(605)),
            Some(one.as_str()),
            false,
        ),
        (
            "handover/auto",
            format!("/browser/handover?tab={one}&auto=1"),
            true,
            Some(StdDuration::from_secs(60)),
            Some(one.as_str()),
            true,
        ),
    ] {
        // 넘겨받는 중이 아니면 탭 목록이 없다(처리기와 같다).
        let shown: &[crate::browser::Tab] = if left.is_some() { &tabs } else { &[] };
        let refresh = (auto && left.is_some()).then_some(3);
        add(
            name,
            &url,
            200,
            Page::Browser,
            None,
            extra::handover(available, left, shown, tab, auto, refresh),
        );
    }
    out
}

/// 로그인 없이 여는 화면들. 이름은 개편 전 목록과 같다.
pub fn standalone() -> Vec<(String, String, Standalone)> {
    let q = samples::q;
    let site = q("site-login");
    let origin = format!("https://{}.example.test", q("site-origin"));
    let login = |error: Option<String>, external| Standalone::Login { error, external };
    let pending = |code_link: Option<&str>| Standalone::SiteLoginPending {
        site: site.clone(),
        note: q("site-pending-note"),
        code_link: code_link.map(str::to_owned),
    };
    let list = vec![
        ("login", login(Some(q("login-error")), false)),
        ("login/plain", login(None, false)),
        ("login/external", login(None, true)),
        ("no-password", Standalone::NoPassword),
        (
            "setup",
            Standalone::Setup {
                token: q("setup-token"),
                error: Some(q("setup-error")),
                min_chars: 12,
            },
        ),
        ("setup/invalid", Standalone::SetupInvalid),
        (
            "google/returned-ok",
            Standalone::GoogleReturned {
                ok: true,
                text: q("returned-ok"),
            },
        ),
        (
            "google/returned-bad",
            Standalone::GoogleReturned {
                ok: false,
                text: q("returned-bad"),
            },
        ),
        (
            "site-login/form",
            Standalone::SiteLoginForm {
                site: site.clone(),
                origin: origin.clone(),
                token: q("site-token"),
                minutes: 10,
                attempts: crate::browser::logins::INPUT_ATTEMPTS,
                error: Some(q("site-error")),
            },
        ),
        (
            "site-login/busy",
            Standalone::SiteLoginBusy { site: site.clone() },
        ),
        (
            "site-login/done",
            Standalone::SiteLoginDone {
                site: site.clone(),
                note: q("site-done-note"),
            },
        ),
        (
            "site-login/pending-code",
            pending(Some("/site-login/code/qz-code-token")),
        ),
        (
            "site-login/pending-handover",
            pending(Some("/browser/handover")),
        ),
        ("site-login/pending-plain", pending(None)),
        (
            "site-login/code",
            Standalone::SiteCodeForm {
                site: site.clone(),
                origin: origin.clone(),
                token: q("code-token"),
                error: Some(q("code-error")),
            },
        ),
        (
            "site-login/failed",
            Standalone::SiteLoginFailed {
                site: site.clone(),
                reason: q("site-failed-why"),
            },
        ),
        ("site-login/invalid", Standalone::SiteLoginInvalid),
        ("not-found", Standalone::NotFound),
    ];
    list.into_iter()
        .map(|(name, screen)| {
            (
                name.to_owned(),
                format!("/login?fixture={}", slug(name)),
                screen,
            )
        })
        .collect()
}

/// 파일 이름으로 쓸 수 있게.
fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() { "home".into() } else { s }
}

// ---------- 배치 검사용: 길고 많은 데이터 ----------

// 끊을 곳이 하나도 없는 글(주소의 `/`나 `-`에서도 줄이 바뀌므로 그런 글자를 넣지 않는다).
const LONG_WORD: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const LONG_TEXT: &str = " 이 글은 화면이 좁을 때 줄이 어떻게 바뀌는지 보려고 일부러 길게 쓴 설명입니다. 여러 줄로 넘어가도 옆의 글이나 단추와 겹치지 않아야 합니다.";

/// 표지가 든 글을 길게 늘이고, 목록을 여러 줄로 불린다(복사본은 이름이 겹치지 않게 꼬리를 붙인다).
fn inflate(v: &mut Value, copy: Option<usize>, key: &str) {
    match v {
        Value::String(text) => {
            if let Some(n) = copy
                && (text.starts_with("qz-") || ["id", "slug", "key", "name"].contains(&key))
            {
                text.push_str(&format!("-d{n}"));
            }
            if text.starts_with("qz-") && !["id", "slug", "key", "token", "tab"].contains(&key) {
                match text.len() % 3 {
                    0 => text.push_str(&format!("_{LONG_WORD}")),
                    1 => text.push_str(LONG_TEXT),
                    _ => {}
                }
            }
        }
        Value::Number(num) => {
            if let (Some(n), Some(id)) = (copy, num.as_i64())
                && key == "id"
            {
                *v = json!(id + 1000 * n as i64);
            }
        }
        Value::Array(list) => {
            for item in list.iter_mut() {
                inflate(item, copy, key);
            }
            // 항목이 객체(또는 [객체, …] 짝)인 목록만 불린다.
            let rows = list.first().is_some_and(|f| {
                f.is_object()
                    || f.as_array()
                        .is_some_and(|a| a.first().is_some_and(Value::is_object))
            });
            if rows
                && copy.is_none()
                && list.len() < 8
                && !["notices", "choices", "paces", "scopes", "tabs"].contains(&key)
            {
                let base = list.clone();
                let mut n = 1;
                while list.len() < 8 {
                    for item in &base {
                        let mut c = item.clone();
                        suffix(&mut c, n, "");
                        list.push(c);
                    }
                    n += 1;
                }
            }
        }
        Value::Object(map) => {
            for (k, item) in map.iter_mut() {
                if k != "consts" && k != "csrf" && k != "path" && k != "page" && k != "view" {
                    inflate(item, copy, k);
                }
            }
        }
        _ => {}
    }
}

/// 복사본의 이름·번호에 꼬리를 붙인다.
fn suffix(v: &mut Value, n: usize, key: &str) {
    match v {
        Value::String(text) => {
            if text.starts_with("qz-") || ["id", "slug", "key", "name"].contains(&key) {
                text.push_str(&format!("-d{n}"));
            }
        }
        Value::Number(num) => {
            if let Some(id) = num.as_i64()
                && (key == "id" || key == "revision")
            {
                *v = json!(id + 1000 * n as i64);
            }
        }
        Value::Array(list) => list.iter_mut().for_each(|i| suffix(i, n, key)),
        Value::Object(map) => map.iter_mut().for_each(|(k, i)| suffix(i, n, k)),
        _ => {}
    }
}

fn stress_pages() -> Vec<FixturePage> {
    let mut list = pages(&samples::full());
    for p in &mut list {
        inflate(&mut p.data, None, "");
    }
    list
}

fn set_pages(set: &str) -> Vec<FixturePage> {
    if set == "stress" {
        stress_pages()
    } else {
        pages(&snapshot(set))
    }
}

// ---------- 파일로 두기 ----------

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/admin/fixtures")
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).expect("json") + "\n"
}

/// 표기 함수의 입력과 데몬이 낸 글(화면 쪽의 같은 함수가 같은 글을 내는지 본다).
fn format_cases() -> Value {
    use super::snapshot::{bytes, exact, num, usd, when};
    use chrono::{Duration, TimeZone, Utc};
    let tz = chrono_tz::Asia::Seoul;
    let now = Utc.with_ymd_and_hms(2026, 11, 20, 3, 30, 10).unwrap();
    let mut times = Vec::new();
    for secs in [
        0,
        5,
        59,
        60,
        61,
        119,
        3_540,
        3_599,
        3_600,
        3_601,
        7_200,
        12_600,
        12_611,
        86_400,
        100_000,
        172_800,
        200_000,
        2_592_000,
        28_000_000,
        40_000_000,
        -1,
        -30,
        -59,
        -60,
        -61,
        -3_540,
        -3_599,
        -3_600,
        -7_200,
        -70_000,
        -86_400,
        -100_000,
        -172_800,
        -3_700_000,
        -40_000_000,
    ] {
        let at = now - Duration::seconds(secs);
        times.push(json!({
            "at": at.to_rfc3339(),
            "now": now.to_rfc3339(),
            "tz": "Asia/Seoul",
            "when": when(at, now, tz),
            "exact": exact(at, tz),
        }));
    }
    let la = chrono_tz::America::Los_Angeles;
    for secs in [3_600 * 5, 3_600 * 30, -3_600 * 20] {
        let at = now - Duration::seconds(secs);
        times.push(json!({
            "at": at.to_rfc3339(),
            "now": now.to_rfc3339(),
            "tz": "America/Los_Angeles",
            "when": when(at, now, la),
            "exact": exact(at, la),
        }));
    }
    let nums: Vec<Value> = [0u64, 7, 999, 1_000, 12_345, 999_999, 1_000_000, 123_456_789]
        .iter()
        .map(|n| json!([n, num(*n)]))
        .collect();
    let usds: Vec<Value> = [0.0, 0.0001, 0.004, 0.0099, 0.01, 0.5, 1.0, 12.345, 100.0]
        .iter()
        .map(|v| json!([v, usd(*v)]))
        .collect();
    let sizes: Vec<Value> = [0usize, 1, 1023, 1024, 1536, 10_000, 1_048_576]
        .iter()
        .map(|n| json!([n, bytes(*n)]))
        .collect();
    json!({ "times": times, "num": nums, "usd": usds, "bytes": sizes })
}

/// 만들 파일 전부: (경로, 내용).
fn files() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut index = Vec::new();
    for set in SETS.into_iter().chain(["stress"]) {
        // 한 묶음의 화면은 같은 스냅숏을 본다. 파일에는 한 번만 두고(`sets/<묶음>.json`), 화면마다 다른 것만
        // 따로 둔다(`pages/<묶음>.json`). 화면 쪽 시험이 둘을 합친다.
        let mut shared: Option<Value> = None;
        let mut list = Vec::new();
        for p in set_pages(set) {
            let mut data = p.data;
            let map = data.as_object_mut().expect("page json");
            let mut common = serde_json::Map::new();
            for key in [
                "snapshot", "notices", "overall", "pending", "consts", "csrf", "kind", "name",
            ] {
                common.insert(key.to_owned(), map.remove(key).unwrap_or(Value::Null));
            }
            let common = Value::Object(common);
            match &shared {
                Some(first) => assert_eq!(first, &common, "{set}: 화면마다 스냅숏이 같아야 한다"),
                None => shared = Some(common),
            }
            index.push(json!({ "set": set, "name": p.name, "url": p.url, "status": p.status }));
            list.push(json!({ "name": p.name, "url": p.url, "status": p.status, "data": data }));
        }
        out.insert(
            format!("sets/{set}.json"),
            pretty(&shared.unwrap_or(Value::Null)),
        );
        out.insert(format!("pages/{set}.json"), pretty(&json!(list)));
    }
    let mut screens = Vec::new();
    for (name, url, screen) in standalone() {
        let file = format!("standalone/{}.json", slug(&name));
        screens.push(json!({ "name": name, "url": url, "file": file }));
        // 로그인 없는 화면의 `name`은 처리기 바깥의 `spa::with_name`이 싣는다(시험 데이터에서는 기본 이름).
        out.insert(
            file,
            pretty(&json!({
                "kind": "standalone",
                "name": crate::profile::DEFAULT_NAME,
                "screen": screen,
            })),
        );
    }
    out.insert(
        "index.json".into(),
        pretty(&json!({ "pages": index, "standalone": screens })),
    );
    out.insert("format-cases.json".into(), pretty(&format_cases()));
    out
}

/// 시험 데이터 파일을 다시 만든다(데몬의 데이터 꼴을 바꿨을 때).
#[test]
#[ignore]
fn write_ui_fixtures() {
    let root = root();
    for dir in ["pages", "sets", "standalone"] {
        let _ = std::fs::remove_dir_all(root.join(dir));
    }
    for (path, text) in files() {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

/// 레포의 시험 데이터 파일이 지금 데몬이 내는 것과 같다(화면 쪽 시험이 낡은 데이터로 통과하지 않게).
#[test]
fn ui_fixtures_are_current() {
    let root = root();
    let mut stale = Vec::new();
    for (path, text) in files() {
        if std::fs::read_to_string(root.join(&path)).ok().as_deref() != Some(text.as_str()) {
            stale.push(path);
        }
    }
    assert!(
        stale.is_empty(),
        "web/admin/fixtures가 낡았다({}개, 예: {}). 다시 만들기: cargo test -p luciad write_ui_fixtures -- --ignored",
        stale.len(),
        stale.first().map(String::as_str).unwrap_or("")
    );
}

// ---------- 실제 라우터로 띄우기 ----------

/// 시험용 그림(회색 PNG). 압축하지 않은 블록으로 적어 인코더 없이 만든다. 브라우저가 실제로 그린다.
fn png(width: u32, height: u32, shade: u8) -> Vec<u8> {
    fn crc(bytes: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for b in bytes {
            c ^= u32::from(*b);
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc(&body).to_be_bytes());
    }
    // 줄마다 거르개 바이트(0) + 회색 값. 가운데에 밝은 띠를 둬 누른 자리를 눈으로 가늠할 수 있게 한다.
    let mut raw = Vec::with_capacity((width as usize + 1) * height as usize);
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            let band = (x / 64 + y / 64) % 2 == 0;
            raw.push(if band {
                shade
            } else {
                shade.saturating_add(24)
            });
        }
    }
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    while let Some(block) = blocks.next() {
        z.push(u8::from(blocks.peek().is_none()));
        z.extend((block.len() as u16).to_le_bytes());
        z.extend((!(block.len() as u16)).to_le_bytes());
        z.extend(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut head = Vec::new();
    head.extend(width.to_be_bytes());
    head.extend(height.to_be_bytes());
    head.extend([8, 0, 0, 0, 0]); // 8비트 회색
    chunk(&mut out, b"IHDR", &head);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// 화면이 묻는 주소에 맞는 시험 데이터로 답한다. 묶음은 `x-fixture-set` 머리(없으면 full)로 고른다.
/// 그림 주소(이모지·스티커, 프로필 그림, 넘겨받기 화면)에는 시험용 그림을 준다(배치 검사가 그림이 있는 모습을 보게).
fn answer(state: &AdminState, headers: &HeaderMap, uri: &Uri) -> Option<Response> {
    let path = uri.path();
    let image = if path.starts_with("/expressions/file/") {
        Some(png(128, 128, 150))
    } else if path.starts_with("/profile/avatar/file/") {
        Some(png(512, 512, 120))
    } else if path.starts_with("/browser/handover/frame/") {
        Some(png(1365, 800, 200))
    } else {
        None
    };
    if let Some(bytes) = image {
        current_session(state, headers)?;
        return Some(([(axum::http::header::CONTENT_TYPE, "image/png")], bytes).into_response());
    }
    if !spa::wants_json(headers) {
        return None;
    }
    let url = uri
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| uri.path().to_owned());
    if let Some((_, _, screen)) = standalone().into_iter().find(|(_, u, _)| *u == url) {
        return Some(screen.respond());
    }
    let set = headers
        .get("x-fixture-set")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("full");
    static CACHE: Mutex<BTreeMap<String, Arc<Vec<FixturePage>>>> = Mutex::new(BTreeMap::new());
    let list = CACHE
        .lock()
        .unwrap()
        .entry(set.to_owned())
        .or_insert_with(|| Arc::new(set_pages(set)))
        .clone();
    let page = list.iter().find(|p| p.url == url)?;
    // 로그인하지 않았으면 처리기에 맡긴다(로그인으로 가라는 답).
    let (_, csrf, flash) = current_session(state, headers)?;
    let mut data = page.data.clone();
    data["csrf"] = json!(csrf);
    data["flash"] = json!(flash);
    let status = StatusCode::from_u16(page.status).unwrap_or(StatusCode::OK);
    Some((status, axum::Json(data)).into_response())
}

/// 실제 라우터를 시험 데이터로 띄워 둔다(브라우저 검사가 끝나면 프로세스를 끝낸다).
/// 비밀번호는 `fixture-password-123`. 쓰기 동작은 임시 저장소에 실제로 간다.
#[tokio::test]
#[ignore]
async fn serve_ui_fixtures() {
    let Ok(addr) = std::env::var("LUCIA_UI_FIXTURE_ADDR") else {
        return;
    };
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::temp_dir().join(format!("lucia-ui-fixture-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.display();
    let cfg = crate::config::Config::parse(&format!(
        "data_dir = \"{d}/data\"\ncredentials_dir = \"{d}/creds\"\n\n[sandbox]\nname = \"lucia\"\nworkdir = \"{d}/ws\"\n\n[admin]\nbind = [\"{addr}\"]\n"
    ))
    .unwrap();
    // `LUCIA_UI_REAL_DB`에 저장소 **사본**의 경로를 주면 시험 데이터 대신 그 데이터를 실제 처리기로 보인다
    // (배포 전에 실제 데이터로 화면을 그려 본다. 사본은 마이그레이션되므로 운영 저장소를 직접 주지 않는다).
    let real = std::env::var("LUCIA_UI_REAL_DB").ok();
    let store = Arc::new(Mutex::new(match &real {
        Some(path) => lucia_store::Store::open(std::path::Path::new(path)).unwrap(),
        None => lucia_store::Store::open_in_memory().unwrap(),
    }));
    let creds = Arc::new(crate::credentials::Credentials::new(
        cfg.credentials_dir.clone(),
        crate::mask::global().clone(),
    ));
    let (inbound, _inbound_rx) = tokio::sync::mpsc::channel(1);
    let (presses, _presses_rx) = tokio::sync::mpsc::channel(1);
    let discord =
        crate::discord_ctl::DiscordControl::new(store.clone(), creds.clone(), inbound, presses);
    let google = crate::google_auth::GoogleAuth::new(creds.clone(), reqwest::Client::new());
    let (jobs_tx, _jobs) = tokio::sync::mpsc::channel(64);
    let hash = super::hash_password("fixture-password-123").unwrap();
    store
        .lock()
        .unwrap()
        .set_admin_password_hash(&hash, chrono::Utc::now())
        .unwrap();
    let profile_store = store.clone();
    let state = AdminState::new(store, discord, google, creds, Arc::new(cfg), jobs_tx);
    state
        .store_checks(vec![crate::check::Check {
            name: "저장소",
            ok: true,
            detail: "정상".into(),
        }])
        .await;
    if real.is_none() {
        let _ = state.fixture.set(Box::new(answer));
        // 시험 서버는 초기 설정 안내를 끈 채 시작한다(저장소가 비어 있어 새 설치로 보이므로, 끄지 않으면 로그인 뒤 첫 화면이
        // 초기 설정이 된다). 그 흐름은 화면 검사의 흐름 시험(`e2e/flows.mjs` 8d)이 켜고 끈다.
        let _ = state
            .store()
            .set_setting(setup::DISMISSED_KEY, "1", chrono::Utc::now());
        // 프로필 그림 올리기가 실제로 되게 한다(앱도 Discord도 없이. 그림은 임시 디렉터리에 둔다).
        state.set_profile(crate::profile::ProfileAccess {
            profile: crate::profile::Profile::new(profile_store.clone(), dir.join("profile")),
            discord: None,
            files: Arc::new(crate::files::FileStore::new(
                dir.join("files"),
                reqwest::Client::new(),
                profile_store,
            )),
        });
    }
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    println!("화면 검사용 서버: http://{addr} (비밀번호 fixture-password-123)");
    let app = super::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, app).await.unwrap();
}
