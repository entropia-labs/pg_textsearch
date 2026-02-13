\set ON_ERROR_STOP on
\timing on

\echo '=== Multi-Tenant Load Test: Insert Benchmarks ==='
\echo '=================================================='

SET enable_seqscan = off;

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

-- ============================================================
-- Benchmark A: Single-row insert (100 iterations)
-- ============================================================
\echo ''
\echo '--- Benchmark A: Single-row insert latency (100 iters) ---'

DO $$
DECLARE
    times numeric[];
    start_ts timestamptz;
    end_ts timestamptz;
    elapsed numeric;
    p50 numeric;
    p95 numeric;
    p99 numeric;
    min_t numeric;
    max_t numeric;
BEGIN
    times := '{}';

    FOR i IN 1..100 LOOP
        start_ts := clock_timestamp();

        INSERT INTO tenant_docs (doc_id, chunk_id, content, tenant_id)
        VALUES (
            1000000 + i,
            1,
            'benchmark insert test single row iteration ' ||
            i || ' database algorithm network protocol server',
            1
        );

        end_ts := clock_timestamp();
        elapsed := extract(epoch from (end_ts - start_ts)) * 1000;
        times := array_append(times, elapsed);
    END LOOP;

    p50 := percentile_from_array(times, 0.50);
    p95 := percentile_from_array(times, 0.95);
    p99 := percentile_from_array(times, 0.99);
    SELECT min(v), max(v) INTO min_t, max_t FROM unnest(times) v;

    RAISE NOTICE 'SINGLE_INSERT: p50=% p95=% p99=% min=% max=%',
        round(p50, 3), round(p95, 3), round(p99, 3),
        round(min_t, 3), round(max_t, 3);
END $$;

-- ============================================================
-- Benchmark B: Batch insert — 200 rows per iteration (50 iters)
-- ============================================================
\echo ''
\echo '--- Benchmark B: Batch insert latency (200 rows x 50 iters) ---'

DO $$
DECLARE
    times numeric[];
    start_ts timestamptz;
    end_ts timestamptz;
    elapsed numeric;
    p50 numeric;
    p95 numeric;
    p99 numeric;
    min_t numeric;
    max_t numeric;
    tenant int;
BEGIN
    times := '{}';

    FOR i IN 1..50 LOOP
        tenant := 1 + (i % 5);
        start_ts := clock_timestamp();

        INSERT INTO tenant_docs (doc_id, chunk_id, content, tenant_id)
        SELECT
            2000000 + i,
            c,
            'benchmark batch insert tenant ' || tenant ||
            ' chunk ' || c ||
            ' database algorithm network protocol server' ||
            ' encryption latency throughput architecture',
            tenant
        FROM generate_series(1, 200) AS c;

        end_ts := clock_timestamp();
        elapsed := extract(epoch from (end_ts - start_ts)) * 1000;
        times := array_append(times, elapsed);
    END LOOP;

    p50 := percentile_from_array(times, 0.50);
    p95 := percentile_from_array(times, 0.95);
    p99 := percentile_from_array(times, 0.99);
    SELECT min(v), max(v) INTO min_t, max_t FROM unnest(times) v;

    RAISE NOTICE 'BATCH_INSERT: p50=% p95=% p99=% min=% max=%',
        round(p50, 3), round(p95, 3), round(p99, 3),
        round(min_t, 3), round(max_t, 3);
END $$;

\echo ''
\echo '=== Insert benchmarks complete ==='
