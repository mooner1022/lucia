//! 이미지 생성(결정 0046): OpenAI Image API의 GPT Image 2.5 두 모델로 그림을 만든다.
//!
//! - **모델**: `gpt-image-2.5-flare`(가장 빠른 일상용)와 `gpt-image-2.5-sunburst`(가장 정밀, 편집에 강하다). 어느 쪽을
//!   쓸지는 Lucia가 요청마다 고른다(도구 인자). 고르지 않으면 새로 그릴 때는 flare, 그림을 고칠 때는 sunburst다
//!   (편집 API의 기본값과 같다). 토큰 요율은 둘이 같다.
//! - **편집**: 그림(파일 ID)을 주면 `POST /v1/images/edits`에 multipart(`image[]`)로 보낸다. 받은 그림은 그대로
//!   OpenAI로 나간다(사진의 메타데이터도).
//! - **작업으로 돈다**: 생성은 2분까지 걸릴 수 있는데 MCP 요청은 60초쯤에 끊긴다. 그래서 요청은 데몬 작업이 맡고,
//!   도구는 [`TOOL_WAIT`]만 기다린 뒤 작업 ID를 돌려준다. 결과(그림 파일, 비용)는 아무도 기다리지 않아도 남긴다
//!   (돈은 이미 썼다). 시간 초과를 스스로 다시 보내지 않는다.
//! - **돈**: 실제 과금이다. Claude 실행의 하루·주 상한과 따로, 하루 예산(관리 페이지, 기본 [`DEFAULT_DAILY_USD`])을
//!   둔다. 비용은 응답의 `usage`에 문서의 요율을 곱해 센다. 한 번에 하나만 만들어 넘겨도 한 장만큼만 넘는다.
//! - **키**: 관리 페이지에서 넣은 OpenAI API 키(자격 증명 `openai-api-key`, systemd-creds로 암호화). 호스트만 가진다.
//! - 프롬프트는 OpenAI로 나간다. 보내기 전에 마스킹을 거친다.
//!
//! 사실 확인(2026-11-19, OpenAI 공식 문서): `docs/decisions/0046-image-generation.md`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, TimeZone as _, Utc};
use chrono_tz::Tz;
use lucia_store::{FileRecord, GeneratedImage, Store};
use serde_json::{Value, json};

use crate::credentials::Credentials;
use crate::files::FileStore;

/// 자격 증명 이름.
pub const CREDENTIAL: &str = "openai-api-key";

pub mod keys {
    /// 하루 예산(달러). 비우면 기본값, 0이면 만들지 않는다.
    pub const DAILY_USD: &str = "images.daily_usd";
}

/// 하루 예산 기본값 [제안].
pub const DEFAULT_DAILY_USD: f64 = 1.0;
/// 도구가 결과를 기다리는 시간(MCP 요청이 60초쯤에 끊기기 전에 답한다).
pub const TOOL_WAIT: Duration = Duration::from_secs(45);
/// OpenAI에 보낸 요청을 기다리는 시간(문서: 복잡한 프롬프트는 2분까지).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
/// Image API의 바탕 주소. 새로 그리기는 `/generations`, 고치기는 `/edits`.
const API_BASE: &str = "https://api.openai.com/v1/images";
/// 편집에 넣는 그림 수 [제안](API는 16장까지 받는다)과 한 장의 크기.
pub const MAX_INPUTS: usize = 4;
const INPUT_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// 프롬프트 길이 상한(API는 32000자까지 받는다).
pub const PROMPT_MAX: usize = 4000;
/// 기록에 남기는 프롬프트 앞부분.
const PROMPT_KEEP: usize = 300;
/// 응답에 `usage`가 없을 때 예산에 잡는 값 [제안](예산이 뚫리지 않게 넉넉히).
const UNKNOWN_COST_USD: f64 = 0.25;
/// 요율(달러 / 100만 토큰, 두 모델이 같다).
const TEXT_IN_PER_M: f64 = 5.0;
const IMAGE_IN_PER_M: f64 = 8.0;
const OUT_PER_M: f64 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Model {
    #[default]
    Flare,
    Sunburst,
}

impl Model {
    /// API에 보내는 모델 ID.
    pub fn id(self) -> &'static str {
        match self {
            Self::Flare => "gpt-image-2.5-flare",
            Self::Sunburst => "gpt-image-2.5-sunburst",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "flare" | "gpt-image-2.5-flare" => Some(Self::Flare),
            "sunburst" | "gpt-image-2.5-sunburst" => Some(Self::Sunburst),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    Low,
    #[default]
    Medium,
    High,
}

impl Quality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Size {
    #[default]
    Square,
    Landscape,
    Portrait,
}

impl Size {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Square => "1024x1024",
            Self::Landscape => "1536x1024",
            Self::Portrait => "1024x1536",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "square" | "1024x1024" => Some(Self::Square),
            "landscape" | "1536x1024" => Some(Self::Landscape),
            "portrait" | "1024x1536" => Some(Self::Portrait),
            _ => None,
        }
    }
}

/// 그림 한 장의 주문.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Spec {
    pub prompt: String,
    /// 고르지 않으면 새로 그릴 때 flare, 고칠 때 sunburst.
    pub model: Option<Model>,
    pub quality: Quality,
    /// 고르지 않으면 새로 그릴 때 정사각형, 고칠 때는 모델에 맡긴다(`auto`).
    pub size: Option<Size>,
    /// 투명 배경(PNG). 아니면 불투명 JPEG.
    pub transparent: bool,
    /// 고치거나 참고할 그림의 파일 ID. 있으면 편집이다.
    pub images: Vec<String>,
}

/// 편집에 넣는 그림 하나.
#[derive(Debug, Clone)]
struct Input {
    name: String,
    mime: String,
    bytes: Vec<u8>,
}

/// 보낼 요청(기본값을 채우고 그림을 읽은 것).
#[derive(Debug, Clone)]
struct Order {
    prompt: String,
    model: Model,
    size: &'static str,
    quality: Quality,
    transparent: bool,
    inputs: Vec<Input>,
}

/// 기다린 결과.
#[derive(Debug)]
pub enum Waited {
    Done(Box<(GeneratedImage, FileRecord)>),
    Failed(String),
    /// 아직 만드는 중.
    Pending,
}

/// 응답의 사용량.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Usage {
    input_tokens: i64,
    output_tokens: i64,
    cost_usd: f64,
}

fn usage_of(v: &Value) -> Option<Usage> {
    let u = v.get("usage")?;
    let n = |v: &Value| v.as_i64().unwrap_or(0).max(0);
    let input = n(&u["input_tokens"]);
    let output = n(&u["output_tokens"]);
    let details = &u["input_tokens_details"];
    // 입력의 글·그림 나눔이 없으면 모두 글로 본다(생성 요청의 입력은 프롬프트다).
    let (text_in, image_in) = if details.is_object() {
        (n(&details["text_tokens"]), n(&details["image_tokens"]))
    } else {
        (input, 0)
    };
    let cost = (text_in as f64 * TEXT_IN_PER_M
        + image_in as f64 * IMAGE_IN_PER_M
        + output as f64 * OUT_PER_M)
        / 1_000_000.0;
    Some(Usage {
        input_tokens: input,
        output_tokens: output,
        cost_usd: cost,
    })
}

/// 실패 응답을 Lucia가 사용자에게 전할 말로. API가 준 글은 외부 데이터라 짧게 잘라 덧붙이기만 한다.
fn failure(status: u16, body: &Value) -> String {
    let err = &body["error"];
    let code = err["code"].as_str().unwrap_or("");
    let detail: String = err["message"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect();
    let detail = crate::mask::global().mask(&detail);
    let with = |head: &str| {
        if detail.is_empty() {
            head.to_owned()
        } else {
            format!("{head} (OpenAI: {detail})")
        }
    };
    if code == "moderation_blocked" {
        return with(
            "OpenAI의 안전 검사에 걸려 만들지 않았다. 같은 요청을 다시 보내지 말고, 사용자에게 알리고 요청을 바꾼다.",
        );
    }
    match status {
        401 => "OpenAI API 키가 틀렸거나 취소됐다. 사용자에게 관리 페이지 \"연결\"의 \"이미지 생성\"에서 키를 다시 넣어 달라고 한다.".into(),
        403 => with("이 키로는 이 모델을 쓸 수 없다. OpenAI 콘솔에서 조직 인증(Organization Verification)이 필요할 수 있다."),
        429 if code == "insufficient_quota" => with("OpenAI 계정의 잔액이나 한도가 부족하다. 사용자가 OpenAI 콘솔에서 확인해야 한다."),
        429 => with("OpenAI 요청 한도에 걸렸다. 몇 분 뒤에 다시 한다."),
        400..=499 => with("OpenAI가 요청을 받지 않았다. 요청을 바꿔야 한다."),
        _ => with(&format!("OpenAI 쪽 오류({status}). 잠시 뒤에 다시 한다.")),
    }
}

/// 하루 예산(달러). 설정이 없거나 읽을 수 없으면 기본값.
pub fn daily_cap(store: &Store) -> f64 {
    store
        .setting(keys::DAILY_USD)
        .ok()
        .flatten()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(DEFAULT_DAILY_USD)
}

/// 오늘(현지 자정부터) 쓴 돈.
pub fn spent_today(store: &Store, tz: Tz, now: DateTime<Utc>) -> f64 {
    let local = now.with_timezone(&tz).date_naive();
    let midnight = local
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| tz.from_local_datetime(&naive).earliest())
        .map_or(now - chrono::Duration::hours(24), |t| t.with_timezone(&Utc));
    store.generated_image_spend_since(midnight).unwrap_or(0.0)
}

/// API 키가 어디서 오나.
enum KeySource {
    Credentials(Arc<Credentials>),
    #[cfg(test)]
    Fixed(Option<String>),
}

pub struct ImageGen {
    store: Arc<Mutex<Store>>,
    files: Arc<FileStore>,
    key: KeySource,
    http: reqwest::Client,
    /// Image API의 바탕 주소(시험에서는 가짜 서버).
    base: String,
    tz: Tz,
    /// 만드는 중인 작업(작업 ID → 끝났는가). 끝나면 지운다.
    running: Mutex<HashMap<String, tokio::sync::watch::Receiver<bool>>>,
}

impl ImageGen {
    pub fn new(
        store: Arc<Mutex<Store>>,
        files: Arc<FileStore>,
        creds: Arc<Credentials>,
        http: reqwest::Client,
        tz: Tz,
    ) -> Arc<Self> {
        Self::build(
            store,
            files,
            KeySource::Credentials(creds),
            http,
            tz,
            API_BASE,
        )
    }

    fn build(
        store: Arc<Mutex<Store>>,
        files: Arc<FileStore>,
        key: KeySource,
        http: reqwest::Client,
        tz: Tz,
        base: &str,
    ) -> Arc<Self> {
        // 앞선 데몬이 끝을 보지 못한 요청은 결과를 받을 길이 없다.
        let _ = store
            .lock()
            .expect("store lock")
            .close_stale_generated_images(
                "데몬이 다시 시작돼 결과를 받지 못했다(과금됐을 수 있다)",
                Utc::now(),
            );
        Arc::new(Self {
            store,
            files,
            key,
            http,
            base: base.trim_end_matches('/').to_owned(),
            tz,
            running: Mutex::new(HashMap::new()),
        })
    }

    async fn api_key(&self) -> Result<String, String> {
        const MISSING: &str = "OpenAI API 키가 없어 그림을 만들 수 없다. 사용자에게 관리 페이지 \"연결\"의 \"이미지 생성\"에 키를 넣어 달라고 한다(키를 대화로 받지 않는다).";
        let key = match &self.key {
            KeySource::Credentials(creds) => creds
                .read(CREDENTIAL)
                .await
                .map_err(|e| format!("OpenAI API 키를 읽지 못했다: {e:#}"))?,
            #[cfg(test)]
            KeySource::Fixed(k) => k.clone(),
        };
        key.map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty())
            .ok_or_else(|| MISSING.to_owned())
    }

    /// 그림 만들기를 시작하고 작업 ID를 돌려준다. 키가 없거나, 예산이 찼거나, 만드는 중인 것이 있으면 까닭.
    pub async fn start(
        self: &Arc<Self>,
        spec: Spec,
        run_id: Option<&str>,
    ) -> Result<String, String> {
        let prompt = spec.prompt.trim();
        if prompt.is_empty() {
            return Err("무엇을 그릴지(prompt)가 비었다.".into());
        }
        if prompt.chars().count() > PROMPT_MAX {
            return Err(format!("prompt는 {PROMPT_MAX}자까지다. 줄여서 다시 한다."));
        }
        // 넣을 그림은 저장소 잠금을 쥐기 전에 읽는다(파일 보관소도 같은 잠금을 쓴다).
        let inputs = self.inputs(&spec.images)?;
        let edit = !inputs.is_empty();
        let model = spec
            .model
            .unwrap_or(if edit { Model::Sunburst } else { Model::Flare });
        let size = match (spec.size, edit) {
            (Some(s), _) => s.as_str(),
            (None, true) => "auto",
            (None, false) => Size::Square.as_str(),
        };
        let key = self.api_key().await?;
        let now = Utc::now();
        let job = {
            let store = self.store.lock().expect("store lock");
            let cap = daily_cap(&store);
            let spent = spent_today(&store, self.tz, now);
            if spent >= cap {
                return Err(format!(
                    "오늘의 이미지 예산(${cap:.2})을 다 썼다(오늘 ${spent:.2}). 내일 다시 하거나, 사용자가 관리 페이지 \"연결\"에서 예산을 올려야 한다."
                ));
            }
            let mut running = self.running.lock().expect("images lock");
            if let Some(id) = running.keys().next() {
                return Err(format!(
                    "이미 만들고 있는 그림이 있다(job={id}). image_result로 그 결과를 먼저 받는다."
                ));
            }
            // 프롬프트는 밖으로 나간다: 가린 것을 보내고, 기록에는 앞부분만 남긴다.
            let prompt = crate::mask::global().mask(prompt);
            let job = GeneratedImage {
                id: format!("img_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
                run_id: run_id.map(str::to_owned),
                model: model.id().into(),
                quality: spec.quality.as_str().into(),
                size: size.into(),
                background: if spec.transparent {
                    "transparent"
                } else {
                    "opaque"
                }
                .into(),
                prompt: prompt.chars().take(PROMPT_KEEP).collect(),
                status: "running".into(),
                file_id: None,
                error: None,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
                created_at: now,
                finished_at: None,
                inputs: inputs.len() as i64,
            };
            store
                .add_generated_image(&job)
                .map_err(|e| format!("기록하지 못했다: {e}"))?;
            let (tx, rx) = tokio::sync::watch::channel(false);
            running.insert(job.id.clone(), rx);
            let this = self.clone();
            let order = Order {
                prompt,
                model,
                size,
                quality: spec.quality,
                transparent: spec.transparent,
                inputs,
            };
            let job_for_task = job.clone();
            tokio::spawn(async move {
                this.run(&key, &order, &job_for_task).await;
                this.running
                    .lock()
                    .expect("images lock")
                    .remove(&job_for_task.id);
                let _ = tx.send(true);
            });
            job
        };
        tracing::info!(job = %job.id, model = %job.model, quality = %job.quality, size = %job.size, "이미지 생성 시작");
        Ok(job.id)
    }

    /// 편집에 넣을 그림을 읽는다. PNG·JPEG·WebP만, 정해진 수와 크기까지.
    fn inputs(&self, ids: &[String]) -> Result<Vec<Input>, String> {
        let ids: Vec<&str> = ids
            .iter()
            .map(|i| i.trim())
            .filter(|i| !i.is_empty())
            .collect();
        if ids.len() > MAX_INPUTS {
            return Err(format!("넣는 그림은 {MAX_INPUTS}장까지다."));
        }
        ids.into_iter()
            .map(|id| {
                let record = self
                    .files
                    .get(id)
                    .ok_or_else(|| format!("그런 파일이 없다: {id}"))?;
                if !matches!(
                    record.mime.as_str(),
                    "image/png" | "image/jpeg" | "image/webp"
                ) {
                    return Err(format!(
                        "{}은(는) 넣을 수 없는 형식이다({}). PNG, JPEG, WebP 그림만 넣는다.",
                        record.name, record.mime
                    ));
                }
                if record.size > INPUT_MAX_BYTES {
                    return Err(format!(
                        "{}이(가) 너무 크다({}까지).",
                        record.name,
                        crate::files::human_size(INPUT_MAX_BYTES)
                    ));
                }
                let bytes = self.files.read(&record)?;
                Ok(Input {
                    name: record.name,
                    mime: record.mime,
                    bytes,
                })
            })
            .collect()
    }

    /// 요청을 보내고, 결과를 파일과 기록으로 남긴다.
    async fn run(&self, key: &str, spec: &Order, job: &GeneratedImage) {
        let outcome = self.request(key, spec).await;
        let now = Utc::now();
        let (file, error, usage) = match outcome {
            Ok((bytes, usage)) => {
                let name = format!(
                    "image-{}.{}",
                    now.with_timezone(&self.tz).format("%Y%m%d-%H%M%S"),
                    if spec.transparent { "png" } else { "jpg" }
                );
                match self
                    .files
                    .create_image(&name, &bytes, job.run_id.as_deref(), now)
                {
                    Ok(f) => (Some(f), None, usage),
                    Err(why) => (
                        None,
                        Some(format!("받은 그림을 저장하지 못했다: {why}")),
                        usage,
                    ),
                }
            }
            Err((why, usage)) => (None, Some(why), usage),
        };
        let store = self.store.lock().expect("store lock");
        let _ = store.finish_generated_image(
            &job.id,
            file.as_ref().map(|f| f.id.as_str()),
            error.as_deref(),
            usage.input_tokens,
            usage.output_tokens,
            usage.cost_usd,
            now,
        );
        let what = if job.inputs > 0 {
            format!("이미지 편집(그림 {}장)", job.inputs)
        } else {
            "이미지 생성".to_owned()
        };
        let summary = match (&file, &error) {
            (Some(_), _) => format!(
                "{what}: {} · {} · {} · ${:.3}",
                job.model, job.quality, job.size, usage.cost_usd
            ),
            (None, why) => format!(
                "{what} 실패: {} — {}",
                job.model,
                why.as_deref().unwrap_or("까닭 모름")
            ),
        };
        let _ = store.add_activity("image_generate", &summary, job.run_id.as_deref(), now);
        tracing::info!(job = %job.id, ok = file.is_some(), cost = usage.cost_usd, "이미지 생성 끝");
    }

    /// OpenAI Image API 한 번. 실패해도 과금된 사용량이 있으면 함께 돌려준다.
    async fn request(&self, key: &str, spec: &Order) -> Result<(Vec<u8>, Usage), (String, Usage)> {
        let none = Usage::default();
        let background = if spec.transparent {
            "transparent"
        } else {
            "opaque"
        };
        let format = if spec.transparent { "png" } else { "jpeg" };
        let request = if spec.inputs.is_empty() {
            let mut body = json!({
                "model": spec.model.id(),
                "prompt": spec.prompt,
                "n": 1,
                "size": spec.size,
                "quality": spec.quality.as_str(),
                "background": background,
                "output_format": format,
            });
            if !spec.transparent {
                body["output_compression"] = json!(90);
            }
            self.http
                .post(format!("{}/generations", self.base))
                .json(&body)
        } else {
            // 편집은 multipart다(공식 안내의 2.5 예시와 SDK가 이렇게 보낸다). 그림은 `image[]`로 싣는다.
            // `input_fidelity`는 2.5 모델에 대한 설명이 없어 보내지 않는다.
            let mut form = reqwest::multipart::Form::new()
                .text("model", spec.model.id())
                .text("prompt", spec.prompt.clone())
                .text("size", spec.size)
                .text("quality", spec.quality.as_str())
                .text("background", background)
                .text("output_format", format);
            if !spec.transparent {
                form = form.text("output_compression", "90");
            }
            for input in &spec.inputs {
                let part = reqwest::multipart::Part::bytes(input.bytes.clone())
                    .file_name(input.name.clone())
                    .mime_str(&input.mime);
                match part {
                    Ok(part) => form = form.part("image[]", part),
                    Err(_) => return Err(("넣을 그림을 싣지 못했다.".into(), none)),
                }
            }
            self.http
                .post(format!("{}/edits", self.base))
                .multipart(form)
        };
        let sent = request
            .bearer_auth(key)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;
        let response = match sent {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Err((
                    "OpenAI가 시간 안에 답하지 않았다. 과금됐을 수 있으니 같은 요청을 바로 다시 보내지 말고 사용자에게 알린다.".into(),
                    Usage { cost_usd: UNKNOWN_COST_USD, ..none },
                ));
            }
            Err(e) => {
                return Err((
                    format!(
                        "OpenAI에 닿지 못했다: {}",
                        crate::mask::global().mask(&e.without_url().to_string())
                    ),
                    none,
                ));
            }
        };
        let status = response.status().as_u16();
        let parsed: Value = match response.bytes().await {
            Ok(b) => serde_json::from_slice(&b).unwrap_or(Value::Null),
            Err(e) if e.is_timeout() => {
                return Err((
                    "OpenAI의 답을 받다가 시간이 다 됐다. 과금됐을 수 있으니 같은 요청을 바로 다시 보내지 않는다.".into(),
                    Usage { cost_usd: UNKNOWN_COST_USD, ..none },
                ));
            }
            Err(_) => Value::Null,
        };
        if !(200..300).contains(&status) {
            return Err((failure(status, &parsed), usage_of(&parsed).unwrap_or(none)));
        }
        // 성공인데 사용량이 없으면 예산이 뚫리지 않게 넉넉히 잡는다.
        let usage = usage_of(&parsed).unwrap_or(Usage {
            cost_usd: UNKNOWN_COST_USD,
            ..none
        });
        let Some(encoded) = parsed["data"][0]["b64_json"].as_str() else {
            return Err(("OpenAI의 답에 그림이 없다.".into(), usage));
        };
        match base64::engine::general_purpose::STANDARD.decode(encoded) {
            Ok(bytes) if !bytes.is_empty() => Ok((bytes, usage)),
            _ => Err(("OpenAI가 준 그림을 풀지 못했다.".into(), usage)),
        }
    }

    /// 작업의 결과를 `max`까지 기다린다.
    pub async fn wait(&self, job_id: &str, max: Duration) -> Waited {
        let job_id = job_id.trim();
        let valid = job_id
            .strip_prefix("img_")
            .is_some_and(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()));
        if !valid {
            return Waited::Failed(
                "그런 작업이 없다(job은 image_generate가 돌려준 img_… 값이다).".into(),
            );
        }
        let rx = self
            .running
            .lock()
            .expect("images lock")
            .get(job_id)
            .cloned();
        if let Some(mut rx) = rx {
            // 시간이 다 되면 "아직"이다. 끝났거나 보내는 쪽이 사라졌으면 기록을 본다.
            let timed_out = tokio::time::timeout(max, rx.wait_for(|done| *done))
                .await
                .is_err();
            if timed_out {
                return Waited::Pending;
            }
        }
        let row = self
            .store
            .lock()
            .expect("store lock")
            .generated_image(job_id)
            .ok()
            .flatten();
        let Some(row) = row else {
            return Waited::Failed("그런 작업이 없다.".into());
        };
        match (row.status.as_str(), row.file_id.clone()) {
            ("done", Some(file_id)) => match self.files.get(&file_id) {
                Some(file) => Waited::Done(Box::new((row, file))),
                None => Waited::Failed("만든 그림 파일이 보관 기간이 지나 지워졌다.".into()),
            },
            ("running", _) => Waited::Pending,
            _ => Waited::Failed(row.error.unwrap_or_else(|| "까닭 모름".into())),
        }
    }

    /// 그림 파일의 바이트(도구가 모델에게 보여 줄 때).
    pub fn read(&self, file: &FileRecord) -> Result<Vec<u8>, String> {
        self.files.read(file)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 받은 요청 하나: (Authorization 머리, 본문).
    pub type Seen = (Option<String>, Value);

    /// 가짜 OpenAI: 받은 요청을 적어 두고, 정해 둔 답을 (정해 둔 만큼 늦게) 돌려준다.
    #[derive(Clone)]
    pub struct Fake {
        pub requests: Arc<Mutex<Vec<Seen>>>,
        pub reply: Arc<Mutex<(u16, Value)>>,
        pub delay: Arc<Mutex<Duration>>,
        pub calls: Arc<AtomicUsize>,
    }

    /// 아주 작은 JPEG 머리(파일 종류 판별에는 이만큼이면 된다).
    pub fn jpeg() -> Vec<u8> {
        vec![
            0xff, 0xd8, 0xff, 0xe0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1,
        ]
    }

    pub fn ok_body(bytes: &[u8]) -> Value {
        json!({
            "created": 1795000000,
            "data": [{"b64_json": base64::engine::general_purpose::STANDARD.encode(bytes)}],
            "usage": {
                "total_tokens": 1100,
                "input_tokens": 100,
                "output_tokens": 1000,
                "input_tokens_details": {"text_tokens": 100, "image_tokens": 0},
            },
        })
    }

    impl Fake {
        pub async fn serve() -> (Self, SocketAddr) {
            let fake = Fake {
                requests: Arc::default(),
                reply: Arc::new(Mutex::new((200, ok_body(&jpeg())))),
                delay: Arc::new(Mutex::new(Duration::ZERO)),
                calls: Arc::default(),
            };
            let app = axum::Router::new()
                .route(
                    "/v1/images/generations",
                    axum::routing::post(
                        |axum::extract::State(f): axum::extract::State<Fake>,
                         headers: axum::http::HeaderMap,
                         axum::Json(body): axum::Json<Value>| async move {
                            f.calls.fetch_add(1, Ordering::SeqCst);
                            let auth = headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_owned);
                            f.requests.lock().unwrap().push((auth, body));
                            let delay = *f.delay.lock().unwrap();
                            tokio::time::sleep(delay).await;
                            let (status, reply) = f.reply.lock().unwrap().clone();
                            (
                                axum::http::StatusCode::from_u16(status).unwrap(),
                                axum::Json(reply),
                            )
                        },
                    ),
                )
                .route(
                    "/v1/images/edits",
                    axum::routing::post(
                        |axum::extract::State(f): axum::extract::State<Fake>,
                         headers: axum::http::HeaderMap,
                         mut form: axum::extract::Multipart| async move {
                            f.calls.fetch_add(1, Ordering::SeqCst);
                            let auth = headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_owned);
                            // 받은 칸을 적는다: 글 칸은 값으로, 파일 칸은 (칸 이름, 파일 이름, 형식, 크기)로.
                            let mut seen = json!({"_edit": true, "_files": []});
                            while let Ok(Some(field)) = form.next_field().await {
                                let name = field.name().unwrap_or("").to_owned();
                                let file = field.file_name().map(str::to_owned);
                                let mime = field.content_type().map(str::to_owned);
                                let data = field.bytes().await.unwrap_or_default();
                                match file {
                                    Some(file) => seen["_files"].as_array_mut().unwrap().push(
                                        json!({"field": name, "name": file, "mime": mime, "size": data.len()}),
                                    ),
                                    None => {
                                        seen[name] = json!(String::from_utf8_lossy(&data));
                                    }
                                }
                            }
                            f.requests.lock().unwrap().push((auth, seen));
                            let (status, reply) = f.reply.lock().unwrap().clone();
                            (
                                axum::http::StatusCode::from_u16(status).unwrap(),
                                axum::Json(reply),
                            )
                        },
                    ),
                )
                .with_state(fake.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            (fake, addr)
        }
    }

    /// 가짜 OpenAI에 붙은 서비스. `key`가 없으면 키가 없는 상태.
    pub fn service(
        store: Arc<Mutex<Store>>,
        dir: &std::path::Path,
        addr: SocketAddr,
        key: Option<&str>,
    ) -> Arc<ImageGen> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let files = Arc::new(FileStore::new(
            dir.join("files"),
            reqwest::Client::new(),
            store.clone(),
        ));
        ImageGen::build(
            store,
            files,
            KeySource::Fixed(key.map(str::to_owned)),
            reqwest::Client::new(),
            chrono_tz::Asia::Seoul,
            &format!("http://{addr}/v1/images"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use std::sync::atomic::Ordering;

    fn spec(prompt: &str) -> Spec {
        Spec {
            prompt: prompt.into(),
            ..Spec::default()
        }
    }

    fn world() -> (Arc<Mutex<Store>>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("lucia-img-{}", uuid::Uuid::new_v4().simple()));
        (Arc::new(Mutex::new(Store::open_in_memory().unwrap())), dir)
    }

    #[test]
    fn choices_map_to_documented_values() {
        assert_eq!(Model::parse("flare").unwrap().id(), "gpt-image-2.5-flare");
        assert_eq!(
            Model::parse(" Sunburst ").unwrap().id(),
            "gpt-image-2.5-sunburst"
        );
        assert_eq!(Model::default(), Model::Flare);
        assert!(Model::parse("gpt-image-2.5").is_none() && Model::parse("dall-e-3").is_none());
        assert_eq!(Size::parse("landscape").unwrap().as_str(), "1536x1024");
        assert_eq!(Size::parse("portrait").unwrap().as_str(), "1024x1536");
        assert_eq!(Size::default().as_str(), "1024x1024");
        assert!(Size::parse("4096x4096").is_none());
        assert_eq!(Quality::default().as_str(), "medium");
        assert!(Quality::parse("max").is_none() && Quality::parse("auto").is_none());
        // 요율: 글 입력 $5, 그림 입력 $8, 출력 $30(100만 토큰).
        let u = usage_of(
            &json!({"usage": {"input_tokens": 300, "output_tokens": 2000,
            "input_tokens_details": {"text_tokens": 100, "image_tokens": 200}}}),
        )
        .unwrap();
        let want = (100.0 * 5.0 + 200.0 * 8.0 + 2000.0 * 30.0) / 1e6;
        assert!((u.cost_usd - want).abs() < 1e-12, "{}", u.cost_usd);
        let plain = usage_of(&json!({"usage": {"input_tokens": 100, "output_tokens": 0}})).unwrap();
        assert!((plain.cost_usd - 0.0005).abs() < 1e-12);
        assert!(usage_of(&json!({"data": []})).is_none());
    }

    #[tokio::test]
    async fn generates_saves_and_records_cost() {
        let (store, dir) = world();
        let (fake, addr) = Fake::serve().await;
        let gen_ = service(store.clone(), &dir, addr, Some("sk-test-key"));
        let job = gen_
            .start(
                Spec {
                    prompt: "  노을 지는 바다  ".into(),
                    model: Some(Model::Sunburst),
                    quality: Quality::High,
                    size: Some(Size::Landscape),
                    transparent: false,
                    images: Vec::new(),
                },
                Some("chat-1"),
            )
            .await
            .unwrap();
        let Waited::Done(done) = gen_.wait(&job, Duration::from_secs(5)).await else {
            panic!("끝나야 한다");
        };
        let (row, file) = *done;
        // 보낸 것: 문서의 모델 ID와 값, 하나만, 키는 Bearer. 지원하지 않는 칸은 보내지 않는다.
        let (auth, body) = fake.requests.lock().unwrap()[0].clone();
        assert_eq!(auth.as_deref(), Some("Bearer sk-test-key"));
        assert_eq!(
            body,
            json!({
                "model": "gpt-image-2.5-sunburst",
                "prompt": "노을 지는 바다",
                "n": 1,
                "size": "1536x1024",
                "quality": "high",
                "background": "opaque",
                "output_format": "jpeg",
                "output_compression": 90,
            })
        );
        // 남긴 것: 그림 파일, 비용, 활동 기록.
        assert_eq!(
            (row.status.as_str(), row.model.as_str()),
            ("done", "gpt-image-2.5-sunburst")
        );
        assert_eq!((row.input_tokens, row.output_tokens), (100, 1000));
        assert!((row.cost_usd - 0.0305).abs() < 1e-9, "{}", row.cost_usd);
        assert_eq!(
            (file.kind.as_str(), file.mime.as_str()),
            ("image", "image/jpeg")
        );
        assert!(file.name.starts_with("image-") && file.name.ends_with(".jpg"));
        assert_eq!(file.run_id.as_deref(), Some("chat-1"));
        assert_eq!(gen_.read(&file).unwrap(), jpeg());
        {
            let s = store.lock().unwrap();
            assert!((spent_today(&s, chrono_tz::Asia::Seoul, Utc::now()) - 0.0305).abs() < 1e-9);
            assert!(s.recent_activity(3).unwrap().iter().any(|a| {
                a.kind == "image_generate" && a.summary.contains("gpt-image-2.5-sunburst")
            }));
        }
        // 투명 배경은 PNG로, 압축 칸 없이. 기본은 flare·medium·정사각형.
        let png = crate::expressions::testing::png(1024);
        *fake.reply.lock().unwrap() = (200, ok_body(&png));
        let job = gen_
            .start(
                Spec {
                    transparent: true,
                    ..spec("고양이 스티커")
                },
                None,
            )
            .await
            .unwrap();
        let Waited::Done(done) = gen_.wait(&job, Duration::from_secs(5)).await else {
            panic!("끝나야 한다");
        };
        assert!(done.1.name.ends_with(".png"));
        let body = fake.requests.lock().unwrap()[1].1.clone();
        assert_eq!(
            (
                body["model"].as_str(),
                body["quality"].as_str(),
                body["size"].as_str()
            ),
            (
                Some("gpt-image-2.5-flare"),
                Some("medium"),
                Some("1024x1024")
            )
        );
        assert_eq!(
            (body["background"].as_str(), body["output_format"].as_str()),
            (Some("transparent"), Some("png"))
        );
        assert!(body.get("output_compression").is_none());
        for absent in ["response_format", "style", "user", "safety_identifier"] {
            assert!(body.get(absent).is_none(), "{absent}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 편집: 받은 그림을 multipart의 `image[]`로 싣고, 고르지 않으면 sunburst와 `auto` 크기로 보낸다.
    #[tokio::test]
    async fn edits_send_the_pictures_as_multipart_and_default_to_sunburst() {
        let (store, dir) = world();
        let (fake, addr) = Fake::serve().await;
        let gen_ = service(store.clone(), &dir, addr, Some("sk-test-key"));
        // 서비스와 같은 보관소에 사용자가 붙인 파일을 둔다.
        let files = FileStore::new(dir.join("files"), reqwest::Client::new(), store.clone());
        let now = Utc::now();
        let put = |name: &str, bytes: &[u8]| {
            files
                .save("in", name, bytes, Some("app:cl-1"), now)
                .unwrap()
                .id
        };
        let photo = put("사진.jpg", &jpeg());
        let logo = put("logo.png", &crate::expressions::testing::png(512));
        let note = put("메모.txt", "글".as_bytes());
        let gif = put("움짤.gif", b"GIF89a\x01\x00\x01\x00\x00\x00\x00");
        let mut reply = ok_body(&jpeg());
        reply["usage"]["input_tokens"] = json!(300);
        reply["usage"]["input_tokens_details"] = json!({"text_tokens": 100, "image_tokens": 200});
        *fake.reply.lock().unwrap() = (200, reply);
        let job = gen_
            .start(
                Spec {
                    prompt: "사진 속 컵에 로고를 넣어 줘".into(),
                    images: vec![photo.clone(), format!(" {logo} ")],
                    ..Spec::default()
                },
                Some("chat-1"),
            )
            .await
            .unwrap();
        let Waited::Done(done) = gen_.wait(&job, Duration::from_secs(5)).await else {
            panic!("끝나야 한다");
        };
        let (row, _file) = *done;
        let (auth, sent) = fake.requests.lock().unwrap()[0].clone();
        assert_eq!(auth.as_deref(), Some("Bearer sk-test-key"));
        assert_eq!(sent["_edit"], true, "편집 주소로 갔다");
        assert_eq!(
            (
                sent["model"].as_str(),
                sent["size"].as_str(),
                sent["quality"].as_str()
            ),
            (Some("gpt-image-2.5-sunburst"), Some("auto"), Some("medium"))
        );
        assert_eq!(
            (
                sent["prompt"].as_str(),
                sent["background"].as_str(),
                sent["output_format"].as_str(),
                sent["output_compression"].as_str()
            ),
            (
                Some("사진 속 컵에 로고를 넣어 줘"),
                Some("opaque"),
                Some("jpeg"),
                Some("90")
            )
        );
        assert_eq!(
            sent["_files"],
            json!([
                {"field": "image[]", "name": "사진.jpg", "mime": "image/jpeg", "size": jpeg().len()},
                {"field": "image[]", "name": "logo.png", "mime": "image/png", "size": crate::expressions::testing::png(512).len()},
            ])
        );
        for absent in ["input_fidelity", "response_format", "mask", "user"] {
            assert!(sent.get(absent).is_none(), "{absent}");
        }
        // 기록: 넣은 그림 수, 그림 입력 토큰까지 센 비용.
        assert_eq!((row.inputs, row.size.as_str()), (2, "auto"));
        let want = (100.0 * 5.0 + 200.0 * 8.0 + 1000.0 * 30.0) / 1e6;
        assert!((row.cost_usd - want).abs() < 1e-9, "{}", row.cost_usd);
        assert!(
            store
                .lock()
                .unwrap()
                .recent_activity(3)
                .unwrap()
                .iter()
                .any(|a| a.summary.contains("이미지 편집(그림 2장)"))
        );
        // 고른 값은 그대로 간다.
        let job = gen_
            .start(
                Spec {
                    prompt: "배경을 지워 줘".into(),
                    model: Some(Model::Flare),
                    size: Some(Size::Portrait),
                    transparent: true,
                    images: vec![photo.clone()],
                    ..Spec::default()
                },
                None,
            )
            .await
            .unwrap();
        assert!(matches!(
            gen_.wait(&job, Duration::from_secs(5)).await,
            Waited::Done(_)
        ));
        let sent = fake.requests.lock().unwrap()[1].1.clone();
        assert_eq!(
            (
                sent["model"].as_str(),
                sent["size"].as_str(),
                sent["background"].as_str()
            ),
            (
                Some("gpt-image-2.5-flare"),
                Some("1024x1536"),
                Some("transparent")
            )
        );
        assert!(sent.get("output_compression").is_none());
        // 넣을 수 없는 것은 보내기 전에 거절한다.
        let calls = fake.calls.load(Ordering::SeqCst);
        let refuse = |images: Vec<String>| {
            let gen_ = gen_.clone();
            async move {
                gen_.start(
                    Spec {
                        prompt: "고쳐 줘".into(),
                        images,
                        ..Spec::default()
                    },
                    None,
                )
                .await
                .unwrap_err()
            }
        };
        assert!(
            refuse(vec!["f_000000000000".into()])
                .await
                .contains("그런 파일이 없다")
        );
        assert!(refuse(vec![note]).await.contains("PNG, JPEG, WebP"));
        assert!(refuse(vec![gif]).await.contains("PNG, JPEG, WebP"));
        assert!(
            refuse(vec![photo; MAX_INPUTS + 1])
                .await
                .contains("4장까지")
        );
        assert_eq!(fake.calls.load(Ordering::SeqCst), calls);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn slow_jobs_return_an_id_and_only_one_runs_at_a_time() {
        let (store, dir) = world();
        let (fake, addr) = Fake::serve().await;
        *fake.delay.lock().unwrap() = Duration::from_millis(400);
        let gen_ = service(store.clone(), &dir, addr, Some("sk-test-key"));
        let job = gen_.start(spec("느린 그림"), None).await.unwrap();
        // 기다리는 시간 안에 안 끝나면 "아직"이다. 그 사이 새 요청은 받지 않는다(겹쳐 과금되지 않게).
        assert!(matches!(
            gen_.wait(&job, Duration::from_millis(50)).await,
            Waited::Pending
        ));
        let busy = gen_.start(spec("다른 그림"), None).await.unwrap_err();
        assert!(
            busy.contains(&job) && busy.contains("image_result"),
            "{busy}"
        );
        // 다시 기다리면 받는다. 요청은 한 번만 갔다.
        assert!(matches!(
            gen_.wait(&job, Duration::from_secs(5)).await,
            Waited::Done(_)
        ));
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        // 끝난 작업은 다시 물어도 같은 결과를 준다(기록에서).
        assert!(matches!(
            gen_.wait(&job, Duration::from_millis(10)).await,
            Waited::Done(_)
        ));
        // 아무도 기다리지 않아도 결과와 비용은 남는다.
        let quiet = gen_.start(spec("기다리지 않는 그림"), None).await.unwrap();
        tokio::time::sleep(Duration::from_millis(700)).await;
        let row = store
            .lock()
            .unwrap()
            .generated_image(&quiet)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "done");
        assert!(row.cost_usd > 0.0 && row.file_id.is_some());
        assert!(matches!(
            gen_.wait("img_zz", Duration::from_millis(10)).await,
            Waited::Failed(_)
        ));
        assert!(matches!(
            gen_.wait("img_000000000000", Duration::from_millis(10))
                .await,
            Waited::Failed(_)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn key_budget_and_errors_are_explained() {
        let (store, dir) = world();
        let (fake, addr) = Fake::serve().await;
        // 키가 없으면 보내지 않고, 어디에 넣는지 말한다.
        let no_key = service(store.clone(), &dir, addr, None);
        let why = no_key.start(spec("그림"), None).await.unwrap_err();
        assert!(
            why.contains("관리 페이지") && why.contains("대화로 받지 않는다"),
            "{why}"
        );
        let gen_ = service(store.clone(), &dir, addr, Some("sk-test-key"));
        assert!(
            gen_.start(spec("   "), None)
                .await
                .unwrap_err()
                .contains("비었다")
        );
        assert!(
            gen_.start(spec(&"가".repeat(PROMPT_MAX + 1)), None)
                .await
                .unwrap_err()
                .contains("4000자")
        );
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
        // 실패 응답: 까닭을 사람에게 전할 말로 바꾼다.
        let fail = |status: u16, body: Value| {
            let gen_ = gen_.clone();
            let fake = fake.clone();
            async move {
                *fake.reply.lock().unwrap() = (status, body);
                let job = gen_.start(spec("그림"), None).await.unwrap();
                match gen_.wait(&job, Duration::from_secs(5)).await {
                    Waited::Failed(why) => why,
                    other => panic!("실패여야 한다: {other:?}"),
                }
            }
        };
        let why = fail(
            400,
            json!({"error": {"type": "image_generation_user_error", "code": "moderation_blocked", "message": "blocked"}}),
        )
        .await;
        assert!(
            why.contains("안전 검사") && why.contains("다시 보내지 말고"),
            "{why}"
        );
        assert!(
            fail(401, json!({"error": {"message": "bad key"}}))
                .await
                .contains("키가 틀렸거나")
        );
        assert!(
            fail(403, json!({"error": {"message": "verify"}}))
                .await
                .contains("조직 인증")
        );
        assert!(
            fail(
                429,
                json!({"error": {"code": "insufficient_quota", "message": "quota"}})
            )
            .await
            .contains("잔액이나 한도")
        );
        assert!(
            fail(429, json!({"error": {"message": "slow down"}}))
                .await
                .contains("몇 분 뒤")
        );
        assert!(
            fail(500, json!("oops"))
                .await
                .contains("OpenAI 쪽 오류(500)")
        );
        assert!(fail(200, json!({"data": []})).await.contains("그림이 없다"));
        let failed = store.lock().unwrap().recent_generated_images(1).unwrap();
        assert_eq!(failed[0].status, "failed");
        // 사용량 없이 성공으로 온 답은 넉넉히 잡아 예산에 넣는다.
        assert!(
            spent_today(&store.lock().unwrap(), chrono_tz::Asia::Seoul, Utc::now())
                >= UNKNOWN_COST_USD
        );
        // 예산: 다 쓰면 보내지 않는다. 0이면 아예 만들지 않는다. 올리면 다시 된다.
        *fake.reply.lock().unwrap() = (200, ok_body(&jpeg()));
        let calls = fake.calls.load(Ordering::SeqCst);
        store
            .lock()
            .unwrap()
            .set_setting(keys::DAILY_USD, "0.2", Utc::now())
            .unwrap();
        let why = gen_.start(spec("그림"), None).await.unwrap_err();
        assert!(
            why.contains("예산($0.20)") && why.contains("관리 페이지"),
            "{why}"
        );
        store
            .lock()
            .unwrap()
            .set_setting(keys::DAILY_USD, "0", Utc::now())
            .unwrap();
        assert!(gen_.start(spec("그림"), None).await.is_err());
        assert_eq!(
            fake.calls.load(Ordering::SeqCst),
            calls,
            "예산이 찼으면 보내지 않는다"
        );
        store
            .lock()
            .unwrap()
            .set_setting(keys::DAILY_USD, "5", Utc::now())
            .unwrap();
        let job = gen_.start(spec("그림"), None).await.unwrap();
        assert!(matches!(
            gen_.wait(&job, Duration::from_secs(5)).await,
            Waited::Done(_)
        ));
        assert_eq!(daily_cap(&store.lock().unwrap()), 5.0);
        // 다시 뜬 데몬은 끝을 보지 못한 요청을 실패로 닫는다.
        let last = store.lock().unwrap().recent_generated_images(1).unwrap()[0].clone();
        store
            .lock()
            .unwrap()
            .add_generated_image(&GeneratedImage {
                id: "img_0000000000aa".into(),
                status: "running".into(),
                ..last
            })
            .unwrap();
        let again = service(store.clone(), &dir, addr, Some("sk-test-key"));
        match again
            .wait("img_0000000000aa", Duration::from_millis(10))
            .await
        {
            Waited::Failed(why) => assert!(why.contains("다시 시작"), "{why}"),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
