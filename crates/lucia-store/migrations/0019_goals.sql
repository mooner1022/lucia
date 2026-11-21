-- 목표(결정 0032): 후속 조치·반복 루틴·감시를 하나로 묶어 진행을 추적하고, 목표 점검이 다음 할 일을 정한다.
CREATE TABLE goals (
    id                 TEXT PRIMARY KEY,
    title              TEXT NOT NULL,
    purpose            TEXT NOT NULL,
    success            TEXT NOT NULL,            -- 끝났다고 볼 조건
    deadline           TEXT,                     -- 선택(RFC 3339)
    status             TEXT NOT NULL,            -- active | waiting | done | abandoned
    interests          TEXT NOT NULL DEFAULT '', -- 신호 1단계 관심사(줄마다)
    review_every_hours INTEGER NOT NULL DEFAULT 24,
    next_review_at     TEXT,
    conversation       TEXT,                     -- 점검 결과를 보낼 대화
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    closed_at          TEXT
);

-- 진행 기록(점검마다 한 줄, 상태 바뀜, 하위 항목 연결).
CREATE TABLE goal_log (
    id    INTEGER PRIMARY KEY,
    goal  TEXT NOT NULL,
    at    TEXT NOT NULL,
    kind  TEXT NOT NULL,   -- review | note | status | child
    text  TEXT NOT NULL
);
CREATE INDEX goal_log_by_goal ON goal_log (goal, id);

-- 하위 항목. by_review = 목표 점검(사람 없음)이 만든 것: 깨울 때 목표 점검보다 넓은 범위로 돌지 않는다.
CREATE TABLE goal_children (
    goal       TEXT NOT NULL,
    kind       TEXT NOT NULL,  -- followup | routine | watch
    child      TEXT NOT NULL,
    by_review  INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    PRIMARY KEY (kind, child)
);
CREATE INDEX goal_children_by_goal ON goal_children (goal);
