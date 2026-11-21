-- 접속 API의 클라이언트(결정 0021, 명세 7단계 "어댑터 인터페이스를 WebSocket이나 HTTP API로", 결정 0013의 기기 대비).
-- 토큰은 관리 페이지가 발급하고 해시만 남긴다. 역할: app(자체 앱 채널) | device(나중의 외부 기기, 아직 받지 않는다).
CREATE TABLE clients (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    role         TEXT NOT NULL CHECK (role IN ('app', 'device')),
    token_hash   TEXT NOT NULL UNIQUE,
    created_at   TEXT NOT NULL,
    last_seen_at TEXT,
    revoked_at   TEXT
);
