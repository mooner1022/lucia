//! 되돌릴 수 없는 바깥 동작의 승인(결정 0059): git push, 릴리스·배포·패키지 게시, 바깥으로 보내는 쓰기 요청처럼
//! 데몬이 대신 해 주지 않고 Lucia가 VM 안에서 직접 하는 일을, 하기 전에 사용자에게 카드로 묻는다.
//!
//! - **누가 묻나**: 세 길이 같은 문(이 모듈)으로 온다.
//!   - 상주 VM의 훅(`plugin/hooks/gate.py`): Bash 명령이 바깥에 쓰는 모양이면 실행 전에 데몬에 묻는다.
//!     훅 파일은 읽기 전용 마운트지만 훅이 쓰는 `sh`·`python3`은 VM 안의 것이라, 쉘과 sudo를 가진 Lucia가 마음먹으면
//!     끌 수 있다. 명령의 모양으로 가려내므로 놓치는 것도 있다. 실수를 막는 장치이지 벽이 아니다.
//!   - 브라우저 도구의 스크립트 실행과 파일 올리기.
//!   - 도구 `approval_request`: 훅이 알아볼 수 없는 일(코드 안에서 보내는 요청 등)을 Lucia가 스스로 묻는다.
//! - **데몬은 실행하지 않는다.** 승인은 "해도 된다"는 답일 뿐이고, 실행은 물어본 쪽이 한다. 그래서 승인된 뒤
//!   실행에 실패해도 카드는 "승인했습니다"로 남는다.
//! - **권한 모드와 상관없이 묻는다.** 해제 모드도 이 카드는 건너뛰지 않는다(사용자 지시 2026-11-20).
//! - **늦은 승인**: 기다리던 쪽이 떠난 뒤 승인되면 [`GRANT_TTL`] 동안 기억해 둔다. 같은 동작을 다시 청하면 묻지
//!   않고 한 번 통과시킨다(그래서 Lucia는 "승인되면 다시 실행"하면 된다).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use lucia_core::ConversationId;
use lucia_store::{Approval, Store};
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc;

use crate::approvals::{ApprovalKind, Executed, KindCtx};
use crate::approvals::{CardState, clip_raw, escape_markdown, open_or_closed};
use crate::browser::BoxFut;

pub const KIND_EXTERNAL_ACTION: &str = "external_action";
/// 카드가 살아 있는 시간 [제안].
pub const CARD_MINUTES: i64 = 30;
/// 승인된 뒤 같은 동작을 다시 청하면 묻지 않고 한 번 통과시키는 시간 [제안].
pub const GRANT_TTL: Duration = Duration::from_secs(30 * 60);
/// 카드에 싣는 내용 상한(문자).
const DETAIL_MAX: usize = 1500;
const WHAT_MAX: usize = 120;

/// 요청 하나의 지금 상태.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Pending,
    Approved,
    /// 거절됐거나 만료됐다(까닭).
    Denied(String),
}

struct Entry {
    digest: String,
    status: Status,
    at: Instant,
    /// 승인을 한 번 써 버렸다(같은 동작을 또 하려면 다시 묻는다).
    used: bool,
}

pub struct ActionGate {
    store: Arc<Mutex<Store>>,
    posts: Option<mpsc::UnboundedSender<String>>,
    entries: Mutex<HashMap<String, Entry>>,
    changed: tokio::sync::Notify,
}

fn digest(detail: &str) -> String {
    Sha256::digest(detail.trim().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl ActionGate {
    pub fn new(
        store: Arc<Mutex<Store>>,
        posts: Option<mpsc::UnboundedSender<String>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            posts,
            entries: Mutex::default(),
            changed: tokio::sync::Notify::new(),
        })
    }

    /// 승인을 청한다. 돌려주는 것은 요청 id다(상태는 [`Self::wait`]로 본다).
    /// 같은 동작이 이미 기다리는 중이면 그 요청을, 방금 승인됐고 아직 쓰지 않았으면 그 승인을 돌려준다.
    pub fn request(
        &self,
        run: &str,
        what: &str,
        detail: &str,
        source: &str,
    ) -> Result<String, String> {
        let detail = detail.trim();
        if detail.is_empty() {
            return Err("무엇을 하려는지(명령이나 내용)가 비었다".into());
        }
        // 승인은 내용 전체에 해당한다. 카드에 다 보이지 않는 것은 승인받지 않는다(긴 스크립트 끝에 숨은 push를
        // 보지 못한 채 승인하게 하지 않는다).
        if detail.chars().count() > DETAIL_MAX {
            return Err(format!(
                "내용이 너무 길어 승인 카드에 다 보일 수 없다({DETAIL_MAX}자까지). 승인이 필요한 부분(push, 게시, 보내는 요청)만 따로 짧은 명령으로 나눠 실행한다"
            ));
        }
        let key = digest(detail);
        {
            let mut entries = self.entries.lock().expect("action entries");
            entries.retain(|_, e| e.at.elapsed() < GRANT_TTL);
            let known = entries.iter().find(|(_, e)| {
                e.digest == key
                    && (e.status == Status::Pending || (e.status == Status::Approved && !e.used))
            });
            if let Some((id, _)) = known {
                return Ok(id.clone());
            }
        }
        let Some(posts) = &self.posts else {
            return Err(
                "이 실행에서는 사용자에게 승인 카드를 올릴 수 없다(승인을 받을 수 없으니 하지 않는다)"
                    .into(),
            );
        };
        let id = uuid::Uuid::new_v4().simple().to_string();
        let now = Utc::now();
        let mask = crate::mask::global();
        let what = clip_raw(&mask.mask(what.trim()), WHAT_MAX);
        {
            let store = self.store.lock().expect("store lock");
            store
                .create_approval(
                    &lucia_store::NewApproval {
                        id: id.clone(),
                        kind: KIND_EXTERNAL_ACTION.into(),
                        payload: serde_json::json!({
                            "what": what,
                            "detail": clip_raw(&mask.mask(detail), DETAIL_MAX),
                            "source": source,
                            "digest": key,
                        }),
                        summary: format!("승인 요청: {what}"),
                        run_id: Some(run.to_owned()),
                        expires_at: now + chrono::Duration::minutes(CARD_MINUTES),
                    },
                    now,
                )
                .map_err(|e| format!("승인 요청을 남기지 못했다: {e}"))?;
            let _ = store.add_activity(
                "action_requested",
                &format!("승인 요청({source}): {what}"),
                Some(run),
                now,
            );
        }
        self.entries.lock().expect("action entries").insert(
            id.clone(),
            Entry {
                digest: key,
                status: Status::Pending,
                at: Instant::now(),
                used: false,
            },
        );
        let _ = posts.send(id.clone());
        Ok(id)
    }

    /// 지금 상태. 승인이면 그 승인을 쓴 것으로 적는다(한 번만 통과한다).
    fn take(&self, id: &str) -> Status {
        let mut entries = self.entries.lock().expect("action entries");
        match entries.get_mut(id) {
            None => Status::Denied("그런 승인 요청이 없다(만료됐거나 데몬이 다시 떴다)".into()),
            Some(e) if e.status == Status::Approved => {
                if e.used {
                    Status::Denied("이 승인은 이미 썼다. 다시 하려면 새로 승인을 받는다".into())
                } else {
                    e.used = true;
                    Status::Approved
                }
            }
            Some(e) => e.status.clone(),
        }
    }

    fn peek(&self, id: &str) -> Option<Status> {
        self.entries
            .lock()
            .expect("action entries")
            .get(id)
            .map(|e| e.status.clone())
    }

    /// 답이 올 때까지 `wait`만큼 기다린다. 그때까지 답이 없으면 `Pending`.
    pub async fn wait(&self, id: &str, wait: Duration) -> Status {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = self.changed.notified();
            if self.peek(id) != Some(Status::Pending) {
                return self.take(id);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Status::Pending;
            }
        }
    }

    /// 카드의 답(승인 데스크가 부른다). 모르는 요청(데몬이 다시 뜬 뒤 눌린 카드)이면 새로 적어 둔다.
    fn decide(&self, approval: &Approval, status: Status) {
        let mut entries = self.entries.lock().expect("action entries");
        let entry = entries.entry(approval.id.clone()).or_insert_with(|| Entry {
            digest: approval.payload["digest"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            status: Status::Pending,
            at: Instant::now(),
            used: false,
        });
        entry.status = status;
        entry.at = Instant::now();
        drop(entries);
        self.changed.notify_waiters();
    }
}

/// 승인 카드: 무엇을 하려는지와 실제로 실행될 명령·내용.
pub fn action_card(
    approval: &Approval,
    conversation: ConversationId,
    tz: chrono_tz::Tz,
    state: CardState<'_>,
) -> lucia_core::Card {
    let e = escape_markdown;
    let p = &approval.payload;
    let t = |k: &str| p[k].as_str().unwrap_or("").to_owned();
    let (headline, tone, buttons, footer) = open_or_closed(
        approval,
        tz,
        state,
        format!("🛂 **승인이 필요합니다** {}", e(&clip_raw(&t("what"), 80))),
        "승인",
        "하지 않기",
        "하지 않습니다",
    );
    let source = match t("source").as_str() {
        "shell" => "Lucia가 VM에서 실행하려는 명령",
        "browser" => "Lucia가 브라우저에서 하려는 일",
        _ => "Lucia가 하려는 일",
    };
    lucia_core::Card {
        conversation,
        headline,
        title: "바깥에 영향을 주는 동작".to_owned(),
        fields: Vec::new(),
        body: format!(
            "{source}:\n{}\n\n되돌릴 수 없거나 바깥에 남는 동작일 수 있습니다. 승인하면 Lucia가 이어서 실행합니다(데몬이 대신 실행하지 않습니다).",
            e(&t("detail"))
        ),
        footer: Some(footer),
        tone,
        buttons,
    }
}

pub struct ActionKind {
    pub gate: Arc<ActionGate>,
}

impl ApprovalKind for ActionKind {
    fn kind(&self) -> &'static str {
        KIND_EXTERNAL_ACTION
    }
    fn label(&self) -> &'static str {
        "바깥 동작"
    }
    fn not_done(&self) -> &'static str {
        "하지 않았습니다"
    }
    fn card(
        &self,
        approval: &Approval,
        conversation: ConversationId,
        tz: chrono_tz::Tz,
        state: CardState<'_>,
    ) -> lucia_core::Card {
        action_card(approval, conversation, tz, state)
    }
    fn execute<'a>(
        &'a self,
        approval: &'a Approval,
        _ctx: &'a KindCtx,
        _input: Option<&'a lucia_core::SecretText>,
    ) -> BoxFut<'a, Executed> {
        Box::pin(async move {
            self.gate.decide(approval, Status::Approved);
            Executed::simple(
                Ok(format!(
                    "승인했습니다: {}. Lucia가 이어서 실행합니다",
                    approval.payload["what"].as_str().unwrap_or("")
                )),
                "action_approved",
                "action_denied",
            )
        })
    }
    fn closed(&self, approval: &Approval, why: &str, _ctx: &KindCtx) {
        self.gate.decide(approval, Status::Denied(why.to_owned()));
    }
    fn resume_wanted(&self, _approval: &Approval) -> bool {
        false
    }
    fn interrupted(&self) -> (&'static str, &'static str, &'static str) {
        (
            "action_denied",
            "승인 처리 도중 데몬이 멈춤",
            "⚠️ 승인 처리 도중 데몬이 멈췄습니다. 필요하면 Lucia에게 다시 요청해 주세요.",
        )
    }
}

/// 모델·훅에게 돌려줄 말.
pub fn explain(status: &Status) -> String {
    match status {
        Status::Approved => "사용자가 승인했다. 승인된 그 동작만 지금 한 번 한다.".into(),
        Status::Denied(why) => format!(
            "사용자가 승인하지 않았다({why}). 하지 않는다. 다른 방법으로 같은 일을 하지 않는다."
        ),
        Status::Pending => "사용자가 아직 답하지 않았다. 하지 않고, 승인 카드를 확인해 달라고 보고한다. 승인된 뒤 같은 동작을 다시 청하면 바로 통과한다.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> (
        Arc<ActionGate>,
        mpsc::UnboundedReceiver<String>,
        Arc<Mutex<Store>>,
    ) {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let (tx, rx) = mpsc::unbounded_channel();
        (ActionGate::new(store.clone(), Some(tx)), rx, store)
    }

    fn approval(store: &Arc<Mutex<Store>>, id: &str) -> Approval {
        store.lock().unwrap().approval(id).unwrap().unwrap()
    }

    #[tokio::test]
    async fn a_request_posts_a_card_and_waits_for_the_answer() {
        let (g, mut posts, store) = gate();
        let id = g
            .request("chat-1", "git push", "git push origin main", "shell")
            .unwrap();
        assert_eq!(posts.try_recv().unwrap(), id);
        let a = approval(&store, &id);
        assert_eq!(a.kind, KIND_EXTERNAL_ACTION);
        assert_eq!(a.payload["detail"], "git push origin main");
        // 같은 동작을 다시 청해도 카드는 한 장이다.
        assert_eq!(
            g.request("chat-1", "git push", " git push origin main ", "shell")
                .unwrap(),
            id
        );
        assert!(posts.try_recv().is_err());
        assert_eq!(
            g.wait(&id, Duration::from_millis(30)).await,
            Status::Pending
        );
        // 기다리는 동안 승인되면 바로 안다.
        let kind = ActionKind { gate: g.clone() };
        let waiter = {
            let (g, id) = (g.clone(), id.clone());
            tokio::spawn(async move { g.wait(&id, Duration::from_secs(5)).await })
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        let ctx = KindCtx {
            store: store.clone(),
            tz: chrono_tz::Asia::Seoul,
        };
        let done = kind.execute(&a, &ctx, None).await;
        assert!(done.ok && done.status.contains("승인했습니다"));
        assert_eq!(waiter.await.unwrap(), Status::Approved);
        // 승인은 한 번만 쓴다: 같은 동작을 또 하려면 새 카드가 올라간다.
        let again = g
            .request("chat-1", "git push", "git push origin main", "shell")
            .unwrap();
        assert_ne!(again, id);
        assert_eq!(posts.try_recv().unwrap(), again);
    }

    #[tokio::test]
    async fn a_late_approval_lets_the_same_action_through_once() {
        let (g, mut posts, store) = gate();
        let id = g
            .request("chat-1", "릴리스", "gh release create v1.0", "shell")
            .unwrap();
        let _ = posts.try_recv();
        // 기다리던 쪽이 떠난 뒤 승인됐다.
        assert_eq!(
            g.wait(&id, Duration::from_millis(10)).await,
            Status::Pending
        );
        let kind = ActionKind { gate: g.clone() };
        let ctx = KindCtx {
            store: store.clone(),
            tz: chrono_tz::Asia::Seoul,
        };
        kind.execute(&approval(&store, &id), &ctx, None).await;
        // 같은 동작을 다시 청하면 새 카드 없이 그 승인으로 통과한다.
        let retry = g
            .request("chat-2", "릴리스", "gh release create v1.0", "shell")
            .unwrap();
        assert_eq!(retry, id);
        assert!(posts.try_recv().is_err());
        assert_eq!(
            g.wait(&retry, Duration::from_millis(10)).await,
            Status::Approved
        );
        assert!(matches!(
            g.wait(&retry, Duration::from_millis(10)).await,
            Status::Denied(_)
        ));
        // 다른 동작은 그 승인으로 지나가지 못한다.
        let other = g
            .request("chat-2", "릴리스", "gh release create v2.0", "shell")
            .unwrap();
        assert_ne!(other, id);
    }

    #[tokio::test]
    async fn denial_and_missing_desk_mean_no() {
        let (g, _posts, store) = gate();
        let id = g
            .request(
                "chat-1",
                "글 올리기",
                "curl -X POST https://example.com/post",
                "shell",
            )
            .unwrap();
        let kind = ActionKind { gate: g.clone() };
        let ctx = KindCtx {
            store: store.clone(),
            tz: chrono_tz::Asia::Seoul,
        };
        kind.closed(&approval(&store, &id), "거절", &ctx);
        let status = g.wait(&id, Duration::from_millis(10)).await;
        assert!(
            matches!(&status, Status::Denied(w) if w == "거절"),
            "{status:?}"
        );
        assert!(explain(&status).contains("하지 않는다"));
        assert!(matches!(
            g.wait("없는요청", Duration::from_millis(10)).await,
            Status::Denied(_)
        ));
        // 카드를 올릴 곳이 없으면(개발용 실행 등) 승인을 받을 수 없으니 거절한다. 빈 내용도 받지 않는다.
        let bare = ActionGate::new(store.clone(), None);
        assert!(bare.request("chat-1", "x", "git push", "shell").is_err());
        assert!(g.request("chat-1", "x", "  ", "shell").is_err());
        // 카드에 다 보이지 않는 긴 내용은 승인받지 않는다(끝에 숨은 명령을 보지 못한 채 승인하지 않게).
        let long = format!("{}\ngit push --force", "echo 줄\n".repeat(400));
        let err = g
            .request("chat-1", "긴 스크립트", &long, "shell")
            .unwrap_err();
        assert!(err.contains("너무 길어") && err.contains("나눠"), "{err}");
        // 데몬이 다시 뜬 뒤 눌린 카드도 적어 둔다.
        let fresh = ActionGate::new(store.clone(), None);
        let kind = ActionKind {
            gate: fresh.clone(),
        };
        kind.execute(&approval(&store, &id), &ctx, None).await;
        assert_eq!(
            fresh.wait(&id, Duration::from_millis(10)).await,
            Status::Approved
        );
        // 카드에는 실행될 명령이 그대로 보인다(마크다운은 이스케이프한다).
        let card = action_card(
            &approval(&store, &id),
            ConversationId("dm".into()),
            chrono_tz::Asia::Seoul,
            CardState::Open,
        );
        assert!(card.body.contains("curl") && card.body.contains("example\\.com"));
        assert_eq!(card.buttons.len(), 2);
    }
}
