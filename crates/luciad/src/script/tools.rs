//! 스크립트 도구(결정 0025): 코드 실행(code_run), 스킬 스크립트(skill_run), 반복 루틴 만들기·목록·고치기·지우기.
//! MCP 서버의 도구 메서드는 여기로 넘긴다.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use chrono_tz::Tz;
use lucia_store::{NewRoutine, Routine, RoutineStatus, Store, Trigger};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ScriptJob, ScriptOutcome, ScriptRunner};
use crate::tools::Policy;

/// 모델에게 돌려줄 결과 상한(바이트) [제안]: 거른 결과만 맥락에 들어오게.
pub const RETURN_MAX: usize = 8 * 1024;

/// 스크립트 도구가 쓰는 것.
#[derive(Clone)]
pub struct ScriptAccess {
    /// 스크립트 실행기. 꺼져 있으면 `None`(스크립트 없는 루틴만 만든다).
    pub runner: Option<Arc<ScriptRunner>>,
    pub store: Arc<Mutex<Store>>,
    /// 승인 데스크에 카드를 올리는 통로(루틴 승인).
    pub posts: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    pub tz: Tz,
    pub chat_timeout: Duration,
    pub routine_timeout: Duration,
    /// 스크립트 샌드박스의 패키지 설치(결정 0047). 스크립트가 꺼져 있으면 `None`.
    pub packages: Option<Arc<crate::packages::Packages>>,
}

/// `lib_install` 인자.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LibInstall {
    /// 설치할 PyPI 패키지(8개까지). 이름과 버전 조건만 쓴다. 예: ["pillow"], ["numpy==2.3.0", "pandas>=2"].
    pub packages: Vec<String>,
}

/// `code_run` 인자.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CodeRun {
    /// Python 3 코드(표준 라이브러리와 lib_install로 설치한 패키지). lucia.online이 True면 코드가 바깥 인터넷에 직접 나갈 수 있다. `lucia` 모듈로 도구를 부른다:
    /// lucia.call(도구, **인자) — 이 실행에서 쓸 수 있는 Lucia 도구(mail_search·mail_read·calendar_events·discord_messages는
    /// list/dict, 나머지는 글); lucia.mail_search(query, limit), lucia.mail_read(id), lucia.calendar_events(start, end, query, limit),
    /// lucia.discord_messages(channel, limit, before, after); lucia.fetch(url) — 공개 페이지 글(브라우저를 쓸 수 있는 실행만), lucia.fetch(url, session=True) — 로그인된 브라우저의 세션으로(사이트 위임한 사이트만);
    /// lucia.download(url) — 웹 주소의 파일(zip·PDF·그림 등)을 받아 파일 id를 돌려준다(100 MB까지. session=True면 로그인 세션으로, 사이트 위임한 사이트만);
    /// lucia.read_file(파일 id) — 파일 내용을 bytes로(사용자가 붙인 파일, 만든 그림, 웹에서 받은 파일. 100 MB까지), lucia.file_info(파일 id) — 이름·종류·크기,
    /// lucia.save_file(이름, bytes) — 새 파일로 남기고 파일 id를 돌려준다(30 MB까지. 그 id를 report_result attachments나 expression_add에 쓴다);
    /// lucia.input — 넘긴 입력; lucia.result(값) 또는 print — 돌려줄 것. 실패하면 lucia.ToolError.
    /// 루틴 스크립트를 시험할 때 lucia.notify/wake/next_run/finish는 실제로 하지 않고 무엇을 할지만 보인다.
    pub code: String,
    /// lucia.input으로 넘길 값(선택).
    #[serde(default)]
    pub input: Option<Value>,
}

/// `skill_run` 인자.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SkillRun {
    /// 스크립트가 있는 스킬 이름.
    pub name: String,
    /// lucia.input으로 넘길 값(선택).
    #[serde(default)]
    pub input: Option<Value>,
}

/// `routine_create` 인자.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RoutineCreate {
    /// 이름(한 줄, 60자까지). 예: "과제 메일 확인".
    pub name: String,
    /// Lucia를 깨울 때 할 일(스크립트가 lucia.wake를 부르거나, 스크립트가 없을 때마다). 예: "새 과제 메일을 마감일과 함께 요약해 알린다".
    pub task: String,
    /// 언제(cron, every_minutes, on_change 중 하나): cron 식(분 시 일 월 요일, 사용자 시간대). 예: "0 9 * * 1-5".
    #[serde(default)]
    pub cron: Option<String>,
    /// 또는 지난 실행에서 몇 분 뒤마다. 스크립트가 있으면 5분부터, 없으면(매번 Lucia를 깨우므로) 60분부터.
    #[serde(default)]
    pub every_minutes: Option<i64>,
    /// every_minutes와 함께: 이 시간대에만("HH:MM-HH:MM", 예: "09:00-21:00").
    #[serde(default)]
    pub between: Option<String>,
    /// 또는 사전 체크(한 시간마다)가 새 변화를 찾을 때: "gmail", "calendar", "watch"(모든 감시), "watch:<id>".
    /// 스크립트는 lucia.input["changes"]로 변화 목록([{summary, payload}])을 받는다.
    #[serde(default)]
    pub on_change: Option<String>,
    /// 먼저 돌릴 Python 스크립트(code_run과 같은 환경). 알릴 것만 lucia.notify(글)로 바로 알리고(급하지 않아 모아
    /// 보내도 되면 batch=True), 판단이 필요하면 lucia.wake(무엇을 볼지, 자료)로 Lucia를 깨우고, 할 일이 없으면 아무것도
    /// 하지 않는다(모델 0번).
    /// lucia.state(dict)는 다음 실행에 넘어간다(본 메일 ID 등). lucia.next_run(분)은 다음 실행을 한 번 당기거나 미루고,
    /// lucia.finish(까닭)는 루틴을 끝낸다. 없으면 매번 Lucia를 깨운다. code_run으로 먼저 돌려 본 코드를 넣는다.
    #[serde(default)]
    pub script: Option<String>,
    /// 스크립트가 부를 도구(읽기 도구와 공개 페이지 가져오기 "fetch", 로그인 세션 가져오기 "fetch_session"만). 여기 없는 도구는 부르지 못한다.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// lucia.input으로 넘길 값(선택).
    #[serde(default)]
    pub input: Option<Value>,
    /// 이때가 지나면 끝(사용자 시간대, 예: "2026-12-01 18:00"). 없으면 계속.
    #[serde(default)]
    pub until: Option<String>,
}

/// `routine_update` 인자. 바꿀 칸만 준다.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RoutineUpdate {
    /// routine_list의 id.
    pub id: String,
    /// "pause"(멈춤), "resume"(다시 켬). 다른 칸과 함께 써도 된다.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
    /// 언제를 바꿀 때: cron, every_minutes(+between), on_change 중 하나.
    #[serde(default)]
    pub cron: Option<String>,
    #[serde(default)]
    pub every_minutes: Option<i64>,
    #[serde(default)]
    pub between: Option<String>,
    #[serde(default)]
    pub on_change: Option<String>,
    /// 새 스크립트(빈 글이면 스크립트를 떼고 매번 Lucia를 깨운다). 바꾸면 한 번 시험 실행한다.
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub input: Option<Value>,
    /// 새 기한(빈 글이면 기한 없음).
    #[serde(default)]
    pub until: Option<String>,
}

/// `routine_delete` 인자.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RoutineDelete {
    pub id: String,
    /// 지우는 까닭 한 줄.
    pub reason: String,
}

const OFF: &str = "스크립트 샌드박스가 꺼져 있다(관리자가 설정 [scripts]를 켜야 한다). 지금은 도구를 직접 불러서 한다.";

impl ScriptAccess {
    /// `code_run`: 이 실행의 범위로 코드를 돌리고, 거른 결과만 돌려준다.
    /// `lib_install`: 스크립트 샌드박스에 패키지를 설치한다. 오래 걸리면 계속 설치하고 "아직"이라고 답한다.
    pub async fn lib_install(&self, run: &str, args: LibInstall) -> String {
        let Some(packages) = &self.packages else {
            return OFF.into();
        };
        let mut rx = match packages.start(&args.packages, Some(run)) {
            Ok(rx) => rx,
            Err(why) => return format!("설치하지 않았다: {why}"),
        };
        let waited =
            tokio::time::timeout(crate::packages::TOOL_WAIT, rx.wait_for(|v| v.is_some())).await;
        let text = match waited {
            Ok(Ok(done)) => match done.clone() {
                Some(Ok(text)) | Some(Err(text)) => text,
                None => "설치 결과를 받지 못했다.".into(),
            },
            Ok(Err(_)) => "설치 결과를 받지 못했다.".into(),
            Err(_) => {
                return "아직 설치하고 있다(큰 패키지는 몇 분 걸린다). 잠시 뒤 lib_list로 설치됐는지 확인하고 code_run에서 쓴다. 같은 설치를 다시 부르지 않는다.".into();
            }
        };
        // pip의 출력은 바깥(패키지 색인)에서 온 글이다.
        crate::mcp::external("lib_install", &text)
    }

    /// `lib_list`: 스크립트 샌드박스에 설치된 패키지.
    pub async fn lib_list(&self) -> String {
        let Some(packages) = &self.packages else {
            return OFF.into();
        };
        let doing = packages
            .installing()
            .map(|p| format!("지금 설치 중: {p}\n"))
            .unwrap_or_default();
        match packages.list().await {
            Ok(list) => {
                let lines: Vec<String> = list.iter().map(|(n, v)| format!("{n} {v}")).collect();
                crate::mcp::external(
                    "lib_list",
                    &format!(
                        "{doing}설치된 패키지 {}개:\n{}",
                        list.len(),
                        lines.join("\n")
                    ),
                )
            }
            Err(why) => format!("{doing}{why}"),
        }
    }

    /// `code_run`: 결과 글과, 스크립트가 만든 파일(도구가 그림을 함께 보여 준다, 결정 0048).
    pub async fn code_run_full(
        &self,
        run: &str,
        grants: Policy,
        args: CodeRun,
    ) -> (String, Vec<lucia_store::FileRecord>) {
        let none = Vec::new();
        let Some(runner) = &self.runner else {
            return (OFF.into(), none);
        };
        if args.code.trim().is_empty() {
            return ("코드가 비었다.".into(), none);
        }
        if args.code.len() > lucia_store::skill_limits::SCRIPT_MAX_BYTES {
            return (
                format!(
                    "코드가 너무 길다({}KB까지).",
                    lucia_store::skill_limits::SCRIPT_MAX_BYTES / 1024
                ),
                none,
            );
        }
        let out = runner
            .run(ScriptJob {
                code: args.code,
                input: args.input.unwrap_or_else(|| json!({})),
                state: json!({}),
                grants: grants.for_script(),
                run: format!("{run}-code"),
                declared: None,
                // 바깥으로 나가는 가져오기는 브라우저를 쓸 수 있는 실행만. 사람 없는 실행은 위임한 사이트만(결정 0029).
                fetch: grants.allows("browser_open"),
                fetch_hosts: self.fetch_hosts(run, grants),
                session_hosts: grants.allows("browser_open").then(|| self.granted_hosts()),
                // 파일 읽기·만들기(결정 0047)는 파일을 만들 수 있는 실행만.
                files: grants.allows("file_create"),
                net: self.net_open(grants),
                timeout: self.chat_timeout,
            })
            .await;
        self.log("code_run", &format!("코드 실행: {}", summary(&out)), run);
        (
            crate::mcp::external("code_run", &out.text(RETURN_MAX)),
            out.files,
        )
    }

    /// 코드가 바깥 인터넷에 직접 나가도 되는가(결정 0058): 사람이 대화 중이고 브라우저를 쓸 수 있는 실행만.
    /// 프록시는 TLS 안을 보지 못해 나가는 글 검사(결정 0029)가 걸리지 않는다. 그래서 그 검사가 "막기"이거나 권한
    /// 모드가 엄격이면 열지 않는다(코드는 여전히 `lucia.fetch`·`lucia.download`로 나간다).
    fn net_open(&self, grants: Policy) -> bool {
        grants.human
            && grants.allows("browser_open")
            && self
                .store
                .lock()
                .is_ok_and(|store| !crate::leak::blocking(&store))
    }

    /// 사람 없는 실행(위임 전 후속 조치 빼고)이면 가져올 수 있는 호스트(사이트 위임). 사람이 대화 중이면 `None`.
    fn fetch_hosts(&self, run: &str, grants: Policy) -> Option<Vec<String>> {
        let caller = crate::delegation::Caller {
            run: run.to_owned(),
            human: grants.human,
        };
        if !crate::delegation::bounded(&caller) {
            return None;
        }
        let now = Utc::now();
        Some(
            self.store
                .lock()
                .map(|s| {
                    s.site_grants()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|g| g.active(now))
                        .map(|g| g.host)
                        .collect()
                })
                .unwrap_or_default(),
        )
    }

    /// `skill_run`: 스킬의 스크립트를 선언한 도구로 돌린다. 쓴 기록과 상태(`skill:<이름>`)를 남긴다.
    #[cfg(test)]
    pub async fn skill_run(&self, run: &str, grants: Policy, args: SkillRun) -> String {
        self.skill_run_full(run, grants, args, &mut Vec::new())
            .await
    }

    /// `skill_run`. 스크립트가 만든 파일은 `made`에 담는다(도구가 그림을 함께 보여 준다, 결정 0048).
    pub async fn skill_run_full(
        &self,
        run: &str,
        grants: Policy,
        args: SkillRun,
        made: &mut Vec<lucia_store::FileRecord>,
    ) -> String {
        let Some(runner) = &self.runner else {
            return OFF.into();
        };
        let name = args
            .name
            .trim()
            .trim_start_matches("[[")
            .trim_end_matches("]]");
        let owner = format!("skill:{name}");
        let (code, tools, state) = {
            let Ok(store) = self.store.lock() else {
                return "저장소를 쓸 수 없다.".into();
            };
            match store.skill(name) {
                Ok(Some(s)) if s.status.usable() => {}
                Ok(Some(s)) => return format!("그 스킬은 지금 쓸 수 없다({}).", s.status.label()),
                Ok(None) => return "그런 스킬이 없다.".into(),
                Err(e) => return format!("읽지 못했다: {e}"),
            }
            let Some((code, tools)) = store.skill_script(name).ok().flatten() else {
                return "그 스킬에는 스크립트가 없다. skill_read로 절차를 읽고 한다.".into();
            };
            let task = crate::tasks::task_of_run(run).map(str::to_owned);
            let _ = store.record_skill_use(name, run, task.as_deref(), Utc::now());
            let state = store
                .script_state(&owner)
                .ok()
                .flatten()
                .unwrap_or_else(|| json!({}));
            (code, tools, state)
        };
        let fetch = tools.iter().any(|t| t == "fetch") && grants.allows("browser_open");
        let session_hosts = (tools.iter().any(|t| t == "fetch_session")
            && grants.allows("browser_open"))
        .then(|| self.granted_hosts());
        let out = runner
            .run(ScriptJob {
                code,
                input: args.input.unwrap_or_else(|| json!({})),
                state,
                grants: grants.for_script(),
                run: format!("{run}-skill"),
                declared: Some(tools),
                fetch,
                fetch_hosts: self.fetch_hosts(run, grants),
                session_hosts,
                files: grants.allows("file_create"),
                // 스킬 스크립트는 선언한 가져오기로만 나간다.
                net: false,
                timeout: self.chat_timeout,
            })
            .await;
        made.extend(out.files.iter().cloned());
        if let (true, Some(state)) = (out.ok, &out.state)
            && let Ok(store) = self.store.lock()
        {
            let _ = store.set_script_state(&owner, state, Utc::now());
        }
        self.log(
            "skill_run",
            &format!("스킬 스크립트 {name}: {}", summary(&out)),
            run,
        );
        let mut text = crate::mcp::external("skill_run", &out.text(RETURN_MAX));
        if !out.ok {
            text.push_str("\n스크립트가 실패했다. skill_read로 절차를 읽고 직접 하고, 사람과의 대화라면 고친 스크립트를 skill_save로 남긴다.");
        }
        text
    }

    /// `routine_create`: 검사하고, 스크립트가 있으면 한 번 시험해 기준 상태를 잡은 뒤 건다.
    pub async fn routine_create(&self, run: &str, args: RoutineCreate) -> String {
        let now = Utc::now();
        let trigger = match crate::routines::parse_trigger(
            args.cron.as_deref(),
            args.every_minutes,
            args.between.as_deref(),
            args.on_change.as_deref(),
        ) {
            Ok(t) => t,
            Err(e) => return format!("만들지 않았다: {e}"),
        };
        let script = args.script.filter(|s| !s.trim().is_empty());
        let tools = args.tools.unwrap_or_default();
        let input = args.input.unwrap_or_else(|| json!({}));
        let until = match args
            .until
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            Some(u) => match crate::autonomy::parse_when(u, self.tz) {
                Ok(t) if t > now => Some(t),
                Ok(_) => return "만들지 않았다: until이 지났다.".into(),
                Err(e) => return format!("만들지 않았다: until: {e}"),
            },
            None => None,
        };
        if let Err(e) =
            self.check_fields(&args.name, &args.task, &trigger, script.as_deref(), &tools)
        {
            return format!("만들지 않았다: {e}");
        }
        if let Err(e) = review_tools(run, &tools) {
            return format!("만들지 않았다: {e}");
        }
        if !input.is_object() {
            return "만들지 않았다: input은 객체여야 한다.".into();
        }
        // 시험 실행: 실패하면 걸지 않는다. 성공하면 상태를 기준선으로 남긴다(효과는 하지 않고 보이기만).
        let mut trial = String::new();
        let mut baseline = None;
        if let Some(code) = &script {
            let Some(runner) = &self.runner else {
                return format!("만들지 않았다: {OFF}");
            };
            let mut test_input = input.clone();
            if matches!(trigger, Trigger::Change(_)) {
                test_input["changes"] = json!([]);
            }
            let out = runner
                .run(self.routine_job(code, &tools, test_input, json!({}), run, self.chat_timeout))
                .await;
            if !out.ok {
                return format!(
                    "만들지 않았다: 시험 실행이 실패했다. 고쳐서 다시 만든다.\n{}",
                    crate::mcp::external("routine-trial", &out.text(RETURN_MAX))
                );
            }
            baseline = out.state.clone();
            trial = crate::mcp::external("routine-trial", &out.text(RETURN_MAX / 2));
        }
        // 권한 모드 엄격(결정 0040): 새 루틴은 모두 승인 카드를 거친다.
        let strict = self.store.lock().is_ok_and(|s| crate::security::strict(&s));
        let review =
            strict || (script.is_some() && crate::routines::needs_review(&tools, &trigger));
        let id = format!(
            "routine-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let Ok(store) = self.store.lock() else {
            return "저장소를 쓸 수 없다.".into();
        };
        let created = store.create_routine(
            &id,
            &NewRoutine {
                name: &args.name,
                trigger: &trigger,
                task: &args.task,
                script: script.as_deref(),
                tools: &tools,
                input: &input,
                status: if review {
                    RoutineStatus::Review
                } else {
                    RoutineStatus::Active
                },
                conversation: None,
                until,
            },
            now,
        );
        match created {
            Ok(Ok(())) => {}
            Ok(Err(why)) => return format!("만들지 않았다: {why}"),
            Err(e) => return format!("만들지 못했다: {e}"),
        }
        if let Some(state) = &baseline {
            let _ = store.set_script_state(&format!("routine:{id}"), state, now);
        }
        // 목표 점검이 만든 루틴은 그 목표의 하위 항목이다(결정 0032): 깨울 때 점검과 같은 범위로 돈다.
        if let Some(goal) = crate::runctx::get(run)
            .filter(|c| c.goal_review)
            .and_then(|c| c.goal)
        {
            let _ = store.link_goal_child(&goal, "routine", &id, true, now);
            let _ = store.add_goal_log(&goal, "child", &format!("점검이 루틴 {id} 만듦"), now);
        }
        let _ = store.add_activity(
            "routine_create",
            &crate::mask::global().mask(&format!(
                "루틴 만듦 {id} \"{}\" ({}){}",
                args.name.trim(),
                crate::routines::trigger_label(&trigger),
                if script.is_some() {
                    ", 스크립트"
                } else {
                    ""
                }
            )),
            Some(run),
            now,
        );
        let Some(r) = store.routine(&id).ok().flatten() else {
            return "만들었지만 다시 읽지 못했다.".into();
        };
        if review {
            let posted = self.request_review(&store, &r);
            return format!(
                "루틴 {id}을(를) 만들었다. 사용자 자료를 읽으면서 바깥 페이지도 가져오는 스크립트라 사용자가 Discord 카드에서 켜야 돈다{}.\n{trial}",
                if posted {
                    ""
                } else {
                    "(카드를 올리지 못했다: 관리 페이지에서 켤 수 있다)"
                }
            );
        }
        let next = crate::routines::arm(&store, &r, now, self.tz)
            .ok()
            .flatten();
        format!(
            "루틴 {id}을(를) 만들고 켰다: {}. {}{}\n{trial}",
            crate::routines::trigger_label(&trigger),
            match next {
                Some(at) => format!(
                    "다음 실행 {}.",
                    at.with_timezone(&self.tz).format("%m-%d %H:%M")
                ),
                None => "새 변화가 있을 때 돈다.".to_owned(),
            },
            if script.is_some() {
                " 시험 실행의 상태를 기준선으로 남겼다(위 효과는 실제로 하지 않았다)."
            } else {
                " 스크립트가 없어 돌 때마다 Lucia를 깨운다."
            }
        )
    }

    /// `routine_list`.
    pub fn routine_list(&self) -> String {
        let Ok(store) = self.store.lock() else {
            return "저장소를 쓸 수 없다.".into();
        };
        let list = store.routines().unwrap_or_default();
        if list.is_empty() {
            return "루틴이 없다.".into();
        }
        let now = Utc::now();
        list.iter()
            .map(|r| {
                let next = store
                    .schedule(&r.id)
                    .ok()
                    .flatten()
                    .filter(|s| s.enabled)
                    .and_then(|s| s.next_due_at)
                    .map(|t| {
                        format!(
                            " · 다음 {}",
                            t.with_timezone(&self.tz).format("%m-%d %H:%M")
                        )
                    })
                    .unwrap_or_default();
                let last = match (&r.last_run_at, r.last_ok) {
                    (Some(t), Some(ok)) => format!(
                        " · 지난 {} {}{}",
                        t.with_timezone(&self.tz).format("%m-%d %H:%M"),
                        if ok { "성공" } else { "실패" },
                        r.last_text
                            .as_deref()
                            .map(|x| format!(": {}", x.chars().take(80).collect::<String>()))
                            .unwrap_or_default()
                    ),
                    _ => String::new(),
                };
                let _ = now;
                format!(
                    "- {} \"{}\" [{}] {}{}{}{} · 할 일: {}",
                    r.id,
                    r.name,
                    r.status.label(),
                    crate::routines::trigger_label(&r.trigger),
                    if r.script.is_some() {
                        format!(" · 스크립트(도구: {})", r.tools.join(", "))
                    } else {
                        " · 매번 Lucia".to_owned()
                    },
                    next,
                    last,
                    r.task.chars().take(100).collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `routine_update`.
    pub async fn routine_update(&self, run: &str, args: RoutineUpdate) -> String {
        let now = Utc::now();
        let Some(mut r) = self
            .store
            .lock()
            .ok()
            .and_then(|s| s.routine(&args.id).ok().flatten())
        else {
            return "그런 루틴이 없다.".into();
        };
        if r.status == RoutineStatus::Done {
            return "끝난 루틴은 고치지 않는다. 새로 만든다.".into();
        }
        let when_given = args.cron.is_some()
            || args.every_minutes.is_some()
            || args.between.is_some()
            || args.on_change.is_some();
        if when_given {
            match crate::routines::parse_trigger(
                args.cron.as_deref(),
                args.every_minutes,
                args.between.as_deref(),
                args.on_change.as_deref(),
            ) {
                Ok(t) => r.trigger = t,
                Err(e) => return format!("고치지 않았다: {e}"),
            }
        }
        if let Some(n) = args.name {
            r.name = n;
        }
        if let Some(t) = args.task {
            r.task = t;
        }
        let script_changed = args.script.is_some() || args.tools.is_some();
        if let Some(s) = args.script {
            r.script = (!s.trim().is_empty()).then_some(s);
        }
        if let Some(t) = args.tools {
            r.tools = t;
        }
        if let Some(i) = args.input {
            if !i.is_object() {
                return "고치지 않았다: input은 객체여야 한다.".into();
            }
            r.input = i;
        }
        if let Some(u) = args.until {
            r.until = match u.trim() {
                "" => None,
                u => match crate::autonomy::parse_when(u, self.tz) {
                    Ok(t) if t > now => Some(t),
                    Ok(_) => return "고치지 않았다: until이 지났다.".into(),
                    Err(e) => return format!("고치지 않았다: until: {e}"),
                },
            };
        }
        if let Err(e) =
            self.check_fields(&r.name, &r.task, &r.trigger, r.script.as_deref(), &r.tools)
        {
            return format!("고치지 않았다: {e}");
        }
        if let Err(e) = review_tools(run, &r.tools) {
            return format!("고치지 않았다: {e}");
        }
        let mut trial = String::new();
        let mut baseline = None;
        if script_changed && let Some(code) = &r.script {
            let Some(runner) = &self.runner else {
                return format!("고치지 않았다: {OFF}");
            };
            let state = self
                .store
                .lock()
                .ok()
                .and_then(|s| s.script_state(&format!("routine:{}", r.id)).ok().flatten())
                .unwrap_or_else(|| json!({}));
            let mut input = r.input.clone();
            if matches!(r.trigger, Trigger::Change(_)) {
                input["changes"] = json!([]);
            }
            let out = runner
                .run(self.routine_job_for(
                    Some(&r.id),
                    code,
                    &r.tools,
                    input,
                    state,
                    run,
                    self.chat_timeout,
                ))
                .await;
            if !out.ok {
                return format!(
                    "고치지 않았다: 새 스크립트의 시험 실행이 실패했다.\n{}",
                    crate::mcp::external("routine-trial", &out.text(RETURN_MAX))
                );
            }
            baseline = out.state.clone();
            trial = crate::mcp::external("routine-trial", &out.text(RETURN_MAX / 2));
        }
        // 승인이 필요한 루틴은 카드(또는 관리 페이지)에서만 켠다: 승인 전이면 무엇을 고쳐도 승인 대기로 두고,
        // 켜진 것도 코드·도구·언제가 바뀌면 다시 승인을 받는다.
        let was_review = r.status == RoutineStatus::Review;
        let strict = self.store.lock().is_ok_and(|s| crate::security::strict(&s));
        let needs =
            strict || (r.script.is_some() && crate::routines::needs_review(&r.tools, &r.trigger));
        let changed = script_changed || when_given;
        let review = needs && (was_review || changed);
        let repost = review && (changed || !was_review);
        let Ok(store) = self.store.lock() else {
            return "저장소를 쓸 수 없다.".into();
        };
        if let Err(e) = store.update_routine(&r, now) {
            return format!("고치지 못했다: {e}");
        }
        r.next_at = None;
        if let Some(state) = &baseline {
            let _ = store.set_script_state(&format!("routine:{}", r.id), state, now);
        }
        let wanted = match args.state.as_deref().map(str::trim) {
            Some("pause") => Some(RoutineStatus::Paused),
            Some("resume") => Some(RoutineStatus::Active),
            Some("") | None => None,
            Some(other) => {
                return format!("state는 pause 또는 resume이다({other:?}). 다른 칸은 고쳤다.");
            }
        };
        let status = if review {
            RoutineStatus::Review
        } else {
            match (wanted, r.status) {
                (Some(s), _) => s,
                // 승인을 기다리던 것은 스크립트를 바꿔 승인이 필요 없어졌으면 켠다.
                (None, RoutineStatus::Review) => RoutineStatus::Active,
                (None, s) => s,
            }
        };
        let _ = store.set_routine_status(&r.id, status, now);
        r.status = status;
        let _ = store.add_activity(
            "routine_update",
            &crate::mask::global().mask(&format!(
                "루틴 고침 {} \"{}\" ({}, {})",
                r.id,
                r.name,
                crate::routines::trigger_label(&r.trigger),
                status.label()
            )),
            Some(run),
            now,
        );
        let next = crate::routines::arm(&store, &r, now, self.tz)
            .ok()
            .flatten();
        if review {
            let posted = !repost || self.request_review(&store, &r);
            return format!(
                "고쳤다. 사용자 자료를 읽으면서 바깥 페이지도 가져오는 스크립트라 사용자가 카드에서 켜야 돈다(이 도구로는 켤 수 없다){}.\n{trial}",
                if posted {
                    ""
                } else {
                    "(카드를 올리지 못했다: 관리 페이지에서 켤 수 있다)"
                }
            );
        }
        format!(
            "고쳤다({}). {}\n{trial}",
            status.label(),
            match next {
                Some(at) => format!(
                    "다음 실행 {}.",
                    at.with_timezone(&self.tz).format("%m-%d %H:%M")
                ),
                None if status == RoutineStatus::Active => "새 변화가 있을 때 돈다.".into(),
                None => String::new(),
            }
        )
    }

    /// `routine_delete`.
    pub fn routine_delete(&self, run: &str, args: RoutineDelete) -> String {
        let Ok(mut store) = self.store.lock() else {
            return "저장소를 쓸 수 없다.".into();
        };
        let Some(r) = store.routine(&args.id).ok().flatten() else {
            return "그런 루틴이 없다.".into();
        };
        let _ = crate::routines::disarm(&store, &r.id);
        match store.delete_routine(&r.id) {
            Ok(true) => {
                let _ = store.add_activity(
                    "routine_delete",
                    &crate::mask::global().mask(&format!(
                        "루틴 지움 {} \"{}\": {}",
                        r.id,
                        r.name,
                        args.reason.chars().take(200).collect::<String>()
                    )),
                    Some(run),
                    Utc::now(),
                );
                format!("루틴 {} \"{}\"을(를) 지웠다.", r.id, r.name)
            }
            Ok(false) => "그런 루틴이 없다.".into(),
            Err(e) => format!("지우지 못했다: {e}"),
        }
    }

    /// 루틴 스크립트 실행(사람 없음): 읽기 범위와 선언한 도구만. 만들 때의 시험은 대화 중이므로 대화 상한으로,
    /// 발사는 루틴 상한으로 돈다.
    pub fn routine_job(
        &self,
        code: &str,
        tools: &[String],
        input: Value,
        state: Value,
        run: &str,
        timeout: Duration,
    ) -> ScriptJob {
        self.routine_job_for(None, code, tools, input, state, run, timeout)
    }

    /// 목표 점검이 만든 루틴(또는 점검이 지금 만드는·고치는 루틴)의 가져오기는 사이트 위임으로 묶는다(결정 0032):
    /// 사람이 정한 가져오기가 아니라서 결정 0025의 전제("만들 때 사람이 정한 가져오기")가 맞지 않는다.
    fn review_bound(&self, routine: Option<&str>, run: &str) -> bool {
        crate::runctx::get(run).is_some_and(|c| c.goal_review)
            || routine.is_some_and(|id| {
                self.store
                    .lock()
                    .ok()
                    .and_then(|s| s.goal_parent("routine", id).ok().flatten())
                    .is_some_and(|p| p.by_review)
            })
    }

    fn granted_hosts(&self) -> Vec<String> {
        let now = Utc::now();
        self.store
            .lock()
            .map(|s| {
                s.site_grants()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|g| g.active(now))
                    .map(|g| g.host)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 루틴 스크립트 실행. `routine`이 있으면 그 루틴의 출처로 가져오기를 묶는다.
    #[allow(clippy::too_many_arguments)]
    pub fn routine_job_for(
        &self,
        routine: Option<&str>,
        code: &str,
        tools: &[String],
        input: Value,
        state: Value,
        run: &str,
        timeout: Duration,
    ) -> ScriptJob {
        let fetch_hosts = self
            .review_bound(routine, run)
            .then(|| self.granted_hosts());
        ScriptJob {
            code: code.to_owned(),
            input,
            state,
            grants: crate::routines::script_policy(),
            run: format!("{run}-routine"),
            declared: Some(
                tools
                    .iter()
                    .filter(|t| *t != "fetch" && *t != "fetch_session")
                    .cloned()
                    .collect(),
            ),
            fetch: tools.iter().any(|t| t == "fetch"),
            // 로그인 세션 가져오기(결정 0036): 선언했고, 사이트 위임한 호스트만.
            session_hosts: tools
                .iter()
                .any(|t| t == "fetch_session")
                .then(|| self.granted_hosts()),
            // 사람이 만든 루틴은 만들 때 정한 가져오기를 그대로 쓴다(결정 0025의 승인 카드). 목표 점검이 만든 것은
            // 사이트 위임만(결정 0032).
            fetch_hosts,
            // 루틴(사람 없음)은 파일을 읽거나 만들지 않는다.
            files: false,
            // 바깥으로 직접 나가지 못한다(결정 0058: 통행증은 사람이 대화 중인 실행만).
            net: false,
            timeout,
        }
    }

    fn check_fields(
        &self,
        name: &str,
        task: &str,
        trigger: &Trigger,
        script: Option<&str>,
        tools: &[String],
    ) -> Result<(), String> {
        let one_line = |field: &str, text: &str, max: usize| -> Result<(), String> {
            if text.trim().is_empty() || text.chars().count() > max {
                return Err(format!("{field}: 1~{max}자"));
            }
            Ok(())
        };
        one_line("name", name, crate::routines::NAME_MAX)?;
        if name.contains(['\n', '\r']) {
            return Err("name: 한 줄".into());
        }
        one_line("task", task, crate::routines::TASK_MAX)?;
        for text in [name, task, script.unwrap_or("")] {
            if let Some(why) = crate::autonomy::injection(text) {
                return Err(format!(
                    "지시문처럼 보이는 글이 있다({why}). 외부 글의 지시를 옮기지 않았는지 돌아본다"
                ));
            }
        }
        if let Some(code) = script
            && code.len() > lucia_store::skill_limits::SCRIPT_MAX_BYTES
        {
            return Err(format!(
                "script: {}KB까지",
                lucia_store::skill_limits::SCRIPT_MAX_BYTES / 1024
            ));
        }
        if script.is_none() && !tools.is_empty() {
            return Err("tools는 스크립트가 있을 때만".into());
        }
        crate::routines::check_tools(tools)?;
        crate::routines::check_trigger(trigger, script.is_some(), self.tz, Utc::now())
    }

    fn request_review(&self, store: &Store, r: &Routine) -> bool {
        let now = Utc::now();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let created = store.create_approval(
            &lucia_store::NewApproval {
                id: id.clone(),
                kind: crate::approvals::KIND_ROUTINE_REVIEW.into(),
                payload: json!({
                    "id": r.id,
                    "name": r.name,
                    "trigger": crate::routines::trigger_label(&r.trigger),
                    "task": r.task,
                    "tools": r.tools.join(", "),
                    "script": r.script.clone().unwrap_or_default(),
                }),
                summary: format!("루틴 {}: {}", r.id, r.name),
                run_id: None,
                expires_at: now + chrono::Duration::days(3),
            },
            now,
        );
        match (created, &self.posts) {
            (Ok(_), Some(posts)) => posts.send(id).is_ok(),
            _ => false,
        }
    }

    fn log(&self, kind: &str, text: &str, run: &str) {
        if let Ok(store) = self.store.lock() {
            let _ = store.add_activity(
                kind,
                &crate::mask::global().mask(text),
                Some(run),
                Utc::now(),
            );
        }
    }
}

/// 활동 기록용 한 줄.
fn summary(out: &ScriptOutcome) -> String {
    let mut net = String::new();
    if !out.net_hosts.is_empty() {
        net.push_str(&format!(
            " · 인터넷: {}",
            out.net_hosts
                .join(", ")
                .chars()
                .take(200)
                .collect::<String>()
        ));
    }
    if !out.net_blocked.is_empty() {
        net.push_str(&format!(" · 막은 연결 {}개", out.net_blocked.len()));
    }
    format!(
        "{} · 도구 {}번 · {:.1}초{}{net}",
        if out.ok { "성공" } else { "실패" },
        out.calls.len(),
        out.elapsed.as_secs_f64(),
        if out.calls.is_empty() {
            String::new()
        } else {
            let mut names: Vec<&str> = out.calls.iter().map(String::as_str).collect();
            names.dedup();
            format!(
                " ({})",
                names.join(", ").chars().take(150).collect::<String>()
            )
        }
    )
}

/// 목표 점검이 만들거나 고치는 루틴의 스크립트 도구는 점검 자신이 쓸 수 있는 것만(결정 0032: 만든 실행보다 넓게
/// 돌지 않는다. 예: 점검에는 Discord 읽기가 없다).
fn review_tools(run: &str, tools: &[String]) -> Result<(), String> {
    if !crate::runctx::get(run).is_some_and(|c| c.goal_review) {
        return Ok(());
    }
    let grants = crate::agent::RunProfile::GoalReview.grants();
    match tools.iter().find(|t| *t != "fetch" && !grants.allows(t)) {
        Some(t) => Err(format!(
            "목표 점검이 만드는 루틴은 점검이 쓸 수 있는 도구만 쓴다({t}은(는) 안 된다)"
        )),
        None => Ok(()),
    }
}
