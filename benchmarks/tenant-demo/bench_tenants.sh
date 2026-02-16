#!/bin/bash
#
# Compare BM25 query latency: tenant index vs global index,
# at 5 and 50 tenants.
#
# For each tenant count the script:
#   1. Runs setup.sh (creates table + tenant-aware index)
#   2. Benchmarks queries on the tenant index
#   3. Drops the index, rebuilds as a global index (no tenant_column)
#   4. Benchmarks the same queries on the global index
#
# Usage:
#   cd benchmarks/tenant-demo
#   PATH="/opt/homebrew/opt/postgresql@17/bin:$PATH" bash bench_tenants.sh
#
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATABASE_URL="${DATABASE_URL:-postgres:///postgres}"

# Temp files for capturing results (4 configurations)
R_5T=$(mktemp)   # 5 tenants, tenant index
R_5G=$(mktemp)   # 5 tenants, global index
R_50T=$(mktemp)  # 50 tenants, tenant index
R_50G=$(mktemp)  # 50 tenants, global index
trap 'rm -f "$R_5T" "$R_5G" "$R_50T" "$R_50G"' EXIT

# SQL that creates bench helpers and runs queries.
# Emits CSV key,value rows to stdout.
read -r -d '' BENCH_SQL << 'EOSQL' || true
\set ON_ERROR_STOP on
SET enable_seqscan = off;

-- percentile helper
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
    IF lo = hi THEN RETURN sorted[lo]; END IF;
    RETURN sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo);
END;
$$ LANGUAGE plpgsql;

-- bench_query helper
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

    FOR i IN 1..warmup LOOP
        EXECUTE q;
        GET DIAGNOSTICS cnt = ROW_COUNT;
    END LOOP;

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

-- Collect metadata
SELECT 'row_count,' || count(*) FROM tenant_docs;
SELECT 'index_size,' || pg_size_pretty(
    pg_relation_size('tenant_docs_bm25_idx'));

-- Single-term query on tenant 1
SELECT 'single_p50,' || round(p50_ms, 3)
FROM bench_query('database', 1);
SELECT 'single_p95,' || round(p95_ms, 3)
FROM bench_query('database', 1);

-- Multi-term query on tenant 1
SELECT 'multi_p50,' || round(p50_ms, 3)
FROM bench_query('algorithm database optimization', 1);
SELECT 'multi_p95,' || round(p95_ms, 3)
FROM bench_query('algorithm database optimization', 1);

-- Domain-specific term: 'diagnosis' is Medical (domain 0).
SELECT 'domain_p50,' || round(p50_ms, 3)
FROM bench_query('diagnosis', 1);
SELECT 'domain_p95,' || round(p95_ms, 3)
FROM bench_query('diagnosis', 1);
EOSQL

# SQL to replace the tenant index with a global index
read -r -d '' SWAP_INDEX_SQL << 'EOSQL' || true
\set ON_ERROR_STOP on
\timing on
\echo '--- Replacing tenant index with global index ---'
DROP INDEX tenant_docs_bm25_idx;
CREATE INDEX tenant_docs_bm25_idx ON tenant_docs
    USING bm25(content)
    WITH (text_config='english');
\echo '--- Spill memtable ---'
DO $$
BEGIN
    PERFORM bm25_spill_index('tenant_docs_bm25_idx');
    RAISE NOTICE 'Spill completed';
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE 'Spill skipped (parallel build): %', SQLERRM;
END $$;
SELECT 'Global index size:' AS info,
       pg_size_pretty(pg_relation_size('tenant_docs_bm25_idx'))
           AS index_size;
EOSQL

run_bench() {
    local label=$1
    local output_file=$2

    echo ""
    echo "--- Running queries (${label}) ---"
    echo "${BENCH_SQL}" \
        | psql -t -A "${DATABASE_URL}" \
        > "${output_file}" 2>/dev/null
}

# Parse a result file into shell variables with a given prefix
parse_results() {
    local file=$1
    local prefix=$2
    while IFS=, read -r key value; do
        [ -z "$key" ] && continue
        key=$(echo "$key" | tr -d ' ')
        value=$(echo "$value" | tr -d ' ')
        eval "${prefix}_${key}=\"${value}\""
    done < "$file"
}

# ---- 5 tenants ----
echo ""
echo "========================================"
echo "  Setup: 5 tenants"
echo "========================================"
echo ""

NUM_TENANTS=5 DOCS_MULTIPLIER=1 \
    DATABASE_URL="${DATABASE_URL}" \
    bash "${SCRIPT_DIR}/setup.sh"

run_bench "5 tenants, tenant index" "$R_5T"

echo "${SWAP_INDEX_SQL}" | psql "${DATABASE_URL}" 2>&1

run_bench "5 tenants, global index" "$R_5G"

# ---- 50 tenants ----
echo ""
echo "========================================"
echo "  Setup: 50 tenants"
echo "========================================"
echo ""

NUM_TENANTS=50 DOCS_MULTIPLIER=1 \
    DATABASE_URL="${DATABASE_URL}" \
    bash "${SCRIPT_DIR}/setup.sh"

run_bench "50 tenants, tenant index" "$R_50T"

echo "${SWAP_INDEX_SQL}" | psql "${DATABASE_URL}" 2>&1

run_bench "50 tenants, global index" "$R_50G"

# ---- Parse all results ----
parse_results "$R_5T"  "t5t"
parse_results "$R_5G"  "t5g"
parse_results "$R_50T" "t50t"
parse_results "$R_50G" "t50g"

# ---- Print comparison table ----
echo ""
echo "========================================"
echo "  Results: Tenant Index vs Global Index"
echo "========================================"
echo ""
printf "%-20s %-15s %-15s %-15s %-15s\n" \
    "" "5t tenant" "5t global" "50t tenant" "50t global"
printf "%-20s %-15s %-15s %-15s %-15s\n" \
    "" "---------" "---------" "----------" "----------"
printf "%-20s %-15s %-15s %-15s %-15s\n" \
    "single-term p50:" \
    "${t5t_single_p50} ms" "${t5g_single_p50} ms" \
    "${t50t_single_p50} ms" "${t50g_single_p50} ms"
printf "%-20s %-15s %-15s %-15s %-15s\n" \
    "single-term p95:" \
    "${t5t_single_p95} ms" "${t5g_single_p95} ms" \
    "${t50t_single_p95} ms" "${t50g_single_p95} ms"
printf "%-20s %-15s %-15s %-15s %-15s\n" \
    "multi-term p50:" \
    "${t5t_multi_p50} ms" "${t5g_multi_p50} ms" \
    "${t50t_multi_p50} ms" "${t50g_multi_p50} ms"
printf "%-20s %-15s %-15s %-15s %-15s\n" \
    "multi-term p95:" \
    "${t5t_multi_p95} ms" "${t5g_multi_p95} ms" \
    "${t50t_multi_p95} ms" "${t50g_multi_p95} ms"
printf "%-20s %-15s %-15s %-15s %-15s\n" \
    "domain-term p50:" \
    "${t5t_domain_p50} ms" "${t5g_domain_p50} ms" \
    "${t50t_domain_p50} ms" "${t50g_domain_p50} ms"
printf "%-20s %-15s %-15s %-15s %-15s\n" \
    "domain-term p95:" \
    "${t5t_domain_p95} ms" "${t5g_domain_p95} ms" \
    "${t50t_domain_p95} ms" "${t50g_domain_p95} ms"
printf "%-20s %-15s %-15s %-15s %-15s\n" \
    "index size:" \
    "${t5t_index_size}" "${t5g_index_size}" \
    "${t50t_index_size}" "${t50g_index_size}"
printf "%-20s %-15s %-15s %-15s %-15s\n" \
    "row count:" \
    "${t5t_row_count}" "${t5g_row_count}" \
    "${t50t_row_count}" "${t50g_row_count}"
echo ""
