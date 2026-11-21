//! 사전 체크.
//!
//! 공통 트레이트([`Source`]) 뒤에 소스 모듈을 둔다. 새 소스는 모듈 하나를 추가하는 것으로 끝낸다.
//! 결과는 규격화된 "바뀐 것" 목록이고, 변화가 없으면 모델을 부르지 않고 끝난다(명세 원칙 6).
//!
//! 기준점(cursor)은 데몬이 저장한다. 소스는 받은 기준점 이후의 변화와 새 기준점만 돌려준다.
//! 기준점이 없으면(첫 동기화) 새 기준점만 잡고, 그때 돌려준 변화는 데몬이 버린다.
//!
//! MVP 소스: Google Calendar(syncToken), Gmail(historyId). 시험용: [`fake::FakeSource`].

pub mod fake;
pub mod google;

use std::future::Future;

use serde_json::Value;

/// 소스가 감지한 변화 하나.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// 같은 변화를 두 번 넣지 않기 위한 키. 버전을 포함한다(예: 일정 ID + 수정 시각).
    pub key: String,
    /// 사람이 읽을 한 줄 요약.
    pub summary: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckOutput {
    /// 다음 확인에 쓸 기준점.
    pub cursor: Option<String>,
    pub changes: Vec<Change>,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// 인증 만료·철회: 사람이 다시 로그인해야 한다(반드시 알린다).
    #[error("인증이 필요하다: {0}")]
    Auth(String),
    /// 기준점이 너무 오래돼 버려졌다(예: syncToken 410). 기준선을 다시 잡아야 한다.
    #[error("기준점이 무효다: {0}")]
    CursorExpired(String),
    /// 일시 오류: 다음 확인 때 다시 한다.
    #[error("일시 오류: {0}")]
    Transient(String),
}

pub trait Source: Send {
    /// 저장소에 쓰는 소스 이름(예: "google-calendar").
    fn name(&self) -> &str;

    /// `cursor` 이후의 변화를 확인한다. `cursor`가 `None`이면 기준선만 잡는다.
    fn check(
        &mut self,
        cursor: Option<&str>,
    ) -> impl Future<Output = Result<CheckOutput, SourceError>> + Send;
}

/// 소스가 하나도 없는 데몬용(Google 연결 전). 값을 만들 수 없는 타입이다.
pub enum NoSource {}

impl Source for NoSource {
    fn name(&self) -> &str {
        match *self {}
    }

    async fn check(&mut self, _cursor: Option<&str>) -> Result<CheckOutput, SourceError> {
        match *self {}
    }
}
