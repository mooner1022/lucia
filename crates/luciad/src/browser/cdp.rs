//! 작은 CDP(Chrome DevTools Protocol) 클라이언트. 웹소켓은 아무 바이트 통로 위에서 연다
//! (실제로는 `sbx exec -i` 표준 입출력, PoC P8).
//!
//! - 호출마다 시간 제한을 둔다. 연결이 끊기면 기다리던 호출은 모두 오류로 끝난다.
//! - 이벤트는 broadcast로 나눠 준다(대화상자 자동 닫기, 페이지 로드 기다리기).
//! - 매개변수와 결과를 로그에 남기지 않는다(로그인 입력에 비밀번호가 실린다).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{broadcast, oneshot};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

/// 웹소켓 아래 바이트 통로.
pub trait Duplex: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Duplex for T {}

type Sink = SplitSink<WebSocketStream<Box<dyn Duplex>>, Message>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// CDP 이벤트.
#[derive(Debug, Clone)]
pub struct Event {
    pub method: String,
    pub params: Value,
    /// 붙은 대상(페이지)의 세션. 브라우저 수준 이벤트면 `None`.
    pub session: Option<String>,
}

pub struct Cdp {
    sink: tokio::sync::Mutex<Sink>,
    pending: Pending,
    events: broadcast::Sender<Event>,
    next: AtomicU64,
    alive: Arc<AtomicBool>,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for Cdp {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl Cdp {
    /// `url`(예: `ws://127.0.0.1:9222/devtools/browser/<id>`)로 웹소켓 핸드셰이크를 하고 읽기 작업을 띄운다.
    pub async fn connect(stream: Box<dyn Duplex>, url: &str) -> Result<Self, String> {
        let (ws, _response) = tokio_tungstenite::client_async(url, stream)
            .await
            .map_err(|e| format!("CDP 웹소켓 연결: {e}"))?;
        let (sink, mut source) = ws.split();
        let pending: Pending = Arc::default();
        let (events, _) = broadcast::channel(256);
        let alive = Arc::new(AtomicBool::new(true));
        let reader = {
            let pending = pending.clone();
            let events = events.clone();
            let alive = alive.clone();
            tokio::spawn(async move {
                while let Some(message) = source.next().await {
                    let text = match message {
                        Ok(Message::Text(t)) => t,
                        Ok(Message::Close(_)) | Err(_) => break,
                        Ok(_) => continue,
                    };
                    let Ok(v) = serde_json::from_str::<Value>(text.as_str()) else {
                        continue;
                    };
                    if let Some(id) = v.get("id").and_then(Value::as_u64) {
                        let waiter = pending.lock().expect("pending lock").remove(&id);
                        if let Some(tx) = waiter {
                            let result = match v.get("error") {
                                Some(e) => Err(e
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("알 수 없는 오류")
                                    .to_owned()),
                                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(result);
                        }
                    } else if let Some(method) = v.get("method").and_then(Value::as_str) {
                        let _ = events.send(Event {
                            method: method.to_owned(),
                            params: v.get("params").cloned().unwrap_or(Value::Null),
                            session: v
                                .get("sessionId")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                        });
                    }
                }
                alive.store(false, Ordering::SeqCst);
                for (_, tx) in pending.lock().expect("pending lock").drain() {
                    let _ = tx.send(Err("CDP 연결이 끊겼다".into()));
                }
            })
        };
        Ok(Self {
            sink: tokio::sync::Mutex::new(sink),
            pending,
            events,
            next: AtomicU64::new(1),
            alive,
            reader,
        })
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// 이벤트 구독. 명령을 보내기 **전에** 구독해야 그 명령이 일으킨 이벤트를 놓치지 않는다.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// 명령 하나. `session`이 있으면 그 대상(페이지)에 보낸다.
    pub async fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        if !self.alive() {
            return Err("CDP 연결이 끊겼다".into());
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending lock").insert(id, tx);
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            message["sessionId"] = json!(s);
        }
        let sent = self
            .sink
            .lock()
            .await
            .send(Message::Text(message.to_string().into()))
            .await;
        if let Err(e) = sent {
            self.pending.lock().expect("pending lock").remove(&id);
            return Err(format!("{method}: 보내지 못했다({e})"));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(format!("{method}: {e}")),
            Ok(Err(_)) => Err(format!("{method}: CDP 연결이 끊겼다")),
            Err(_) => {
                self.pending.lock().expect("pending lock").remove(&id);
                Err(format!("{method}: 시간 초과({}초)", timeout.as_secs()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 가짜 Chrome: 웹소켓 서버 쪽에서 명령을 받아 정해진 대로 답한다.
    async fn fake_chrome() -> (Cdp, tokio::task::JoinHandle<Vec<Value>>) {
        let (client, server) = tokio::io::duplex(1 << 16);
        let server = tokio::spawn(async move {
            let mut ws = tokio_tungstenite::accept_async(server).await.unwrap();
            let mut seen = Vec::new();
            while let Some(Ok(Message::Text(t))) = ws.next().await {
                let v: Value = serde_json::from_str(t.as_str()).unwrap();
                seen.push(v.clone());
                let id = v["id"].clone();
                let reply = match v["method"].as_str().unwrap() {
                    "Echo.ok" => json!({"id": id, "result": {"got": v["params"]}}),
                    "Echo.fail" => json!({"id": id, "error": {"code": -32000, "message": "안 됨"}}),
                    "Echo.event" => {
                        ws.send(Message::Text(
                            json!({"method": "Page.loadEventFired", "params": {"t": 1}, "sessionId": "s1"})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                        json!({"id": id, "result": {}})
                    }
                    "Echo.silent" => continue,
                    "Echo.close" => break,
                    _ => json!({"id": id, "result": {}}),
                };
                ws.send(Message::Text(reply.to_string().into()))
                    .await
                    .unwrap();
            }
            seen
        });
        let cdp = Cdp::connect(Box::new(client), "ws://127.0.0.1:9222/devtools/browser/x")
            .await
            .unwrap();
        (cdp, server)
    }

    #[tokio::test]
    async fn calls_errors_events_timeouts_and_disconnects() {
        let (cdp, server) = fake_chrome().await;
        let t = Duration::from_secs(2);
        let ok = cdp
            .call(Some("s1"), "Echo.ok", json!({"a": 1}), t)
            .await
            .unwrap();
        assert_eq!(ok["got"]["a"], 1);
        let err = cdp.call(None, "Echo.fail", json!({}), t).await.unwrap_err();
        assert!(err.contains("안 됨"), "{err}");

        let mut events = cdp.subscribe();
        cdp.call(None, "Echo.event", json!({}), t).await.unwrap();
        let e = events.recv().await.unwrap();
        assert_eq!(e.method, "Page.loadEventFired");
        assert_eq!(e.session.as_deref(), Some("s1"));

        let late = cdp
            .call(None, "Echo.silent", json!({}), Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(late.contains("시간 초과"), "{late}");

        let closed = cdp
            .call(None, "Echo.close", json!({}), t)
            .await
            .unwrap_err();
        assert!(closed.contains("끊겼다"), "{closed}");
        assert!(!cdp.alive());
        assert!(cdp.call(None, "Echo.ok", json!({}), t).await.is_err());

        let seen = server.await.unwrap();
        assert_eq!(seen[0]["sessionId"], "s1", "세션 ID를 싣는다");
        assert!(seen[1].get("sessionId").is_none());
    }
}
