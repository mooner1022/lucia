-- 개발 위임(결정 0020): 작업 종류와 레포, 임시 샌드박스.
ALTER TABLE tasks ADD COLUMN kind TEXT NOT NULL DEFAULT 'general';   -- general | dev
ALTER TABLE tasks ADD COLUMN repo TEXT;                               -- 개발 작업의 레포(https 주소 또는 허용한 로컬 경로)
ALTER TABLE tasks ADD COLUMN sandbox TEXT;                            -- 임시 샌드박스 이름(정리하면 비운다)
ALTER TABLE tasks ADD COLUMN base TEXT;                               -- 시작 커밋
