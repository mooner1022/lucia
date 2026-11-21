-- 게시판 감시와 빠른 경로(결정 0036): 구간 감시(region)에 더해 글 목록의 새 항목을 보는 감시(items).
-- 상태 CHECK는 그대로 두고, 멈춘 까닭은 paused_reason에 둔다(noise | login | error).
ALTER TABLE watches ADD COLUMN mode          TEXT NOT NULL DEFAULT 'region'; -- region | items
ALTER TABLE watches ADD COLUMN item_pattern  TEXT NOT NULL DEFAULT '';       -- 글 링크 패턴(items)
ALTER TABLE watches ADD COLUMN include_words TEXT NOT NULL DEFAULT '';       -- 제목에 들어가야 할 말(줄마다)
ALTER TABLE watches ADD COLUMN exclude_words TEXT NOT NULL DEFAULT '';       -- 빼야 할 말(줄마다)
ALTER TABLE watches ADD COLUMN notify        TEXT NOT NULL DEFAULT 'digest'; -- now | classify | digest
ALTER TABLE watches ADD COLUMN fetcher       TEXT NOT NULL DEFAULT 'plain';  -- plain | session | browser
ALTER TABLE watches ADD COLUMN failures      INTEGER NOT NULL DEFAULT 0;     -- 이어진 실패(물러서기)
ALTER TABLE watches ADD COLUMN paused_reason TEXT NOT NULL DEFAULT '';
ALTER TABLE watches ADD COLUMN sent_day      TEXT NOT NULL DEFAULT '';       -- 바로 알림 하루 상한(YYYY-MM-DD)
ALTER TABLE watches ADD COLUMN sent_count    INTEGER NOT NULL DEFAULT 0;
