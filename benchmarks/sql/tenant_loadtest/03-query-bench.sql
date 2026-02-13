\set ON_ERROR_STOP on
\timing on

\echo '=== Multi-Tenant Load Test: Query Benchmarks ==='
\echo '================================================='

SET enable_seqscan = off;
SET pg_textsearch.log_bmw_stats = on;

-- Helper: compute percentiles from a numeric array
CREATE OR REPLACE FUNCTION percentile_from_array(
    arr numeric[], pct numeric
) RETURNS numeric AS $$
DECLARE
    n int := array_length(arr, 1);
    sorted numeric[];
    pos numeric;
    lo int;
    hi int;
BEGIN
    SELECT array_agg(v ORDER BY v) INTO sorted FROM unnest(arr) v;
    pos := 1 + (n - 1) * pct;
    lo := floor(pos)::int;
    hi := ceil(pos)::int;
    IF lo = hi THEN
        RETURN sorted[lo];
    END IF;
    RETURN sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo);
END;
$$ LANGUAGE plpgsql;

-- Reusable benchmark function
CREATE OR REPLACE FUNCTION bench_query(
    term text,
    p_tenant_id int,
    p_limit int DEFAULT 10,
    warmup int DEFAULT 3,
    iterations int DEFAULT 20
) RETURNS TABLE(
    p50_ms numeric, p95_ms numeric, p99_ms numeric,
    avg_ms numeric, min_ms numeric, max_ms numeric,
    result_count bigint
) AS $$
DECLARE
    times numeric[];
    start_ts timestamptz;
    end_ts timestamptz;
    elapsed numeric;
    cnt bigint := 0;
    q text;
BEGIN
    -- Build the query (no count wrapper — use GET DIAGNOSTICS)
    IF p_tenant_id IS NOT NULL THEN
        q := format(
            'SELECT id FROM tenant_docs '
            'WHERE content <@> to_bm25query(%L, '
            '''tenant_docs_bm25_idx'') < 0 '
            'AND tenant_id = %s '
            'ORDER BY content <@> to_bm25query(%L, '
            '''tenant_docs_bm25_idx'') '
            'LIMIT %s',
            term, p_tenant_id, term, p_limit
        );
    ELSE
        q := format(
            'SELECT id FROM tenant_docs '
            'WHERE content <@> to_bm25query(%L, '
            '''tenant_docs_bm25_idx'') < 0 '
            'ORDER BY content <@> to_bm25query(%L, '
            '''tenant_docs_bm25_idx'') '
            'LIMIT %s',
            term, term, p_limit
        );
    END IF;

    -- Warmup rounds
    FOR i IN 1..warmup LOOP
        EXECUTE q;
        GET DIAGNOSTICS cnt = ROW_COUNT;
    END LOOP;

    -- Timed iterations
    times := '{}';
    FOR i IN 1..iterations LOOP
        start_ts := clock_timestamp();
        EXECUTE q;
        GET DIAGNOSTICS cnt = ROW_COUNT;
        end_ts := clock_timestamp();
        elapsed := extract(epoch from (end_ts - start_ts)) * 1000;
        times := array_append(times, elapsed);
    END LOOP;

    p50_ms := percentile_from_array(times, 0.50);
    p95_ms := percentile_from_array(times, 0.95);
    p99_ms := percentile_from_array(times, 0.99);
    SELECT avg(v), min(v), max(v)
        INTO avg_ms, min_ms, max_ms
    FROM unnest(times) v;
    result_count := cnt;

    RETURN NEXT;
END;
$$ LANGUAGE plpgsql;

-- ============================================================
-- Benchmark 1: Single-term, tenant-scoped
-- ============================================================
\echo ''
\echo '--- Benchmark 1: Single-term tenant-scoped queries ---'

SELECT 'tenant=1' AS test,
       round(p50_ms, 3) AS p50,
       round(p95_ms, 3) AS p95,
       round(p99_ms, 3) AS p99,
       round(avg_ms, 3) AS avg,
       result_count AS rows
FROM bench_query('database', 1);

SELECT 'tenant=3' AS test,
       round(p50_ms, 3) AS p50,
       round(p95_ms, 3) AS p95,
       round(p99_ms, 3) AS p99,
       round(avg_ms, 3) AS avg,
       result_count AS rows
FROM bench_query('database', 3);

SELECT 'tenant=5' AS test,
       round(p50_ms, 3) AS p50,
       round(p95_ms, 3) AS p95,
       round(p99_ms, 3) AS p99,
       round(avg_ms, 3) AS avg,
       result_count AS rows
FROM bench_query('database', 5);

-- ============================================================
-- Benchmark 2: Multi-term, tenant-scoped
-- ============================================================
\echo ''
\echo '--- Benchmark 2: Multi-term tenant-scoped queries ---'

SELECT 'tenant=1' AS test,
       round(p50_ms, 3) AS p50,
       round(p95_ms, 3) AS p95,
       round(p99_ms, 3) AS p99,
       round(avg_ms, 3) AS avg,
       result_count AS rows
FROM bench_query('algorithm database optimization', 1);

SELECT 'tenant=3' AS test,
       round(p50_ms, 3) AS p50,
       round(p95_ms, 3) AS p95,
       round(p99_ms, 3) AS p99,
       round(avg_ms, 3) AS avg,
       result_count AS rows
FROM bench_query('algorithm database optimization', 3);

-- ============================================================
-- Benchmark 3: Non-existent term
-- ============================================================
\echo ''
\echo '--- Benchmark 3: Non-existent term ---'

SELECT 'nonexistent' AS test,
       round(p50_ms, 3) AS p50,
       round(p95_ms, 3) AS p95,
       round(avg_ms, 3) AS avg,
       result_count AS rows
FROM bench_query('xyznonexistentterm', 1);

-- ============================================================
-- Benchmark 4: Non-existent tenant
-- ============================================================
\echo ''
\echo '--- Benchmark 4: Non-existent tenant ---'

SELECT 'tenant=999' AS test,
       round(p50_ms, 3) AS p50,
       round(p95_ms, 3) AS p95,
       round(avg_ms, 3) AS avg,
       result_count AS rows
FROM bench_query('database', 999);

-- ============================================================
-- Benchmark 5: Global query (no tenant filter)
-- ============================================================
\echo ''
\echo '--- Benchmark 5: Global query (no tenant filter) ---'

SELECT 'global' AS test,
       round(p50_ms, 3) AS p50,
       round(p95_ms, 3) AS p95,
       round(p99_ms, 3) AS p99,
       round(avg_ms, 3) AS avg,
       result_count AS rows
FROM bench_query('database', NULL);

-- ============================================================
-- Benchmark 6: Varying LIMIT
-- ============================================================
\echo ''
\echo '--- Benchmark 6: Varying LIMIT (tenant=1) ---'

SELECT 'LIMIT 1' AS test,
       round(p50_ms, 3) AS p50,
       round(p95_ms, 3) AS p95,
       round(avg_ms, 3) AS avg,
       result_count AS rows
FROM bench_query('database', 1, 1);

SELECT 'LIMIT 10' AS test,
       round(p50_ms, 3) AS p50,
       round(p95_ms, 3) AS p95,
       round(avg_ms, 3) AS avg,
       result_count AS rows
FROM bench_query('database', 1, 10);

SELECT 'LIMIT 100' AS test,
       round(p50_ms, 3) AS p50,
       round(p95_ms, 3) AS p95,
       round(avg_ms, 3) AS avg,
       result_count AS rows
FROM bench_query('database', 1, 100);

SELECT 'LIMIT 1000' AS test,
       round(p50_ms, 3) AS p50,
       round(p95_ms, 3) AS p95,
       round(avg_ms, 3) AS avg,
       result_count AS rows
FROM bench_query('database', 1, 1000);

\echo ''
\echo '=== Query benchmarks complete ==='
