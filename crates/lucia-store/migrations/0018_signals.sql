-- 신호 처리(결정 0031): 사전 체크가 찾은 변화와 외부 이벤트를 세 단계(코드 거르기 → 작은 모델 분류 → Lucia)로
-- 나눈 기록. 같은 변화를 두 번 나누지 않게 하고, 브리핑이 이미 알린 것을 되풀이하지 않게 한다.
CREATE TABLE signals (
    id          INTEGER PRIMARY KEY,
    source      TEXT NOT NULL,   -- gmail | google-calendar | event:<보낸 곳>
    key         TEXT NOT NULL,   -- 변화 키(사전 체크는 source_changes.change_key와 같다)
    summary     TEXT NOT NULL,   -- 사람이 읽을 한 줄
    payload     TEXT NOT NULL,   -- JSON(보낸 사람, 제목, 미리보기, 라벨 등)
    interest    TEXT,            -- 1단계에서 맞은 관심사
    verdict     TEXT,            -- NULL(분류 대기) | drop | digest | now | briefing
    reason      TEXT,            -- 판정 근거 한 줄
    received_at TEXT NOT NULL,
    decided_at  TEXT,
    woke_at     TEXT             -- now: Lucia에게 넘긴 시각
);
CREATE UNIQUE INDEX signals_by_key ON signals (source, key);
CREATE INDEX signals_by_verdict ON signals (verdict, woke_at);
