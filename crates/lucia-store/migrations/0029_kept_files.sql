-- 파일 보관함(결정 0052): 보관한 파일은 30일 정리에서 빠진다.
CREATE TABLE kept_files (
    file_id  TEXT PRIMARY KEY REFERENCES files (id),
    title    TEXT NOT NULL DEFAULT '',   -- 찾기 쉬운 이름(없으면 파일 이름을 쓴다)
    note     TEXT NOT NULL DEFAULT '',   -- 무엇인지 한 줄
    kept_at  TEXT NOT NULL
);
