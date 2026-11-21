-- 감시(결정 0019, 명세 §도구화 사다리 3단계 "사전 체크 모듈"): 사람이 승인한 공개 페이지 구간을 데몬이 모델 없이
-- 주기적으로 확인한다. 변화는 사전 체크의 "바뀐 것"(source_changes, 소스 이름 watch:<id>)으로 들어간다.
CREATE TABLE watches (
    id               TEXT PRIMARY KEY,
    url              TEXT NOT NULL,
    what             TEXT NOT NULL,           -- 무엇을 보는지(사람이 읽는 이름)
    start_text       TEXT NOT NULL DEFAULT '', -- 이 글 뒤부터(비면 처음부터)
    end_text         TEXT NOT NULL DEFAULT '', -- 이 글 앞까지(비면 끝까지)
    interval_minutes INTEGER NOT NULL,
    status           TEXT NOT NULL CHECK (status IN ('pending', 'active', 'paused', 'removed')),
    changes_in_row   INTEGER NOT NULL DEFAULT 0,
    run_id           TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);
