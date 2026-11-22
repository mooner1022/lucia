//! 알림을 사람이 있는 기기로(결정 0066·0067). 앱 대화의 프레임(앱에서 한 말의 답, 브리핑·알림·승인 카드)은 앱마다
//! 상자에 넣고(번호는 앱마다 제 것), 울리는 곳은 한 번에 한 곳으로 고른다. 이 모듈은 그 장부다: 어느 알림이 어느 앱의
//! 몇 번인지, 누가 봤는지, 언제 다른 앱(푸시를 받는 휴대폰)으로 넘길지.
//!
//! - **울릴 곳**(게이트웨이의 `plan_ring`): 한 앱의 대화로 가는 것은 그 앱을 먼저(앞에 있거나, 푸시를 받거나, 자리에
//!   있으면). 그 밖에는 앞에 있는 앱(가장 최근에 앞에 온 것) → 사람이 있는(뒤에 있지만 자리를 비우지 않은) 스스로
//!   알리는 앱(푸시 토큰이 없는 데스크톱) → 푸시를 받는 앱 → 닿는 앱 모두. 울리지 않는 앱의 프레임에는
//!   `"alert": false`가 붙는다.
//! - **넘기기**: 글은 [`TEXT_GRACE`], 카드는 [`CARD_GRACE`] 안에 봤다는 알림이 없으면 푸시를 받는 앱으로 넘긴다
//!   (그 앱의 상자에 이미 있는 프레임을 `alert{seq}`와 푸시로 알린다). 때를 기다려 넘기는 것은 울린 앱이
//!   `seen`을 말하거나(`seen`이나 `away`를 한 번이라도 보냈으면) 푸시가 없는 앱(데스크톱)일 때다(결정 0067). 푸시를
//!   받는 앱(예전 휴대폰 앱)이 말하지 않으면 봤는지 알 길이 없어 두 번 울리게 되므로 넘기지 않는다. 푸시가 없는 앱이
//!   말하지 않으면 늦게 한 번 더 울릴 수 있지만, 승인 카드가 아무 데도 닿지 않고 만료되는 것보다 낫다. 울린 앱이 자리를 비우거나(`away`) 끊기면 바로 넘긴다(게이트웨이가 그 앱이 스스로 알리는
//!   앱이거나 `seen`을 말할 때만 부른다).
//! - **봤음**: 한 앱이 본 알림은 같은 알림을 가진 다른 앱에 `seen`으로 알린다(끊긴 앱에는 알림을 지우는 푸시).
//!   번호 하나로 알리므로, 그 앱에 아직 보지 않은 앞의 것이 있으면 그 앞까지만 알린다.
//! - 장부는 메모리에만 둔다. 데몬이 다시 시작하면 기다리던 넘기기는 사라진다(상자의 프레임은 남는다).

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::time::Duration;

// 넘기기 때는 tokio 시계로 잰다(시험에서 시간을 멈출 수 있게).
use tokio::time::Instant;

/// 글을 넘기기 전에 기다리는 시간 [제안].
pub(super) const TEXT_GRACE: Duration = Duration::from_secs(120);
/// 카드를 넘기기 전에 기다리는 시간 [제안].
pub(super) const CARD_GRACE: Duration = Duration::from_secs(20);
/// 장부에 남기는 알림 수(오래된 것부터 버린다).
const ITEMS_MAX: usize = 500;
/// 앱마다 기억하는 보지 않은 번호 수.
const UNSEEN_MAX: usize = 1000;

/// 여러 앱에 둔(또는 넘길 수 있는) 알림 하나.
#[derive(Debug, Clone)]
pub(super) struct Item {
    pub id: u64,
    /// 프레임 종류(`text`, `files`, `card`, `sticker`).
    pub kind: String,
    /// 푸시 미리보기 후보.
    pub preview: String,
    /// 앱마다 이 알림의 번호.
    pub seqs: HashMap<String, i64>,
    /// 처음 울린 앱.
    pub rung: Vec<String>,
    /// 울린 앱 가운데 푸시가 없는 앱(스스로 알리는 데스크톱)이 있다. 그런 앱은 `seen`을 말하지 않아도 때가 되면
    /// 넘긴다(결정 0067): 늦게 한 번 더 울리는 것이 승인 카드가 그대로 만료되는 것보다 낫다.
    pub rung_unpushed: bool,
    /// 넘길 앱(푸시를 받는 앱). 비었으면 넘기지 않는다.
    pub forward_to: Vec<String>,
    /// 넘길 때. `None`이면 때를 기다리지 않는다(자리를 뜰 때만).
    pub due: Option<Instant>,
    pub seen: bool,
    pub forwarded: bool,
}

#[derive(Default)]
pub(super) struct Alerts {
    items: VecDeque<Item>,
    next: u64,
    /// 앱마다 아직 보지 않은 알림 번호.
    unseen: HashMap<String, BTreeSet<i64>>,
    /// 앱마다 다른 곳에서 본 알림의 번호(아직 알리지 못한 것).
    elsewhere: HashMap<String, BTreeSet<i64>>,
    /// 앱마다 마지막으로 알린 `seen` 번호(그 앱이 스스로 알린 번호를 포함한다).
    told: HashMap<String, i64>,
    /// `seen`을 한 번이라도 보낸 앱(그 앱이 울린 것만 넘긴다).
    speaks: HashSet<String>,
}

impl Alerts {
    /// 알림을 장부에 넣는다. 넣은 ID.
    pub fn add(&mut self, mut item: Item) -> u64 {
        self.next += 1;
        item.id = self.next;
        self.items.push_back(item);
        while self.items.len() > ITEMS_MAX {
            self.items.pop_front();
        }
        self.next
    }

    /// 앱에 울릴 수 있는 프레임을 넣었다(아직 보지 않았다).
    pub fn delivered(&mut self, client: &str, seq: i64) {
        let set = self.unseen.entry(client.to_owned()).or_default();
        set.insert(seq);
        while set.len() > UNSEEN_MAX {
            set.pop_first();
        }
    }

    pub fn mark_speaks(&mut self, client: &str) {
        self.speaks.insert(client.to_owned());
    }

    pub fn speaks(&self, client: &str) -> bool {
        self.speaks.contains(client)
    }

    /// `client`가 `seq`까지 봤다. 알릴 `(앱, 번호)`를 돌려준다: 같은 알림을 가진 다른 앱, 그리고 앞의 것을 봐서
    /// 다른 곳에서 본 알림까지 알릴 수 있게 된 `client` 자신.
    pub fn seen(&mut self, client: &str, seq: i64) -> Vec<(String, i64)> {
        if let Some(set) = self.unseen.get_mut(client) {
            *set = set.split_off(&(seq + 1));
        }
        if let Some(set) = self.elsewhere.get_mut(client) {
            *set = set.split_off(&(seq + 1));
        }
        let told = self.told.entry(client.to_owned()).or_insert(0);
        *told = (*told).max(seq);
        for item in self.items.iter_mut() {
            if item.seen || item.seqs.get(client).is_none_or(|s| *s > seq) {
                continue;
            }
            item.seen = true;
            for (other, s) in &item.seqs {
                if other == client {
                    continue;
                }
                if let Some(set) = self.unseen.get_mut(other) {
                    set.remove(s);
                }
                self.elsewhere.entry(other.clone()).or_default().insert(*s);
            }
        }
        // 번호 하나로 알린다: 다른 곳에서 본 알림 가운데, 그 앱에 보지 않은 것이 앞에 없는 가장 큰 번호까지.
        let mut tells = Vec::new();
        for (app, pending) in self.elsewhere.iter_mut() {
            let first_unseen = self.unseen.get(app).and_then(|s| s.first()).copied();
            let best = match first_unseen {
                Some(f) => pending.range(..f).next_back(),
                None => pending.last(),
            }
            .copied();
            let Some(best) = best else { continue };
            *pending = pending.split_off(&(best + 1));
            let told = self.told.entry(app.clone()).or_insert(0);
            if best > *told {
                *told = best;
                tells.push((app.clone(), best));
            }
        }
        tells.sort();
        tells
    }

    /// 지금 넘길 알림을 꺼낸다(넘긴 것으로 표시한다): 때가 됐고 울린 앱이 `seen`을 말하거나, `left`(자리를 뜬 앱)가
    /// 울린 것.
    pub fn take_due(&mut self, now: Instant, left: Option<&str>) -> Vec<Item> {
        let mut out = Vec::new();
        let speaks = &self.speaks;
        for item in self.items.iter_mut() {
            if item.seen || item.forwarded || item.forward_to.is_empty() {
                continue;
            }
            let timed = item.due.is_some_and(|d| d <= now)
                && (item.rung_unpushed || item.rung.iter().any(|r| speaks.contains(r)));
            let gone = left.is_some_and(|l| item.rung.iter().any(|r| r == l));
            if timed || gone {
                item.forwarded = true;
                out.push(item.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(seqs: &[(&str, i64)], rung: &[&str], forward_to: &[&str]) -> Item {
        Item {
            id: 0,
            kind: "text".into(),
            preview: String::new(),
            seqs: seqs.iter().map(|(c, s)| ((*c).to_owned(), *s)).collect(),
            rung: rung.iter().map(|c| (*c).to_owned()).collect(),
            rung_unpushed: false,
            forward_to: forward_to.iter().map(|c| (*c).to_owned()).collect(),
            due: Some(Instant::now() + TEXT_GRACE),
            seen: false,
            forwarded: false,
        }
    }

    #[test]
    fn seen_on_one_app_is_told_to_the_others_only_up_to_their_first_unseen() {
        let mut a = Alerts::default();
        // 휴대폰에는 제 대화의 보지 않은 글(3번)이 먼저 있다.
        a.delivered("phone", 3);
        for (desk, phone) in [(10, 4), (11, 5)] {
            a.delivered("desk", desk);
            a.delivered("phone", phone);
            a.add(item(
                &[("desk", desk), ("phone", phone)],
                &["desk"],
                &["phone"],
            ));
        }
        // 데스크톱에서 다 봤다: 휴대폰에는 보지 않은 3번이 앞에 있어 아직 알리지 않는다.
        assert!(a.seen("desk", 11).is_empty());
        // 휴대폰이 제 3번을 봤다: 이제 데스크톱에서 본 5번까지 휴대폰에 알린다.
        assert_eq!(a.seen("phone", 3), vec![("phone".to_owned(), 5)]);
        // 그 뒤 새 알림을 휴대폰에서 보면 데스크톱에 그 번호까지 알린다.
        a.delivered("desk", 12);
        a.delivered("phone", 6);
        a.add(item(&[("desk", 12), ("phone", 6)], &["desk"], &["phone"]));
        assert_eq!(a.seen("phone", 6), vec![("desk".to_owned(), 12)]);
        // 같은 것을 다시 봐도 두 번 알리지 않는다.
        assert!(a.seen("phone", 6).is_empty());
        // 본 것은 넘기지 않는다.
        assert!(a.take_due(Instant::now() + TEXT_GRACE * 2, None).is_empty());
    }

    #[test]
    fn due_items_and_items_rung_by_a_leaving_app_are_taken_once() {
        let mut a = Alerts::default();
        // 푸시를 받는 앱이 울린 것을 때를 기다려 넘기는 것은 울린 앱이 `seen`을 말할 때만.
        let mut quiet = item(&[("old", 9)], &["old"], &["phone"]);
        quiet.due = Some(Instant::now());
        a.add(quiet);
        assert!(a.take_due(Instant::now(), None).is_empty());
        assert_eq!(
            a.take_due(Instant::now(), Some("old")).len(),
            1,
            "자리를 뜨면 넘긴다"
        );
        // 푸시가 없는 앱(데스크톱)이 울린 것은 말하지 않아도 때가 되면 넘긴다(결정 0067).
        let mut desk = item(&[("quiet-desk", 4)], &["quiet-desk"], &["phone"]);
        desk.rung_unpushed = true;
        desk.due = Some(Instant::now() + CARD_GRACE);
        a.add(desk);
        assert!(a.take_due(Instant::now(), None).is_empty(), "때가 되기 전");
        assert_eq!(
            a.take_due(Instant::now() + CARD_GRACE, None).len(),
            1,
            "말하지 않는 데스크톱이 울린 것도 넘긴다"
        );
        a.mark_speaks("desk");
        let mut early = item(&[("desk", 1)], &["desk"], &["phone"]);
        early.due = Some(Instant::now() + CARD_GRACE);
        a.add(early);
        let mut later = item(&[("desk", 2)], &["desk"], &["phone"]);
        later.due = Some(Instant::now() + TEXT_GRACE);
        a.add(later);
        a.add(item(&[("desk", 3)], &["desk"], &[]));
        assert!(a.take_due(Instant::now(), None).is_empty());
        let due = a.take_due(Instant::now() + CARD_GRACE + Duration::from_secs(1), None);
        assert_eq!(due.len(), 1);
        // 자리를 뜨면 기다리던 것도 바로. 넘길 곳이 없는 것은 넘기지 않는다.
        let gone = a.take_due(Instant::now(), Some("desk"));
        assert_eq!(gone.iter().map(|i| i.seqs["desk"]).collect::<Vec<_>>(), [2]);
        assert!(
            a.take_due(Instant::now(), Some("desk")).is_empty(),
            "한 번만"
        );
    }
}
