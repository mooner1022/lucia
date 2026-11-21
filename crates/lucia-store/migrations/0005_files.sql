-- 0005: 주고받은 파일(결정 0005). 파일 자체는 <data_dir>/files 아래에 두고 여기에는 목록만.
CREATE TABLE files (
    id            TEXT PRIMARY KEY,          -- f_… (에이전트에게 보이는 ID)
    direction     TEXT NOT NULL,             -- in(사용자가 보냄) | out(에이전트가 만듦)
    conversation  TEXT,
    name          TEXT NOT NULL,             -- 정리한 파일 이름
    mime          TEXT NOT NULL,
    kind          TEXT NOT NULL,             -- image | pdf | text | office | other
    size          INTEGER NOT NULL,
    sha256        TEXT NOT NULL,
    rel_path      TEXT NOT NULL,             -- files 디렉터리 기준 경로
    created_at    TEXT NOT NULL,
    run_id        TEXT
);
CREATE INDEX files_created ON files (created_at);
