-- 스킬(결정 0018, 명세 §성공한 작업의 도구화): 절차 메모. 본문은 개정으로 남기고, 파일(SKILL.md)은 데몬이 git으로도 남긴다.
CREATE TABLE skills (
    slug         TEXT PRIMARY KEY,
    title        TEXT NOT NULL,
    description  TEXT NOT NULL,             -- 언제 쓰는지 한 줄(맥락 색인에 보인다)
    status       TEXT NOT NULL CHECK (status IN ('pending', 'trial', 'active', 'stale', 'archived')),
    pinned       INTEGER NOT NULL DEFAULT 0, -- 사람이 고정: 자동 수정·보관 대상에서 빠진다
    revision     INTEGER NOT NULL,          -- 지금 개정 번호
    source       TEXT NOT NULL,             -- chat | retro
    task_id      TEXT,                      -- 만든 작업(회고)
    footprint    TEXT NOT NULL,             -- JSON: 원래 작업이 실제로 쓴 도구와 연 호스트
    uses         INTEGER NOT NULL DEFAULT 0,
    successes    INTEGER NOT NULL DEFAULT 0,
    last_used_at TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE TABLE skill_revisions (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    slug     TEXT NOT NULL,
    revision INTEGER NOT NULL,
    at       TEXT NOT NULL,
    author   TEXT NOT NULL,                 -- agent | user
    reason   TEXT NOT NULL,
    body     TEXT NOT NULL,
    UNIQUE (slug, revision)
);

-- 스킬을 읽은 실행(쓴 기록). 그 작업을 사람이 완료로 확인하면 성공으로 센다.
CREATE TABLE skill_uses (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    slug    TEXT NOT NULL,
    run_id  TEXT NOT NULL,
    task_id TEXT,
    at      TEXT NOT NULL,
    counted INTEGER NOT NULL DEFAULT 0      -- 성공으로 셌다
);
CREATE INDEX skill_uses_by_task ON skill_uses (task_id);

-- 회고를 마친 작업(한 번만 한다).
ALTER TABLE tasks ADD COLUMN retro_at TEXT;
