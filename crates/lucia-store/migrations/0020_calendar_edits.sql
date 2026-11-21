-- 캘린더 쓰기(결정 0033): Lucia가 고친 일정의 직전 상태와 뒤 상태. 잘못 고쳤을 때 되돌릴 근거다.
CREATE TABLE calendar_edits (
    id       INTEGER PRIMARY KEY,
    at       TEXT NOT NULL,
    op       TEXT NOT NULL,   -- create | update | delete
    event_id TEXT NOT NULL,
    before   TEXT,            -- JSON(고치기·지우기)
    after    TEXT,            -- JSON(만들기·고치기)
    run_id   TEXT
);
CREATE INDEX calendar_edits_by_event ON calendar_edits (event_id, id);
