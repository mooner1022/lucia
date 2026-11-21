//! 브라우저 출구 프록시(결정 0008): 데몬이 여는 SOCKS5(인증 없음, CONNECT만).
//!
//! - 브라우저 샌드박스는 이 포트 하나만 허용하고(`deploy/create-browser-sandbox.sh`), Chrome은
//!   `--proxy-server=socks5://host.docker.internal:<포트>`로 모든 연결을 여기로 보낸다(이름은 Chrome이 풀지 않고
//!   그대로 넘긴다).
//! - 연결마다: 포트(80·443) → 이름 검사([`Egress::check_name`]) → 이름을 **한 번** 풀어 IPv4 주소를 모두 검사 →
//!   검사한 그 주소로 연결. 풀린 주소 중 하나라도 특수 주소면 통째로 거절한다(다시 풀지 않으므로 DNS
//!   리바인딩에 뚫리지 않는다).
//! - 루프백에만 묶는다. 상주 샌드박스(`lucia`)는 이 포트를 허용하지 않는다(허용하면 에이전트가 인터넷을 얻는다).
//! - 최근 결정을 모아 둔다: 페이지가 안 열린 까닭을 모델에게 알리고 관리 페이지에 보인다.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::BoxFut;
use super::egress::{Egress, PORTS, ipv4_block_reason};

const HANDSHAKE: Duration = Duration::from_secs(10);
const CONNECT: Duration = Duration::from_secs(10);
const MAX_CONNECTIONS: usize = 256;
const LOG_MAX: usize = 300;

/// 이름 풀기(실제는 이 서버의 리졸버, 시험은 가짜).
pub trait Resolve: Send + Sync {
    fn lookup<'a>(&'a self, host: &'a str) -> BoxFut<'a, std::io::Result<Vec<IpAddr>>>;
}

/// 연결(시험에서 실제로 어디로 연결하려 했는지 본다).
pub trait Dial: Send + Sync {
    fn dial(&self, addr: SocketAddrV4) -> BoxFut<'_, std::io::Result<TcpStream>>;
}

pub struct SystemResolver;

impl Resolve for SystemResolver {
    fn lookup<'a>(&'a self, host: &'a str) -> BoxFut<'a, std::io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            Ok(tokio::net::lookup_host((host, 0))
                .await?
                .map(|a| a.ip())
                .collect())
        })
    }
}

pub struct TcpDial;

impl Dial for TcpDial {
    fn dial(&self, addr: SocketAddrV4) -> BoxFut<'_, std::io::Result<TcpStream>> {
        Box::pin(TcpStream::connect(addr))
    }
}

/// 프록시의 결정 하나.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Decision {
    pub at: DateTime<Utc>,
    pub host: String,
    pub port: u16,
    /// 막았으면 이유.
    pub blocked: Option<String>,
}

pub struct Proxy {
    egress: Arc<Egress>,
    resolver: Arc<dyn Resolve>,
    dial: Arc<dyn Dial>,
    log: Mutex<VecDeque<Decision>>,
    slots: Arc<Semaphore>,
}

/// SOCKS5 응답 코드.
mod rep {
    pub const OK: u8 = 0;
    pub const NOT_ALLOWED: u8 = 2;
    pub const HOST_UNREACHABLE: u8 = 4;
    pub const REFUSED: u8 = 5;
    pub const COMMAND: u8 = 7;
    pub const ADDRESS_TYPE: u8 = 8;
}

impl Proxy {
    pub fn new(egress: Arc<Egress>, resolver: Arc<dyn Resolve>, dial: Arc<dyn Dial>) -> Arc<Self> {
        Arc::new(Self {
            egress,
            resolver,
            dial,
            log: Mutex::default(),
            slots: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        })
    }

    pub fn system(egress: Arc<Egress>) -> Arc<Self> {
        Self::new(egress, Arc::new(SystemResolver), Arc::new(TcpDial))
    }

    fn note(&self, host: &str, port: u16, blocked: Option<String>) {
        if let Some(why) = &blocked {
            tracing::info!(host, port, why, "브라우저 연결을 막았다");
        } else {
            tracing::debug!(host, port, "브라우저 연결");
        }
        let mut log = self.log.lock().expect("proxy log");
        if log.len() >= LOG_MAX {
            log.pop_front();
        }
        log.push_back(Decision {
            at: Utc::now(),
            host: host.to_owned(),
            port,
            blocked,
        });
    }

    /// 최근에 막은 것(새것부터).
    pub fn recent_blocks(&self, limit: usize) -> Vec<Decision> {
        self.log
            .lock()
            .expect("proxy log")
            .iter()
            .rev()
            .filter(|d| d.blocked.is_some())
            .take(limit)
            .cloned()
            .collect()
    }

    /// `since` 뒤에 막은 것 중 그 호스트의 것(페이지가 안 열린 까닭).
    pub fn block_reason(&self, host: &str, since: DateTime<Utc>) -> Option<String> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.log
            .lock()
            .expect("proxy log")
            .iter()
            .rev()
            .take_while(|d| d.at >= since)
            .find(|d| d.host == host && d.blocked.is_some())
            .and_then(|d| d.blocked.clone())
    }

    /// `since` 뒤에 막은 연결 수(하위 요청을 포함해).
    pub fn blocked_since(&self, since: DateTime<Utc>) -> Vec<Decision> {
        self.log
            .lock()
            .expect("proxy log")
            .iter()
            .filter(|d| d.at >= since && d.blocked.is_some())
            .cloned()
            .collect()
    }

    /// 감시(결정 0019) 같은 데몬의 직접 요청도 같은 검사를 쓴다: 이름을 한 번 풀어 모든 답을 검사한
    /// IPv4 주소들(80·443만). 그 주소로만 연결해야 한다(이름을 다시 풀지 않는다).
    pub async fn checked_addrs(&self, host: &str, port: u16) -> Result<Vec<SocketAddrV4>, String> {
        let result = self.decide(host, port).await.map_err(|(_, why)| why);
        if let Err(why) = &result {
            self.note(host, port, Some(why.clone()));
        }
        result
    }

    /// 같은 검사를 거쳐 목적지에 연결한다(스크립트 출구 프록시, 결정 0058). 기록은 부르는 쪽이 남긴다.
    pub async fn connect(&self, host: &str, port: u16) -> Result<TcpStream, String> {
        let targets = self.decide(host, port).await.map_err(|(_, why)| why)?;
        let mut last = "연결할 주소가 없다".to_owned();
        for addr in &targets {
            match tokio::time::timeout(CONNECT, self.dial.dial(*addr)).await {
                Ok(Ok(s)) => return Ok(s),
                Ok(Err(e)) => last = format!("연결하지 못했다: {e}"),
                Err(_) => last = "연결 시간 초과".into(),
            }
        }
        Err(last)
    }

    /// 목적지를 정한다: 연결할 IPv4 주소들(검사를 마친 것) 또는 (응답 코드, 이유).
    async fn decide(&self, host: &str, port: u16) -> Result<Vec<SocketAddrV4>, (u8, String)> {
        if !PORTS.contains(&port) {
            return Err((rep::NOT_ALLOWED, format!("포트 {port}(80·443만)")));
        }
        self.egress
            .check_name(host)
            .map_err(|e| (rep::NOT_ALLOWED, e))?;
        let ips: Vec<IpAddr> = match host.parse::<IpAddr>() {
            Ok(ip) => vec![ip],
            Err(_) => tokio::time::timeout(CONNECT, self.resolver.lookup(host))
                .await
                .map_err(|_| (rep::HOST_UNREACHABLE, "이름 풀기 시간 초과".to_owned()))?
                .map_err(|e| (rep::HOST_UNREACHABLE, format!("이름을 풀지 못했다: {e}")))?,
        };
        let mut v4 = Vec::new();
        for ip in &ips {
            match ip {
                IpAddr::V4(a) => {
                    if let Some(why) = ipv4_block_reason(*a) {
                        return Err((rep::NOT_ALLOWED, format!("{host} → {a}: {why}")));
                    }
                    v4.push(SocketAddrV4::new(*a, port));
                }
                IpAddr::V6(a) => {
                    // IPv4-매핑 주소는 IPv4로 보고 검사한다(그 주소로는 연결하지 않는다).
                    if let Some(m) = a.to_ipv4_mapped()
                        && let Some(why) = ipv4_block_reason(m)
                    {
                        return Err((rep::NOT_ALLOWED, format!("{host} → {a}: {why}")));
                    }
                }
            }
        }
        if v4.is_empty() {
            return Err((
                rep::HOST_UNREACHABLE,
                format!("{host}: IPv4 주소가 없다(IPv6로는 나가지 않는다)"),
            ));
        }
        Ok(v4)
    }

    async fn handle(&self, mut client: TcpStream) -> std::io::Result<()> {
        let request = tokio::time::timeout(HANDSHAKE, read_request(&mut client)).await;
        let (host, port) = match request {
            Ok(Ok(Ok(target))) => target,
            Ok(Ok(Err(code))) => {
                reply(&mut client, code).await?;
                return Ok(());
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => return Ok(()),
        };
        let targets = match self.decide(&host, port).await {
            Ok(t) => t,
            Err((code, why)) => {
                self.note(&host, port, Some(why));
                reply(&mut client, code).await?;
                return Ok(());
            }
        };
        let mut upstream = None;
        let mut last_err = None;
        for addr in &targets {
            match tokio::time::timeout(CONNECT, self.dial.dial(*addr)).await {
                Ok(Ok(s)) => {
                    upstream = Some(s);
                    break;
                }
                Ok(Err(e)) => last_err = Some(e.to_string()),
                Err(_) => last_err = Some("연결 시간 초과".into()),
            }
        }
        let Some(mut upstream) = upstream else {
            tracing::debug!(host, port, err = ?last_err, "브라우저 연결 실패");
            reply(&mut client, rep::REFUSED).await?;
            return Ok(());
        };
        self.note(&host, port, None);
        reply(&mut client, rep::OK).await?;
        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
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
            "브라우저 프록시는 루프백에만 묶는다"
        );
        let listener = TcpListener::bind(bind).await?;
        let local = listener.local_addr()?;
        tracing::info!(%local, "브라우저 출구 프록시");
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = cancel.cancelled() => return,
                    a = listener.accept() => a,
                };
                let (stream, peer) = match accepted {
                    Ok(a) => a,
                    Err(e) => {
                        tracing::warn!(err = %e, "브라우저 프록시 accept");
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let Ok(permit) = self.slots.clone().try_acquire_owned() else {
                    tracing::warn!("브라우저 프록시 연결이 너무 많다");
                    continue;
                };
                let me = self.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(e) = me.handle(stream).await {
                        tracing::debug!(err = %e, "브라우저 프록시 연결");
                    }
                });
            }
        });
        Ok(local)
    }
}

/// 인사와 요청을 읽는다. 받을 수 없는 요청이면 안쪽 `Err(응답 코드)`.
async fn read_request(client: &mut TcpStream) -> std::io::Result<Result<(String, u16), u8>> {
    let mut head = [0u8; 2];
    client.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(std::io::Error::other("SOCKS5가 아니다"));
    }
    let mut methods = vec![0u8; head[1] as usize];
    client.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        client.write_all(&[5, 0xFF]).await?;
        return Err(std::io::Error::other("인증 없는 방식을 고르지 않았다"));
    }
    client.write_all(&[5, 0]).await?;

    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;
    if req[0] != 5 {
        return Err(std::io::Error::other("SOCKS5 요청이 아니다"));
    }
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            client.read_exact(&mut a).await?;
            Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            client.read_exact(&mut name).await?;
            match String::from_utf8(name) {
                Ok(n) => n.trim_end_matches('.').to_ascii_lowercase(),
                Err(_) => {
                    let mut port = [0u8; 2];
                    client.read_exact(&mut port).await?;
                    return Ok(Err(rep::ADDRESS_TYPE));
                }
            }
        }
        4 => {
            let mut a = [0u8; 16];
            client.read_exact(&mut a).await?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        _ => return Ok(Err(rep::ADDRESS_TYPE)),
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await?;
    if req[1] != 1 {
        return Ok(Err(rep::COMMAND));
    }
    Ok(Ok((host, u16::from_be_bytes(port))))
}

async fn reply(client: &mut TcpStream, code: u8) -> std::io::Result<()> {
    client.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    if code != rep::OK {
        let _ = client.shutdown().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// 이름마다 정해 둔 주소를 돌려준다. 같은 이름을 두 번 물으면 두 번째 값을 준다(리바인딩 흉내).
    #[derive(Default)]
    struct FakeResolver {
        answers: Mutex<HashMap<String, Vec<Vec<IpAddr>>>>,
        asked: Mutex<Vec<String>>,
    }

    impl FakeResolver {
        fn set(&self, host: &str, answers: &[&[&str]]) {
            self.answers.lock().unwrap().insert(
                host.into(),
                answers
                    .iter()
                    .map(|a| a.iter().map(|s| s.parse().unwrap()).collect())
                    .collect(),
            );
        }
    }

    impl Resolve for FakeResolver {
        fn lookup<'a>(&'a self, host: &'a str) -> BoxFut<'a, std::io::Result<Vec<IpAddr>>> {
            self.asked.lock().unwrap().push(host.to_owned());
            let mut answers = self.answers.lock().unwrap();
            let out = match answers.get_mut(host) {
                Some(list) if list.len() > 1 => Ok(list.remove(0)),
                Some(list) => Ok(list[0].clone()),
                None => Err(std::io::Error::other("NXDOMAIN")),
            };
            Box::pin(async move { out })
        }
    }

    /// 어디로 연결하려 했는지 적고, 실제로는 시험용 서버로 잇는다.
    struct FakeDial {
        target: SocketAddr,
        dialed: Mutex<Vec<SocketAddrV4>>,
    }

    impl Dial for FakeDial {
        fn dial(&self, addr: SocketAddrV4) -> BoxFut<'_, std::io::Result<TcpStream>> {
            self.dialed.lock().unwrap().push(addr);
            Box::pin(TcpStream::connect(self.target))
        }
    }

    async fn echo_server() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 64];
                    if let Ok(n) = s.read(&mut buf).await {
                        let _ = s.write_all(&buf[..n]).await;
                    }
                });
            }
        });
        addr
    }

    async fn setup() -> (Arc<Proxy>, SocketAddr, Arc<FakeResolver>, Arc<FakeDial>) {
        let resolver = Arc::new(FakeResolver::default());
        let dial = Arc::new(FakeDial {
            target: echo_server().await,
            dialed: Mutex::default(),
        });
        let proxy = Proxy::new(
            Arc::new(Egress::new(vec!["blocked.example".into()])),
            resolver.clone(),
            dial.clone(),
        );
        let addr = proxy
            .clone()
            .serve("127.0.0.1:0".parse().unwrap(), CancellationToken::new())
            .await
            .unwrap();
        (proxy, addr, resolver, dial)
    }

    /// SOCKS5 CONNECT를 보내고 응답 코드를 돌려준다. 성공이면 "ping"을 보내 메아리를 확인한다.
    async fn connect(proxy: SocketAddr, atyp: u8, addr: &[u8], port: u16) -> u8 {
        let mut s = TcpStream::connect(proxy).await.unwrap();
        s.write_all(&[5, 1, 0]).await.unwrap();
        let mut m = [0u8; 2];
        s.read_exact(&mut m).await.unwrap();
        assert_eq!(m, [5, 0]);
        let mut req = vec![5, 1, 0, atyp];
        if atyp == 3 {
            req.push(addr.len() as u8);
        }
        req.extend_from_slice(addr);
        req.extend_from_slice(&port.to_be_bytes());
        s.write_all(&req).await.unwrap();
        let mut r = [0u8; 10];
        s.read_exact(&mut r).await.unwrap();
        if r[1] == 0 {
            s.write_all(b"ping").await.unwrap();
            let mut back = [0u8; 4];
            s.read_exact(&mut back).await.unwrap();
            assert_eq!(&back, b"ping");
        }
        r[1]
    }

    async fn name(proxy: SocketAddr, host: &str, port: u16) -> u8 {
        connect(proxy, 3, host.as_bytes(), port).await
    }

    #[tokio::test]
    async fn public_names_pass_and_connect_to_the_checked_address() {
        let (proxy, addr, resolver, dial) = setup().await;
        resolver.set("quasarzone.com", &[&["104.18.1.1", "2606:4700::1"]]);
        assert_eq!(name(addr, "QuasarZone.com.", 443).await, rep::OK);
        assert_eq!(
            *dial.dialed.lock().unwrap(),
            [SocketAddrV4::new("104.18.1.1".parse().unwrap(), 443)],
            "IPv4로만, 검사한 그 주소로"
        );
        assert!(proxy.recent_blocks(5).is_empty());
    }

    #[tokio::test]
    async fn private_answers_internal_names_and_odd_ports_are_refused() {
        let (proxy, addr, resolver, dial) = setup().await;
        resolver.set("192.168.0.250.nip.io", &[&["192.168.0.250"]]);
        resolver.set("mixed.example.com", &[&["104.18.1.1", "10.0.0.5"]]);
        resolver.set("v6only.example.com", &[&["2606:4700::1"]]);
        resolver.set("mapped.example.com", &[&["::ffff:127.0.0.1", "104.18.1.1"]]);
        for (host, port, code) in [
            ("192.168.0.250.nip.io", 443, rep::NOT_ALLOWED),
            ("mixed.example.com", 443, rep::NOT_ALLOWED),
            ("mapped.example.com", 443, rep::NOT_ALLOWED),
            ("v6only.example.com", 443, rep::HOST_UNREACHABLE),
            ("localhost", 80, rep::NOT_ALLOWED),
            ("host.docker.internal", 80, rep::NOT_ALLOWED),
            ("nas", 80, rep::NOT_ALLOWED),
            ("api.anthropic.com", 443, rep::NOT_ALLOWED),
            ("www.blocked.example", 443, rep::NOT_ALLOWED),
            ("127.0.0.1", 80, rep::NOT_ALLOWED),
            ("0x7f.0.0.1", 80, rep::HOST_UNREACHABLE),
            ("example.com", 22, rep::NOT_ALLOWED),
            ("nxdomain.example.com", 443, rep::HOST_UNREACHABLE),
        ] {
            assert_eq!(name(addr, host, port).await, code, "{host}:{port}");
        }
        // IPv4·IPv6 주소 형식으로 직접.
        assert_eq!(
            connect(addr, 1, &[10, 1, 2, 3], 443).await,
            rep::NOT_ALLOWED
        );
        assert_eq!(
            connect(
                addr,
                4,
                &"::1".parse::<std::net::Ipv6Addr>().unwrap().octets(),
                443
            )
            .await,
            rep::NOT_ALLOWED
        );
        assert!(
            dial.dialed.lock().unwrap().is_empty(),
            "하나도 연결하지 않았다"
        );
        let reason = proxy
            .block_reason(
                "192.168.0.250.nip.io",
                Utc::now() - chrono::Duration::minutes(1),
            )
            .unwrap();
        assert!(reason.contains("사설"), "{reason}");
    }

    #[tokio::test]
    async fn names_are_resolved_once_so_rebinding_cannot_swap_the_address() {
        let (_proxy, addr, resolver, dial) = setup().await;
        // 처음엔 공인 주소, 두 번째로 물으면 사설 주소를 주는 이름.
        resolver.set(
            "rebind.example.com",
            &[&["93.184.216.34"], &["192.168.0.1"]],
        );
        assert_eq!(name(addr, "rebind.example.com", 443).await, rep::OK);
        assert_eq!(resolver.asked.lock().unwrap().len(), 1, "한 번만 푼다");
        assert_eq!(
            dial.dialed.lock().unwrap()[0].ip().to_string(),
            "93.184.216.34"
        );
        // 다음 연결은 다시 풀고, 이번 답(사설)은 막힌다.
        assert_eq!(
            name(addr, "rebind.example.com", 443).await,
            rep::NOT_ALLOWED
        );
        assert_eq!(dial.dialed.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn non_connect_commands_are_refused() {
        let (_proxy, addr, _resolver, _dial) = setup().await;
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(&[5, 1, 0]).await.unwrap();
        let mut m = [0u8; 2];
        s.read_exact(&mut m).await.unwrap();
        // UDP ASSOCIATE
        s.write_all(&[5, 3, 0, 1, 1, 1, 1, 1, 0, 53]).await.unwrap();
        let mut r = [0u8; 10];
        s.read_exact(&mut r).await.unwrap();
        assert_eq!(r[1], rep::COMMAND);
    }
}
