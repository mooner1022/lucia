//! `luciad script-check`(결정 0025): 실제 스크립트 샌드박스를 점검한다. 모델을 부르지 않고, 데몬 MCP 서버에도
//! 붙지 않는다(도구 다리는 정해 둔 답을 주는 가짜). 운영 데몬과 함께 돌려도 된다.
//!
//! 1. 코드가 돌고 도구 다리가 오간다(Python 버전).
//! 2. 샌드박스에서 바깥(공인 주소, 데몬 MCP 포트, 관리 페이지)에 닿지 않는다.
//! 3. 시간을 넘긴 코드가 끝나고, VM 안에 남지 않는다.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::{Launcher, SbxLauncher, ScriptJob, ScriptRunner, ToolBridge, ToolSession};
use crate::browser::BoxFut;
use crate::config::Config;

struct Echo;

struct EchoSession;

impl ToolBridge for Echo {
    fn session(&self, _run: String, _grants: crate::tools::Policy) -> Box<dyn ToolSession> {
        Box::new(EchoSession)
    }
}

impl ToolSession for EchoSession {
    fn call<'a>(
        &'a mut self,
        name: &'a str,
        args: Map<String, Value>,
    ) -> BoxFut<'a, Result<String, String>> {
        Box::pin(async move {
            Ok(match name {
                "mail_search" => json!([{"id": "m1", "subject": "점검", "args": args}]).to_string(),
                _ => format!("{name} 결과"),
            })
        })
    }

    fn close(&mut self) -> BoxFut<'_, ()> {
        Box::pin(async {})
    }
}

fn job(code: &str, timeout: Duration) -> ScriptJob {
    ScriptJob {
        code: code.to_owned(),
        input: json!({}),
        state: json!({}),
        grants: crate::routines::script_policy(),
        run: "script-check".into(),
        declared: None,
        fetch: false,
        fetch_hosts: None,
        session_hosts: None,
        files: false,
        net: false,
        timeout,
    }
}

// sbx는 TCP 연결을 받아 둔 뒤 정책으로 끊는다(2026-11-17 실측: 연결은 되고 응답이 없다). 그래서 응답이 오는지로 본다.
const NETWORK: &str = r#"
import socket, ssl
reached = []
def http(host, port):
    s = socket.create_connection((host, port), timeout=5)
    s.sendall(f"GET / HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n".encode())
    data = s.recv(64)
    s.close()
    return bool(data)
for host, port in [("1.1.1.1", 80), ("example.com", 80), ("host.docker.internal", 47810),
                   ("host.docker.internal", 8780), ("localhost", 47810)]:
    try:
        if http(host, port):
            reached.append(f"{host}:{port}")
    except OSError:
        pass
try:
    with socket.create_connection(("1.1.1.1", 443), timeout=5) as s:
        with ssl.create_default_context().wrap_socket(s, server_hostname="one.one.one.one"):
            reached.append("1.1.1.1:443 TLS")
except (OSError, ssl.SSLError):
    pass
result = reached
"#;

// 출구 프록시(결정 0058): 통행증이 있는 실행에서 실제 클라이언트들이 나가는지, 내부 주소가 막히는지,
// 통행증을 뗀 요청이 거절되는지.
const ONLINE: &str = r#"
import os, subprocess, sys, urllib.request, urllib.error
out = {"online": lucia.online}
try:
    out["urllib"] = urllib.request.urlopen("https://www.rust-lang.org/", timeout=20).status
except Exception as e:
    out["urllib"] = repr(e)[:160]
def sh(name, cmd, env=None, t=60):
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=t, env=env)
        out[name] = [p.returncode, (p.stdout + p.stderr).strip()[-200:]]
    except Exception as e:
        out[name] = [-1, repr(e)[:160]]
code = ["curl", "-sS", "-m", "15", "-o", "/dev/null", "-w", "%{http_code}"]
sh("curl", code + ["https://example.com/"])
sh("http", code + ["http://example.com/"])
sh("private", code + ["https://192.168.0.1/"])
sh("daemon", code + ["http://host.docker.internal:8780/"])
sh("git", ["git", "ls-remote", "https://github.com/octocat/Hello-World", "HEAD"])
sh("pip", [sys.executable, "-m", "pip", "download", "--no-deps", "-q", "-d", "/tmp/lucia-check-pip",
           "--disable-pip-version-check", "idna"], t=90)
subprocess.run(["rm", "-rf", "/tmp/lucia-check-pip"])
bare = dict(os.environ)
proxy = bare["HTTPS_PROXY"]
for k in list(bare):
    if k.lower().endswith("_proxy") and k.lower() != "no_proxy":
        bare[k] = "http://" + proxy.split("@", 1)[1]
sh("no_pass", code + ["https://example.com/"], env=bare)
result = out
"#;

const LEFTOVER: &str = r#"
import os
me = str(os.getpid())
left = []
for pid in os.listdir("/proc"):
    if not pid.isdigit() or pid == me:
        continue
    try:
        cmd = open(f"/proc/{pid}/cmdline", "rb").read().replace(b"\0", b" ").decode(errors="replace")
    except OSError:
        continue
    if "lucia-run-" in cmd and "while True" in cmd:
        left.append(pid)
result = left
"#;

pub async fn run(cfg: &Config) -> anyhow::Result<()> {
    anyhow::ensure!(
        cfg.scripts.enabled,
        "설정에서 스크립트가 꺼져 있다([scripts] enabled = true, sandbox = \"{}\")",
        cfg.scripts.sandbox
    );
    let launcher: Arc<dyn Launcher> = Arc::new(SbxLauncher {
        sbx: cfg.sandbox.sbx.clone(),
        sandbox: cfg.scripts.sandbox.clone(),
    });
    let runner = ScriptRunner::new(launcher, Arc::new(Echo), None);
    let mut failed = 0;
    let mut report = |ok: bool, what: &str, detail: String| {
        println!("{} {what}: {detail}", if ok { "✓" } else { "✗" });
        if !ok {
            failed += 1;
        }
    };

    let out = runner
        .run(job(
            "import sys\nmails = lucia.mail_search('점검', limit=1)\nprint(sys.version.split()[0])\nresult = mails[0]['subject']",
            Duration::from_secs(60),
        ))
        .await;
    report(
        out.ok && out.result == Some(json!("점검")),
        "실행과 도구 다리",
        format!(
            "Python {} · {:.1}초{}",
            out.printed.trim(),
            out.elapsed.as_secs_f64(),
            out.error.map(|e| format!(" · {e}")).unwrap_or_default()
        ),
    );

    let out = runner.run(job(NETWORK, Duration::from_secs(40))).await;
    report(
        out.ok && out.result == Some(json!([])),
        "네트워크 막힘",
        match (&out.result, &out.error) {
            (_, Some(e)) => e.clone(),
            (Some(Value::Array(a)), _) if a.is_empty() => {
                "공인 주소·데몬 포트 모두 닿지 않음".into()
            }
            (r, _) => format!("닿은 곳: {r:?}"),
        },
    );

    let out = runner
        .run(job("while True:\n    pass\n", Duration::from_secs(2)))
        .await;
    let stopped = !out.ok && out.elapsed < Duration::from_secs(15);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let left = runner.run(job(LEFTOVER, Duration::from_secs(20))).await;
    report(
        stopped && left.ok && left.result == Some(json!([])),
        "시간 초과 끝내기",
        format!(
            "{:.1}초에 멈춤({}) · VM 안 남은 프로세스: {}",
            out.elapsed.as_secs_f64(),
            out.error.unwrap_or_default().lines().next().unwrap_or(""),
            left.result
                .map(|v| v.to_string())
                .unwrap_or_else(|| left.error.unwrap_or_default())
        ),
    );

    // 출구 프록시(결정 0058): 이 점검이 프록시를 직접 띄운다. 데몬이 같은 설정으로 돌고 있으면 포트를 잡지 못하므로
    // 건너뛴다(그때는 `luciad check`의 "스크립트 출구 프록시 포트"와 실제 대화로 본다).
    if cfg.scripts.net {
        let net = super::net::ScriptNet::new(crate::browser::proxy::Proxy::system(Arc::new(
            crate::browser::egress::Egress::default(),
        )));
        let cancel = tokio_util::sync::CancellationToken::new();
        match net
            .clone()
            .serve(cfg.scripts.proxy_bind, cancel.clone())
            .await
        {
            Err(err) => println!(
                "- 출구 프록시: 건너뜀({}를 잡지 못했다: {err}. 데몬이 돌고 있으면 정상이다)",
                cfg.scripts.proxy_bind
            ),
            Ok(_) => {
                let online = ScriptRunner::new(
                    Arc::new(SbxLauncher {
                        sbx: cfg.sandbox.sbx.clone(),
                        sandbox: cfg.scripts.sandbox.clone(),
                    }),
                    Arc::new(Echo),
                    None,
                )
                .with_net(Some((
                    net,
                    format!("{}:{}", cfg.mcp.host_alias, cfg.scripts.proxy_bind.port()),
                )));
                let mut with_pass = job(ONLINE, Duration::from_secs(170));
                with_pass.net = true;
                let out = online.run(with_pass).await;
                let r = out.result.clone().unwrap_or_default();
                let code = |name: &str, want: &str| {
                    r[name][0] == json!(0) && r[name][1].as_str() == Some(want)
                };
                // 터널(https)은 curl이 실패로 끝나고, 평문 요청은 403 응답이 온다.
                let refused = |name: &str, status: &str| {
                    r[name][1].as_str().is_some_and(|t| t.contains(status))
                };
                let ok = out.ok
                    && r["online"] == json!(true)
                    && r["urllib"] == json!(200)
                    && code("curl", "200")
                    && code("http", "200")
                    && refused("private", "403")
                    && refused("daemon", "403")
                    && r["git"][0] == json!(0)
                    && r["pip"][0] == json!(0)
                    && refused("no_pass", "407");
                report(
                    ok,
                    "출구 프록시(통행증)",
                    if ok {
                        format!(
                            "urllib·curl·git·pip가 나감({}) · 내부 주소 403 · 통행증 없으면 407 · {:.1}초",
                            out.net_hosts.join(", "),
                            out.elapsed.as_secs_f64()
                        )
                    } else {
                        format!("{r} {}", out.error.unwrap_or_default())
                    },
                );
                // 통행증을 주지 않은 실행(루틴, 사람 없는 실행)은 프록시에 닿아도 나가지 못한다.
                let mut no_pass = job(
                    "import subprocess, os\nenv = dict(os.environ)\nfor k in list(env):\n    if k.lower().endswith('_proxy') and k.lower() != 'no_proxy':\n        env[k] = 'http://' + lucia.input['daemon']\np = subprocess.run(['curl', '-sS', '-m', '15', '-o', '/dev/null', '-w', '%{http_code}', 'https://example.com/'], capture_output=True, text=True, env=env)\nresult = [lucia.online, p.returncode, (p.stdout + p.stderr)[-120:]]",
                    Duration::from_secs(40),
                );
                no_pass.input = json!({"daemon": format!("{}:{}", cfg.mcp.host_alias, cfg.scripts.proxy_bind.port())});
                let without = online.run(no_pass).await;
                let r = without.result.clone().unwrap_or_default();
                let ok = without.ok
                    && r[0] == json!(false)
                    && r[1] != json!(0)
                    && r[2].as_str().is_some_and(|t| t.contains("407"));
                report(
                    ok,
                    "통행증 없는 실행",
                    if ok {
                        "프록시에 닿아도 407로 거절된다".into()
                    } else {
                        format!("{r} {}", without.error.unwrap_or_default())
                    },
                );
                cancel.cancel();
            }
        }
    }

    // 큰 파일(결정 0057): 상한에 가까운 파일을 조각으로 읽고, 상한에 가까운 파일을 만들어 남긴다.
    // 임시 보관소를 쓴다(운영·개발 파일 보관소와 저장소는 건드리지 않는다).
    let dir = std::env::temp_dir().join(format!(
        "lucia-script-check-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let files = Arc::new(crate::files::FileStore::new(
        dir.clone(),
        reqwest::Client::new(),
        Arc::new(std::sync::Mutex::new(lucia_store::Store::open_in_memory()?)),
    ));
    let big: Vec<u8> = (0..super::FILE_READ_MAX - 1024 * 1024)
        .map(|i| (i % 251) as u8)
        .collect();
    let source = files
        .save_web("big.bin", &big, None, chrono::Utc::now())
        .map_err(|e| anyhow::anyhow!(e))?;
    let with_files = ScriptRunner::new(
        Arc::new(SbxLauncher {
            sbx: cfg.sandbox.sbx.clone(),
            sandbox: cfg.scripts.sandbox.clone(),
        }),
        Arc::new(Echo),
        None,
    )
    .with_files(Some(files.clone()));
    let mut read_big = job(
        "import hashlib\ndata = lucia.read_file(lucia.input['id'])\nmade = lucia.save_file('made.bin', data[:lucia.input['save']])\nresult = [len(data), hashlib.sha256(data).hexdigest(), made]",
        Duration::from_secs(170),
    );
    read_big.files = true;
    let save_len = super::FILE_SAVE_MAX - 1024 * 1024;
    read_big.input = json!({"id": source.id, "save": save_len});
    let out = with_files.run(read_big).await;
    let made_ok = out
        .result
        .as_ref()
        .and_then(|r| r[2].as_str())
        .and_then(|id| files.get(id))
        .is_some_and(|f| {
            f.size == save_len as u64 && files.read(&f).is_ok_and(|b| b == big[..save_len])
        });
    report(
        out.ok
            && out.result.as_ref().is_some_and(|r| {
                r[0] == json!(big.len()) && r[1].as_str() == Some(source.sha256.as_str())
            })
            && made_ok,
        "큰 파일 읽기·만들기",
        format!(
            "{} 읽기(조각 {}개) · {} 만들기 · {:.1}초{}",
            crate::files::human_size(big.len() as u64),
            big.len().div_ceil(super::READ_CHUNK),
            crate::files::human_size(save_len as u64),
            out.elapsed.as_secs_f64(),
            out.error.map(|e| format!(" · {e}")).unwrap_or_default()
        ),
    );
    let _ = std::fs::remove_dir_all(&dir);

    anyhow::ensure!(failed == 0, "{failed}개 항목 실패");
    Ok(())
}
