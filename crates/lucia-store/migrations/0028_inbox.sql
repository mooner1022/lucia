-- 받은 편지함(결정 0050): 받은 메시지를 처리가 끝날 때까지 남긴다. 데몬이 다시 뜨면 기다리던 것은 이어서
-- 처리하고, 처리하다 끊긴 것은 사용자에게 알린다.
CREATE TABLE inbox (
    key         TEXT PRIMARY KEY,               -- 대화|보낸 사람|받은 시각
    payload     TEXT NOT NULL,                  -- 메시지(JSON)
    state       TEXT NOT NULL,                  -- queued | running
    enqueued_at TEXT NOT NULL,
    started_at  TEXT
);
