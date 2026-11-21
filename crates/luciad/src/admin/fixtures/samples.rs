//! 시험 데이터의 상태들(스냅숏). 개편 전에는 서버가 HTML을 그리던 예전 화면의 시험이 같이 썼다(결정 0061).
//!
//! - `healthy`·`fresh`·`troubled`: 모두 정상인 상태, 막 설치한 상태, 문제가 겹친 상태(현실적인 값).
//!   초기 설정(결정 0064)은 `healthy`가 끝난 상태, `fresh`가 막 시작한 상태(안내를 끄지 않음), `troubled`가 끝나지
//!   않았는데 안내를 끈 상태다.
//! - `full`·`alt`·`open_security`: 모든 목록이 채워진 상태와 분기를 바꾼 짝. 문자열 필드마다 표지(`qz-…`, [`q`])가
//!   심겨 있어 화면에서 어느 칸이 어디에 나오는지 가려 볼 수 있다.
//!
//! 값과 순서를 바꾸면 `web/admin/fixtures`가 달라지므로 `ui_fixtures_are_current`가 실패한다.

use chrono::{DateTime, Duration, TimeZone, Utc};
use lucia_store::{
    Activity, Alert, Approval, ApprovalStatus, MemoryAuthor, MemorySection, Schedule, ScheduleKind,
    SessionRole, SessionRow, SourceState, SpendSummary,
};

use crate::admin::snapshot::{
    BackupView, DiscordView, Flash, GoalView, GoogleView, LastRun, ProfileView, RoutineView,
    SetupView, Snapshot, SourceView, Tone,
};
use crate::check::Check;
use crate::discord_ctl::DiscordState;

pub(super) const CSRF: &str = "csrf-fixture-0123456789";

fn kst(d: u32, h: u32, m: u32) -> DateTime<Utc> {
    chrono_tz::Asia::Seoul
        .with_ymd_and_hms(2026, 11, d, h, m, 0)
        .unwrap()
        .with_timezone(&Utc)
}

fn source(name: &str, error: Option<&str>) -> Option<SourceState> {
    Some(SourceState {
        source: name.into(),
        cursor: Some("c".into()),
        baseline_at: Some(kst(16, 17, 53)),
        checked_at: Some(kst(16, 19, 0)),
        last_error: error.map(str::to_owned),
    })
}

fn approval(id: &str, status: ApprovalStatus, summary: &str, result: Option<&str>) -> Approval {
    Approval {
        id: id.into(),
        kind: "mail_send".into(),
        payload: serde_json::json!({}),
        summary: summary.into(),
        status,
        requested_at: kst(16, 19, 7),
        expires_at: kst(17, 19, 7),
        decided_at: None,
        run_id: Some("chat-1".into()),
        conversation: Some("discord:1".into()),
        card_ref: Some("1".into()),
        result: result.map(str::to_owned),
        auto: false,
    }
}

fn act(id: i64, kind: &str, summary: &str, m: u32) -> Activity {
    Activity {
        id,
        at: kst(16, 19, m),
        kind: kind.into(),
        summary: summary.into(),
        run_id: None,
    }
}

/// 모두 연결되고 정상인 상태.
pub(super) fn healthy() -> Snapshot {
    let now = kst(16, 19, 30);
    Snapshot {
        browser: Some(crate::admin::snapshot::BrowserView {
            sandbox: "lucia-browser".into(),
            running: false,
            idle_minutes: 10,
            sites: vec![
                crate::browser::logins::SiteStatus {
                    id: "quasarzone".into(),
                    name: "퀘이사존".into(),
                    origin: "https://quasarzone.com".into(),
                    saved: true,
                    username: Some("me".into()),
                },
                crate::browser::logins::SiteStatus {
                    id: "www.example.com".into(),
                    name: "www.example.com".into(),
                    origin: "https://www.example.com".into(),
                    saved: false,
                    username: Some("me2".into()),
                },
            ],
            blocked: vec!["ads.example.com".into(), "<b>x.example</b>".into()],
            recent_blocks: vec![crate::browser::proxy::Decision {
                at: now - chrono::Duration::minutes(3),
                host: "192.168.0.250.nip.io".into(),
                port: 443,
                blocked: Some("192.168.0.250.nip.io → 192.168.0.250: 사설 주소".into()),
            }],
        }),
        now,
        tz: chrono_tz::Asia::Seoul,
        discord: DiscordView {
            state: DiscordState::Connected {
                bot: "루시아 Lucia".into(),
            },
            token_set: true,
            owner: Some("123456789012345678".into()),
            notify: Some("discord:987654321098765432".into()),
            guild_calls: true,
            prefix: Some("!루시아".into()),
            content_refused: false,
            servers: Some(Ok(vec![
                lucia_channel::discord::server::Server {
                    id: 111,
                    name: "스터디".into(),
                    admin: true,
                    owner: false,
                },
                lucia_channel::discord::server::Server {
                    id: 222,
                    name: "친구들".into(),
                    admin: false,
                    owner: false,
                },
            ])),
        },
        google: GoogleView {
            has_client: true,
            connected: true,
            account: Some("me@example.com".into()),
            scopes: Some(format!(
                "{} {} {} {}",
                crate::google_auth::SCOPE_CALENDAR_READ,
                crate::google_auth::SCOPE_GMAIL_READ,
                crate::google_auth::SCOPE_GMAIL_SEND,
                crate::google_auth::SCOPE_CALENDAR_WRITE
            )),
            consent_minutes_left: None,
            sources: vec![
                SourceView {
                    label: "캘린더",
                    state: source("google-calendar", None),
                },
                SourceView {
                    label: "메일",
                    state: source("gmail", None),
                },
            ],
        },
        checks: [
            "sbx 데몬 kvm 권한",
            "Claude 로그인(sbx 시크릿)",
            "상주 샌드박스",
            "MCP 포트 허용",
            "저장소",
        ]
        .into_iter()
        .map(|name| Check {
            name,
            ok: true,
            detail: "정상".into(),
        })
        .collect(),
        checks_at: Some(kst(16, 19, 25)),
        today: SpendSummary {
            runs: 14,
            model_runs: 9,
            input_tokens: 412_330,
            output_tokens: 6_210,
            cache_tokens: 380_000,
            cost_usd: 0.192,
        },
        week: SpendSummary {
            runs: 30,
            model_runs: 18,
            input_tokens: 812_000,
            output_tokens: 12_400,
            cache_tokens: 760_000,
            cost_usd: 0.41,
        },
        schedules: vec![
            Schedule {
                id: "morning-briefing".into(),
                kind: ScheduleKind::Cron,
                spec: "30 7 * * *".into(),
                timezone: "Asia/Seoul".into(),
                action: "morning_briefing".into(),
                payload: serde_json::json!({}),
                max_delay_secs: 3600,
                next_due_at: Some(kst(17, 7, 30)),
                last_fired_at: None,
                enabled: true,
            },
            Schedule {
                id: "hourly-precheck".into(),
                kind: ScheduleKind::Cron,
                spec: "0 * * * *".into(),
                timezone: "Asia/Seoul".into(),
                action: "precheck".into(),
                payload: serde_json::json!({}),
                max_delay_secs: 1800,
                next_due_at: Some(kst(16, 20, 0)),
                last_fired_at: Some(kst(16, 19, 0)),
                enabled: true,
            },
        ],
        session: Some(SessionRow {
            id: "0a241c2a-610c-4b1b-af85-195abbac8af5".into(),
            role: SessionRole::Resident,
            sandbox: Some("lucia".into()),
            status: "active".into(),
            created_at: kst(16, 17, 23),
            last_used_at: Some(kst(16, 19, 12)),
            last_context_tokens: 27_585,
            started: true,
            cost_total_usd: 0.192,
            last_compacted_at: None,
            runs_since_compact: 11,
        }),
        memories: vec![
            lucia_store::MemoryEntry {
                slug: "preferences".into(),
                section: lucia_store::MemorySection::Me,
                title: "선호".into(),
                summary: "존댓말, 간결하게. 새 메일은 me@example.com로".into(),
                body: "- 말투: 존댓말\n- 메일 규칙은 [[mail-rules]] 참고".into(),
                revision: 7,
                created_at: kst(16, 9, 0),
                updated_at: kst(16, 19, 10),
            },
            lucia_store::MemoryEntry {
                slug: "devices".into(),
                section: lucia_store::MemorySection::Topic,
                title: "기기 <b>".into(),
                summary: "태블릿과 휴대폰".into(),
                body: "# 휴대\n- 갤럭시 Z 폴드 7\n일반 문단".into(),
                revision: 9,
                created_at: kst(16, 9, 0),
                updated_at: kst(16, 18, 0),
            },
        ],
        last_remember: None,
        compact_at_tokens: Some(100_000),
        rotate_tokens: 150_000,
        backup: BackupView {
            enabled: true,
            cron: "0 4 * * *".into(),
            keep_days: 14,
            last: Some("11-16 19:20 · 8개 파일, 808 KB".into()),
        },
        alerts: vec![],
        approvals: vec![approval(
            "a1",
            ApprovalStatus::Sent,
            "메일 → me@example.com · 메일 전송 테스트",
            Some("보냄(Gmail 1a1e3d394f8f0db1)"),
        )],
        activity: vec![
            act(
                5,
                "calendar_read",
                "일정 조회 11-16 00:00 ~ 11-17 00:00 → 0건",
                12,
            ),
            act(
                4,
                "mail_read",
                "메일 읽기: AliExpress · Order confirmed",
                10,
            ),
            act(
                3,
                "mail_search",
                "메일 검색 \"aliexpress newer_than:60d\" → 8건",
                10,
            ),
            act(2, "mail_sent", "보냄(Gmail 1a1e3d394f8f0db1)", 7),
            act(
                1,
                "memory_write",
                "핵심 기억 개정 3: 사용자가 별칭 규칙을 알려 줌",
                5,
            ),
        ],
        approval_ttl_hours: 24,
        last_precheck: Some(LastRun {
            at: kst(16, 19, 0),
            ok: true,
            text: "새 변화 0건".into(),
        }),
        last_briefing: Some(LastRun {
            at: kst(16, 19, 12),
            ok: true,
            text: "브리핑을 보냄".into(),
        }),
        last_backup: Some(LastRun {
            at: kst(16, 19, 20),
            ok: true,
            text: "8개 파일, 808 KB".into(),
        }),
        last_compact: None,
        goals: Vec::new(),
        followups_open: vec![lucia_store::Followup {
            id: "followup-abc".into(),
            created_at: now - chrono::Duration::hours(3),
            due_at: now + chrono::Duration::hours(14),
            task: "치과 예약 <확정> 메일 확인".into(),
            detail: String::new(),
            conversation: None,
            run_id: None,
            origin: "chat".into(),
            status: lucia_store::FollowupStatus::Open,
            closed_at: None,
            result: None,
        }],
        followups_closed: vec![lucia_store::Followup {
            id: "followup-old".into(),
            created_at: now - chrono::Duration::days(1),
            due_at: now - chrono::Duration::hours(2),
            task: "택배 도착 확인".into(),
            detail: String::new(),
            conversation: None,
            run_id: None,
            origin: "chat".into(),
            status: lucia_store::FollowupStatus::Done,
            closed_at: Some(now - chrono::Duration::hours(2)),
            result: Some("도착했다고 알림".into()),
        }],
        tasks_open: vec![],
        tasks_closed: vec![],
        daily_cap: Some(2.5),
        weekly_cap: None,
        quiet_hours: Some("23:00-07:00".into()),
        quiet_calendar: false,
        quiet_except: String::new(),
        quiet_busy: 0,
        notice_rules: Vec::new(),
        nudges: Vec::new(),
        routine_wakes: None,
        task_channel: None,
        keep_days: None,
        watches: vec![],
        watch_now_daily: 20,
        security_mode: crate::security::Mode::Standard,
        routines: vec![],
        scripts_sandbox: None,
        site_grants: vec![],
        egress_block: false,
        signals: Default::default(),
        clients: vec![],
        gateway_binds: vec![],
        access: Default::default(),
        app: Default::default(),
        profile: ProfileView {
            name: crate::profile::DEFAULT_NAME.into(),
            name_set: false,
            avatar: None,
        },
        setup: SetupView::default(),
        expressions: None,
        images: Default::default(),
        kept_files: vec![],
        dev_cost_cap: None,
        dev_allow_hosts: String::new(),
        dev_local_roots: String::new(),
    }
}

pub(super) fn fresh() -> Snapshot {
    let mut s = healthy();
    s.browser = None;
    // 접속 API는 켰지만 앱 토큰도 푸시도 아직 없다(초기 설정의 앱 단계가 폼을 모두 보인다).
    s.gateway_binds = vec!["192.168.0.10:8790".parse().unwrap()];
    s.followups_open = Vec::new();
    s.followups_closed = Vec::new();
    s.daily_cap = None;
    s.quiet_hours = None;
    s.discord = DiscordView {
        state: DiscordState::NotConfigured,
        token_set: false,
        owner: None,
        notify: None,
        guild_calls: false,
        prefix: None,
        content_refused: false,
        servers: None,
    };
    s.google = GoogleView {
        has_client: false,
        connected: false,
        account: None,
        scopes: None,
        consent_minutes_left: None,
        sources: vec![
            SourceView {
                label: "캘린더",
                state: None,
            },
            SourceView {
                label: "메일",
                state: None,
            },
        ],
    };
    s.session = None;
    s.memories.clear();
    s.approvals.clear();
    s.activity.clear();
    s.last_precheck = None;
    s.last_briefing = None;
    s.last_backup = None;
    s.today = SpendSummary::default();
    s.week = SpendSummary::default();
    s
}

/// 여러 문제가 겹친 상태: Discord 멈춤, 발송 범위 없음, 동의 진행 중, 점검 실패, 승인 대기, 못 보낸 알림.
pub(super) fn troubled() -> Snapshot {
    let mut s = healthy();
    s.discord.state = DiscordState::Stopped {
        reason: "토큰이 거부됐다(401)".into(),
    };
    // 소유자가 없어 대화할 곳이 없고, 초기 설정 안내는 껐다(결정 0064).
    s.discord.owner = None;
    s.discord.notify = None;
    s.setup.dismissed = true;
    s.google.scopes = Some(crate::google_auth::SCOPE_GMAIL_READ.into());
    s.google.consent_minutes_left = Some(7);
    s.google.sources[1].state = source("gmail", Some("Gmail history: HTTP 500"));
    s.checks[1].ok = false;
    s.checks[1].detail = "로그인이 만료됐다".into();
    s.approvals.insert(
        0,
        approval(
            "p1",
            ApprovalStatus::Pending,
            "메일 → prof@example.edu · 과제 제출 확인",
            None,
        ),
    );
    s.alerts.push(Alert {
        id: 1,
        at: kst(16, 19, 0),
        text: "Discord 연결이 끊겨 브리핑을 보내지 못했습니다".into(),
        delivered: false,
    });
    s.last_backup = Some(LastRun {
        at: kst(16, 4, 0),
        ok: false,
        text: "sbx cp 실패: sandbox not found".into(),
    });
    s
}

/// 화면 테스트용 주 모델(결정 0055): 설정 파일 값은 sonnet, `picked`면 Opus를 골랐다.
pub(super) fn sample_models(picked: bool) -> crate::models::MainModel {
    let models = crate::models::MainModel::new("sonnet".into(), "sonnet".into());
    if picked {
        let store = lucia_store::Store::open_in_memory().expect("store");
        models
            .set(&store, crate::models::parse("opus"), "시험", Utc::now())
            .expect("set");
    }
    models
}

pub(super) fn q(name: &str) -> String {
    format!("qz-{name}")
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

// ---------- 시험용 데이터 ----------

/// 모든 목록이 채워지고, 문자열 필드마다 표지가 심긴 상태.
pub(super) fn full() -> Snapshot {
    let mut s = healthy();
    let now = s.now;
    let h = |n: i64| now - Duration::hours(n);
    s.discord.state = DiscordState::Connected {
        bot: q("discord-bot"),
    };
    s.discord.owner = Some(q("discord-owner"));
    s.discord.notify = Some(q("discord-notify"));
    s.discord.prefix = Some(q("discord-prefix"));
    s.profile = ProfileView {
        name: q("profile-name"),
        name_set: true,
        avatar: Some(crate::profile::Avatar {
            id: "a_0123456789abcdef01234567".into(),
            mime: "image/png".into(),
            width: 800,
            height: 800,
            size: 123_456,
            set_at: kst(16, 18, 0),
            by: "owner".into(),
        }),
    };
    s.setup.config_path = Some(q("config-path"));
    s.google.account = Some(q("google-account"));
    s.checks = [
        ("check-a", true, "check-a-detail"),
        ("check-b", false, "check-b-detail"),
    ]
    .into_iter()
    .map(|(name, ok, detail)| crate::check::Check {
        name: leak(q(name)),
        ok,
        detail: q(detail),
    })
    .collect();
    s.last_remember = Some(LastRun {
        at: h(1),
        ok: true,
        text: q("last-remember"),
    });
    s.last_precheck = Some(LastRun {
        at: h(1),
        ok: true,
        text: q("last-precheck"),
    });
    s.last_briefing = Some(LastRun {
        at: h(1),
        ok: false,
        text: q("last-briefing"),
    });
    s.last_backup = Some(LastRun {
        at: h(1),
        ok: true,
        text: q("last-backup"),
    });
    s.last_compact = Some(LastRun {
        at: h(1),
        ok: true,
        text: q("last-compact"),
    });
    s.backup.last = Some(q("backup-last"));
    s.alerts = vec![
        lucia_store::Alert {
            id: 1,
            at: h(1),
            text: q("alert-delivered"),
            delivered: true,
        },
        lucia_store::Alert {
            id: 2,
            at: h(2),
            text: q("alert-undelivered"),
            delivered: false,
        },
    ];
    s.approvals = vec![
        approval(
            "p1",
            ApprovalStatus::Pending,
            &q("approval-pending"),
            Some(&q("approval-pending-result")),
        ),
        approval(
            "a2",
            ApprovalStatus::Sent,
            &q("approval-sent"),
            Some(&q("approval-sent-result")),
        ),
        approval("a3", ApprovalStatus::Denied, &q("approval-denied"), None),
        approval("a4", ApprovalStatus::Expired, &q("approval-expired"), None),
        approval(
            "a5",
            ApprovalStatus::Failed,
            &q("approval-failed"),
            Some(&q("approval-failed-result")),
        ),
    ];
    let kinds = [
        "mail_search",
        "mail_read",
        "calendar_read",
        "mail_sent",
        "memory_write",
        "web_search",
        "file_create",
        "approval_requested",
    ];
    s.activity = kinds
        .iter()
        .enumerate()
        .map(|(i, k)| act(i as i64 + 1, k, &q(&format!("activity-{k}")), 10 + i as u32))
        .collect();
    s.memories = vec![
        lucia_store::MemoryEntry {
            slug: q("mem-me"),
            section: MemorySection::Me,
            title: q("mem-me-title"),
            summary: q("mem-me-summary"),
            body: q("mem-me-body"),
            revision: 3,
            created_at: h(5),
            updated_at: h(1),
        },
        lucia_store::MemoryEntry {
            slug: q("mem-topic"),
            section: MemorySection::Topic,
            title: q("mem-topic-title"),
            summary: q("mem-topic-summary"),
            body: q("mem-topic-body"),
            revision: 2,
            created_at: h(5),
            updated_at: h(2),
        },
        lucia_store::MemoryEntry {
            slug: q("mem-area"),
            section: MemorySection::Area,
            title: q("mem-area-title"),
            summary: q("mem-area-summary"),
            body: q("mem-area-body"),
            revision: 1,
            created_at: h(5),
            updated_at: h(3),
        },
    ];

    s.goals = vec![
        GoalView {
            goal: lucia_store::Goal {
                id: q("goal-a"),
                title: q("goal-a-title"),
                purpose: q("goal-a-purpose"),
                success: q("goal-a-success"),
                deadline: Some(now + Duration::days(3)),
                status: lucia_store::GoalStatus::Active,
                interests: q("goal-a-interests"),
                review_every_hours: 24,
                next_review_at: Some(now + Duration::hours(5)),
                conversation: None,
                created_at: h(30),
                updated_at: h(2),
                closed_at: None,
            },
            log: vec![lucia_store::GoalLog {
                at: h(2),
                kind: "review".into(),
                text: q("goal-a-log"),
            }],
            children: vec![
                lucia_store::GoalChild {
                    goal: q("goal-a"),
                    kind: "followup".into(),
                    child: q("goal-a-child"),
                    by_review: true,
                },
                lucia_store::GoalChild {
                    goal: q("goal-a"),
                    kind: "routine".into(),
                    child: q("goal-a-child2"),
                    by_review: false,
                },
            ],
        },
        GoalView {
            goal: lucia_store::Goal {
                id: q("goal-b"),
                title: q("goal-b-title"),
                purpose: String::new(),
                success: q("goal-b-success"),
                deadline: None,
                status: lucia_store::GoalStatus::Waiting,
                interests: String::new(),
                review_every_hours: 24,
                next_review_at: None,
                conversation: None,
                created_at: h(30),
                updated_at: h(2),
                closed_at: None,
            },
            log: Vec::new(),
            children: Vec::new(),
        },
        GoalView {
            goal: lucia_store::Goal {
                id: q("goal-c"),
                title: q("goal-c-title"),
                purpose: String::new(),
                success: q("goal-c-success"),
                deadline: None,
                status: lucia_store::GoalStatus::Done,
                interests: String::new(),
                review_every_hours: 24,
                next_review_at: None,
                conversation: None,
                created_at: h(30),
                updated_at: h(2),
                closed_at: Some(h(1)),
            },
            log: Vec::new(),
            children: Vec::new(),
        },
    ];
    let followup = |id: &str, origin: &str, task: &str| lucia_store::Followup {
        id: q(id),
        created_at: h(3),
        due_at: now + Duration::hours(6),
        task: q(task),
        detail: q(&format!("{task}-detail")),
        conversation: None,
        run_id: None,
        origin: origin.into(),
        status: lucia_store::FollowupStatus::Open,
        closed_at: None,
        result: None,
    };
    s.followups_open = vec![
        followup("fu-chat", "chat", "fu-chat-task"),
        followup("fu-briefing", "briefing", "fu-briefing-task"),
        followup("fu-user", crate::autonomy::ORIGIN_USER, "fu-user-task"),
    ];
    s.followups_closed = vec![
        lucia_store::Followup {
            status: lucia_store::FollowupStatus::Done,
            closed_at: Some(h(2)),
            result: Some(q("fu-done-result")),
            ..followup("fu-done", "chat", "fu-done-task")
        },
        lucia_store::Followup {
            status: lucia_store::FollowupStatus::Failed,
            closed_at: Some(h(2)),
            result: Some(q("fu-failed-result")),
            ..followup("fu-failed", "chat", "fu-failed-task")
        },
        lucia_store::Followup {
            status: lucia_store::FollowupStatus::Cancelled,
            closed_at: Some(h(2)),
            result: None,
            ..followup("fu-cancelled", "chat", "fu-cancelled-task")
        },
    ];
    let task =
        |id: &str, status: lucia_core::TaskStatus, thread: bool, closed: bool| lucia_store::Task {
            id: q(id),
            created_at: h(4),
            updated_at: h(1),
            title: q(&format!("{id}-title")),
            brief: q(&format!("{id}-brief")),
            status,
            initiator: lucia_core::Initiator::Human,
            source: "discord".into(),
            conversation: None,
            thread,
            session_id: None,
            run_id: None,
            result: Some(q(&format!("{id}-result"))),
            closed_at: closed.then(|| h(1)),
            kind: "general".into(),
            repo: None,
            sandbox: None,
            base: None,
        };
    s.tasks_open = vec![
        task("task-run", lucia_core::TaskStatus::Running, true, false),
        task(
            "task-wait",
            lucia_core::TaskStatus::WaitingApproval,
            false,
            false,
        ),
    ];
    s.tasks_closed = vec![
        task("task-done", lucia_core::TaskStatus::Done, false, true),
        task("task-failed", lucia_core::TaskStatus::Failed, false, true),
    ];
    s.weekly_cap = Some(10.0);
    s.quiet_calendar = true;
    s.quiet_except = q("quiet-except");
    s.quiet_busy = 2;
    s.notice_rules = vec![lucia_store::NoticeRule {
        key: q("rule-key"),
        rule: crate::attention::rule::MUTE.into(),
        label: q("rule-label"),
        created_at: h(9),
    }];
    s.nudges = vec![
        lucia_store::Nudge {
            id: q("nudge-a"),
            title: q("nudge-a-title"),
            due_at: now + Duration::days(2),
            note: q("nudge-a-note"),
            goal: None,
            stage: 0,
            status: "open".into(),
            created_at: h(9),
        },
        lucia_store::Nudge {
            id: q("nudge-b"),
            title: q("nudge-b-title"),
            due_at: now + Duration::hours(20),
            note: String::new(),
            goal: None,
            stage: 2,
            status: "open".into(),
            created_at: h(9),
        },
    ];
    s.routine_wakes = Some("12".into());
    s.task_channel = Some("1234567890123456789".into());
    s.keep_days = Some(90);
    s.dev_cost_cap = Some(3.5);
    s.dev_allow_hosts = q("dev-hosts");
    s.dev_local_roots = q("dev-roots");
    s.clients = vec![
        (
            lucia_store::Client {
                id: q("client-a"),
                name: q("client-a-name"),
                role: "app".into(),
                created_at: h(40),
                last_seen_at: Some(h(1)),
                revoked_at: None,
                scopes: vec![crate::gateway::scopes::SECURITY_MODE.into()],
                push_token: Some(q("push-token")),
                push_platform: Some("android".into()),
                push_at: Some(h(2)),
                acked_seq: 3,
            },
            true,
        ),
        (
            lucia_store::Client {
                id: q("client-b"),
                name: q("client-b-name"),
                role: "device".into(),
                created_at: h(40),
                last_seen_at: None,
                revoked_at: None,
                scopes: vec![],
                push_token: None,
                push_platform: None,
                push_at: None,
                acked_seq: 0,
            },
            false,
        ),
        (
            lucia_store::Client {
                id: q("client-c"),
                name: q("client-c-name"),
                role: "app".into(),
                created_at: h(40),
                last_seen_at: Some(h(20)),
                revoked_at: Some(h(10)),
                scopes: vec![],
                push_token: None,
                push_platform: None,
                push_at: None,
                acked_seq: 0,
            },
            false,
        ),
    ];
    s.gateway_binds = vec!["192.168.0.10:8790".parse().unwrap()];
    let routine = |id: &str,
                   status: lucia_store::RoutineStatus,
                   trigger: lucia_store::Trigger,
                   script: bool| RoutineView {
        routine: lucia_store::Routine {
            id: q(id),
            name: q(&format!("{id}-name")),
            trigger,
            task: q(&format!("{id}-task")),
            script: script.then(|| q(&format!("{id}-script"))),
            tools: if script {
                vec![q(&format!("{id}-tool"))]
            } else {
                Vec::new()
            },
            input: serde_json::json!({}),
            status,
            conversation: None,
            until: None,
            next_at: None,
            created_at: h(50),
            updated_at: h(1),
            last_run_at: Some(h(1)),
            last_ok: Some(true),
            last_text: Some(q(&format!("{id}-last"))),
            runs: 7,
            wakes: 2,
            failures: if status == lucia_store::RoutineStatus::Review {
                3
            } else {
                0
            },
        },
        next: Some(now + Duration::hours(2)),
        runs: vec![
            lucia_store::RoutineRun {
                at: h(1),
                ok: true,
                woke: true,
                text: q(&format!("{id}-run-ok")),
            },
            lucia_store::RoutineRun {
                at: h(2),
                ok: false,
                woke: false,
                text: q(&format!("{id}-run-fail")),
            },
        ],
    };
    s.routines = vec![
        routine(
            "rt-active",
            lucia_store::RoutineStatus::Active,
            lucia_store::Trigger::Cron("0 9 * * *".into()),
            true,
        ),
        routine(
            "rt-paused",
            lucia_store::RoutineStatus::Paused,
            lucia_store::Trigger::Every {
                minutes: 90,
                window: Some("09:00-18:00".into()),
            },
            false,
        ),
        routine(
            "rt-review",
            lucia_store::RoutineStatus::Review,
            lucia_store::Trigger::Change("gmail".into()),
            true,
        ),
        routine(
            "rt-done",
            lucia_store::RoutineStatus::Done,
            lucia_store::Trigger::Cron("0 9 * * *".into()),
            false,
        ),
    ];
    s.scripts_sandbox = Some(q("script-sandbox"));
    let grant = |id: &str, origin: &str, login: bool, actions: bool, expires: bool| {
        lucia_store::SiteGrant {
            id: q(id),
            host: format!("{}.example.test", q(id)),
            login,
            actions,
            purpose: q(&format!("{id}-purpose")),
            origin: origin.into(),
            created_at: h(70),
            expires_at: expires.then(|| now + Duration::days(2)),
            revoked_at: None,
            uses: 4,
            last_used_at: Some(h(3)),
        }
    };
    s.site_grants = vec![
        grant("grant-chat", "chat", true, true, false),
        grant("grant-egress", "egress", false, false, true),
        grant("grant-admin", "admin", false, false, false),
        grant("grant-once", "once", false, false, true),
        grant("grant-watch", "watch", false, false, false),
        lucia_store::SiteGrant {
            revoked_at: Some(h(1)),
            ..grant("grant-revoked", "chat", false, false, false)
        },
    ];
    s.egress_block = true;
    s.signals = crate::admin::snapshot::SignalsView {
        enabled: true,
        weekly: true,
        evening: false,
        consistency: true,
        poll_minutes: 7,
        interests: q("signals-interests"),
        classify_daily: 40,
        wake_daily: 9,
        classify_used: 11,
        wake_used: 3,
        recent: ["now", "digest", "drop", "briefing"]
            .into_iter()
            .enumerate()
            .map(|(i, v)| lucia_store::Signal {
                id: i as i64 + 1,
                source: "gmail".into(),
                key: q(&format!("sig-key-{v}")),
                summary: q(&format!("sig-{v}-summary")),
                payload: serde_json::json!({}),
                interest: (i == 0).then(|| q("sig-interest")),
                verdict: Some(v.into()),
                reason: Some(q(&format!("sig-{v}-reason"))),
                received_at: h(i as i64 + 1),
                decided_at: None,
                woke_at: None,
            })
            .chain(std::iter::once(lucia_store::Signal {
                id: 9,
                source: "gmail".into(),
                key: q("sig-key-pending"),
                summary: q("sig-pending-summary"),
                payload: serde_json::json!({}),
                interest: None,
                verdict: None,
                reason: None,
                received_at: h(6),
                decided_at: None,
                woke_at: None,
            }))
            .collect(),
    };
    let watch =
        |id: &str, status: lucia_store::WatchStatus, mode: &str, fetcher: &str, paused: &str| {
            lucia_store::Watch {
                id: q(id),
                url: format!("https://{}.example.test/board", q(id)),
                what: q(&format!("{id}-what")),
                start_text: q(&format!("{id}-start")),
                end_text: q(&format!("{id}-end")),
                interval_minutes: 30,
                status,
                changes_in_row: 0,
                run_id: None,
                created_at: h(20),
                updated_at: h(1),
                mode: mode.into(),
                item_pattern: q(&format!("{id}-pattern")),
                include_words: q(&format!("{id}-include")),
                exclude_words: q(&format!("{id}-exclude")),
                notify: lucia_store::watch_notify::CLASSIFY.into(),
                fetcher: fetcher.into(),
                failures: 2,
                paused_reason: paused.into(),
                sent_day: String::new(),
                sent_count: 0,
            }
        };
    let watch_state = |id: &str, err: bool| SourceState {
        source: q(id),
        cursor: Some("c".into()),
        baseline_at: Some(h(5)),
        checked_at: Some(h(1)),
        last_error: err.then(|| q(&format!("{id}-error"))),
    };
    s.watches = vec![
        (
            watch(
                "w-items",
                lucia_store::WatchStatus::Active,
                lucia_store::watch_mode::ITEMS,
                lucia_store::watch_fetcher::BROWSER,
                "",
            ),
            Some(watch_state("w-items", true)),
        ),
        (
            watch(
                "w-region",
                lucia_store::WatchStatus::Paused,
                lucia_store::watch_mode::REGION,
                lucia_store::watch_fetcher::PLAIN,
                lucia_store::watch_paused::NOISE,
            ),
            Some(watch_state("w-region", false)),
        ),
        (
            watch(
                "w-login",
                lucia_store::WatchStatus::Paused,
                lucia_store::watch_mode::ITEMS,
                lucia_store::watch_fetcher::SESSION,
                lucia_store::watch_paused::LOGIN,
            ),
            None,
        ),
        (
            watch(
                "w-pending",
                lucia_store::WatchStatus::Pending,
                lucia_store::watch_mode::REGION,
                lucia_store::watch_fetcher::PLAIN,
                "",
            ),
            None,
        ),
    ];
    s.watch_now_daily = 17;
    if let Some(b) = s.browser.as_mut() {
        b.sandbox = q("browser-sandbox");
        b.running = true;
        b.sites[0].name = q("site-a-name");
        b.sites[0].origin = format!("https://{}.example.test", q("site-a"));
        b.sites[0].username = Some(q("site-a-user"));
        b.sites[1].name = q("site-b-name");
        b.sites[1].origin = format!("https://{}.example.test", q("site-b"));
        b.sites[1].username = Some(q("site-b-user"));
        b.blocked = vec![q("blocked-a"), q("blocked-b")];
        b.recent_blocks[0].host = q("recent-block-host");
        b.recent_blocks[0].blocked = Some(q("recent-block-why"));
    }
    s.discord.servers = Some(Ok(vec![
        lucia_channel::discord::server::Server {
            id: 111,
            name: q("server-a"),
            admin: true,
            owner: false,
        },
        lucia_channel::discord::server::Server {
            id: 222,
            name: q("server-b"),
            admin: false,
            owner: false,
        },
    ]));
    s.access = crate::admin::snapshot::AccessView {
        admin_url: Some(q("admin-url")),
        gateway_url: Some(q("gateway-url")),
        totp_enabled: true,
        totp_pending: None,
        session_external: false,
        external_blocked: true,
    };
    s.app = crate::admin::snapshot::AppView {
        notify_client: Some(q("client-a")),
        push_preview: true,
        fcm_ready: true,
        fcm_project: Some(q("fcm-project")),
    };
    // 파일 보관함(결정 0052).
    s.kept_files = vec![lucia_store::KeptFile {
        file: lucia_store::FileRecord {
            id: "f_0a1b2c3d4e5f".into(),
            direction: "out".into(),
            conversation: None,
            name: q("kept-name"),
            mime: "image/png".into(),
            kind: "image".into(),
            size: 4096,
            sha256: "x".into(),
            rel_path: "out/x".into(),
            created_at: h(40),
            run_id: None,
        },
        title: q("kept-title"),
        note: q("kept-note"),
        kept_at: h(3),
    }];
    // 이미지 생성(결정 0046): 키가 있고, 만든 것 하나와 실패한 것 하나.
    let generated = |id: &str, status: &str, error: Option<&str>| lucia_store::GeneratedImage {
        id: id.into(),
        run_id: None,
        model: q(&format!("{id}-model")),
        quality: q(&format!("{id}-quality")),
        size: q(&format!("{id}-size")),
        background: "transparent".into(),
        prompt: q(&format!("{id}-prompt")),
        status: status.into(),
        file_id: None,
        error: error.map(q),
        input_tokens: 10,
        output_tokens: 1000,
        cost_usd: 0.03,
        created_at: h(2),
        finished_at: Some(h(2)),
        inputs: 2,
    };
    s.images = crate::admin::snapshot::ImagesView {
        key_set: true,
        daily_cap: 1.5,
        today_usd: 0.03,
        recent: vec![
            generated("img-a", "done", None),
            generated("img-b", "failed", Some("img-b-error")),
        ],
    };
    // 이모지·스티커(결정 0045): 이모지 하나, 스티커 하나, 분류 하나.
    let expression = |id: &str, kind: &str, name: &str, origin: &str| lucia_store::Expression {
        id: id.into(),
        kind: kind.into(),
        name: q(name),
        label: if kind == "emoji" {
            q(&format!("{name}-label"))
        } else {
            String::new()
        },
        description: q(&format!("{name}-description")),
        tags: vec![q(&format!("{name}-tag"))],
        category: "qzcat".into(),
        origin: origin.into(),
        created_at: h(30),
        updated_at: h(30),
        file: lucia_store::ExpressionFile {
            id: format!("x_{id}"),
            mime: "image/png".into(),
            width: 128,
            height: 128,
            size: 2048,
            animated: kind == "sticker",
            created_at: h(30),
            unlisted_at: None,
        },
    };
    s.expressions = Some(std::sync::Arc::new(crate::expressions::Catalog {
        version: "qzversion".into(),
        categories: vec![lucia_store::ExpressionCategory {
            id: "qzcat".into(),
            label: q("expr-category-label"),
            sort: 3,
            icon: Some("e_0a1b2c3d4e5f".into()),
        }],
        emoji: vec![expression("e_0a1b2c3d4e5f", "emoji", "expr-emoji", "admin")],
        stickers: vec![expression(
            "s_0a1b2c3d4e5f",
            "sticker",
            "expr-sticker",
            "agent",
        )],
    }));
    s
}

/// 분기를 바꾼 짝: 멈춘 Discord, 동의 중인 Google, 읽기만 있는 범위, 서버 목록 실패, 세션 없음.
pub(super) fn alt() -> Snapshot {
    let mut s = full();
    s.discord.state = DiscordState::Stopped {
        reason: q("discord-stopped"),
    };
    s.discord.content_refused = true;
    s.discord.guild_calls = false;
    s.discord.prefix = None;
    s.discord.servers = Some(Err(q("servers-error")));
    s.google.consent_minutes_left = Some(6);
    s.google.scopes = Some(crate::google_auth::SCOPE_GMAIL_READ.into());
    s.google.sources[0].state = source("google-calendar", Some(&q("source-error")));
    s.google.sources[1].state = None;
    s.session = None;
    s.compact_at_tokens = None;
    s.backup.enabled = false;
    s.egress_block = false;
    s.quiet_calendar = false;
    s.security_mode = crate::security::Mode::Strict;
    s.scripts_sandbox = None;
    s.gateway_binds = Vec::new();
    s.browser = None;
    s.signals.enabled = false;
    s.checks_at = None;
    // 2단계 인증 설정 중, 푸시 없음(결정 0043).
    s.access.totp_enabled = false;
    s.access.totp_pending = Some((q("totp-secret"), q("totp-uri")));
    s.app.fcm_ready = false;
    s.app.notify_client = None;
    s.images.key_set = false;
    // 그림은 Lucia가 정했다.
    if let Some(avatar) = s.profile.avatar.as_mut() {
        avatar.by = "lucia".into();
        avatar.mime = "image/jpeg".into();
    }
    s
}

pub(super) fn open_security() -> Snapshot {
    let mut s = full();
    s.security_mode = crate::security::Mode::Open;
    s
}

// ---------- 화면에 곁들이는 값 ----------

pub(super) fn flash_sample() -> Flash {
    Flash {
        tone: Tone::Ok,
        text: q("flash-text"),
    }
}

pub(super) fn memory_revision(
    id: i64,
    author: MemoryAuthor,
    run: Option<&str>,
    action: lucia_store::MemoryAction,
    s: &Snapshot,
) -> lucia_store::MemoryRevision {
    lucia_store::MemoryRevision {
        id,
        slug: q("mem-me"),
        at: s.now,
        author,
        run_id: run.map(str::to_owned),
        reason: q(&format!("memrev-{id}-reason")),
        action,
        section: MemorySection::Me,
        title: q(&format!("memrev-{id}-title")),
        summary: q(&format!("memrev-{id}-summary")),
        body: q(&format!("memrev-{id}-body")),
    }
}

pub(super) fn skill_samples() -> Vec<lucia_store::Skill> {
    use lucia_store::SkillStatus as S;
    let skill = |slug: &str, status: S, pinned: bool, script: bool| lucia_store::Skill {
        slug: q(slug),
        title: q(&format!("{slug}-title")),
        description: q(&format!("{slug}-desc")),
        status,
        pinned,
        revision: 2,
        source: if pinned { "chat" } else { "retro" }.into(),
        task_id: pinned.then(|| q(&format!("{slug}-task"))),
        footprint: serde_json::json!({ "tools": ["mail_search"], "hosts": ["a.example.test"] }),
        uses: 5,
        successes: 3,
        last_used_at: None,
        created_at: kst(10, 9, 0),
        updated_at: kst(15, 9, 0),
        script_tools: script.then(|| vec![q(&format!("{slug}-tool"))]),
    };
    vec![
        skill("sk-pending", S::Pending, false, true),
        skill("sk-trial", S::Trial, false, false),
        skill("sk-active", S::Active, true, false),
        skill("sk-stale", S::Stale, false, false),
        skill("sk-archived", S::Archived, false, false),
    ]
}
