//! 스크립트 샌드박스의 Python 패키지(결정 0047): Lucia가 필요한 라이브러리를 설치해 `code_run`에서 쓴다.
//!
//! - **어디에**: 스크립트 샌드박스(VM) 안에만 설치한다. 호스트와 다른 샌드박스는 건드리지 않는다.
//! - **네트워크**: 평소에는 그 샌드박스에 허용 규칙이 하나도 없다. 설치하는 동안만 패키지 색인 두 호스트
//!   ([`INDEX_HOSTS`])를 그 샌드박스에 열고, 끝나면(실패·시간 초과여도) 닫는다. 데몬이 뜰 때는 앞선 실행이 남긴
//!   허용 규칙을 치운다.
//! - **무엇을**: 이름과 버전 조건만 받는다(주소·경로·옵션은 받지 않는다). 그래서 색인 밖에서는 가져오지 않는다.
//! - **오래 걸리면**: 설치는 데몬 작업으로 돌고 도구는 [`TOOL_WAIT`]만 기다린다(MCP 요청은 60초쯤에 끊긴다).
//! - 한 번에 하나만 설치한다. 결과는 활동 기록에 남긴다.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lucia_store::Store;
use tokio::process::Command;
use tokio::sync::watch;

use crate::browser::BoxFut;

/// 설치하는 동안만 여는 호스트(PyPI 색인과 파일).
pub const INDEX_HOSTS: &[&str] = &["pypi.org", "files.pythonhosted.org"];
/// 도구가 설치를 기다리는 시간.
pub const TOOL_WAIT: Duration = Duration::from_secs(45);
/// 설치 하나의 상한(VM 안).
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
/// 호스트에서 `sbx` 명령 하나를 기다리는 시간(규칙 걸기·지우기·목록). VM을 깨우는 시간을 넣어 잡는다.
const SBX_TIMEOUT: Duration = Duration::from_secs(40);
/// 설치 명령을 호스트에서 기다리는 시간(VM 안 상한보다 조금 길게). `sbx exec`가 멈춰도 길은 닫아야 한다.
const INSTALL_HOST_TIMEOUT: Duration = Duration::from_secs(330);
/// 한 번에 설치하는 패키지 수.
pub const MAX_SPECS: usize = 8;
/// 돌려주는 pip 출력(끝부분).
const OUTPUT_TAIL: usize = 1500;

/// 패키지 이름과 버전 조건만(예: `pillow`, `numpy==2.3.0`, `requests[socks]>=2.31`). 주소·경로·옵션은 아니다.
pub fn valid_spec(spec: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"^[A-Za-z0-9](?:[A-Za-z0-9._-]{0,62}[A-Za-z0-9])?(?:\[[A-Za-z0-9,._-]{1,60}\])?(?:(?:==|>=|<=|~=|!=|<|>)[A-Za-z0-9.*+!_-]{1,32}(?:,(?:==|>=|<=|~=|!=|<|>)[A-Za-z0-9.*+!_-]{1,32})*)?$",
        )
        .expect("패키지 표기")
    })
    .is_match(spec)
}

/// 샌드박스에 하는 일(운영은 sbx, 시험은 가짜).
pub trait Sandbox: Send + Sync {
    /// 패키지 색인으로 나가는 길을 연다. 돌려주는 것은 닫을 때 쓸 규칙 ID.
    fn open(&self) -> BoxFut<'_, Result<Vec<String>, String>>;
    fn close<'a>(&'a self, ids: &'a [String]) -> BoxFut<'a, ()>;
    /// 앞선 실행이 남긴 허용 규칙을 치운다. 치운 수.
    fn sweep(&self) -> BoxFut<'_, Result<usize, String>>;
    /// 설치한다. (성공했나, 출력).
    fn install<'a>(&'a self, specs: &'a [String]) -> BoxFut<'a, Result<(bool, String), String>>;
    /// 설치된 패키지(이름, 버전).
    fn list(&self) -> BoxFut<'_, Result<Vec<(String, String)>, String>>;
}

/// 스크립트 샌드박스(`sbx`).
pub struct SbxSandbox {
    pub sbx: PathBuf,
    pub name: String,
    /// 늘 열어 두는 허용 규칙의 대상(스크립트 출구 프록시 `localhost:<포트>`, 결정 0058). 치울 때 남기고,
    /// 없으면 건다. `None`이면 허용 규칙을 하나도 남기지 않는다.
    pub keep: Option<String>,
}

impl SbxSandbox {
    async fn run(&self, args: &[&str]) -> Result<(bool, String), String> {
        self.run_for(args, SBX_TIMEOUT).await
    }

    /// `sbx` 명령 하나. `limit` 안에 끝나지 않으면 끊는다(`kill_on_drop`).
    async fn run_for(&self, args: &[&str], limit: Duration) -> Result<(bool, String), String> {
        let child = Command::new(&self.sbx)
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(limit, child)
            .await
            .map_err(|_| format!("sbx가 {}초 안에 답하지 않았다", limit.as_secs()))?
            .map_err(|e| format!("sbx를 실행하지 못했다: {e}"))?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        Ok((out.status.success(), text))
    }
}

fn rule_ids(text: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
            .expect("규칙 ID")
    })
    .find_iter(text)
    .map(|m| m.as_str().to_owned())
    .collect()
}

/// `sbx policy ls <이름> --json`에서 이 샌드박스에 걸린 네트워크 허용 규칙: (치울 규칙의 ID, 남길 규칙이 있는가).
/// 남길 규칙은 대상이 정확히 `keep` 하나인 것이다(다른 대상이 섞인 규칙은 치운다).
fn allow_rules(json: &str, sandbox: &str, keep: Option<&str>) -> (Vec<String>, bool) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return (Vec::new(), false);
    };
    let scope = format!("sandbox:{sandbox}");
    let mut kept = false;
    let mut remove = Vec::new();
    for r in v["rules"].as_array().into_iter().flatten() {
        if !(r["scope"] == scope.as_str()
            && r["resource_type"] == "network"
            && r["decision"] == "allow"
            && r["editable"] == true)
        {
            continue;
        }
        let only_keep = keep.is_some_and(|k| {
            r["resources"]
                .as_array()
                .is_some_and(|a| a.len() == 1 && a[0] == k)
        });
        if only_keep && !kept {
            kept = true;
        } else if let Some(id) = r["id"].as_str() {
            remove.push(id.to_owned());
        }
    }
    (remove, kept)
}

/// 샌드박스 하나에 허용 규칙 하나(`target`, 예: `localhost:47812`)가 있게(`want`) 또는 없게 한다(결정 0059).
/// 그 샌드박스의 다른 규칙은 건드리지 않는다. 데몬이 뜰 때 한 번 부른다.
pub async fn ensure_allow_rule(sbx: &std::path::Path, sandbox: &str, target: &str, want: bool) {
    let run = |args: Vec<String>| async move {
        let child = Command::new(sbx)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        match tokio::time::timeout(SBX_TIMEOUT, child).await {
            Ok(Ok(out)) => Some((
                out.status.success(),
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                ),
            )),
            _ => None,
        }
    };
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    let Some((_, listed)) = run(s(&["policy", "ls", sandbox, "--json"])).await else {
        tracing::warn!(sandbox, target, "샌드박스의 네트워크 규칙을 읽지 못했다");
        return;
    };
    let json = listed.find('{').map_or("", |i| &listed[i..]);
    let ids = rules_for(json, sandbox, target);
    match (want, ids.is_empty()) {
        (true, true) => {
            let added = run(s(&[
                "policy",
                "allow",
                "network",
                "--sandbox",
                sandbox,
                target,
            ]))
            .await;
            if matches!(added, Some((true, _))) {
                tracing::info!(sandbox, target, "출구 프록시로 가는 허용 규칙을 걸었다");
            } else {
                tracing::warn!(
                    sandbox,
                    target,
                    "출구 프록시로 가는 허용 규칙을 걸지 못했다"
                );
            }
        }
        (false, false) => {
            for id in ids {
                let _ = run(s(&[
                    "policy",
                    "rm",
                    "network",
                    "--sandbox",
                    sandbox,
                    "--id",
                    &id,
                    "--force",
                ]))
                .await;
            }
            tracing::info!(sandbox, target, "출구 프록시로 가는 허용 규칙을 치웠다");
        }
        _ => {}
    }
}

/// `sbx policy ls --json`에서 이 샌드박스에 걸린, 대상이 정확히 `target` 하나인 허용 규칙의 ID.
fn rules_for(json: &str, sandbox: &str, target: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let scope = format!("sandbox:{sandbox}");
    v["rules"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| {
            r["scope"] == scope.as_str()
                && r["resource_type"] == "network"
                && r["decision"] == "allow"
                && r["editable"] == true
                && r["resources"]
                    .as_array()
                    .is_some_and(|a| a.len() == 1 && a[0] == target)
        })
        .filter_map(|r| r["id"].as_str().map(str::to_owned))
        .collect()
}

/// `pip list --format=json`의 출력에서 (이름, 버전).
fn parse_list(text: &str) -> Option<Vec<(String, String)>> {
    let line = text
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('['))?;
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    Some(
        v.as_array()?
            .iter()
            .filter_map(|p| {
                Some((
                    p["name"].as_str()?.to_owned(),
                    p["version"].as_str()?.to_owned(),
                ))
            })
            .collect(),
    )
}

impl Sandbox for SbxSandbox {
    fn open(&self) -> BoxFut<'_, Result<Vec<String>, String>> {
        Box::pin(async move {
            // 네트워크 규칙은 반드시 이 샌드박스에만 건다(빼면 모든 샌드박스에 적용된다).
            let hosts = INDEX_HOSTS.join(",");
            let (ok, text) = self
                .run(&[
                    "policy",
                    "allow",
                    "network",
                    "--sandbox",
                    &self.name,
                    &hosts,
                ])
                .await
                // 답을 못 받았어도 규칙은 걸렸을 수 있다: 아래에서 치운다.
                .unwrap_or_else(|why| (false, why));
            let ids = rule_ids(&text);
            if !ok || ids.is_empty() {
                // 열렸는지 알 수 없으면 남기지 않는다.
                let _ = self.sweep().await;
                return Err("패키지 색인으로 나가는 길을 열지 못했다".into());
            }
            Ok(ids)
        })
    }

    fn close<'a>(&'a self, ids: &'a [String]) -> BoxFut<'a, ()> {
        Box::pin(async move {
            for id in ids {
                let done = self
                    .run(&[
                        "policy",
                        "rm",
                        "network",
                        "--sandbox",
                        &self.name,
                        "--id",
                        id,
                        "--force",
                    ])
                    .await;
                if !matches!(done, Ok((true, _))) {
                    tracing::warn!(rule = %id, sandbox = %self.name, "패키지 색인 허용 규칙을 지우지 못했다");
                }
            }
        })
    }

    fn sweep(&self) -> BoxFut<'_, Result<usize, String>> {
        Box::pin(async move {
            let (ok, text) = self.run(&["policy", "ls", &self.name, "--json"]).await?;
            if !ok {
                return Err("샌드박스의 네트워크 규칙을 읽지 못했다".into());
            }
            // 안내 줄이 섞일 수 있어 JSON이 시작하는 곳부터 읽는다.
            let json = text.find('{').map_or("", |i| &text[i..]);
            let (ids, kept) = allow_rules(json, &self.name, self.keep.as_deref());
            self.close(&ids).await;
            // 출구 프록시로 가는 길(결정 0058)은 늘 있어야 한다: 없으면 건다(샌드박스를 다시 만든 뒤 등).
            if let (Some(target), false) = (&self.keep, kept) {
                let added = self
                    .run(&[
                        "policy",
                        "allow",
                        "network",
                        "--sandbox",
                        &self.name,
                        target,
                    ])
                    .await;
                match added {
                    Ok((true, _)) => {
                        tracing::info!(sandbox = %self.name, target, "스크립트 출구 프록시로 가는 허용 규칙을 걸었다");
                    }
                    other => {
                        tracing::warn!(sandbox = %self.name, target, ?other, "스크립트 출구 프록시로 가는 허용 규칙을 걸지 못했다");
                    }
                }
            }
            Ok(ids.len())
        })
    }

    fn install<'a>(&'a self, specs: &'a [String]) -> BoxFut<'a, Result<(bool, String), String>> {
        Box::pin(async move {
            let secs = INSTALL_TIMEOUT.as_secs().to_string();
            // 스크립트는 `python3 -I`로 돈다(사용자 자리의 패키지를 보지 않는다). 그래서 시스템 자리에 설치한다.
            let mut args = vec![
                "exec",
                &self.name,
                "timeout",
                "-s",
                "KILL",
                "-k",
                "2",
                &secs,
                "sudo",
                "python3",
                "-m",
                "pip",
                "install",
                "--no-cache-dir",
                "--disable-pip-version-check",
                "--no-input",
                "--break-system-packages",
                "--prefer-binary",
                "--",
            ];
            args.extend(specs.iter().map(String::as_str));
            self.run_for(&args, INSTALL_HOST_TIMEOUT).await
        })
    }

    fn list(&self) -> BoxFut<'_, Result<Vec<(String, String)>, String>> {
        Box::pin(async move {
            let (ok, text) = self
                .run(&[
                    "exec",
                    &self.name,
                    "python3",
                    "-I",
                    "-m",
                    "pip",
                    "list",
                    "--format=json",
                    "--disable-pip-version-check",
                ])
                .await?;
            parse_list(&text)
                .filter(|_| ok)
                .ok_or_else(|| "설치된 패키지를 읽지 못했다".into())
        })
    }
}

fn tail(text: &str) -> String {
    let clean = crate::mask::global().mask(text.trim());
    let count = clean.chars().count();
    if count <= OUTPUT_TAIL {
        return clean;
    }
    let cut: String = clean.chars().skip(count - OUTPUT_TAIL).collect();
    format!("…{cut}")
}

/// 설치 하나의 결과(모델에게 돌려줄 글).
pub type Outcome = Result<String, String>;

pub struct Packages {
    sandbox: Arc<dyn Sandbox>,
    store: Arc<Mutex<Store>>,
    /// 지금 도는 설치: (패키지들, 끝나면 결과).
    current: Mutex<Option<(String, watch::Receiver<Option<Outcome>>)>>,
}

impl Packages {
    pub fn new(sandbox: Arc<dyn Sandbox>, store: Arc<Mutex<Store>>) -> Arc<Self> {
        Arc::new(Self {
            sandbox,
            store,
            current: Mutex::new(None),
        })
    }

    /// 데몬이 뜰 때: 앞선 실행이 설치 중에 죽어 남긴 허용 규칙을 치운다.
    pub async fn sweep(&self) {
        match self.sandbox.sweep().await {
            Ok(0) => {}
            Ok(n) => tracing::warn!(
                n,
                "스크립트 샌드박스에 남아 있던 네트워크 허용 규칙을 지웠다"
            ),
            Err(err) => tracing::warn!(err, "스크립트 샌드박스의 네트워크 규칙을 확인하지 못했다"),
        }
    }

    /// 설치를 시작하고 결과를 받을 통로를 돌려준다. 표기가 잘못됐거나 설치 중이면 까닭.
    pub fn start(
        self: &Arc<Self>,
        specs: &[String],
        run_id: Option<&str>,
    ) -> Result<watch::Receiver<Option<Outcome>>, String> {
        let specs: Vec<String> = specs
            .iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        if specs.is_empty() {
            return Err("설치할 패키지가 없다.".into());
        }
        if specs.len() > MAX_SPECS {
            return Err(format!("한 번에 {MAX_SPECS}개까지 설치한다."));
        }
        if let Some(bad) = specs.iter().find(|s| !valid_spec(s)) {
            return Err(format!(
                "받지 않는 표기다: {bad}. PyPI 패키지 이름과 버전 조건만 쓴다(예: pillow, numpy==2.3.0). 주소·경로·옵션은 받지 않는다."
            ));
        }
        let label = specs.join(" ");
        let mut current = self.current.lock().expect("packages lock");
        if let Some((doing, rx)) = current.as_ref()
            && rx.borrow().is_none()
        {
            return Err(format!(
                "이미 설치하고 있다({doing}). 끝난 뒤에 다시 한다(lib_list로 확인)."
            ));
        }
        let (tx, rx) = watch::channel(None);
        *current = Some((label.clone(), rx.clone()));
        let this = self.clone();
        let run_id = run_id.map(str::to_owned);
        tokio::spawn(async move {
            let outcome = this.install(&specs).await;
            let summary = match &outcome {
                Ok(_) => format!("패키지 설치: {label}"),
                Err(_) => format!("패키지 설치 실패: {label}"),
            };
            if let Ok(store) = this.store.lock() {
                let _ = store.add_activity(
                    "lib_install",
                    &summary,
                    run_id.as_deref(),
                    chrono::Utc::now(),
                );
            }
            tracing::info!(packages = %label, ok = outcome.is_ok(), "스크립트 패키지 설치 끝");
            let _ = tx.send(Some(outcome));
        });
        Ok(rx)
    }

    /// 길을 열고, 설치하고, 닫는다. 설치가 실패하거나 `sbx`가 멈춰도 닫는다.
    async fn install(&self, specs: &[String]) -> Outcome {
        // 앞선 설치가 닫지 못한 규칙이 있으면 먼저 치운다.
        let _ = self.sandbox.sweep().await;
        let ids = self.sandbox.open().await?;
        let done = self.sandbox.install(specs).await;
        self.sandbox.close(&ids).await;
        match done {
            Ok((true, out)) => Ok(format!(
                "설치했다: {}. code_run에서 import해 쓴다.\n{}",
                specs.join(", "),
                tail(&out)
            )),
            Ok((false, out)) => Err(format!(
                "설치하지 못했다: {}.\n{}",
                specs.join(", "),
                tail(&out)
            )),
            Err(why) => Err(why),
        }
    }

    pub async fn list(&self) -> Result<Vec<(String, String)>, String> {
        self.sandbox.list().await
    }

    /// 지금 설치 중인 것(없으면 `None`).
    pub fn installing(&self) -> Option<String> {
        let current = self.current.lock().expect("packages lock");
        current
            .as_ref()
            .filter(|(_, rx)| rx.borrow().is_none())
            .map(|(label, _)| label.clone())
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// 가짜 샌드박스: 한 일을 순서대로 적고, 정해 둔 결과를 (정해 둔 만큼 늦게) 돌려준다.
    #[derive(Default)]
    pub struct Fake {
        pub log: Mutex<Vec<String>>,
        pub fail: Mutex<bool>,
        pub delay: Mutex<Duration>,
        pub leftover: Mutex<usize>,
    }

    impl Sandbox for Fake {
        fn open(&self) -> BoxFut<'_, Result<Vec<String>, String>> {
            Box::pin(async move {
                self.log.lock().unwrap().push("open".into());
                Ok(vec!["rule-1".into(), "rule-2".into()])
            })
        }

        fn close<'a>(&'a self, ids: &'a [String]) -> BoxFut<'a, ()> {
            Box::pin(async move {
                self.log
                    .lock()
                    .unwrap()
                    .push(format!("close {}", ids.join(",")));
            })
        }

        fn sweep(&self) -> BoxFut<'_, Result<usize, String>> {
            Box::pin(async move {
                self.log.lock().unwrap().push("sweep".into());
                Ok(std::mem::take(&mut *self.leftover.lock().unwrap()))
            })
        }

        fn install<'a>(
            &'a self,
            specs: &'a [String],
        ) -> BoxFut<'a, Result<(bool, String), String>> {
            Box::pin(async move {
                self.log
                    .lock()
                    .unwrap()
                    .push(format!("install {}", specs.join(" ")));
                let delay = *self.delay.lock().unwrap();
                tokio::time::sleep(delay).await;
                if *self.fail.lock().unwrap() {
                    Ok((false, "ERROR: No matching distribution found".into()))
                } else {
                    Ok((true, "Successfully installed".into()))
                }
            })
        }

        fn list(&self) -> BoxFut<'_, Result<Vec<(String, String)>, String>> {
            Box::pin(async move { Ok(vec![("pillow".into(), "13.0.0".into())]) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Fake;
    use super::*;

    async fn wait(mut rx: watch::Receiver<Option<Outcome>>) -> Outcome {
        rx.wait_for(|v| v.is_some()).await.unwrap().clone().unwrap()
    }

    #[test]
    fn specs_are_names_and_versions_only() {
        for ok in [
            "pillow",
            "Pillow",
            "numpy==2.3.0",
            "requests[socks]>=2.31",
            "scikit-learn~=1.5",
            "a",
            "ruamel.yaml",
            "pandas>=2,<3",
        ] {
            assert!(valid_spec(ok), "{ok}");
        }
        for bad in [
            "",
            "-r requirements.txt",
            "--index-url=https://evil.example",
            "git+https://github.com/x/y",
            "https://example.com/x.whl",
            "./local",
            "../x",
            "pillow; rm -rf /",
            "pillow numpy",
            "pillow @ https://example.com/x.whl",
            "-pillow",
            "pkg==",
        ] {
            assert!(!valid_spec(bad), "{bad}");
        }
    }

    #[test]
    fn sbx_outputs_are_read() {
        assert_eq!(
            rule_ids(
                "Added rule 729b2c89-3c4c-4b59-86c0-27734998e4fa\n006905e1-864b-452c-97f4-963d2007f8e6"
            ),
            [
                "729b2c89-3c4c-4b59-86c0-27734998e4fa",
                "006905e1-864b-452c-97f4-963d2007f8e6"
            ]
        );
        let ls = r#"{"rules": [
            {"id": "a", "scope": "sandbox:lucia-script", "resource_type": "network", "decision": "allow", "editable": true},
            {"id": "b", "scope": "sandbox:lucia-script", "resource_type": "network", "decision": "deny", "editable": true},
            {"id": "c", "scope": "sandbox:lucia", "resource_type": "network", "decision": "allow", "editable": true},
            {"id": "d", "scope": "global", "resource_type": "network", "decision": "allow", "editable": false}
        ]}"#;
        assert_eq!(
            allow_rules(ls, "lucia-script", None),
            (vec!["a".to_owned()], false),
            "이 샌드박스의 허용 규칙만"
        );
        assert!(allow_rules("안내 글", "lucia-script", None).0.is_empty());
        // 출구 프록시로 가는 규칙(결정 0058)은 남긴다: 대상이 정확히 그 포트 하나인 것 하나만.
        let ls = r#"{"rules": [
            {"id": "p", "scope": "sandbox:lucia-script", "resource_type": "network", "decision": "allow", "editable": true, "resources": ["localhost:47812"]},
            {"id": "q", "scope": "sandbox:lucia-script", "resource_type": "network", "decision": "allow", "editable": true, "resources": ["pypi.org", "files.pythonhosted.org"]},
            {"id": "r", "scope": "sandbox:lucia-script", "resource_type": "network", "decision": "allow", "editable": true, "resources": ["localhost:47812", "example.com"]},
            {"id": "s", "scope": "sandbox:lucia-script", "resource_type": "network", "decision": "allow", "editable": true, "resources": ["localhost:47812"]},
            {"id": "t", "scope": "sandbox:lucia-script", "resource_type": "network", "decision": "deny", "editable": true, "resources": ["localhost:47810"]}
        ]}"#;
        assert_eq!(
            allow_rules(ls, "lucia-script", Some("localhost:47812")),
            (vec!["q".to_owned(), "r".to_owned(), "s".to_owned()], true)
        );
        assert_eq!(
            allow_rules(ls, "lucia-script", Some("localhost:47822")),
            (
                vec![
                    "p".to_owned(),
                    "q".to_owned(),
                    "r".to_owned(),
                    "s".to_owned()
                ],
                false
            ),
            "다른 포트의 규칙은 남기지 않는다(없으면 새로 건다)"
        );
        assert_eq!(allow_rules(ls, "lucia-script", None).0.len(), 4);
        // 한 샌드박스의 규칙 하나만 고른다(상주 샌드박스의 프록시 규칙, 결정 0059).
        assert_eq!(rules_for(ls, "lucia-script", "localhost:47812"), ["p", "s"]);
        assert!(rules_for(ls, "lucia", "localhost:47812").is_empty());
        assert!(rules_for(ls, "lucia-script", "pypi.org").is_empty());
        let listed = parse_list(
            "  ✓ sandbox lucia-script — started\n[{\"name\": \"pillow\", \"version\": \"13.0.0\"}, {\"name\": \"pip\", \"version\": \"26.3\"}]\n",
        )
        .unwrap();
        assert_eq!(listed[0], ("pillow".to_owned(), "13.0.0".to_owned()));
        assert!(parse_list("오류").is_none());
    }

    /// `sbx`가 멈춰도 기다리다 끊는다(그래야 길을 닫는 데까지 간다).
    #[tokio::test]
    async fn a_stuck_sbx_is_cut_off() {
        let stuck = SbxSandbox {
            keep: None,
            sbx: "sleep".into(),
            name: "lucia-script".into(),
        };
        let started = std::time::Instant::now();
        let out = stuck.run_for(&["30"], Duration::from_millis(200)).await;
        assert!(out.unwrap_err().contains("안에 답하지 않았다"));
        assert!(started.elapsed() < Duration::from_secs(5));
        let missing = SbxSandbox {
            keep: None,
            sbx: "/nonexistent/sbx".into(),
            name: "lucia-script".into(),
        };
        assert!(
            missing
                .run(&["ls"])
                .await
                .unwrap_err()
                .contains("실행하지 못했다")
        );
    }

    #[tokio::test]
    async fn install_opens_the_index_only_while_installing() {
        let fake = Arc::new(Fake::default());
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let packages = Packages::new(fake.clone(), store.clone());
        // 표기가 잘못되면 길을 열지도 않는다.
        for bad in [vec![], vec!["--index-url=https://evil.example".to_owned()]] {
            assert!(packages.start(&bad, None).is_err());
        }
        assert!(
            packages
                .start(&vec!["a".to_owned(); MAX_SPECS + 1], None)
                .unwrap_err()
                .contains("8개까지")
        );
        assert!(fake.log.lock().unwrap().is_empty());
        // 성공: 열고 → 설치하고 → 닫는다. 활동 기록이 남는다.
        let rx = packages
            .start(&[" pillow ".into(), "numpy==2.3.0".into()], Some("chat-1"))
            .unwrap();
        let out = wait(rx).await.unwrap();
        assert!(out.contains("설치했다: pillow, numpy==2.3.0") && out.contains("import"));
        assert_eq!(
            *fake.log.lock().unwrap(),
            [
                "sweep",
                "open",
                "install pillow numpy==2.3.0",
                "close rule-1,rule-2"
            ]
        );
        assert!(
            store
                .lock()
                .unwrap()
                .recent_activity(3)
                .unwrap()
                .iter()
                .any(|a| a.kind == "lib_install" && a.summary.contains("pillow"))
        );
        // 실패해도 닫는다.
        fake.log.lock().unwrap().clear();
        *fake.fail.lock().unwrap() = true;
        let err = wait(packages.start(&["no-such-pkg".into()], None).unwrap())
            .await
            .unwrap_err();
        assert!(err.contains("설치하지 못했다") && err.contains("No matching distribution"));
        assert_eq!(
            fake.log.lock().unwrap().last().unwrap(),
            "close rule-1,rule-2"
        );
        // 설치 중에는 새 설치를 받지 않는다.
        *fake.fail.lock().unwrap() = false;
        *fake.delay.lock().unwrap() = Duration::from_millis(300);
        let slow = packages.start(&["pandas".into()], None).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(packages.installing().as_deref(), Some("pandas"));
        assert!(
            packages
                .start(&["scipy".into()], None)
                .unwrap_err()
                .contains("이미 설치하고 있다(pandas)")
        );
        assert!(wait(slow).await.is_ok());
        assert!(packages.installing().is_none());
        // 데몬이 뜰 때 남은 허용 규칙을 치운다.
        *fake.leftover.lock().unwrap() = 2;
        packages.sweep().await;
        assert_eq!(fake.log.lock().unwrap().last().unwrap(), "sweep");
        assert_eq!(packages.list().await.unwrap()[0].0, "pillow");
    }
}
