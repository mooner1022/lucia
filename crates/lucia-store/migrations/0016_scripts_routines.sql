-- 스크립트와 반복 루틴(결정 0025, 명세 §도구화 사다리 2단계).

-- 스킬에 붙은 스크립트(Python). 개정마다 코드가 남는다. 스크립트가 쓸 도구(JSON 배열)는 지금 개정의 것만 둔다.
ALTER TABLE skill_revisions ADD COLUMN script TEXT;
ALTER TABLE skills ADD COLUMN script_tools TEXT;          -- NULL이면 스크립트 없음

-- 스크립트가 다음 실행에 넘기는 상태(본 메일 ID 같은 것). 주인: "routine:<id>" 또는 "skill:<이름>".
-- 성공하고 결과를 보낸 뒤에만 바뀐다.
CREATE TABLE script_state (
    owner      TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 반복 루틴: 사람이 대화에서 만든 반복 실행. 스크립트가 있으면 먼저 돌려 알릴 것·깨울 것을 고른다.
CREATE TABLE routines (
    id           TEXT PRIMARY KEY,                 -- routine-…
    name         TEXT NOT NULL,
    trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('cron', 'every', 'change')),
    trigger_spec TEXT NOT NULL,                    -- cron 식 | "분[ HH:MM-HH:MM]" | 소스 이름
    task         TEXT NOT NULL,                    -- Lucia를 깨울 때 할 일
    script       TEXT,                             -- 만들 때 고정한 코드(없으면 매번 Lucia를 깨운다)
    tools        TEXT NOT NULL DEFAULT '[]',       -- 스크립트가 쓸 도구(JSON 배열, "fetch" 포함)
    input        TEXT NOT NULL DEFAULT '{}',       -- lucia.input
    status       TEXT NOT NULL CHECK (status IN ('review', 'active', 'paused', 'done')),
    conversation TEXT,                             -- 결과를 보낼 대화
    until_at     TEXT,                             -- 이때가 지나면 끝
    next_at      TEXT,                             -- 스크립트가 한 번 정한 다음 실행
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    last_run_at  TEXT,
    last_ok      INTEGER,
    last_text    TEXT,
    runs         INTEGER NOT NULL DEFAULT 0,
    wakes        INTEGER NOT NULL DEFAULT 0,
    failures     INTEGER NOT NULL DEFAULT 0        -- 이어진 실패 수
);

-- 루틴 실행 기록(관리 화면, 하루 깨우기 상한).
CREATE TABLE routine_runs (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    routine TEXT NOT NULL,
    at      TEXT NOT NULL,
    ok      INTEGER NOT NULL,
    woke    INTEGER NOT NULL DEFAULT 0,          -- Lucia(모델)를 깨웠다
    text    TEXT NOT NULL
);
CREATE INDEX routine_runs_by_routine ON routine_runs (routine, id);
CREATE INDEX routine_runs_by_at ON routine_runs (at);
