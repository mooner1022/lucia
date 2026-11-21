-- 후속 조치(결정 0010, 명세 §진행 상태): Lucia가 약속한 일을 레코드로 남기고, 데몬이 그 시각에 상주 세션을 깨운다.
-- 발사 시각은 같은 ID의 일회성 스케줄(`schedules`, action = 'followup')이 맡는다.
CREATE TABLE followups (
    id           TEXT PRIMARY KEY,
    created_at   TEXT NOT NULL,
    due_at       TEXT NOT NULL,
    task         TEXT NOT NULL,      -- Lucia가 스스로 남긴 메모(사용자 지시가 아니다)
    detail       TEXT NOT NULL,
    conversation TEXT,               -- 등록한 대화(결과를 보낼 곳)
    run_id       TEXT,               -- 등록한 실행
    origin       TEXT NOT NULL,      -- chat | background | followup | user
    status       TEXT NOT NULL CHECK (status IN ('open', 'done', 'cancelled', 'failed')),
    closed_at    TEXT,
    result       TEXT
);
CREATE INDEX followups_by_status ON followups (status, due_at);
