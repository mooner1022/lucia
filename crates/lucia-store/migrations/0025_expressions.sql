-- 앱 이모지·스티커(결정 0045): 목록은 여기, 그림 파일은 <data_dir>/expressions/<파일 ID>에 둔다.
CREATE TABLE expression_files (
    id          TEXT PRIMARY KEY,               -- x_ + 내용 해시(SHA-256) 앞 24자. 내용이 같으면 같은 ID다.
    mime        TEXT NOT NULL,
    width       INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    size        INTEGER NOT NULL,
    animated    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    unlisted_at TEXT                            -- 가리키는 항목이 없어진 때(30일 뒤 지운다). 다시 쓰이면 NULL.
);

CREATE TABLE expressions (
    id          TEXT PRIMARY KEY,               -- e_… (이모지) | s_… (스티커)
    kind        TEXT NOT NULL,                  -- emoji | sticker
    name        TEXT NOT NULL,
    name_key    TEXT NOT NULL,                  -- 겹침 확인용(앞뒤 공백 없이 소문자)
    label       TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    tags        TEXT NOT NULL DEFAULT '[]',     -- JSON 배열
    category    TEXT NOT NULL DEFAULT '',       -- 분류 ID. ''면 분류 없음
    file_id     TEXT NOT NULL REFERENCES expression_files (id),
    origin      TEXT NOT NULL DEFAULT 'admin',  -- admin(관리 페이지) | agent(Lucia가 대화에서)
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    UNIQUE (kind, name_key)
);

CREATE TABLE expression_categories (
    id    TEXT PRIMARY KEY,
    label TEXT NOT NULL,
    sort  INTEGER NOT NULL DEFAULT 0,
    icon  TEXT                                  -- 탭 아이콘으로 쓸 항목 ID(없을 수 있다)
);
