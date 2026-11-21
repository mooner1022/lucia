//! 답을 메시지 여러 개로 나눈다(결정 0005). 구분자는 `%%`만 있는 줄이다(`\n%%\n`).
//!
//! 모든 보고→메시지 경로(채팅, 브리핑, 델타가 없을 때)가 [`split`]을 쓴다. 그래서 `%%`가 그대로 나가지 않는다.
//! 채팅은 [`Streamer`]로 받는 대로 나눠 먼저 보낸다.
//! 말풍선 사이는 [`Pacer`]가 사람이 치는 시간처럼 조금 띄운다(결정 0026).

/// 구분자 줄(앞뒤 공백 무시).
fn is_separator(line: &str) -> bool {
    line.trim() == "%%"
}

/// 전체 글을 조각으로. 앞뒤 공백을 다듬고 빈 조각은 버린다. 맨 앞·맨 뒤 구분자도 괜찮다.
pub fn split(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for line in text.split('\n') {
        if is_separator(line) {
            let piece = current.trim();
            if !piece.is_empty() {
                parts.push(piece.to_owned());
            }
            current.clear();
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }
    let piece = current.trim();
    if !piece.is_empty() {
        parts.push(piece.to_owned());
    }
    parts
}

/// 조각을 이어 하나의 글로(에피소드 기록용).
pub fn join(parts: &[String]) -> String {
    parts.join("\n\n")
}

/// 받는 대로 나눈다. 지금까지 받은 글 전체를 넣으면, 구분자 줄이 완성된 곳까지 새 조각을 내놓는다.
#[derive(Debug, Default, Clone)]
pub struct Streamer {
    /// 이미 조각으로 내보낸 글의 끝(바이트, 구분자 줄 다음).
    consumed: usize,
    /// 그때까지의 글(나중에 최종 글과 맞춰 본다).
    consumed_text: String,
}

impl Streamer {
    pub fn feed(&mut self, so_far: &str) -> Vec<String> {
        let mut out = Vec::new();
        if !so_far.starts_with(&self.consumed_text) {
            // 받은 글이 앞과 달라졌다(같은 도구를 다시 불렀다 등). 더 나누지 않는다.
            return out;
        }
        loop {
            let rest = &so_far[self.consumed..];
            // 줄 시작 위치마다 그 줄이 완성된(뒤에 \n이 있는) 구분자인지 본다.
            let mut found = None;
            let mut line_start = 0;
            while let Some(nl) = rest[line_start..].find('\n') {
                let line_end = line_start + nl;
                if is_separator(&rest[line_start..line_end]) {
                    found = Some((line_start, line_end + 1));
                    break;
                }
                line_start = line_end + 1;
            }
            let Some((sep_start, after)) = found else {
                break;
            };
            let piece = rest[..sep_start].trim();
            if !piece.is_empty() {
                out.push(piece.to_owned());
            }
            self.consumed += after;
            self.consumed_text = so_far[..self.consumed].to_owned();
        }
        out
    }

    /// 무엇이라도 내보냈는가.
    #[cfg(test)]
    pub fn started(&self) -> bool {
        self.consumed > 0
    }

    /// 최종 글에서 아직 내보내지 않은 부분. 최종 글이 내보낸 글로 시작하지 않으면 `None`.
    pub fn rest<'a>(&self, final_text: &'a str) -> Option<&'a str> {
        final_text
            .starts_with(&self.consumed_text)
            .then(|| &final_text[self.consumed..])
    }
}

/// 나눠 보낼 때 말풍선 사이 간격(결정 0026). 관리 페이지 "캐릭터" 화면에서 고르고 저장소 설정에 둔다.
pub const PACE_KEY: &str = "chat.pace";

/// 말풍선 사이를 얼마나 띄울지. 다음 조각을 사람이 치는 시간처럼, 글이 길수록 조금 더 띄운다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Pace {
    Off,
    Fast,
    Normal,
    Slow,
}

impl Pace {
    pub const ALL: [Pace; 4] = [Pace::Off, Pace::Fast, Pace::Normal, Pace::Slow];

    /// 설정이 없을 때. 시험에서는 기다리지 않는다(실제 시간으로 잠들지 않게).
    pub const DEFAULT: Pace = if cfg!(test) { Pace::Off } else { Pace::Normal };

    pub fn key(self) -> &'static str {
        match self {
            Pace::Off => "off",
            Pace::Fast => "fast",
            Pace::Normal => "normal",
            Pace::Slow => "slow",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Pace::Off => "띄우지 않음(바로 이어 보냄)",
            Pace::Fast => "빠르게",
            Pace::Normal => "보통",
            Pace::Slow => "느긋하게",
        }
    }

    pub fn parse(s: &str) -> Option<Pace> {
        Pace::ALL.into_iter().find(|p| p.key() == s.trim())
    }

    /// (기본 간격, 글자당 간격, 한 번 상한, 답 하나의 합계 상한), 밀리초.
    fn params(self) -> (u64, u64, u64, u64) {
        match self {
            Pace::Off => (0, 0, 0, 0),
            Pace::Fast => (300, 12, 1_500, 5_000),
            Pace::Normal => (600, 25, 3_000, 9_000),
            Pace::Slow => (900, 40, 4_500, 12_000),
        }
    }

    /// 조각 하나를 보내기 전의 목표 간격(앞 조각을 보낸 때부터).
    pub fn gap(self, piece: &str) -> std::time::Duration {
        let (base, per_char, cap, _) = self.params();
        let chars = piece.chars().filter(|c| !c.is_whitespace()).count() as u64;
        std::time::Duration::from_millis((base + per_char * chars).min(cap))
    }
}

/// 답 하나(채팅 한 번, 알림 한 번)의 말풍선 간격을 잰다. 첫 조각은 기다리지 않는다(이미 생각하는 동안
/// 기다렸다). 다음 조각은 앞 조각을 보낸 뒤 흐른 시간(모델이 이어 쓰는 시간)을 빼고 모자란 만큼만 기다린다.
/// 기다리는 동안 작업 큐가 멈추므로 답 하나의 합계에 상한을 둔다.
#[derive(Debug, Clone)]
pub struct Pacer {
    pace: Pace,
    last: Option<tokio::time::Instant>,
    waited: std::time::Duration,
}

impl Pacer {
    pub fn new(pace: Pace) -> Self {
        Self {
            pace,
            last: None,
            waited: std::time::Duration::ZERO,
        }
    }

    /// 지금 보내려는 조각 앞에서 더 기다릴 시간.
    pub fn wait_before(&self, piece: &str, now: tokio::time::Instant) -> std::time::Duration {
        let Some(last) = self.last else {
            return std::time::Duration::ZERO;
        };
        let (_, _, _, total) = self.pace.params();
        let room = std::time::Duration::from_millis(total).saturating_sub(self.waited);
        self.pace
            .gap(piece)
            .saturating_sub(now.saturating_duration_since(last))
            .min(room)
    }

    /// 기다린 시간을 더한다.
    pub fn waited(&mut self, d: std::time::Duration) {
        self.waited += d;
    }

    /// 조각 하나를 보냈다.
    pub fn sent(&mut self, now: tokio::time::Instant) {
        self.last = Some(now);
    }
}

impl Default for Pacer {
    fn default() -> Self {
        Self::new(Pace::DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn pace_gap_grows_with_length_and_is_capped() {
        assert_eq!(
            Pace::Off.gap("아주 긴 글".repeat(50).as_str()),
            Duration::ZERO
        );
        let short = Pace::Normal.gap("네!");
        let mid = Pace::Normal.gap("내일은 오전 수업 하나뿐이에요.");
        let long = Pace::Normal.gap(&"글".repeat(500));
        assert!(short < mid && mid < long, "{short:?} {mid:?} {long:?}");
        assert_eq!(long, Duration::from_millis(3_000), "한 번 상한");
        assert!(Pace::Fast.gap("가나다") < Pace::Slow.gap("가나다"));
        for p in Pace::ALL {
            assert_eq!(Pace::parse(p.key()), Some(p));
        }
        assert_eq!(Pace::parse("warp"), None);
    }

    #[test]
    fn pacer_waits_only_for_what_generation_did_not_cover() {
        let t0 = tokio::time::Instant::now();
        let mut p = Pacer::new(Pace::Normal);
        assert_eq!(
            p.wait_before("첫 조각", t0),
            Duration::ZERO,
            "첫 조각은 바로"
        );
        p.sent(t0);
        let gap = Pace::Normal.gap("둘째 조각이에요");
        assert_eq!(p.wait_before("둘째 조각이에요", t0), gap);
        assert_eq!(
            p.wait_before("둘째 조각이에요", t0 + Duration::from_millis(500)),
            gap - Duration::from_millis(500),
            "모델이 쓰는 동안 흐른 시간은 뺀다"
        );
        assert_eq!(
            p.wait_before("둘째 조각이에요", t0 + Duration::from_secs(10)),
            Duration::ZERO
        );
        // 합계 상한: 다 쓰면 더 기다리지 않는다.
        p.waited(Duration::from_millis(8_800));
        assert_eq!(
            p.wait_before(&"글".repeat(200), t0),
            Duration::from_millis(200)
        );
        p.waited(Duration::from_millis(200));
        assert_eq!(p.wait_before(&"글".repeat(200), t0), Duration::ZERO);
    }

    #[test]
    fn split_handles_edges() {
        assert_eq!(split("하나\n%%\n둘"), ["하나", "둘"]);
        assert_eq!(
            split("%%\n하나\n%%\n\n%%\n둘\n%%"),
            ["하나", "둘"],
            "앞뒤·빈 조각"
        );
        assert_eq!(split("하나\n  %% \n둘"), ["하나", "둘"]);
        assert_eq!(
            split("100%% 확실\n50 %%"),
            ["100%% 확실\n50 %%"],
            "줄 전체가 아니면 구분자가 아니다"
        );
        assert_eq!(split("  "), Vec::<String>::new());
        assert_eq!(split("줄1\n\n줄2"), ["줄1\n\n줄2"], "빈 줄은 나누지 않는다");
    }

    #[test]
    fn streamer_waits_for_a_complete_separator_line() {
        let mut s = Streamer::default();
        assert!(s.feed("안녕하세요").is_empty());
        assert!(
            s.feed("안녕하세요\n%").is_empty(),
            "구분자가 아직 안 끝났다"
        );
        assert!(s.feed("안녕하세요\n%%").is_empty(), "줄이 아직 안 끝났다");
        assert_eq!(s.feed("안녕하세요\n%%\n"), ["안녕하세요"]);
        assert!(s.feed("안녕하세요\n%%\n둘째 이야기").is_empty());
        assert_eq!(
            s.feed("안녕하세요\n%%\n둘째 이야기\n%%\n셋째"),
            ["둘째 이야기"]
        );
        assert_eq!(
            s.rest("안녕하세요\n%%\n둘째 이야기\n%%\n셋째 끝"),
            Some("셋째 끝")
        );
        assert_eq!(s.rest("다른 글"), None);
        assert!(s.started());
    }

    /// 한 글자씩 넣어도 결과(먼저 보낸 조각 + 나머지)는 split과 같다.
    #[test]
    fn streaming_equals_split() {
        let text = "%%\n첫 문단\n두 줄\n%%\n\n%%\n둘째 😀\n%%\n셋째\n%%";
        let chars: Vec<char> = text.chars().collect();
        let mut s = Streamer::default();
        let mut got = Vec::new();
        let mut so_far = String::new();
        for c in &chars {
            so_far.push(*c);
            got.extend(s.feed(&so_far));
        }
        got.extend(split(s.rest(text).unwrap()));
        assert_eq!(got, split(text));
    }
}
