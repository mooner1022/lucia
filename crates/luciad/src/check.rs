//! 시작 시 점검(명세 원칙 8: 조용한 실패는 없다). 모델을 부르지 않는다.
//!
//! - sbx 데몬이 kvm 그룹으로 돌고 있는가(아니면 VM을 못 만든다. PoC P1의 함정)
//! - sbx 진단: 가상화, Docker 인증, 전역 네트워크 정책
//! - Claude 로그인(sbx 전역 시크릿 `anthropic`)
//! - 상주 샌드박스가 있고, 데몬 MCP 포트가 그 샌드박스 정책에서 허용되는가
//! - 저장소를 열 수 있는가

use std::process::Command;

use crate::config::Config;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

fn check(name: &'static str, ok: bool, detail: impl Into<String>) -> Check {
    Check {
        name,
        ok,
        detail: detail.into(),
    }
}

fn run(program: &std::path::Path, args: &[&str]) -> (bool, String) {
    match Command::new(program).args(args).output() {
        Ok(out) => (
            out.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        ),
        Err(err) => (false, format!("실행 실패: {err}")),
    }
}

/// kvm 그룹 ID(`/etc/group`).
fn kvm_gid() -> Option<u32> {
    std::fs::read_to_string("/etc/group")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("kvm:"))
        .and_then(|rest| rest.split(':').nth(1))
        .and_then(|gid| gid.parse().ok())
}

/// sbx 데몬 프로세스의 GID 목록(실효 GID와 보조 그룹).
fn sbx_daemon_gids() -> Option<Vec<u32>> {
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
        let is_daemon = args.len() >= 3
            && args[0].ends_with(b"sbx")
            && args[1] == b"daemon"
            && args[2] == b"start";
        if !is_daemon {
            continue;
        }
        let status = std::fs::read_to_string(entry.path().join("status")).ok()?;
        let mut gids = Vec::new();
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Gid:") {
                gids.extend(
                    rest.split_whitespace()
                        .filter_map(|g| g.parse::<u32>().ok()),
                );
            }
            if let Some(rest) = line.strip_prefix("Groups:") {
                gids.extend(
                    rest.split_whitespace()
                        .filter_map(|g| g.parse::<u32>().ok()),
                );
            }
        }
        return Some(gids);
    }
    None
}

pub fn run_checks(cfg: &Config) -> Vec<Check> {
    let sbx = &cfg.sandbox.sbx;
    let mut checks = Vec::new();

    match (kvm_gid(), sbx_daemon_gids()) {
        (Some(kvm), Some(gids)) => checks.push(check(
            "sbx 데몬 kvm 권한",
            gids.contains(&kvm),
            if gids.contains(&kvm) {
                "데몬이 kvm 그룹으로 돈다".to_owned()
            } else {
                "데몬에 kvm 그룹이 없다: `sbx daemon stop` 후 `sg kvm -c 'sbx ls'`".to_owned()
            },
        )),
        (_, None) => checks.push(check(
            "sbx 데몬 kvm 권한",
            false,
            "sbx 데몬이 없다. kvm 그룹이 있는 셸에서 띄운다(`sg kvm -c 'sbx ls'`)",
        )),
        (None, _) => checks.push(check("sbx 데몬 kvm 권한", false, "kvm 그룹을 찾을 수 없다")),
    }

    let (_, diag) = run(sbx, &["diagnose"]);
    for (name, key) in [
        ("sbx Docker 인증", "Authentication"),
        ("sbx 전역 네트워크 정책", "Global network policy"),
    ] {
        let line = diag
            .lines()
            .find(|l| l.contains(key))
            .unwrap_or("")
            .trim()
            .to_owned();
        checks.push(check(name, line.starts_with('✓'), line));
    }

    let (_, secrets) = run(sbx, &["secret", "ls"]);
    let claude = secrets
        .lines()
        .any(|l| l.contains("anthropic") && l.contains("configured"));
    checks.push(check(
        "Claude 로그인(sbx 시크릿)",
        claude,
        if claude {
            "anthropic 시크릿 있음".to_owned()
        } else {
            "임시 샌드박스에서 /login 후 그 샌드박스를 지운다(PoC P1)".to_owned()
        },
    ));

    let (_, list) = run(sbx, &["ls", "-q"]);
    let exists = list.lines().any(|l| l.trim() == cfg.sandbox.name);
    checks.push(check(
        "상주 샌드박스",
        exists,
        if exists {
            format!("{} 있음", cfg.sandbox.name)
        } else {
            format!("{} 없음: deploy/create-sandbox.sh", cfg.sandbox.name)
        },
    ));
    if exists {
        let port = format!("localhost:{}", cfg.mcp.bind.port());
        let (_, policy) = run(
            sbx,
            &[
                "policy",
                "check",
                "network",
                &port,
                "--sandbox",
                &cfg.sandbox.name,
            ],
        );
        let allowed = policy.trim_start().starts_with("Allowed");
        checks.push(check(
            "MCP 포트 허용",
            allowed,
            policy.lines().next().unwrap_or("").trim().to_owned(),
        ));

        // 호스트 쪽 통제(PoC P2·P4): 킷이 여는 계정 쪽·업데이트 주소와 허용 목록 밖 주소는 막혀야 한다.
        let leaks: Vec<&str> = [
            "mcp-proxy.anthropic.com:443",
            "bridge.claudeusercontent.com:443",
            "claude.com:443",
            "downloads.claude.ai:443",
            "example.com:443",
        ]
        .into_iter()
        .filter(|host| {
            let (_, out) = run(
                sbx,
                &[
                    "policy",
                    "check",
                    "network",
                    host,
                    "--sandbox",
                    &cfg.sandbox.name,
                ],
            );
            !out.trim_start().starts_with("Denied")
        })
        .collect();
        checks.push(check(
            "네트워크 거부 규칙",
            leaks.is_empty(),
            if leaks.is_empty() {
                "계정 쪽·업데이트·허용 목록 밖 주소가 막혀 있다".to_owned()
            } else {
                format!("막혀 있지 않다: {}", leaks.join(", "))
            },
        ));

        // 관리 페이지는 VM에서 닿으면 안 된다(MCP 포트만 허용).
        let reachable: Vec<String> = cfg
            .admin
            .bind
            .iter()
            .map(|addr| {
                if addr.ip().is_loopback() {
                    format!("localhost:{}", addr.port())
                } else {
                    addr.to_string()
                }
            })
            .filter(|target| {
                let (_, out) = run(
                    sbx,
                    &[
                        "policy",
                        "check",
                        "network",
                        target,
                        "--sandbox",
                        &cfg.sandbox.name,
                    ],
                );
                !out.trim_start().starts_with("Denied")
            })
            .collect();
        checks.push(check(
            "관리 페이지 VM 차단",
            reachable.is_empty(),
            if reachable.is_empty() {
                "VM에서 관리 페이지 주소로 갈 수 없다".to_owned()
            } else {
                format!("VM에서 닿을 수 있다: {}", reachable.join(", "))
            },
        ));

        if let Some(plugin) = &cfg.sandbox.plugin_dir {
            let (writable, _) = run(sbx, &["exec", &cfg.sandbox.name, "test", "-w", plugin]);
            checks.push(check(
                "plugin 읽기 전용",
                !writable,
                if writable {
                    format!("{plugin}에 쓸 수 있다: 읽기 전용 마운트가 아니다")
                } else {
                    format!("{plugin} 읽기 전용")
                },
            ));
        }

        // 캐릭터 파일(결정 0009): 관리 페이지가 고칠 호스트 쪽 파일을 마운트에서 찾고, VM에서는 읽기 전용인지.
        if let Some(vm_file) = &cfg.sandbox.character_file {
            let (_, json) = run(sbx, &["ls", "--json"]);
            let host = crate::character::host_path_from_ls_json(&json, &cfg.sandbox.name, vm_file);
            let detail = match &host {
                Some(path) => match std::fs::read_to_string(path) {
                    Ok(text) if text.len() <= crate::character::MAX_BYTES => Ok(format!(
                        "{} ({}바이트, 관리 페이지에서 고친다)",
                        path.display(),
                        text.len()
                    )),
                    Ok(text) => Err(format!(
                        "{}: {}바이트로 상한({})을 넘는다",
                        path.display(),
                        text.len(),
                        crate::character::MAX_BYTES
                    )),
                    Err(e) => Err(format!("{}: {e}", path.display())),
                },
                None => Err(format!("{vm_file}을 품은 마운트를 찾지 못했다")),
            };
            checks.push(check(
                "캐릭터 파일",
                detail.is_ok(),
                detail.unwrap_or_else(|e| e),
            ));
            let dir = std::path::Path::new(vm_file)
                .parent()
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            let (writable, _) = run(sbx, &["exec", &cfg.sandbox.name, "test", "-w", &dir]);
            checks.push(check(
                "캐릭터 읽기 전용",
                !writable,
                if writable {
                    format!("{dir}에 쓸 수 있다: 에이전트가 캐릭터를 고칠 수 있다")
                } else {
                    format!("{dir} 읽기 전용(Lucia는 고칠 수 없다)")
                },
            ));
        }
    }

    // 브라우저 샌드박스(결정 0006·0008): 있는지, 직접 나가는 길·Anthropic 쪽·데몬 포트가 막혀 있는지, 프록시 포트.
    if cfg.browser.enabled {
        let name = &cfg.browser.sandbox;
        let exists = list.lines().any(|l| l.trim() == name);
        checks.push(check(
            "브라우저 샌드박스",
            exists,
            if exists {
                format!("{name} 있음")
            } else {
                format!("{name} 없음: deploy/create-browser-sandbox.sh")
            },
        ));
        if exists {
            let mut targets = vec![
                "api.anthropic.com:443".to_owned(),
                "example.com:443".to_owned(),
                format!("localhost:{}", cfg.mcp.bind.port()),
            ];
            targets.extend(cfg.admin.bind.iter().map(|addr| {
                if addr.ip().is_loopback() {
                    format!("localhost:{}", addr.port())
                } else {
                    addr.to_string()
                }
            }));
            let open: Vec<String> = targets
                .into_iter()
                .filter(|target| {
                    let (_, out) = run(
                        sbx,
                        &["policy", "check", "network", target, "--sandbox", name],
                    );
                    !out.trim_start().starts_with("Denied")
                })
                .collect();
            checks.push(check(
                "브라우저 네트워크 격리",
                open.is_empty(),
                if open.is_empty() {
                    "직접 나가는 길, Anthropic 쪽, 데몬 포트가 막혀 있다(데몬 프록시로만 나간다)"
                        .to_owned()
                } else {
                    format!("막혀 있지 않다: {}", open.join(", "))
                },
            ));
            // 결정 0008: 브라우저는 프록시 포트에 닿고, 상주 샌드박스(에이전트)는 닿지 않는다.
            let proxy = format!("localhost:{}", cfg.browser.proxy_bind.port());
            let allowed = |sandbox: &str| {
                let (_, out) = run(
                    sbx,
                    &["policy", "check", "network", &proxy, "--sandbox", sandbox],
                );
                out.trim_start().starts_with("Allowed")
            };
            let browser_ok = allowed(name);
            let agent_blocked = !allowed(&cfg.sandbox.name);
            checks.push(check(
                "브라우저 프록시 포트",
                browser_ok && agent_blocked,
                match (browser_ok, agent_blocked) {
                    (true, true) => format!(
                        "{name}만 {proxy}에 닿는다({}는 막힘)",
                        cfg.sandbox.name
                    ),
                    (false, _) => format!(
                        "{name}이 {proxy}에 닿지 않는다: deploy/create-browser-sandbox.sh --policy-only"
                    ),
                    (true, false) => format!(
                        "상주 샌드박스 {}가 {proxy}에 닿는다(에이전트가 인터넷을 얻는다). 그 허용을 지운다",
                        cfg.sandbox.name
                    ),
                },
            ));
        }
    }

    // 스크립트 샌드박스(결정 0025): 있는지, 바깥·데몬 포트·브라우저 프록시가 모두 막혀 있는지(허용 규칙 없음).
    if cfg.scripts.enabled {
        let name = &cfg.scripts.sandbox;
        let exists = list.lines().any(|l| l.trim() == name);
        checks.push(check(
            "스크립트 샌드박스",
            exists,
            if exists {
                format!("{name} 있음")
            } else {
                format!("{name} 없음: deploy/create-script-sandbox.sh")
            },
        ));
        if exists {
            let mut targets = vec![
                "api.anthropic.com:443".to_owned(),
                "example.com:443".to_owned(),
                format!("localhost:{}", cfg.mcp.bind.port()),
                format!("localhost:{}", cfg.browser.proxy_bind.port()),
            ];
            targets.extend(cfg.admin.bind.iter().map(|addr| {
                if addr.ip().is_loopback() {
                    format!("localhost:{}", addr.port())
                } else {
                    addr.to_string()
                }
            }));
            let open: Vec<String> = targets
                .into_iter()
                .filter(|target| {
                    let (_, out) = run(
                        sbx,
                        &["policy", "check", "network", target, "--sandbox", name],
                    );
                    !out.trim_start().starts_with("Denied")
                })
                .collect();
            checks.push(check(
                "스크립트 네트워크 격리",
                open.is_empty(),
                if open.is_empty() {
                    "바깥, Anthropic 쪽, 데몬·브라우저 프록시·관리 페이지 포트가 모두 막혀 있다(바깥은 통행증이 있는 출구 프록시로만)"
                        .to_owned()
                } else {
                    format!(
                        "막혀 있지 않다: {}. deploy/create-script-sandbox.sh --policy-only",
                        open.join(", ")
                    )
                },
            ));
            // 결정 0058·0059: 출구 프록시 포트에는 스크립트 샌드박스와(코드의 통행증), 내장 도구를 연 상주 샌드박스만
            // (쉘의 통행증) 닿는다. 브라우저 샌드박스는 닿지 않는다. 통행증이 없으면 닿아도 나가지 못한다.
            let proxy = format!("localhost:{}", cfg.scripts.proxy_bind.port());
            let allowed = |sandbox: &str| {
                let (_, out) = run(
                    sbx,
                    &["policy", "check", "network", &proxy, "--sandbox", sandbox],
                );
                out.trim_start().starts_with("Allowed")
            };
            let mut wrong = Vec::new();
            let mut expect = |sandbox: &str, want: bool| {
                if allowed(sandbox) != want {
                    wrong.push(format!(
                        "{sandbox}{}",
                        if want {
                            "이 닿지 않는다"
                        } else {
                            "이 닿는다"
                        }
                    ));
                }
            };
            expect(name, cfg.scripts.net);
            expect(
                &cfg.sandbox.name,
                cfg.scripts.net && cfg.sandbox.native_tools,
            );
            if cfg.browser.enabled {
                expect(&cfg.browser.sandbox, false);
            }
            checks.push(check(
                "출구 프록시 포트(통행증)",
                wrong.is_empty(),
                if wrong.is_empty() {
                    format!(
                        "{proxy}: {}",
                        match (cfg.scripts.net, cfg.sandbox.native_tools) {
                            (true, true) => format!("{name}와 {}만 닿는다", cfg.sandbox.name),
                            (true, false) => format!("{name}만 닿는다"),
                            (false, _) => "꺼져 있다([scripts] net = false). 아무도 닿지 않는다".to_owned(),
                        }
                    )
                } else {
                    format!(
                        "{proxy}: {}. 데몬을 다시 띄우면 맞춘다(스크립트 쪽은 deploy/create-script-sandbox.sh --policy-only)",
                        wrong.join(", ")
                    )
                },
            ));
        }
    }

    match crate::app::open_store(cfg) {
        Ok(_) => checks.push(check("저장소", true, cfg.db_path().display().to_string())),
        Err(err) => checks.push(check("저장소", false, format!("{err:#}"))),
    }
    // 관리 화면(결정 0061): 정적 빌드가 바이너리에 들어 있고, 틀이 가리키는 파일이 모두 있는가.
    let missing = crate::admin::ui_missing_assets();
    checks.push(check(
        "관리 화면",
        crate::admin::ui_built() && missing.is_empty(),
        if !crate::admin::ui_built() {
            "이 바이너리에 화면이 들어 있지 않다(deploy/build-ui.sh 뒤 다시 빌드)".to_owned()
        } else if missing.is_empty() {
            "화면이 바이너리에 들어 있다".to_owned()
        } else {
            format!("틀이 가리키는 파일이 없다: {}", missing.join(", "))
        },
    ));
    checks
}
