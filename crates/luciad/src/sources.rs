//! 데몬이 쓰는 사전 체크 소스 묶음. Google이 연결돼 있을 때만 Calendar·Gmail을 만든다.
//! 작업자가 루틴 때마다 새로 만들므로 관리 페이지에서 연결·해제하면 다음 확인부터 반영된다.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use lucia_precheck::google::{CALENDAR_BASE, GMAIL_BASE, Gmail, GoogleCalendar};
use lucia_precheck::{CheckOutput, Source, SourceError};
use lucia_store::Store;

use crate::google_auth::GoogleAuth;

pub enum DaemonSource {
    Calendar(GoogleCalendar<GoogleAuth>),
    Gmail(Gmail<GoogleAuth>),
}

impl Source for DaemonSource {
    fn name(&self) -> &str {
        match self {
            Self::Calendar(s) => s.name(),
            Self::Gmail(s) => s.name(),
        }
    }

    async fn check(&mut self, cursor: Option<&str>) -> Result<CheckOutput, SourceError> {
        match self {
            Self::Calendar(s) => s.check(cursor).await,
            Self::Gmail(s) => s.check(cursor).await,
        }
    }
}

/// 연결 직후 기준선을 잡는다. 그래서 "연결한 뒤에 온 메일, 바뀐 일정"부터 변화가 된다(첫 사전 체크까지 기다려
/// 기준선을 잡으면 그 사이에 온 메일이 기준선에 묻힌다). 전에 있던 기준점은 다른 계정의 것일 수 있으므로 버린다.
/// 실패한 소스는 기준점 없이 두고 오류만 남긴다(다음 사전 체크가 기준선을 잡는다). 실패한 (소스, 이유)를 돌려준다.
///
/// 작업자의 사전 체크와 겹치면 작업자가 옛 기준점으로 얻은 결과를 나중에 저장할 수 있다. 같은 계정이면 옛 기준점
/// 이후의 변화가 더 보고될 뿐이고, 다른 계정이면 옛 기준점이 무효(404/410)라 기준선을 다시 잡는다.
pub async fn rebaseline<S: Source>(
    sources: &mut [S],
    store: &Mutex<Store>,
) -> Vec<(String, String)> {
    let mut failed = Vec::new();
    for source in sources.iter_mut() {
        let name = source.name().to_owned();
        if let Err(err) = store.lock().expect("store lock").reset_source(&name) {
            failed.push((name, format!("{err:#}")));
            continue;
        }
        let result = source.check(None).await;
        let now = Utc::now();
        let mut store = store.lock().expect("store lock");
        let saved = match result {
            // 기준선을 잡는 확인이므로 변화는 버린다.
            Ok(out) => store
                .commit_source_check(&name, out.cursor.as_deref(), &[], now)
                .map(|_| None),
            Err(err) => {
                let why = err.to_string();
                store
                    .record_source_error(&name, &why, now)
                    .map(|()| Some(why))
            }
        };
        match saved {
            Ok(None) => {}
            Ok(Some(why)) => failed.push((name, why)),
            Err(err) => failed.push((name, format!("{err:#}"))),
        }
    }
    failed
}

/// 지금 쓸 수 있는 소스들: Google(연결돼 있으면). 감시는 작업 큐 밖 전용 타이머가 확인한다(결정 0036).
pub fn current(google: &Arc<GoogleAuth>) -> Vec<DaemonSource> {
    google_sources(google)
}

/// Google 소스만(연결돼 있으면). 연결 직후 기준선을 잡을 때 쓴다.
pub fn google_sources(google: &Arc<GoogleAuth>) -> Vec<DaemonSource> {
    if !google.connected() {
        return Vec::new();
    }
    let http = google.http().clone();
    vec![
        DaemonSource::Calendar(GoogleCalendar::new(
            http.clone(),
            google.clone(),
            CALENDAR_BASE,
        )),
        DaemonSource::Gmail(Gmail::new(http, google.clone(), GMAIL_BASE)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucia_precheck::Change;
    use lucia_precheck::fake::FakeSource;
    use serde_json::json;

    fn output(cursor: &str, changes: usize) -> CheckOutput {
        CheckOutput {
            cursor: Some(cursor.to_owned()),
            changes: (0..changes)
                .map(|i| Change {
                    key: format!("k{i}"),
                    summary: format!("변화 {i}"),
                    payload: json!({}),
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn rebaseline_replaces_old_cursor_and_reports_nothing() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let then = Utc::now() - chrono::Duration::hours(3);
        store
            .lock()
            .unwrap()
            .commit_source_check("mail", Some("old-account"), &[], then)
            .unwrap();

        let mail = FakeSource::new("mail").then(Ok(output("h-100", 2)));
        let seen = mail.seen_cursors();
        let failed = rebaseline(&mut [mail], &store).await;

        assert!(failed.is_empty());
        assert_eq!(*seen.lock().unwrap(), vec![None], "옛 기준점을 쓰지 않는다");
        let store = store.lock().unwrap();
        let s = store.source_state("mail").unwrap().unwrap();
        assert_eq!(s.cursor.as_deref(), Some("h-100"));
        assert!(s.baseline_at.unwrap() > then, "기준선 시각을 새로 잡는다");
        assert!(store.pending_source_changes().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_rebaseline_leaves_no_cursor() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store
            .lock()
            .unwrap()
            .commit_source_check("mail", Some("old-account"), &[], Utc::now())
            .unwrap();

        let mail = FakeSource::new("mail").then(Err(SourceError::Transient("timeout".into())));
        let cal = FakeSource::new("calendar").then(Ok(output("sync-1", 0)));
        let failed = rebaseline(&mut [mail, cal], &store).await;

        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, "mail");
        let store = store.lock().unwrap();
        let mail = store.source_state("mail").unwrap().unwrap();
        assert!(mail.cursor.is_none(), "다음 사전 체크가 기준선을 잡는다");
        assert!(mail.last_error.is_some());
        let cal = store.source_state("calendar").unwrap().unwrap();
        assert_eq!(cal.cursor.as_deref(), Some("sync-1"));
    }
}
