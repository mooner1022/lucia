//! 비용 원장: 실행마다 한 줄(명세 §스케줄링, 사전 체크, 비용 관리).
//! 모델을 부르지 않고 끝난 실행도 남겨, "바뀐 게 없는 날 토큰 0"을 원장으로 확인할 수 있게 한다.

use chrono::{DateTime, Utc};
use rusqlite::params;

use crate::{Result, Store, ts};

#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub run_id: String,
    /// routine | chat | event
    pub origin: String,
    /// morning_briefing | chat | ...
    pub purpose: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub called_model: bool,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub context_tokens: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
    /// ok | no_change | error | ...
    pub outcome: String,
    pub detail: Option<String>,
    /// 모델이 돈 턴 수(결정 0018).
    pub num_turns: u32,
    /// 이 실행이 부른 도구 수.
    pub tool_calls: u32,
}

impl RunRecord {
    /// 모델 없이 끝난 실행.
    pub fn without_model(
        run_id: &str,
        origin: &str,
        purpose: &str,
        started_at: DateTime<Utc>,
        finished_at: DateTime<Utc>,
        outcome: &str,
    ) -> Self {
        Self {
            run_id: run_id.to_owned(),
            origin: origin.to_owned(),
            purpose: purpose.to_owned(),
            started_at,
            finished_at,
            called_model: false,
            model: None,
            session_id: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            context_tokens: 0,
            cost_usd: 0.0,
            duration_ms: 0,
            outcome: outcome.to_owned(),
            detail: None,
            num_turns: 0,
            tool_calls: 0,
        }
    }
}

/// 실행이 부른 도구 하나(결정 0018).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTool {
    pub run_id: String,
    pub name: String,
    pub arg: String,
}

/// 기간 합계.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct SpendSummary {
    pub runs: u64,
    pub model_runs: u64,
    /// 캐시를 거치지 않은 입력 토큰.
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// 캐시에서 읽거나 캐시에 쓴 입력 토큰(대화가 이어지면 입력 대부분이 여기 든다).
    pub cache_tokens: u64,
    pub cost_usd: f64,
}

impl Store {
    pub fn record_run(&self, r: &RunRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO ledger (run_id, origin, purpose, started_at, finished_at, called_model,
                 model, session_id, input_tokens, output_tokens, cache_read_tokens,
                 cache_creation_tokens, context_tokens, cost_usd, duration_ms, outcome, detail,
                 num_turns, tool_calls)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                r.run_id,
                r.origin,
                r.purpose,
                ts(r.started_at),
                ts(r.finished_at),
                r.called_model,
                r.model,
                r.session_id,
                r.input_tokens as i64,
                r.output_tokens as i64,
                r.cache_read_tokens as i64,
                r.cache_creation_tokens as i64,
                r.context_tokens as i64,
                r.cost_usd,
                r.duration_ms as i64,
                r.outcome,
                r.detail,
                r.num_turns,
                r.tool_calls,
            ],
        )?;
        Ok(())
    }

    /// 실행의 도구 흔적을 남긴다(이름과 핵심 인자, 이미 가린 것).
    pub fn record_run_tools(&self, run_id: &str, tools: &[(String, String)]) -> Result<()> {
        for (i, (name, arg)) in tools.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO run_tools (run_id, seq, name, arg) VALUES (?1, ?2, ?3, ?4)",
                params![run_id, i as i64, name, arg],
            )?;
        }
        Ok(())
    }

    /// `since` 뒤에 시작한 실행들의 도구별 사용 횟수(많은 순, 같으면 이름 순). 도구 목록에 바로 보일 도구를
    /// 고르는 데 쓴다(결정 0022).
    pub fn tool_usage_since(&self, since: DateTime<Utc>) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name, COUNT(*) FROM run_tools t JOIN ledger l ON l.run_id = t.run_id
             WHERE l.started_at >= ?1 GROUP BY t.name ORDER BY 2 DESC, 1",
        )?;
        let rows = stmt.query_map([ts(since)], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 실행 ID가 `prefix`로 시작하는 실행들의 도구 흔적(작업 하나의 모든 실행 등).
    pub fn run_tools_with_prefix(&self, prefix: &str) -> Result<Vec<RunTool>> {
        let like = format!("{}%", prefix.replace('%', "").replace('_', "\\_"));
        let mut stmt = self.conn.prepare(
            "SELECT run_id, name, arg FROM run_tools WHERE run_id LIKE ?1 ESCAPE '\\' ORDER BY id",
        )?;
        let rows = stmt.query_map([like], |r| {
            Ok(RunTool {
                run_id: r.get(0)?,
                name: r.get(1)?,
                arg: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 실행 ID가 `prefix`로 시작하는 실행들의 비용 합계(작업 하나의 비용, 결정 0020).
    pub fn cost_with_prefix(&self, prefix: &str) -> Result<f64> {
        let like = format!("{}%", prefix.replace('%', "").replace('_', "\\_"));
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(cost_usd), 0) FROM ledger WHERE run_id LIKE ?1 ESCAPE '\\'",
            [like],
            |r| r.get::<_, f64>(0),
        )?)
    }

    /// 실행 ID가 `prefix`로 시작하는 실행 중 가장 큰 컨텍스트(토큰).
    pub fn max_context_with_prefix(&self, prefix: &str) -> Result<u64> {
        let like = format!("{}%", prefix.replace('%', "").replace('_', "\\_"));
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(context_tokens), 0) FROM ledger WHERE run_id LIKE ?1 ESCAPE '\\'",
            [like],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }

    /// 실행 ID가 `prefix`로 시작하는 실행들의 턴 합계와 도구 호출 합계.
    pub fn turns_with_prefix(&self, prefix: &str) -> Result<(u64, u64)> {
        let like = format!("{}%", prefix.replace('%', "").replace('_', "\\_"));
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(num_turns), 0), COALESCE(SUM(tool_calls), 0) FROM ledger WHERE run_id LIKE ?1 ESCAPE '\\'",
            [like],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
        )?)
    }

    /// `[from, to)` 구간에 시작한 실행의 합계.
    pub fn spend_between(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<SpendSummary> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(called_model), 0), COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0), COALESCE(SUM(cost_usd), 0),
                    COALESCE(SUM(cache_read_tokens + cache_creation_tokens), 0)
             FROM ledger WHERE started_at >= ?1 AND started_at < ?2",
            params![ts(from), ts(to)],
            |r| {
                Ok(SpendSummary {
                    runs: r.get::<_, i64>(0)? as u64,
                    model_runs: r.get::<_, i64>(1)? as u64,
                    input_tokens: r.get::<_, i64>(2)? as u64,
                    output_tokens: r.get::<_, i64>(3)? as u64,
                    cost_usd: r.get(4)?,
                    cache_tokens: r.get::<_, i64>(5)? as u64,
                })
            },
        )?)
    }

    /// 기간의 실행을 목적·모델별로(주간 보고, 결정 0035): (목적, 모델, 실행 수, 비용). 비용 많은 순.
    pub fn spend_by_purpose(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<(String, String, i64, f64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT purpose, COALESCE(model, '-'), COUNT(*), COALESCE(SUM(cost_usd), 0)
             FROM ledger WHERE started_at >= ?1 AND started_at < ?2 AND called_model = 1
             GROUP BY 1, 2 ORDER BY 4 DESC LIMIT 30",
        )?;
        let rows = stmt.query_map(params![ts(from), ts(to)], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 원장의 `detail` 값(오래된 순). 점검·시험용.
    pub fn ledger_details(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT detail FROM ledger WHERE detail IS NOT NULL ORDER BY id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 실행 하나의 기록.
    pub fn run_record(&self, run_id: &str) -> Result<Option<(bool, String)>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT called_model, outcome FROM ledger WHERE run_id = ?1",
                [run_id],
                |r| Ok((r.get::<_, bool>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod run_tool_tests {
    use super::*;
    use crate::test_util::at;

    #[test]
    fn tool_traces_and_turns_add_up_per_task() {
        let store = Store::open_in_memory().unwrap();
        for (run, turns, calls) in [
            ("task-ab12cd34-r1", 7, 5),
            ("task-ab12cd34-r2", 3, 1),
            ("chat-1", 9, 9),
        ] {
            let mut r = RunRecord::without_model(run, "routine", "task", at(9, 0), at(9, 1), "ok");
            r.num_turns = turns;
            r.tool_calls = calls;
            store.record_run(&r).unwrap();
        }
        store
            .record_run_tools(
                "task-ab12cd34-r1",
                &[
                    ("WebSearch".into(), "노트북 최저가".into()),
                    ("browser_open".into(), "https://example.com".into()),
                ],
            )
            .unwrap();
        assert_eq!(store.turns_with_prefix("task-ab12cd34").unwrap(), (10, 6));
        let tools = store.run_tools_with_prefix("task-ab12cd34").unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[1].arg, "https://example.com");
        assert_eq!(store.turns_with_prefix("chat-1").unwrap(), (9, 9));
        assert!(
            store
                .run_tools_with_prefix("task-ab12_d34")
                .unwrap()
                .is_empty(),
            "밑줄은 와일드카드가 아니다"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;

    #[test]
    fn tool_usage_counts_recent_runs_only() {
        let store = Store::open_in_memory().unwrap();
        for (id, hour) in [("old", 1), ("new1", 9), ("new2", 10)] {
            store
                .record_run(&RunRecord::without_model(
                    id,
                    "chat",
                    "chat",
                    at(hour, 0),
                    at(hour, 0),
                    "ok",
                ))
                .unwrap();
        }
        let t = |n: &str| (n.to_owned(), String::new());
        store
            .record_run_tools("old", &[t("mail_send"), t("mail_send"), t("mail_send")])
            .unwrap();
        store
            .record_run_tools("new1", &[t("browser_open"), t("mail_read")])
            .unwrap();
        store
            .record_run_tools("new2", &[t("browser_open")])
            .unwrap();
        store.record_run_tools("no-ledger", &[t("x")]).unwrap();
        assert_eq!(
            store.tool_usage_since(at(8, 0)).unwrap(),
            vec![("browser_open".into(), 2), ("mail_read".into(), 1)]
        );
    }

    #[test]
    fn records_model_and_no_model_runs() {
        let store = Store::open_in_memory().unwrap();
        store
            .record_run(&RunRecord::without_model(
                "r1",
                "routine",
                "morning_briefing",
                at(7, 30),
                at(7, 30),
                "no_change",
            ))
            .unwrap();
        let mut model = RunRecord::without_model("r2", "chat", "chat", at(9, 0), at(9, 1), "ok");
        model.called_model = true;
        model.input_tokens = 4000;
        model.output_tokens = 120;
        model.cost_usd = 0.01;
        store.record_run(&model).unwrap();

        let day = store.spend_between(at(0, 0), at(23, 59)).unwrap();
        assert_eq!(day.runs, 2);
        assert_eq!(day.model_runs, 1);
        assert_eq!(day.input_tokens, 4000);
        assert!((day.cost_usd - 0.01).abs() < 1e-9);

        let morning = store.spend_between(at(7, 0), at(8, 0)).unwrap();
        assert_eq!(morning.model_runs, 0, "변화 없는 아침은 모델 실행 0");
        assert_eq!(
            store.run_record("r1").unwrap(),
            Some((false, "no_change".to_owned()))
        );
        assert!(store.record_run(&model).is_err(), "run_id는 한 번만");
    }
}
