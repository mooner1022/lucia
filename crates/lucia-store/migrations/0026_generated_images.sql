-- 이미지 생성(결정 0046): 요청 하나가 한 줄. 하루 예산을 세고 관리 페이지에 보인다.
CREATE TABLE generated_images (
    id            TEXT PRIMARY KEY,             -- img_…
    run_id        TEXT,                         -- 부른 실행
    model         TEXT NOT NULL,                -- 보낸 모델 ID
    quality       TEXT NOT NULL,
    size          TEXT NOT NULL,
    background    TEXT NOT NULL,                -- opaque | transparent
    prompt        TEXT NOT NULL,                -- 가린 뒤 앞부분만
    status        TEXT NOT NULL,                -- running | done | failed
    file_id       TEXT,                         -- 만든 그림(files 표)
    error         TEXT,
    input_tokens  INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd      REAL NOT NULL DEFAULT 0,      -- 응답의 usage에서 계산한 값
    created_at    TEXT NOT NULL,
    finished_at   TEXT
);
CREATE INDEX generated_images_created ON generated_images (created_at);
