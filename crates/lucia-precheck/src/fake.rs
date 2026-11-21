//! 시험용 소스: 정해 둔 결과를 차례로 돌려주고, 받은 기준점을 기록한다.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::{CheckOutput, Source, SourceError};

pub struct FakeSource {
    name: String,
    script: VecDeque<Result<CheckOutput, SourceError>>,
    seen_cursors: Arc<Mutex<Vec<Option<String>>>>,
}

impl FakeSource {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            script: VecDeque::new(),
            seen_cursors: Arc::default(),
        }
    }

    /// 다음 확인에서 돌려줄 결과를 더한다.
    pub fn then(mut self, result: Result<CheckOutput, SourceError>) -> Self {
        self.script.push_back(result);
        self
    }

    /// 지금까지 받은 기준점(확인 순서대로).
    pub fn seen_cursors(&self) -> Arc<Mutex<Vec<Option<String>>>> {
        self.seen_cursors.clone()
    }
}

impl Source for FakeSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn check(&mut self, cursor: Option<&str>) -> Result<CheckOutput, SourceError> {
        self.seen_cursors
            .lock()
            .unwrap()
            .push(cursor.map(str::to_owned));
        self.script.pop_front().unwrap_or_else(|| {
            Ok(CheckOutput {
                cursor: cursor.map(str::to_owned),
                changes: Vec::new(),
            })
        })
    }
}
