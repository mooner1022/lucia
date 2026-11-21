-- 알림 묶기(결정 0015, 명세 §알림 묶기): 급하지 않은 데몬 소식을 모아 두었다가 한 번에 보낸다.
CREATE TABLE notices (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL,
    kind       TEXT NOT NULL,       -- 예: periodic_check
    key        TEXT,                -- 같은 소식을 다시 넣지 않기 위한 키(예: task-…:waiting)
    body       TEXT NOT NULL,
    sent_at    TEXT
);
CREATE INDEX notices_pending ON notices (sent_at, created_at);
CREATE INDEX notices_by_key ON notices (key, created_at);
