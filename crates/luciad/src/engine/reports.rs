//! 주간 자기 보고와 저녁 미리보기 실행(결정 0035). 규칙은 `crate::reports`에 있다.

use chrono::Duration;
use lucia_store::{EpisodeKind, NewEpisode, Origin};
use uuid::Uuid;

use super::{
    AgentRunner, ChannelAdapter, ContextBuilder, Engine, REPORT_NUDGE, RunProfile, SessionStart,
    failure_reason, report_missing, resident_session,
};
use crate::reports as rules;

const WEEKLY_INSTRUCTIONS: &str = concat!("\
주간 자기 보고를 쓴다. 위 weekly_stats는 데몬 기록에서 모은 지난 7일의 숫자다(지어내지 말고 이것과 goals·기억에 있는 것만 쓴다).
네 덩어리로 짧게 쓴다: 한 일(목표별 진행, 사람 없이 한 일), 못 한 일(막힘과 까닭, 만료된 카드, 실패하거나 멈춘 루틴), 비용(합계와 많이 쓴 곳), 제안.
제안은 실행하지 않는다. 다음 주에 바꾸면 좋을 것(끌 루틴, 늘릴 감시, 만들 스킬, 필요한 사이트 위임, 줄일 알림)을 \"제안: \"으로 시작하는 줄에 하나씩, 사용자가 그대로 Lucia에게 말할 문장으로 쓴다(예: \"제안: 과제 메일 루틴을 하루 한 번으로 줄여 줘\"). 5개까지. 데몬이 이 줄들을 떼어 [진행] 카드로 올린다.
결과는 report_result로 정확히 한 번 보고한다. ", super::notice_split_rule!(), "
", super::voice_rule!());

const EVENING_INSTRUCTIONS: &str = concat!("\
저녁 미리보기다. calendar_events로 내일 일정을 보고, deadlines(챙기는 마감)·goals(목표)·progress(약속)와 엮어서 \"내일 챙길 것\"을 짧게 정리한다: 시각 순 일정, 내일·모레 마감, 목표의 다음 단계, 준비물. 오늘 이미 끝난 일은 빼고, 없는 것을 지어내지 않는다.
챙길 것이 없으면 summary를 `-` 하나로 보고한다(그러면 아무것도 보내지 않는다).
결과는 report_result로 정확히 한 번 보고한다. ", super::notice_split_rule!(), "
", super::voice_rule!());

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportOutcome {
    Sent { suggestions: usize },
    Quiet,
    Skipped(String),
    Failed(String),
}

impl<R: AgentRunner, C: ChannelAdapter> Engine<R, C> {
    /// 주간 자기 보고(작업 큐 안).
    pub async fn run_weekly_review(&self) -> anyhow::Result<ReportOutcome> {
        if !rules::enabled(&self.store(), rules::WEEKLY_KEY) {
            return Ok(ReportOutcome::Skipped("꺼짐".into()));
        }
        let now = self.clock.now();
        let tz = self.cfg.timezone;
        let block = {
            let store = self.store();
            format!(
                "{}{}",
                rules::weekly_stats(&store, now, tz),
                crate::goals::context_block(&store, now, tz)
            )
        };
        self.run_report(
            "weekly_review",
            "이번 주 돌아보기",
            &block,
            WEEKLY_INSTRUCTIONS,
            true,
        )
        .await
    }

    /// 저녁 미리보기(작업 큐 안).
    pub async fn run_evening_preview(&self) -> anyhow::Result<ReportOutcome> {
        if !rules::enabled(&self.store(), rules::EVENING_KEY) {
            return Ok(ReportOutcome::Skipped("꺼짐".into()));
        }
        let now = self.clock.now();
        let tz = self.cfg.timezone;
        let block = {
            let store = self.store();
            format!(
                "{}{}",
                crate::goals::context_block(&store, now, tz),
                crate::attention::context_block(&store, tz)
            )
        };
        let tomorrow = (now + Duration::days(1))
            .with_timezone(&tz)
            .format("%Y-%m-%d (%a)");
        self.run_report(
            "evening_preview",
            &format!("내일({tomorrow}) 챙길 것"),
            &block,
            EVENING_INSTRUCTIONS,
            false,
        )
        .await
    }

    /// 보고 실행 한 번: 상주 세션, 브리핑 범위(사람 없음), 결과를 알림 대화로.
    async fn run_report(
        &self,
        purpose: &str,
        request: &str,
        block: &str,
        instructions: &str,
        weekly: bool,
    ) -> anyhow::Result<ReportOutcome> {
        let now = self.clock.now();
        let tz = self.cfg.timezone;
        if let Some(why) = self.budget_blocked().await {
            return Ok(ReportOutcome::Skipped(why));
        }
        let notify = match self.notify_target() {
            Some(t) if self.channel.ready() => t,
            _ => return Ok(ReportOutcome::Skipped("메신저가 연결되지 않았다".into())),
        };
        let prompt = {
            let store = self.store();
            let memories = store.memories()?;
            let open = store.open_followups()?;
            ContextBuilder::new(tz, self.cfg.budget, now)
                .memory_index(&memories)
                .progress(&open)
                .finish_with(purpose, request, block, instructions)
        };
        let first_id = format!("{}-{}", purpose.replace('_', "-"), Uuid::new_v4().simple());
        let mut attempt = first_id.clone();
        let mut retried = false;
        let (mut run_id, session, mut response, mut finished) = loop {
            let session = resident_session(&mut self.store(), &self.cfg, now)?;
            let (response, finished) = self
                .run_plain(
                    &attempt,
                    session,
                    prompt.clone(),
                    RunProfile::Background,
                    self.models.briefing(),
                    purpose,
                )
                .await?;
            if response.session_missing && !retried {
                retried = true;
                attempt = format!("{first_id}-retry");
                continue;
            }
            break (attempt, session, response, finished);
        };
        if report_missing(&response) {
            let nudge_id = format!("{run_id}-report");
            let (r, f) = self
                .run_plain(
                    &nudge_id,
                    SessionStart::Resume(session.id()),
                    REPORT_NUDGE.to_owned(),
                    RunProfile::ReportOnly,
                    self.models.briefing(),
                    purpose,
                )
                .await?;
            (run_id, response, finished) = (nudge_id, r, f);
        }
        let Some(report) = response.single_report() else {
            let why = failure_reason(&response);
            self.alert(format!("{request}을(를) 만들지 못했습니다: {why}"))
                .await;
            return Ok(ReportOutcome::Failed(why));
        };
        let mut text = report.summary.clone();
        if let Some(d) = report.details.as_deref() {
            text.push_str("\n%%\n");
            text.push_str(d);
        }
        let (body, suggestions) = if weekly {
            rules::split_suggestions(&text)
        } else {
            (text.trim().to_owned(), Vec::new())
        };
        if body.is_empty() || body == "-" {
            return Ok(ReportOutcome::Quiet);
        }
        let sent = self
            .deliver(
                &notify,
                crate::segments::split(&body),
                &report.attachments,
                Some(&run_id),
            )
            .await?;
        let joined = crate::segments::join(&sent);
        {
            let store = self.store();
            store.add_episode(&NewEpisode {
                at: finished,
                kind: EpisodeKind::AgentReport,
                origin: Origin::Agent,
                conversation: Some(notify.0.clone()),
                session_id: Some(session.id().to_string()),
                run_id: Some(run_id.clone()),
                body: format!("[{request}] {joined}"),
            })?;
            if !weekly {
                rules::remember_evening(&store, &joined, finished);
            }
        }
        let mut posted = 0;
        for s in &suggestions {
            if let (Some(id), Some(posts)) = (
                rules::request_suggestion(&self.store(), s, Some(&run_id), finished),
                &self.approvals,
            ) {
                let _ = posts.send(id);
                posted += 1;
            }
        }
        Ok(ReportOutcome::Sent {
            suggestions: posted,
        })
    }
}
