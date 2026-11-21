-- 도구화 루프의 측정(결정 0018): 실행마다 턴 수·도구 호출 수, 그리고 실행별 도구 흔적(이름과 핵심 인자).
ALTER TABLE ledger ADD COLUMN num_turns INTEGER NOT NULL DEFAULT 0;
ALTER TABLE ledger ADD COLUMN tool_calls INTEGER NOT NULL DEFAULT 0;

CREATE TABLE run_tools (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id  TEXT NOT NULL,
    seq     INTEGER NOT NULL,
    name    TEXT NOT NULL,   -- 도구 이름(mcp 접두사 뺌)
    arg     TEXT NOT NULL    -- 핵심 인자 한 줄(주소·검색어·이름, 가린 뒤 200자까지)
);
CREATE INDEX run_tools_by_run ON run_tools (run_id, seq);
