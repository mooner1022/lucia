-- 앱 연동(결정 0043): 클라이언트 범위, 푸시 토큰, 보낸 프레임 상자(재접속 때 다시 받기).
ALTER TABLE clients ADD COLUMN scopes TEXT NOT NULL DEFAULT '[]';   -- JSON 배열(예: ["security_mode"])
ALTER TABLE clients ADD COLUMN push_token TEXT;                     -- FCM 등록 토큰
ALTER TABLE clients ADD COLUMN push_platform TEXT;                  -- android | ios | web
ALTER TABLE clients ADD COLUMN push_at TEXT;
ALTER TABLE clients ADD COLUMN acked_seq INTEGER NOT NULL DEFAULT 0; -- 앱이 받았다고 알린 마지막 번호
ALTER TABLE clients ADD COLUMN out_seq INTEGER NOT NULL DEFAULT 0;   -- 마지막으로 매긴 번호(지워져도 되돌리지 않는다)

CREATE TABLE app_outbox (
    client_id  TEXT NOT NULL REFERENCES clients (id),
    seq        INTEGER NOT NULL,                 -- 클라이언트마다 1부터
    kind       TEXT NOT NULL,                    -- text | note | files | card | card_closed | ...
    frame      TEXT NOT NULL,                    -- 앱에 보낸 프레임(JSON, seq 포함)
    created_at TEXT NOT NULL,
    PRIMARY KEY (client_id, seq)
);
