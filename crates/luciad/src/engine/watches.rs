//! 감시 확인(결정 0019·0036): 작업 큐 밖 전용 타이머가 1분마다 부른다. 모델을 부르지 않는다.
//!
//! - 구간 감시(region): 구간이 바뀌면 변화와 소식, 세 번 이어 바뀌면 잡음으로 멈춤(0019 그대로).
//! - 게시판 감시(items): 새 글을 키워드로 거르고, 알림 방식대로 보낸다.
//!   - `now`: 모델 없이 바로(방해 금지 시간이거나 하루 상한을 넘으면 소식).
//!   - `classify`: 신호 처리(결정 0031)로.
//!   - `digest`: 소식으로만.
//!   - 이 감시로 도는 `on_change` 루틴이 있으면 감시는 알리지 않는다(루틴의 스크립트가 정한다, 같은 글을 두 번 알리지 않게).
//! - 어느 방식이든 변화는 사전 체크의 "바뀐 것"으로 남아 브리핑과 루틴이 받는다.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use lucia_precheck::Change;
use lucia_store::{NewSourceChange, Watch, WatchStatus, watch_notify as notify};
use serde_json::json;

use super::{AgentRunner, ChannelAdapter, Engine};
use crate::board;
use crate::watch::{self as rules, Fetch, FetchError, Via};

/// 감시 방식 바꾸기 카드(결정 0036): HTTP가 막힌 감시를 브라우저로.
pub const KIND_WATCH_SWITCH: &str = "watch_switch";
/// 로그인으로 멈춘 감시를 (브라우저가 떠 있을 때) 다시 해 보는 간격(분).
const RECOVER_MINUTES: i64 = 10;

/// 그 방식으로 볼 것이 있는가(게시판이면 글, 구간이면 구간 글).
pub fn has_content(w: &Watch, html: &str) -> bool {
    if w.is_items() {
        url::Url::parse(&w.url)
            .ok()
            .and_then(|base| {
                let p = board::LinkPattern::parse(&w.item_pattern, &base).ok()?;
                Some(!board::items(html, &base, &p).is_empty())
            })
            .unwrap_or(false)
    } else {
        !rules::extract(html, &w.start_text, &w.end_text).is_empty()
    }
}

/// 한 메시지에 싣는 새 글 수.
const LINES_PER_MESSAGE: usize = 10;

/// 확인 한 번의 결과(시험·기록용).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchCheck {
    Baseline,
    Same,
    /// 남긴 변화 수(게시판이면 거르기를 지난 새 글 수).
    Changed(usize),
    /// 한꺼번에 너무 많이 바뀌어 기준선만 다시 잡았다.
    Rebaselined,
    Failed(String),
}

impl<R: AgentRunner, C: ChannelAdapter> Engine<R, C> {
    pub fn with_watch_fetch(mut self, fetch: Option<Arc<dyn Fetch>>) -> Self {
        self.watch_fetch = fetch;
        self
    }

    pub fn has_watch_fetch(&self) -> bool {
        self.watch_fetch.is_some()
    }

    /// 확인할 때가 된 감시를 하나씩 확인한다. 같은 사이트는 [`rules::HOST_GAP_MINUTES`] 안에 다시 가지 않는다
    /// (`hosts`: 사이트마다 마지막으로 간 때, 부르는 쪽이 들고 있다). 새로 남긴 변화 수를 돌려준다.
    pub async fn run_watch_round(&self, hosts: &mut HashMap<String, DateTime<Utc>>) -> usize {
        let Some(fetch) = self.watch_fetch.clone() else {
            return 0;
        };
        let now = self.clock.now();
        let all = self.store().watches().unwrap_or_default();
        // 브라우저 방식 감시가 하나라도 켜져 있으면 브라우저를 계속 켜 둔다(결정 0036).
        fetch
            .keep_browser(all.iter().any(|w| {
                w.status == WatchStatus::Active && Via::parse(&w.fetcher) == Via::Browser
            }));
        // 가장 오래 확인하지 않은 것부터(같은 사이트의 감시들이 사이트 간격을 나눠 쓰고, 하나가 굶지 않게).
        let mut due: Vec<(Option<DateTime<Utc>>, Watch)> = {
            let store = self.store();
            all.into_iter()
                .filter_map(|w| {
                    let checked = store
                        .source_state(&rules::source_name(&w.id))
                        .ok()
                        .flatten()
                        .and_then(|s| s.checked_at);
                    let ready = match (w.status, w.paused_reason.as_str()) {
                        (WatchStatus::Active, _) => rules::due(&w, checked, now),
                        // 로그인으로 멈춘 감시: 브라우저가 이미 떠 있을 때만(사람이 막 로그인했을 수 있다) 가끔 다시 해 본다.
                        (WatchStatus::Paused, lucia_store::watch_paused::LOGIN) => {
                            fetch.browser_running()
                                && checked.is_none_or(|c| {
                                    now - c >= chrono::Duration::minutes(RECOVER_MINUTES)
                                })
                        }
                        _ => false,
                    };
                    ready.then_some((checked, w))
                })
                .collect()
        };
        due.sort_by_key(|(checked, _)| *checked);
        let gap = chrono::Duration::minutes(rules::HOST_GAP_MINUTES);
        let mut added = 0;
        for (_, w) in due {
            let host = rules::host_of(&w.url);
            if hosts.get(&host).is_some_and(|t| now - *t < gap) {
                continue;
            }
            hosts.insert(host, now);
            let result = if w.status == WatchStatus::Paused {
                self.recover_watch(&w, fetch.as_ref()).await
            } else {
                self.check_watch(&w, fetch.as_ref()).await
            };
            match result {
                WatchCheck::Changed(n) => added += n,
                WatchCheck::Failed(why) => tracing::info!(id = %w.id, %why, "감시 확인 실패"),
                _ => {}
            }
        }
        added
    }

    /// 로그인으로 멈춘 감시를 다시 해 본다. 읽히면 감시를 켜고(기준선은 그대로라 그 사이 새 글을 알린다) 확인한다.
    async fn recover_watch(&self, w: &Watch, fetch: &dyn Fetch) -> WatchCheck {
        let via = Via::parse(&w.fetcher);
        let name = rules::source_name(&w.id);
        let now = self.clock.now();
        match self.read_page(w, fetch, via, true).await {
            Ok(html) if !has_content(w, &html) => {
                let why = "아직 볼 것이 없다(로그인 전일 수 있다)".to_owned();
                let _ = self.store().record_source_error(&name, &why, now);
                WatchCheck::Failed(why)
            }
            Ok(_) => {
                let _ = self
                    .store()
                    .set_watch_status(&w.id, WatchStatus::Active, now);
                let _ = self.store().add_activity(
                    "watch_resumed",
                    &format!("감시 {}: 로그인이 돌아와 다시 켬", w.id),
                    None,
                    now,
                );
                let mut active = w.clone();
                active.status = WatchStatus::Active;
                active.paused_reason.clear();
                active.failures = 0;
                self.check_watch(&active, fetch).await
            }
            Err(e) => {
                let _ = self.store().record_source_error(&name, &e.to_string(), now);
                WatchCheck::Failed(e.to_string())
            }
        }
    }

    /// 정한 방식으로 페이지를 읽는다. 세션·브라우저 방식에서 로그인이 풀렸으면 세션을 다시 받고 한 번 더 해 본다.
    ///
    /// - 로그인 풀림: 로그인 주소로 넘겨줌·401, 또는 볼 것이 없고 비밀번호 칸이 보임. 다시 받아도 그러면 로그인 멈춤.
    /// - 볼 것만 없음(비밀번호 칸 없이 알림창·스크립트 이동만 주는 로그아웃 화면 등): 이어진 실패가 없을 때
    ///   (`retry_empty`) 한 번 다시 받아 해 본다. 그래도 비면 일반 실패(물러서기·5번이면 알림)로 둔다. 개편과
    ///   구별할 수 없고, 실패마다 다시 받으면 브라우저 VM을 계속 깨우게 된다.
    async fn read_page(
        &self,
        w: &Watch,
        fetch: &dyn Fetch,
        via: Via,
        retry_empty: bool,
    ) -> Result<String, FetchError> {
        let logged_out = |r: &Result<String, FetchError>| match r {
            Err(FetchError::Login(_)) => true,
            Ok(html) => !has_content(w, html) && rules::looks_logged_out(html),
            _ => false,
        };
        let empty = |r: &Result<String, FetchError>| matches!(r, Ok(html) if !has_content(w, html));
        let first = fetch.fetch_via(&w.url, via).await;
        if via == Via::Plain {
            return first;
        }
        let out = logged_out(&first);
        if !(out || (retry_empty && empty(&first))) {
            return first;
        }
        match fetch.refresh(&w.url).await {
            Ok(()) => {}
            // 로그인 화면이었으면 로그인 문제, 비기만 했으면 처음 결과(일반 실패)로.
            Err(e) if out => return Err(e),
            Err(_) => return first,
        }
        let second = fetch.fetch_via(&w.url, via).await;
        if logged_out(&second) {
            return Err(FetchError::Login(
                "세션을 다시 받아도 로그인 화면이다".into(),
            ));
        }
        second
    }

    /// 감시 하나를 확인한다.
    pub async fn check_watch(&self, w: &Watch, fetch: &dyn Fetch) -> WatchCheck {
        let name = rules::source_name(&w.id);
        let now = self.clock.now();
        let via = Via::parse(&w.fetcher);
        let cursor = self
            .store()
            .source_state(&name)
            .ok()
            .flatten()
            .and_then(|s| s.cursor);
        // 로그인 세션·브라우저로 읽는 것은 위임한 사이트만(위임을 거두면 실패로 물러서고, 이어지면 알린다).
        let page = if via != Via::Plain && !self.watch_granted(&w.url) {
            Err(FetchError::Other(
                "사이트 위임이 없어 로그인 세션으로 읽지 않는다(관리 페이지 \"사이트 위임\")"
                    .into(),
            ))
        } else {
            self.read_page(w, fetch, via, cursor.is_some() && w.failures == 0)
                .await
        };
        let result = match page {
            Ok(html) if w.is_items() => self.check_board(w, &name, cursor.as_deref(), &html).await,
            Ok(html) => self.check_section(w, &name, cursor.as_deref(), &html),
            // 브라우저가 다른 일로 바쁘면 이번 회차만 건너뛴다(실패로 세지 않고, 다음 바퀴에 다시).
            Err(FetchError::Busy) => return WatchCheck::Same,
            Err(FetchError::Login(why)) if via != Via::Plain => {
                self.pause_for_login(w, &why).await;
                return WatchCheck::Failed(why);
            }
            Err(FetchError::Blocked(why)) => {
                if via != Via::Browser {
                    self.offer_browser(w, fetch).await;
                }
                Err(format!("사이트가 막았다: {why}"))
            }
            Err(e) => Err(e.to_string()),
        };
        match result {
            Ok(outcome) => {
                let _ = self.store().note_watch_fetch(&w.id, true);
                outcome
            }
            Err(why) => {
                let store = self.store();
                let _ = store.record_source_error(&name, &why, now);
                let failures = store.note_watch_fetch(&w.id, false).unwrap_or(0);
                if failures == rules::FAIL_NOTICE_AT {
                    let _ = store.add_notice(
                        "watch",
                        None,
                        &crate::mask::global().mask(&format!(
                            "\"{}\" 감시가 {failures}번 이어 실패했습니다: {why}. 간격을 늘려 가며 다시 해 봅니다(관리 페이지에서 확인).",
                            w.what
                        )),
                        now,
                        now,
                    );
                }
                WatchCheck::Failed(why)
            }
        }
    }

    fn watch_granted(&self, url: &str) -> bool {
        let host = rules::host_of(url);
        let store = self.store();
        crate::delegation::grant_for(&store, &host, self.clock.now()).is_some()
    }

    /// 로그인이 풀려 다시 들어가지 못했다: 감시를 멈추고 한 번 알린다. 브라우저가 떠 있을 때 가끔 다시 해 보고,
    /// 읽히면 스스로 다시 켠다.
    async fn pause_for_login(&self, w: &Watch, why: &str) {
        let now = self.clock.now();
        let name = rules::source_name(&w.id);
        let paused = {
            let store = self.store();
            let _ = store.record_source_error(&name, why, now);
            store
                .pause_watch(&w.id, lucia_store::watch_paused::LOGIN, now)
                .unwrap_or(false)
        };
        if !paused {
            return;
        }
        let host = rules::host_of(&w.url);
        let text = crate::mask::global().mask(&format!(
            "🔒 \"{}\" 감시를 멈췄습니다: {host} 로그인이 풀렸습니다({why}). Lucia에게 \"{host} 로그인해 줘\"라고 하거나 관리 페이지 \"사이트 위임\"에서 로그인을 허용해 주세요. 로그인되면 스스로 다시 켭니다.",
            w.what
        ));
        self.tell(text, now).await;
    }

    /// HTTP가 막힌 감시: 브라우저로 한 번 읽어 보고, 되면 브라우저로 바꿀지 카드로 묻는다(사람 없이 바꾸지 않는다,
    /// 브라우저를 계속 켜 두는 비용이 있다). 같은 감시에는 일주일에 한 장.
    async fn offer_browser(&self, w: &Watch, fetch: &dyn Fetch) {
        let Some(posts) = &self.approvals else {
            return;
        };
        let now = self.clock.now();
        let asked = self
            .store()
            .recent_approvals(300)
            .unwrap_or_default()
            .iter()
            .any(|a| {
                a.kind == KIND_WATCH_SWITCH
                    && a.payload["id"] == w.id.as_str()
                    && now - a.requested_at < chrono::Duration::days(7)
            });
        if asked || !self.watch_granted(&w.url) {
            return;
        }
        let Ok(html) = fetch.fetch_via(&w.url, Via::Browser).await else {
            return;
        };
        if !has_content(w, &html) {
            return;
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let created = self.store().create_approval(
            &lucia_store::NewApproval {
                id: id.clone(),
                kind: KIND_WATCH_SWITCH.into(),
                payload: json!({
                    "id": w.id,
                    "what": w.what,
                    "url": w.url,
                    "from": w.fetcher,
                }),
                summary: format!("감시 방식 바꾸기: {}", w.what),
                run_id: None,
                expires_at: now + chrono::Duration::days(3),
            },
            now,
        );
        if created.is_ok() {
            let _ = posts.send(id);
        }
    }

    /// 사람에게 알린다: 방해 금지 시간이거나 메신저가 없으면 소식으로.
    async fn tell(&self, text: String, now: DateTime<Utc>) {
        let tz = self.cfg.timezone;
        let quiet =
            crate::autonomy::quiet_hours(&self.store()).is_some_and(|q| q.contains(now, tz));
        let sent = match (quiet, self.channel.ready(), self.notify_target()) {
            (false, true, Some(target)) => self.send(&target, text.clone()).await.is_ok(),
            _ => false,
        };
        if !sent {
            let _ = self.store().add_notice("watch", None, &text, now, now);
        }
    }

    fn check_section(
        &self,
        w: &Watch,
        name: &str,
        cursor: Option<&str>,
        html: &str,
    ) -> Result<WatchCheck, String> {
        let text = rules::extract(html, &w.start_text, &w.end_text);
        if text.is_empty() {
            return Err("보려던 구간이 비었다(표지 글을 찾지 못했다)".into());
        }
        let hash = rules::digest(&text);
        let now = self.clock.now();
        let changed = cursor.is_some_and(|old| old != hash);
        let changes: Vec<NewSourceChange> = if changed {
            vec![NewSourceChange {
                source: name.to_owned(),
                change_key: format!("{}:{hash}", w.id),
                summary: format!("감시 중인 \"{}\"이(가) 바뀌었다", w.what),
                payload: json!({
                    "url": w.url,
                    "what": w.what,
                    "excerpt": text.chars().take(600).collect::<String>(),
                }),
            }]
        } else {
            Vec::new()
        };
        let store_result = (|| -> lucia_store::Result<usize> {
            let mut store = self.store();
            let added = store.commit_source_check(name, Some(&hash), &changes, now)?;
            if cursor.is_none() {
                return Ok(added);
            }
            let paused = store.note_watch_check(&w.id, changed, rules::PAUSE_AFTER, now)?;
            if changed {
                store.add_notice(
                    "watch",
                    None,
                    &format!(
                        "감시 중인 \"{}\"이(가) 바뀌었습니다(다음 브리핑에서 자세히).",
                        w.what
                    ),
                    now,
                    now,
                )?;
            }
            if paused {
                store.add_notice(
                    "watch",
                    None,
                    &format!(
                        "\"{}\" 감시를 멈췄습니다: 확인할 때마다 바뀌어 잡음으로 봤습니다. 관리 페이지에서 보는 구간을 좁히거나 다시 켜 주세요.",
                        w.what
                    ),
                    now,
                    now,
                )?;
            }
            Ok(added)
        })();
        store_result.map_err(|e| e.to_string())?;
        if changed {
            self.queue_change_fires(name, &changes);
        }
        Ok(match (cursor, changed) {
            (None, _) => WatchCheck::Baseline,
            (_, true) => WatchCheck::Changed(changes.len()),
            _ => WatchCheck::Same,
        })
    }

    async fn check_board(
        &self,
        w: &Watch,
        name: &str,
        cursor: Option<&str>,
        html: &str,
    ) -> Result<WatchCheck, String> {
        let base = url::Url::parse(&w.url).map_err(|e| e.to_string())?;
        let pattern = board::LinkPattern::parse(&w.item_pattern, &base)?;
        let found = board::items(html, &base, &pattern);
        if found.is_empty() {
            return Err(
                "글 목록이 비었다(링크 패턴이 맞지 않거나, 사이트가 바뀌었거나, 로그인이 풀렸다)"
                    .into(),
            );
        }
        let before = board::Seen::parse(cursor);
        let diff = board::diff(&found, before.as_ref());
        let now = self.clock.now();
        let include = board::words(&w.include_words);
        let exclude = board::words(&w.exclude_words);
        let hits: Vec<&board::Item> = diff
            .new
            .iter()
            .filter(|i| board::keep(&i.title, &include, &exclude))
            .collect();
        let mask = crate::mask::global();
        // 로그인해야 보이는 글(결정 0037)은 비공개 자료: 출구 검사 대조 자료에 넣는다(결정 0029).
        if Via::parse(&w.fetcher) != Via::Plain {
            for i in &hits {
                crate::leak::record(&i.title);
            }
        }
        let mut changes: Vec<NewSourceChange> = hits
            .iter()
            .map(|i| NewSourceChange {
                source: name.to_owned(),
                change_key: format!("{}:{}", w.id, rules::digest(&i.key)),
                summary: mask.mask(&format!("[{}] 새 글: {}", w.what, i.title)),
                payload: json!({
                    "kind": "item",
                    "watch": w.id,
                    "what": w.what,
                    "title": mask.mask(&i.title),
                    "url": i.url,
                    "key": i.key,
                }),
            })
            .collect();
        if diff.burst {
            let _ = self.store().add_notice(
                "watch",
                None,
                &format!(
                    "\"{}\": 새 글이 한꺼번에 {}개 넘게 보여 알리지 않고 기준을 다시 잡았습니다(목록 모양이 바뀌었을 수 있습니다).",
                    w.what,
                    board::BURST_MAX
                ),
                now,
                now,
            );
        }
        if !changes.is_empty() {
            let has_routine = self
                .store()
                .routines_on_change(name)
                .is_ok_and(|r| !r.is_empty());
            if !has_routine {
                match w.notify.as_str() {
                    notify::NOW => {
                        self.send_items(w, &hits, now).await;
                        for c in &mut changes {
                            c.summary.push_str(" (이미 알림 보냄)");
                        }
                    }
                    // 신호 처리가 꺼져 있으면 사라지지 않게 소식으로.
                    notify::CLASSIFY if crate::signals::enabled(&self.store()) => {
                        let as_signals: Vec<Change> = changes
                            .iter()
                            .map(|c| Change {
                                key: c.change_key.clone(),
                                summary: c.summary.clone(),
                                payload: c.payload.clone(),
                            })
                            .collect();
                        self.record_signals(name, &as_signals);
                    }
                    _ => {
                        let _ = self.store().add_notice(
                            "watch",
                            None,
                            &items_text(w, &hits, false),
                            now,
                            now,
                        );
                    }
                }
            }
        }
        self.store()
            .commit_source_check(name, Some(&diff.cursor.to_cursor()), &changes, now)
            .map_err(|e| e.to_string())?;
        if !changes.is_empty() {
            self.queue_change_fires(name, &changes);
        }
        Ok(if diff.burst {
            WatchCheck::Rebaselined
        } else if diff.baseline {
            WatchCheck::Baseline
        } else if changes.is_empty() {
            WatchCheck::Same
        } else {
            WatchCheck::Changed(changes.len())
        })
    }

    /// 새 글을 모델 없이 바로 보낸다. 방해 금지 시간이거나 메신저가 없거나 하루 상한을 넘은 것은 소식으로.
    async fn send_items(&self, w: &Watch, hits: &[&board::Item], now: DateTime<Utc>) {
        let tz = self.cfg.timezone;
        let (quiet, take) = {
            let store = self.store();
            let quiet = crate::autonomy::quiet_hours(&store).is_some_and(|q| q.contains(now, tz));
            let today = now.with_timezone(&tz).format("%Y-%m-%d").to_string();
            let cap = rules::now_daily(&store);
            let take = store
                .take_watch_sends(&w.id, &today, hits.len() as i64, cap)
                .unwrap_or(0) as usize;
            (quiet, take)
        };
        let (direct, over) = hits.split_at(take.min(hits.len()));
        if !direct.is_empty() {
            let body = items_text(w, direct, true);
            let sent = match (quiet, self.channel.ready(), self.notify_target()) {
                (false, true, Some(target)) => self.send(&target, body.clone()).await.is_ok(),
                _ => false,
            };
            if !sent {
                let _ = self.store().add_notice("watch", None, &body, now, now);
            }
        }
        if !over.is_empty() {
            let _ = self.store().add_notice(
                "watch",
                None,
                &format!(
                    "{}\n(오늘 바로 알림 상한을 넘어 소식으로 모았습니다. 관리 페이지 \"페이지 감시\"에서 상한을 바꿀 수 있습니다.)",
                    items_text(w, over, false)
                ),
                now,
                now,
            );
        }
    }
}

/// 새 글 알림 글. 제목은 가린 뒤 싣는다(주소는 채널이 미리보기 없게 감싼다).
fn items_text(w: &Watch, hits: &[&board::Item], now: bool) -> String {
    let mask = crate::mask::global();
    let icon = if now { "📌" } else { "🗂" };
    if hits.len() == 1 {
        let i = hits[0];
        return mask.mask(&format!("{icon} {} 새 글: {}\n{}", w.what, i.title, i.url));
    }
    let mut out = format!("{icon} {} 새 글 {}건", w.what, hits.len());
    for i in hits.iter().take(LINES_PER_MESSAGE) {
        out.push_str(&format!("\n- {} {}", i.title, i.url));
    }
    if hits.len() > LINES_PER_MESSAGE {
        out.push_str(&format!("\n… 그 밖에 {}건", hits.len() - LINES_PER_MESSAGE));
    }
    mask.mask(&out)
}
