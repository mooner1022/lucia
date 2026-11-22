-- 데스크톱 앱이 낸 접속 API 확장(결정 0066).

-- 기록의 모양(앱의 history shape 2): 여러 프레임으로 나뉘어 간 조각과 그 말에 붙은 파일.
-- 파일은 보관 기간이 지나 지워져도 이름이 남게 그때의 정보를 함께 둔다.
CREATE TABLE episode_shapes (
    episode_id INTEGER PRIMARY KEY,
    parts      TEXT,                       -- JSON 문자열 배열(둘 이상으로 나뉘어 갔을 때만), 아니면 NULL
    files      TEXT NOT NULL DEFAULT '[]'  -- JSON [{id, name, size, mime, kind}]
);

-- 대화가 받을 수 있는 파일: 파일 자신의 대화 말고도, 그 대화로 보낸 파일(같은 파일을 여러 곳에 보낸다).
CREATE TABLE file_grants (
    file_id      TEXT NOT NULL,
    conversation TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    PRIMARY KEY (file_id, conversation)
);

-- 짧게 두는 파일(앱이 올린 화면 사본 등): 이 시각이 지나면 지운다. 최근 파일 목록·보관함·답 첨부에서 빠진다.
ALTER TABLE files ADD COLUMN expires_at TEXT;
CREATE INDEX files_expires ON files (expires_at) WHERE expires_at IS NOT NULL;

-- 메모(데스크톱 앱의 스티커 메모)를 데몬에도 둔다. 앱은 제 사본을 이것과 맞춘다.
CREATE TABLE notes (
    id         TEXT PRIMARY KEY,           -- n-[a-z0-9]{6,16}, 만드는 쪽이 정한다
    text       TEXT NOT NULL,
    color      TEXT NOT NULL,              -- yellow | pink | blue | green | white
    by         TEXT NOT NULL,              -- user | lucia
    updated_at TEXT NOT NULL,              -- 고칠 때마다 달라지는 표(앱은 뜻을 읽지 않는다)
    rev        INTEGER NOT NULL,           -- 마지막으로 바뀐 판(전체에서 하나씩 는다, settings `notes.rev`)
    removed    INTEGER NOT NULL DEFAULT 0, -- 뗀 것(묘비). 30일 뒤 지운다
    created_at TEXT NOT NULL
);
CREATE INDEX notes_rev ON notes (rev);
