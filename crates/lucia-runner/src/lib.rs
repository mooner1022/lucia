//! 실행 백엔드.
//!
//! - 백엔드 트레이트: 구독 인증과 API 키를 설정 하나로 바꾼다(명세 원칙 7)
//! - `claude -p` 호출 조합: PoC에서 확정한다(docs/PLAN.md §5)
//! - stream-json 파서: 테스트는 tools/fake-claude가 기록된 출력을 재생해 한다
//! - sbx 래퍼: 상주 작업 공간, 임시 샌드박스
//!
//! `--bare`는 쓰지 않는다. 훅을 건너뛰고 구독 인증을 읽지 않기 때문이다(docs/PLAN.md §2.2).

pub mod claude;
pub mod report_stream;
pub mod stream_json;

pub use claude::{ClaudeCommand, RunError, RunOutput, SessionMode, sbx_exec_prefix};
pub use stream_json::{ApiRetry, Event, ParseError, RunResult, parse_line};
