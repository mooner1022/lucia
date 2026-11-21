-- 사이트 위임(결정 0028): 사람이 한 번 허용한 사이트는 사람 없는 실행이 다시 묻지 않고 쓴다.
-- 허용은 그 호스트와 하위 도메인이다. 만들기·넓히기는 사람이 카드나 관리 페이지에서만 한다.
CREATE TABLE site_grants (
    id            TEXT PRIMARY KEY,
    host          TEXT NOT NULL,              -- 허용한 호스트(소문자, 하위 도메인 포함)
    login         INTEGER NOT NULL DEFAULT 0, -- 저장된 로그인으로 묻지 않고 다시 로그인
    actions       INTEGER NOT NULL DEFAULT 0, -- 승인이 필요한 브라우저 동작도 묻지 않고(금지 동작은 그대로 못 함)
    purpose       TEXT NOT NULL,              -- 사람이 읽는 한 줄
    origin        TEXT NOT NULL,              -- chat(대화에서 제안·승인) | egress(사람 없는 실행의 카드) | admin | once(이번만)
    created_at    TEXT NOT NULL,
    expires_at    TEXT,                       -- 비면 기한 없음(오래 안 쓰면 만료, 데몬이 본다)
    revoked_at    TEXT,
    uses          INTEGER NOT NULL DEFAULT 0,
    last_used_at  TEXT
);
CREATE INDEX site_grants_by_host ON site_grants (host);

-- 이 시각 전에 만든 후속 조치는 예전처럼 어디든 연다(결정 0029, 배포 전 약속을 깨지 않으려고).
INSERT OR IGNORE INTO settings (key, value, updated_at)
VALUES ('delegation.since', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
