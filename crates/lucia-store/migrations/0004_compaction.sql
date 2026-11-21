-- 0004: 상주 세션 compact(결정 0004)와 비용 원장 고침.

-- 세션: claude가 주는 누적 비용의 마지막 값(실행 비용 = 차이), 마지막 compact, 그 뒤 실행 수.
ALTER TABLE sessions ADD COLUMN cost_total_usd REAL NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN last_compacted_at TEXT;
ALTER TABLE sessions ADD COLUMN runs_since_compact INTEGER NOT NULL DEFAULT 0;

-- 지금까지 원장의 cost_usd는 세션 누적값이었다(result.total_cost_usd). 세션별 차이로 고친다.
CREATE TEMP TABLE ledger_cost_fix AS
    SELECT id,
           cost_usd - COALESCE(LAG(cost_usd) OVER (PARTITION BY session_id ORDER BY id), 0) AS delta
    FROM ledger
    WHERE called_model = 1 AND session_id IS NOT NULL;
UPDATE ledger
    SET cost_usd = MAX(0, (SELECT delta FROM ledger_cost_fix f WHERE f.id = ledger.id))
    WHERE id IN (SELECT id FROM ledger_cost_fix);
UPDATE sessions
    SET cost_total_usd = COALESCE(
        (SELECT SUM(cost_usd) FROM ledger WHERE ledger.session_id = sessions.id AND called_model = 1), 0);
UPDATE sessions
    SET runs_since_compact = (SELECT COUNT(*) FROM ledger WHERE ledger.session_id = sessions.id AND called_model = 1);
DROP TABLE ledger_cost_fix;
