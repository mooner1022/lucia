//! 앱 푸시 작업자(결정 0043): 접속 API가 맡긴 깨우기([`PushJob`])를 FCM으로 보낸다.
//!
//! - 자격 증명: 관리 페이지에서 넣은 Firebase 서비스 계정 JSON(자격 증명 파일 `fcm-service-account`, systemd-creds로
//!   암호화). 없으면 푸시를 쓰지 않고, 끊긴 앱으로 갈 글은 예전처럼 Discord로 간다.
//! - 내용: 기본은 "새 메시지가 있습니다" 같은 고정 문구만 싣는다(푸시는 Google을 거친다). 관리 페이지에서
//!   미리보기를 켜면 글 앞부분을 싣는다. 데이터에는 `seq`가 있어 앱이 열리면 상자에서 받아 간다.
//! - 글 알림은 한 앱에 10초에 한 번까지(나머지는 앱을 열면 다 받는다). 카드는 늘 보낸다.
//! - 등록 해제된 토큰(UNREGISTERED)은 지운다. 설정 문제(인증)는 활동 기록에 남긴다.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use lucia_store::Store;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;

use crate::credentials::Credentials;
use crate::gateway::{Gateway, PushJob, keys};
use crate::push::{Fcm, PushError, PushMessage, ServiceAccount};

/// 자격 증명 이름.
pub const CREDENTIAL: &str = "fcm-service-account";
/// 글 알림 간격(한 앱).
const TEXT_GAP: Duration = Duration::from_secs(10);
/// 일시 오류 뒤 다시 보낼 때 기다리는 최대 시간.
const RETRY_MAX: Duration = Duration::from_secs(60);

pub struct AppPush {
    gateway: Arc<Gateway>,
    store: Arc<Mutex<Store>>,
    creds: Arc<Credentials>,
    http: reqwest::Client,
    fcm: RwLock<Option<Arc<Fcm>>>,
    last_text: Mutex<HashMap<String, Instant>>,
    /// 시험용: FCM·토큰 주소 바꾸기.
    #[cfg(test)]
    endpoints: Option<(String, String)>,
}

impl AppPush {
    pub fn new(
        gateway: Arc<Gateway>,
        store: Arc<Mutex<Store>>,
        creds: Arc<Credentials>,
        http: reqwest::Client,
    ) -> Arc<Self> {
        Arc::new(Self {
            gateway,
            store,
            creds,
            http,
            fcm: RwLock::new(None),
            last_text: Mutex::new(HashMap::new()),
            #[cfg(test)]
            endpoints: None,
        })
    }

    /// 자격 증명을 (다시) 읽는다. 있으면 프로젝트 ID.
    pub async fn reload(&self) -> Result<Option<String>, String> {
        let json = self
            .creds
            .read(CREDENTIAL)
            .await
            .map_err(|e| format!("자격 증명을 읽지 못했다: {e}"))?;
        let Some(json) = json else {
            *self.fcm.write().await = None;
            self.gateway.set_push_ready(false);
            return Ok(None);
        };
        let account = ServiceAccount::parse(&json)?;
        let project = account.project_id().to_owned();
        let fcm = Fcm::new(self.http.clone(), account);
        #[cfg(test)]
        let fcm = match &self.endpoints {
            Some((base, token)) => fcm.with_endpoints(base, token),
            None => fcm,
        };
        *self.fcm.write().await = Some(Arc::new(fcm));
        self.gateway.set_push_ready(true);
        Ok(Some(project))
    }

    /// 서비스 계정 JSON을 확인하고 저장한 뒤 다시 읽는다(관리 페이지).
    pub async fn install(&self, json: &str) -> Result<String, String> {
        let account = ServiceAccount::parse(json)?;
        self.creds
            .write(CREDENTIAL, json.trim())
            .await
            .map_err(|e| format!("저장하지 못했다: {e}"))?;
        self.reload().await?;
        Ok(account.project_id().to_owned())
    }

    pub async fn remove(&self) -> Result<(), String> {
        self.creds
            .remove(CREDENTIAL)
            .await
            .map_err(|e| format!("지우지 못했다: {e}"))?;
        self.reload().await.map(|_| ())
    }

    pub async fn project(&self) -> Option<String> {
        self.fcm
            .read()
            .await
            .as_ref()
            .map(|f| f.account().project_id().to_owned())
    }

    fn preview_on(&self) -> bool {
        self.store
            .lock()
            .expect("store lock")
            .setting(keys::PUSH_PREVIEW)
            .ok()
            .flatten()
            .is_some_and(|v| v == "1")
    }

    /// 깨우기 하나를 메시지로.
    fn message(&self, job: &PushJob) -> PushMessage {
        // 다른 기기에서 봤다(결정 0066): 보이는 알림 없이, 앱이 띄워 둔 알림을 치우게 한다.
        if job.kind == "clear" {
            let mut data = BTreeMap::new();
            data.insert("type".into(), "clear".into());
            data.insert("seq".into(), job.seq.to_string());
            data.insert("client".into(), job.client_id.clone());
            return PushMessage {
                token: job.token.clone(),
                title: None,
                body: None,
                data,
                high_priority: false,
                collapse: None,
            };
        }
        let generic = match job.kind.as_str() {
            "card" => "확인할 카드가 있습니다",
            "files" => "파일이 왔습니다",
            "sticker" => "스티커를 보냈어요",
            _ => "새 메시지가 있습니다",
        };
        let body = if self.preview_on() && !job.preview.trim().is_empty() {
            crate::mask::global().mask(&job.preview.chars().take(120).collect::<String>())
        } else {
            generic.to_owned()
        };
        let mut data = BTreeMap::new();
        data.insert("type".into(), "wake".into());
        data.insert("kind".into(), job.kind.clone());
        data.insert("seq".into(), job.seq.to_string());
        data.insert("client".into(), job.client_id.clone());
        PushMessage {
            token: job.token.clone(),
            title: Some("Lucia".into()),
            body: Some(body),
            data,
            high_priority: job.high,
            // 글은 한 알림으로 바꿔 끼우고, 카드는 따로 보인다.
            collapse: (job.kind != "card").then(|| "lucia-messages".to_owned()),
        }
    }

    /// 글 알림 간격을 지키나(카드는 늘 보낸다).
    fn due(&self, job: &PushJob) -> bool {
        if job.kind == "card" || job.kind == "clear" {
            return true;
        }
        let mut last = self.last_text.lock().expect("push lock");
        match last.get(&job.client_id) {
            Some(at) if at.elapsed() < TEXT_GAP => false,
            _ => {
                last.insert(job.client_id.clone(), Instant::now());
                true
            }
        }
    }

    /// 하나 보낸다. 결과를 돌려준다(시험·관리 페이지 시험 발송).
    pub async fn send(&self, job: &PushJob) -> Result<(), PushError> {
        let Some(fcm) = self.fcm.read().await.clone() else {
            return Err(PushError::Auth("푸시 자격 증명이 없다".into()));
        };
        let msg = self.message(job);
        let mut result = fcm.send(&msg).await;
        if let Err(PushError::Transient { retry_after, .. }) = &result {
            let wait = retry_after.unwrap_or(Duration::from_secs(5)).min(RETRY_MAX);
            tokio::time::sleep(wait).await;
            result = fcm.send(&msg).await;
        }
        match &result {
            Ok(_) => {}
            Err(PushError::Unregistered) => {
                let store = self.store.lock().expect("store lock");
                let _ = store.set_client_push(&job.client_id, None, None, Utc::now());
                let _ = store.add_activity(
                    "client_push",
                    &format!("앱 푸시 토큰이 해제돼 지웠다: {}", job.client_id),
                    None,
                    Utc::now(),
                );
            }
            Err(e) => {
                tracing::warn!(client = %job.client_id, err = %e, "앱 푸시 실패");
                // 설정 문제(인증)면 푸시를 쓸 수 없는 것으로 본다: 그 뒤의 글·카드는 Discord DM으로 간다.
                // 이번 것은 앱 상자에 남는다(앱을 열면 받는다). 자격 증명을 다시 넣으면 되살아난다.
                if matches!(e, PushError::Auth(_)) {
                    self.gateway.set_push_ready(false);
                }
                let _ = self.store.lock().expect("store lock").add_activity(
                    "push_failed",
                    &format!("앱 푸시 실패({}): {e}", job.client_id),
                    None,
                    Utc::now(),
                );
            }
        }
        result.map(|_| ())
    }

    /// 접속 API가 맡긴 깨우기를 차례로 보낸다.
    pub async fn run(
        self: Arc<Self>,
        mut jobs: mpsc::UnboundedReceiver<PushJob>,
        cancel: CancellationToken,
    ) {
        loop {
            let job = tokio::select! {
                _ = cancel.cancelled() => return,
                job = jobs.recv() => match job {
                    Some(job) => job,
                    None => return,
                },
            };
            if !self.due(&job) {
                continue;
            }
            // 일시 오류의 기다림이 뒤의 카드를 막지 않게 떨어진 작업으로.
            let me = self.clone();
            tokio::spawn(async move {
                let _ = me.send(&job).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker() -> Arc<AppPush> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let (itx, _irx) = mpsc::channel(1);
        let (ptx, _prx) = mpsc::channel(1);
        let gw = Gateway::new(store.clone(), itx, ptx);
        let dir = std::env::temp_dir().join(format!("lucia-push-{}", uuid::Uuid::new_v4()));
        let creds = Arc::new(Credentials::new(dir, crate::mask::global().clone()));
        AppPush::new(gw, store, creds, reqwest::Client::new())
    }

    fn job(kind: &str) -> PushJob {
        PushJob {
            client_id: "cl-1".into(),
            token: "tok".into(),
            kind: kind.into(),
            seq: 7,
            preview: "오늘 3시 회의가 있어요".into(),
            high: kind == "card",
        }
    }

    #[test]
    fn messages_hide_content_unless_preview_is_on() {
        let w = worker();
        let m = w.message(&job("text"));
        assert_eq!(m.body.as_deref(), Some("새 메시지가 있습니다"));
        assert_eq!(m.data["seq"], "7");
        assert_eq!(m.data["kind"], "text");
        assert_eq!(m.collapse.as_deref(), Some("lucia-messages"));
        let c = w.message(&job("card"));
        assert_eq!(c.body.as_deref(), Some("확인할 카드가 있습니다"));
        assert!(c.high_priority && c.collapse.is_none());
        w.store
            .lock()
            .unwrap()
            .set_setting(keys::PUSH_PREVIEW, "1", Utc::now())
            .unwrap();
        assert_eq!(
            w.message(&job("text")).body.as_deref(),
            Some("오늘 3시 회의가 있어요")
        );
    }

    #[test]
    fn text_wakes_are_spaced_but_cards_are_not() {
        let w = worker();
        assert!(w.due(&job("text")));
        assert!(!w.due(&job("text")), "10초 안의 두 번째 글은 넘어간다");
        assert!(w.due(&job("card")));
        assert!(w.due(&job("card")));
    }
}
