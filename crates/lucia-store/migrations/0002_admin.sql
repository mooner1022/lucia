-- 0002: 관리 페이지(D8)와 운영 상태.

-- 사람이 관리 페이지에서 정하는 값(하드코딩하지 않는다). 예: owner.discord_user_id, notify.conversation
CREATE TABLE settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

-- 알림 기록. 메신저가 아직 없거나 전송에 실패해도 상태 화면에서 볼 수 있게 남긴다(명세 원칙 8).
CREATE TABLE alerts (
    id         INTEGER PRIMARY KEY,
    at         TEXT NOT NULL,
    text       TEXT NOT NULL,             -- 마스킹을 거친 글
    delivered  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX alerts_at ON alerts (at);

-- 관리 페이지 비밀번호(해시만). 한 줄만 둔다.
CREATE TABLE admin_auth (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    password_hash  TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

-- 초기 설정 링크(비밀번호가 없을 때만, 만료 있음). 토큰은 해시만 둔다.
CREATE TABLE admin_setup (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    token_hash  TEXT NOT NULL,
    expires_at  TEXT NOT NULL
);
