-- 작업(결정 0014, 명세 §작업 모델과 승인): 상태가 명시된 작업 레코드. 상태는 데몬만 바꾼다.
CREATE TABLE tasks (
    id           TEXT PRIMARY KEY,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    title        TEXT NOT NULL,
    brief        TEXT NOT NULL,      -- 작업 세션에 맡긴 내용(Lucia가 정리한 것)
    status       TEXT NOT NULL CHECK (status IN ('created', 'running', 'waiting_approval', 'waiting_input',
                                                 'reported', 'done', 'failed', 'cancelled', 'expired')),
    initiator    TEXT NOT NULL,      -- human | delegated | autonomous
    source       TEXT NOT NULL,      -- chat | followup | routine | precheck | device
    conversation TEXT,               -- 진행과 결과를 보내는 대화(작업 스레드, 없으면 소유자 DM)
    thread       INTEGER NOT NULL DEFAULT 0, -- conversation이 이 작업만의 스레드인가
    session_id   TEXT,               -- 작업 세션(claude)
    run_id       TEXT,               -- 등록한 실행
    result       TEXT,               -- 마지막 보고(요약)
    closed_at    TEXT
);
CREATE INDEX tasks_by_status ON tasks (status, updated_at);
CREATE INDEX tasks_by_conversation ON tasks (conversation);

-- 상태 전이 기록.
CREATE TABLE task_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL REFERENCES tasks (id),
    at          TEXT NOT NULL,
    from_status TEXT,
    to_status   TEXT NOT NULL,
    note        TEXT NOT NULL
);
CREATE INDEX task_events_by_task ON task_events (task_id, id);
