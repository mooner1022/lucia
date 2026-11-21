//! 외부 노출(결정 0043): 관리 페이지와 접속 API를 리버스 프록시(예: Traefik, TLS를 맡는다) 뒤에서 인터넷에 연다.
//!
//! - 설정 `[public]`이 비어 있으면 아무것도 바뀌지 않는다(집 LAN·VPN에서만, 결정 0002·0021).
//! - **누가 보냈나**: 소켓 상대가 믿는 프록시(`trusted_proxies`)일 때만 `X-Forwarded-For`를 읽고, 그 요청을
//!   "외부"로 본다. 오른쪽부터 믿는 프록시가 아닌 첫 주소가 실제 상대다(왼쪽 값은 클라이언트가 지어낼 수 있다).
//!   믿는 프록시가 머리를 주지 않으면 거절한다(누군지 모르는 외부 요청을 프록시 주소 하나로 뭉치지 않게).
//! - 프록시가 아닌 상대에서 온 요청은 예전처럼 "집 안"이다(바인드가 LAN·루프백뿐이다).

use std::net::IpAddr;

use axum::http::HeaderMap;

/// 믿는 프록시 목록(IP나 CIDR)과, 있으면 실제 접속 주소를 싣는 머리.
#[derive(Debug, Clone, Default)]
pub struct Proxies(Vec<(IpAddr, u8)>, Option<String>);

impl Proxies {
    /// `"172.18.0.11"`, `"172.18.0.0/16"`, `"::1"` 같은 글을 읽는다.
    pub fn parse(list: &[String]) -> Result<Self, String> {
        let mut out = Vec::new();
        for raw in list {
            let s = raw.trim();
            let (addr, bits) = match s.split_once('/') {
                Some((a, b)) => (a, Some(b)),
                None => (s, None),
            };
            let ip: IpAddr = addr
                .parse()
                .map_err(|_| format!("믿는 프록시 주소 형식이 아니다: {s}"))?;
            let ip = normalize(ip);
            let max = if ip.is_ipv4() { 32 } else { 128 };
            let bits = match bits {
                Some(b) => b
                    .parse::<u8>()
                    .ok()
                    .filter(|b| *b <= max)
                    .ok_or_else(|| format!("믿는 프록시 대역 형식이 아니다: {s}"))?,
                None => max,
            };
            if !crate::gateway::local_peer(ip) {
                return Err(format!(
                    "믿는 프록시는 이 호스트의 루프백·사설 주소여야 한다: {s}"
                ));
            }
            out.push((ip, bits));
        }
        Ok(Self(out, None))
    }

    /// 믿는 프록시가 실제 접속 주소를 이 머리 하나에 싣는다(예: Cloudflare 뒤의 Traefik이 `cloudflarewarp`로 채우는
    /// `X-Real-Ip`). 정하면 `X-Forwarded-For` 대신 이것을 읽는다. 앞단(Cloudflare)의 주소가 `X-Forwarded-For`
    /// 오른쪽에 덧붙어, 오른쪽부터 읽으면 엣지 주소가 나오기 때문이다(결정 0043 후속).
    pub fn with_real_ip_header(mut self, name: Option<&str>) -> Self {
        self.1 = name
            .map(|n| n.trim().to_ascii_lowercase())
            .filter(|n| !n.is_empty());
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = normalize(ip);
        self.0.iter().any(|(net, bits)| in_net(ip, *net, *bits))
    }
}

/// IPv4에 대응한 IPv6 주소(`::ffff:a.b.c.d`)는 IPv4로 본다.
pub fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

fn in_net(ip: IpAddr, net: IpAddr, bits: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            (u32::from(a) & mask) == (u32::from(n) & mask)
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            (u128::from(a) & mask) == (u128::from(n) & mask)
        }
        _ => false,
    }
}

/// 요청이 어디서 왔나.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// 프록시를 거치지 않은 연결(집 LAN·VPN·루프백). 주소는 소켓 상대.
    Local(IpAddr),
    /// 믿는 프록시를 거쳐 온 외부 요청. 주소는 `X-Forwarded-For`에서 읽은 실제 상대.
    External(IpAddr),
}

impl Origin {
    pub fn ip(self) -> IpAddr {
        match self {
            Self::Local(ip) | Self::External(ip) => ip,
        }
    }

    pub fn external(self) -> bool {
        matches!(self, Self::External(_))
    }
}

/// 소켓 상대와 머리로 요청의 출처를 정한다. 믿는 프록시가 `X-Forwarded-For`를 주지 않았으면 `Err`.
pub fn resolve(peer: IpAddr, headers: &HeaderMap, proxies: &Proxies) -> Result<Origin, String> {
    let peer = normalize(peer);
    if !proxies.contains(peer) {
        // 믿지 않는 상대가 전달 머리를 실었다: 프록시 주소를 잘못 적었거나(Traefik 주소가 바뀌었다) 지어낸 것이다.
        // 집 안으로 받으면 밖의 요청이 집 안 대접을 받으므로 거절한다(닫힌 쪽으로 실패).
        if headers.contains_key("x-forwarded-for") || headers.contains_key("forwarded") {
            return Err("믿지 않는 상대가 전달 머리(X-Forwarded-For)를 실었다".into());
        }
        return Ok(Origin::Local(peer));
    }
    if let Some(name) = &proxies.1 {
        let mut values = headers.get_all(name.as_str()).iter();
        let (Some(value), None) = (values.next(), values.next()) else {
            return Err(format!("믿는 프록시가 {name}를 하나만 주지 않았다"));
        };
        let ip = value
            .to_str()
            .ok()
            .and_then(|v| parse_hop(v.trim()))
            .ok_or_else(|| format!("{name} 형식이 아니다"))?;
        return Ok(Origin::External(ip));
    }
    let mut hops: Vec<&str> = Vec::new();
    for value in headers.get_all("x-forwarded-for") {
        let Ok(text) = value.to_str() else {
            return Err("X-Forwarded-For를 읽지 못했다".into());
        };
        hops.extend(text.split(',').map(str::trim).filter(|s| !s.is_empty()));
    }
    for hop in hops.iter().rev() {
        let ip = parse_hop(hop).ok_or_else(|| "X-Forwarded-For 형식이 아니다".to_owned())?;
        if !proxies.contains(ip) {
            return Ok(Origin::External(ip));
        }
    }
    Err("믿는 프록시가 X-Forwarded-For를 주지 않았다".into())
}

/// `1.2.3.4`, `1.2.3.4:5678`, `[::1]:80`, `::1`.
fn parse_hop(hop: &str) -> Option<IpAddr> {
    if let Ok(ip) = hop.parse::<IpAddr>() {
        return Some(normalize(ip));
    }
    if let Ok(sa) = hop.parse::<std::net::SocketAddr>() {
        return Some(normalize(sa.ip()));
    }
    None
}

/// 공개 주소(`https://host[:port]`)를 확인하고, 요청의 Host 머리와 맞출 값(`host[:port]`, 소문자)을 돌려준다.
pub fn public_host(url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(url.trim()).map_err(|_| format!("주소 형식이 아니다: {url}"))?;
    if parsed.scheme() != "https" {
        return Err(format!("공개 주소는 https여야 한다: {url}"));
    }
    if !(parsed.path() == "/" || parsed.path().is_empty())
        || parsed.query().is_some()
        || !parsed.username().is_empty()
    {
        return Err(format!(
            "공개 주소에는 호스트만 쓴다(경로·질의 없이): {url}"
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("호스트가 없다: {url}"))?
        .to_ascii_lowercase();
    Ok(match parsed.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    })
}

/// 공개 주소의 끝 `/`를 뗀 앞부분(링크를 만들 때).
pub fn base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(xff: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in xff {
            h.append("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn proxies_parse_and_match() {
        let p = Proxies::parse(&["172.18.0.0/16".into(), "127.0.0.1".into()]).unwrap();
        assert!(p.contains("172.18.3.4".parse().unwrap()));
        assert!(p.contains("::ffff:172.18.0.11".parse().unwrap()));
        assert!(!p.contains("172.20.0.1".parse().unwrap()));
        assert!(p.contains("127.0.0.1".parse().unwrap()));
        assert!(
            Proxies::parse(&["8.8.8.8".into()]).is_err(),
            "공인 주소는 프록시로 믿지 않는다"
        );
        assert!(Proxies::parse(&["10.0.0.0/33".into()]).is_err());
        assert!(Proxies::parse(&["nope".into()]).is_err());
    }

    #[test]
    fn origin_trusts_forwarded_for_only_from_proxies() {
        let p = Proxies::parse(&["172.18.0.11".into()]).unwrap();
        // 프록시가 아닌 상대: 머리가 없으면 집 안, 전달 머리를 실었으면 거절(프록시 설정이 틀렸거나 지어냈다).
        assert_eq!(
            resolve("192.168.0.5".parse().unwrap(), &headers(&[]), &p).unwrap(),
            Origin::Local("192.168.0.5".parse().unwrap())
        );
        assert!(resolve("192.168.0.5".parse().unwrap(), &headers(&["1.2.3.4"]), &p).is_err());
        let mut fwd = HeaderMap::new();
        fwd.insert("forwarded", HeaderValue::from_static("for=1.2.3.4"));
        assert!(resolve("192.168.0.5".parse().unwrap(), &fwd, &p).is_err());
        // 프록시: 오른쪽부터 프록시가 아닌 첫 주소. 왼쪽의 지어낸 값은 쓰지 않는다.
        assert_eq!(
            resolve(
                "172.18.0.11".parse().unwrap(),
                &headers(&["9.9.9.9, 1.2.3.4"]),
                &p
            )
            .unwrap(),
            Origin::External("1.2.3.4".parse().unwrap())
        );
        assert_eq!(
            resolve(
                "172.18.0.11".parse().unwrap(),
                &headers(&["9.9.9.9", "1.2.3.4, 172.18.0.11"]),
                &p
            )
            .unwrap(),
            Origin::External("1.2.3.4".parse().unwrap())
        );
        // 머리가 없거나 프록시뿐이거나 깨졌으면 거절.
        assert!(resolve("172.18.0.11".parse().unwrap(), &headers(&[]), &p).is_err());
        assert!(
            resolve(
                "172.18.0.11".parse().unwrap(),
                &headers(&["172.18.0.11"]),
                &p
            )
            .is_err()
        );
        assert!(resolve("172.18.0.11".parse().unwrap(), &headers(&["bogus"]), &p).is_err());
        // 집 안 주소가 프록시를 거쳐 와도 외부로 본다(공개 입구를 지났다).
        assert_eq!(
            resolve(
                "172.18.0.11".parse().unwrap(),
                &headers(&["192.168.0.9"]),
                &p
            )
            .unwrap(),
            Origin::External("192.168.0.9".parse().unwrap())
        );
    }

    /// Cloudflare 뒤(결정 0043 후속): X-Forwarded-For는 "실제, 엣지"라 오른쪽부터 읽으면 엣지가 나온다.
    #[test]
    fn real_ip_header_wins_behind_a_cdn() {
        let p = Proxies::parse(&["172.18.0.3".into()])
            .unwrap()
            .with_real_ip_header(Some("X-Real-Ip"));
        let mut h = headers(&["116.33.158.182, 172.68.85.152"]);
        h.insert("x-real-ip", HeaderValue::from_static("116.33.158.182"));
        assert_eq!(
            resolve("172.18.0.3".parse().unwrap(), &h, &p).unwrap(),
            Origin::External("116.33.158.182".parse().unwrap())
        );
        // 머리가 없거나 둘이거나 깨졌으면 거절.
        assert!(resolve("172.18.0.3".parse().unwrap(), &headers(&["1.2.3.4"]), &p).is_err());
        let mut two = h.clone();
        two.append("x-real-ip", HeaderValue::from_static("9.9.9.9"));
        assert!(resolve("172.18.0.3".parse().unwrap(), &two, &p).is_err());
        let mut bad = HeaderMap::new();
        bad.insert("x-real-ip", HeaderValue::from_static("nope"));
        assert!(resolve("172.18.0.3".parse().unwrap(), &bad, &p).is_err());
        // 프록시가 아닌 상대가 실은 X-Real-Ip는 읽지 않는다(집 안).
        let mut local = HeaderMap::new();
        local.insert("x-real-ip", HeaderValue::from_static("8.8.8.8"));
        assert_eq!(
            resolve("192.168.0.5".parse().unwrap(), &local, &p).unwrap(),
            Origin::Local("192.168.0.5".parse().unwrap())
        );
    }

    #[test]
    fn public_host_wants_plain_https() {
        assert_eq!(
            public_host("https://Lucia.Example.com/").unwrap(),
            "lucia.example.com"
        );
        assert_eq!(
            public_host("https://lucia.example.com:8443").unwrap(),
            "lucia.example.com:8443"
        );
        assert!(public_host("http://lucia.example.com").is_err());
        assert!(public_host("https://lucia.example.com/admin").is_err());
        assert!(public_host("lucia.example.com").is_err());
        assert_eq!(base("https://a.example/"), "https://a.example");
    }
}
