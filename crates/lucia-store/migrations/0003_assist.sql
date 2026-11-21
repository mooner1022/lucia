-- 0003: 외부 쓰기 승인(메일 발송)과 활동 기록(결정 0003).

-- 승인 요청. 내용(payload)은 요청할 때 고정하고, 승인되면 데몬이 이 내용 그대로 실행한다.
-- 상태: pending → approved → sent | failed, pending → denied | expired.
CREATE TABLE approvals (
    id            TEXT PRIMARY KEY,          -- 버튼에 싣는 요청 ID
    kind          TEXT NOT NULL,             -- mail_send
    payload       TEXT NOT NULL,             -- 고정된 내용(JSON)
    summary       TEXT NOT NULL,             -- 한 줄 요약(활동 기록·관리 페이지)
    status        TEXT NOT NULL,
    requested_at  TEXT NOT NULL,
    expires_at    TEXT NOT NULL,
    decided_at    TEXT,
    finished_at   TEXT,
    run_id        TEXT,                      -- 요청을 만든 실행
    conversation  TEXT,                      -- 카드를 올린 대화
    card_ref      TEXT,                      -- 버튼이 달린 메시지(플랫폼 메시지 ID)
    result        TEXT                       -- 보낸 메시지 ID나 실패 사유
);
CREATE INDEX approvals_status ON approvals (status);

-- 활동 기록: 에이전트가 한 일(외부 읽기, 승인 요청과 결과, 기억 변경). 사람이 보는 감사용 기록.
CREATE TABLE activity (
    id       INTEGER PRIMARY KEY,
    at       TEXT NOT NULL,
    kind     TEXT NOT NULL,
    summary  TEXT NOT NULL,
    run_id   TEXT
);
CREATE INDEX activity_at ON activity (at);
