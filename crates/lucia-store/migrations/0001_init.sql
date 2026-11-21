-- 0001: MVP(2단계) 스키마. 시각은 모두 UTC RFC3339 문자열.
-- 3단계 범위(작업 상태 머신, 후속 조치, 승인)는 넣지 않는다.

-- Claude Code 세션. 상주 세션은 role='resident'이고 status='active'인 것이 하나다.
CREATE TABLE sessions (
    id                  TEXT PRIMARY KEY,           -- claude 세션 UUID
    role                TEXT NOT NULL,              -- resident | task | routine
    sandbox             TEXT,
    status              TEXT NOT NULL,              -- active | rotated | closed
    created_at          TEXT NOT NULL,
    last_used_at        TEXT,
    last_context_tokens INTEGER NOT NULL DEFAULT 0,
    rotated_at          TEXT
);
CREATE INDEX sessions_role_status ON sessions (role, status);

-- 에피소드 기억: 지난 대화, 보고, 보낸 브리핑. 실행이 끝날 때 자동으로 쌓인다.
CREATE TABLE episodes (
    id           INTEGER PRIMARY KEY,
    at           TEXT NOT NULL,
    kind         TEXT NOT NULL,      -- user_message | agent_report | briefing | rotation_summary
    origin       TEXT NOT NULL,      -- user | agent | daemon
    conversation TEXT,               -- 플랫폼 중립 대화 ID
    session_id   TEXT,
    run_id       TEXT,
    body         TEXT NOT NULL
);
CREATE INDEX episodes_at ON episodes (at);
CREATE INDEX episodes_origin_at ON episodes (origin, at);

-- 한국어 검색: trigram. 세 글자 이상은 MATCH, 두 글자 이하는 LIKE(tests/fts_korean.rs).
CREATE VIRTUAL TABLE episodes_fts USING fts5 (
    body, content = 'episodes', content_rowid = 'id', tokenize = 'trigram'
);
CREATE TRIGGER episodes_ai AFTER INSERT ON episodes BEGIN
    INSERT INTO episodes_fts (rowid, body) VALUES (new.id, new.body);
END;
CREATE TRIGGER episodes_ad AFTER DELETE ON episodes BEGIN
    INSERT INTO episodes_fts (episodes_fts, rowid, body) VALUES ('delete', old.id, old.body);
END;
CREATE TRIGGER episodes_au AFTER UPDATE ON episodes BEGIN
    INSERT INTO episodes_fts (episodes_fts, rowid, body) VALUES ('delete', old.id, old.body);
    INSERT INTO episodes_fts (rowid, body) VALUES (new.id, new.body);
END;

-- 핵심 기억: 마크다운 한 덩어리. 현재 값은 마지막 개정. 모든 변경을 남겨 되돌릴 수 있게 한다.
CREATE TABLE core_memory_revisions (
    id      INTEGER PRIMARY KEY,
    at      TEXT NOT NULL,
    author  TEXT NOT NULL,           -- agent | user | daemon
    run_id  TEXT,
    reason  TEXT NOT NULL,
    body    TEXT NOT NULL
);

-- 비용 원장: 실행마다 한 줄. 모델 없이 끝난 실행도 남긴다(called_model = 0).
CREATE TABLE ledger (
    id                    INTEGER PRIMARY KEY,
    run_id                TEXT NOT NULL UNIQUE,
    origin                TEXT NOT NULL,     -- routine | chat | event
    purpose               TEXT NOT NULL,     -- morning_briefing | chat | ...
    started_at            TEXT NOT NULL,
    finished_at           TEXT NOT NULL,
    called_model          INTEGER NOT NULL,
    model                 TEXT,
    session_id            TEXT,
    input_tokens          INTEGER NOT NULL DEFAULT 0,
    output_tokens         INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens     INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    context_tokens        INTEGER NOT NULL DEFAULT 0,
    cost_usd              REAL NOT NULL DEFAULT 0,
    duration_ms           INTEGER NOT NULL DEFAULT 0,
    outcome               TEXT NOT NULL,     -- ok | no_change | error | ...
    detail                TEXT
);
CREATE INDEX ledger_started_at ON ledger (started_at);

-- 스케줄: cron 루틴과 일회성 리마인더. 재시작 뒤에도 유지된다.
CREATE TABLE schedules (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,           -- cron | once
    spec            TEXT NOT NULL,           -- cron 식 또는 RFC3339 시각
    timezone        TEXT NOT NULL,
    action          TEXT NOT NULL,           -- morning_briefing | ...
    payload         TEXT NOT NULL DEFAULT '{}',
    max_delay_secs  INTEGER NOT NULL,        -- 이보다 늦으면 건너뛰고 기록
    next_due_at     TEXT,                    -- NULL이면 더 실행하지 않는다
    last_fired_at   TEXT,
    enabled         INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE schedule_runs (
    id           INTEGER PRIMARY KEY,
    schedule_id  TEXT NOT NULL REFERENCES schedules (id),
    due_at       TEXT NOT NULL,
    decided_at   TEXT NOT NULL,
    outcome      TEXT NOT NULL,              -- fired | skipped_late
    UNIQUE (schedule_id, due_at)
);

-- 사전 체크: 소스별 기준점과 감지한 변화.
CREATE TABLE source_state (
    source         TEXT PRIMARY KEY,
    cursor         TEXT,                     -- syncToken, historyId, 해시 등
    baseline_at    TEXT,                     -- 첫 동기화(기준선)를 잡은 시각
    checked_at     TEXT,
    last_error     TEXT,
    last_error_at  TEXT
);
CREATE TABLE source_changes (
    id           INTEGER PRIMARY KEY,
    source       TEXT NOT NULL,
    detected_at  TEXT NOT NULL,
    change_key   TEXT NOT NULL,              -- 같은 변화를 두 번 넣지 않기 위한 키(버전 포함)
    summary      TEXT NOT NULL,
    payload      TEXT NOT NULL DEFAULT '{}',
    consumed_at  TEXT,                       -- 브리핑에 넣은 시각
    UNIQUE (source, change_key)
);
CREATE INDEX source_changes_pending ON source_changes (consumed_at);
