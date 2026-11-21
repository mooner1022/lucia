-- 알림이 반응에서 배우기(결정 0034).
-- 알림 규칙: 사용자가 알림 카드에서 누른 반응. 키는 보낸 사람 주소(또는 일정·기기 이벤트 출처).
CREATE TABLE notice_rules (
    key        TEXT PRIMARY KEY,
    rule       TEXT NOT NULL,  -- useful(유용해요) | briefing(브리핑에만) | mute(필요 없어요)
    label      TEXT NOT NULL,  -- 사람이 읽을 이름
    created_at TEXT NOT NULL
);

-- 단계적 독촉: 마감이 있는 일을 추적하고, 처리되지 않은 채 다가오면 단계를 올린다.
CREATE TABLE nudges (
    id         TEXT PRIMARY KEY,
    title      TEXT NOT NULL,
    due_at     TEXT NOT NULL,
    note       TEXT NOT NULL DEFAULT '',
    goal       TEXT,
    stage      INTEGER NOT NULL DEFAULT 0, -- 0 등록 | 1 소식 | 2 단독 알림 | 3 카드
    status     TEXT NOT NULL,              -- open | done | expired | dropped
    run_id     TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX nudges_open ON nudges (status, due_at);
