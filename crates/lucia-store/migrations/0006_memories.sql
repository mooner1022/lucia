-- 기억 항목(결정 0007): 한 덩어리 핵심 기억을 나·주제·영역의 항목으로 나눈다.
-- 평소 맥락에는 색인(이름·제목·요약)만 붙고, 세부 내용은 도구(memory_read)로 읽는다.
CREATE TABLE memories (
    slug       TEXT PRIMARY KEY,                                      -- kebab-case 이름([[이름]]으로 가리킨다)
    section    TEXT NOT NULL CHECK (section IN ('me', 'topic', 'area')),
    title      TEXT NOT NULL,
    summary    TEXT NOT NULL,
    body       TEXT NOT NULL,
    revision   INTEGER NOT NULL,                                      -- 마지막 개정 ID(읽지 않은 내용 덮어쓰기 방지)
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 모든 변경(쓰기·지우기). 지우기는 지우기 전 내용을 남긴다.
CREATE TABLE memory_revisions (
    id      INTEGER PRIMARY KEY,
    slug    TEXT NOT NULL,
    at      TEXT NOT NULL,
    author  TEXT NOT NULL,                                            -- agent | user | daemon
    run_id  TEXT,
    reason  TEXT NOT NULL,
    action  TEXT NOT NULL CHECK (action IN ('write', 'delete')),
    section TEXT NOT NULL,
    title   TEXT NOT NULL,
    summary TEXT NOT NULL,
    body    TEXT NOT NULL
);
CREATE INDEX memory_revisions_by_slug ON memory_revisions (slug, id);

-- 예전 핵심 기억(마지막 개정)을 항목 하나로 그대로 옮긴다. 원래 개정 기록(core_memory_revisions)은 남겨 둔다.
INSERT INTO memory_revisions (slug, at, author, run_id, reason, action, section, title, summary, body)
SELECT 'core-notes', at, 'daemon', NULL, '핵심 기억을 항목 기억으로 옮김(마이그레이션 0006)', 'write', 'me',
       '예전 핵심 기억', '항목으로 나누기 전의 핵심 기억 전체. 주제별 항목으로 옮겨 정리한 뒤 지운다.', body
FROM core_memory_revisions
ORDER BY id DESC
LIMIT 1;
INSERT INTO memories (slug, section, title, summary, body, revision, created_at, updated_at)
SELECT slug, section, title, summary, body, id, at, at FROM memory_revisions;
