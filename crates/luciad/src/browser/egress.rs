//! 브라우저가 갈 수 있는 곳(결정 0008): **차단 목록** 방식.
//!
//! - 공개 인터넷의 http·https는 연다. 막는 것: 사설·루프백·링크 로컬·CGNAT·멀티캐스트 같은 특수 주소,
//!   내부 이름(`localhost`, `*.local`, `*.internal`, 점 없는 이름 등), Claude 계정 쪽 도메인, 사용자가 관리 페이지에
//!   적은 도메인.
//! - 이름 검사는 두 곳에서 한다: 모델이 연다고 한 주소([`Egress::check_url`])와, 브라우저의 모든 연결이 지나는
//!   데몬 프록시([`super::proxy`]). 주소(IP) 검사는 프록시가 이름을 **한 번** 풀어 그 주소로 연결하며 한다
//!   (sbx는 이름으로 풀린 사설 IP에 CIDR 거부를 적용하지 않는다, PoC P8b).
//! - IPv4로만 나간다. IPv6는 사설 대역 검사로 LAN 기기의 공인 IPv6 주소를 가려낼 수 없어서다.

use std::net::Ipv4Addr;
use std::sync::RwLock;

use url::{Host, Url};

/// 늘 막는 이름. 항목은 그 도메인과 하위 도메인 모두를 막는다.
pub const BUILTIN_BLOCKED: &[&str] = &[
    // 내부 이름(이 서버나 LAN의 기기로 풀릴 수 있다).
    "localhost",
    "local",
    "internal",
    "lan",
    "home",
    "home.arpa",
    "arpa",
    "intranet",
    "corp",
    "private",
    "test",
    "invalid",
    "onion",
    // Claude 계정 쪽(로그인된 브라우저가 에이전트 자신의 계정에 닿지 않게).
    "anthropic.com",
    "claude.ai",
    "claude.com",
    "claudeusercontent.com",
];

/// 열 수 있는 목적지 포트.
pub const PORTS: &[u16] = &[80, 443];

/// 사용자 차단 목록 상한.
pub const USER_MAX: usize = 300;

/// 관리 페이지 설정 키(줄마다 도메인 하나).
pub const SETTING_BLOCKED: &str = "browser.blocked_hosts";

pub struct Egress {
    user: RwLock<Vec<String>>,
    /// 개발 점검(`luciad browser-check`) 전용: 그대로 여는 출처(VM 루프백의 시험 페이지). 데몬은 쓰지 않는다.
    dev_origins: Vec<String>,
}

impl Default for Egress {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

/// 특수 용도 IPv4 주소면 그 이유.
pub fn ipv4_block_reason(ip: Ipv4Addr) -> Option<&'static str> {
    let [a, b, c, _] = ip.octets();
    Some(match (a, b, c) {
        _ if ip.is_unspecified() || a == 0 => "0.0.0.0/8(이 네트워크)",
        _ if ip.is_loopback() => "루프백",
        _ if ip.is_private() => "사설 주소",
        _ if ip.is_link_local() => "링크 로컬",
        (100, 64..=127, _) => "CGNAT 공유 주소",
        (192, 0, 0) => "IETF 예약",
        (192, 88, 99) => "6to4 중계(예약)",
        (198, 18..=19, _) => "벤치마크용",
        _ if ip.is_documentation() => "문서용 주소",
        _ if ip.is_multicast() => "멀티캐스트",
        _ if ip.is_broadcast() || a >= 240 => "예약 주소",
        _ => return None,
    })
}

/// 사용자가 적은 도메인 한 줄을 다듬는다: 소문자, 앞의 `*.`·뒤의 `.` 제거. 형식이 이상하면 `Err`.
pub fn normalize_domain(raw: &str) -> Result<String, String> {
    let d = raw
        .trim()
        .trim_start_matches("*.")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    // 주소를 통째로 붙여 넣었으면 호스트만 쓴다.
    let d = match Url::parse(&d) {
        Ok(u) if u.host_str().is_some() && d.contains("://") => {
            u.host_str().unwrap_or_default().to_owned()
        }
        _ => d,
    };
    let ok = !d.is_empty()
        && d.len() <= 253
        && d.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    if ok {
        Ok(d)
    } else {
        Err(format!("도메인 형식이 아니다: {raw:?}"))
    }
}

/// `domain`이 그 도메인이거나 하위 도메인인가.
fn under(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

impl Egress {
    pub fn new(user: Vec<String>) -> Self {
        Self {
            user: RwLock::new(user),
            dev_origins: Vec::new(),
        }
    }

    /// 개발 점검 전용: 이 출처(`http://127.0.0.1:8000`)는 주소 검사를 건너뛴다. 프록시 검사와는 상관없다
    /// (Chrome은 루프백을 프록시로 보내지 않는다).
    pub fn with_dev_origin(mut self, origin: &str) -> Self {
        self.dev_origins.push(origin.to_owned());
        self
    }

    /// 관리 페이지에서 바꾼 목록으로 바꾼다.
    pub fn set_user(&self, list: Vec<String>) {
        *self.user.write().expect("egress lock") = list;
    }

    pub fn user(&self) -> Vec<String> {
        self.user.read().expect("egress lock").clone()
    }

    /// 여러 줄 글 → 다듬은 목록(중복 제거). 틀린 줄이 있으면 그 줄들을 모아 `Err`.
    pub fn parse_list(text: &str) -> Result<Vec<String>, String> {
        let mut out: Vec<String> = Vec::new();
        let mut bad = Vec::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            match normalize_domain(line) {
                Ok(d) if !out.contains(&d) => out.push(d),
                Ok(_) => {}
                Err(_) => bad.push(line.to_owned()),
            }
        }
        if !bad.is_empty() {
            return Err(format!("도메인 형식이 아닌 줄: {}", bad.join(", ")));
        }
        if out.len() > USER_MAX {
            return Err(format!("{USER_MAX}개까지 적을 수 있다"));
        }
        Ok(out)
    }

    /// 이름(도메인)을 검사한다. IP 글자는 [`Self::check_ip_literal`]로 넘긴다.
    pub fn check_name(&self, raw: &str) -> Result<(), String> {
        let host = raw.trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() {
            return Err("호스트가 없다".into());
        }
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            return Self::check_ip_literal(ip);
        }
        if !host.contains('.') {
            return Err(format!("{host}: 점 없는 이름(내부 이름)은 열지 않는다"));
        }
        if let Some(d) = BUILTIN_BLOCKED.iter().find(|d| under(&host, d)) {
            return Err(format!("{host}: 막아 둔 곳({d})"));
        }
        if let Some(d) = self
            .user
            .read()
            .expect("egress lock")
            .iter()
            .find(|d| under(&host, d))
        {
            return Err(format!("{host}: 차단 목록({d})"));
        }
        Ok(())
    }

    /// IP를 직접 적은 주소.
    pub fn check_ip_literal(ip: std::net::IpAddr) -> Result<(), String> {
        match ip {
            std::net::IpAddr::V4(v4) => match ipv4_block_reason(v4) {
                Some(why) => Err(format!("{v4}: {why}")),
                None => Ok(()),
            },
            std::net::IpAddr::V6(v6) => Err(format!("{v6}: IPv6 주소는 열지 않는다")),
        }
    }

    /// 모델이 연다고 한 주소(또는 브라우저가 도착한 주소)를 검사한다.
    pub fn check_url(&self, raw: &str) -> Result<Url, String> {
        let url = Url::parse(raw.trim()).map_err(|e| format!("주소가 아니다: {e}"))?;
        self.check_parsed(&url)?;
        Ok(url)
    }

    pub fn check_parsed(&self, url: &Url) -> Result<(), String> {
        if !self.dev_origins.is_empty()
            && self
                .dev_origins
                .iter()
                .any(|o| *o == url.origin().ascii_serialization())
        {
            return Ok(());
        }
        if !matches!(url.scheme(), "https" | "http") {
            return Err(format!(
                "http·https 주소만 열 수 있다(받은 스킴: {})",
                url.scheme()
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("주소에 사용자 정보(아이디·비밀번호)를 넣을 수 없다".into());
        }
        let port = url.port_or_known_default().unwrap_or(0);
        if !PORTS.contains(&port) {
            return Err(format!("포트 {port}는 열지 않는다(80·443만)"));
        }
        match url.host() {
            Some(Host::Domain(d)) => self.check_name(d),
            Some(Host::Ipv4(ip)) => Self::check_ip_literal(ip.into()),
            Some(Host::Ipv6(ip)) => Self::check_ip_literal(ip.into()),
            None => Err("호스트가 없다".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_ipv4_ranges_are_blocked() {
        for (ip, blocked) in [
            ("0.0.0.0", true),
            ("0.1.2.3", true),
            ("10.1.2.3", true),
            ("100.64.0.1", true),
            ("100.127.255.254", true),
            ("100.128.0.1", false),
            ("127.0.0.1", true),
            ("127.8.8.8", true),
            ("169.254.169.254", true),
            ("172.16.5.5", true),
            ("172.31.255.255", true),
            ("172.32.0.1", false),
            ("192.0.0.8", true),
            ("192.0.2.1", true),
            ("192.88.99.1", true),
            ("192.168.0.1", true),
            ("192.168.0.150", true),
            ("198.18.0.1", true),
            ("198.19.255.1", true),
            ("198.20.0.1", false),
            ("198.51.100.7", true),
            ("203.0.113.9", true),
            ("224.0.0.251", true),
            ("239.255.255.250", true),
            ("240.0.0.1", true),
            ("255.255.255.255", true),
            ("1.1.1.1", false),
            ("8.8.8.8", false),
            ("104.18.32.7", false),
        ] {
            let ip: Ipv4Addr = ip.parse().unwrap();
            assert_eq!(ipv4_block_reason(ip).is_some(), blocked, "{ip}");
        }
    }

    #[test]
    fn urls_are_checked_by_scheme_port_userinfo_and_host() {
        let e = Egress::new(vec!["blocked.example.org".into()]);
        for ok in [
            "https://quasarzone.com/bbs/x",
            "http://example.com/",
            "https://www.naver.com:443/",
            "https://1.1.1.1/",
            "https://sub.blocked.example.org.evil.com/",
        ] {
            assert!(e.check_url(ok).is_ok(), "{ok} 는 열려야 한다");
        }
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,hi",
            "chrome://settings",
            "ftp://example.com/",
            "https://user:pw@example.com/",
            "https://example.com:8443/",
            "http://localhost/",
            "http://LOCALHOST./",
            "http://foo.localhost/",
            "http://nas.local/",
            "http://host.docker.internal:47810/",
            "http://router/",
            "http://192.168.0.1/",
            "http://0x7f.1/",
            "http://2130706433/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "https://[2001:4860:4860::8888]/",
            "https://claude.ai/",
            "https://api.anthropic.com/v1/messages",
            "https://blocked.example.org/",
            "https://a.b.blocked.example.org/",
            "quasarzone.com",
        ] {
            assert!(e.check_url(bad).is_err(), "{bad} 는 막혀야 한다");
        }
        assert!(
            e.check_name("192.168.0.250").is_err(),
            "프록시가 받은 이름이 IP 글자면 IP로 본다"
        );
    }

    #[test]
    fn user_list_is_normalized_and_validated() {
        let list = Egress::parse_list(
            "Example.COM\n*.ads.example.net.\n# 주석\n\nhttps://Tracker.io/path\nexample.com\n",
        )
        .unwrap();
        assert_eq!(list, ["example.com", "ads.example.net", "tracker.io"]);
        assert!(Egress::parse_list("good.com\nbad domain\n").is_err());
        let e = Egress::default();
        assert!(e.check_name("www.example.com").is_ok());
        e.set_user(list);
        assert!(e.check_name("www.example.com").is_err(), "하위 도메인도");
        assert!(e.check_name("notexample.com").is_ok(), "점 경계");
    }
}
