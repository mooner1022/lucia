//! 실제 브라우저: 전용 샌드박스(`deploy/create-browser-sandbox.sh`)의 화면 있는 Chromium(PoC P8).
//!
//! - 처음 쓸 때 `sbx exec -i <샌드박스> sh -c <시작 스크립트>`를 띄운다. 이 세션이 샌드박스를 깨어 있게 하고
//!   (세션이 하나도 없으면 1분쯤 뒤 스스로 멈춘다), 그 표준 입출력이 CDP 통로다(VM 안 `socat STDIO`).
//!   호스트에 CDP 포트를 열지 않으므로 이 서버의 다른 프로세스가 로그인된 브라우저를 조종할 수 없다.
//! - Chrome의 모든 연결은 데몬 프록시를 지난다(`--proxy-server=socks5://…`, 결정 0008). VM 루프백만 예외다.
//! - 한동안 쓰지 않으면 Chrome을 정상 종료해 쿠키를 디스크에 쓰게 하고 세션을 끝낸다. 프로필은 남는다.
//! - 모든 조작은 한 잠금 아래에서 차례로 한다(로그인 입력 도중 다른 이동이 끼지 않게).
//! - 데몬 스크립트는 페이지와 떨어진 **격리된 실행 공간**(isolated world)에서 돈다: 페이지 스크립트가 DOM 함수를
//!   바꿔치기해 데몬이 보는 것을 속이지 못하게. 폼의 속성은 이름 가로채기(form 안 `name="action"` 칸 등)를
//!   피해 원형 함수로 읽는다.
//! - 클릭은 실제 마우스 이벤트로 요소 한가운데를 누른다. 그 점에 다른 요소가 덮여 있으면 누르지 않는다.
//! - 로그인 입력은 칸을 다시 확인한 직후에 넣는다: 최상위 문서의 출처가 그 사이트이고, 그 칸에 초점이 있고,
//!   비밀번호는 `type=password` 칸에만. 한 번만 시도하고, 실패하면 칸을 비운다.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::Command;
use tokio::sync::broadcast;
use url::Url;

use super::cdp::{Cdp, Event};
use super::egress::Egress;
use super::sites::AUTO_ATTR;
use super::{
    Act, BoxFut, Browser, BrowserError, Element, Expect, Focus, FrameView, Hit, LoginResult,
    PageView, SEEN_DOWNLOADS_MAX, Scroll, SeenDownload, Shot, SiteProfile,
};

#[derive(Debug, Clone)]
pub struct SbxConfig {
    pub sbx: PathBuf,
    pub sandbox: String,
    /// 이만큼 쓰지 않으면 브라우저를 닫는다.
    pub idle: Duration,
    /// Chrome 프록시(`socks5://host.docker.internal:47811`).
    pub proxy: String,
}

/// VM 안에서 도는 시작 스크립트. 화면(Xvfb)과 Chromium이 없으면 띄우고, CDP 주소를 한 줄 알린 뒤
/// 표준 입출력을 CDP에 잇는다. 띄운 프로세스는 표준 입출력을 로그 파일과 /dev/null로 돌린다(통로를 더럽히지 않게).
/// `/tmp`와 프로필은 샌드박스가 멈췄다 켜져도 남으므로, 프로세스가 없으면 남은 잠금 파일을 지운다(PoC P8).
/// 이미 뜬 Chromium이 다른 프록시로 떠 있으면(예전 설정) 끄고 다시 띄운다.
fn start_script(proxy: &str) -> String {
    format!(
        r#"
PROFILE=/home/agent/lucia-browser/profile
PROXY='{proxy}'
mkdir -p "$PROFILE"
if ! pgrep -x Xvfb >/dev/null; then
  rm -f /tmp/.X99-lock /tmp/.X11-unix/X99
  setsid nohup Xvfb :99 -screen 0 1366x900x24 -nolisten tcp >/tmp/lucia-xvfb.log 2>&1 </dev/null &
  i=0; while [ ! -S /tmp/.X11-unix/X99 ] && [ $i -lt 40 ]; do sleep 0.25; i=$((i+1)); done
fi
P=$(pgrep -xo chromium || true)
if [ -n "$P" ] && ! tr '\0' '\n' </proc/$P/cmdline | grep -qx -- "--proxy-server=$PROXY"; then
  pkill -x chromium; i=0; while pgrep -x chromium >/dev/null && [ $i -lt 40 ]; do sleep 0.25; i=$((i+1)); done
fi
if ! pgrep -x chromium >/dev/null; then
  rm -f "$PROFILE"/Singleton*
  rm -rf /tmp/lucia-upload
  DISPLAY=:99 LC_ALL=C.UTF-8 setsid nohup chromium --no-first-run --no-default-browser-check \
    --password-store=basic --user-data-dir="$PROFILE" \
    --remote-debugging-port=9222 \
    --proxy-server="$PROXY" \
    --disable-background-networking --disable-component-update --disable-sync \
    --window-size=1366,900 --lang=ko-KR about:blank >/tmp/lucia-chromium.log 2>&1 </dev/null &
fi
WS=
i=0
while [ $i -lt 160 ]; do
  WS=$(curl -s --noproxy '*' -m 2 http://127.0.0.1:9222/json/version | sed -n 's/.*"webSocketDebuggerUrl": *"\([^"]*\)".*/\1/p')
  [ -n "$WS" ] && break
  sleep 0.25; i=$((i+1))
done
if [ -z "$WS" ]; then echo "LUCIA-CDP-ERROR Chromium이 뜨지 않았다"; tail -3 /tmp/lucia-chromium.log >&2; exit 1; fi
echo "LUCIA-CDP $WS"
exec socat STDIO TCP:127.0.0.1:9222
"#
    )
}

const CALL: Duration = Duration::from_secs(20);
/// 페이지 스크립트(`browser_eval`)를 기다리는 시간.
const EVAL: Duration = Duration::from_secs(30);
const LOAD: Duration = Duration::from_secs(25);
const SUBMIT: Duration = Duration::from_secs(15);
const START: Duration = Duration::from_secs(120);
/// 동작 뒤 이동이 시작되는지 지켜보는 시간.
const NAV_START: Duration = Duration::from_millis(1500);
/// 동작이 닿은 iframe의 이동을 기다리는 상한(결정 0023).
const FRAME_LOAD: Duration = Duration::from_secs(8);
/// 화면에서 읽어 올 본문 글 상한(문자). 도구가 다시 자른다.
const READ_TEXT_MAX: usize = 60_000;
/// 화면에 넣을 요소 수 상한.
const ELEMENTS_MAX: usize = 200;
/// 전체 페이지 스크린샷 높이 상한(픽셀).
const FULL_PAGE_MAX_HEIGHT: f64 = 8000.0;

struct Live {
    child: tokio::process::Child,
    cdp: Arc<Cdp>,
    session: String,
    /// 이 페이지의 타깃 ID(최상위 프레임 ID와 같다).
    target: String,
    /// 자동으로 닫은 대화상자의 문구(로그인 실패 이유, 동작 결과로 쓴다).
    dialogs: Arc<Mutex<Vec<String>>>,
    /// 동작 중 새로 열린 창(닫고 주소만 남긴다).
    popups: Arc<Mutex<Vec<String>>>,
    /// 사람이 승인한 동작 중이다: 확인(confirm) 대화상자를 받아들인다.
    approved: Arc<AtomicBool>,
    /// 사람이 화면을 넘겨받았다(결정 0017): 새 창을 닫지 않고 탭으로 남긴다.
    handover: Arc<AtomicBool>,
    /// 넘겨받는 동안 붙은 탭의 CDP 세션(타깃 ID → 세션).
    tab_sessions: Mutex<HashMap<String, String>>,
    /// 데몬 스크립트용 격리된 실행 공간(문서가 바뀌면 다시 만든다).
    world: Mutex<Option<i64>>,
    /// iframe의 격리된 실행 공간((세션, 프레임) → 공간, 결정 0023).
    frame_worlds: Mutex<HashMap<(String, String), i64>>,
    /// 다른 사이트 iframe(별도 프로세스)의 CDP 세션(프레임 ID → 세션, 결정 0023). 붙은 순서대로.
    children: Arc<Mutex<Vec<ChildFrame>>>,
    /// 마지막 히트 테스트·초점 읽기가 닿은 iframe(프레임 ID). 그 동작 뒤 그 프레임의 이동만 기다린다.
    acted_frame: Mutex<Option<String>>,
    /// 감시 탭(결정 0036): (타깃, 세션). 대화용 탭과 따로 둔다. 새 창 닫기에서 뺀다.
    watch_tab: Arc<Mutex<Option<(String, String)>>>,
    /// 감시 탭을 만드는 중(그 사이 생기는 빈 탭은 닫지 않는다).
    making_tab: Arc<AtomicBool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

/// 별도 프로세스로 뜬 iframe(다른 사이트, 결정 0023). 자동으로 붙는다.
#[derive(Debug, Clone)]
struct ChildFrame {
    /// 프레임 ID(= 타깃 ID).
    frame: String,
    session: String,
    /// 이 프레임을 품은 문서의 세션.
    parent_session: String,
}

/// 한 번에 살필 iframe 수(화면 읽기).
const FRAMES_SCAN: usize = 10;
/// iframe 하나에서 읽을 글(문자)과 요소 수.
const FRAME_READ_TEXT: usize = 12_000;
const FRAME_READ_ELEMENTS: usize = 60;

pub struct SbxBrowser {
    cfg: SbxConfig,
    egress: Arc<Egress>,
    live: tokio::sync::Mutex<Option<Live>>,
    running: AtomicBool,
    last_used: Mutex<Instant>,
    /// 브라우저 방식 감시가 있다: 쓰지 않아도 닫지 않는다(결정 0036).
    keep: AtomicBool,
    /// 브라우저가 시작하려던 내려받기(결정 0057). 브라우저는 받지 않고 주소와 이름만 남긴다.
    downloads: Arc<Mutex<Vec<SeenDownload>>>,
}

impl SbxBrowser {
    pub fn new(cfg: SbxConfig, egress: Arc<Egress>) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            egress,
            live: tokio::sync::Mutex::new(None),
            running: AtomicBool::new(false),
            last_used: Mutex::new(Instant::now()),
            keep: AtomicBool::new(false),
            downloads: Arc::default(),
        })
    }

    /// 쓰지 않으면 닫는 작업. 데몬이 멈출 때도 정상 종료한다(쿠키를 디스크에 쓰게).
    pub fn spawn_reaper(self: &Arc<Self>, cancel: tokio_util::sync::CancellationToken) {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        me.shutdown().await;
                        return;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(30)) => me.release_if_idle().await,
                }
            }
        });
    }

    pub async fn shutdown(&self) {
        let mut slot = self.live.lock().await;
        if let Some(live) = slot.take() {
            close(live).await;
            tracing::info!("브라우저를 닫았다");
        }
        self.running.store(false, Ordering::SeqCst);
    }

    async fn release_if_idle(&self) {
        // 조작 중이면(잠금을 쥐고 있으면) 건너뛴다.
        let Ok(mut slot) = self.live.try_lock() else {
            return;
        };
        let idle = self.last_used.lock().expect("last_used lock").elapsed() >= self.cfg.idle
            && !self.keep.load(Ordering::SeqCst);
        if idle && let Some(live) = slot.take() {
            close(live).await;
            self.running.store(false, Ordering::SeqCst);
            tracing::info!("브라우저를 한동안 쓰지 않아 닫았다");
        }
    }

    fn touch(&self) {
        *self.last_used.lock().expect("last_used lock") = Instant::now();
    }

    async fn ensure<'a>(&self, slot: &'a mut Option<Live>) -> Result<&'a Live, BrowserError> {
        if !slot.as_ref().is_some_and(|l| l.cdp.alive()) {
            if let Some(dead) = slot.take() {
                close(dead).await;
            }
            self.running.store(false, Ordering::SeqCst);
            let live = self.start().await?;
            *slot = Some(live);
            self.running.store(true, Ordering::SeqCst);
        }
        Ok(slot.as_ref().expect("방금 채웠다"))
    }

    /// 브라우저가 떠 있지 않으면 `None`(지금 화면을 보는 동작은 새로 띄우지 않는다).
    fn current(slot: &Option<Live>) -> Result<&Live, BrowserError> {
        match slot {
            Some(l) if l.cdp.alive() => Ok(l),
            _ => Err(BrowserError::Failed(
                "열린 페이지가 없다(한동안 쓰지 않아 브라우저를 닫았다). browser_open으로 다시 연다".into(),
            )),
        }
    }

    async fn start(&self) -> Result<Live, BrowserError> {
        let unavailable = |m: String| BrowserError::Unavailable(m);
        let script = start_script(&self.cfg.proxy);
        let mut child = Command::new(&self.cfg.sbx)
            .args(["exec", "-i", &self.cfg.sandbox, "sh", "-c", &script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| unavailable(format!("sbx exec 실행: {e}")))?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(line = %crate::mask::global().mask(&line), "브라우저 샌드박스");
                }
            });
        }
        let stdout = child.stdout.take().expect("stdout piped");
        let stdin = child.stdin.take().expect("stdin piped");
        let mut reader = BufReader::new(stdout);
        let ws_url = tokio::time::timeout(START, async {
            loop {
                let mut line = String::new();
                let n = reader
                    .read_line(&mut line)
                    .await
                    .map_err(|e| format!("시작 출력 읽기: {e}"))?;
                if n == 0 {
                    return Err("브라우저 세션이 바로 끝났다(샌드박스가 있는지 확인)".to_owned());
                }
                let line = line.trim();
                if let Some(url) = line.strip_prefix("LUCIA-CDP ") {
                    return Ok(url.to_owned());
                }
                if let Some(why) = line.strip_prefix("LUCIA-CDP-ERROR ") {
                    return Err(why.to_owned());
                }
            }
        })
        .await
        .map_err(|_| unavailable("브라우저가 제때 뜨지 않았다".into()))?
        .map_err(unavailable)?;
        if !ws_url.starts_with("ws://127.0.0.1:9222/devtools/browser/") {
            return Err(unavailable(format!("예상과 다른 CDP 주소: {ws_url}")));
        }
        // BufReader에 남은 바이트가 있어도 그대로 이어 읽는다.
        let stream = tokio::io::join(reader, stdin);
        let cdp = Arc::new(
            Cdp::connect(Box::new(stream), &ws_url)
                .await
                .map_err(unavailable)?,
        );
        let fail = |e: String| BrowserError::Failed(e);
        // 브라우저는 내려받지 않는다(VM 디스크에 남의 파일을 두지 않는다). 시작하려던 주소만 알려 받아, 데몬이
        // 같은 검사로 받는다(결정 0057).
        let _ = cdp
            .call(
                None,
                "Browser.setDownloadBehavior",
                json!({"behavior": "deny", "eventsEnabled": true}),
                CALL,
            )
            .await;
        let targets = cdp
            .call(None, "Target.getTargets", json!({}), CALL)
            .await
            .map_err(fail)?;
        let pages: Vec<String> = targets["targetInfos"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|t| t["type"] == "page")
                    .filter_map(|t| t["targetId"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let target = match pages.first() {
            Some(t) => t.clone(),
            None => cdp
                .call(
                    None,
                    "Target.createTarget",
                    json!({"url": "about:blank"}),
                    CALL,
                )
                .await
                .map_err(fail)?["targetId"]
                .as_str()
                .ok_or_else(|| fail("페이지를 만들지 못했다".into()))?
                .to_owned(),
        };
        // 탭은 하나만 쓴다(지난번에 남은 창은 닫는다).
        for extra in pages.iter().skip(1) {
            let _ = cdp
                .call(None, "Target.closeTarget", json!({"targetId": extra}), CALL)
                .await;
        }
        let attached = cdp
            .call(
                None,
                "Target.attachToTarget",
                json!({"targetId": target, "flatten": true}),
                CALL,
            )
            .await
            .map_err(fail)?;
        let session = attached["sessionId"]
            .as_str()
            .ok_or_else(|| fail("페이지에 붙지 못했다".into()))?
            .to_owned();

        // 대화상자: 알림(alert)과 떠나기 확인은 받아들이고, 확인·입력(confirm·prompt)은 승인된 동작 중에만
        // 받아들인다(아니면 취소: "정말 지울까요?"에 데몬이 멋대로 "예"를 누르지 않게). 열린 채로 두면 페이지가 멈춘다.
        // 새 창(target=_blank, window.open)은 닫고 주소만 남긴다(탭은 하나만 쓴다).
        let dialogs: Arc<Mutex<Vec<String>>> = Arc::default();
        let popups: Arc<Mutex<Vec<String>>> = Arc::default();
        let handover: Arc<AtomicBool> = Arc::default();
        let approved = Arc::new(AtomicBool::new(false));
        let _ = cdp
            .call(
                None,
                "Target.setDiscoverTargets",
                json!({"discover": true}),
                CALL,
            )
            .await;
        let children: Arc<Mutex<Vec<ChildFrame>>> = Arc::default();
        let watch_tab: Arc<Mutex<Option<(String, String)>>> = Arc::default();
        let making_tab: Arc<AtomicBool> = Arc::default();
        let mut events = cdp.subscribe();
        let watch_task = {
            let cdp = cdp.clone();
            let session = session.clone();
            let target = target.clone();
            let dialogs = dialogs.clone();
            let popups = popups.clone();
            let approved = approved.clone();
            let handover = handover.clone();
            let children = children.clone();
            let watch_tab = watch_tab.clone();
            let making_tab = making_tab.clone();
            let downloads = self.downloads.clone();
            tokio::spawn(async move {
                loop {
                    let e = match events.recv().await {
                        Ok(e) => e,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return,
                    };
                    // 내려받기가 시작되려 했다(브라우저 단위 사건과 페이지 단위 사건이 함께 올 수 있다).
                    if e.method == "Browser.downloadWillBegin"
                        || e.method == "Page.downloadWillBegin"
                    {
                        // Lucia가 쓰는 탭의 것만 남긴다. 사람이 화면을 넘겨받은 동안의 것(사람이 누른 것)과
                        // 감시 탭의 것(모델 없이 도는 읽기)은 버린다: 남겨 두면 다음 대화의 브라우저 도구가
                        // 그 대화의 권한과 로그인 세션으로 대신 받게 된다(결정 0029의 경계).
                        let from_watch = watch_tab
                            .lock()
                            .expect("watch tab lock")
                            .as_ref()
                            .is_some_and(|(tab, tab_session)| {
                                e.params["frameId"].as_str() == Some(tab.as_str())
                                    || e.session.as_deref() == Some(tab_session.as_str())
                            });
                        if handover.load(Ordering::SeqCst) || from_watch {
                            continue;
                        }
                        let url = e.params["url"].as_str().unwrap_or("").to_owned();
                        let name = e.params["suggestedFilename"]
                            .as_str()
                            .unwrap_or("")
                            .to_owned();
                        let mut seen = downloads.lock().expect("downloads lock");
                        if !url.is_empty()
                            && seen.len() < SEEN_DOWNLOADS_MAX
                            && !seen.iter().any(|d| d.url == url)
                        {
                            tracing::debug!("브라우저가 내려받기를 시작하려 했다");
                            seen.push(SeenDownload { url, name });
                        }
                        continue;
                    }
                    let ours = |s: Option<&str>| {
                        s == Some(session.as_str())
                            || s.is_some_and(|s| {
                                children
                                    .lock()
                                    .expect("children lock")
                                    .iter()
                                    .any(|c| c.session == s)
                            })
                    };
                    if e.method == "Target.attachedToTarget"
                        && e.params["targetInfo"]["type"] == "iframe"
                        && ours(e.session.as_deref())
                    {
                        // 다른 사이트 iframe(결정 0023): 대화상자·로드 사건을 받고, 그 안의 iframe에도 붙는다.
                        let (Some(child), Some(frame)) = (
                            e.params["sessionId"].as_str(),
                            e.params["targetInfo"]["targetId"].as_str(),
                        ) else {
                            continue;
                        };
                        tracing::debug!(url = %e.params["targetInfo"]["url"], "iframe 세션에 붙었다");
                        children.lock().expect("children lock").push(ChildFrame {
                            frame: frame.to_owned(),
                            session: child.to_owned(),
                            parent_session: e.session.clone().unwrap_or_default(),
                        });
                        let cdp = cdp.clone();
                        let child = child.to_owned();
                        tokio::spawn(async move {
                            let _ = cdp.call(Some(&child), "Page.enable", json!({}), CALL).await;
                            let _ = cdp
                                .call(Some(&child), "Target.setAutoAttach", auto_attach(), CALL)
                                .await;
                        });
                        continue;
                    }
                    if e.method == "Target.detachedFromTarget" {
                        if let Some(gone) = e.params["sessionId"].as_str() {
                            children
                                .lock()
                                .expect("children lock")
                                .retain(|c| c.session != gone);
                        }
                        continue;
                    }
                    // 감시 탭의 대화상자는 닫기만 한다(열린 채로 두면 읽기가 멈춘다).
                    if e.method == "Page.javascriptDialogOpening"
                        && e.session.is_some()
                        && watch_tab
                            .lock()
                            .expect("watch tab lock")
                            .as_ref()
                            .is_some_and(|(_, s)| Some(s.as_str()) == e.session.as_deref())
                    {
                        let session = e.session.clone().unwrap_or_default();
                        let _ = cdp
                            .call(
                                Some(&session),
                                "Page.handleJavaScriptDialog",
                                json!({"accept": false}),
                                CALL,
                            )
                            .await;
                        continue;
                    }
                    if e.method == "Page.javascriptDialogOpening" && ours(e.session.as_deref()) {
                        let dialog_session = e.session.clone().unwrap_or_default();
                        let kind = e.params["type"].as_str().unwrap_or("alert").to_owned();
                        let accept = matches!(kind.as_str(), "alert" | "beforeunload")
                            || approved.load(Ordering::SeqCst);
                        let message: String = e.params["message"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(300)
                            .collect();
                        let note = match (kind.as_str(), accept) {
                            ("alert", _) => message,
                            (_, true) => format!("[{kind} 수락] {message}"),
                            (_, false) => format!("[{kind} 취소함] {message}"),
                        };
                        dialogs.lock().expect("dialogs lock").push(note);
                        let _ = cdp
                            .call(
                                Some(&dialog_session),
                                "Page.handleJavaScriptDialog",
                                json!({"accept": accept}),
                                CALL,
                            )
                            .await;
                    } else if e.method == "Target.targetCreated"
                        && e.params["targetInfo"]["type"] == "page"
                        && e.params["targetInfo"]["targetId"].as_str() != Some(target.as_str())
                    {
                        let info = &e.params["targetInfo"];
                        let url = info["url"].as_str().unwrap_or("").to_owned();
                        // 데몬이 만든 감시 탭은 닫지 않는다(결정 0036).
                        let id = info["targetId"].as_str().unwrap_or("");
                        if (making_tab.load(Ordering::SeqCst) && url == "about:blank")
                            || watch_tab
                                .lock()
                                .expect("watch tab lock")
                                .as_ref()
                                .is_some_and(|(t, _)| t == id)
                        {
                            continue;
                        }
                        // 넘겨받는 중이면 사람이 다루는 새 창(본인인증 등)이다: 닫지 않는다.
                        if handover.load(Ordering::SeqCst) {
                            popups.lock().expect("popups lock").push(url);
                            continue;
                        }
                        if let Some(id) = info["targetId"].as_str() {
                            let _ = cdp
                                .call(None, "Target.closeTarget", json!({"targetId": id}), CALL)
                                .await;
                        }
                        popups.lock().expect("popups lock").push(url);
                    }
                }
            })
        };
        cdp.call(Some(&session), "Page.enable", json!({}), CALL)
            .await
            .map_err(fail)?;
        // 다른 사이트 iframe은 별도 프로세스라 따로 붙어야 그 안을 읽는다(결정 0023, 실측: 붙지 않으면 히트 테스트가
        // iframe 요소에서 멈춘다).
        if let Err(e) = cdp
            .call(Some(&session), "Target.setAutoAttach", auto_attach(), CALL)
            .await
        {
            tracing::warn!(err = %e, "iframe 자동 붙기를 켜지 못했다");
        }
        tracing::info!(sandbox = %self.cfg.sandbox, "브라우저를 띄웠다");
        Ok(Live {
            child,
            cdp,
            session,
            target,
            dialogs,
            popups,
            approved,
            handover,
            tab_sessions: Mutex::default(),
            world: Mutex::new(None),
            frame_worlds: Mutex::default(),
            children,
            acted_frame: Mutex::new(None),
            watch_tab,
            making_tab,
            tasks: vec![watch_task],
        })
    }
}

/// iframe 자동 붙기: 멈추지 않고(waitForDebuggerOnStart=false), 같은 연결 위의 세션으로(flatten).
fn auto_attach() -> Value {
    json!({"autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true})
}

async fn close(mut live: Live) {
    let _ = live
        .cdp
        .call(None, "Browser.close", json!({}), Duration::from_secs(5))
        .await;
    // Chrome이 끝나면 VM 안 socat이 끝나고 sbx exec도 끝난다.
    if tokio::time::timeout(Duration::from_secs(10), live.child.wait())
        .await
        .is_err()
    {
        let _ = live.child.kill().await;
    }
    for t in live.tasks {
        t.abort();
    }
}

/// 이벤트를 기다린다(조건에 맞는 것이 오면 `Some`). 시간이 지나면 `None`.
async fn wait_for(
    events: &mut broadcast::Receiver<Event>,
    timeout: Duration,
    mut matches: impl FnMut(&Event) -> bool,
) -> Option<Event> {
    tokio::time::timeout(timeout, async {
        loop {
            match events.recv().await {
                Ok(e) if matches(&e) => return Some(e),
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
    .await
    .ok()
    .flatten()
}

/// JS 문자열 리터럴(선택자를 안전하게 끼워 넣는다).
fn js_str(s: &str) -> String {
    serde_json::to_string(s).expect("문자열 직렬화")
}

/// 요소 참조 형식(`e` + 숫자). 선택자에 끼워 넣기 전에 확인한다.
pub fn valid_ref(r: &str) -> bool {
    r.len() <= 8
        && r.strip_prefix('e')
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// 모든 데몬 스크립트가 쓰는 도우미(격리된 실행 공간에서 돈다).
const PRELUDE: &str = r#"
const P = {
  getAttr: Element.prototype.getAttribute,
  setAttr: Element.prototype.setAttribute,
  remAttr: Element.prototype.removeAttribute,
  tag: Object.getOwnPropertyDescriptor(Element.prototype, 'tagName').get,
  formEls: Object.getOwnPropertyDescriptor(HTMLFormElement.prototype, 'elements').get,
  qsa: Element.prototype.querySelectorAll,
};
// 폼의 조작 요소. `form.elements`에는 이미지 제출 단추(input type=image)가 빠지므로 더한다.
const formPool = form => [...P.formEls.call(form), ...P.qsa.call(form, 'input[type=image]')];
const attr = (el, n) => P.getAttr.call(el, n);
const tagOf = el => P.tag.call(el);
const one = s => String(s || '').replace(/\s+/g, ' ').trim();
const clip = (s, n) => { s = one(s); return s.length > n ? s.slice(0, n - 1) + '…' : s; };
const clean = s => String(s || '').replace(/[ \t ]+/g, ' ').replace(/\n[ \n]*\n/g, '\n\n').trim();
const vis = el => {
  const r = el.getBoundingClientRect();
  if (r.width < 1 || r.height < 1) return false;
  const st = getComputedStyle(el);
  return st.visibility !== 'hidden' && st.display !== 'none' && parseFloat(st.opacity || '1') > 0.05;
};
const inView = el => { const r = el.getBoundingClientRect(); return r.bottom > 0 && r.right > 0 && r.top < innerHeight && r.left < innerWidth; };
const typeOf = el => {
  const t = tagOf(el);
  const a = (attr(el, 'type') || '').toLowerCase();
  if (t === 'BUTTON') return a || 'submit';
  if (t === 'INPUT') return a || 'text';
  return a;
};
const labelOf = el => {
  const a = attr(el, 'aria-label'); if (one(a)) return clip(a, 100);
  const lb = attr(el, 'aria-labelledby');
  if (lb) {
    const t = lb.split(/\s+/).map(id => document.getElementById(id)).filter(Boolean).map(e => e.innerText || e.textContent).join(' ');
    if (one(t)) return clip(t, 100);
  }
  const tag = tagOf(el), type = typeOf(el);
  if (tag === 'INPUT' && ['submit', 'button', 'reset'].includes(type)) return clip(el.value || type, 100);
  if (tag === 'INPUT' && type === 'image') return clip(attr(el, 'alt') || 'image', 100);
  if (['INPUT', 'TEXTAREA', 'SELECT'].includes(tag) && el.labels && el.labels.length) {
    const t = [...el.labels].map(l => l.innerText).join(' ');
    if (one(t)) return clip(t, 100);
  }
  if (tag === 'INPUT' || tag === 'TEXTAREA') return clip(attr(el, 'placeholder') || attr(el, 'title') || attr(el, 'name') || '', 100);
  if (tag === 'SELECT') return clip(attr(el, 'title') || attr(el, 'name') || '', 100);
  const t = one(el.innerText);
  if (t) return clip(t, 100);
  const img = el.querySelector('img[alt]'); if (img && one(img.alt)) return clip(img.alt, 100);
  return clip(attr(el, 'title') || '', 100);
};
const kindOf = el => {
  const tag = tagOf(el), role = (attr(el, 'role') || '').toLowerCase(), type = typeOf(el);
  if (tag === 'A' && attr(el, 'href') !== null) return 'link';
  if (tag === 'INPUT') {
    if (type === 'password') return 'password';
    if (type === 'submit' || type === 'image') return el.form ? 'submit' : 'button';
    if (type === 'button') return 'button';
    if (type === 'reset') return 'reset';
    if (type === 'checkbox') return 'checkbox';
    if (type === 'radio') return 'radio';
    if (type === 'file') return 'file';
    if (type === 'hidden') return 'hidden';
    return 'text';
  }
  if (tag === 'BUTTON') { if (type === 'submit' && el.form) return 'submit'; if (type === 'reset') return 'reset'; return 'button'; }
  if (tag === 'TEXTAREA') return 'textarea';
  if (tag === 'SELECT') return 'select';
  if (tag === 'SUMMARY') return 'summary';
  if (role === 'link') return 'link';
  if (role === 'tab') return 'tab';
  if (['checkbox', 'switch', 'radio'].includes(role)) return 'checkbox';
  if (['textbox', 'searchbox', 'combobox'].includes(role)) return el.isContentEditable ? 'editable' : 'text';
  if (el.isContentEditable) return 'editable';
  return 'button';
};
const resolve = u => { try { return new URL(u, document.baseURI).href; } catch (e) { return String(u); } };
const formInfo = (f, submitter) => {
  if (!f) return null;
  const fields = []; let hidden = 0, pw = false, first = null;
  for (const x of P.formEls.call(f)) {
    const tag = tagOf(x), t = typeOf(x);
    if (!first && ((tag === 'BUTTON' && t === 'submit') || (tag === 'INPUT' && (t === 'submit' || t === 'image')))) first = x;
    if (t === 'password') { pw = true; continue; }
    if (t === 'hidden') { hidden++; continue; }
    if (!['INPUT', 'SELECT', 'TEXTAREA'].includes(tag)) continue;
    if (['submit', 'button', 'reset', 'image', 'file'].includes(t) || x.disabled) continue;
    let v;
    if (t === 'checkbox' || t === 'radio') { if (!x.checked) continue; v = x.value || 'on'; }
    else if (tag === 'SELECT') v = [...x.selectedOptions].map(o => o.label).join(', ');
    else v = x.value;
    if (fields.length < 30) fields.push([clip(labelOf(x) || attr(x, 'name') || attr(x, 'id') || t, 60), String(v || '').slice(0, 300)]);
  }
  const by = submitter || first;
  let method = ((by && attr(by, 'formmethod')) || attr(f, 'method') || 'get').toLowerCase();
  if (!['get', 'post', 'dialog'].includes(method)) method = 'get';
  const action = resolve((by && attr(by, 'formaction')) || attr(f, 'action') || location.href);
  return { method, action, fields, hidden, password: pw };
};
const describe = (el, ref) => {
  const kind = kindOf(el), tag = tagOf(el), type = typeOf(el);
  const hp = attr(el, 'aria-haspopup');
  const d = { ref, kind, tag, label: labelOf(el), disabled: !!el.disabled || attr(el, 'aria-disabled') === 'true',
              in_view: inView(el), popup: !!hp && hp !== 'false', options: [] };
  if (tag === 'A') d.href = el.href;
  if (tag === 'INPUT') d.input_type = type;
  const ex = attr(el, 'aria-expanded'); if (ex === 'true' || ex === 'false') d.expanded = ex === 'true';
  if (kind === 'text' || kind === 'textarea') d.value = String(el.value === undefined ? '' : el.value).slice(0, 200);
  if (kind === 'editable') d.value = clip(el.innerText, 200);
  if (kind === 'select') { d.value = [...el.selectedOptions].map(o => o.label).join(', '); d.options = [...el.options].slice(0, 30).map(o => clip(o.label, 60)); }
  if (kind === 'checkbox' || kind === 'radio') d.checked = tag === 'INPUT' ? !!el.checked : attr(el, 'aria-checked') === 'true';
  const f = el.form || el.closest('form');
  if (f && tag !== 'A') d.form = formInfo(f, kind === 'submit' ? el : null);
  const hint = [attr(el, 'name'), attr(el, 'id'), attr(el, 'placeholder'), attr(el, 'aria-label'), f ? attr(f, 'role') : '', f ? attr(f, 'action') : ''].join(' ');
  d.search = type === 'search' || (attr(el, 'role') || '') === 'searchbox' || /search|query|keyword|검색|찾기/i.test(hint);
  return d;
};
const byRef = ref => document.querySelector('[data-lucia-ref="' + ref + '"]');
const SEL = 'a[href], button, input, select, textarea, summary, [role=button], [role=link], [role=tab], [role=checkbox], [role=switch], [role=radio], [role=menuitem], [role=option], [role=combobox], [role=searchbox], [role=textbox], [contenteditable=""], [contenteditable=true], [onclick]';
"#;

/// 히트 테스트로 찾은 노드(`this`)에서 누를 수 있는 가장 가까운 조상을 읽는다(결정 0023). 없으면 그 노드를
/// "화면의 그 자리"(`area`)로, 손가락 커서인지와 함께. iframe 요소면 그 안을 따로 본다.
fn probe_fn() -> String {
    format!(
        r#"function() {{
{PRELUDE}
const n = this && this.nodeType === 1 ? this : (this ? this.parentElement : null);
if (!n) return JSON.stringify({{ frame: false, url: location.href, el: null, pointer: false }});
if (['IFRAME', 'FRAME'].includes(tagOf(n))) return JSON.stringify({{ frame: true, url: location.href, el: null, pointer: false }});
let act = n.closest(SEL);
// label을 누르면 연결된 컨트롤이 눌린다(label 안의 링크·단추를 누른 것이 아니면).
const lab = n.closest('label');
if (lab && lab.control && (!act || act.contains(lab))) act = lab.control;
let d;
if (act && !(tagOf(act) === 'INPUT' && typeOf(act) === 'hidden')) d = describe(act, '');
else {{
  d = describe(n, ''); d.kind = 'area'; delete d.form; delete d.href; d.expanded = undefined; d.popup = false;
  d.label = clip(n.innerText || n.textContent || attr(n, 'aria-label') || attr(n, 'title') || tagOf(n), 80);
}}
let pointer = false;
for (let x = n; x && x.nodeType === 1; x = x.parentElement) {{ if (getComputedStyle(x).cursor === 'pointer') {{ pointer = true; break; }} }}
return JSON.stringify({{ frame: false, url: location.href, el: d, pointer }});
}}"#
    )
}

#[derive(Deserialize)]
struct ProbeJs {
    frame: bool,
    url: String,
    el: Option<Element>,
    #[serde(default)]
    pointer: bool,
}

/// 이 문서의 초점(그림자 DOM 안까지). 프레임마다 돈다(결정 0023).
fn focus_js() -> String {
    format!(
        r#"(() => {{
{PRELUDE}
let a = document.activeElement;
while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement;
const has = document.hasFocus();
// 문서 전체가 편집 모드면(designMode, body contenteditable: 에디터가 글자를 받는 숨은 iframe 등) body의 초점이 곧
// 편집 칸이다(2026-11-18 네이버 카페 글쓰기: 이것을 "초점 없음"으로 봐 Enter마다 승인을 물었다).
if (a === document.body && (document.designMode === 'on' || a.isContentEditable))
  return JSON.stringify({{ state: 'el', has, url: location.href, el: Object.assign(describe(a, ''), {{ kind: 'editable' }}) }});
if (!a || a === document.body || a === document.documentElement) return JSON.stringify({{ state: 'none', has, url: location.href }});
if (['IFRAME', 'FRAME'].includes(tagOf(a))) return JSON.stringify({{ state: 'frame', has, url: location.href }});
const el = (a.matches && a.matches(SEL)) ? a : ((a.closest && a.closest(SEL)) || a);
return JSON.stringify({{ state: 'el', has, url: location.href, el: describe(el, '') }});
}})()"#
    )
}

#[derive(Deserialize)]
struct FocusJs {
    state: String,
    has: bool,
    url: String,
    #[serde(default)]
    el: Option<Element>,
}

/// iframe 하나의 화면: 글과, 지금 보이는 요소의 가운데 점(그 프레임 화면 기준). 참조는 달지 않는다(결정 0023).
fn frame_view_js() -> String {
    format!(
        r#"(() => {{
{PRELUDE}
const out = [];
for (const el of document.querySelectorAll(SEL)) {{
  if (out.length >= {max}) break;
  if (tagOf(el) === 'INPUT' && typeOf(el) === 'hidden') continue;
  if (!vis(el) || !inView(el)) continue;
  if (!['INPUT', 'SELECT', 'TEXTAREA'].includes(tagOf(el)) && el.parentElement && el.parentElement.closest('a[href], button')) continue;
  const r = el.getBoundingClientRect();
  const l = Math.max(r.left, 0), t = Math.max(r.top, 0), rr = Math.min(r.right, innerWidth), b = Math.min(r.bottom, innerHeight);
  out.push({{ x: Math.round((l + rr) / 2), y: Math.round((t + b) / 2), el: describe(el, '') }});
}}
return JSON.stringify({{ url: location.href, text: clean(document.body ? document.body.innerText : '').slice(0, {text}), elements: out }});
}})()"#,
        max = FRAME_READ_ELEMENTS,
        text = FRAME_READ_TEXT,
    )
}

#[derive(Deserialize)]
struct FramePoint {
    x: i32,
    y: i32,
    el: Element,
}

#[derive(Deserialize)]
struct FrameViewJs {
    url: String,
    text: String,
    elements: Vec<FramePoint>,
}

/// 히트 테스트 결과(한 세션 안). 잠깐 쓰고 버리는 값이라 크기 차이는 두었다.
#[allow(clippy::large_enum_variant)]
enum Probed {
    Found { el: Element, pointer: bool },
    Unknown(String),
}

/// 화면 읽기: 주소, 제목, 본문 글, 조작할 수 있는 요소(참조를 새로 단다), 로그인 폼 유무, 스크롤.
fn view_js() -> String {
    format!(
        r#"(() => {{
{PRELUDE}
for (const el of document.querySelectorAll('[data-lucia-ref]')) P.remAttr.call(el, 'data-lucia-ref');
const all = [];
for (const el of document.querySelectorAll(SEL)) {{
  if (typeOf(el) === 'hidden' && tagOf(el) === 'INPUT') continue;
  let shown = vis(el);
  if (!shown && tagOf(el) === 'INPUT' && el.labels && [...el.labels].some(vis)) shown = true;
  if (!shown) continue;
  if (!['INPUT', 'SELECT', 'TEXTAREA'].includes(tagOf(el)) && el.parentElement && el.parentElement.closest('a[href], button')) continue;
  all.push(el);
}}
const index = new Map(all.map((e, i) => [e, i]));
let pick = all;
if (all.length > {max}) {{
  pick = all.filter(inView).concat(all.filter(e => !inView(e))).slice(0, {max});
  pick.sort((a, b) => index.get(a) - index.get(b));
}}
const elements = pick.map(el => {{ const ref = 'e' + (index.get(el) + 1); P.setAttr.call(el, 'data-lucia-ref', ref); return describe(el, ref); }});
const login_form = [...document.querySelectorAll('input[type=password]')].some(vis);
return JSON.stringify({{
  url: location.href,
  title: document.title,
  text: clean(document.body ? document.body.innerText : '').slice(0, {text}),
  elements, total: all.length, login_form,
  scroll: [Math.round(scrollY), Math.round(innerHeight), Math.round(document.documentElement.scrollHeight)],
  vw: Math.round(innerWidth),
}});
}})()"#,
        max = ELEMENTS_MAX,
        text = READ_TEXT_MAX,
    )
}

#[derive(Deserialize)]
struct ViewJs {
    url: String,
    title: String,
    text: String,
    elements: Vec<Element>,
    total: usize,
    login_form: bool,
    scroll: (i64, i64, i64),
    #[serde(default)]
    vw: i64,
}

fn inspect_js(r: &str) -> String {
    format!(
        r#"(() => {{
{PRELUDE}
const el = byRef({r});
return JSON.stringify({{ url: location.href, el: el ? describe(el, {r}) : null }});
}})()"#,
        r = js_str(r)
    )
}

#[derive(Deserialize)]
struct InspectJs {
    url: String,
    el: Option<Element>,
}

/// 누를 점: 요소를 화면 가운데로 옮기고 한가운데 점에 그 요소(또는 그 안쪽)가 있는지 본다.
/// 새 창으로 여는 링크는 누르지 않고 그 주소로 연다(탭은 하나만 쓴다).
fn point_js(r: &str) -> String {
    format!(
        r#"(() => {{
{PRELUDE}
const el = byRef({r});
if (!el) return JSON.stringify({{ ok: false, why: '요소가 없다' }});
if (tagOf(el) === 'A' && (attr(el, 'target') || '').toLowerCase() === '_blank' && /^https?:/.test(el.href)) return JSON.stringify({{ ok: true, nav: el.href }});
let target = el;
if (!vis(el) && tagOf(el) === 'INPUT' && el.labels) {{ const l = [...el.labels].find(vis); if (l) target = l; }}
target.scrollIntoView({{ block: 'center', inline: 'center', behavior: 'instant' }});
const r = target.getBoundingClientRect();
const x = Math.max(1, Math.min(innerWidth - 2, r.left + r.width / 2));
const y = Math.max(1, Math.min(innerHeight - 2, r.top + r.height / 2));
const hit = document.elementFromPoint(x, y);
const lab = hit && hit.closest ? hit.closest('label') : null;
const ok = !!hit && (hit === target || target.contains(hit) || (!!lab && !!el.labels && [...el.labels].includes(lab)));
return JSON.stringify({{ ok, x, y, why: ok ? null : '다른 요소에 가려져 있다' + (hit ? ' (' + clip(labelOf(hit) || tagOf(hit), 40) + ')' : '') }});
}})()"#,
        r = js_str(r)
    )
}

#[derive(Deserialize)]
struct PointJs {
    ok: bool,
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    nav: Option<String>,
}

/// 글 칸에 초점을 주고 지금 값을 비운다(React 같은 틀도 알아차리게 원형 setter와 input 이벤트로).
fn focus_clear_js(r: &str) -> String {
    format!(
        r#"(() => {{
{PRELUDE}
const el = byRef({r});
if (!el) return JSON.stringify({{ ok: false, why: '요소가 없다' }});
el.scrollIntoView({{ block: 'center', behavior: 'instant' }});
el.focus();
const tag = tagOf(el);
const active = document.activeElement;
if (active !== el && !(el.isContentEditable && el.contains(active))) return JSON.stringify({{ ok: false, why: '초점을 줄 수 없다' }});
if (tag === 'INPUT' || tag === 'TEXTAREA') {{
  const proto = tag === 'INPUT' ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
  Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, '');
  el.dispatchEvent(new Event('input', {{ bubbles: true }}));
}} else if (el.isContentEditable) {{
  const s = getSelection(); const rg = document.createRange(); rg.selectNodeContents(el); s.removeAllRanges(); s.addRange(rg);
}}
return JSON.stringify({{ ok: true }});
}})()"#,
        r = js_str(r)
    )
}

fn select_js(r: &str, want: &str) -> String {
    format!(
        r#"(() => {{
{PRELUDE}
const el = byRef({r});
if (!el || tagOf(el) !== 'SELECT') return JSON.stringify({{ ok: false, why: '선택 상자가 아니다' }});
const w = one({want}).toLowerCase();
const opts = [...el.options];
const opt = opts.find(o => one(o.label).toLowerCase() === w) || opts.find(o => String(o.value).toLowerCase() === w) || opts.find(o => one(o.label).toLowerCase().includes(w));
if (!opt) return JSON.stringify({{ ok: false, why: '그런 선택지가 없다' }});
el.value = opt.value;
el.dispatchEvent(new Event('input', {{ bubbles: true }}));
el.dispatchEvent(new Event('change', {{ bubbles: true }}));
return JSON.stringify({{ ok: true, chosen: one(opt.label) }});
}})()"#,
        r = js_str(r),
        want = js_str(want)
    )
}

#[derive(Deserialize)]
struct OkJs {
    ok: bool,
    #[serde(default)]
    why: Option<String>,
}

fn scroll_js(to: Scroll) -> String {
    let step = match to {
        Scroll::Down => "scrollBy({ top: innerHeight * 0.85, behavior: 'instant' })",
        Scroll::Up => "scrollBy({ top: -innerHeight * 0.85, behavior: 'instant' })",
        Scroll::Top => "scrollTo({ top: 0, behavior: 'instant' })",
        Scroll::Bottom => {
            "scrollTo({ top: document.documentElement.scrollHeight, behavior: 'instant' })"
        }
    };
    format!("(() => {{ {step}; return true; }})()")
}

/// 일반 프로필의 로그인 칸 찾기: 보이는 비밀번호 칸 하나, 그 앞의 아이디 칸, 로그인 유지 체크, 로그인 버튼.
fn detect_login_js() -> String {
    format!(
        r#"(() => {{
{PRELUDE}
const A = '{AUTO_ATTR}';
for (const e of document.querySelectorAll('[' + A + ']')) P.remAttr.call(e, A);
const pws = [...document.querySelectorAll('input[type=password]')].filter(e => vis(e) && !e.disabled && !e.readOnly);
if (pws.length === 0) return JSON.stringify({{ ok: false, pw: false, why: '비밀번호 칸이 없다' }});
if (pws.length > 1) return JSON.stringify({{ ok: false, pw: true, why: '비밀번호 칸이 여러 개다(가입이나 비밀번호 바꾸기 화면일 수 있다)' }});
const pw = pws[0];
const form = pw.form;
const pool = form ? formPool(form) : [...document.querySelectorAll('input, button')];
const before = pool.filter(e => tagOf(e) === 'INPUT' && ['text', 'email', 'tel', ''].includes((attr(e, 'type') || '').toLowerCase())
  && vis(e) && !e.disabled && (e.compareDocumentPosition(pw) & Node.DOCUMENT_POSITION_FOLLOWING));
const user = before.find(e => /username|email/.test(attr(e, 'autocomplete') || '')) || before[before.length - 1];
if (!user) return JSON.stringify({{ ok: false, pw: true, why: '아이디 칸을 찾지 못했다' }});
P.setAttr.call(user, A, 'user');
P.setAttr.call(pw, A, 'pw');
const keep = /remember|keep|stay|persist|auto|유지|자동|기억/i;
const remember = pool.find(e => tagOf(e) === 'INPUT' && (attr(e, 'type') || '').toLowerCase() === 'checkbox'
  && keep.test([attr(e, 'id'), attr(e, 'name'), e.labels ? [...e.labels].map(l => l.innerText).join(' ') : ''].join(' ')));
if (remember) P.setAttr.call(remember, A, 'remember');
const word = /log ?in|sign ?in|로그인|확인/i;
let submit = pool.find(e => vis(e) && !e.disabled && ((tagOf(e) === 'BUTTON' && typeOf(e) === 'submit') || (tagOf(e) === 'INPUT' && ['submit', 'image'].includes(typeOf(e)))));
if (!submit) submit = [...(form || document).querySelectorAll('button, input[type=button], a[role=button], [role=button]')].find(e => vis(e) && word.test(labelOf(e)));
if (submit) P.setAttr.call(submit, A, 'submit');
return JSON.stringify({{ ok: true, pw: true, submit: !!submit }});
}})()"#
    )
}

/// 단계형 로그인 첫 단계(결정 0017): 비밀번호 칸 없이 아이디 칸 하나와 "다음" 단추만 있는 화면.
/// 아이디 칸이 하나로 정해지지 않으면(여러 개) 하지 않는다.
fn detect_username_step_js() -> String {
    format!(
        r#"(() => {{
{PRELUDE}
const A = '{AUTO_ATTR}';
for (const e of document.querySelectorAll('[' + A + ']')) P.remAttr.call(e, A);
if (window.top !== window) return JSON.stringify({{ ok: false, pw: false, why: '최상위 창이 아니다' }});
const texts = [...document.querySelectorAll('input')].filter(e => ['text', 'email', 'tel', ''].includes((attr(e, 'type') || '').toLowerCase())
  && vis(e) && !e.disabled && !e.readOnly);
const named = texts.filter(e => /username|email/.test(attr(e, 'autocomplete') || ''));
const user = named.length === 1 ? named[0] : (texts.length === 1 ? texts[0] : null);
if (!user) return JSON.stringify({{ ok: false, pw: false, why: texts.length ? '아이디 칸이 여러 개라 정하지 못했다' : '아이디 칸이 없다' }});
P.setAttr.call(user, A, 'user');
const form = user.form;
const pool = form ? formPool(form) : [...document.querySelectorAll('button, input')];
const word = /다음|계속|next|continue|로그인|log ?in|sign ?in|확인/i;
let next = pool.find(e => vis(e) && !e.disabled && ((tagOf(e) === 'BUTTON' && typeOf(e) === 'submit') || (tagOf(e) === 'INPUT' && ['submit', 'image'].includes(typeOf(e)))));
if (!next) next = [...(form || document).querySelectorAll('button, input[type=button], a[role=button], [role=button]')].find(e => vis(e) && word.test(labelOf(e)));
if (next) P.setAttr.call(next, A, 'next');
return JSON.stringify({{ ok: true, pw: false, submit: !!next }});
}})()"#
    )
}

/// 단계형 로그인 둘째 단계: 비밀번호 칸 하나(아이디 칸은 없어도 된다)와 로그인 단추.
fn detect_password_step_js() -> String {
    format!(
        r#"(() => {{
{PRELUDE}
const A = '{AUTO_ATTR}';
for (const e of document.querySelectorAll('[' + A + ']')) P.remAttr.call(e, A);
if (window.top !== window) return JSON.stringify({{ ok: false, pw: false, why: '최상위 창이 아니다' }});
const pws = [...document.querySelectorAll('input[type=password]')].filter(e => vis(e) && !e.disabled && !e.readOnly);
if (pws.length === 0) return JSON.stringify({{ ok: false, pw: false, why: '비밀번호 칸이 없다' }});
if (pws.length > 1) return JSON.stringify({{ ok: false, pw: true, why: '비밀번호 칸이 여러 개다(가입이나 비밀번호 바꾸기 화면일 수 있다)' }});
const pw = pws[0];
P.setAttr.call(pw, A, 'pw');
const form = pw.form;
const pool = form ? formPool(form) : [...document.querySelectorAll('input, button')];
const keep = /remember|keep|stay|persist|auto|유지|자동|기억/i;
const remember = pool.find(e => tagOf(e) === 'INPUT' && (attr(e, 'type') || '').toLowerCase() === 'checkbox'
  && keep.test([attr(e, 'id'), attr(e, 'name'), e.labels ? [...e.labels].map(l => l.innerText).join(' ') : ''].join(' ')));
if (remember) P.setAttr.call(remember, A, 'remember');
const word = /log ?in|sign ?in|로그인|확인|다음|next/i;
let submit = pool.find(e => vis(e) && !e.disabled && ((tagOf(e) === 'BUTTON' && typeOf(e) === 'submit') || (tagOf(e) === 'INPUT' && ['submit', 'image'].includes(typeOf(e)))));
if (!submit) submit = [...(form || document).querySelectorAll('button, input[type=button], a[role=button], [role=button]')].find(e => vis(e) && word.test(labelOf(e)));
if (submit) P.setAttr.call(submit, A, 'submit');
return JSON.stringify({{ ok: true, pw: true, submit: !!submit }});
}})()"#
    )
}

/// 인증 코드(OTP) 칸 찾기(결정 0017): 비밀번호 칸이 아니고, `autocomplete=one-time-code`이거나 이름·라벨이
/// 인증 코드를 뜻하는 칸이 하나뿐일 때만. 애매하면 찾지 않는다(사람이 넘겨받는다).
fn detect_code_js() -> String {
    format!(
        r#"(() => {{
{PRELUDE}
const A = '{AUTO_ATTR}';
for (const e of document.querySelectorAll('[' + A + '=code], [' + A + '=code-submit]')) P.remAttr.call(e, A);
if (window.top !== window) return JSON.stringify({{ ok: false }});
const word = /otp|one.?time|verif|2fa|two.?factor|mfa|인증 ?(번호|코드)|보안 ?(번호|코드)|확인 ?코드|일회용/i;
const text = e => [attr(e, 'name'), attr(e, 'id'), attr(e, 'placeholder'), attr(e, 'aria-label'),
  e.labels ? [...e.labels].map(l => l.innerText).join(' ') : ''].join(' ');
const cands = [...document.querySelectorAll('input')].filter(e => vis(e) && !e.disabled && !e.readOnly
  && ['text', 'tel', 'number', ''].includes((attr(e, 'type') || '').toLowerCase())
  && ((attr(e, 'autocomplete') || '') === 'one-time-code' || word.test(text(e))));
if (cands.length !== 1) return JSON.stringify({{ ok: false }});
if (document.querySelector('input[type=password]') && [...document.querySelectorAll('input[type=password]')].some(vis))
  return JSON.stringify({{ ok: false }});
const code = cands[0];
P.setAttr.call(code, A, 'code');
const form = code.form;
const pool = form ? formPool(form) : [...document.querySelectorAll('button, input')];
const sw = /확인|인증|제출|로그인|다음|verify|submit|continue|confirm|sign ?in|log ?in/i;
let submit = pool.find(e => vis(e) && !e.disabled && ((tagOf(e) === 'BUTTON' && typeOf(e) === 'submit') || (tagOf(e) === 'INPUT' && ['submit', 'image'].includes(typeOf(e)))));
if (!submit) submit = [...(form || document).querySelectorAll('button, input[type=button], [role=button]')].find(e => vis(e) && sw.test(labelOf(e)));
if (submit) P.setAttr.call(submit, A, 'code-submit');
const hint = one(code.labels && code.labels.length ? [...code.labels].map(l => l.innerText).join(' ') : (attr(code, 'placeholder') || attr(code, 'aria-label') || '인증 코드')).slice(0, 40);
return JSON.stringify({{ ok: true, hint, submit: !!submit, max: parseInt(attr(code, 'maxlength') || '0') || 0 }});
}})()"#
    )
}

#[derive(Deserialize)]
struct CodeJs {
    ok: bool,
    #[serde(default)]
    hint: String,
    #[serde(default)]
    submit: bool,
}

/// 코드 칸 선택자.
fn code_selector() -> String {
    format!("[{AUTO_ATTR}=\"code\"]")
}

/// 단계형 로그인에서 비밀번호 칸을 기다리는 시간과 간격 [제안].
const STEP_WAIT: Duration = Duration::from_secs(15);
const STEP_POLL: Duration = Duration::from_millis(500);

#[derive(Deserialize)]
struct DetectJs {
    ok: bool,
    pw: bool,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    submit: bool,
}

/// 로그인 칸에 초점을 주고, 지금 이 사이트의 최상위 문서에서 그 칸에 초점이 있는지 확인한다.
/// 글자 하나의 자판 키: (key, code, 가상 키 코드, Shift). 자판에서 바로 치는 영문·숫자·기호만.
fn key_of(c: char) -> Option<(String, String, i64, bool)> {
    const SHIFTED: &str = "~!@#$%^&*()_+{}|:\"<>?";
    const PLAIN: &str = "`1234567890-=[]\\;',./";
    let (code, vk): (String, i64) = match c {
        'a'..='z' => (
            format!("Key{}", c.to_ascii_uppercase()),
            c.to_ascii_uppercase() as i64,
        ),
        'A'..='Z' => (format!("Key{c}"), c as i64),
        '0'..='9' => (format!("Digit{c}"), c as i64),
        ' ' => ("Space".into(), 32),
        _ if c.is_ascii_graphic() => {
            // 기호: 같은 자리의 키(US 자판).
            let base = SHIFTED
                .find(c)
                .map(|i| PLAIN.chars().nth(i).unwrap_or(c))
                .unwrap_or(c);
            let code = match base {
                '`' => "Backquote",
                '-' => "Minus",
                '=' => "Equal",
                '[' => "BracketLeft",
                ']' => "BracketRight",
                '\\' => "Backslash",
                ';' => "Semicolon",
                '\'' => "Quote",
                ',' => "Comma",
                '.' => "Period",
                '/' => "Slash",
                '0'..='9' => "",
                _ => "",
            };
            let code = if base.is_ascii_digit() {
                format!("Digit{base}")
            } else {
                code.to_owned()
            };
            (code, 0)
        }
        _ => return None,
    };
    let shift = c.is_ascii_uppercase() || "~!@#$%^&*()_+{}|:\"<>?".contains(c);
    Some((c.to_string(), code, vk, shift))
}

/// `clear`: 넣기 전에 칸을 비운다(넣을 때). 제출하려고 초점만 줄 때는 비우지 않는다(넣은 값을 지우지 않게).
fn login_focus_js(selector: &str, password: bool, clear: bool) -> String {
    format!(
        r#"(() => {{
{PRELUDE}
const el = document.querySelector({sel});
if (!el || tagOf(el) !== 'INPUT') return JSON.stringify({{ ok: false, why: '칸이 없다', origin: location.origin }});
if ((typeOf(el) === 'password') !== {password}) return JSON.stringify({{ ok: false, why: '칸 종류가 다르다', origin: location.origin }});
if ({clear}) Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, '');
el.focus();
return JSON.stringify({{ ok: document.activeElement === el && window.top === window, origin: location.origin }});
}})()"#,
        sel = js_str(selector),
    )
}

/// 로그인 상태 읽기: 주소, 비밀번호 칸(선택자·보이는 것), 사이트가 보인 오류 문구, 본문(입력값이 드러났는지만 본다).
fn login_state_js(password_selector: &str) -> String {
    format!(
        r#"(() => {{
{PRELUDE}
return JSON.stringify({{
  url: location.href,
  pw: !!document.querySelector({pw}),
  pw_any: [...document.querySelectorAll('input[type=password]')].some(vis),
  err: [...document.querySelectorAll('.invalid-feedback, .alert, .error, .help-block, [role=alert], .swal2-html-container, .swal-text')]
    .map(e => one(e.innerText)).filter(Boolean).join(' / ').slice(0, 300),
  body: (document.body ? document.body.innerText : '').slice(0, 200000),
  challenge: (() => {{
    // 사람이 풀어야 하는 단계(2026-11-18 운영: 네이버가 "추가 보안 확인"을 띄웠는데 비밀번호 칸이 없어 성공으로 봤다).
    const t = (document.body ? document.body.innerText : '').slice(0, 20000);
    const w = /captcha|security verification|verify (that )?you('| a)re (a )?human|not a robot|unusual (activity|traffic)|자동\s*입력\s*방지|추가\s*보안|보안\s*(확인|문자)|로봇이 아닙니다|사람인지 확인|새로운 (기기|환경|브라우저)|기기 등록|2단계 인증|two.?step|2-step|추가 인증/i;
    const m = t.match(w);
    if (m) return one(m[0]);
    const frames = [...document.querySelectorAll('iframe')].some(f => /recaptcha|hcaptcha|turnstile|captcha/i.test(attr(f, 'src') || ''));
    const els = document.querySelector('[id*=captcha i],[name*=captcha i],[class*=captcha i]');
    return frames || (els && vis(els)) ? 'captcha' : '';
  }})(),
}});
}})()"#,
        pw = js_str(password_selector),
    )
}

#[derive(Deserialize)]
struct StateJs {
    url: String,
    pw: bool,
    pw_any: bool,
    err: String,
    body: String,
    /// 사람 확인 화면(캡차·추가 보안 확인·새 기기 확인 등)으로 보이면 그 문구(아니면 빈 글).
    #[serde(default)]
    challenge: String,
}

/// 사이트가 보인 문구에서 비밀번호를 가린다.
fn scrub(text: &str, password: &str) -> String {
    if password.chars().count() >= 3 {
        text.replace(password, crate::mask::MASK)
    } else {
        text.to_owned()
    }
}

impl Live {
    fn is_mine(&self, e: &Event) -> bool {
        e.session.as_deref() == Some(self.session.as_str())
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.cdp
            .call(Some(&self.session), method, params, CALL)
            .await
    }

    /// 격리된 실행 공간(없으면 만든다).
    async fn world(&self) -> Result<i64, String> {
        if let Some(id) = *self.world.lock().expect("world lock") {
            return Ok(id);
        }
        let created = self
            .call(
                "Page.createIsolatedWorld",
                json!({"frameId": self.target, "worldName": "lucia", "grantUniveralAccess": false}),
            )
            .await?;
        let id = created["executionContextId"]
            .as_i64()
            .ok_or("격리된 실행 공간을 만들지 못했다")?;
        *self.world.lock().expect("world lock") = Some(id);
        Ok(id)
    }

    fn forget_world(&self) {
        *self.world.lock().expect("world lock") = None;
        self.frame_worlds.lock().expect("frame worlds lock").clear();
    }

    async fn call_in(&self, session: &str, method: &str, params: Value) -> Result<Value, String> {
        self.cdp.call(Some(session), method, params, CALL).await
    }

    fn is_main(&self, session: &str, frame: &str) -> bool {
        session == self.session && frame == self.target
    }

    /// 프레임의 격리된 실행 공간(결정 0023).
    async fn world_in(&self, session: &str, frame: &str) -> Result<i64, String> {
        if self.is_main(session, frame) {
            return self.world().await;
        }
        let key = (session.to_owned(), frame.to_owned());
        if let Some(id) = self
            .frame_worlds
            .lock()
            .expect("frame worlds lock")
            .get(&key)
        {
            return Ok(*id);
        }
        let created = self
            .call_in(
                session,
                "Page.createIsolatedWorld",
                json!({"frameId": frame, "worldName": "lucia", "grantUniveralAccess": false}),
            )
            .await?;
        let id = created["executionContextId"]
            .as_i64()
            .ok_or("iframe의 격리된 실행 공간을 만들지 못했다")?;
        self.frame_worlds
            .lock()
            .expect("frame worlds lock")
            .insert(key, id);
        Ok(id)
    }

    /// 프레임의 격리된 실행 공간에서 JSON 문자열을 돌려주는 식을 평가한다. 공간이 사라졌으면 한 번 다시 만든다.
    async fn eval_json_in<T: for<'de> Deserialize<'de>>(
        &self,
        session: &str,
        frame: &str,
        expression: &str,
    ) -> Result<T, String> {
        if self.is_main(session, frame) {
            return self.eval_json(expression).await;
        }
        let mut last = String::new();
        for _ in 0..2 {
            let ctx = self.world_in(session, frame).await?;
            let r = self
                .call_in(
                    session,
                    "Runtime.evaluate",
                    json!({"expression": expression, "returnByValue": true, "contextId": ctx}),
                )
                .await;
            match r {
                Ok(r) if r.get("exceptionDetails").is_none() => {
                    let text = r["result"]["value"]
                        .as_str()
                        .ok_or("페이지 스크립트가 문자열을 돌려주지 않았다")?;
                    return serde_json::from_str(text)
                        .map_err(|e| format!("페이지 스크립트 결과: {e}"));
                }
                Ok(r) => last = format!("페이지 스크립트 오류: {}", r["exceptionDetails"]["text"]),
                Err(e) => last = e,
            }
            self.frame_worlds
                .lock()
                .expect("frame worlds lock")
                .remove(&(session.to_owned(), frame.to_owned()));
        }
        Err(last)
    }

    /// 모든 프레임(세션, 프레임 ID): 최상위 문서의 프레임 트리(같은 프로세스 iframe)와 붙은 iframe 세션들의 트리.
    async fn all_frames(&self) -> Vec<(String, String)> {
        fn walk(node: &Value, session: &str, out: &mut Vec<(String, String)>) {
            if let Some(id) = node["frame"]["id"].as_str() {
                out.push((session.to_owned(), id.to_owned()));
            }
            for c in node["childFrames"].as_array().into_iter().flatten() {
                walk(c, session, out);
            }
        }
        let mut out = Vec::new();
        let mut sessions = vec![self.session.clone()];
        sessions.extend(
            self.children
                .lock()
                .expect("children lock")
                .iter()
                .map(|c| c.session.clone()),
        );
        for s in sessions {
            if let Ok(tree) = self.call_in(&s, "Page.getFrameTree", json!({})).await {
                walk(&tree["frameTree"], &s, &mut out);
            }
        }
        out
    }

    /// iframe 요소의 안쪽 영역(그 요소가 있는 세션의 화면 기준): (x, y, 너비, 높이). 브라우저가 계산한 값이다
    /// (페이지 스크립트가 꾸밀 수 없다).
    async fn owner_box(&self, session: &str, frame: &str) -> Option<(f64, f64, f64, f64)> {
        let owner = self
            .call_in(session, "DOM.getFrameOwner", json!({"frameId": frame}))
            .await
            .ok()?;
        let node = owner["backendNodeId"].as_i64()?;
        let bm = self
            .call_in(session, "DOM.getBoxModel", json!({"backendNodeId": node}))
            .await
            .ok()?;
        let q = bm["model"]["content"].as_array()?;
        let x = q.first()?.as_f64()?;
        let y = q.get(1)?.as_f64()?;
        let w = q.get(2)?.as_f64()? - x;
        let h = q.get(5)?.as_f64()? - y;
        Some((x, y, w, h))
    }

    /// 화면의 iframe들(결정 0023): 보이는 것만, 글과 요소의 화면 좌표. 큰 것부터.
    async fn read_frames(&self, viewport: (i32, i32)) -> Vec<FrameView> {
        let mut frames = self.read_frames_unsorted(viewport).await;
        frames.sort_by_key(|f| std::cmp::Reverse(i64::from(f.rect.2) * i64::from(f.rect.3)));
        frames
    }

    async fn read_frames_unsorted(&self, viewport: (i32, i32)) -> Vec<FrameView> {
        let children = self.children.lock().expect("children lock").clone();
        let mut origin: HashMap<String, (f64, f64)> = HashMap::new();
        origin.insert(self.session.clone(), (0.0, 0.0));
        let mut out = Vec::new();
        let frames = self.all_frames().await;
        tracing::debug!(?frames, children = children.len(), "화면의 프레임");
        for (session, frame) in frames.into_iter().take(FRAMES_SCAN * 2) {
            if self.is_main(&session, &frame) || out.len() >= FRAMES_SCAN {
                continue;
            }
            // 별도 프로세스 iframe의 맨 위 프레임이면 그 요소는 부모 세션에 있다.
            let root = children
                .iter()
                .find(|c| c.frame == frame && c.session == session);
            let (owner_session, base) = match root {
                Some(c) => match origin.get(&c.parent_session) {
                    Some(b) => (c.parent_session.clone(), *b),
                    None => continue,
                },
                None => match origin.get(&session) {
                    Some(b) => (session.clone(), *b),
                    None => continue,
                },
            };
            let Some((x, y, w, h)) = self.owner_box(&owner_session, &frame).await else {
                tracing::debug!(frame, "iframe 위치를 읽지 못했다");
                continue;
            };
            let (fx, fy) = (base.0 + x, base.1 + y);
            if root.is_some() {
                origin.insert(session.clone(), (fx, fy));
            }
            // 작거나(추적 픽셀·광고 조각) 화면 밖이면 넘긴다.
            let (vw, vh) = (f64::from(viewport.0), f64::from(viewport.1));
            if w * h < 2500.0 || fx + w <= 0.0 || fy + h <= 0.0 || fx >= vw || fy >= vh {
                continue;
            }
            let v = match self
                .eval_json_in::<FrameViewJs>(&session, &frame, &frame_view_js())
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::debug!(frame, err = %e, "iframe을 읽지 못했다");
                    continue;
                }
            };
            if v.text.trim().is_empty() && v.elements.is_empty() {
                continue;
            }
            let (l, t) = (fx.max(0.0), fy.max(0.0));
            let (r, b) = ((fx + w).min(vw), (fy + h).min(vh));
            let elements = v
                .elements
                .into_iter()
                .filter_map(|p| {
                    let (px, py) = (fx + f64::from(p.x), fy + f64::from(p.y));
                    if px < l || py < t || px >= r || py >= b {
                        return None;
                    }
                    let mut el = p.el;
                    el.frame = Some(v.url.clone());
                    Some((
                        super::Point {
                            x: px.round() as i32,
                            y: py.round() as i32,
                        },
                        el,
                    ))
                })
                .collect();
            out.push(FrameView {
                url: v.url,
                rect: (
                    fx.round() as i32,
                    fy.round() as i32,
                    w.round() as i32,
                    h.round() as i32,
                ),
                text: v.text,
                elements,
            });
        }
        out
    }

    /// 화면 좌표의 요소(결정 0023). 히트 테스트는 문서 좌표(화면 + 스크롤)를 받고 화면 안만 맞힌다(실측).
    async fn hit(&self, at: super::Point) -> Result<Hit, String> {
        let m = self.call("Page.getLayoutMetrics", json!({})).await?;
        let vv = &m["cssVisualViewport"];
        let scroll = (
            vv["pageX"].as_f64().unwrap_or(0.0),
            vv["pageY"].as_f64().unwrap_or(0.0),
        );
        let viewport = (
            vv["clientWidth"].as_f64().unwrap_or(0.0).round() as i32,
            vv["clientHeight"].as_f64().unwrap_or(0.0).round() as i32,
        );
        let url = self.location().await?;
        let mut hit = Hit {
            url,
            viewport,
            element: None,
            pointer: false,
            why: None,
        };
        if at.x < 0 || at.y < 0 || at.x >= viewport.0 || at.y >= viewport.1 {
            hit.why = Some("화면 밖".into());
            return Ok(hit);
        }
        let session = self.session.clone();
        *self.acted_frame.lock().expect("acted lock") = None;
        match self
            .hit_in(session, (f64::from(at.x), f64::from(at.y)), scroll, 0)
            .await
        {
            Probed::Found { el, pointer } => {
                hit.element = Some(el);
                hit.pointer = pointer;
            }
            Probed::Unknown(why) => hit.why = Some(why),
        }
        Ok(hit)
    }

    /// 한 세션(그 세션의 맨 위 프레임 화면 기준 좌표 `v`, 그 문서의 스크롤 `scroll`) 안의 히트 테스트.
    /// 별도 프로세스 iframe 요소에 닿으면 그 iframe 세션으로 내려간다.
    fn hit_in(
        &self,
        session: String,
        v: (f64, f64),
        scroll: (f64, f64),
        depth: u8,
    ) -> BoxFut<'_, Probed> {
        Box::pin(async move {
            let r = match self
                .call_in(
                    &session,
                    "DOM.getNodeForLocation",
                    json!({"x": (v.0 + scroll.0).round() as i64, "y": (v.1 + scroll.1).round() as i64,
                           "includeUserAgentShadowDOM": false, "ignorePointerEventsNone": false}),
                )
                .await
            {
                Ok(r) => r,
                Err(e) => return Probed::Unknown(format!("그 자리를 읽지 못했다: {e}")),
            };
            let (Some(node), Some(frame)) = (r["backendNodeId"].as_i64(), r["frameId"].as_str())
            else {
                return Probed::Unknown("그 자리를 읽지 못했다".into());
            };
            let frame = frame.to_owned();
            let probe = async {
                let world = self.world_in(&session, &frame).await?;
                let obj = self
                    .call_in(
                        &session,
                        "DOM.resolveNode",
                        json!({"backendNodeId": node, "executionContextId": world}),
                    )
                    .await?;
                let oid = obj["object"]["objectId"]
                    .as_str()
                    .ok_or("노드를 읽지 못했다")?
                    .to_owned();
                let out = self
                    .call_in(
                        &session,
                        "Runtime.callFunctionOn",
                        json!({"objectId": oid, "functionDeclaration": probe_fn(), "returnByValue": true}),
                    )
                    .await;
                let _ = self
                    .call_in(&session, "Runtime.releaseObject", json!({"objectId": oid}))
                    .await;
                let out = out?;
                let text = out["result"]["value"]
                    .as_str()
                    .ok_or("그 자리를 읽지 못했다")?;
                serde_json::from_str::<ProbeJs>(text).map_err(|e| e.to_string())
            };
            let p: ProbeJs = match probe.await {
                Ok(p) => p,
                Err(e) => {
                    self.frame_worlds
                        .lock()
                        .expect("frame worlds lock")
                        .remove(&(session.clone(), frame.clone()));
                    return Probed::Unknown(format!("그 자리를 읽지 못했다: {e}"));
                }
            };
            if !p.frame {
                let Some(mut el) = p.el else {
                    return Probed::Unknown("그 자리에 요소가 없다".into());
                };
                if !self.is_main(&session, &frame) {
                    el.frame = Some(p.url);
                    *self.acted_frame.lock().expect("acted lock") = Some(frame.clone());
                }
                return Probed::Found {
                    el,
                    pointer: p.pointer,
                };
            }
            // 다른 사이트 iframe: 그 iframe 세션에서 그 안의 좌표로 다시 본다.
            if depth >= 3 {
                return Probed::Unknown("iframe이 너무 깊이 겹쳐 있다".into());
            }
            let candidates: Vec<ChildFrame> = self
                .children
                .lock()
                .expect("children lock")
                .iter()
                .filter(|c| c.parent_session == session)
                .cloned()
                .collect();
            let mut found = None;
            for c in candidates {
                let owner = self
                    .call_in(&session, "DOM.getFrameOwner", json!({"frameId": c.frame}))
                    .await;
                if owner.ok().and_then(|o| o["backendNodeId"].as_i64()) == Some(node) {
                    found = Some(c);
                    break;
                }
            }
            let Some(child) = found else {
                return Probed::Unknown("다른 사이트의 iframe 안이라 읽지 못했다".into());
            };
            let Some((x, y, w, h)) = self.owner_box(&session, &child.frame).await else {
                return Probed::Unknown("iframe 위치를 읽지 못했다".into());
            };
            let local = (v.0 - x, v.1 - y);
            if local.0 < 0.0 || local.1 < 0.0 || local.0 >= w || local.1 >= h {
                // iframe의 테두리·여백.
                let mut el = super::Element::unknown("iframe 테두리");
                el.kind = "area".into();
                return Probed::Found { el, pointer: false };
            }
            #[derive(Deserialize)]
            struct Inner(f64, f64, f64, f64);
            let Ok(s) = self
                .eval_json_in::<Inner>(
                    &child.session,
                    &child.frame,
                    "JSON.stringify([scrollX, scrollY, innerWidth, innerHeight])",
                )
                .await
            else {
                return Probed::Unknown("iframe 안을 읽지 못했다".into());
            };
            // CSS로 늘이거나 줄이거나 돌린 iframe은 지역 좌표가 어긋난다: 판정할 수 없다.
            if (s.2 - w).abs() > 2.0 || (s.3 - h).abs() > 2.0 {
                return Probed::Unknown("iframe이 늘이거나 줄여 그려져 좌표를 맞출 수 없다".into());
            }
            self.hit_in(child.session, local, (s.0, s.1), depth + 1)
                .await
        })
    }

    /// 지금 초점(결정 0023). 프레임마다 그 문서의 초점을 읽어, 초점이 있는 문서의 것을 고른다.
    async fn focus(&self) -> Result<Focus, String> {
        let mut found: Vec<(bool, Element, Option<String>)> = Vec::new();
        let mut main_state = String::new();
        *self.acted_frame.lock().expect("acted lock") = None;
        // 광고 iframe이 많은 페이지에서도 초점이 있는 프레임까지 닿게 넉넉히 본다(프레임마다 짧은 읽기 하나).
        for (session, frame) in self.all_frames().await.into_iter().take(FRAMES_SCAN * 8) {
            let Ok(f) = self
                .eval_json_in::<FocusJs>(&session, &frame, &focus_js())
                .await
            else {
                continue;
            };
            let main = self.is_main(&session, &frame);
            if main {
                main_state = f.state.clone();
            }
            if f.state == "el"
                && let Some(mut el) = f.el
            {
                if !main {
                    el.frame = Some(f.url);
                }
                found.push((f.has, el, (!main).then(|| frame.clone())));
            }
        }
        if found.len() > 1 {
            found.retain(|(h, _, _)| *h);
        }
        Ok(match found.len() {
            0 if main_state == "frame" => Focus::Unknown {
                why: "iframe 안의 초점을 읽지 못했다".into(),
            },
            0 if main_state == "el" => Focus::Unknown {
                why: "초점이 있는 칸을 하나로 정하지 못했다".into(),
            },
            0 => Focus::Nothing,
            1 => {
                let (_, element, frame) = found.remove(0);
                *self.acted_frame.lock().expect("acted lock") = frame;
                Focus::On { element }
            }
            _ => Focus::Unknown {
                why: "초점이 있는 칸을 하나로 정하지 못했다".into(),
            },
        })
    }

    /// 정해진 키 하나(결정 0023).
    async fn key(&self, name: &str) -> Result<(), String> {
        // (key, code, 가상 키, 글, 수정 키(1 Alt, 2 Ctrl, 4 Meta, 8 Shift), 편집 명령)
        let (key, code, vk, text, modifiers, command): (
            &str,
            &str,
            i64,
            Option<&str>,
            i64,
            Option<&str>,
        ) = match name {
            "enter" => ("Enter", "Enter", 13, Some("\r"), 0, None),
            "space" => (" ", "Space", 32, Some(" "), 0, None),
            "tab" => ("Tab", "Tab", 9, None, 0, None),
            "shift+tab" => ("Tab", "Tab", 9, None, 8, None),
            "escape" => ("Escape", "Escape", 27, None, 0, None),
            "backspace" => ("Backspace", "Backspace", 8, None, 0, None),
            "delete" => ("Delete", "Delete", 46, None, 0, None),
            "up" => ("ArrowUp", "ArrowUp", 38, None, 0, None),
            "down" => ("ArrowDown", "ArrowDown", 40, None, 0, None),
            "left" => ("ArrowLeft", "ArrowLeft", 37, None, 0, None),
            "right" => ("ArrowRight", "ArrowRight", 39, None, 0, None),
            "home" => ("Home", "Home", 36, None, 0, None),
            "end" => ("End", "End", 35, None, 0, None),
            "pageup" => ("PageUp", "PageUp", 33, None, 0, None),
            "pagedown" => ("PageDown", "PageDown", 34, None, 0, None),
            "select_all" => ("a", "KeyA", 65, None, 2, Some("selectAll")),
            other => return Err(format!("모르는 키: {other}")),
        };
        for kind in ["keyDown", "keyUp"] {
            let mut p = json!({"type": kind, "key": key, "code": code, "modifiers": modifiers,
                "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk});
            if kind == "keyDown" {
                if let Some(t) = text {
                    p["text"] = json!(t);
                }
                if let Some(c) = command {
                    p["commands"] = json!([c]);
                }
            }
            self.call("Input.dispatchKeyEvent", p).await?;
        }
        Ok(())
    }

    /// 그 점에서 휠을 굴린다(그 아래 스크롤 영역이 움직인다).
    async fn wheel(&self, at: super::Point, to: Scroll) -> Result<(), String> {
        let dy = match to {
            Scroll::Down => 600,
            Scroll::Up => -600,
            Scroll::Top => -100_000,
            Scroll::Bottom => 100_000,
        };
        self.call(
            "Input.dispatchMouseEvent",
            json!({"type": "mouseMoved", "x": at.x, "y": at.y}),
        )
        .await?;
        self.call(
            "Input.dispatchMouseEvent",
            json!({"type": "mouseWheel", "x": at.x, "y": at.y, "deltaX": 0, "deltaY": dy}),
        )
        .await
        .map(|_| ())
    }

    /// 데몬 스크립트를 격리된 실행 공간에서 돈다. 문서가 바뀌어 공간이 사라졌으면 한 번 다시 만든다.
    async fn eval(&self, expression: &str) -> Result<Value, String> {
        let mut last = String::new();
        for _ in 0..2 {
            let ctx = self.world().await?;
            let r = self
                .call(
                    "Runtime.evaluate",
                    json!({"expression": expression, "returnByValue": true, "contextId": ctx}),
                )
                .await;
            let r = match r {
                Ok(r) => r,
                Err(e) => {
                    last = e;
                    self.forget_world();
                    continue;
                }
            };
            if let Some(ex) = r.get("exceptionDetails") {
                return Err(format!(
                    "페이지 스크립트 오류: {}",
                    ex["exception"]["description"]
                        .as_str()
                        .or(ex["text"].as_str())
                        .unwrap_or("?")
                        .chars()
                        .take(200)
                        .collect::<String>()
                ));
            }
            return Ok(r["result"]["value"].clone());
        }
        Err(last)
    }

    /// JSON 문자열을 돌려주는 식을 평가해 풀어낸다.
    async fn eval_json<T: for<'de> Deserialize<'de>>(&self, expression: &str) -> Result<T, String> {
        let v = self.eval(expression).await?;
        let text = v
            .as_str()
            .ok_or("페이지 스크립트가 문자열을 돌려주지 않았다")?;
        serde_json::from_str(text).map_err(|e| format!("페이지 스크립트 결과: {e}"))
    }

    /// 지금 주소를 페이지 스크립트 없이 읽는다(넘어가는 도중에도 된다).
    async fn nav_url(&self) -> Result<String, String> {
        let h = self.call("Page.getNavigationHistory", json!({})).await?;
        let i = h["currentIndex"].as_u64().ok_or("기록 위치가 없다")? as usize;
        h["entries"][i]["url"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "주소를 읽지 못했다".into())
    }

    async fn location(&self) -> Result<String, String> {
        let v = self.eval("location.href").await?;
        v.as_str()
            .map(str::to_owned)
            .ok_or_else(|| "주소를 읽지 못했다".into())
    }

    async fn navigate(&self, url: &str) -> Result<(), String> {
        let mut events = self.cdp.subscribe();
        let r = self.call("Page.navigate", json!({"url": url})).await?;
        self.forget_world();
        if let Some(err) = r["errorText"].as_str().filter(|e| !e.is_empty()) {
            return Err(format!("페이지를 열 수 없다: {err}"));
        }
        if r.get("loaderId").is_some() {
            let loaded = wait_for(&mut events, LOAD, |e| {
                e.method == "Page.loadEventFired" && self.is_mine(e)
            })
            .await;
            if loaded.is_none() {
                tracing::warn!("페이지 로드 완료를 기다리다 넘어갔다");
            }
        }
        // 스크립트가 그리는 내용이 자리 잡을 시간.
        tokio::time::sleep(Duration::from_millis(700)).await;
        Ok(())
    }

    /// 동작 뒤: 최상위 문서의 이동이 시작되면 로드를 기다리고, 아니면 잠깐 기다린다.
    /// `frame`(그 동작이 닿은 iframe, 결정 0023)이 있으면 그 프레임의 이동도 짧은 상한으로 기다린다. 다른 iframe
    /// (스스로 다시 뜨는 광고 등)의 사건은 보지 않는다.
    async fn settle_in(&self, events: &mut broadcast::Receiver<Event>, frame: Option<&str>) {
        let target = self.target.clone();
        let started = wait_for(events, NAV_START, |e| {
            let f = e.params["frameId"].as_str();
            let nav = e.method == "Page.frameStartedLoading"
                || e.method == "Page.frameRequestedNavigation"
                || e.method == "Page.navigatedWithinDocument";
            nav && ((self.is_mine(e) && f == Some(target.as_str()))
                || (frame.is_some() && f == frame))
        })
        .await;
        if let Some(e) = started
            && e.method != "Page.navigatedWithinDocument"
        {
            self.forget_world();
            if e.params["frameId"].as_str() == Some(target.as_str()) {
                let _ = wait_for(events, LOAD, |e| {
                    e.method == "Page.loadEventFired" && self.is_mine(e)
                })
                .await;
            } else if let Some(f) = frame {
                let _ = wait_for(events, FRAME_LOAD, |e| {
                    e.method == "Page.frameStoppedLoading"
                        && e.params["frameId"].as_str() == Some(f)
                })
                .await;
            }
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }

    async fn settle(&self, events: &mut broadcast::Receiver<Event>) {
        self.settle_in(events, None).await;
    }

    /// 좌표·초점 동작 뒤(그 동작이 닿은 iframe의 이동까지).
    async fn settle_acted(&self, events: &mut broadcast::Receiver<Event>) {
        let frame = self.acted_frame.lock().expect("acted lock").clone();
        self.settle_in(events, frame.as_deref()).await;
    }

    async fn read_view(&self) -> Result<PageView, String> {
        let v: ViewJs = self.eval_json(&view_js()).await?;
        let viewport = (v.vw as i32, v.scroll.1 as i32);
        let frames = self.read_frames(viewport).await;
        Ok(PageView {
            frames,
            viewport,
            url: v.url,
            title: v.title,
            text: v.text,
            element_total: v.total,
            elements: v.elements,
            login_form: v.login_form,
            scroll: v.scroll,
            dialogs: std::mem::take(&mut *self.dialogs.lock().expect("dialogs lock")),
            popups: std::mem::take(&mut *self.popups.lock().expect("popups lock")),
            login_redirect: false,
        })
    }

    async fn inspect(&self, r: &str) -> Result<(String, Element), BrowserError> {
        if !valid_ref(r) {
            return Err(BrowserError::Stale(format!("요소 참조 형식이 아니다: {r}")));
        }
        let found: InspectJs = self
            .eval_json(&inspect_js(r))
            .await
            .map_err(BrowserError::Failed)?;
        match found.el {
            Some(el) => Ok((found.url, el)),
            None => Err(BrowserError::Stale(format!(
                "{r} 요소가 없다(화면이 바뀌었다). browser_snapshot으로 다시 본다"
            ))),
        }
    }

    async fn click_at(&self, x: f64, y: f64) -> Result<(), String> {
        for (kind, buttons) in [("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 0)] {
            let mut p = json!({"type": kind, "x": x, "y": y});
            if kind != "mouseMoved" {
                p["button"] = json!("left");
                p["clickCount"] = json!(1);
                p["buttons"] = json!(buttons);
            }
            self.call("Input.dispatchMouseEvent", p).await?;
        }
        Ok(())
    }

    async fn press_enter(&self) -> Result<(), String> {
        for kind in ["keyDown", "keyUp"] {
            let mut p = json!({
                "type": kind, "key": "Enter", "code": "Enter",
                "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13,
            });
            if kind == "keyDown" {
                p["text"] = json!("\r");
            }
            self.call("Input.dispatchKeyEvent", p).await?;
        }
        Ok(())
    }

    async fn insert(&self, text: &str) -> Result<(), String> {
        self.call("Input.insertText", json!({"text": text}))
            .await
            .map(|_| ())
    }

    /// 로그인 칸에 사람처럼 한 글자씩 친다(키 누름·뗌, 글자 사이 짧은 간격). 키 입력 기록으로 자동 입력을 가려내는
    /// 사이트(네이버 등)가 한 번에 넣은 글에 보안 확인을 띄우므로. 자판에 없는 글자는 그 글자만 그대로 넣는다.
    async fn type_text(&self, text: &str) -> Result<(), String> {
        for (i, c) in text.chars().enumerate() {
            match key_of(c) {
                Some((key, code, vk, shift)) => {
                    let modifiers = if shift { 8 } else { 0 };
                    let s = c.to_string();
                    self.call(
                        "Input.dispatchKeyEvent",
                        json!({
                            "type": "keyDown", "key": key, "code": code, "text": s, "unmodifiedText": s,
                            "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": modifiers,
                        }),
                    )
                    .await?;
                    self.call(
                        "Input.dispatchKeyEvent",
                        json!({
                            "type": "keyUp", "key": key, "code": code,
                            "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": modifiers,
                        }),
                    )
                    .await?;
                }
                None => self.insert(&c.to_string()).await?,
            }
            // 40~110ms(글자마다 다르게).
            let gap = 40 + (i as u64 * 37 + c as u64 * 13) % 70;
            tokio::time::sleep(Duration::from_millis(gap)).await;
        }
        Ok(())
    }

    /// 칸 글자 수(값은 꺼내지 않는다).
    async fn value_len(&self, selector: &str) -> Result<usize, String> {
        let v = self
            .eval(&format!(
                "(el => el ? [...el.value].length : -1)(document.querySelector({}))",
                js_str(selector)
            ))
            .await?;
        v.as_i64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| "칸이 사라졌다".into())
    }

    /// 로그인 칸에 초점을 주고, 지금 이 사이트의 최상위 문서에서 그 칸에 초점이 있는지 확인한다.
    async fn login_focus(
        &self,
        site: &SiteProfile,
        selector: &str,
        password: bool,
        clear: bool,
    ) -> Result<(), String> {
        #[derive(Deserialize)]
        struct Focus {
            ok: bool,
            why: Option<String>,
            origin: String,
        }
        let f: Focus = self
            .eval_json(&login_focus_js(selector, password, clear))
            .await?;
        if f.origin.trim_end_matches('/') != site.origin.trim_end_matches('/') {
            return Err(format!("다른 출처에 있다: {}", f.origin));
        }
        if !f.ok {
            return Err(f.why.unwrap_or_else(|| "칸에 초점을 줄 수 없다".into()));
        }
        Ok(())
    }

    /// 단계형 로그인의 첫 단계: 아이디를 넣고 "다음"을 누른 뒤, 같은 사이트에서 비밀번호 칸이 나올 때까지
    /// 기다린다(페이지가 넘어가지 않는 화면도 있어 주기적으로 본다). 다른 곳으로 넘어가면 그 페이지를 닫고 멈춘다.
    async fn username_step(&self, site: &SiteProfile, username: &str) -> Result<(), BrowserError> {
        let fail = BrowserError::Failed;
        let found: DetectJs = self
            .eval_json(&detect_username_step_js())
            .await
            .map_err(fail)?;
        if !found.ok {
            return Err(fail(
                found
                    .why
                    .unwrap_or_else(|| "아이디 칸을 찾지 못했다".into()),
            ));
        }
        let user = format!("[{AUTO_ATTR}=\"user\"]");
        self.login_focus(site, &user, false, true)
            .await
            .map_err(|e| fail(format!("아이디 칸: {e}")))?;
        self.type_text(username).await.map_err(fail)?;
        if self.value_len(&user).await.map_err(fail)? != username.chars().count() {
            return Err(fail("아이디 칸: 입력한 글자 수가 맞지 않는다".into()));
        }
        if found.submit {
            let clicked = self
                .eval(&format!(
                    "(el => {{ if (!el) return false; el.click(); return true; }})(document.querySelector({}))",
                    js_str(&format!("[{AUTO_ATTR}=\"next\"]"))
                ))
                .await
                .map_err(fail)?;
            if clicked != Value::Bool(true) {
                return Err(fail("다음 단추가 사라졌다".into()));
            }
        } else {
            self.press_enter().await.map_err(fail)?;
        }
        let started = std::time::Instant::now();
        let mut said = String::new();
        loop {
            tokio::time::sleep(STEP_POLL).await;
            // 주소는 페이지 스크립트 없이 먼저 본다(넘어가는 도중이면 스크립트 공간이 없어 오래 걸린다).
            if let Ok(url) = self.nav_url().await
                && let Ok(now_at) = Url::parse(&url)
                && !site.allows(&now_at)
                && url != "about:blank"
            {
                let _ = self.navigate("about:blank").await;
                return Err(BrowserError::NotAllowed(format!(
                    "아이디 다음 단계가 다른 곳으로 넘어가 비밀번호를 넣지 않았다: {}",
                    now_at.origin().ascii_serialization()
                )));
            }
            let read = tokio::time::timeout(
                Duration::from_secs(3),
                self.eval_json::<StateJs>(&login_state_js("input[type=password]")),
            )
            .await;
            if let Ok(Ok(state)) = read {
                let now_at = Url::parse(&state.url).map_err(|e| fail(format!("주소: {e}")))?;
                if !site.allows(&now_at) {
                    let _ = self.navigate("about:blank").await;
                    return Err(BrowserError::NotAllowed(format!(
                        "아이디 다음 단계가 다른 곳으로 넘어가 비밀번호를 넣지 않았다: {}",
                        now_at.host_str().unwrap_or("?")
                    )));
                }
                if state.pw_any {
                    return Ok(());
                }
                said = state.err;
            }
            if started.elapsed() >= STEP_WAIT {
                let why = if said.trim().is_empty() {
                    "아이디를 넣은 뒤 비밀번호 칸이 나오지 않았다".to_owned()
                } else {
                    format!("아이디를 넣은 뒤 사이트가 말했다: {said}")
                };
                return Err(fail(why));
            }
        }
    }

    async fn clear_code(&self) {
        let _ = self
            .eval(&format!(
                "(el => {{ if (el) Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, ''); }})(document.querySelector({}))",
                js_str(&code_selector())
            ))
            .await;
    }

    async fn clear_login(&self, site: &SiteProfile) {
        let _ = self
            .eval(&format!(
                "[{}, {}].forEach(s => {{ const el = document.querySelector(s); if (el) Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, ''); }})",
                js_str(&site.login.username),
                js_str(&site.login.password)
            ))
            .await;
    }
}

impl SbxBrowser {
    /// 도착한 주소를 검사한다. 막힌 곳이면 빈 페이지로 돌리고 `Err`.
    async fn arrival(&self, live: &Live, url: &str) -> Result<(), BrowserError> {
        if url == "about:blank" {
            return Ok(());
        }
        if url.starts_with("chrome-error:") {
            return Err(BrowserError::Failed(
                "페이지를 열지 못했다(연결 실패 또는 막힌 곳)".into(),
            ));
        }
        if let Err(why) = self.egress.check_url(url) {
            let _ = live.navigate("about:blank").await;
            return Err(BrowserError::NotAllowed(format!(
                "막힌 곳으로 넘어가 빈 페이지로 돌렸다: {why}"
            )));
        }
        Ok(())
    }

    async fn view_checked(&self, live: &Live) -> Result<PageView, BrowserError> {
        let view = live.read_view().await.map_err(BrowserError::Failed)?;
        self.arrival(live, &view.url).await?;
        Ok(view)
    }

    async fn do_open(
        &self,
        url: &str,
        site: Option<&SiteProfile>,
    ) -> Result<PageView, BrowserError> {
        let mut slot = self.live.lock().await;
        self.touch();
        let live = self.ensure(&mut slot).await?;
        live.dialogs.lock().expect("dialogs lock").clear();
        live.popups.lock().expect("popups lock").clear();
        live.navigate(url).await.map_err(BrowserError::Failed)?;
        let mut view = self.view_checked(live).await?;
        if let Some(site) = site
            && let Ok(arrived) = Url::parse(&view.url)
        {
            view.login_redirect = site.allows(&arrived) && site.is_login_path(&arrived);
        }
        self.touch();
        Ok(view)
    }

    /// 그 주소로 보낼 쿠키와 사용자 에이전트(결정 0036). 값은 로그에 남기지 않는다.
    /// 지금 페이지에서 스크립트를 돈다(결정 0059): 페이지 자신의 실행 공간에서, 사용자 동작처럼. 돌려준 값(JSON)과
    /// 그 뒤의 화면.
    async fn do_eval(&self, code: &str) -> Result<(String, PageView), BrowserError> {
        let slot = self.live.lock().await;
        self.touch();
        let live = Self::current(&slot)?;
        live.dialogs.lock().expect("dialogs lock").clear();
        live.popups.lock().expect("popups lock").clear();
        let mut events = live.cdp.subscribe();
        // `return`으로 값을 돌려줄 수 있게 비동기 함수로 감싼다. 값은 JSON으로 바꿀 수 있는 것만 온다.
        let expression = format!("(async () => {{\n{code}\n}})()");
        let r = live
            .cdp
            .call(
                Some(&live.session),
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "awaitPromise": true,
                    "returnByValue": true,
                    "userGesture": true,
                }),
                EVAL,
            )
            .await
            .map_err(BrowserError::Failed)?;
        if let Some(ex) = r.get("exceptionDetails") {
            return Err(BrowserError::Failed(format!(
                "스크립트 오류: {}",
                ex["exception"]["description"]
                    .as_str()
                    .or(ex["text"].as_str())
                    .unwrap_or("?")
                    .chars()
                    .take(600)
                    .collect::<String>()
            )));
        }
        let value = match &r["result"]["value"] {
            Value::Null => "(돌려준 값 없음)".to_owned(),
            Value::String(text) => text.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        };
        // 스크립트가 페이지를 옮겼을 수 있다.
        live.forget_world();
        live.settle(&mut events).await;
        let view = self.view_checked(live).await?;
        Ok((value, view))
    }

    /// 파일 고르기 칸에 파일을 넣는다(결정 0059). 파일은 브라우저 VM의 임시 자리에 두었다가(올릴 때 Chrome이 읽는다)
    /// 브라우저를 다시 띄울 때 지운다.
    async fn do_upload(&self, r: &str, name: &str, bytes: &[u8]) -> Result<PageView, BrowserError> {
        use tokio::io::AsyncWriteExt as _;
        if !valid_ref(r) {
            return Err(BrowserError::Stale(format!("요소 참조 형식이 아니다: {r}")));
        }
        let fail = BrowserError::Failed;
        let slot = self.live.lock().await;
        self.touch();
        let live = Self::current(&slot)?;
        let (_, el) = live.inspect(r).await?;
        if el.kind != "file" {
            return Err(BrowserError::Failed(format!(
                "{r}는 파일 칸이 아니다({})",
                el.kind
            )));
        }
        // 이름은 파일 보관소가 이미 다듬었다(경로 구분자가 없다). 위치 인자로 넘겨 쉘이 해석하지 않게 한다.
        let mut child = Command::new(&self.cfg.sbx)
            .args([
                "exec",
                "-i",
                &self.cfg.sandbox,
                "sh",
                "-c",
                "mkdir -p /tmp/lucia-upload && cat > \"/tmp/lucia-upload/$1\"",
                "sh",
                name,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| fail(format!("파일을 브라우저에 넘기지 못했다: {e}")))?;
        let mut stdin = child.stdin.take().expect("stdin piped");
        stdin
            .write_all(bytes)
            .await
            .map_err(|e| fail(format!("파일을 브라우저에 넘기지 못했다: {e}")))?;
        drop(stdin);
        let done = tokio::time::timeout(Duration::from_secs(60), child.wait()).await;
        if !matches!(done, Ok(Ok(status)) if status.success()) {
            return Err(fail("파일을 브라우저에 넘기지 못했다".into()));
        }
        let doc = live
            .call("DOM.getDocument", json!({"depth": 0}))
            .await
            .map_err(fail)?;
        let node = live
            .call(
                "DOM.querySelector",
                json!({
                    "nodeId": doc["root"]["nodeId"],
                    "selector": format!("[data-lucia-ref=\"{r}\"]"),
                }),
            )
            .await
            .map_err(fail)?;
        let node_id = node["nodeId"].as_i64().filter(|n| *n != 0).ok_or_else(|| {
            fail("그 파일 칸을 찾지 못했다(iframe 안의 칸은 아직 다루지 못한다)".into())
        })?;
        live.call(
            "DOM.setFileInputFiles",
            json!({"files": [format!("/tmp/lucia-upload/{name}")], "nodeId": node_id}),
        )
        .await
        .map_err(fail)?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.view_checked(live).await
    }

    async fn do_cookies(&self, url: &str) -> Result<crate::browser::SessionCookies, BrowserError> {
        let mut slot = self.live.lock().await;
        self.touch();
        let live = self.ensure(&mut slot).await?;
        let fail = BrowserError::Failed;
        let got = live
            .call("Network.getCookies", json!({"urls": [url]}))
            .await
            .map_err(fail)?;
        let cookies = got["cookies"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|c| {
                        Some((
                            c["name"].as_str()?.to_owned(),
                            c["value"].as_str()?.to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let version = live
            .cdp
            .call(None, "Browser.getVersion", json!({}), CALL)
            .await
            .map_err(fail)?;
        Ok(crate::browser::SessionCookies {
            cookies,
            user_agent: version["userAgent"].as_str().unwrap_or("").to_owned(),
        })
    }

    /// 감시 탭에서 주소를 열고 그려진 문서와 도착한 주소를 읽는다(결정 0036). 대화용 탭은 건드리지 않는다.
    /// 다른 조작이 쥐고 있으면 기다리지 않는다.
    async fn do_read_page(&self, url: &str) -> Result<(String, String), BrowserError> {
        let Ok(mut slot) = self.live.try_lock() else {
            return Err(BrowserError::Busy);
        };
        self.touch();
        let live = self.ensure(&mut slot).await?;
        let fail = BrowserError::Failed;
        let existing = live.watch_tab.lock().expect("watch tab lock").clone();
        let (_, session) = match existing {
            Some(t) => t,
            None => {
                live.making_tab.store(true, Ordering::SeqCst);
                let created = live
                    .cdp
                    .call(
                        None,
                        "Target.createTarget",
                        json!({"url": "about:blank", "background": true}),
                        CALL,
                    )
                    .await;
                let target = match created
                    .map_err(fail)?
                    .get("targetId")
                    .and_then(Value::as_str)
                {
                    Some(t) => t.to_owned(),
                    None => {
                        live.making_tab.store(false, Ordering::SeqCst);
                        return Err(fail("감시 탭을 만들지 못했다".into()));
                    }
                };
                *live.watch_tab.lock().expect("watch tab lock") =
                    Some((target.clone(), String::new()));
                live.making_tab.store(false, Ordering::SeqCst);
                let attached = live
                    .cdp
                    .call(
                        None,
                        "Target.attachToTarget",
                        json!({"targetId": target, "flatten": true}),
                        CALL,
                    )
                    .await
                    .map_err(fail)?;
                let session = attached["sessionId"]
                    .as_str()
                    .ok_or_else(|| fail("감시 탭에 붙지 못했다".into()))?
                    .to_owned();
                live.cdp
                    .call(Some(&session), "Page.enable", json!({}), CALL)
                    .await
                    .map_err(fail)?;
                *live.watch_tab.lock().expect("watch tab lock") =
                    Some((target.clone(), session.clone()));
                (target, session)
            }
        };
        let mut events = live.cdp.subscribe();
        let r = live
            .cdp
            .call(Some(&session), "Page.navigate", json!({"url": url}), CALL)
            .await
            .map_err(|e| {
                // 탭이 사라졌으면 다음에 새로 만든다.
                *live.watch_tab.lock().expect("watch tab lock") = None;
                fail(e)
            })?;
        if let Some(err) = r["errorText"].as_str().filter(|e| !e.is_empty()) {
            return Err(fail(format!("페이지를 열 수 없다: {err}")));
        }
        if r.get("loaderId").is_some() {
            let _ = wait_for(&mut events, LOAD, |e| {
                e.method == "Page.loadEventFired" && e.session.as_deref() == Some(session.as_str())
            })
            .await;
        }
        // 스크립트가 목록을 그릴 시간.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let got = live
            .cdp
            .call(
                Some(&session),
                "Runtime.evaluate",
                json!({
                    "expression": "JSON.stringify([location.href, document.documentElement.outerHTML])",
                    "returnByValue": true,
                }),
                CALL,
            )
            .await
            .map_err(fail)?;
        let pair: (String, String) = got["result"]["value"]
            .as_str()
            .and_then(|v| serde_json::from_str(v).ok())
            .ok_or_else(|| fail("문서를 읽지 못했다".into()))?;
        let (arrived, html) = pair;
        // 도착한 곳도 출구 검사(넘겨주기로 막힌 곳에 닿았으면 글을 쓰지 않는다).
        self.egress
            .check_url(&arrived)
            .map_err(BrowserError::NotAllowed)?;
        self.touch();
        Ok((html.chars().take(2_000_000).collect(), arrived))
    }

    async fn do_view(&self) -> Result<PageView, BrowserError> {
        let slot = self.live.lock().await;
        self.touch();
        let live = Self::current(&slot)?;
        self.view_checked(live).await
    }

    async fn do_screenshot(&self, full_page: bool) -> Result<Shot, BrowserError> {
        let slot = self.live.lock().await;
        self.touch();
        let live = Self::current(&slot)?;
        let fail = BrowserError::Failed;
        #[derive(Deserialize)]
        struct Where {
            url: String,
            title: String,
        }
        let at: Where = live
            .eval_json("JSON.stringify({url: location.href, title: document.title})")
            .await
            .map_err(fail)?;
        if at.url == "about:blank" {
            return Err(fail(
                "열린 페이지가 없다. browser_open으로 먼저 연다".into(),
            ));
        }
        self.arrival(live, &at.url).await?;
        let mut params = json!({"format": "jpeg", "quality": 70});
        let metrics = live
            .call("Page.getLayoutMetrics", json!({}))
            .await
            .map_err(fail)?;
        let viewport = (
            metrics["cssVisualViewport"]["clientWidth"]
                .as_f64()
                .unwrap_or(0.0)
                .round() as i32,
            metrics["cssVisualViewport"]["clientHeight"]
                .as_f64()
                .unwrap_or(0.0)
                .round() as i32,
        );
        if full_page {
            let size = &metrics["cssContentSize"];
            let width = size["width"].as_f64().unwrap_or(1366.0).min(1366.0);
            let height = size["height"]
                .as_f64()
                .unwrap_or(900.0)
                .min(FULL_PAGE_MAX_HEIGHT);
            params["clip"] = json!({"x": 0, "y": 0, "width": width, "height": height, "scale": 1});
            params["captureBeyondViewport"] = json!(true);
        }
        let shot = live
            .cdp
            .call(
                Some(&live.session),
                "Page.captureScreenshot",
                params,
                Duration::from_secs(30),
            )
            .await
            .map_err(fail)?;
        let data = shot["data"]
            .as_str()
            .ok_or_else(|| fail("빈 스크린샷".into()))?;
        let jpeg = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| fail(format!("스크린샷 해독: {e}")))?;
        Ok(Shot {
            url: at.url,
            title: at.title,
            jpeg,
            viewport,
        })
    }

    async fn do_inspect(&self, r: &str) -> Result<(String, Element), BrowserError> {
        let slot = self.live.lock().await;
        self.touch();
        let live = Self::current(&slot)?;
        live.inspect(r).await
    }

    async fn do_act(&self, act: &Act, expect: Option<&Expect>) -> Result<PageView, BrowserError> {
        let slot = self.live.lock().await;
        self.touch();
        let live = Self::current(&slot)?;
        let fail = BrowserError::Failed;
        // 좌표·초점 동작(결정 0023): 잠금 안에서 그 점(초점)을 다시 읽어 같은 대상인지 본다.
        if let Some(exp) = expect
            && matches!(
                act,
                Act::ClickAt { .. } | Act::TypeText { .. } | Act::Key { .. }
            )
        {
            let now = live.location().await.map_err(fail)?;
            if strip_fragment(&now) != strip_fragment(&exp.url) {
                return Err(BrowserError::Stale(format!(
                    "다른 페이지에 있다(지금 {now})"
                )));
            }
            let el = match act {
                Act::ClickAt { at } => live.hit(*at).await.map_err(fail)?.target(),
                _ => live.focus().await.map_err(fail)?.target(),
            };
            if !el.same_target(&exp.element) {
                return Err(BrowserError::Stale(format!(
                    "그 자리(초점)의 요소가 바뀌었다(지금: {} {:?}). 스크린샷으로 다시 본다",
                    el.kind, el.label
                )));
            }
            if exp.approved
                && el.form.as_ref().map(|f| &f.fields)
                    != exp.element.form.as_ref().map(|f| &f.fields)
            {
                return Err(BrowserError::Stale(
                    "폼에 든 값이 승인한 뒤 바뀌었다".into(),
                ));
            }
        }
        // 잠금 안에서 다시 확인: 같은 주소, 같은 요소(승인 카드에 보인 것).
        if let (Some(exp), Some(r)) = (expect, act.target()) {
            let now = live.location().await.map_err(fail)?;
            if strip_fragment(&now) != strip_fragment(&exp.url) {
                return Err(BrowserError::Stale(format!(
                    "다른 페이지에 있다(지금 {now})"
                )));
            }
            let (_, el) = live.inspect(r).await?;
            if !el.same_target(&exp.element) {
                return Err(BrowserError::Stale(format!(
                    "{r} 요소가 바뀌었다(지금: {} {:?})",
                    el.kind, el.label
                )));
            }
            if exp.approved
                && el.form.as_ref().map(|f| &f.fields)
                    != exp.element.form.as_ref().map(|f| &f.fields)
            {
                return Err(BrowserError::Stale(
                    "폼에 든 값이 승인한 뒤 바뀌었다".into(),
                ));
            }
        }
        live.dialogs.lock().expect("dialogs lock").clear();
        live.popups.lock().expect("popups lock").clear();
        let approved = expect.is_some_and(|e| e.approved);
        live.approved.store(approved, Ordering::SeqCst);
        let result = self.perform(live, act).await;
        live.approved.store(false, Ordering::SeqCst);
        result?;
        self.touch();
        self.view_checked(live).await
    }

    async fn perform(&self, live: &Live, act: &Act) -> Result<(), BrowserError> {
        let fail = BrowserError::Failed;
        let mut events = live.cdp.subscribe();
        match act {
            Act::Click { r } => {
                let p: PointJs = live.eval_json(&point_js(r)).await.map_err(fail)?;
                if let Some(nav) = p.nav {
                    self.egress
                        .check_url(&nav)
                        .map_err(BrowserError::NotAllowed)?;
                    return live.navigate(&nav).await.map_err(fail);
                }
                if !p.ok {
                    return Err(fail(p.why.unwrap_or_else(|| "누를 수 없다".into())));
                }
                live.click_at(p.x, p.y).await.map_err(fail)?;
                live.settle(&mut events).await;
            }
            Act::Type { r, text, submit } => {
                let (_, el) = live.inspect(r).await?;
                if el.kind == "select" {
                    let chosen: OkJs = live.eval_json(&select_js(r, text)).await.map_err(fail)?;
                    if !chosen.ok {
                        return Err(fail(chosen.why.unwrap_or_default()));
                    }
                } else {
                    let focused: OkJs = live.eval_json(&focus_clear_js(r)).await.map_err(fail)?;
                    if !focused.ok {
                        return Err(fail(focused.why.unwrap_or_default()));
                    }
                    if !text.is_empty() {
                        live.insert(text).await.map_err(fail)?;
                    }
                    if el.kind == "text" || el.kind == "textarea" {
                        let sel = format!("[data-lucia-ref=\"{r}\"]");
                        let len = live.value_len(&sel).await.unwrap_or(usize::MAX);
                        if len != usize::MAX && len != text.chars().count() {
                            return Err(fail(format!(
                                "칸에 {}자 중 {len}자만 들어갔다(글자 수 제한일 수 있다)",
                                text.chars().count()
                            )));
                        }
                    }
                }
                if *submit {
                    live.press_enter().await.map_err(fail)?;
                    live.settle(&mut events).await;
                }
            }
            Act::ClickAt { at } => {
                live.click_at(f64::from(at.x), f64::from(at.y))
                    .await
                    .map_err(fail)?;
                live.settle_acted(&mut events).await;
            }
            Act::TypeText { text, submit } => {
                if !text.is_empty() {
                    live.insert(text).await.map_err(fail)?;
                }
                if *submit {
                    live.press_enter().await.map_err(fail)?;
                    live.settle_acted(&mut events).await;
                } else {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }
            Act::Key { key } => {
                live.key(key).await.map_err(fail)?;
                if key == "enter" || key == "space" {
                    live.settle_acted(&mut events).await;
                } else {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }
            Act::Scroll { to, at: Some(at) } => {
                live.wheel(*at, *to).await.map_err(fail)?;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Act::Scroll { to, at: None } => {
                live.eval(&scroll_js(*to)).await.map_err(fail)?;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Act::Back => {
                let h = live
                    .call("Page.getNavigationHistory", json!({}))
                    .await
                    .map_err(fail)?;
                let current = h["currentIndex"].as_i64().unwrap_or(0);
                if current <= 0 {
                    return Err(fail("뒤로 갈 페이지가 없다".into()));
                }
                let entry = &h["entries"][usize::try_from(current - 1).unwrap_or(0)];
                let id = entry["id"]
                    .as_i64()
                    .ok_or_else(|| fail("기록이 없다".into()))?;
                if let Some(url) = entry["url"].as_str()
                    && url != "about:blank"
                {
                    self.egress
                        .check_url(url)
                        .map_err(BrowserError::NotAllowed)?;
                }
                live.call("Page.navigateToHistoryEntry", json!({"entryId": id}))
                    .await
                    .map_err(fail)?;
                live.forget_world();
                live.settle(&mut events).await;
            }
        }
        Ok(())
    }

    async fn do_login(
        &self,
        site: &SiteProfile,
        username: &str,
        password: &str,
    ) -> Result<LoginResult, BrowserError> {
        let mut slot = self.live.lock().await;
        self.touch();
        let live = self.ensure(&mut slot).await?;
        let fail = BrowserError::Failed;
        live.navigate(&site.login.url).await.map_err(fail)?;
        let state: StateJs = live
            .eval_json(&login_state_js(&site.login.password))
            .await
            .map_err(fail)?;
        let arrived = Url::parse(&state.url).map_err(|e| fail(format!("도착 주소: {e}")))?;
        if !site.allows(&arrived) {
            let _ = live.navigate("about:blank").await;
            return Err(BrowserError::NotAllowed(format!(
                "로그인 페이지가 다른 곳으로 넘어갔다: {}",
                arrived.host_str().unwrap_or("?")
            )));
        }
        let mut submit = site.login.submit.clone();
        // 단계형 로그인(아이디 → 다음 → 비밀번호, 결정 0017): 아이디를 이미 넣었으면 둘째 단계에서 넣지 않는다.
        let mut stepped = false;
        if site.login.auto {
            if !state.pw_any {
                if !site.is_login_path(&arrived) {
                    return Ok(LoginResult::AlreadyLoggedIn);
                }
                if let Err(e) = live.username_step(site, username).await {
                    live.clear_login(site).await;
                    return Err(e);
                }
                stepped = true;
            }
            let detect = if stepped {
                detect_password_step_js()
            } else {
                detect_login_js()
            };
            let found: DetectJs = live.eval_json(&detect).await.map_err(fail)?;
            if !found.ok {
                return Err(fail(
                    found
                        .why
                        .unwrap_or_else(|| "로그인 칸을 찾지 못했다".into()),
                ));
            }
            debug_assert!(found.pw);
            if !found.submit {
                submit = None;
            }
        } else if !site.is_login_path(&arrived) && !state.pw {
            return Ok(LoginResult::AlreadyLoggedIn);
        }

        // 칸마다: 다시 확인 → 넣기 → 글자 수 확인.
        let fill = async |selector: &str, text: &str, is_password: bool| -> Result<(), String> {
            live.login_focus(site, selector, is_password, true).await?;
            live.type_text(text).await?;
            let len = live.value_len(selector).await?;
            if len != text.chars().count() {
                return Err("입력한 글자 수가 맞지 않는다".into());
            }
            Ok(())
        };
        if !stepped && let Err(e) = fill(&site.login.username, username, false).await {
            live.clear_login(site).await;
            return Err(fail(format!("아이디 칸: {e}")));
        }
        if let Err(e) = fill(&site.login.password, password, true).await {
            live.clear_login(site).await;
            return Err(fail(format!("비밀번호 칸: {e}")));
        }
        if let Some(remember) = &site.login.remember {
            let _ = live
                .eval(&format!(
                    "(el => {{ if (!el) return false; if (!el.checked) el.click(); return el.checked; }})(document.querySelector({}))",
                    js_str(remember)
                ))
                .await;
        }

        // 제출 직전에 두 칸이 그대로인지 본다(사이트 스크립트가 지웠으면 제출하지 않고 까닭을 남긴다).
        for (sel, text, what) in [
            (&site.login.username, username, "아이디"),
            (&site.login.password, password, "비밀번호"),
        ] {
            if stepped && what == "아이디" {
                continue;
            }
            if live.value_len(sel).await.map_err(fail)? != text.chars().count() {
                live.clear_login(site).await;
                return Err(fail(format!(
                    "제출 직전에 {what} 칸이 바뀌었다(사이트 스크립트가 지웠을 수 있다)"
                )));
            }
        }
        live.dialogs.lock().expect("dialogs lock").clear();
        let mut events = live.cdp.subscribe();
        match &submit {
            Some(sel) => {
                let clicked = live
                    .eval(&format!(
                        "(el => {{ if (!el) return false; el.click(); return true; }})(document.querySelector({}))",
                        js_str(sel)
                    ))
                    .await
                    .map_err(fail)?;
                if clicked != Value::Bool(true) {
                    live.clear_login(site).await;
                    return Err(fail("로그인 버튼이 없다".into()));
                }
            }
            None => {
                // 버튼이 없으면 비밀번호 칸에서 Enter. 초점만 주고 칸은 비우지 않는다(넣은 비밀번호가 지워져
                // 사이트가 "비밀번호를 입력하세요"라고 했다, 2026-11-18 운영).
                live.login_focus(site, &site.login.password, true, false)
                    .await
                    .map_err(fail)?;
                live.press_enter().await.map_err(fail)?;
            }
        }
        // 다음 페이지가 뜨거나 대화상자가 열릴 때까지.
        let _ = wait_for(&mut events, SUBMIT, |e| {
            (e.method == "Page.loadEventFired" || e.method == "Page.javascriptDialogOpening")
                && live.is_mine(e)
        })
        .await;
        live.forget_world();
        tokio::time::sleep(Duration::from_millis(1000)).await;

        let state: StateJs = live
            .eval_json(&login_state_js(&site.login.password))
            .await
            .map_err(fail)?;
        let exposed = password.chars().count() >= 3
            && (state.url.contains(password) || state.body.contains(password));
        let after = Url::parse(&state.url).ok();
        let success = if site.login.auto {
            !state.pw_any
                && after
                    .as_ref()
                    .is_some_and(|u| self.egress.check_parsed(u).is_ok())
        } else {
            !state.pw
                && after
                    .as_ref()
                    .is_some_and(|u| site.allows(u) && !site.is_login_path(u))
        };
        if exposed {
            // 입력값이 페이지에 드러났다: 그 페이지를 닫는다.
            live.clear_login(site).await;
            let _ = live.navigate("about:blank").await;
            tracing::warn!(site = %site.id, "로그인 뒤 페이지에 입력값이 보여 페이지를 닫았다");
        }
        self.touch();
        // 비밀번호 뒤 인증 코드 칸이 나왔으면(같은 사이트, 칸이 하나로 정해질 때만) 그 단계에 둔다(결정 0017).
        // 일반 프로필은 비밀번호 칸이 없어지면 성공으로 보므로, 성공 판단보다 먼저 본다.
        if !exposed
            && !state.pw_any
            && after.as_ref().is_some_and(|u| site.allows(u))
            && let Ok(code) = live.eval_json::<CodeJs>(&detect_code_js()).await
            && code.ok
        {
            return Ok(LoginResult::NeedsCode {
                hint: scrub(&code.hint, password),
            });
        }
        // 사람 확인(캡차·추가 보안 확인 등): 비밀번호 칸이 없어도 성공이 아니다. 그 단계에 두고 사람에게 넘긴다.
        if !exposed && !state.pw_any && !state.challenge.is_empty() {
            return Ok(LoginResult::NeedsHuman {
                why: scrub(&state.challenge, password),
            });
        }
        if success && !exposed {
            return Ok(LoginResult::LoggedIn);
        }
        live.clear_login(site).await;
        let dialog = live.dialogs.lock().expect("dialogs lock").join(" / ");
        let reason = [dialog, state.err]
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" / ");
        let reason = if reason.is_empty() {
            "로그인 페이지에 그대로 있다(아이디나 비밀번호를 확인)".to_owned()
        } else {
            scrub(&reason, password)
        };
        Ok(LoginResult::Rejected(reason))
    }
}

impl SbxBrowser {
    /// 인증 코드 단계: 같은 사이트의 최상위 창에서, 코드 칸이 여전히 하나로 정해질 때만 넣고 제출한다.
    async fn do_submit_code(
        &self,
        site: &SiteProfile,
        code: &str,
    ) -> Result<LoginResult, BrowserError> {
        let slot = self.live.lock().await;
        self.touch();
        let live = Self::current(&slot)?;
        let fail = BrowserError::Failed;
        let url = live.nav_url().await.map_err(fail)?;
        let at = Url::parse(&url).map_err(|e| fail(format!("주소: {e}")))?;
        if !site.allows(&at) {
            return Err(BrowserError::NotAllowed(format!(
                "브라우저가 그 사이트에 있지 않다: {}",
                at.origin().ascii_serialization()
            )));
        }
        let found: CodeJs = live.eval_json(&detect_code_js()).await.map_err(fail)?;
        if !found.ok {
            return Err(fail(
                "코드 칸이 없다(시간이 지나 화면이 바뀌었을 수 있다)".into(),
            ));
        }
        let sel = code_selector();
        live.login_focus(site, &sel, false, true)
            .await
            .map_err(fail)?;
        live.type_text(code).await.map_err(fail)?;
        if live.value_len(&sel).await.map_err(fail)? != code.chars().count() {
            live.clear_code().await;
            return Err(fail("코드 칸: 입력한 글자 수가 맞지 않는다".into()));
        }
        live.dialogs.lock().expect("dialogs lock").clear();
        let mut events = live.cdp.subscribe();
        if found.submit {
            let _ = live
                .eval(&format!(
                    "(el => {{ if (el) el.click(); }})(document.querySelector({}))",
                    js_str(&format!("[{AUTO_ATTR}=\"code-submit\"]"))
                ))
                .await;
        } else {
            live.press_enter().await.map_err(fail)?;
        }
        let _ = wait_for(&mut events, SUBMIT, |e| {
            (e.method == "Page.loadEventFired" || e.method == "Page.javascriptDialogOpening")
                && live.is_mine(e)
        })
        .await;
        live.forget_world();
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let state: StateJs = live
            .eval_json(&login_state_js(&site.login.password))
            .await
            .map_err(fail)?;
        let still: CodeJs = live.eval_json(&detect_code_js()).await.unwrap_or(CodeJs {
            ok: false,
            hint: String::new(),
            submit: false,
        });
        let after = Url::parse(&state.url).ok();
        let left_login = after
            .as_ref()
            .is_some_and(|u| self.egress.check_parsed(u).is_ok() && !site.is_login_path(u));
        self.touch();
        if !still.ok && !state.pw_any && !state.challenge.is_empty() {
            return Ok(LoginResult::NeedsHuman {
                why: state.challenge,
            });
        }
        if !still.ok && !state.pw_any && left_login {
            return Ok(LoginResult::LoggedIn);
        }
        live.clear_code().await;
        let dialog = live.dialogs.lock().expect("dialogs lock").join(" / ");
        let reason = [dialog, state.err]
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" / ");
        Ok(LoginResult::Rejected(if reason.is_empty() {
            "코드를 넣었지만 로그인 화면을 벗어나지 못했다".to_owned()
        } else {
            scrub(&reason, code)
        }))
    }
}

impl SbxBrowser {
    /// 탭의 CDP 세션: 원래 페이지면 그 세션, 아니면 붙어서(한 번) 얻는다.
    async fn tab_session(&self, live: &Live, tab: &str) -> Result<String, BrowserError> {
        if tab == live.target {
            return Ok(live.session.clone());
        }
        if let Some(s) = live.tab_sessions.lock().expect("tabs lock").get(tab) {
            return Ok(s.clone());
        }
        let r = live
            .cdp
            .call(
                None,
                "Target.attachToTarget",
                json!({"targetId": tab, "flatten": true}),
                CALL,
            )
            .await
            .map_err(BrowserError::Failed)?;
        let session = r["sessionId"]
            .as_str()
            .ok_or_else(|| BrowserError::Failed("탭에 붙지 못했다".into()))?
            .to_owned();
        live.tab_sessions
            .lock()
            .expect("tabs lock")
            .insert(tab.to_owned(), session.clone());
        Ok(session)
    }

    async fn do_set_handover(&self, on: bool) -> Result<(), BrowserError> {
        let mut slot = self.live.lock().await;
        self.touch();
        let live = self.ensure(&mut slot).await?;
        live.handover.store(on, Ordering::SeqCst);
        if !on {
            // 넘겨받는 동안 열린 탭을 닫는다.
            let targets = live
                .cdp
                .call(None, "Target.getTargets", json!({}), CALL)
                .await
                .unwrap_or_default();
            for t in targets["targetInfos"].as_array().into_iter().flatten() {
                let id = t["targetId"].as_str().unwrap_or("");
                if t["type"] == "page" && id != live.target {
                    let _ = live
                        .cdp
                        .call(None, "Target.closeTarget", json!({"targetId": id}), CALL)
                        .await;
                }
            }
            live.tab_sessions.lock().expect("tabs lock").clear();
            live.forget_world();
        }
        Ok(())
    }

    async fn do_handover_tabs(&self) -> Result<Vec<super::Tab>, BrowserError> {
        let slot = self.live.lock().await;
        self.touch();
        let live = Self::current(&slot)?;
        let watch_tab = live
            .watch_tab
            .lock()
            .expect("watch tab lock")
            .as_ref()
            .map(|(t, _)| t.clone());
        let targets = live
            .cdp
            .call(None, "Target.getTargets", json!({}), CALL)
            .await
            .map_err(BrowserError::Failed)?;
        let mut tabs: Vec<super::Tab> = targets["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["type"] == "page")
            // 감시 탭(결정 0036)은 사람에게 넘기지 않는다.
            .filter(|t| t["targetId"].as_str() != watch_tab.as_deref())
            .map(|t| super::Tab {
                id: t["targetId"].as_str().unwrap_or("").to_owned(),
                url: t["url"].as_str().unwrap_or("").to_owned(),
                title: t["title"].as_str().unwrap_or("").chars().take(80).collect(),
            })
            .collect();
        // 원래 페이지를 맨 앞에.
        tabs.sort_by_key(|t| t.id != live.target);
        Ok(tabs)
    }

    async fn do_handover_frame(&self, tab: &str) -> Result<super::Frame, BrowserError> {
        let slot = self.live.lock().await;
        self.touch();
        let live = Self::current(&slot)?;
        let session = self.tab_session(live, tab).await?;
        let fail = BrowserError::Failed;
        let metrics = live
            .cdp
            .call(Some(&session), "Page.getLayoutMetrics", json!({}), CALL)
            .await
            .map_err(fail)?;
        let vp = &metrics["cssLayoutViewport"];
        let width = vp["clientWidth"].as_f64().unwrap_or(1280.0) as u32;
        let height = vp["clientHeight"].as_f64().unwrap_or(800.0) as u32;
        let shot = live
            .cdp
            .call(
                Some(&session),
                "Page.captureScreenshot",
                json!({"format": "jpeg", "quality": 60,
                       "clip": {"x": 0, "y": 0, "width": width, "height": height, "scale": 1}}),
                CALL,
            )
            .await
            .map_err(fail)?;
        use base64::Engine as _;
        let jpeg = base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap_or(""))
            .map_err(|e| fail(format!("화면: {e}")))?;
        Ok(super::Frame { jpeg })
    }

    async fn do_handover_input(
        &self,
        tab: &str,
        input: &super::HandInput,
    ) -> Result<(), BrowserError> {
        use super::HandInput;
        let slot = self.live.lock().await;
        self.touch();
        let live = Self::current(&slot)?;
        if !live.handover.load(Ordering::SeqCst) {
            return Err(BrowserError::Failed("넘겨받은 상태가 아니다".into()));
        }
        let session = self.tab_session(live, tab).await?;
        let call = async |method: &str, params: Value| -> Result<(), BrowserError> {
            live.cdp
                .call(Some(&session), method, params, CALL)
                .await
                .map(|_| ())
                .map_err(BrowserError::Failed)
        };
        match input {
            HandInput::Click { x, y } => {
                call(
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseMoved", "x": x, "y": y}),
                )
                .await?;
                for kind in ["mousePressed", "mouseReleased"] {
                    call(
                        "Input.dispatchMouseEvent",
                        json!({"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1}),
                    )
                    .await?;
                }
            }
            HandInput::Text(text) => {
                call("Input.insertText", json!({"text": text})).await?;
            }
            HandInput::Key(name) => {
                let (key, code, vk, text) = match name.as_str() {
                    "enter" => ("Enter", "Enter", 13, Some("\r")),
                    "tab" => ("Tab", "Tab", 9, None),
                    "backspace" => ("Backspace", "Backspace", 8, None),
                    "escape" => ("Escape", "Escape", 27, None),
                    "up" => ("ArrowUp", "ArrowUp", 38, None),
                    "down" => ("ArrowDown", "ArrowDown", 40, None),
                    other => return Err(BrowserError::Failed(format!("모르는 키: {other}"))),
                };
                for kind in ["keyDown", "keyUp"] {
                    let mut p = json!({"type": kind, "key": key, "code": code,
                        "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk});
                    if kind == "keyDown"
                        && let Some(t) = text
                    {
                        p["text"] = json!(t);
                    }
                    call("Input.dispatchKeyEvent", p).await?;
                }
            }
            HandInput::Scroll(dy) => {
                call(
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseWheel", "x": 200, "y": 200, "deltaX": 0, "deltaY": dy}),
                )
                .await?;
            }
        }
        // 화면이 바뀌었을 수 있다(데몬 스크립트 공간은 다시 만든다).
        live.forget_world();
        Ok(())
    }
}

fn strip_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

impl SbxBrowser {
    async fn do_hit(&self, at: super::Point) -> Result<Hit, BrowserError> {
        let slot = self.live.lock().await;
        self.touch();
        let live = Self::current(&slot)?;
        live.hit(at).await.map_err(BrowserError::Failed)
    }

    async fn do_focused(&self) -> Result<(String, Focus), BrowserError> {
        let slot = self.live.lock().await;
        self.touch();
        let live = Self::current(&slot)?;
        let url = live.location().await.map_err(BrowserError::Failed)?;
        let focus = live.focus().await.map_err(BrowserError::Failed)?;
        Ok((url, focus))
    }
}

impl Browser for SbxBrowser {
    fn hit(&self, at: super::Point) -> BoxFut<'_, Result<Hit, BrowserError>> {
        Box::pin(self.do_hit(at))
    }

    fn focused(&self) -> BoxFut<'_, Result<(String, Focus), BrowserError>> {
        Box::pin(self.do_focused())
    }

    fn open<'a>(
        &'a self,
        url: &'a str,
        site: Option<&'a SiteProfile>,
    ) -> BoxFut<'a, Result<PageView, BrowserError>> {
        Box::pin(self.do_open(url, site))
    }

    fn view(&self) -> BoxFut<'_, Result<PageView, BrowserError>> {
        Box::pin(self.do_view())
    }

    fn screenshot(&self, full_page: bool) -> BoxFut<'_, Result<Shot, BrowserError>> {
        Box::pin(self.do_screenshot(full_page))
    }

    fn inspect<'a>(&'a self, r: &'a str) -> BoxFut<'a, Result<(String, Element), BrowserError>> {
        Box::pin(self.do_inspect(r))
    }

    fn act<'a>(
        &'a self,
        act: &'a Act,
        expect: Option<&'a Expect>,
    ) -> BoxFut<'a, Result<PageView, BrowserError>> {
        Box::pin(self.do_act(act, expect))
    }

    fn login<'a>(
        &'a self,
        site: &'a SiteProfile,
        username: &'a str,
        password: &'a str,
    ) -> BoxFut<'a, Result<LoginResult, BrowserError>> {
        Box::pin(self.do_login(site, username, password))
    }

    fn submit_code<'a>(
        &'a self,
        site: &'a SiteProfile,
        code: &'a str,
    ) -> BoxFut<'a, Result<LoginResult, BrowserError>> {
        Box::pin(self.do_submit_code(site, code))
    }

    fn set_handover<'a>(&'a self, on: bool) -> BoxFut<'a, Result<(), BrowserError>> {
        Box::pin(self.do_set_handover(on))
    }

    fn handover_tabs(&self) -> BoxFut<'_, Result<Vec<super::Tab>, BrowserError>> {
        Box::pin(self.do_handover_tabs())
    }

    fn handover_frame<'a>(
        &'a self,
        tab: &'a str,
    ) -> BoxFut<'a, Result<super::Frame, BrowserError>> {
        Box::pin(self.do_handover_frame(tab))
    }

    fn handover_input<'a>(
        &'a self,
        tab: &'a str,
        input: &'a super::HandInput,
    ) -> BoxFut<'a, Result<(), BrowserError>> {
        Box::pin(self.do_handover_input(tab, input))
    }

    fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn cookies<'a>(
        &'a self,
        url: &'a str,
    ) -> BoxFut<'a, Result<crate::browser::SessionCookies, BrowserError>> {
        Box::pin(self.do_cookies(url))
    }

    fn read_page<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<(String, String), BrowserError>> {
        Box::pin(self.do_read_page(url))
    }

    fn set_keep_alive(&self, on: bool) {
        self.keep.store(on, Ordering::SeqCst);
    }

    fn take_downloads(&self) -> Vec<SeenDownload> {
        std::mem::take(&mut *self.downloads.lock().expect("downloads lock"))
    }

    fn eval<'a>(&'a self, code: &'a str) -> BoxFut<'a, Result<(String, PageView), BrowserError>> {
        Box::pin(self.do_eval(code))
    }

    fn upload<'a>(
        &'a self,
        r: &'a str,
        name: &'a str,
        bytes: &'a [u8],
    ) -> BoxFut<'a, Result<PageView, BrowserError>> {
        Box::pin(self.do_upload(r, name, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_quote_inputs_and_scrub_passwords() {
        let js = login_state_js("#pw\"); alert(1); (\"");
        assert!(js.contains(r##"document.querySelector("#pw\"); alert(1); (\"")"##));
        assert!(select_js("e1", "a'); x('").contains(r#"one("a'); x('")"#));
        assert_eq!(
            scrub("비밀번호 hunter22 틀림", "hunter22"),
            format!("비밀번호 {} 틀림", crate::mask::MASK)
        );
        assert_eq!(
            scrub("a b", "ab"),
            "a b",
            "너무 짧은 값은 가리지 않는다(오탐)"
        );
        let start = start_script("socks5://host.docker.internal:47811");
        assert!(start.contains("exec socat STDIO TCP:127.0.0.1:9222"));
        assert!(start.contains(r#"--proxy-server="$PROXY""#));
        assert!(start.contains("PROXY='socks5://host.docker.internal:47811'"));
        assert!(
            !start.contains("--remote-debugging-address"),
            "CDP는 VM 루프백에만"
        );
    }

    #[test]
    fn refs_are_validated_before_use_in_selectors() {
        assert!(valid_ref("e1") && valid_ref("e199"));
        for bad in ["", "e", "1", "e1\"]", "e-1", "x12", "e123456789"] {
            assert!(!valid_ref(bad), "{bad}");
        }
        assert!(inspect_js("e12").contains(r#"byRef("e12")"#));
    }

    #[test]
    fn element_json_from_the_page_parses() {
        let raw = r#"{"ref":"e3","kind":"submit","tag":"BUTTON","label":"등록","disabled":false,"in_view":true,
            "popup":false,"options":[],"search":false,
            "form":{"method":"post","action":"https://x.example/write","fields":[["제목","안녕"]],"hidden":2,"password":false}}"#;
        let el: Element = serde_json::from_str(raw).unwrap();
        assert_eq!(el.r, "e3");
        assert_eq!(el.form.as_ref().unwrap().fields[0].1, "안녕");
        let mut other = el.clone();
        other.form.as_mut().unwrap().fields[0].1 = "바뀐 값".into();
        assert!(el.same_target(&other), "값은 대상 비교에 넣지 않는다");
        other.form.as_mut().unwrap().action = "https://x.example/delete".into();
        assert!(!el.same_target(&other));
    }
}
