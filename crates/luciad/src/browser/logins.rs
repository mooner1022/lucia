//! 사이트 로그인 흐름(결정 0006, 명세 §세션 만료 처리 흐름).
//!
//! 0. 사이트: 내장 프로필(퀘이사존) 또는 일반 프로필(그 밖의 https 사이트, 결정 0008). 일반 프로필의 토큰·저장된
//!    로그인은 정확한 출처(`https://호스트`)에 묶고, 메시지·입력 폼·승인 카드에 그 출처를 그대로 보인다.
//!    일반 사이트는 Lucia가 `browser_login`을 명시적으로 부를 때만 요청한다(페이지에 비밀번호 칸이 있다는
//!    이유만으로 보내지 않는다: 악성 페이지가 닮은 도메인의 입력 링크를 사용자에게 보내게 만들 수 있다).
//! 1. 로그인이 필요하면(내장 사이트의 로그인 페이지로 튕김, 또는 Lucia가 요청):
//!    - 저장된 로그인이 있으면 Discord 승인 카드를 올린다(저장된 로그인을 다시 쓸 때마다 승인, dots 방식).
//!    - 없으면 관리 페이지의 **입력 링크**(10분, 한 사이트에만)를 Discord로 보낸다. 성공하면 닫히고, 실패하면 같은
//!      링크로 [`INPUT_ATTEMPTS`]번까지 다시 넣는다(결정 0038). 하는 중에 다시 오면 기다리라고 한다.
//! 2. 사람이 링크에서 아이디·비밀번호를 넣으면 데몬이 브라우저 칸에 바로 넣고 제출한다(한 번만 시도).
//!    받은 즉시 비밀번호를 마스킹 필터에 등록한다. 값은 모델·로그·원장·메신저 어디에도 가지 않는다.
//! 3. "저장"을 고르면 비밀번호는 `systemd-creds`로 암호화해 두고(자동 재로그인), 아이디는 설정에 둔다
//!    (아이디는 비밀이 아니고, 가리면 사이트 글에서 사용자 이름까지 가려진다).
//!
//! 사람을 기다리는 동안 도구 호출을 붙잡지 않는다. 결과는 에피소드로 남겨 Lucia가 다음 대화에서 안다.

use crate::gateway::NotifyTarget as _;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use lucia_channel::ChannelAdapter;
use lucia_core::{ConversationId, Outbound};
use lucia_store::{ApprovalStatus, EpisodeKind, NewApproval, NewEpisode, Origin, Store};
use serde_json::json;
use tokio::sync::mpsc;
use url::Url;
use uuid::Uuid;

use super::{BoxFut, Browser, LoginResult, SiteProfile};
use crate::approvals::{KIND_LOGIN_CODE, KIND_LOGIN_INPUT, KIND_SITE_LOGIN, SiteLoginExec};

/// 소유자에게 알린다(Discord DM).
pub trait Notifier: Send + Sync {
    /// 보냈으면 `true`.
    fn notify(&self, text: String) -> BoxFut<'_, bool>;
}

/// 비밀 저장소(실제는 `systemd-creds` 파일).
pub trait SecretStore: Send + Sync {
    fn read<'a>(&'a self, name: &'a str) -> BoxFut<'a, anyhow::Result<Option<String>>>;
    fn write<'a>(&'a self, name: &'a str, value: &'a str) -> BoxFut<'a, anyhow::Result<()>>;
    fn remove<'a>(&'a self, name: &'a str) -> BoxFut<'a, anyhow::Result<()>>;
    fn exists(&self, name: &str) -> bool;
}

impl SecretStore for crate::credentials::Credentials {
    fn read<'a>(&'a self, name: &'a str) -> BoxFut<'a, anyhow::Result<Option<String>>> {
        Box::pin(crate::credentials::Credentials::read(self, name))
    }
    fn write<'a>(&'a self, name: &'a str, value: &'a str) -> BoxFut<'a, anyhow::Result<()>> {
        Box::pin(crate::credentials::Credentials::write(self, name, value))
    }
    fn remove<'a>(&'a self, name: &'a str) -> BoxFut<'a, anyhow::Result<()>> {
        Box::pin(crate::credentials::Credentials::remove(self, name))
    }
    fn exists(&self, name: &str) -> bool {
        crate::credentials::Credentials::exists(self, name)
    }
}

/// 메신저로 알리는 [`Notifier`]: 관리 페이지에서 정한 알림 대화(소유자 DM)로 보낸다.
pub struct ChannelNotifier<C> {
    pub channel: Arc<C>,
    pub store: Arc<Mutex<Store>>,
    pub fallback: Option<ConversationId>,
}

impl<C: ChannelAdapter> Notifier for ChannelNotifier<C> {
    fn notify(&self, text: String) -> BoxFut<'_, bool> {
        Box::pin(async move {
            let conversation = self
                .store
                .lock()
                .expect("store lock")
                .notify_conversation()
                .map(ConversationId)
                .or_else(|| self.fallback.clone());
            match conversation {
                Some(conversation) if self.channel.ready() => self
                    .channel
                    .send(Outbound::Text {
                        conversation,
                        body: crate::mask::global().mask(&text),
                    })
                    .await
                    .is_ok(),
                _ => false,
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct LoginsConfig {
    /// 입력 링크의 앞부분(관리 페이지 주소, 예: `http://192.168.0.2:8780`).
    pub form_base: String,
    pub input_ttl: Duration,
    pub approval_ttl: Duration,
}

/// 로그인 요청을 어떻게 했는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requested {
    /// 입력 링크를 보냈다.
    InputLink,
    /// 앱에 로그인 입력 카드를 올렸다(결정 0063). 카드 본문에 입력 링크도 있다.
    InputCard,
    /// 저장된 로그인을 다시 쓸지 승인 카드를 올렸다.
    ApprovalCard,
    /// 같은 사이트에 대한 요청이 이미 기다리고 있다(다시 보내지 않았다).
    AlreadyPending,
}

/// 입력 폼 제출 결과.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum FormOutcome {
    Done {
        site_name: String,
        note: String,
    },
    /// 사이트가 받지 않았거나 실패했다. 같은 링크로 다시 넣을 수 있으면 `retry_token`(시도 상한이면 `None`).
    Failed {
        reason: String,
        retry_token: Option<String>,
    },
    /// 없는 토큰, 만료, 다 쓴 토큰.
    Invalid,
    /// 같은 링크로 넣은 로그인을 지금 하고 있다(제출을 두 번 눌렀거나 브라우저가 다시 보냄).
    Busy {
        site_name: String,
    },
    /// 아직 끝나지 않았다(인증 코드가 필요하다, 결정 0017). `code_link`는 코드를 넣는 일회용 링크.
    Pending {
        site_name: String,
        note: String,
        code_link: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SiteStatus {
    pub id: String,
    pub name: String,
    pub origin: String,
    pub saved: bool,
    pub username: Option<String>,
}

struct PendingInput {
    site: String,
    expires: DateTime<Utc>,
    /// 실패한 시도 수(같은 링크로 다시 넣을 수 있다, [`INPUT_ATTEMPTS`]번까지).
    attempts: u32,
    /// 지금 이 링크로 로그인하는 중(제출이 끝날 때까지 수십 초 걸린다).
    busy: bool,
    /// 마지막으로 끝난 시도의 결과(결과 화면이 보인다). 성공·인증 코드·시도 상한이면 이 링크는 끝났다(`done`).
    last: Option<FormOutcome>,
    done: bool,
    /// 앱 카드로 요청했다(결정 0063): 다시 넣을 수 있는 실패면 카드를 새로 올린다.
    card: bool,
    /// 요청한 까닭(다시 올리는 카드에 그대로 보인다).
    why: String,
}

/// 입력 링크의 지금 상태(결과 화면, 결정 0038).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum FormStatus {
    /// 없는 토큰, 만료.
    Invalid,
    /// 넣을 수 있다(마지막 시도가 실패했으면 그 까닭).
    Ready {
        site: Box<SiteProfile>,
        error: Option<String>,
    },
    /// 로그인하고 있다.
    Busy { site_name: String },
    /// 끝났다(성공, 인증 코드 필요, 시도 상한).
    Finished(FormOutcome),
}

/// 입력한 로그인 하나에 주는 시간(이 안에 끝나지 않으면 실패로 센다).
const SUBMIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(150);

/// 입력 링크 하나로 해 볼 수 있는 로그인 수 [제안]. 비밀번호를 잘못 넣어도 Discord 링크를 다시 쓸 수 있게.
pub const INPUT_ATTEMPTS: u32 = 3;

pub struct SiteLogins {
    browser: Arc<dyn Browser>,
    secrets: Arc<dyn SecretStore>,
    store: Arc<Mutex<Store>>,
    notifier: Arc<dyn Notifier>,
    /// 승인 데스크에 카드 올리기를 맡기는 통로(메일 발송 요청과 같다).
    approval_posts: Option<mpsc::UnboundedSender<String>>,
    sites: Vec<SiteProfile>,
    cfg: LoginsConfig,
    inputs: Mutex<HashMap<String, PendingInput>>,
    /// 인증 코드를 기다리는 사이트와 그 웹 입력 토큰(결정 0017). 기다리는 동안 다른 쪽은 브라우저를 쓰지 않는다.
    code: Mutex<Option<PendingCode>>,
    /// 사람이 화면을 넘겨받아 쓰는 중(결정 0017): 데몬은 로그인 칸을 채우지 않는다.
    handover: std::sync::atomic::AtomicBool,
}

/// 저장된 로그인으로 다시 로그인한 결과(승인 카드의 실행).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedLogin {
    LoggedIn(String),
    /// 비밀번호는 받아들였고 인증 코드를 기다린다(아직 로그인하지 않았다).
    NeedsCode(String),
}

const HANDOVER_BUSY: &str = "사람이 브라우저 화면을 넘겨받아 쓰는 중이라 로그인하지 않았다";

struct PendingCode {
    site: String,
    token: String,
    expires: DateTime<Utc>,
}

/// 인증 코드를 기다리는 시간 [제안]. OTP는 대개 3~5분 안에 만료된다.
pub const CODE_TTL_MINUTES: i64 = 5;
const CODE_MAX: usize = 12;

const SITE_PREFIX: &str = "browser.site.";

fn username_key(site: &SiteProfile) -> String {
    format!("{SITE_PREFIX}{}.username", site.id)
}

/// 일반 프로필의 로그인 페이지 주소.
fn login_url_key(id: &str) -> String {
    format!("{SITE_PREFIX}{id}.login_url")
}

const USERNAME_MAX: usize = 100;
const PASSWORD_MAX: usize = 200;

impl SiteLogins {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        browser: Arc<dyn Browser>,
        secrets: Arc<dyn SecretStore>,
        store: Arc<Mutex<Store>>,
        notifier: Arc<dyn Notifier>,
        approval_posts: Option<mpsc::UnboundedSender<String>>,
        sites: Vec<SiteProfile>,
        cfg: LoginsConfig,
    ) -> Self {
        Self {
            browser,
            secrets,
            store,
            notifier,
            approval_posts,
            sites,
            cfg,
            inputs: Mutex::default(),
            code: Mutex::default(),
            handover: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// 화면 넘겨받기 상태(BrowserTools가 알린다).
    pub fn set_handover(&self, on: bool) {
        self.handover.store(on, std::sync::atomic::Ordering::SeqCst);
    }

    fn handed_over(&self) -> bool {
        self.handover.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 인증 코드를 기다리는 중인가(브라우저를 그 단계에 붙잡아 둔다).
    pub fn code_pending(&self) -> bool {
        self.code
            .lock()
            .expect("code lock")
            .as_ref()
            .is_some_and(|c| c.expires > Utc::now())
    }

    /// 인증 코드 칸이 나왔다: 승인 카드([코드 입력] 버튼)와 웹 입력 링크(버튼을 못 쓰는 채널용)를 함께 준다.
    /// 사람에게 보일 안내를 돌려준다.
    fn request_code(&self, site: &SiteProfile, hint: &str) -> String {
        let now = Utc::now();
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let expires = now + Duration::minutes(CODE_TTL_MINUTES);
        *self.code.lock().expect("code lock") = Some(PendingCode {
            site: site.id.clone(),
            token: token.clone(),
            expires,
        });
        let link = format!(
            "{}/site-login/code/{token}",
            self.cfg.form_base.trim_end_matches('/')
        );
        let card = self.approval_posts.as_ref().and_then(|posts| {
            let id = Uuid::new_v4().simple().to_string();
            self.store()
                .create_approval(
                    &NewApproval {
                        id: id.clone(),
                        kind: KIND_LOGIN_CODE.into(),
                        payload: json!({
                            "site": site.id,
                            "site_name": site.name,
                            "origin": site.origin,
                            "hint": hint,
                            "link": link,
                        }),
                        summary: format!("인증 코드 → {}", site.display()),
                        run_id: None,
                        expires_at: expires,
                    },
                    now,
                )
                .ok()?;
            posts.send(id).ok()
        });
        self.activity(
            "login_code",
            &format!("{} 인증 코드 요청", site.display()),
            None,
        );
        match card {
            Some(()) => format!(
                "인증 코드가 필요합니다({hint}). 카드의 [코드 입력]이나 카드의 링크로 {CODE_TTL_MINUTES}분 안에 넣어 주세요."
            ),
            None => format!(
                "인증 코드가 필요합니다({hint}). {CODE_TTL_MINUTES}분 안에 이 링크에서 넣어 주세요: {link}"
            ),
        }
    }

    /// 인증 코드를 넣는다(Discord 입력창 또는 웹 입력). 한 번만 시도하고, 값은 어디에도 남기지 않는다.
    pub async fn submit_code(&self, site_id: &str, code: &str) -> Result<String, String> {
        if self.handed_over() {
            return Err(HANDOVER_BUSY.into());
        }
        let code = code.trim();
        let pending = self.code.lock().expect("code lock").take();
        let Some(pending) = pending.filter(|p| p.site == site_id && p.expires > Utc::now()) else {
            return Err("코드를 기다리는 로그인이 없다(시간이 지났거나 이미 넣었다)".into());
        };
        let _ = pending.token;
        if code.is_empty() || code.chars().count() > CODE_MAX || code.contains(['\r', '\n']) {
            return Err("코드를 확인해 주세요(빈 값이나 너무 긴 값)".into());
        }
        crate::mask::global().register(code);
        let site = self
            .site(site_id)
            .ok_or_else(|| format!("모르는 사이트: {site_id}"))?;
        match self.browser.submit_code(&site, code).await {
            Ok(LoginResult::LoggedIn | LoginResult::AlreadyLoggedIn) => {
                self.activity(
                    "login_ok",
                    &format!("{} 로그인(인증 코드)", site.name),
                    None,
                );
                self.episode(&format!(
                    "[로그인] {}에 로그인했다(인증 코드, 사용자 입력). 멈췄던 일을 이어서 할 수 있다.",
                    site.name
                ));
                Ok("로그인했습니다. 하던 일을 다시 말씀해 주세요.".into())
            }
            Ok(LoginResult::NeedsCode { .. }) => Err("코드를 넣었지만 다시 코드를 묻는다".into()),
            Ok(LoginResult::NeedsHuman { why }) => {
                let note = self.human_step(&site, &why).await;
                Err(note)
            }
            Ok(LoginResult::Rejected(reason)) => {
                self.activity(
                    "login_failed",
                    &format!("{} 인증 코드 실패: {reason}", site.name),
                    None,
                );
                Err(format!("사이트가 받지 않았습니다: {reason}"))
            }
            Err(e) => Err(crate::mask::global().mask(&e.to_string())),
        }
    }

    /// 지금 기다리는 인증 코드의 웹 입력 링크.
    pub fn code_link(&self) -> Option<String> {
        let code = self.code.lock().expect("code lock");
        code.as_ref().filter(|c| c.expires > Utc::now()).map(|c| {
            format!(
                "{}/site-login/code/{}",
                self.cfg.form_base.trim_end_matches('/'),
                c.token
            )
        })
    }

    /// 웹 입력 링크의 토큰이 지금 기다리는 코드의 것이면 그 사이트(쓰지는 않는다).
    pub fn code_form_peek(&self, token: &str) -> Option<SiteProfile> {
        let site = {
            let code = self.code.lock().expect("code lock");
            code.as_ref()
                .filter(|c| c.token == token && c.expires > Utc::now())
                .map(|c| c.site.clone())
        }?;
        self.site(&site)
    }

    /// 웹 입력 링크로 넣은 코드.
    pub async fn code_form_submit(&self, token: &str, code: &str) -> Result<String, String> {
        let site = self
            .code_form_peek(token)
            .ok_or("없는 링크이거나 시간이 지났다")?;
        // 같은 요청의 Discord 카드는 데스크가 만료로 닫는다.
        self.submit_code(&site.id, code).await
    }

    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().expect("store lock")
    }

    /// 내장 프로필.
    pub fn sites(&self) -> &[SiteProfile] {
        &self.sites
    }

    /// ID로 프로필을 찾는다: 내장 ID, 또는 일반 프로필의 호스트(저장된 로그인 페이지 주소를 쓴다).
    pub fn site(&self, id: &str) -> Option<SiteProfile> {
        if let Some(s) = self.sites.iter().find(|s| s.id == id) {
            return Some(s.clone());
        }
        let host = crate::browser::egress::normalize_domain(id).ok()?;
        if host != id || !host.contains('.') {
            return None;
        }
        let saved = self.store().setting(&login_url_key(&host)).ok().flatten();
        let url = saved
            .and_then(|u| Url::parse(&u).ok())
            .filter(|u| u.host_str() == Some(host.as_str()))
            .or_else(|| Url::parse(&format!("https://{host}/")).ok())?;
        SiteProfile::generic(&url).ok()
    }

    /// 주소에 맞는 프로필: 내장 사이트의 주소면 그 프로필, 아니면 그 호스트의 일반 프로필.
    pub fn profile_for(&self, url: &Url) -> Result<SiteProfile, String> {
        if let Some(s) = self.sites.iter().find(|s| s.allows(url)) {
            return Ok(s.clone());
        }
        SiteProfile::generic(url)
    }

    /// 내장 사이트의 주소면 그 프로필.
    pub fn builtin_for(&self, url: &Url) -> Option<&SiteProfile> {
        self.sites.iter().find(|s| s.allows(url))
    }

    /// 일반 프로필의 로그인 페이지 주소를 기억한다(다음 재로그인에 쓴다).
    pub fn remember_login_url(&self, site: &SiteProfile) {
        if site.is_builtin() {
            return;
        }
        let _ = self
            .store()
            .set_setting(&login_url_key(&site.id), &site.login.url, Utc::now());
    }

    pub fn browser(&self) -> &Arc<dyn Browser> {
        &self.browser
    }

    /// 내장 사이트와, 로그인한 적 있는 일반 사이트.
    pub fn status(&self) -> Vec<SiteStatus> {
        let generic: Vec<String> = {
            let store = self.store();
            let mut hosts: Vec<String> = store
                .settings_with_prefix(SITE_PREFIX)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(k, _)| {
                    let rest = k.strip_prefix(SITE_PREFIX)?;
                    rest.strip_suffix(".username")
                        .or_else(|| rest.strip_suffix(".login_url"))
                        .map(str::to_owned)
                })
                .filter(|h| h.contains('.') && !self.sites.iter().any(|s| &s.id == h))
                .collect();
            hosts.sort();
            hosts.dedup();
            hosts
        };
        let profiles: Vec<SiteProfile> = self
            .sites
            .iter()
            .cloned()
            .chain(generic.iter().filter_map(|h| self.site(h)))
            .collect();
        let store = self.store();
        profiles
            .iter()
            .map(|s| SiteStatus {
                id: s.id.clone(),
                name: s.name.clone(),
                origin: s.origin.clone(),
                saved: self.secrets.exists(&s.credential_name()),
                username: store.setting(&username_key(s)).ok().flatten(),
            })
            .collect()
    }

    fn issue(&self, site: &str, why: &str, card: bool, now: DateTime<Utc>) -> String {
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let mut inputs = self.inputs.lock().expect("inputs lock");
        inputs.retain(|_, p| p.expires > now);
        inputs.insert(
            token.clone(),
            PendingInput {
                site: site.to_owned(),
                expires: now + self.cfg.input_ttl,
                attempts: 0,
                busy: false,
                last: None,
                done: false,
                card,
                why: why.chars().take(300).collect(),
            },
        );
        token
    }

    /// 로그인 입력 카드를 올릴 수 있는가(결정 0063): 알림 받을 곳이 자체 앱이고 승인 데스크가 있다.
    /// 앱은 값이 데몬까지 곧장 온다. Discord로는 비밀번호를 받지 않는다(그때는 입력 링크).
    fn card_target(&self) -> bool {
        self.approval_posts.is_some()
            && self
                .store()
                .notify_conversation()
                .is_some_and(|c| crate::gateway::client_of(&c).is_some())
    }

    /// 로그인 입력 카드를 올린다. `error`는 앞선 시도가 실패한 까닭(다시 올릴 때).
    fn post_input_card(
        &self,
        site: &SiteProfile,
        why: &str,
        token: &str,
        error: Option<&str>,
        run_id: Option<&str>,
    ) -> Result<(), String> {
        let Some(posts) = &self.approval_posts else {
            return Err("승인 카드를 올릴 수 없다(승인 데스크 없음)".into());
        };
        let now = Utc::now();
        let id = Uuid::new_v4().simple().to_string();
        self.store()
            .create_approval(
                &NewApproval {
                    id: id.clone(),
                    kind: KIND_LOGIN_INPUT.into(),
                    payload: json!({
                        "site": site.id,
                        "site_name": site.name,
                        "origin": site.origin,
                        "reason": why,
                        "token": token,
                        "link": self.link(token),
                        "error": error,
                        "attempts": INPUT_ATTEMPTS,
                    }),
                    summary: format!("로그인 입력 → {}", site.display()),
                    run_id: run_id.map(str::to_owned),
                    expires_at: now + self.cfg.input_ttl,
                },
                now,
            )
            .map_err(|e| format!("승인 요청 저장: {e}"))?;
        posts
            .send(id)
            .map_err(|_| "승인 데스크가 멈췄다".to_owned())
    }

    /// 앱 카드에서 넣은 로그인(결정 0063). 링크와 같은 토큰을 쓰므로 한쪽에서 하면 다른 쪽은 닫힌다.
    /// 로그인은 떨어진 작업으로 돌고(결정 0038) 결과는 알림으로 간다. 받았으면 카드에 보일 문구.
    pub fn submit_input(
        self: &Arc<Self>,
        token: &str,
        username: String,
        password: String,
        save: bool,
    ) -> Result<String, String> {
        let now = Utc::now();
        let state = {
            let inputs = self.inputs.lock().expect("inputs lock");
            inputs
                .get(token)
                .filter(|p| p.expires > now)
                .map(|p| (p.busy, p.done))
        };
        match state {
            None => Err("시간이 지난 요청입니다. Lucia에게 로그인을 다시 요청해 주세요.".into()),
            Some((true, _)) => Err("같은 요청으로 이미 로그인하고 있습니다.".into()),
            Some((_, true)) => {
                Err("이미 끝난 요청입니다(링크에서 처리했거나 시도를 다 썼습니다).".into())
            }
            Some(_) => {
                if self.spawn_submit(token, username, password, save) {
                    Ok("로그인하고 있습니다. 결과는 곧 알려 드립니다.".into())
                } else {
                    Err("시간이 지난 요청입니다. Lucia에게 로그인을 다시 요청해 주세요.".into())
                }
            }
        }
    }

    /// 앱 카드로 받은 요청이 다시 넣을 수 있는 실패로 끝났으면 카드를 새로 올린다(결정 0063). 쓴 카드는 닫혔다.
    /// 링크에서 낸 실패처럼 열린 카드가 아직 있으면 올리지 않는다(같은 요청의 카드를 둘로 만들지 않는다).
    fn repost_card(&self, token: &str, site: &SiteProfile, outcome: &FormOutcome) {
        let FormOutcome::Failed {
            reason,
            retry_token: Some(_),
        } = outcome
        else {
            return;
        };
        let why = {
            let inputs = self.inputs.lock().expect("inputs lock");
            inputs
                .get(token)
                .filter(|p| p.card && !p.done)
                .map(|p| p.why.clone())
        };
        let Some(why) = why else { return };
        let now = Utc::now();
        let open = self
            .store()
            .recent_approvals(50)
            .unwrap_or_default()
            .iter()
            .any(|a| {
                a.kind == KIND_LOGIN_INPUT
                    && a.status == ApprovalStatus::Pending
                    && a.expires_at > now
                    && a.payload["token"].as_str() == Some(token)
            });
        if open {
            return;
        }
        // 사이트의 오류 문구가 넣은 값을 되풀이해도 카드와 승인 기록에 남지 않게 가린다.
        let reason = crate::mask::global().mask(reason);
        if let Err(e) = self.post_input_card(site, &why, token, Some(&reason), None) {
            tracing::warn!(site = %site.id, %e, "로그인 입력 카드를 다시 올리지 못했다");
        }
    }

    /// 사용자가 입력 카드를 취소했거나 카드가 만료됐다: 아직 쓰지 않은 요청이면 링크도 닫는다.
    pub fn cancel_input(&self, token: &str) {
        let mut inputs = self.inputs.lock().expect("inputs lock");
        if inputs.get(token).is_some_and(|p| !p.busy && !p.done) {
            inputs.remove(token);
        }
    }

    fn link(&self, token: &str) -> String {
        format!(
            "{}/site-login/{token}",
            self.cfg.form_base.trim_end_matches('/')
        )
    }

    fn input_message(&self, site: &SiteProfile, why: &str, token: &str) -> String {
        format!(
            "🔐 **로그인이 필요합니다: {}** {why}\n\
             사이트 주소를 확인하고, 맞으면 아래 링크에서 아이디와 비밀번호를 넣어 주세요({}분 안, 실패하면 같은 링크로 3번까지). \
             입력한 값은 Lucia(모델)에게 가지 않고 데몬이 브라우저에 바로 넣습니다.\n{}",
            site.display(),
            self.cfg.input_ttl.num_minutes(),
            self.link(token)
        )
    }

    fn pending_approval(&self, site: &str, now: DateTime<Utc>) -> bool {
        self.store()
            .recent_approvals(50)
            .unwrap_or_default()
            .iter()
            .any(|a| {
                a.kind == KIND_SITE_LOGIN
                    && a.status == ApprovalStatus::Pending
                    && a.expires_at > now
                    && a.payload["site"].as_str() == Some(site)
            })
    }

    fn activity(&self, kind: &str, text: &str, run_id: Option<&str>) {
        let _ =
            self.store()
                .add_activity(kind, &crate::mask::global().mask(text), run_id, Utc::now());
    }

    /// 페이지 열기를 활동 기록에 남긴다.
    pub fn note_open(&self, url: &str, run_id: Option<&str>) {
        self.activity(
            "browser_open",
            &format!("페이지 열기: {}", url.chars().take(200).collect::<String>()),
            run_id,
        );
    }

    /// Lucia가 다음 대화에서 알 수 있게 남긴다.
    fn episode(&self, text: &str) {
        let store = self.store();
        let conversation = store.notify_conversation();
        let _ = store.add_episode(&NewEpisode {
            at: Utc::now(),
            kind: EpisodeKind::AgentReport,
            origin: Origin::Daemon,
            conversation,
            session_id: None,
            run_id: None,
            body: crate::mask::global().mask(text),
        });
    }

    /// 로그인을 요청한다(사람이 대화 중일 때와 같은 카드). 실제 경로는 [`Self::request_with`]를 쓴다.
    #[cfg(test)]
    pub async fn request(
        &self,
        site_id: &str,
        why: &str,
        run_id: Option<&str>,
    ) -> Result<Requested, String> {
        self.request_with(site_id, why, run_id, None).await
    }

    /// 사람 없는 실행의 요청(결정 0030): 재로그인 카드가 길게 살고, 방해 금지 시간에는 미루고, 승인되면 이어 맡긴다.
    /// `unattended`는 (만료, 이어 하기 정보).
    pub async fn request_with(
        &self,
        site_id: &str,
        why: &str,
        run_id: Option<&str>,
        unattended: Option<(DateTime<Utc>, serde_json::Value)>,
    ) -> Result<Requested, String> {
        let site = self
            .site(site_id)
            .ok_or_else(|| format!("모르는 사이트: {site_id}"))?;
        let now = Utc::now();
        {
            let mut inputs = self.inputs.lock().expect("inputs lock");
            inputs.retain(|_, p| p.expires > now);
            if inputs.values().any(|p| p.site == site.id && !p.done) {
                return Ok(Requested::AlreadyPending);
            }
        }
        if self.pending_approval(&site.id, now) {
            return Ok(Requested::AlreadyPending);
        }
        let username = self.store().setting(&username_key(&site)).ok().flatten();
        if let Some(username) = username
            && self.secrets.exists(&site.credential_name())
        {
            let Some(posts) = &self.approval_posts else {
                return Err("승인 카드를 올릴 수 없다(승인 데스크 없음)".into());
            };
            let id = Uuid::new_v4().simple().to_string();
            let summary = format!("로그인 → {} ({username})", site.display());
            self.store()
                .create_approval(
                    &NewApproval {
                        id: id.clone(),
                        kind: KIND_SITE_LOGIN.into(),
                        payload: json!({
                            "site": site.id,
                            "site_name": site.name,
                            "host": site.hosts.first(),
                            "origin": site.origin,
                            "username": username,
                            "reason": why,
                            "defer_quiet": unattended.is_some(),
                            "resume": unattended.as_ref().map(|u| u.1.clone()),
                        }),
                        summary: summary.clone(),
                        run_id: run_id.map(str::to_owned),
                        expires_at: unattended
                            .as_ref()
                            .map(|u| u.0)
                            .unwrap_or(now + self.cfg.approval_ttl),
                    },
                    now,
                )
                .map_err(|e| format!("승인 요청 저장: {e}"))?;
            self.activity("login_requested", &format!("{summary} 승인 요청"), run_id);
            posts
                .send(id)
                .map_err(|_| "승인 데스크가 멈췄다".to_owned())?;
            return Ok(Requested::ApprovalCard);
        }
        self.ask_input(&site, why, run_id).await
    }

    /// 아이디·비밀번호를 사람에게 청한다: 알림 받을 곳이 자체 앱이면 입력 카드(결정 0063), 아니면 입력 링크.
    async fn ask_input(
        &self,
        site: &SiteProfile,
        why: &str,
        run_id: Option<&str>,
    ) -> Result<Requested, String> {
        let now = Utc::now();
        if self.card_target() {
            let token = self.issue(&site.id, why, true, now);
            if let Err(e) = self.post_input_card(site, why, &token, None, run_id) {
                self.inputs.lock().expect("inputs lock").remove(&token);
                return Err(e);
            }
            self.activity(
                "login_requested",
                &format!("{} 로그인 입력 카드를 올렸다", site.display()),
                run_id,
            );
            return Ok(Requested::InputCard);
        }
        let token = self.issue(&site.id, why, false, now);
        if !self
            .notifier
            .notify(self.input_message(site, why, &token))
            .await
        {
            self.inputs.lock().expect("inputs lock").remove(&token);
            return Err("메신저로 입력 링크를 보내지 못했다".into());
        }
        self.activity(
            "login_requested",
            &format!("{} 로그인 입력 링크를 보냈다", site.display()),
            run_id,
        );
        Ok(Requested::InputLink)
    }

    /// 입력 링크가 아직 쓸 수 있으면 그 사이트(쓰지는 않는다).
    #[cfg(test)]
    pub fn form_peek(&self, token: &str) -> Option<SiteProfile> {
        let inputs = self.inputs.lock().expect("inputs lock");
        let site = inputs
            .get(token)
            .filter(|p| p.expires > Utc::now() && !p.done)?
            .site
            .clone();
        drop(inputs);
        self.site(&site)
    }

    /// 입력 링크의 상태(결과 화면).
    pub fn form_status(&self, token: &str) -> FormStatus {
        let now = Utc::now();
        let (site_id, busy, last, done) = {
            let inputs = self.inputs.lock().expect("inputs lock");
            match inputs.get(token).filter(|p| p.expires > now) {
                None => return FormStatus::Invalid,
                Some(p) => (p.site.clone(), p.busy, p.last.clone(), p.done),
            }
        };
        let Some(site) = self.site(&site_id) else {
            return FormStatus::Invalid;
        };
        if busy {
            return FormStatus::Busy {
                site_name: site.name,
            };
        }
        match (done, last) {
            (true, Some(o)) => FormStatus::Finished(o),
            (_, Some(FormOutcome::Failed { reason, .. })) => FormStatus::Ready {
                site: Box::new(site),
                error: Some(reason),
            },
            _ => FormStatus::Ready {
                site: Box::new(site),
                error: None,
            },
        }
    }

    /// 입력 폼 제출을 받아 로그인을 **요청과 떨어진 작업**으로 돌린다(결정 0038). 브라우저 연결이 끊기거나 다시
    /// 보내져 요청 처리가 버려져도 로그인은 끝까지 하고 결과를 남긴다(결과는 [`Self::form_status`]와 Discord로).
    /// 시작했거나 이미 하는 중·끝났으면 `true`(결과 화면으로 보낸다), 쓸 수 없는 링크면 `false`.
    pub fn spawn_submit(
        self: &Arc<Self>,
        token: &str,
        username: String,
        password: String,
        save: bool,
    ) -> bool {
        let site = match self.claim(token) {
            Ok(site) => site,
            Err(FormOutcome::Invalid) => return false,
            Err(_) => return true,
        };
        if !password.is_empty() {
            crate::mask::global().register(&password);
        }
        let me = self.clone();
        let token = token.to_owned();
        tokio::spawn(async move {
            let run = me.run_submit(&token, &site, &username, &password, save);
            if tokio::time::timeout(SUBMIT_DEADLINE, run).await.is_err() {
                let outcome = me
                    .form_failed(
                        &site,
                        &token,
                        "로그인이 시간 안에 끝나지 않았다(사이트가 느리거나 멈췄다)".into(),
                    )
                    .await;
                me.repost_card(&token, &site, &outcome);
            }
        });
        true
    }

    /// 링크를 잡는다: 쓸 수 있으면 "하는 중"으로 두고 그 사이트, 아니면 지금 상태(없음·하는 중·끝남).
    fn claim(&self, token: &str) -> Result<SiteProfile, FormOutcome> {
        let now = Utc::now();
        let site_id = {
            let mut inputs = self.inputs.lock().expect("inputs lock");
            match inputs.get_mut(token).filter(|p| p.expires > now) {
                None => {
                    inputs.remove(token);
                    return Err(FormOutcome::Invalid);
                }
                Some(p) if p.done => return Err(p.last.clone().unwrap_or(FormOutcome::Invalid)),
                Some(p) if p.busy => {
                    let site_name = self
                        .sites
                        .iter()
                        .find(|s| s.id == p.site)
                        .map_or_else(|| p.site.clone(), |s| s.name.clone());
                    return Err(FormOutcome::Busy { site_name });
                }
                Some(p) => {
                    p.busy = true;
                    p.site.clone()
                }
            }
        };
        match self.site(&site_id) {
            Some(site) => Ok(site),
            None => {
                self.inputs.lock().expect("inputs lock").remove(token);
                Err(FormOutcome::Invalid)
            }
        }
    }

    /// 시도가 끝났다: 결과를 링크에 남긴다(결과 화면). 성공·인증 코드면 이 링크는 끝난다.
    fn finish(&self, token: &str, outcome: &FormOutcome) {
        let mut inputs = self.inputs.lock().expect("inputs lock");
        if let Some(p) = inputs.get_mut(token) {
            p.busy = false;
            p.last = Some(outcome.clone());
            if matches!(
                outcome,
                FormOutcome::Done { .. } | FormOutcome::Pending { .. }
            ) {
                p.done = true;
                p.expires = Utc::now() + self.cfg.input_ttl;
            }
        }
    }

    /// 입력 폼 제출(기다려서 결과를 받는다, 시험용). 실제 화면은 [`Self::spawn_submit`]을 쓴다.
    #[cfg(test)]
    pub async fn form_submit(
        &self,
        token: &str,
        username: &str,
        password: &str,
        save: bool,
    ) -> FormOutcome {
        let site = match self.claim(token) {
            Ok(site) => site,
            Err(o) => return o,
        };
        self.run_submit(token, &site, username, password, save)
            .await
    }

    /// 잡은 링크로 로그인한다. 하는 동안 토큰을 "하는 중"으로 두고, 끝나면 결과를 남긴다(결정 0038).
    async fn run_submit(
        &self,
        token: &str,
        site: &SiteProfile,
        username: &str,
        password: &str,
        save: bool,
    ) -> FormOutcome {
        let outcome = self.attempt(token, site, username, password, save).await;
        self.finish(token, &outcome);
        self.repost_card(token, site, &outcome);
        outcome
    }

    async fn attempt(
        &self,
        token: &str,
        site: &SiteProfile,
        username: &str,
        password: &str,
        save: bool,
    ) -> FormOutcome {
        let site = site.clone();
        // 토큰을 확인한 뒤에야 가림 목록에 넣는다(이 경로에는 로그인이 없으므로, 아무 글이나 가림 목록에
        // 밀어 넣어 Lucia의 말을 망가뜨리지 못하게). 오류 문구에 섞여 나가지 않게 시도하기 전에 넣는다.
        if !password.is_empty() {
            crate::mask::global().register(password);
        }
        let username = username.trim();
        if username.is_empty()
            || password.is_empty()
            || username.chars().count() > USERNAME_MAX
            || password.chars().count() > PASSWORD_MAX
            || username.contains(['\r', '\n'])
        {
            // 시도하지 않았으니 세지 않는다.
            return FormOutcome::Failed {
                reason: "아이디와 비밀번호를 확인해 주세요(빈 칸이나 너무 긴 값)".into(),
                retry_token: self.release(token, false),
            };
        }
        if self.handed_over() {
            return FormOutcome::Failed {
                reason: HANDOVER_BUSY.into(),
                retry_token: self.release(token, false),
            };
        }
        let result = self.browser.login(&site, username, password).await;
        match result {
            Ok(LoginResult::LoggedIn) => {
                let note = if save {
                    match self.save(&site, username, password).await {
                        Ok(()) => "다음부터는 저장된 로그인을 승인만 하면 됩니다.".to_owned(),
                        Err(e) => {
                            tracing::warn!(site = %site.id, err = %crate::mask::global().mask(&e.to_string()), "로그인 저장 실패");
                            "로그인은 됐지만 저장하지 못했습니다.".to_owned()
                        }
                    }
                } else {
                    let _ = self.forget(&site.id).await;
                    "저장하지 않았습니다.".to_owned()
                };
                self.activity("login_ok", &format!("{} 로그인(입력)", site.name), None);
                self.episode(&format!(
                    "[로그인] {}에 로그인했다(사용자 입력). 로그인이 필요해 멈췄던 일을 이어서 할 수 있다.",
                    site.name
                ));
                self.notifier
                    .notify(format!(
                        "✅ {}에 로그인했습니다. {note} 하던 일을 다시 말씀해 주세요.",
                        site.name
                    ))
                    .await;
                FormOutcome::Done {
                    site_name: site.name.clone(),
                    note,
                }
            }
            Ok(LoginResult::NeedsHuman { why }) => {
                // 사람 확인 단계: 비밀번호가 맞았는지는 모른다(틀려도 보안 확인이 먼저 뜬다). 사용자가 저장을 골랐으면
                // 저장해 둔다(틀렸으면 다음 재로그인이 거절되고 입력 링크가 다시 간다).
                let saved = save && self.save(&site, username, password).await.is_ok();
                let note = format!(
                    "{}{}",
                    self.human_step(&site, &why).await,
                    if saved {
                        " 넣은 로그인 정보는 저장했습니다(아직 확인되지 않음)."
                    } else {
                        ""
                    }
                );
                FormOutcome::Pending {
                    site_name: site.name.clone(),
                    note,
                    code_link: Some(self.handover_link()),
                }
            }
            Ok(LoginResult::NeedsCode { hint }) => {
                // 비밀번호는 맞았다(코드 단계로 넘어갔다). 저장은 코드까지 끝난 뒤가 아니라 지금 한다.
                let saved = if save {
                    self.save(&site, username, password).await.is_ok()
                } else {
                    false
                };
                let note = format!(
                    "{}{}",
                    self.request_code(&site, &hint),
                    if saved {
                        " 로그인 정보는 저장했습니다."
                    } else {
                        ""
                    }
                );
                FormOutcome::Pending {
                    site_name: site.name.clone(),
                    note,
                    code_link: self.code_link(),
                }
            }
            Ok(LoginResult::AlreadyLoggedIn) => {
                let note =
                    "이미 로그인돼 있었습니다. 넣은 비밀번호는 확인하지 못해 저장하지 않았습니다."
                        .to_owned();
                self.activity("login_ok", &format!("{} 이미 로그인됨", site.name), None);
                // 카드로 넣었으면 결과를 볼 화면이 없다(카드는 "로그인하고 있습니다"로 닫혔다). 알림으로 알린다.
                let by_card = self
                    .inputs
                    .lock()
                    .expect("inputs lock")
                    .get(token)
                    .is_some_and(|p| p.card);
                if by_card {
                    self.notifier
                        .notify(format!("✅ {}: {note}", site.name))
                        .await;
                }
                FormOutcome::Done {
                    site_name: site.name.clone(),
                    note,
                }
            }
            Ok(LoginResult::Rejected(reason)) => self.form_failed(&site, token, reason).await,
            Err(e) => {
                let reason = crate::mask::global().mask(&e.to_string());
                self.form_failed(&site, token, reason).await
            }
        }
    }

    /// 입력한 로그인이 실패했다: 기록하고, 같은 링크로 다시 넣을 수 있는지 정하고, Discord로도 알린다(제출 화면의
    /// 응답을 놓쳤어도 결과를 알 수 있게).
    async fn form_failed(&self, site: &SiteProfile, token: &str, reason: String) -> FormOutcome {
        self.activity(
            "login_failed",
            &format!("{} 로그인 실패: {reason}", site.name),
            None,
        );
        let retry = self.release(token, true);
        let by_card = self
            .inputs
            .lock()
            .expect("inputs lock")
            .get(token)
            .is_some_and(|p| p.card);
        let next = match &retry {
            Some(_) if by_card => "새로 올린 카드에서 다시 넣을 수 있습니다.".to_owned(),
            Some(_) => "같은 링크에서 다시 넣을 수 있습니다.".to_owned(),
            None => format!(
                "이 링크로 {INPUT_ATTEMPTS}번 시도해 닫았습니다. 다시 하려면 Lucia에게 로그인을 다시 요청해 주세요."
            ),
        };
        self.notifier
            .notify(crate::mask::global().mask(&format!(
                "⚠️ {} 로그인하지 못했습니다: {reason}. {next}",
                site.name
            )))
            .await;
        let outcome = FormOutcome::Failed {
            reason,
            retry_token: retry,
        };
        if let Some(p) = self.inputs.lock().expect("inputs lock").get_mut(token) {
            p.last = Some(outcome.clone());
        }
        outcome
    }

    /// 관리 페이지의 화면 넘겨받기 주소(결정 0017).
    fn handover_link(&self) -> String {
        format!(
            "{}/browser/handover",
            self.cfg.form_base.trim_end_matches('/')
        )
    }

    /// 사람 확인 단계(캡차·추가 보안 확인 등)가 나왔다: 기록하고 Discord로 알린다. 브라우저는 그 화면에 그대로 있다.
    /// 사람에게 보일 안내를 돌려준다.
    async fn human_step(&self, site: &SiteProfile, why: &str) -> String {
        self.activity(
            "login_challenge",
            &format!("{} 로그인: 사람 확인이 필요함({why})", site.name),
            None,
        );
        self.episode(&format!(
            "[로그인] {}이(가) 아이디·비밀번호를 넣은 뒤 사람 확인({why})을 띄웠다. 로그인은 아직 안 됐다. 사용자가 화면 넘겨받기(관리 페이지, 또는 허용한 앱)로 풀어야 한다.",
            site.name
        ));
        // 알림 받을 앱이 화면 넘겨받기를 쓸 수 있으면 그 길을 먼저 알린다(결정 0063).
        let where_ = if crate::gateway::notify_app_can_handover(&self.store()) {
            "앱의 브라우저 화면 넘겨받기(또는 관리 페이지 \"브라우저 넘겨받기\")에서"
        } else {
            "관리 페이지 \"브라우저 넘겨받기\"에서"
        };
        let note = format!(
            "{}이(가) 사람 확인({why})을 띄워 아직 로그인되지 않았습니다. 10분 안에 {where_} 그 화면을 직접 풀어 주세요(아이디·비밀번호는 넣어 두었습니다. 브라우저를 한동안 쓰지 않으면 닫혀 그 화면이 사라집니다): {}",
            site.name,
            self.handover_link()
        );
        self.notifier
            .notify(crate::mask::global().mask(&format!("🧩 {note}")))
            .await;
        note
    }

    /// 하던 시도를 끝낸다. `counted`면 실패 수를 센다. 다시 쓸 수 있으면 그 토큰(만료는 지금부터 다시).
    fn release(&self, token: &str, counted: bool) -> Option<String> {
        let now = Utc::now();
        let mut inputs = self.inputs.lock().expect("inputs lock");
        let p = inputs.get_mut(token)?;
        p.busy = false;
        if counted {
            p.attempts += 1;
        }
        p.expires = now + self.cfg.input_ttl;
        if p.attempts >= INPUT_ATTEMPTS {
            // 결과 화면에 "닫았다"가 보이도록 남겨 둔다(넣을 수는 없다).
            p.done = true;
            return None;
        }
        Some(token.to_owned())
    }

    async fn save(&self, site: &SiteProfile, username: &str, password: &str) -> anyhow::Result<()> {
        // JSON으로 감싸 저장한다(저장소가 앞뒤 공백을 다듬으므로 비밀번호를 그대로 지키려고).
        self.secrets
            .write(
                &site.credential_name(),
                &json!({ "password": password }).to_string(),
            )
            .await?;
        self.store()
            .set_setting(&username_key(site), username, Utc::now())?;
        Ok(())
    }

    /// 저장된 로그인을 지운다(관리 페이지, 또는 저장하지 않기로 했을 때).
    pub async fn forget(&self, site_id: &str) -> anyhow::Result<()> {
        let site = self
            .site(site_id)
            .ok_or_else(|| anyhow::anyhow!("모르는 사이트: {site_id}"))?;
        if self.secrets.exists(&site.credential_name()) {
            self.secrets.remove(&site.credential_name()).await?;
        }
        self.store().delete_setting(&username_key(&site))?;
        if !site.is_builtin() {
            self.store().delete_setting(&login_url_key(&site.id))?;
        }
        Ok(())
    }

    /// 이 사이트에 저장된 로그인(아이디와 비밀번호)이 있는가.
    pub fn has_saved(&self, site_id: &str) -> bool {
        let Some(site) = self.site(site_id) else {
            return false;
        };
        self.store()
            .setting(&username_key(&site))
            .ok()
            .flatten()
            .is_some()
            && self.secrets.exists(&site.credential_name())
    }

    /// 승인된 재로그인: 저장된 로그인으로 한 번 시도한다. 사이트가 받지 않으면 입력 링크를 새로 보낸다.
    pub async fn login_saved(&self, site_id: &str) -> Result<SavedLogin, String> {
        self.login_saved_as(site_id, None).await
    }

    /// 위임된 재로그인(결정 0028·0037): 사람이 카드를 누르지 않고, 로그인을 위임한 사이트라서 한다.
    /// 기록에 그렇게 남긴다(`by`: 무엇이 다시 로그인했나, 예: "페이지 감시").
    pub async fn login_delegated(&self, site_id: &str, by: &str) -> Result<SavedLogin, String> {
        self.login_saved_as(site_id, Some(by)).await
    }

    async fn login_saved_as(
        &self,
        site_id: &str,
        delegated: Option<&str>,
    ) -> Result<SavedLogin, String> {
        if self.handed_over() {
            return Err(HANDOVER_BUSY.into());
        }
        let site = self
            .site(site_id)
            .ok_or_else(|| format!("모르는 사이트: {site_id}"))?;
        let username = self
            .store()
            .setting(&username_key(&site))
            .ok()
            .flatten()
            .ok_or("저장된 아이디가 없다")?;
        let raw = self
            .secrets
            .read(&site.credential_name())
            .await
            .map_err(|e| crate::mask::global().mask(&e.to_string()))?
            .ok_or("저장된 비밀번호가 없다")?;
        let password = serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v["password"].as_str().map(str::to_owned))
            .ok_or("저장된 비밀번호를 읽을 수 없다")?;
        crate::mask::global().register(&password);
        match self.browser.login(&site, &username, &password).await {
            Ok(LoginResult::LoggedIn) => {
                let how = match delegated {
                    Some(by) => format!("저장된 로그인, 사람에게 묻지 않고: {by}"),
                    None => "저장된 로그인, 사용자 승인".to_owned(),
                };
                self.activity(
                    if delegated.is_some() {
                        "login_delegated"
                    } else {
                        "login_ok"
                    },
                    &format!("{} 로그인({how})", site.name),
                    None,
                );
                self.episode(&format!(
                    "[로그인] {}에 다시 로그인했다({how}). 멈췄던 일을 이어서 할 수 있다.",
                    site.name
                ));
                Ok(SavedLogin::LoggedIn(
                    "로그인했습니다. 하던 일을 다시 말씀해 주세요.".into(),
                ))
            }
            Ok(LoginResult::AlreadyLoggedIn) => {
                Ok(SavedLogin::LoggedIn("이미 로그인돼 있었습니다.".into()))
            }
            Ok(LoginResult::NeedsHuman { why }) => Err(self.human_step(&site, &why).await),
            Ok(LoginResult::NeedsCode { hint }) => {
                Ok(SavedLogin::NeedsCode(self.request_code(&site, &hint)))
            }
            Ok(LoginResult::Rejected(reason)) => {
                self.activity(
                    "login_failed",
                    &format!("{} 저장된 로그인 거절: {reason}", site.name),
                    None,
                );
                let _ = self
                    .ask_input(
                        &site,
                        "저장된 비밀번호로 로그인하지 못했습니다. 다시 넣어 주세요.",
                        None,
                    )
                    .await;
                Err(format!("사이트가 받지 않았습니다: {reason}"))
            }
            Err(e) => Err(crate::mask::global().mask(&e.to_string())),
        }
    }

    /// 시험용: 입력 링크의 만료를 앞당긴다.
    #[cfg(test)]
    fn expire_all(&self) {
        for p in self.inputs.lock().unwrap().values_mut() {
            p.expires = Utc::now() - Duration::seconds(1);
        }
    }
}

impl SiteLoginExec for SiteLogins {
    fn login_saved<'a>(&'a self, site: &'a str) -> BoxFut<'a, Result<SavedLogin, String>> {
        Box::pin(SiteLogins::login_saved(self, site))
    }

    fn submit_code<'a>(
        &'a self,
        site: &'a str,
        code: &'a str,
    ) -> BoxFut<'a, Result<String, String>> {
        Box::pin(SiteLogins::submit_code(self, site, code))
    }

    fn submit_input(
        self: Arc<Self>,
        token: &str,
        username: String,
        password: String,
        save: bool,
    ) -> Result<String, String> {
        SiteLogins::submit_input(&self, token, username, password, save)
    }

    fn cancel_input(&self, token: &str) {
        SiteLogins::cancel_input(self, token);
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::{FakeBrowser, FakeNotifier, FakeSecrets};
    use super::*;
    use crate::browser::BrowserError;

    struct Setup {
        logins: Arc<SiteLogins>,
        browser: Arc<FakeBrowser>,
        secrets: Arc<FakeSecrets>,
        notes: Arc<FakeNotifier>,
        posts: mpsc::UnboundedReceiver<String>,
        store: Arc<Mutex<Store>>,
    }

    fn setup() -> Setup {
        let browser = Arc::new(FakeBrowser::default());
        let secrets = Arc::new(FakeSecrets::default());
        let notes = Arc::new(FakeNotifier::default());
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let (tx, rx) = mpsc::unbounded_channel();
        let logins = SiteLogins::new(
            browser.clone(),
            secrets.clone(),
            store.clone(),
            notes.clone(),
            Some(tx),
            crate::browser::sites::builtin(),
            LoginsConfig {
                form_base: "http://192.168.0.2:8780/".into(),
                input_ttl: Duration::minutes(10),
                approval_ttl: Duration::minutes(30),
            },
        );
        Setup {
            logins: Arc::new(logins),
            browser,
            secrets,
            notes,
            posts: rx,
            store,
        }
    }

    fn token_from(note: &str) -> String {
        note.rsplit("/site-login/")
            .next()
            .unwrap()
            .trim()
            .to_owned()
    }

    #[tokio::test]
    async fn first_login_goes_through_a_one_time_link_and_is_saved() {
        let s = setup();
        let r = s
            .logins
            .request(
                "quasarzone",
                "페이지를 열다가 로그인이 필요했다.",
                Some("chat-1"),
            )
            .await
            .unwrap();
        assert_eq!(r, Requested::InputLink);
        let note = s.notes.0.lock().unwrap()[0].clone();
        assert!(
            note.contains("http://192.168.0.2:8780/site-login/"),
            "{note}"
        );
        assert!(note.contains("퀘이사존"));
        assert_eq!(
            s.logins.request("quasarzone", "다시", None).await.unwrap(),
            Requested::AlreadyPending,
            "이미 보낸 링크가 살아 있으면 다시 보내지 않는다"
        );

        let token = token_from(&note);
        assert_eq!(s.logins.form_peek(&token).unwrap().id, "quasarzone");
        let out = s
            .logins
            .form_submit(&token, " me ", "pa ss-word ", true)
            .await;
        assert!(matches!(out, FormOutcome::Done { .. }), "{out:?}");
        assert_eq!(
            *s.browser.passwords.lock().unwrap(),
            ["pa ss-word "],
            "비밀번호는 그대로(공백 포함)"
        );
        assert_eq!(
            s.browser.calls.lock().unwrap()[0].1,
            "quasarzone:me",
            "아이디는 앞뒤 공백만 다듬는다"
        );
        let saved = s.secrets.0.lock().unwrap()["site-quasarzone"].clone();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&saved).unwrap()["password"],
            "pa ss-word ",
            "저장소가 다듬어도 비밀번호는 지켜진다"
        );
        assert_eq!(
            s.logins.status()[0],
            SiteStatus {
                id: "quasarzone".into(),
                name: "퀘이사존".into(),
                origin: "https://quasarzone.com".into(),
                saved: true,
                username: Some("me".into()),
            }
        );
        assert!(
            s.notes.0.lock().unwrap()[1].contains("로그인했습니다"),
            "끝나면 알린다"
        );
        let logins_before = s.browser.calls.lock().unwrap().len();
        assert!(
            matches!(
                s.logins.form_submit(&token, "me", "x", true).await,
                FormOutcome::Done { .. }
            ),
            "끝난 링크는 결과만 보인다"
        );
        assert_eq!(
            s.browser.calls.lock().unwrap().len(),
            logins_before,
            "다시 로그인하지 않는다"
        );
        assert!(matches!(
            s.logins.form_status(&token),
            FormStatus::Finished(FormOutcome::Done { .. })
        ));
        let store = s.store.lock().unwrap();
        assert!(
            !store.search_episodes("로그인했다", 5).unwrap().is_empty(),
            "Lucia가 알 수 있게 남긴다"
        );
        let activity = store.recent_activity(10).unwrap();
        assert!(
            activity.iter().all(|a| !a.summary.contains("pa ss-word")),
            "활동 기록에 비밀번호 없음"
        );
    }

    #[tokio::test]
    async fn saved_login_needs_approval_and_a_rejected_one_asks_again() {
        let mut s = setup();
        let token = {
            s.logins.request("quasarzone", "처음", None).await.unwrap();
            token_from(&s.notes.0.lock().unwrap()[0])
        };
        s.logins.form_submit(&token, "me", "secret-1", true).await;

        let r = s
            .logins
            .request("quasarzone", "세션이 끝났다.", Some("chat-2"))
            .await
            .unwrap();
        assert_eq!(r, Requested::ApprovalCard);
        let id = s.posts.try_recv().unwrap();
        let approval = s.store.lock().unwrap().approval(&id).unwrap().unwrap();
        assert_eq!(approval.kind, KIND_SITE_LOGIN);
        assert_eq!(approval.payload["site"], "quasarzone");
        assert_eq!(approval.payload["username"], "me");
        assert!(
            !approval.payload.to_string().contains("secret-1"),
            "승인 요청에 비밀번호 없음"
        );
        assert_eq!(
            s.logins.request("quasarzone", "또", None).await.unwrap(),
            Requested::AlreadyPending
        );

        assert!(s.logins.login_saved("quasarzone").await.is_ok());
        assert_eq!(s.browser.passwords.lock().unwrap()[1], "secret-1");

        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::Rejected("비밀번호가 다릅니다".into())));
        let err = s.logins.login_saved("quasarzone").await.unwrap_err();
        assert!(err.contains("비밀번호가 다릅니다"), "{err}");
        let last = s.notes.0.lock().unwrap().last().cloned().unwrap();
        assert!(
            last.contains("/site-login/") && last.contains("다시 넣어"),
            "거절되면 새 입력 링크: {last}"
        );
    }

    /// 비밀번호 뒤 사람 확인(캡차 등): 성공으로 치지 않고, 넘겨받기 링크와 함께 Discord로 알린다.
    #[tokio::test]
    async fn human_checks_are_handed_over_not_counted_as_logged_in() {
        let s = setup();
        s.logins.request("quasarzone", "처음", None).await.unwrap();
        let token = token_from(&s.notes.0.lock().unwrap()[0]);
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::NeedsHuman {
                why: "security verification".into(),
            }));
        let out = s.logins.form_submit(&token, "me", "pw-1", false).await;
        let FormOutcome::Pending {
            note, code_link, ..
        } = out
        else {
            panic!("{out:?}");
        };
        assert!(
            note.contains("사람 확인") && note.contains("아직 로그인되지 않았습니다"),
            "{note}"
        );
        assert!(code_link.unwrap().ends_with("/browser/handover"));
        assert!(
            s.notes.0.lock().unwrap().last().unwrap().contains("🧩"),
            "Discord로도 알린다"
        );
        assert!(
            !s.store
                .lock()
                .unwrap()
                .recent_activity(10)
                .unwrap()
                .iter()
                .any(|a| a.kind == "login_ok"),
            "로그인함으로 기록하지 않는다"
        );
    }

    /// 실패해도 같은 링크로 3번까지 다시 넣는다(Discord 링크를 다시 열어도 된다). 하는 중에 또 오면 기다리라고 하고,
    /// 그동안 Lucia가 로그인을 다시 요청해도 새 링크를 보내지 않는다. 실패는 Discord로도 알린다. 저장은 안 한다.
    #[tokio::test]
    async fn failures_keep_the_same_link_and_nothing_is_saved() {
        let s = setup();
        s.logins.request("quasarzone", "처음", None).await.unwrap();
        let token = token_from(&s.notes.0.lock().unwrap()[0]);
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::Rejected("아이디 없음".into())));
        let out = s.logins.form_submit(&token, "me", "pw-1", true).await;
        let FormOutcome::Failed {
            reason,
            retry_token,
        } = out
        else {
            panic!("{out:?}");
        };
        assert_eq!(reason, "아이디 없음");
        assert_eq!(retry_token.as_deref(), Some(token.as_str()), "같은 링크");
        assert!(s.logins.form_peek(&token).is_some());
        assert!(
            s.notes
                .0
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .contains("같은 링크에서 다시"),
            "실패는 Discord로도"
        );
        assert!(
            s.secrets.0.lock().unwrap().is_empty(),
            "실패하면 저장 안 함"
        );
        assert_eq!(
            s.logins.request("quasarzone", "또", None).await.unwrap(),
            Requested::AlreadyPending,
            "링크가 살아 있으면 새 링크를 보내지 않는다"
        );
        // 빈 칸은 시도로 세지 않는다.
        assert!(matches!(
            s.logins.form_submit(&token, "me", "", true).await,
            FormOutcome::Failed {
                retry_token: Some(_),
                ..
            }
        ));
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Err(BrowserError::Unavailable("샌드박스 없음".into())));
        assert!(matches!(
            s.logins.form_submit(&token, "me", "pw-2", true).await,
            FormOutcome::Failed {
                retry_token: Some(_),
                ..
            }
        ));
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::Rejected("또 틀림".into())));
        assert!(
            matches!(
                s.logins.form_submit(&token, "me", "pw-3", true).await,
                FormOutcome::Failed {
                    retry_token: None,
                    ..
                }
            ),
            "세 번째 실패면 닫는다"
        );
        assert!(
            matches!(
                s.logins.form_status(&token),
                FormStatus::Finished(FormOutcome::Failed {
                    retry_token: None,
                    ..
                })
            ),
            "닫힌 링크는 결과만 보이고 넣을 수 없다"
        );
        assert_eq!(
            s.logins.request("quasarzone", "다시", None).await.unwrap(),
            Requested::InputLink,
            "닫힌 링크는 기다리는 요청이 아니다"
        );

        // 떨어진 작업(결정 0038): 요청을 기다리지 않아도 로그인은 끝나고 결과가 남는다.
        let detached = s.logins.issue("quasarzone", "시험", false, Utc::now());
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::Rejected("떨어진 작업 실패".into())));
        assert!(
            s.logins
                .spawn_submit(&detached, "me".into(), "pw-5".into(), false)
        );
        let mut status = s.logins.form_status(&detached);
        for _ in 0..200 {
            if !matches!(status, FormStatus::Busy { .. }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            status = s.logins.form_status(&detached);
        }
        assert!(
            matches!(&status, FormStatus::Ready { error: Some(e), .. } if e.contains("떨어진 작업 실패")),
            "{status:?}"
        );
        assert!(
            !s.logins
                .spawn_submit("없는토큰", "me".into(), "pw".into(), false)
        );

        // 하는 중에 같은 링크가 또 오면 기다리라고 한다.
        let busy = s.logins.issue("quasarzone", "시험", false, Utc::now());
        s.logins.inputs.lock().unwrap().get_mut(&busy).unwrap().busy = true;
        assert!(matches!(
            s.logins.form_submit(&busy, "me", "pw", true).await,
            FormOutcome::Busy { .. }
        ));
        s.logins.inputs.lock().unwrap().clear();

        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::AlreadyLoggedIn));
        s.logins.expire_all();
        let fresh = s.logins.issue("quasarzone", "시험", false, Utc::now());
        assert!(matches!(
            s.logins.form_submit(&fresh, "me", "pw-3", true).await,
            FormOutcome::Done { .. }
        ));
        assert!(
            s.secrets.0.lock().unwrap().is_empty(),
            "이미 로그인돼 있었으면 확인 안 된 비밀번호는 저장하지 않는다"
        );

        let stale = s.logins.issue("quasarzone", "시험", false, Utc::now());
        s.logins.expire_all();
        assert_eq!(
            s.logins.form_submit(&stale, "me", "pw", true).await,
            FormOutcome::Invalid,
            "만료"
        );
        assert!(s.logins.form_peek(&stale).is_none());
    }

    /// 실제 비밀 저장소(systemd-creds)로 사이트 자격 증명 이름과 앞뒤 공백 있는 비밀번호가 그대로 오간다.
    #[tokio::test]
    async fn real_credentials_keep_the_password_exactly() {
        if std::process::Command::new("systemd-creds")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("systemd-creds 없음, 건너뜀");
            return;
        }
        let dir = std::env::temp_dir().join(format!("lucia-site-creds-{}", Uuid::new_v4()));
        let creds =
            crate::credentials::Credentials::new(dir.clone(), crate::mask::Masker::default());
        let store: &dyn SecretStore = &creds;
        let site = &crate::browser::sites::builtin()[0];
        let value = json!({ "password": "  edge spaces pw-7  " }).to_string();
        store.write(&site.credential_name(), &value).await.unwrap();
        assert!(store.exists("site-quasarzone"));
        let back = store.read("site-quasarzone").await.unwrap().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&back).unwrap()["password"],
            "  edge spaces pw-7  "
        );
        store.remove("site-quasarzone").await.unwrap();
        assert!(!store.exists("site-quasarzone"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn generic_sites_are_bound_to_their_https_origin() {
        let s = setup();
        let url = Url::parse("https://www.example.com/member/login?next=%2F").unwrap();
        let site = s.logins.profile_for(&url).unwrap();
        assert_eq!(site.id, "www.example.com");
        assert!(
            s.logins
                .profile_for(&Url::parse("https://img2.quasarzone.com/x").unwrap())
                .unwrap()
                .is_builtin(),
            "내장 사이트의 주소는 내장 프로필"
        );
        s.logins.remember_login_url(&site);
        assert_eq!(
            s.logins.site("www.example.com").unwrap().login.url,
            "https://www.example.com/member/login?next=%2F",
            "로그인 페이지 주소를 기억한다"
        );
        assert!(
            s.logins.site("Www.Example.com").is_none(),
            "다듬은 호스트만"
        );
        assert!(s.logins.site("localhost").is_none());

        s.logins
            .request("www.example.com", "Lucia가 로그인을 요청했습니다.", None)
            .await
            .unwrap();
        let note = s.notes.0.lock().unwrap()[0].clone();
        assert!(
            note.contains("https://www.example.com"),
            "출처를 보인다: {note}"
        );
        let token = token_from(&note);
        assert_eq!(
            s.logins.form_peek(&token).unwrap().origin,
            "https://www.example.com"
        );
        let out = s.logins.form_submit(&token, "me", "pw-9", true).await;
        assert!(matches!(out, FormOutcome::Done { .. }), "{out:?}");
        assert_eq!(s.browser.calls.lock().unwrap()[0].1, "www.example.com:me");
        let cred = site.credential_name();
        assert!(s.secrets.0.lock().unwrap().contains_key(&cred), "{cred}");
        let status = s.logins.status();
        let entry = status.iter().find(|x| x.id == "www.example.com").unwrap();
        assert!(entry.saved && entry.username.as_deref() == Some("me"));

        s.logins.forget("www.example.com").await.unwrap();
        assert!(!s.secrets.0.lock().unwrap().contains_key(&cred));
        assert!(
            !s.logins.status().iter().any(|x| x.id == "www.example.com"),
            "지우면 목록에서도 빠진다"
        );
    }

    #[tokio::test]
    async fn forget_removes_the_saved_login() {
        let s = setup();
        s.logins.request("quasarzone", "처음", None).await.unwrap();
        let token = token_from(&s.notes.0.lock().unwrap()[0]);
        s.logins.form_submit(&token, "me", "pw", true).await;
        assert!(s.logins.status()[0].saved);
        s.logins.forget("quasarzone").await.unwrap();
        assert!(!s.logins.status()[0].saved);
        assert_eq!(s.logins.status()[0].username, None);
        assert!(s.logins.request("nope", "x", None).await.is_err());
    }

    /// 결정 0017: 비밀번호 뒤 인증 코드가 필요하면 카드와 웹 링크를 주고, 기다리는 동안 브라우저를 붙잡는다.
    /// 코드는 한 번만, 그 사이트에만 넣는다.
    #[tokio::test]
    async fn code_step_waits_for_one_code_and_holds_the_browser() {
        let mut s = setup();
        s.logins.request("quasarzone", "시험", None).await.unwrap();
        let token = token_from(&s.notes.0.lock().unwrap()[0]);
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::NeedsCode {
                hint: "인증 번호".into(),
            }));
        let out = s.logins.form_submit(&token, "me", "pw", false).await;
        let FormOutcome::Pending {
            note, code_link, ..
        } = out
        else {
            panic!("{out:?}");
        };
        assert!(note.contains("인증 코드") && note.contains("카드의 [코드 입력]"));
        let link = code_link.unwrap();
        assert!(link.contains("/site-login/code/"));
        assert!(
            s.logins.code_pending(),
            "코드를 기다리는 동안 브라우저를 붙잡는다"
        );
        let id = s.posts.try_recv().unwrap();
        let card = s.store.lock().unwrap().approval(&id).unwrap().unwrap();
        assert_eq!(card.kind, KIND_LOGIN_CODE);
        assert!(
            card.payload["link"]
                .as_str()
                .unwrap()
                .ends_with(link.rsplit('/').next().unwrap())
        );

        assert!(
            s.logins.submit_code("other-site", "123456").await.is_err(),
            "다른 사이트에는 넣지 않는다"
        );
        // 다른 사이트로 시도하면 기다림이 끝난다(한 번만).
        assert!(!s.logins.code_pending());
        assert!(s.logins.submit_code("quasarzone", "123456").await.is_err());
        assert!(
            !s.browser
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|(k, _)| k == "code")
        );
    }

    #[tokio::test]
    async fn code_from_the_web_link_logs_in() {
        let s = setup();
        s.logins.request("quasarzone", "시험", None).await.unwrap();
        let token = token_from(&s.notes.0.lock().unwrap()[0]);
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::NeedsCode {
                hint: "인증 번호".into(),
            }));
        let FormOutcome::Pending { code_link, .. } =
            s.logins.form_submit(&token, "me", "pw", false).await
        else {
            panic!()
        };
        let code_token = code_link.unwrap().rsplit('/').next().unwrap().to_owned();
        assert!(s.logins.code_form_peek(&code_token).is_some());
        let done = s.logins.code_form_submit(&code_token, " 654321 ").await;
        assert!(done.is_ok(), "{done:?}");
        assert!(
            s.browser
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|(k, v)| k == "code" && v == "quasarzone")
        );
        assert_eq!(
            s.browser.passwords.lock().unwrap().last().unwrap(),
            "654321"
        );
        assert!(!s.logins.code_pending());
        assert!(
            s.logins.code_form_peek(&code_token).is_none(),
            "링크는 한 번만"
        );
    }

    /// 저장된 로그인이 인증 코드 단계로 넘어가면 "로그인함"이 아니라 코드 대기다. 넘겨받는 중이면 하지 않는다.
    #[tokio::test]
    async fn saved_login_waiting_for_a_code_is_not_logged_in() {
        let s = setup();
        s.logins
            .save(&s.logins.site("quasarzone").unwrap(), "me", "pw")
            .await
            .unwrap();
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::NeedsCode {
                hint: "인증 번호".into(),
            }));
        let out = s.logins.login_saved("quasarzone").await.unwrap();
        assert!(
            matches!(out, SavedLogin::NeedsCode(ref n) if n.contains("인증 코드")),
            "{out:?}"
        );
        assert!(s.logins.code_pending());

        s.logins.set_handover(true);
        assert!(s.logins.login_saved("quasarzone").await.is_err());
        assert!(s.logins.submit_code("quasarzone", "1").await.is_err());
        s.logins.set_handover(false);
    }

    // ---------- 결정 0063: 앱 카드로 받는 로그인 정보 ----------

    /// 알림 받을 곳이 자체 앱인 설정(앱이 화면 넘겨받기 범위를 가졌는지는 `scopes`).
    fn app_setup_with(scopes: &[&str]) -> Setup {
        let s = setup();
        {
            let st = s.store.lock().unwrap();
            st.create_client("cl-1", "앱", "app", "h1", Utc::now())
                .unwrap();
            st.set_client_scopes(
                "cl-1",
                &scopes.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>(),
            )
            .unwrap();
            st.set_setting(crate::gateway::keys::NOTIFY_CLIENT, "cl-1", Utc::now())
                .unwrap();
        }
        s
    }

    fn app_setup() -> Setup {
        app_setup_with(&[])
    }

    /// 카드 제출(`SiteLoginExec`와 이름이 겹쳐 `Arc`를 옮기지 않도록 고유 메서드를 곧장 부른다).
    fn submit_card(
        s: &Setup,
        token: &str,
        username: String,
        password: String,
        save: bool,
    ) -> Result<String, String> {
        // 실제 데스크처럼 누른 카드의 승인을 먼저 닫는다(열린 카드가 있으면 새 카드를 올리지 않는다).
        {
            let store = s.store.lock().unwrap();
            for a in store.recent_approvals(50).unwrap() {
                if a.kind == KIND_LOGIN_INPUT
                    && a.status == ApprovalStatus::Pending
                    && a.payload["token"].as_str() == Some(token)
                {
                    store.decide_approval(&a.id, true, Utc::now()).unwrap();
                }
            }
        }
        SiteLogins::submit_input(&s.logins, token, username, password, save)
    }

    /// 데스크가 받을 다음 카드(승인 요청)를 꺼낸다.
    async fn next_card(s: &mut Setup) -> lucia_store::Approval {
        let id = tokio::time::timeout(std::time::Duration::from_secs(5), s.posts.recv())
            .await
            .expect("카드가 올라오지 않았다")
            .expect("승인 데스크 통로가 닫혔다");
        s.store.lock().unwrap().approval(&id).unwrap().unwrap()
    }

    /// 떨어진 로그인이 끝날 때까지 기다린다(`spawn_submit`은 바로 "하는 중"으로 두고 돌아온다).
    async fn wait_idle(s: &Setup, token: &str) -> FormStatus {
        for _ in 0..400 {
            let status = s.logins.form_status(token);
            if !matches!(status, FormStatus::Busy { .. }) {
                return status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("로그인이 시간 안에 끝나지 않았다");
    }

    fn input_cards(s: &Setup) -> Vec<lucia_store::Approval> {
        s.store
            .lock()
            .unwrap()
            .recent_approvals(100)
            .unwrap()
            .into_iter()
            .filter(|a| a.kind == KIND_LOGIN_INPUT)
            .collect()
    }

    /// 저장소 어디에도(승인 요청·결과·활동·에피소드) 이 값이 없다. 먼저 뒤질 곳이 비어 있지 않음을 확인한다
    /// (에피소드는 로그인에 성공했거나 사람 확인이 떴을 때만 남는다: `expect_episode`).
    fn assert_absent_from_store(s: &Setup, secret: &str, expect_episode: bool) {
        let store = s.store.lock().unwrap();
        let approvals = store.recent_approvals(100).unwrap();
        assert!(!approvals.is_empty());
        for a in &approvals {
            let all = serde_json::to_string(a).unwrap();
            assert!(!all.contains(secret), "승인 요청에 비밀번호가 있다: {all}");
        }
        let activity = store.recent_activity(100).unwrap();
        assert!(!activity.is_empty());
        for a in &activity {
            assert!(
                !a.summary.contains(secret),
                "활동 기록에 비밀번호가 있다: {}",
                a.summary
            );
        }
        let episodes = store.search_episodes("로그인", 50).unwrap();
        assert!(
            !expect_episode || !episodes.is_empty(),
            "에피소드가 있어야 뒤진 것이다"
        );
        for e in &episodes {
            assert!(!e.body.contains(secret), "에피소드에 비밀번호가 있다");
        }
    }

    /// 알림 받을 곳이 앱이면 입력 링크 글 대신 승인 카드(로그인 입력)를 올리고, 링크는 카드 본문용으로 실린다.
    #[tokio::test]
    async fn app_notify_target_gets_an_input_card_instead_of_link_text() {
        let mut s = app_setup();
        let r = s
            .logins
            .request(
                "quasarzone",
                "페이지를 열다가 로그인이 필요했다.",
                Some("chat-1"),
            )
            .await
            .unwrap();
        assert_eq!(r, Requested::InputCard);
        assert!(
            s.notes.0.lock().unwrap().is_empty(),
            "링크 글을 따로 보내지 않는다"
        );
        let card = next_card(&mut s).await;
        assert_eq!(card.kind, KIND_LOGIN_INPUT);
        assert_eq!(card.run_id.as_deref(), Some("chat-1"));
        assert_eq!(card.status, ApprovalStatus::Pending);
        let p = &card.payload;
        assert_eq!(p["site"], "quasarzone");
        assert_eq!(p["site_name"], "퀘이사존");
        assert_eq!(p["origin"], "https://quasarzone.com");
        assert_eq!(p["reason"], "페이지를 열다가 로그인이 필요했다.");
        assert_eq!(p["attempts"], INPUT_ATTEMPTS);
        assert!(p["error"].is_null(), "처음 카드에는 앞선 시도가 없다");
        let token = p["token"].as_str().unwrap().to_owned();
        assert_eq!(token.len(), 64);
        assert_eq!(
            p["link"].as_str().unwrap(),
            format!("http://192.168.0.2:8780/site-login/{token}"),
            "링크는 같은 토큰으로 끝난다"
        );
        assert!(
            card.expires_at > Utc::now() + Duration::minutes(9),
            "입력 링크와 같은 시간"
        );
        // 카드의 토큰은 링크의 토큰이다: 같은 요청이 열려 있다.
        assert_eq!(s.logins.form_peek(&token).unwrap().id, "quasarzone");
        assert_eq!(
            s.logins.request("quasarzone", "다시", None).await.unwrap(),
            Requested::AlreadyPending
        );
        assert!(s.posts.try_recv().is_err(), "카드를 또 올리지 않는다");
        assert!(
            s.store
                .lock()
                .unwrap()
                .recent_activity(10)
                .unwrap()
                .iter()
                .any(|a| a.kind == "login_requested" && a.summary.contains("입력 카드")),
            "카드를 올렸다고 남긴다"
        );
    }

    /// 앱 카드에서 받은 값으로 로그인하면 떨어진 작업으로 돌고 결과가 알림으로 간다. 비밀번호는 저장소 어디에도
    /// 남지 않는다.
    #[tokio::test]
    async fn submit_input_logs_in_detached_and_the_password_never_reaches_the_store() {
        let mut s = app_setup();
        let secret = "pw-시험-카드-7391";
        s.logins.request("quasarzone", "시험", None).await.unwrap();
        let token = next_card(&mut s).await.payload["token"]
            .as_str()
            .unwrap()
            .to_owned();
        let note = submit_card(&s, &token, "me_1".into(), secret.into(), true).unwrap();
        assert!(note.contains("로그인하고 있습니다"), "{note}");
        let status = wait_idle(&s, &token).await;
        assert!(
            matches!(&status, FormStatus::Finished(FormOutcome::Done { .. })),
            "{status:?}"
        );
        assert_eq!(*s.browser.passwords.lock().unwrap(), [secret]);
        assert_eq!(s.browser.calls.lock().unwrap()[0].1, "quasarzone:me_1");
        let notes = s.notes.0.lock().unwrap().clone();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].starts_with("✅") && notes[0].contains("퀘이사존"),
            "{notes:?}"
        );
        assert!(notes.iter().all(|n| !n.contains(secret)));
        assert!(
            s.secrets.0.lock().unwrap().contains_key("site-quasarzone"),
            "저장을 골랐으니 비밀 저장소에는 있다(저장소 밖에는 없다)"
        );
        assert_eq!(s.logins.status()[0].username.as_deref(), Some("me_1"));
        assert_absent_from_store(&s, secret, true);
        // 같은 카드의 토큰은 한 번만 쓴다.
        assert!(
            submit_card(&s, &token, "me_1".into(), secret.into(), true)
                .unwrap_err()
                .contains("이미 끝난 요청")
        );
        assert_eq!(
            s.browser.calls.lock().unwrap().len(),
            1,
            "다시 로그인하지 않는다"
        );
    }

    /// 저장하지 않기를 고르면 저장하지 않는다(체크 칸 `save`).
    #[tokio::test]
    async fn submit_input_without_save_keeps_nothing() {
        let mut s = app_setup();
        s.logins.request("quasarzone", "시험", None).await.unwrap();
        let token = next_card(&mut s).await.payload["token"]
            .as_str()
            .unwrap()
            .to_owned();
        submit_card(
            &s,
            &token,
            "me_1".into(),
            "pw-시험-저장안함-1180".into(),
            false,
        )
        .unwrap();
        assert!(matches!(
            wait_idle(&s, &token).await,
            FormStatus::Finished(FormOutcome::Done { .. })
        ));
        assert!(s.secrets.0.lock().unwrap().is_empty());
        assert!(!s.logins.status()[0].saved);
    }

    /// 거절당하면 같은 토큰으로 새 카드를 올린다(앞선 시도의 까닭이 실린다). 시도 상한(3번)이면 더는 올리지 않고,
    /// 닫힌 요청에는 넣을 수 없다. 카드로 받은 요청은 "새로 올린 카드"라고 안내한다.
    #[tokio::test]
    async fn rejected_input_posts_a_fresh_card_until_the_attempts_run_out() {
        let mut s = app_setup();
        s.logins.request("quasarzone", "시험", None).await.unwrap();
        let first = next_card(&mut s).await;
        let token = first.payload["token"].as_str().unwrap().to_owned();
        let link = first.payload["link"].as_str().unwrap().to_owned();
        for attempt in 1..=INPUT_ATTEMPTS {
            let reason = format!("비밀번호가 다릅니다({attempt})");
            s.browser
                .logins
                .lock()
                .unwrap()
                .push_back(Ok(LoginResult::Rejected(reason.clone())));
            let secret = format!("pw-시험-틀림-{attempt}-4821");
            submit_card(&s, &token, "me_1".into(), secret.clone(), true).unwrap();
            if attempt < INPUT_ATTEMPTS {
                // 다시 넣을 수 있다: 같은 토큰·링크의 새 카드, 앞선 시도의 까닭이 실린다.
                let again = next_card(&mut s).await;
                assert_eq!(again.kind, KIND_LOGIN_INPUT);
                assert_eq!(again.payload["token"], token.as_str());
                assert_eq!(again.payload["link"], link.as_str());
                assert_eq!(again.payload["error"], reason.as_str());
                assert_eq!(again.payload["reason"], "시험", "까닭은 처음 그대로");
                assert!(!again.payload.to_string().contains(&secret));
                let last = s.notes.0.lock().unwrap().last().cloned().unwrap();
                assert!(
                    last.starts_with("⚠️")
                        && last.contains(&reason)
                        && last.contains("새로 올린 카드에서 다시 넣을 수 있습니다"),
                    "{last}"
                );
                assert!(!last.contains("같은 링크에서"), "{last}");
                assert!(matches!(
                    s.logins.form_status(&token),
                    FormStatus::Ready { error: Some(_), .. }
                ));
            } else {
                // 마지막 시도: 닫았다고 알리고 카드를 더 올리지 않는다.
                let status = wait_idle(&s, &token).await;
                assert!(
                    matches!(
                        &status,
                        FormStatus::Finished(FormOutcome::Failed {
                            retry_token: None,
                            ..
                        })
                    ),
                    "{status:?}"
                );
                let last = s.notes.0.lock().unwrap().last().cloned().unwrap();
                assert!(
                    last.contains("번 시도해 닫았습니다") && !last.contains("새로 올린 카드"),
                    "{last}"
                );
                // 올릴 카드가 있었다면 곧 올라온다: 잠깐 두고 본다.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                assert!(s.posts.try_recv().is_err(), "상한이면 새 카드가 없다");
            }
        }
        assert_eq!(input_cards(&s).len(), 3, "처음 카드와 다시 올린 둘");
        assert_eq!(s.browser.passwords.lock().unwrap().len(), 3);
        let err =
            submit_card(&s, &token, "me_1".into(), "pw-시험-넷째-9903".into(), true).unwrap_err();
        assert!(err.contains("이미 끝난 요청"), "{err}");
        assert_eq!(
            s.browser.passwords.lock().unwrap().len(),
            3,
            "더 시도하지 않는다"
        );
        assert!(
            s.secrets.0.lock().unwrap().is_empty(),
            "실패하면 저장 안 함"
        );
        for n in 1..=INPUT_ATTEMPTS {
            assert_absent_from_store(&s, &format!("pw-시험-틀림-{n}-4821"), false);
        }
    }

    /// 브라우저 오류로 실패해도 같은 길이다(카드를 다시 올린다). 오류 글은 가려서 싣는다.
    #[tokio::test]
    async fn browser_errors_also_repost_the_card() {
        let mut s = app_setup();
        s.logins.request("quasarzone", "시험", None).await.unwrap();
        let token = next_card(&mut s).await.payload["token"]
            .as_str()
            .unwrap()
            .to_owned();
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Err(BrowserError::Unavailable("샌드박스 없음".into())));
        submit_card(&s, &token, "me_1".into(), "pw-시험-오류-2750".into(), false).unwrap();
        let again = next_card(&mut s).await;
        assert!(
            again.payload["error"]
                .as_str()
                .unwrap()
                .contains("샌드박스 없음"),
            "{}",
            again.payload
        );
    }

    /// 빈 칸은 시도로 세지 않는다: 브라우저를 부르지 않고, 몇 번을 비워 보내도 카드를 다시 올리며 상한에 닿지 않는다.
    #[tokio::test]
    async fn empty_fields_are_not_counted_and_the_card_is_posted_again() {
        let mut s = app_setup();
        s.logins.request("quasarzone", "시험", None).await.unwrap();
        let token = next_card(&mut s).await.payload["token"]
            .as_str()
            .unwrap()
            .to_owned();
        for (user, pw) in [
            ("me_1", ""),
            ("", "pw-시험-빈칸-6612"),
            ("  ", "x"),
            ("me_1", ""),
        ]
        .into_iter()
        .cycle()
        .take(INPUT_ATTEMPTS as usize + 2)
        {
            submit_card(&s, &token, user.into(), pw.into(), true).unwrap();
            let again = next_card(&mut s).await;
            assert!(
                again.payload["error"]
                    .as_str()
                    .unwrap()
                    .contains("확인해 주세요"),
                "{}",
                again.payload
            );
            assert_eq!(again.payload["token"], token.as_str());
        }
        assert!(
            s.browser.calls.lock().unwrap().is_empty(),
            "빈 칸으로는 브라우저를 부르지 않는다"
        );
        // 상한에 닿지 않았으므로 제대로 넣으면 로그인된다.
        submit_card(&s, &token, "me_1".into(), "pw-시험-빈칸-6612".into(), false).unwrap();
        assert!(matches!(
            wait_idle(&s, &token).await,
            FormStatus::Finished(FormOutcome::Done { .. })
        ));
    }

    /// 링크와 카드는 토큰 하나를 나눈다: 한쪽이 끝내면 다른 쪽은 쓸 수 없다. 카드 취소는 쓰지 않은 요청만 닫는다.
    #[tokio::test]
    async fn link_failure_does_not_add_a_second_card_and_the_reason_is_masked() {
        let mut s = app_setup();
        s.logins.request("quasarzone", "시험", None).await.unwrap();
        let first = next_card(&mut s).await;
        let token = first.payload["token"].as_str().unwrap().to_owned();
        // 링크에서 낸 실패: 처음 카드가 아직 열려 있으니 새 카드를 올리지 않는다.
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::Rejected("비밀번호가 다릅니다".into())));
        let out = s
            .logins
            .form_submit(&token, "me", "pw-시험-링크-7001", false)
            .await;
        assert!(matches!(out, FormOutcome::Failed { .. }), "{out:?}");
        assert!(s.posts.try_recv().is_err(), "열린 카드가 있는데 또 올렸다");

        // 카드에서 낸 실패: 사이트가 넣은 값을 되풀이해도 새 카드의 까닭에는 남지 않는다.
        let secret = "pw-시험-카드-7002";
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::Rejected(format!(
                "{secret} 은(는) 틀렸습니다"
            ))));
        submit_card(&s, &token, "me".into(), secret.into(), false).unwrap();
        let again = next_card(&mut s).await;
        let error = again.payload["error"].as_str().unwrap();
        assert!(!error.contains(secret), "{error}");
        assert!(!again.payload.to_string().contains(secret));
    }

    #[tokio::test]
    async fn already_logged_in_by_card_is_told_by_a_notice() {
        let mut s = app_setup();
        s.logins.request("quasarzone", "시험", None).await.unwrap();
        let token = next_card(&mut s).await.payload["token"]
            .as_str()
            .unwrap()
            .to_owned();
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::AlreadyLoggedIn));
        submit_card(&s, &token, "me".into(), "pw-시험-카드-7003".into(), false).unwrap();
        wait_idle(&s, &token).await;
        let notes = s.notes.0.lock().unwrap().join("\n");
        assert!(notes.contains("이미 로그인돼 있었습니다"), "{notes}");
    }

    #[tokio::test]
    async fn link_and_card_share_one_token_and_cancel_only_closes_unused_requests() {
        let mut s = app_setup();
        s.logins.request("quasarzone", "시험", None).await.unwrap();
        let token = next_card(&mut s).await.payload["token"]
            .as_str()
            .unwrap()
            .to_owned();
        // 링크에서 로그인했다 → 같은 토큰으로 카드 제출은 막힌다.
        let out = s
            .logins
            .form_submit(&token, "me", "pw-시험-링크-5521", false)
            .await;
        assert!(matches!(out, FormOutcome::Done { .. }), "{out:?}");
        let err =
            submit_card(&s, &token, "me".into(), "pw-시험-카드-5521".into(), false).unwrap_err();
        assert!(err.contains("이미 끝난 요청"), "{err}");
        assert_eq!(
            s.browser.calls.lock().unwrap().len(),
            1,
            "두 번 로그인하지 않는다"
        );
        // 끝난 요청은 취소해도 결과 화면이 남는다.
        s.logins.cancel_input(&token);
        assert!(matches!(
            s.logins.form_status(&token),
            FormStatus::Finished(FormOutcome::Done { .. })
        ));

        // 쓰지 않은 요청: 취소하면 링크도 닫힌다.
        s.logins.request("quasarzone", "다시", None).await.unwrap();
        let unused = next_card(&mut s).await.payload["token"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(unused, token);
        assert!(s.logins.form_peek(&unused).is_some());
        s.logins.cancel_input(&unused);
        assert_eq!(s.logins.form_status(&unused), FormStatus::Invalid);
        assert!(s.logins.form_peek(&unused).is_none());
        assert_eq!(
            s.logins
                .form_submit(&unused, "me", "pw-시험-링크-5522", false)
                .await,
            FormOutcome::Invalid,
            "닫힌 링크로는 넣을 수 없다"
        );
        let err =
            submit_card(&s, &unused, "me".into(), "pw-시험-카드-5522".into(), false).unwrap_err();
        assert!(err.contains("시간이 지난 요청"), "{err}");
        s.logins.cancel_input("없는토큰");

        // 하는 중인 요청은 취소해도 닫지 않는다(로그인이 끝난다).
        s.logins.request("quasarzone", "또", None).await.unwrap();
        let busy = next_card(&mut s).await.payload["token"]
            .as_str()
            .unwrap()
            .to_owned();
        s.logins.inputs.lock().unwrap().get_mut(&busy).unwrap().busy = true;
        s.logins.cancel_input(&busy);
        assert!(matches!(
            s.logins.form_status(&busy),
            FormStatus::Busy { .. }
        ));
        let err =
            submit_card(&s, &busy, "me".into(), "pw-시험-카드-5523".into(), false).unwrap_err();
        assert!(err.contains("이미 로그인하고 있습니다"), "{err}");
    }

    /// 앱이 알림 받을 곳이 아니면(Discord DM, 앱을 취소함, 승인 데스크 없음) 예전 그대로 입력 링크를 글로 보낸다.
    #[tokio::test]
    async fn without_an_app_notify_target_the_link_is_sent_as_text() {
        // 기본(알림 받을 곳 없음).
        let mut s = setup();
        assert_eq!(
            s.logins.request("quasarzone", "시험", None).await.unwrap(),
            Requested::InputLink
        );
        assert!(token_from(&s.notes.0.lock().unwrap()[0]).len() == 64);
        assert!(s.posts.try_recv().is_err(), "카드를 올리지 않는다");

        // Discord DM이 알림 대화.
        let mut s = setup();
        s.store
            .lock()
            .unwrap()
            .set_setting(
                lucia_store::keys::NOTIFY_CONVERSATION,
                "discord:dm",
                Utc::now(),
            )
            .unwrap();
        assert_eq!(
            s.logins.request("quasarzone", "시험", None).await.unwrap(),
            Requested::InputLink
        );
        assert!(s.notes.0.lock().unwrap()[0].contains("/site-login/"));
        assert!(s.posts.try_recv().is_err());

        // 알림 받을 앱을 취소했다.
        let mut s = app_setup();
        s.store
            .lock()
            .unwrap()
            .revoke_client("cl-1", Utc::now())
            .unwrap();
        assert_eq!(
            s.logins.request("quasarzone", "시험", None).await.unwrap(),
            Requested::InputLink
        );
        assert!(s.posts.try_recv().is_err());

        // 승인 데스크가 없는 데몬: 앱이 알림 받을 곳이어도 링크 글.
        let browser = Arc::new(FakeBrowser::default());
        let notes = Arc::new(FakeNotifier::default());
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        {
            let st = store.lock().unwrap();
            st.create_client("cl-1", "앱", "app", "h1", Utc::now())
                .unwrap();
            st.set_setting(crate::gateway::keys::NOTIFY_CLIENT, "cl-1", Utc::now())
                .unwrap();
        }
        let logins = SiteLogins::new(
            browser,
            Arc::new(FakeSecrets::default()),
            store,
            notes.clone(),
            None,
            crate::browser::sites::builtin(),
            LoginsConfig {
                form_base: "http://192.168.0.2:8780/".into(),
                input_ttl: Duration::minutes(10),
                approval_ttl: Duration::minutes(30),
            },
        );
        assert_eq!(
            logins.request("quasarzone", "시험", None).await.unwrap(),
            Requested::InputLink
        );
        assert!(notes.0.lock().unwrap()[0].contains("/site-login/"));
    }

    /// 카드를 올리지 못하면(데스크 통로가 닫힘) 요청이 오류로 끝나고 쓸 수 있는 토큰이 남지 않는다.
    #[tokio::test]
    async fn a_card_that_cannot_be_posted_leaves_no_live_token() {
        let s = app_setup();
        drop(s.posts);
        let err = s
            .logins
            .request("quasarzone", "시험", None)
            .await
            .unwrap_err();
        assert!(err.contains("승인 데스크"), "{err}");
        assert!(s.logins.inputs.lock().unwrap().is_empty());
        assert!(s.notes.0.lock().unwrap().is_empty(), "글로 새지 않는다");
    }

    /// 저장된 로그인이 거절되면 다시 넣을 길도 같은 규칙을 따른다: 앱이 알림 받을 곳이면 카드, 아니면 링크 글.
    #[tokio::test]
    async fn a_rejected_saved_login_asks_again_through_the_same_channel() {
        let mut s = app_setup();
        let site = s.logins.site("quasarzone").unwrap();
        s.logins
            .save(&site, "me", "pw-시험-저장-3340")
            .await
            .unwrap();
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::Rejected("비밀번호가 다릅니다".into())));
        let err = s.logins.login_saved("quasarzone").await.unwrap_err();
        assert!(err.contains("비밀번호가 다릅니다"), "{err}");
        let card = next_card(&mut s).await;
        assert_eq!(card.kind, KIND_LOGIN_INPUT);
        assert_eq!(
            card.payload["reason"],
            "저장된 비밀번호로 로그인하지 못했습니다. 다시 넣어 주세요."
        );
        assert!(
            s.notes.0.lock().unwrap().is_empty(),
            "카드를 올렸으니 링크 글은 없다"
        );
        // 그 카드로 다시 넣으면 로그인된다.
        let token = card.payload["token"].as_str().unwrap().to_owned();
        submit_card(&s, &token, "me".into(), "pw-시험-새것-3341".into(), true).unwrap();
        assert!(matches!(
            wait_idle(&s, &token).await,
            FormStatus::Finished(FormOutcome::Done { .. })
        ));
    }

    /// 사람 확인이 뜨면 안내 글이 화면 넘겨받기 길을 알린다: 알림 받을 앱이 범위를 가졌으면 앱을 먼저 말한다.
    #[tokio::test]
    async fn human_check_note_points_to_the_app_only_when_it_may_take_over() {
        async fn note_for(s: &mut Setup) -> (String, String) {
            s.logins.request("quasarzone", "시험", None).await.unwrap();
            let token = next_card(s).await.payload["token"]
                .as_str()
                .unwrap()
                .to_owned();
            s.browser
                .logins
                .lock()
                .unwrap()
                .push_back(Ok(LoginResult::NeedsHuman {
                    why: "security verification".into(),
                }));
            let FormOutcome::Pending { note, .. } = s
                .logins
                .form_submit(&token, "me", "pw-시험-확인-8802", false)
                .await
            else {
                panic!("사람 확인 단계여야 한다");
            };
            let episode = s
                .store
                .lock()
                .unwrap()
                .search_episodes("로그인", 5)
                .unwrap()
                .into_iter()
                .map(|e| e.body)
                .find(|b| b.contains("사람 확인"))
                .expect("에피소드가 남는다");
            (note, episode)
        }
        let mut with_scope = app_setup_with(&[crate::gateway::scopes::BROWSER_HANDOVER]);
        let (note, episode) = note_for(&mut with_scope).await;
        assert!(note.contains("앱의 브라우저 화면 넘겨받기"), "{note}");
        assert!(
            note.contains("관리 페이지"),
            "관리 페이지 길도 함께: {note}"
        );
        assert!(episode.contains("허용한 앱"), "{episode}");
        assert!(
            with_scope
                .notes
                .0
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .contains("앱의 브라우저 화면 넘겨받기"),
            "알림으로도 간다"
        );

        // 앱에 범위가 없으면 관리 페이지만 말한다.
        let mut without_scope = app_setup_with(&[crate::gateway::scopes::APP_TOOLS]);
        let (note, _) = note_for(&mut without_scope).await;
        assert!(!note.contains("앱의"), "{note}");
        assert!(note.contains("관리 페이지 \"브라우저 넘겨받기\""), "{note}");

        // 앱이 알림 받을 곳이 아니면 마찬가지.
        let s = setup();
        s.logins.request("quasarzone", "시험", None).await.unwrap();
        let token = token_from(&s.notes.0.lock().unwrap()[0]);
        s.browser
            .logins
            .lock()
            .unwrap()
            .push_back(Ok(LoginResult::NeedsHuman {
                why: "captcha".into(),
            }));
        let FormOutcome::Pending { note, .. } = s
            .logins
            .form_submit(&token, "me", "pw-시험-확인-8803", false)
            .await
        else {
            panic!("사람 확인 단계여야 한다");
        };
        assert!(!note.contains("앱의"), "{note}");
    }
}
