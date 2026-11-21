//! 반복 루틴과 스크립트 상태(결정 0025). 발사는 루틴 ID와 같은 일회성 스케줄이 맡고(실행마다 다음 것을 건다),
//! 여기에는 루틴의 내용·상태·기록을 둔다.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension as _, params};
use serde_json::Value;

use crate::{Result, Store, StoreError, parse_opt_ts, parse_ts, ts};

/// 루틴 수 [제안].
pub const MAX_ROUTINES: usize = 30;

/// 언제 도는가.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Trigger {
    /// cron 식(사용자 시간대).
    Cron(String),
    /// 지난 실행에서 몇 분 뒤. `window`("HH:MM-HH:MM", 사용자 시간대)가 있으면 그 안에서만.
    Every {
        minutes: i64,
        window: Option<String>,
    },
    /// 사전 체크가 이 소스(예: `gmail`, `watch:3`)에서 새 변화를 찾았을 때.
    Change(String),
}

impl Trigger {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Cron(_) => "cron",
            Self::Every { .. } => "every",
            Self::Change(_) => "change",
        }
    }

    pub fn spec(&self) -> String {
        match self {
            Self::Cron(c) => c.clone(),
            Self::Every { minutes, window } => match window {
                Some(w) => format!("{minutes} {w}"),
                None => minutes.to_string(),
            },
            Self::Change(s) => s.clone(),
        }
    }

    pub fn parse(kind: &str, spec: &str) -> Option<Self> {
        Some(match kind {
            "cron" => Self::Cron(spec.to_owned()),
            "every" => {
                let mut parts = spec.splitn(2, ' ');
                let minutes = parts.next()?.parse().ok()?;
                Self::Every {
                    minutes,
                    window: parts.next().map(str::to_owned),
                }
            }
            "change" => Self::Change(spec.to_owned()),
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum RoutineStatus {
    /// 사람의 승인을 기다린다(사용자 자료를 읽고 바깥으로 가져가는 스크립트).
    Review,
    Active,
    Paused,
    /// 끝났다(스크립트가 끝냈거나, 기한이 지났다).
    Done,
}

impl RoutineStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Done => "done",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "review" => Self::Review,
            "active" => Self::Active,
            "paused" => Self::Paused,
            "done" => Self::Done,
            other => return Err(StoreError::Corrupt(format!("routine status {other:?}"))),
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Review => "승인 대기",
            Self::Active => "켜짐",
            Self::Paused => "멈춤",
            Self::Done => "끝남",
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub trigger: Trigger,
    pub task: String,
    pub script: Option<String>,
    pub tools: Vec<String>,
    pub input: Value,
    pub status: RoutineStatus,
    pub conversation: Option<String>,
    pub until: Option<DateTime<Utc>>,
    /// 스크립트가 한 번 정한 다음 실행.
    pub next_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_ok: Option<bool>,
    pub last_text: Option<String>,
    pub runs: i64,
    pub wakes: i64,
    /// 이어진 실패 수.
    pub failures: i64,
}

/// 새 루틴.
#[derive(Debug, Clone)]
pub struct NewRoutine<'a> {
    pub name: &'a str,
    pub trigger: &'a Trigger,
    pub task: &'a str,
    pub script: Option<&'a str>,
    pub tools: &'a [String],
    pub input: &'a Value,
    pub status: RoutineStatus,
    pub conversation: Option<&'a str>,
    pub until: Option<DateTime<Utc>>,
}

/// 실행 기록 한 줄.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RoutineRun {
    pub at: DateTime<Utc>,
    pub ok: bool,
    pub woke: bool,
    pub text: String,
}

const COLUMNS: &str = "id, name, trigger_kind, trigger_spec, task, script, tools, input, status, conversation, until_at, next_at, created_at, updated_at, last_run_at, last_ok, last_text, runs, wakes, failures";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Routine>> {
    let kind: String = r.get(2)?;
    let spec: String = r.get(3)?;
    let tools: String = r.get(6)?;
    let input: String = r.get(7)?;
    let status: String = r.get(8)?;
    let until: Option<String> = r.get(10)?;
    let next: Option<String> = r.get(11)?;
    let created: String = r.get(12)?;
    let updated: String = r.get(13)?;
    let last_run: Option<String> = r.get(14)?;
    let id: String = r.get(0)?;
    let name: String = r.get(1)?;
    let task: String = r.get(4)?;
    let script: Option<String> = r.get(5)?;
    let conversation: Option<String> = r.get(9)?;
    let last_ok: Option<bool> = r.get(15)?;
    let last_text: Option<String> = r.get(16)?;
    let (runs, wakes, failures): (i64, i64, i64) = (r.get(17)?, r.get(18)?, r.get(19)?);
    Ok((|| {
        Ok(Routine {
            trigger: Trigger::parse(&kind, &spec)
                .ok_or_else(|| StoreError::Corrupt(format!("routine trigger {kind:?} {spec:?}")))?,
            tools: serde_json::from_str(&tools).unwrap_or_default(),
            input: serde_json::from_str(&input).unwrap_or(Value::Object(Default::default())),
            status: RoutineStatus::parse(&status)?,
            until: parse_opt_ts(until)?,
            next_at: parse_opt_ts(next)?,
            created_at: parse_ts(&created)?,
            updated_at: parse_ts(&updated)?,
            last_run_at: parse_opt_ts(last_run)?,
            id,
            name,
            task,
            script,
            conversation,
            last_ok,
            last_text,
            runs,
            wakes,
            failures,
        })
    })())
}

impl Store {
    /// 루틴을 `id`로 만든다(ID는 데몬이 정한다). 상한에 닿았으면 그 까닭.
    pub fn create_routine(
        &self,
        id: &str,
        r: &NewRoutine<'_>,
        now: DateTime<Utc>,
    ) -> Result<std::result::Result<(), String>> {
        let live: i64 = self.conn.query_row(
            "SELECT count(*) FROM routines WHERE status != 'done'",
            [],
            |row| row.get(0),
        )?;
        if live as usize >= MAX_ROUTINES {
            return Ok(Err(format!(
                "루틴이 {MAX_ROUTINES}개라 더 만들 수 없다. 안 쓰는 것을 지운다"
            )));
        }
        self.conn.execute(
            "INSERT INTO routines (id, name, trigger_kind, trigger_spec, task, script, tools, input, status, conversation, until_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
            params![
                id,
                r.name.trim(),
                r.trigger.kind(),
                r.trigger.spec(),
                r.task.trim(),
                r.script,
                serde_json::to_string(r.tools).unwrap_or_else(|_| "[]".into()),
                r.input.to_string(),
                r.status.as_str(),
                r.conversation,
                r.until.map(ts),
                ts(now)
            ],
        )?;
        Ok(Ok(()))
    }

    pub fn routine(&self, id: &str) -> Result<Option<Routine>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM routines WHERE id = ?1"),
                [id],
                row,
            )
            .optional()?
            .transpose()
    }

    /// 모든 루틴(만든 순).
    pub fn routines(&self) -> Result<Vec<Routine>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM routines ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([], row)?;
        rows.map(|r| r?).collect()
    }

    /// 이 소스의 변화로 도는 켜진 루틴. `watch`는 모든 감시(`watch:*`)에 맞는다.
    pub fn routines_on_change(&self, source: &str) -> Result<Vec<Routine>> {
        Ok(self
            .routines()?
            .into_iter()
            .filter(|r| r.status == RoutineStatus::Active)
            .filter(|r| match &r.trigger {
                Trigger::Change(s) => s == source || (s == "watch" && source.starts_with("watch:")),
                _ => false,
            })
            .collect())
    }

    pub fn set_routine_status(
        &self,
        id: &str,
        status: RoutineStatus,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE routines SET status = ?2, updated_at = ?3, failures = CASE WHEN ?2 = 'active' THEN 0 ELSE failures END WHERE id = ?1",
            params![id, status.as_str(), ts(now)],
        )? > 0)
    }

    /// 고칠 수 있는 칸(이름, 언제, 할 일, 스크립트, 도구, 입력, 기한)을 쓴다.
    pub fn update_routine(&self, r: &Routine, now: DateTime<Utc>) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE routines SET name = ?2, trigger_kind = ?3, trigger_spec = ?4, task = ?5, script = ?6, tools = ?7,
                                 input = ?8, until_at = ?9, next_at = NULL, updated_at = ?10
             WHERE id = ?1",
            params![
                r.id,
                r.name.trim(),
                r.trigger.kind(),
                r.trigger.spec(),
                r.task.trim(),
                r.script,
                serde_json::to_string(&r.tools).unwrap_or_else(|_| "[]".into()),
                r.input.to_string(),
                r.until.map(ts),
                ts(now)
            ],
        )? > 0)
    }

    /// 루틴과 그 상태·기록을 지운다.
    pub fn delete_routine(&mut self, id: &str) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let n = tx.execute("DELETE FROM routines WHERE id = ?1", [id])?;
        tx.execute(
            "DELETE FROM script_state WHERE owner = ?1",
            [format!("routine:{id}")],
        )?;
        tx.execute("DELETE FROM routine_runs WHERE routine = ?1", [id])?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// 발사 하나(스크립트 실행)를 남긴다. 실패면 이어진 실패 수를 올리고, 성공이면 0으로.
    /// `next_at`은 스크립트가 정한 다음 실행. Lucia를 깨운 것은 [`Store::record_routine_wake`]가 따로 남긴다.
    pub fn record_routine_run(
        &self,
        id: &str,
        ok: bool,
        text: &str,
        next_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let text: String = text.chars().take(500).collect();
        self.conn.execute(
            "UPDATE routines SET runs = runs + 1, failures = CASE WHEN ?2 THEN 0 ELSE failures + 1 END,
                                 last_run_at = ?3, last_ok = ?2, last_text = ?4, next_at = ?5
             WHERE id = ?1",
            params![id, ok, ts(now), text, next_at.map(ts)],
        )?;
        self.conn.execute(
            "INSERT INTO routine_runs (routine, at, ok, woke, text) VALUES (?1, ?2, ?3, 0, ?4)",
            params![id, ts(now), ok, text],
        )?;
        // 루틴마다 최근 50개만 둔다.
        self.conn.execute(
            "DELETE FROM routine_runs WHERE routine = ?1 AND id NOT IN
               (SELECT id FROM routine_runs WHERE routine = ?1 ORDER BY id DESC LIMIT 50)",
            [id],
        )?;
        Ok(())
    }

    /// 루틴이 Lucia(모델)를 깨운 실행을 남긴다(하루 깨우기 상한이 센다). 발사 수(`runs`)는 늘리지 않는다.
    pub fn record_routine_wake(
        &self,
        id: &str,
        ok: bool,
        text: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let text: String = text.chars().take(500).collect();
        self.conn.execute(
            "UPDATE routines SET wakes = wakes + 1, last_ok = ?2, last_text = ?3 WHERE id = ?1",
            params![id, ok, text],
        )?;
        self.conn.execute(
            "INSERT INTO routine_runs (routine, at, ok, woke, text) VALUES (?1, ?2, ?3, 1, ?4)",
            params![id, ts(now), ok, text],
        )?;
        Ok(())
    }

    pub fn routine_runs(&self, id: &str, limit: usize) -> Result<Vec<RoutineRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT at, ok, woke, text FROM routine_runs WHERE routine = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![id, limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, bool>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|r| {
            let (at, ok, woke, text) = r?;
            Ok(RoutineRun {
                at: parse_ts(&at)?,
                ok,
                woke,
                text,
            })
        })
        .collect()
    }

    /// `since` 뒤에 루틴이 Lucia를 깨운 횟수(하루 상한).
    pub fn routine_wakes_since(&self, since: DateTime<Utc>) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM routine_runs WHERE woke = 1 AND at > ?1",
            [ts(since)],
            |r| r.get(0),
        )?)
    }

    pub fn script_state(&self, owner: &str) -> Result<Option<Value>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM script_state WHERE owner = ?1",
                [owner],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|v| serde_json::from_str(&v).ok()))
    }

    pub fn set_script_state(&self, owner: &str, value: &Value, now: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO script_state (owner, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (owner) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![owner, value.to_string(), ts(now)],
        )?;
        Ok(())
    }

    pub fn clear_script_state(&self, owner: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM script_state WHERE owner = ?1", [owner])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::at;
    use serde_json::json;

    fn new<'a>(trigger: &'a Trigger, tools: &'a [String], input: &'a Value) -> NewRoutine<'a> {
        NewRoutine {
            name: "과제 메일 확인",
            trigger,
            task: "새 과제 메일을 요약해 알린다",
            script: Some("print(1)"),
            tools,
            input,
            status: RoutineStatus::Active,
            conversation: Some("discord:1"),
            until: None,
        }
    }

    #[test]
    fn routines_roundtrip_runs_and_state() {
        let mut store = Store::open_in_memory().unwrap();
        let every = Trigger::Every {
            minutes: 45,
            window: Some("09:00-21:00".into()),
        };
        let tools = vec!["mail_search".to_owned()];
        let input = json!({"from": "교수"});
        let id = "routine-a1".to_owned();
        store
            .create_routine(&id, &new(&every, &tools, &input), at(9, 0))
            .unwrap()
            .unwrap();
        let r = store.routine(&id).unwrap().unwrap();
        assert_eq!(r.trigger, every);
        assert_eq!(r.tools, tools);
        assert_eq!(r.input, input);
        assert_eq!(r.status, RoutineStatus::Active);
        assert_eq!(
            Trigger::parse("every", "30"),
            Some(Trigger::Every {
                minutes: 30,
                window: None
            })
        );

        store
            .record_routine_run(&id, false, "오류", None, at(9, 45))
            .unwrap();
        store
            .record_routine_run(&id, false, "오류", None, at(10, 30))
            .unwrap();
        assert_eq!(store.routine(&id).unwrap().unwrap().failures, 2);
        store
            .record_routine_run(&id, true, "알림 1건", Some(at(11, 0)), at(10, 40))
            .unwrap();
        store
            .record_routine_wake(&id, true, "요약을 보냄", at(10, 41))
            .unwrap();
        let r = store.routine(&id).unwrap().unwrap();
        assert_eq!((r.runs, r.wakes, r.failures), (3, 1, 0));
        assert_eq!(r.next_at, Some(at(11, 0)));
        assert_eq!(store.routine_wakes_since(at(10, 0)).unwrap(), 1);
        let runs = store.routine_runs(&id, 10).unwrap();
        assert_eq!((runs[0].text.as_str(), runs[0].woke), ("요약을 보냄", true));
        assert_eq!(runs[1].text, "알림 1건");

        let owner = format!("routine:{id}");
        store
            .set_script_state(&owner, &json!({"seen": ["m1"]}), at(10, 40))
            .unwrap();
        assert_eq!(
            store.script_state(&owner).unwrap(),
            Some(json!({"seen": ["m1"]}))
        );

        let gmail = Trigger::Change("gmail".into());
        let watch = Trigger::Change("watch".into());
        let (a, b) = ("routine-g".to_owned(), "routine-w".to_owned());
        store
            .create_routine(&a, &new(&gmail, &tools, &input), at(9, 0))
            .unwrap()
            .unwrap();
        store
            .create_routine(&b, &new(&watch, &tools, &input), at(9, 0))
            .unwrap()
            .unwrap();
        let ids = |src: &str| -> Vec<String> {
            store
                .routines_on_change(src)
                .unwrap()
                .into_iter()
                .map(|r| r.id)
                .collect()
        };
        assert_eq!(ids("gmail"), vec![a.clone()]);
        assert_eq!(ids("watch:3"), vec![b.clone()]);
        store
            .set_routine_status(&a, RoutineStatus::Paused, at(9, 1))
            .unwrap();
        assert!(ids("gmail").is_empty(), "멈춘 루틴은 돌지 않는다");

        assert!(store.delete_routine(&id).unwrap());
        assert!(store.script_state(&owner).unwrap().is_none());
        assert!(store.routine_runs(&id, 10).unwrap().is_empty());
    }

    #[test]
    fn routine_count_is_capped() {
        let store = Store::open_in_memory().unwrap();
        let t = Trigger::Cron("0 9 * * *".into());
        let input = json!({});
        for i in 0..MAX_ROUTINES {
            store
                .create_routine(&format!("routine-{i}"), &new(&t, &[], &input), at(9, 0))
                .unwrap()
                .unwrap();
        }
        assert!(
            store
                .create_routine("routine-x", &new(&t, &[], &input), at(9, 0))
                .unwrap()
                .is_err()
        );
    }
}
