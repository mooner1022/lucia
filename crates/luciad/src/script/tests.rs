//! 스크립트 실행기 시험: 샌드박스 대신 호스트 python3로 같은 실행기를 돌린다(통로·효과·한도).
//! 실제 샌드박스는 `luciad script-check`로 본다.

use std::sync::Mutex;

use super::*;
use crate::tools::{Grade, Group};

pub(crate) struct LocalLauncher;

impl Launcher for LocalLauncher {
    fn command(&self, tag: &str, _timeout: Duration) -> Command {
        let mut c = Command::new("python3");
        c.args(["-I", "-u", "-c", HARNESS, tag]);
        c
    }

    fn kill<'a>(&'a self, _tag: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async {})
    }
}

/// 도구 이름별로 정해 둔 글을 돌려주고, 부른 것을 적어 둔다.
#[derive(Default)]
pub(crate) struct FakeBridge {
    pub calls: Arc<Mutex<Vec<(String, Value)>>>,
    pub answers: Vec<(&'static str, String)>,
}

struct FakeSession {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    answers: Vec<(&'static str, String)>,
}

impl ToolBridge for FakeBridge {
    fn session(&self, _run: String, _grants: Policy) -> Box<dyn ToolSession> {
        Box::new(FakeSession {
            calls: self.calls.clone(),
            answers: self.answers.clone(),
        })
    }
}

impl ToolSession for FakeSession {
    fn call<'a>(
        &'a mut self,
        name: &'a str,
        args: Map<String, Value>,
    ) -> BoxFut<'a, Result<String, String>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_owned(), Value::Object(args)));
            Ok(self
                .answers
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| format!("{name} 결과")))
        })
    }

    fn close(&mut self) -> BoxFut<'_, ()> {
        Box::pin(async {})
    }
}

struct FakeFetch;

impl crate::watch::Fetch for FakeFetch {
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<String, String>> {
        Box::pin(async move { Ok(format!("<html>{url}</html>")) })
    }
}

pub(crate) fn have_python() -> bool {
    std::process::Command::new("python3")
        .arg("-c")
        .arg("pass")
        .status()
        .is_ok_and(|s| s.success())
}

fn runner(bridge: FakeBridge) -> ScriptRunner {
    ScriptRunner::new(
        Arc::new(LocalLauncher),
        Arc::new(bridge),
        Some(Arc::new(FakeFetch)),
    )
}

fn chat_grants() -> Policy {
    Policy::new(
        &[
            Group::Report,
            Group::Memory,
            Group::Google,
            Group::Code,
            Group::Followups,
        ],
        Grade::ExternalWrite,
        true,
    )
    .for_script()
}

fn job(code: &str) -> ScriptJob {
    ScriptJob {
        code: code.to_owned(),
        input: json!({"who": "루시아"}),
        state: json!({"seen": ["a"]}),
        grants: chat_grants(),
        run: "chat-1".into(),
        declared: None,
        fetch: false,
        fetch_hosts: None,
        session_hosts: None,
        files: false,
        net: false,
        timeout: Duration::from_secs(20),
    }
}

#[tokio::test]
async fn code_calls_tools_through_the_bridge_and_returns_effects() {
    if !have_python() {
        eprintln!("python3가 없어 건너뜀");
        return;
    }
    let bridge = FakeBridge {
        answers: vec![
            (
                "mail_search",
                r#"[{"id":"m1","subject":"과제 마감 변경","from":"교수"},{"id":"m2","subject":"광고","from":"쇼핑"}]"#.into(),
            ),
            (
                "memory_read",
                crate::mcp::external("memory", "선호: <라떼> & 녹차"),
            ),
        ],
        ..FakeBridge::default()
    };
    let calls = bridge.calls.clone();
    let r = runner(bridge);
    let out = r
        .run(job(r#"
import os
mails = lucia.mail_search("newer_than:1d", limit=5)
hits = [m for m in mails if "과제" in m["subject"] and m["id"] not in lucia.state["seen"]]
print("찾음", len(hits), lucia.input["who"])
os.write(1, b"fd1 noise\n")  # 통로를 더럽히지 않는다
note = lucia.call("memory_read", name="drinks")
lucia.state["seen"] += [m["id"] for m in hits]
for m in hits:
    lucia.notify("새 과제 메일: " + m["subject"])
lucia.next_run(30)
lucia.result({"hits": len(hits), "note": note})
"#))
        .await;
    assert!(out.ok, "{:?}", out.error);
    assert!(out.printed.contains("찾음 1 루시아"), "{}", out.printed);
    assert!(!out.printed.contains("fd1 noise"));
    let result = out.result.unwrap();
    assert_eq!(result["hits"], 1);
    assert_eq!(
        result["note"], "선호: <라떼> & 녹차",
        "외부 데이터 표지를 벗긴다"
    );
    assert_eq!(out.state.unwrap()["seen"], json!(["a", "m1"]));
    assert_eq!(
        out.effects.notify,
        vec![("새 과제 메일: 과제 마감 변경".to_owned(), false)]
    );
    assert_eq!(out.effects.next_minutes, Some(30.0));
    assert_eq!(out.calls, vec!["mail_search", "memory_read"]);
    let calls = calls.lock().unwrap();
    assert_eq!(calls[0].1, json!({"query": "newer_than:1d", "limit": 5}));
}

#[tokio::test]
async fn refused_tools_raise_and_failures_keep_no_state() {
    if !have_python() {
        return;
    }
    let bridge = FakeBridge::default();
    let calls = bridge.calls.clone();
    let r = runner(bridge);
    let out = r
        .run(job(r#"
errors = []
for name in ["report_result", "followup_create", "browser_open", "routine_create"]:
    try:
        lucia.call(name, x=1)
    except lucia.ToolError as e:
        errors.append(str(e))
try:
    lucia.fetch("https://example.com/")
except lucia.ToolError as e:
    errors.append(str(e))
print(len(errors))
lucia.state["x"] = 1
raise ValueError("일부러")
"#))
        .await;
    assert!(!out.ok);
    assert!(out.printed.contains('5'), "{}", out.printed);
    let err = out.error.unwrap();
    assert!(
        err.contains("ValueError: 일부러") && err.contains("line 14"),
        "{err}"
    );
    assert!(out.state.is_none(), "실패하면 상태를 남기지 않는다");
    assert!(
        calls.lock().unwrap().is_empty(),
        "막힌 도구는 다리로 가지 않는다"
    );

    // 선언한 도구만, fetch는 허락이 있을 때만.
    let mut j = job(r#"
lucia.call("mail_read", id="m1")
page = lucia.fetch("https://example.com/a")
try:
    lucia.call("calendar_events")
except lucia.ToolError as e:
    print("막힘:", e)
result = page
"#);
    j.declared = Some(vec!["mail_read".into()]);
    j.fetch = true;
    let out = runner(FakeBridge {
        answers: vec![("mail_read", r#"{"id":"m1"}"#.into())],
        ..FakeBridge::default()
    })
    .run(j)
    .await;
    assert!(out.ok, "{:?}", out.error);
    assert!(out.printed.contains("선언하지 않은 도구 calendar_events"));
    assert_eq!(
        out.result,
        Some(json!("<html>https://example.com/a</html>"))
    );
    assert_eq!(out.calls, vec!["mail_read", "fetch example.com"]);
}

/// 사람 없는 실행의 코드는 사용자가 위임한 사이트만 가져온다(결정 0029). 하위 도메인도 덮는다.
#[tokio::test]
async fn unattended_code_fetches_only_granted_sites() {
    if !have_python() {
        return;
    }
    let mut j = job(r#"
out = []
for u in ["https://www.example.com/a", "https://evil.example/x?d=1"]:
    try:
        lucia.fetch(u)
        out.append("ok")
    except lucia.ToolError as e:
        out.append("막힘" if "위임한 사이트가 아니라" in str(e) else str(e))
result = out
"#);
    j.fetch = true;
    j.fetch_hosts = Some(vec!["example.com".into()]);
    let out = runner(FakeBridge::default()).run(j).await;
    assert!(out.ok, "{:?}", out.error);
    assert_eq!(out.result, Some(json!(["ok", "막힘"])));
    assert_eq!(out.calls, vec!["fetch www.example.com"]);
}

/// 결정 0047: 스크립트가 파일을 bytes로 읽고, 고쳐서 새 파일로 남긴다. 허락이 없는 스크립트는 못 한다.
#[tokio::test]
async fn code_reads_and_saves_files_as_bytes() {
    if !have_python() {
        eprintln!("python3가 없어 건너뜀");
        return;
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::temp_dir().join(format!(
        "lucia-script-files-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = Arc::new(Mutex::new(lucia_store::Store::open_in_memory().unwrap()));
    let files = Arc::new(crate::files::FileStore::new(
        dir.clone(),
        reqwest::Client::new(),
        store,
    ));
    // 사용자가 붙인 그림(바이너리).
    let png = crate::expressions::testing::png(512);
    let source = files
        .save("in", "사진.png", &png, Some("app:cl-1"), chrono::Utc::now())
        .unwrap();
    let r = runner(FakeBridge::default()).with_files(Some(files.clone()));
    let code = r#"
info = lucia.file_info(lucia.input["id"])
data = lucia.read_file(lucia.input["id"])
assert isinstance(data, bytes) and len(data) == info["size"]
small = data[:24] + bytes([0xAB])        # 바이트를 그대로 다룬다
new_id = lucia.save_file("줄인/그림.png", small)
note_id = lucia.save_file("메모.txt", "글도 된다")
try:
    lucia.read_file("f_000000000000")
except lucia.ToolError as e:
    print("없음:", e)
lucia.result({"name": info["name"], "kind": info["kind"], "new": new_id, "note": note_id})
"#;
    let mut with_files = job(code);
    with_files.files = true;
    with_files.input = json!({"id": source.id});
    let out = r.run(with_files).await;
    assert!(out.ok, "{:?}", out.error);
    let result = out.result.clone().unwrap();
    assert_eq!(
        (result["name"].as_str(), result["kind"].as_str()),
        (Some("사진.png"), Some("image"))
    );
    assert!(out.printed.contains("없음: 그런 파일이 없다"));
    // 만든 파일은 보관소에 남고(이름은 다듬는다), 결과 글에 id가 보인다.
    assert_eq!(out.files.len(), 2);
    let made = files.get(result["new"].as_str().unwrap()).unwrap();
    assert_eq!(
        (
            made.name.as_str(),
            made.kind.as_str(),
            made.direction.as_str()
        ),
        ("그림.png", "image", "out")
    );
    assert_eq!(made.run_id.as_deref(), Some("chat-1"));
    let mut expect = png[..24].to_vec();
    expect.push(0xAB);
    assert_eq!(files.read(&made).unwrap(), expect);
    let note = files.get(result["note"].as_str().unwrap()).unwrap();
    assert_eq!(files.read(&note).unwrap(), "글도 된다".as_bytes());
    let text = out.text(4000);
    assert!(
        text.contains("[만든 파일]") && text.contains(&format!("id={}", made.id)),
        "{text}"
    );
    assert!(
        out.calls.iter().any(|c| c.starts_with("file_read"))
            && out.calls.iter().any(|c| c == "file_save 그림.png")
    );
    // 허락이 없는 스크립트(루틴 등)는 읽지도 만들지도 못한다.
    let mut closed = job("lucia.read_file(lucia.input['id'])");
    closed.input = json!({"id": source.id});
    let out = r.run(closed).await;
    assert!(
        !out.ok
            && out
                .error
                .as_deref()
                .unwrap_or("")
                .contains("파일을 읽거나 만들 수 없다"),
        "{:?}",
        out.error
    );
    // 너무 큰 파일은 만들지 않는다. 없는 패키지는 설치하라고 알려 준다.
    let mut big = job("lucia.save_file('big.bin', bytes(31 * 1024 * 1024))");
    big.files = true;
    let out = r.run(big).await;
    assert!(
        !out.ok && out.error.as_deref().unwrap_or("").contains("너무 크다"),
        "{:?}",
        out.error
    );
    let out = r.run(job("import no_such_package_xyz")).await;
    assert!(
        out.text(4000).contains("lib_install로 설치"),
        "{}",
        out.text(4000)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn json_tools_that_answer_in_words_raise() {
    if !have_python() {
        return;
    }
    let bridge = FakeBridge {
        answers: vec![("mail_search", "Google이 연결되지 않았다.".into())],
        ..FakeBridge::default()
    };
    let out = runner(bridge)
        .run(job(r#"
try:
    lucia.mail_search("x")
except lucia.ToolError as e:
    result = "오류: " + str(e)
"#))
        .await;
    assert!(out.ok, "{:?}", out.error);
    assert_eq!(out.result, Some(json!("오류: Google이 연결되지 않았다.")));
}

#[tokio::test]
async fn runaway_code_is_stopped() {
    if !have_python() {
        return;
    }
    let mut j = job("while True:\n    pass\n");
    j.timeout = Duration::from_millis(500);
    let started = Instant::now();
    let out = runner(FakeBridge::default()).run(j).await;
    assert!(!out.ok);
    assert!(out.error.unwrap().contains("시간 초과"));
    assert!(started.elapsed() < Duration::from_secs(15));
}

#[test]
fn outcome_text_is_short_and_shows_effects() {
    let out = ScriptOutcome {
        ok: true,
        printed: "가".repeat(5000),
        result: Some(json!({"n": 1})),
        effects: Effects {
            notify: vec![("알림".into(), true)],
            wake: Some(("확인".into(), Value::Null)),
            ..Effects::default()
        },
        ..ScriptOutcome::default()
    };
    let text = out.text(8000);
    assert!(text.len() < 8100);
    assert!(text.contains("[출력]") && text.contains("길어서 잘림"));
    let small = ScriptOutcome {
        ok: true,
        result: Some(json!({"n": 1})),
        effects: out.effects.clone(),
        ..ScriptOutcome::default()
    }
    .text(8000);
    assert!(small.contains("- 알림(묶음): 알림") && small.contains("- Lucia 깨우기: 확인"));
    assert_eq!(
        unwrap_external(&crate::mcp::external("x", "a < b")),
        "a < b"
    );
    assert_eq!(unwrap_external("그냥 글"), "그냥 글");
}

#[test]
fn script_policy_never_widens() {
    let chat = Policy::new(
        &[
            Group::Report,
            Group::Memory,
            Group::Browser,
            Group::Code,
            Group::Routines,
            Group::Followups,
        ],
        Grade::ExternalWrite,
        true,
    );
    let s = chat.for_script();
    assert!(s.script && !s.human);
    assert!(s.allows("memory_read") && s.allows("browser_open"));
    for name in [
        "report_result",
        "code_run",
        "routine_create",
        "followup_create",
        "memory_delete",
    ] {
        assert!(!s.allows(name), "{name}");
    }
    // 로그인과 위임 제안은 이름으로 막는다(결정 0028 I1).
    for name in ["browser_login", "site_grant_propose"] {
        assert!(super::never_from_script(name), "{name}");
    }
    let background = Policy::new(&[Group::Memory, Group::Google], Grade::LocalWrite, false);
    assert!(!background.for_script().allows("browser_open"));
}

fn access(store: Arc<Mutex<lucia_store::Store>>, bridge: FakeBridge) -> tools::ScriptAccess {
    tools::ScriptAccess {
        runner: Some(Arc::new(runner(bridge))),
        store,
        posts: None,
        tz: chrono_tz::Asia::Seoul,
        chat_timeout: Duration::from_secs(20),
        routine_timeout: Duration::from_secs(20),
        packages: None,
    }
}

fn create(name: &str) -> tools::RoutineCreate {
    tools::RoutineCreate {
        name: name.into(),
        task: "새 과제 메일을 요약해 알린다".into(),
        cron: None,
        every_minutes: None,
        between: None,
        on_change: None,
        script: None,
        tools: None,
        input: None,
        until: None,
    }
}

/// 루틴 만들기(결정 0025): 간격 하한, 시험 실행, 쓰기 도구 거절, 자료를 가져가는 스크립트만 승인, 기준 상태.
#[tokio::test]
async fn routine_create_checks_trials_and_reviews_leaky_scripts() {
    if !have_python() {
        return;
    }
    let store = Arc::new(Mutex::new(lucia_store::Store::open_in_memory().unwrap()));
    let a = access(
        store.clone(),
        FakeBridge {
            answers: vec![("mail_search", r#"[{"id":"m1"}]"#.into())],
            ..FakeBridge::default()
        },
    );
    let count = || store.lock().unwrap().routines().unwrap().len();

    let mut c = create("매번 깨우기");
    c.every_minutes = Some(30);
    let out = a.routine_create("chat-1", c).await;
    assert!(out.contains("60~"), "{out}");

    let mut c = create("깨진 스크립트");
    c.cron = Some("0 9 * * *".into());
    c.script = Some("raise ValueError('없는 칸')".into());
    let out = a.routine_create("chat-1", c).await;
    assert!(
        out.contains("시험 실행이 실패") && out.contains("없는 칸"),
        "{out}"
    );

    let mut c = create("쓰기");
    c.cron = Some("0 9 * * *".into());
    c.script = Some("pass".into());
    c.tools = Some(vec!["mail_send".into()]);
    assert!(a.routine_create("chat-1", c).await.contains("쓸 수 없다"));
    assert_eq!(count(), 0);

    let mut c = create("자료를 밖으로");
    c.every_minutes = Some(30);
    c.script = Some("lucia.mail_search('x')".into());
    c.tools = Some(vec!["mail_search".into(), "fetch".into()]);
    let out = a.routine_create("chat-1", c).await;
    assert!(out.contains("카드에서 켜야"), "{out}");
    let review = store.lock().unwrap().routines().unwrap()[0].clone();
    assert_eq!(review.status, lucia_store::RoutineStatus::Review);
    assert!(
        store
            .lock()
            .unwrap()
            .schedule(&review.id)
            .unwrap()
            .is_none()
    );
    // 승인 대기는 고치거나 resume해도 이 도구로는 켜지지 않는다(카드·관리 페이지만).
    let still = tools::RoutineUpdate {
        id: review.id.clone(),
        state: Some("resume".into()),
        name: Some("이름만 바꿈".into()),
        task: None,
        cron: None,
        every_minutes: None,
        between: None,
        on_change: None,
        script: None,
        tools: None,
        input: None,
        until: None,
    };
    let out = a.routine_update("chat-1", still).await;
    assert!(out.contains("카드에서 켜야"), "{out}");
    assert_eq!(
        store
            .lock()
            .unwrap()
            .routine(&review.id)
            .unwrap()
            .unwrap()
            .status,
        lucia_store::RoutineStatus::Review
    );
    // 새 메일 변화로 도는 루틴은 변화 목록이 사용자 자료이므로 fetch만 있어도 승인.
    let mut c = create("새 메일과 바깥");
    c.on_change = Some("gmail".into());
    c.script = Some("pass".into());
    c.tools = Some(vec!["fetch".into()]);
    assert!(
        a.routine_create("chat-1", c)
            .await
            .contains("카드에서 켜야")
    );

    let mut c = create("과제 메일");
    c.every_minutes = Some(10);
    c.between = Some("09:00-21:00".into());
    c.script = Some(
        "ms = lucia.mail_search('label:과제')\nlucia.state['seen'] = [m['id'] for m in ms]\nlucia.notify('시험')".into(),
    );
    c.tools = Some(vec!["mail_search".into()]);
    let out = a.routine_create("chat-1", c).await;
    assert!(
        out.contains("만들고 켰다") && out.contains("- 알림: 시험"),
        "{out}"
    );
    let id = out
        .split_whitespace()
        .find(|w| w.starts_with("routine-"))
        .unwrap()
        .trim_end_matches("을(를)")
        .to_owned();
    {
        let s = store.lock().unwrap();
        assert_eq!(
            s.script_state(&format!("routine:{id}")).unwrap(),
            Some(json!({"seen": ["m1"]})),
            "시험 실행의 상태가 기준선"
        );
        assert!(s.schedule(&id).unwrap().unwrap().enabled);
        assert!(
            s.pending_notices().unwrap().is_empty(),
            "시험 실행은 알리지 않는다"
        );
    }
    assert!(a.routine_list().contains("과제 메일"));

    let pause = tools::RoutineUpdate {
        id: id.clone(),
        state: Some("pause".into()),
        name: None,
        task: None,
        cron: None,
        every_minutes: None,
        between: None,
        on_change: None,
        script: None,
        tools: None,
        input: None,
        until: None,
    };
    assert!(
        a.routine_update("chat-2", pause.clone())
            .await
            .contains("멈춤")
    );
    assert!(
        !store
            .lock()
            .unwrap()
            .schedule(&id)
            .unwrap()
            .unwrap()
            .enabled
    );
    let resume = tools::RoutineUpdate {
        state: Some("resume".into()),
        ..pause
    };
    assert!(a.routine_update("chat-2", resume).await.contains("켜짐"));
    assert!(
        store
            .lock()
            .unwrap()
            .schedule(&id)
            .unwrap()
            .unwrap()
            .enabled
    );

    let del = tools::RoutineDelete {
        id: id.clone(),
        reason: "그만".into(),
    };
    assert!(a.routine_delete("chat-3", del).contains("지웠다"));
    assert!(store.lock().unwrap().routine(&id).unwrap().is_none());
}

/// 스킬 스크립트(결정 0025): 선언한 도구로 돌고, 상태와 쓴 기록을 남긴다.
#[tokio::test]
async fn skill_scripts_run_with_declared_tools_and_keep_state() {
    if !have_python() {
        return;
    }
    let store = Arc::new(Mutex::new(lucia_store::Store::open_in_memory().unwrap()));
    let tools_list = vec!["mail_search".to_owned()];
    let fp = json!({"tools": [], "hosts": []});
    for (slug, script) in [
        (
            "unread-count",
            Some((
                "n = len(lucia.mail_search(lucia.input.get('q', 'is:unread')))\nlucia.state['last'] = n\nresult = n",
                tools_list.as_slice(),
            )),
        ),
        ("plain", None),
    ] {
        store
            .lock()
            .unwrap()
            .save_skill(
                &lucia_store::SkillWrite {
                    slug,
                    title: "안 읽은 메일 수",
                    description: "안 읽은 메일 수를 셀 때",
                    body: "1. 센다",
                    script,
                },
                None,
                &lucia_store::SkillOrigin {
                    status: lucia_store::SkillStatus::Active,
                    source: "chat",
                    task_id: None,
                    footprint: &fp,
                },
                "agent",
                "처음",
                chrono::Utc::now(),
            )
            .unwrap()
            .unwrap();
    }
    let a = access(
        store.clone(),
        FakeBridge {
            answers: vec![("mail_search", r#"[{"id":"1"},{"id":"2"}]"#.into())],
            ..FakeBridge::default()
        },
    );
    let out = a
        .skill_run(
            "task-7-r1",
            chat_grants(),
            tools::SkillRun {
                name: "[[unread-count]]".into(),
                input: Some(json!({"q": "is:unread"})),
            },
        )
        .await;
    assert!(out.contains("[결과]\n2"), "{out}");
    {
        let s = store.lock().unwrap();
        assert_eq!(
            s.script_state("skill:unread-count").unwrap(),
            Some(json!({"last": 2}))
        );
        assert_eq!(s.skill("unread-count").unwrap().unwrap().uses, 1);
        assert_eq!(
            s.skill("unread-count").unwrap().unwrap().script_tools,
            Some(tools_list.clone())
        );
    }
    let plain = a
        .skill_run(
            "chat-1",
            chat_grants(),
            tools::SkillRun {
                name: "plain".into(),
                input: None,
            },
        )
        .await;
    assert!(plain.contains("스크립트가 없다"));
}

/// 목표 점검이 만든 루틴(결정 0032): 스크립트 도구는 점검이 쓸 수 있는 것만, 가져오기는 사이트 위임만.
/// 사람이 만든 루틴은 예전대로(만들 때 정한 가져오기).
#[tokio::test]
async fn review_made_routines_are_bounded_like_their_creator() {
    let store = Arc::new(Mutex::new(lucia_store::Store::open_in_memory().unwrap()));
    let a = tools::ScriptAccess {
        runner: None,
        store: store.clone(),
        posts: None,
        tz: chrono_tz::Asia::Seoul,
        chat_timeout: Duration::from_secs(20),
        routine_timeout: Duration::from_secs(20),
        packages: None,
    };
    crate::runctx::register(
        "goal-bound-1",
        crate::runctx::RunCtx {
            goal: Some("goal-x".into()),
            goal_review: true,
            ..Default::default()
        },
    );
    let mut c = create("디스코드 훑기");
    c.cron = Some("0 9 * * *".into());
    c.script = Some("pass".into());
    c.tools = Some(vec!["discord_messages".into()]);
    let out = a.routine_create("goal-bound-1", c).await;
    assert!(out.contains("점검이 쓸 수 있는 도구만"), "{out}");

    let tools = vec!["fetch".to_owned()];
    {
        let s = store.lock().unwrap();
        s.grant_site(
            &lucia_store::NewSiteGrant {
                id: "g1",
                host: "learning.example.ac.kr",
                login: false,
                actions: false,
                purpose: "과제",
                origin: "admin",
                expires_at: None,
            },
            chrono::Utc::now(),
        )
        .unwrap();
    }
    let job = a.routine_job(
        "pass",
        &tools,
        json!({}),
        json!({}),
        "goal-bound-1",
        Duration::from_secs(5),
    );
    assert_eq!(
        job.fetch_hosts,
        Some(vec!["learning.example.ac.kr".to_owned()]),
        "점검이 만드는 중"
    );
    let job = a.routine_job(
        "pass",
        &tools,
        json!({}),
        json!({}),
        "chat-1",
        Duration::from_secs(5),
    );
    assert_eq!(job.fetch_hosts, None, "사람이 만든 루틴은 예전대로");
    {
        let s = store.lock().unwrap();
        s.link_goal_child("goal-x", "routine", "routine-r", true, chrono::Utc::now())
            .unwrap();
    }
    let job = a.routine_job_for(
        Some("routine-r"),
        "pass",
        &tools,
        json!({}),
        json!({}),
        "routine-r-1234",
        Duration::from_secs(5),
    );
    assert!(job.fetch_hosts.is_some(), "점검이 만든 루틴이 돌 때");
}

/// 권한 모드 엄격(결정 0040): 스크립트 없는 루틴도 승인 대기로 만든다(기본이면 바로 켠다).
#[tokio::test]
async fn strict_mode_puts_every_new_routine_up_for_review() {
    let store = Arc::new(Mutex::new(lucia_store::Store::open_in_memory().unwrap()));
    let a = access(store.clone(), FakeBridge::default());
    let mut c = create("매일 확인");
    c.cron = Some("0 9 * * *".into());
    a.routine_create("chat-1", c.clone()).await;
    assert_eq!(
        store.lock().unwrap().routines().unwrap()[0].status,
        lucia_store::RoutineStatus::Active,
        "기본이면 바로 켠다"
    );
    crate::security::set_mode(
        &store.lock().unwrap(),
        crate::security::Mode::Strict,
        "시험",
        chrono::Utc::now(),
    )
    .unwrap();
    c.name = "엄격 확인".into();
    let out = a.routine_create("chat-1", c).await;
    assert!(out.contains("카드에서 켜야"), "{out}");
    let strict = store
        .lock()
        .unwrap()
        .routines()
        .unwrap()
        .into_iter()
        .find(|r| r.name == "엄격 확인")
        .unwrap();
    assert_eq!(strict.status, lucia_store::RoutineStatus::Review);
}

/// 결정 0057: 스크립트가 웹의 파일을 받아(`lucia.download`) 바로 읽는다. 큰 파일은 조각으로 넘어온다.
/// 나갈 수 있는 곳은 `lucia.fetch`와 같다(브라우저를 쓸 수 있는 실행, 사람 없는 실행은 위임한 사이트만,
/// 로그인 세션은 위임한 사이트만). 파일을 다룰 수 없는 스크립트(루틴)는 받지 못한다.
#[tokio::test]
async fn code_downloads_files_and_reads_big_ones_in_chunks() {
    use crate::downloads::tests::file;
    use crate::watch::Via;
    if !have_python() {
        eprintln!("python3가 없어 건너뜀");
        return;
    }
    let d = crate::downloads::tests::setup();
    let zip = "https://files.example.com/pack.zip";
    // 조각 하나(4 MB)를 넘는 파일: 세 조각으로 온다.
    let mut body = b"PK\x03\x04".to_vec();
    body.extend((0..9 * 1024 * 1024u32).map(|i| (i % 251) as u8));
    d.fetch.answer(zip, Ok(file(&body, "application/zip", zip)));
    let r = ScriptRunner::new(
        Arc::new(LocalLauncher),
        Arc::new(FakeBridge::default()),
        Some(d.fetch.clone()),
    )
    .with_files(Some(d.files.clone()))
    .with_downloads(Some(d.downloads.clone()));
    let code = r#"
import hashlib
fid = lucia.download(lucia.input["url"])
data = lucia.read_file(fid)
info = lucia.file_info(fid)
lucia.result({"id": fid, "len": len(data), "sha": hashlib.sha256(data).hexdigest(), "name": info["name"]})
"#;
    let open = |code: &str| {
        let mut j = job(code);
        j.files = true;
        j.fetch = true;
        j.input = json!({"url": zip});
        j
    };
    let out = r.run(open(code)).await;
    assert!(out.ok, "{:?}", out.error);
    let result = out.result.clone().unwrap();
    let saved = d.files.get(result["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        (
            saved.name.as_str(),
            saved.direction.as_str(),
            saved.sha256.as_str()
        ),
        ("pack.zip", "web", result["sha"].as_str().unwrap())
    );
    assert_eq!(result["len"], json!(body.len()));
    assert_eq!(saved.run_id.as_deref(), Some("chat-1"));
    // 부른 것으로는 받기 한 번과 읽기 한 번만 센다(조각마다 세지 않는다).
    assert_eq!(
        out.calls,
        vec![
            "download files.example.com".to_owned(),
            format!("file_read {}", saved.id),
            format!("file_info {}", saved.id)
        ]
    );
    assert!(out.text(4000).contains("웹에서 받음"), "{}", out.text(4000));
    assert_eq!(
        d.fetch.called.lock().unwrap()[0],
        (zip.to_owned(), Via::Plain)
    );

    let fails = |out: &ScriptOutcome, why: &str| {
        assert!(
            !out.ok && out.error.as_deref().unwrap_or("").contains(why),
            "{why}: {:?}",
            out.error
        );
    };
    let get = "lucia.download(lucia.input['url'])";
    // 바깥으로 나갈 수 없는 스크립트, 파일을 다룰 수 없는 스크립트(루틴)는 받지 못한다.
    let mut no_fetch = open(get);
    no_fetch.fetch = false;
    fails(&r.run(no_fetch).await, "fetch를 쓸 수 없다");
    let mut no_files = open(get);
    no_files.files = false;
    fails(&r.run(no_files).await, "파일을 받을 수 없다");
    // 사람 없는 실행은 위임한 사이트만.
    let mut bounded = open(get);
    bounded.fetch_hosts = Some(vec!["other.example".into()]);
    fails(&r.run(bounded).await, "위임한 사이트가 아니라");
    let mut granted = open(get);
    granted.fetch_hosts = Some(vec!["example.com".into()]);
    assert!(r.run(granted).await.ok);
    // 로그인 세션은 위임한 사이트만.
    let with_session = "lucia.download(lucia.input['url'], session=True, name='내 자료.zip')";
    fails(
        &r.run(open(with_session)).await,
        "로그인 세션으로 가져올 수 없다",
    );
    let mut other = open(with_session);
    other.session_hosts = Some(vec!["other.example".into()]);
    fails(&r.run(other).await, "위임한 사이트가 아니라");
    let mut session = open(with_session);
    session.session_hosts = Some(vec!["example.com".into()]);
    let out = r.run(session).await;
    assert!(out.ok, "{:?}", out.error);
    assert_eq!(out.files[0].name, "내 자료.zip");
    assert_eq!(
        out.calls,
        vec!["download_session files.example.com".to_owned()]
    );
    assert_eq!(
        d.fetch.called.lock().unwrap().last().unwrap().1,
        Via::Session
    );
    // 가져오기에 실패하면 까닭이 스크립트의 예외로 온다.
    let mut missing = open(get);
    missing.input = json!({"url": "https://files.example.com/none.zip"});
    fails(&r.run(missing).await, "404");
}

/// 결정 0058: 코드가 바깥에 직접 나가는 통행증은 사람이 대화 중인 `code_run`에만 준다. 실행이 끝나면 거둔다.
/// 사람 없는 실행, 나가는 글 검사가 "막기"이거나 권한 모드가 엄격일 때는 주지 않는다.
#[tokio::test]
async fn only_code_run_in_a_human_chat_gets_a_pass_to_the_internet() {
    if !have_python() {
        eprintln!("python3가 없어 건너뜀");
        return;
    }
    let net = crate::script::net::tests::setup().await;
    let daemon = net.addr.to_string();
    let store = Arc::new(Mutex::new(lucia_store::Store::open_in_memory().unwrap()));
    let a = tools::ScriptAccess {
        runner: Some(Arc::new(
            ScriptRunner::new(
                Arc::new(LocalLauncher),
                Arc::new(FakeBridge::default()),
                Some(Arc::new(FakeFetch)),
            )
            .with_net(Some((net.net.clone(), daemon.clone()))),
        )),
        ..access(store.clone(), FakeBridge::default())
    };
    // 통행증이 든 프록시 주소로 실제로 한 번 나가 본다(가짜 원 서버가 답한다).
    let code = r#"
import os, urllib.request
proxy = os.environ.get("HTTPS_PROXY", "")
status = None
if lucia.online:
    status = urllib.request.urlopen("http://files.example.org/data.json", timeout=10).status
lucia.result({"online": lucia.online, "status": status, "proxy": proxy.split("@")[-1] if lucia.online else proxy,
              "lower": os.environ.get("https_proxy", "") == proxy, "no_proxy": os.environ.get("NO_PROXY", "")})
"#;
    let run = |grants: Policy| {
        let a = &a;
        async move {
            a.code_run_full(
                "chat-1",
                grants,
                tools::CodeRun {
                    code: code.to_owned(),
                    input: None,
                },
            )
            .await
            .0
        }
    };
    let human = Policy::new(
        &[Group::Report, Group::Code, Group::Browser, Group::Files],
        Grade::ExternalWrite,
        true,
    );
    let out = run(human).await;
    assert!(
        out.contains("\"online\": true")
            && out.contains("\"status\": 200")
            && out.contains(&daemon),
        "{out}"
    );
    assert!(
        out.contains("\"lower\": true") && out.contains("127.0.0.1"),
        "{out}"
    );
    let logged = store.lock().unwrap().recent_activity(3).unwrap();
    assert!(
        logged[0].kind == "code_run" && logged[0].summary.contains("인터넷: files.example.org"),
        "{:?}",
        logged[0]
    );
    assert!(
        !logged[0].summary.contains("lucia:"),
        "통행증은 기록에 남지 않는다"
    );
    // 사람 없는 실행(후속 조치·브리핑·작업)과 브라우저를 쓸 수 없는 실행은 받지 못한다.
    let unattended = Policy::new(
        &[Group::Report, Group::Code, Group::Browser, Group::Files],
        Grade::ExternalWrite,
        false,
    );
    assert!(run(unattended).await.contains("\"online\": false"));
    let no_browser = Policy::new(&[Group::Report, Group::Code], Grade::ExternalWrite, true);
    assert!(run(no_browser).await.contains("\"online\": false"));
    // 프록시는 TLS 안을 보지 못한다: 나가는 글 검사가 "막기"이거나 권한 모드가 엄격이면 열지 않는다.
    store
        .lock()
        .unwrap()
        .set_setting(crate::leak::MODE_KEY, "block", chrono::Utc::now())
        .unwrap();
    assert!(run(human).await.contains("\"online\": false"));
    store
        .lock()
        .unwrap()
        .delete_setting(crate::leak::MODE_KEY)
        .unwrap();
    crate::security::set_mode(
        &store.lock().unwrap(),
        crate::security::Mode::Strict,
        "시험",
        chrono::Utc::now(),
    )
    .unwrap();
    assert!(run(human).await.contains("\"online\": false"));
    // 루틴과 스킬 스크립트는 통행증을 청하지 않는다.
    assert!(
        !a.routine_job(
            "result = 1",
            &[],
            json!({}),
            json!({}),
            "r",
            Duration::from_secs(5)
        )
        .net
    );
}
