//! 스크립트 출구 프록시(결정 0058): 사람이 대화 중인 실행의 코드가 바깥 인터넷에 닿는 길.
//!
//! - 데몬이 여는 HTTP 프록시다(`CONNECT`와 평문 http 요청). 스크립트 샌드박스는 이 포트 하나만 허용한다
//!   (`deploy/create-script-sandbox.sh`, 데몬이 뜰 때도 그 규칙을 확인한다). sbx 정책으로 바깥을 직접 열지 않는다:
//!   sbx는 이름으로 풀린 사설 IP를 막지 못한다(PoC P8b).
//! - **통행증**: 요청마다 `Proxy-Authorization`의 통행증을 본다. 통행증은 실행 하나에만 내주고(사람이 대화 중인
//!   `code_run`), 그 실행이 끝나면 거두며 **그 통행증으로 열린 연결도 함께 끊는다**. 통행증이 없는 스크립트(루틴,
//!   사람 없는 실행)는 포트에 닿아도 407만 받는다.
//! - **검사**: 브라우저 출구와 같다([`Proxy::connect`]): 80·443만, 차단 목록, 이름을 한 번 풀어 사설·내부 주소를
//!   거르고 그 IPv4 주소로만 연결.
//! - **기록**: 나간 호스트와 막은 것을 통행증에 모아 실행 결과와 활동 기록에 남긴다. TLS 안은 보지 못한다
//!   (나가는 글 검사는 여기에 걸리지 않는다. 그래서 그 검사가 "막기"이거나 권한 모드가 엄격이면 통행증을 내주지 않는다).
//! - 루프백에만 묶는다. 상주 샌드박스와 브라우저 샌드박스는 이 포트를 허용하지 않는다.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::browser::proxy::Proxy;

const HANDSHAKE: Duration = Duration::from_secs(10);
/// 요청 머리(첫 줄과 헤더) 상한.
const HEAD_MAX: usize = 16 * 1024;
const MAX_CONNECTIONS: usize = 128;
/// 한 연결에서 통행증 없이 다시 묻게 해 주는 횟수(git 같은 클라이언트는 407을 받은 뒤 같은 연결로 다시 보낸다).
const RETRIES: usize = 3;
/// 실행 하나에 적어 두는 호스트 수.
const HOSTS_MAX: usize = 40;

struct PassState {
    cancel: CancellationToken,
    /// 나간 호스트(처음 나간 순서).
    hosts: Mutex<Vec<String>>,
    /// 막은 것("호스트: 까닭").
    blocked: Mutex<Vec<String>>,
}

/// 실행 하나의 통행증. 버리면 거두고, 이 통행증으로 열린 연결을 끊는다.
pub struct Pass {
    net: Arc<ScriptNet>,
    token: String,
    state: Arc<PassState>,
}

impl Pass {
    /// 스크립트에 줄 프록시 주소(`http://lucia:<통행증>@<데몬>:<포트>`).
    pub fn proxy_url(&self, daemon: &str) -> String {
        format!("http://lucia:{}@{daemon}", self.token)
    }

    pub fn hosts(&self) -> Vec<String> {
        self.state.hosts.lock().expect("pass hosts").clone()
    }

    pub fn blocked(&self) -> Vec<String> {
        self.state.blocked.lock().expect("pass blocked").clone()
    }
}

impl Drop for Pass {
    fn drop(&mut self) {
        self.net
            .passes
            .lock()
            .expect("script net passes")
            .remove(&self.token);
        self.state.cancel.cancel();
    }
}

pub struct ScriptNet {
    proxy: Arc<Proxy>,
    passes: Mutex<HashMap<String, Arc<PassState>>>,
    slots: Arc<Semaphore>,
}

/// 요청 머리에서 읽은 것.
struct Head {
    method: String,
    target: String,
    /// 헤더 줄(첫 줄 빼고, 빈 줄 전까지).
    headers: Vec<String>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find_map(|h| {
            let (k, v) = h.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }

    /// `Proxy-Authorization: Basic base64(이름:통행증)`의 통행증.
    fn token(&self) -> Option<String> {
        let value = self.header("proxy-authorization")?;
        let encoded = value
            .strip_prefix("Basic ")
            .or_else(|| value.strip_prefix("basic "))?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .ok()?;
        let text = String::from_utf8(decoded).ok()?;
        text.split_once(':').map(|(_, token)| token.to_owned())
    }
}

/// `호스트:포트`(IPv6 대괄호 포함)를 나눈다.
fn host_port(target: &str, default_port: Option<u16>) -> Option<(String, u16)> {
    let (host, port) = match target.rsplit_once(':') {
        Some((h, p)) if !p.contains(']') => (h, Some(p.parse::<u16>().ok()?)),
        _ => (target, None),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = port.or(default_port)?;
    (!host.is_empty()).then(|| (host.trim_end_matches('.').to_ascii_lowercase(), port))
}

async fn read_head(client: &mut TcpStream) -> std::io::Result<Option<Head>> {
    let mut buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    // 머리 뒤의 바이트(TLS 인사, 요청 본문)를 먼저 읽어 버리지 않게 한 바이트씩 읽는다(머리는 짧다).
    while !buf.ends_with(b"\r\n\r\n") {
        if buf.len() >= HEAD_MAX {
            return Err(std::io::Error::other("요청 머리가 너무 길다"));
        }
        if client.read(&mut byte).await? == 0 {
            return Ok(None);
        }
        buf.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (Some(method), Some(target)) = (first.next(), first.next()) else {
        return Err(std::io::Error::other("HTTP 요청이 아니다"));
    };
    Ok(Some(Head {
        method: method.to_ascii_uppercase(),
        target: target.to_owned(),
        headers: lines.filter(|l| !l.is_empty()).map(str::to_owned).collect(),
    }))
}

async fn respond(
    client: &mut TcpStream,
    status: &str,
    extra: &str,
    body: &str,
) -> std::io::Result<()> {
    client
        .write_all(
            format!(
                "HTTP/1.1 {status}\r\n{extra}Content-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
}

impl ScriptNet {
    pub fn new(proxy: Arc<Proxy>) -> Arc<Self> {
        Arc::new(Self {
            proxy,
            passes: Mutex::default(),
            slots: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        })
    }

    /// 실행 하나에 통행증을 내준다.
    pub fn issue(self: &Arc<Self>) -> Pass {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let state = Arc::new(PassState {
            cancel: CancellationToken::new(),
            hosts: Mutex::default(),
            blocked: Mutex::default(),
        });
        self.passes
            .lock()
            .expect("script net passes")
            .insert(token.clone(), state.clone());
        Pass {
            net: self.clone(),
            token,
            state,
        }
    }

    fn pass(&self, head: &Head) -> Option<Arc<PassState>> {
        let token = head.token()?;
        self.passes
            .lock()
            .expect("script net passes")
            .get(&token)
            .cloned()
    }

    async fn handle(&self, mut client: TcpStream) -> std::io::Result<()> {
        for _ in 0..=RETRIES {
            let head = match tokio::time::timeout(HANDSHAKE, read_head(&mut client)).await {
                Ok(Ok(Some(h))) => h,
                Ok(Ok(None)) | Err(_) => return Ok(()),
                Ok(Err(e)) => {
                    let _ = respond(&mut client, "400 Bad Request", "", "").await;
                    return Err(e);
                }
            };
            let Some(pass) = self.pass(&head) else {
                respond(
                    &mut client,
                    "407 Proxy Authentication Required",
                    "Proxy-Authenticate: Basic realm=\"lucia\"\r\n",
                    "이 실행은 바깥 인터넷에 나갈 수 없다(통행증이 없거나 실행이 끝났다).\n",
                )
                .await?;
                // 본문이 딸린 요청이면 이어 읽을 수 없다.
                if head.method != "CONNECT"
                    && head.header("content-length").is_some_and(|n| n != "0")
                    || head.header("transfer-encoding").is_some()
                {
                    return Ok(());
                }
                continue;
            };
            return self.forward(client, head, &pass).await;
        }
        Ok(())
    }

    async fn forward(
        &self,
        mut client: TcpStream,
        head: Head,
        pass: &PassState,
    ) -> std::io::Result<()> {
        // 목적지: CONNECT는 `호스트:포트`, 평문 요청은 `http://호스트[:포트]/경로`.
        let connect = head.method == "CONNECT";
        let (target, origin_form) = if connect {
            (host_port(&head.target, None), String::new())
        } else {
            match url::Url::parse(&head.target) {
                Ok(u) if u.scheme() == "http" => {
                    let path = match u.query() {
                        Some(q) => format!("{}?{q}", u.path()),
                        None => u.path().to_owned(),
                    };
                    (
                        u.host_str().map(|h| {
                            (
                                h.trim_start_matches('[')
                                    .trim_end_matches(']')
                                    .to_ascii_lowercase(),
                                u.port().unwrap_or(80),
                            )
                        }),
                        path,
                    )
                }
                _ => (None, String::new()),
            }
        };
        let Some((host, port)) = target else {
            return respond(
                &mut client,
                "400 Bad Request",
                "",
                "프록시 요청의 목적지를 읽지 못했다(CONNECT 호스트:포트 또는 http:// 주소).\n",
            )
            .await;
        };
        let mut upstream = match self.proxy.connect(&host, port).await {
            Ok(s) => s,
            Err(why) => {
                tracing::info!(host, port, why, "스크립트 연결을 막았다");
                {
                    let mut blocked = pass.blocked.lock().expect("pass blocked");
                    if blocked.len() < HOSTS_MAX {
                        blocked.push(format!("{host}: {why}"));
                    }
                }
                return respond(
                    &mut client,
                    "403 Forbidden",
                    "",
                    &format!("데몬 프록시가 막았다: {why}\n"),
                )
                .await;
            }
        };
        {
            let mut hosts = pass.hosts.lock().expect("pass hosts");
            if hosts.len() < HOSTS_MAX && !hosts.contains(&host) {
                hosts.push(host.clone());
            }
        }
        tracing::debug!(host, port, "스크립트 연결");
        if connect {
            client
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await?;
        } else {
            // 원 서버에는 경로만 보내고, 프록시용 헤더는 뺀다. 연결은 요청 하나로 끝낸다(다른 호스트로 이어 쓰지 않게).
            let mut request = format!("{} {} HTTP/1.1\r\n", head.method, origin_form);
            for h in &head.headers {
                let name = h
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase();
                if matches!(
                    name.as_str(),
                    "proxy-authorization" | "proxy-connection" | "connection"
                ) {
                    continue;
                }
                request.push_str(h);
                request.push_str("\r\n");
            }
            request.push_str("Connection: close\r\n\r\n");
            upstream.write_all(request.as_bytes()).await?;
        }
        // 통행증을 거두면(실행이 끝나면) 열려 있던 연결도 끊는다.
        tokio::select! {
            _ = pass.cancel.cancelled() => {}
            _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {}
        }
        Ok(())
    }

    /// 프록시를 띄운다. 루프백 주소만 받는다(설정 검사가 먼저 막는다).
    pub async fn serve(
        self: Arc<Self>,
        bind: SocketAddr,
        cancel: CancellationToken,
    ) -> anyhow::Result<SocketAddr> {
        anyhow::ensure!(
            bind.ip().is_loopback(),
            "스크립트 프록시는 루프백에만 묶는다"
        );
        let listener = TcpListener::bind(bind).await?;
        let local = listener.local_addr()?;
        tracing::info!(%local, "스크립트 출구 프록시");
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = cancel.cancelled() => return,
                    a = listener.accept() => a,
                };
                let (stream, peer) = match accepted {
                    Ok(a) => a,
                    Err(e) => {
                        tracing::warn!(err = %e, "스크립트 프록시 accept");
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let Ok(permit) = self.slots.clone().try_acquire_owned() else {
                    tracing::warn!("스크립트 프록시 연결이 너무 많다");
                    continue;
                };
                let me = self.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(e) = me.handle(stream).await {
                        tracing::debug!(err = %e, "스크립트 프록시 연결");
                    }
                });
            }
        });
        Ok(local)
    }
}

#[cfg(test)]
pub mod tests {
    use std::net::{IpAddr, SocketAddrV4};

    use super::*;
    use crate::browser::BoxFut;
    use crate::browser::egress::Egress;
    use crate::browser::proxy::{Dial, Resolve};

    /// 이름을 정해 둔 주소로 푼다(없는 이름은 공인 주소 하나로).
    pub struct Names(pub Vec<(&'static str, &'static str)>);

    impl Resolve for Names {
        fn lookup<'a>(&'a self, host: &'a str) -> BoxFut<'a, std::io::Result<Vec<IpAddr>>> {
            let ip = self
                .0
                .iter()
                .find(|(h, _)| *h == host)
                .map_or("93.184.216.34", |(_, ip)| ip);
            let ip: IpAddr = ip.parse().unwrap();
            Box::pin(async move { Ok(vec![ip]) })
        }
    }

    /// 어디로 연결하려 했는지 적고, 실제로는 시험용 서버로 잇는다.
    pub struct ToServer {
        pub target: SocketAddr,
        pub dialed: Mutex<Vec<SocketAddrV4>>,
    }

    impl Dial for ToServer {
        fn dial(&self, addr: SocketAddrV4) -> BoxFut<'_, std::io::Result<TcpStream>> {
            self.dialed.lock().unwrap().push(addr);
            Box::pin(TcpStream::connect(self.target))
        }
    }

    /// 받은 것을 그대로 적어 두고, 정해 둔 답을 보낸 뒤 연결을 열어 둔다.
    async fn origin(seen: Arc<Mutex<Vec<u8>>>) -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    while let Ok(n) = s.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        seen.lock().unwrap().extend_from_slice(&buf[..n]);
                        let _ = s
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                            .await;
                    }
                });
            }
        });
        addr
    }

    pub struct Setup {
        pub net: Arc<ScriptNet>,
        pub addr: SocketAddr,
        pub seen: Arc<Mutex<Vec<u8>>>,
        pub dial: Arc<ToServer>,
    }

    pub async fn setup() -> Setup {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let dial = Arc::new(ToServer {
            target: origin(seen.clone()).await,
            dialed: Mutex::default(),
        });
        let proxy = Proxy::new(
            Arc::new(Egress::new(vec!["blocked.example".into()])),
            Arc::new(Names(vec![("nas.example", "192.168.0.7")])),
            dial.clone(),
        );
        let net = ScriptNet::new(proxy);
        let addr = net
            .clone()
            .serve("127.0.0.1:0".parse().unwrap(), CancellationToken::new())
            .await
            .unwrap();
        Setup {
            net,
            addr,
            seen,
            dial,
        }
    }

    fn auth(pass: &Pass) -> String {
        format!(
            "Proxy-Authorization: Basic {}\r\n",
            base64::engine::general_purpose::STANDARD.encode(format!("lucia:{}", pass.token))
        )
    }

    /// 요청을 보내고 답의 머리(빈 줄까지)를 읽는다.
    async fn ask(s: &mut TcpStream, request: &str) -> String {
        s.write_all(request.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let mut b = [0u8; 1];
        while !out.ends_with(b"\r\n\r\n") {
            match tokio::time::timeout(Duration::from_secs(5), s.read(&mut b)).await {
                Ok(Ok(1)) => out.push(b[0]),
                _ => break,
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn only_a_live_pass_gets_out_and_the_same_checks_apply() {
        let s = setup().await;
        let pass = s.net.issue();
        assert!(
            pass.proxy_url("host.docker.internal:47812")
                .starts_with("http://lucia:")
        );
        // 통행증이 없거나 틀리면 407. 같은 연결로 통행증을 붙여 다시 보내면 지나간다(git이 이렇게 한다).
        let mut c = TcpStream::connect(s.addr).await.unwrap();
        let got = ask(
            &mut c,
            "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
        )
        .await;
        assert!(
            got.starts_with("HTTP/1.1 407") && got.contains("Proxy-Authenticate: Basic"),
            "{got}"
        );
        let mut body = [0u8; 256];
        let _ = c.read(&mut body).await;
        let got = ask(
            &mut c,
            &format!(
                "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n{}\r\n",
                auth(&pass)
            ),
        )
        .await;
        assert!(got.starts_with("HTTP/1.1 200"), "{got}");
        // 터널 안의 바이트는 그대로 오간다.
        let got = ask(&mut c, "GET /inside HTTP/1.1\r\n\r\n").await;
        assert!(got.starts_with("HTTP/1.1 200 OK"), "{got}");
        assert!(String::from_utf8_lossy(&s.seen.lock().unwrap()).contains("GET /inside"));
        assert_eq!(
            s.dial.dialed.lock().unwrap()[0].to_string(),
            "93.184.216.34:443"
        );
        let mut wrong = TcpStream::connect(s.addr).await.unwrap();
        let got = ask(
            &mut wrong,
            "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic bHVjaWE6d3Jvbmc=\r\n\r\n",
        )
        .await;
        assert!(got.starts_with("HTTP/1.1 407"), "{got}");
        // 같은 검사: 차단 목록, 사설 주소로 풀리는 이름, 사설 주소, 다른 포트, 내부 이름.
        for target in [
            "blocked.example:443",
            "nas.example:443",
            "192.168.0.1:443",
            "127.0.0.1:8780",
            "example.com:22",
            "localhost:443",
            "api.anthropic.com:443",
        ] {
            let mut c = TcpStream::connect(s.addr).await.unwrap();
            let got = ask(
                &mut c,
                &format!("CONNECT {target} HTTP/1.1\r\n{}\r\n", auth(&pass)),
            )
            .await;
            assert!(got.starts_with("HTTP/1.1 403"), "{target}: {got}");
        }
        assert_eq!(
            s.dial.dialed.lock().unwrap().len(),
            1,
            "막은 곳으로는 연결하지 않았다"
        );
        assert_eq!(pass.hosts(), vec!["example.com".to_owned()]);
        let blocked = pass.blocked();
        assert_eq!(blocked.len(), 7);
        assert!(
            blocked[1].starts_with("nas.example: ") && blocked[1].contains("사설"),
            "{blocked:?}"
        );
    }

    #[tokio::test]
    async fn plain_http_is_forwarded_without_the_pass_and_one_request_per_connection() {
        let s = setup().await;
        let pass = s.net.issue();
        let mut c = TcpStream::connect(s.addr).await.unwrap();
        let got = ask(
            &mut c,
            &format!(
                "GET http://deb.example.org/debian/dists/InRelease?x=1 HTTP/1.1\r\nHost: deb.example.org\r\nUser-Agent: apt\r\nProxy-Connection: keep-alive\r\n{}\r\n",
                auth(&pass)
            ),
        )
        .await;
        assert!(got.starts_with("HTTP/1.1 200 OK"), "{got}");
        let seen = String::from_utf8_lossy(&s.seen.lock().unwrap()).into_owned();
        assert!(
            seen.starts_with("GET /debian/dists/InRelease?x=1 HTTP/1.1\r\n"),
            "{seen}"
        );
        assert!(seen.contains("Host: deb.example.org") && seen.contains("Connection: close"));
        assert!(
            !seen.to_ascii_lowercase().contains("proxy-") && !seen.contains(&pass.token),
            "통행증은 원 서버로 가지 않는다: {seen}"
        );
        assert_eq!(s.dial.dialed.lock().unwrap()[0].port(), 80);
        // https 주소를 평문 요청으로 보내는 것, 목적지가 없는 요청은 받지 않는다.
        for bad in [
            "GET https://example.com/ HTTP/1.1",
            "GET /local HTTP/1.1",
            "CONNECT nohost HTTP/1.1",
        ] {
            let mut c = TcpStream::connect(s.addr).await.unwrap();
            let got = ask(&mut c, &format!("{bad}\r\n{}\r\n", auth(&pass))).await;
            assert!(got.starts_with("HTTP/1.1 400"), "{bad}: {got}");
        }
    }

    #[tokio::test]
    async fn ending_the_run_revokes_the_pass_and_closes_its_tunnels() {
        let s = setup().await;
        let pass = s.net.issue();
        let header = auth(&pass);
        let mut c = TcpStream::connect(s.addr).await.unwrap();
        let got = ask(
            &mut c,
            &format!("CONNECT example.com:443 HTTP/1.1\r\n{header}\r\n"),
        )
        .await;
        assert!(got.starts_with("HTTP/1.1 200"), "{got}");
        // 다른 실행의 통행증은 따로다.
        let other = s.net.issue();
        drop(pass);
        // 열려 있던 터널이 끊긴다(남은 프로세스가 실행이 끝난 뒤에도 쓰지 못한다).
        let mut buf = [0u8; 16];
        let closed = tokio::time::timeout(Duration::from_secs(3), c.read(&mut buf)).await;
        assert!(matches!(closed, Ok(Ok(0)) | Ok(Err(_))), "{closed:?}");
        // 거둔 통행증으로는 새로 열지 못한다.
        let mut c = TcpStream::connect(s.addr).await.unwrap();
        let got = ask(
            &mut c,
            &format!("CONNECT example.com:443 HTTP/1.1\r\n{header}\r\n"),
        )
        .await;
        assert!(got.starts_with("HTTP/1.1 407"), "{got}");
        let mut c = TcpStream::connect(s.addr).await.unwrap();
        let got = ask(
            &mut c,
            &format!("CONNECT example.com:443 HTTP/1.1\r\n{}\r\n", auth(&other)),
        )
        .await;
        assert!(got.starts_with("HTTP/1.1 200"), "{got}");
    }
}
