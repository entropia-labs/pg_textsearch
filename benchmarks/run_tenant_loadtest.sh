#!/bin/bash
#
# Multi-Tenant Load Test Benchmark
#
# Measures insert and query performance at realistic multi-tenant scale:
# 5 tenants, ~500 documents (200 chunks each), ~100K total rows.
#
# Usage:
#   ./benchmarks/run_tenant_loadtest.sh
#   PG_BIN_DIR=/opt/homebrew/opt/postgresql@17/bin \
#       ./benchmarks/run_tenant_loadtest.sh
#
# Modes:
#   --local     Start a dedicated PG instance (default)
#   --external  Use existing PG (honors PGPORT/PGHOST/PGDATABASE)

set -e

MODE="local"
for arg in "$@"; do
    case "$arg" in
        --external) MODE="external" ;;
        --local) MODE="local" ;;
    esac
done

# Use PG_BIN_DIR if provided
if [ -n "${PG_BIN_DIR}" ]; then
    export PATH="${PG_BIN_DIR}:${PATH}"
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SQL_DIR="${SCRIPT_DIR}/sql/tenant_loadtest"
DATA_DIR="${SCRIPT_DIR}/tmp_tenant_loadtest"
LOGFILE="${DATA_DIR}/postgres.log"
NUM_TENANTS=${NUM_TENANTS:-5}
DOCS_MULTIPLIER=${DOCS_MULTIPLIER:-1}

if [ "$MODE" = "external" ]; then
    TEST_PORT=${PGPORT:-5432}
    TEST_DB=${PGDATABASE:-postgres}
    SOCK_DIR="${PGHOST:-/tmp}"
else
    TEST_PORT=55438
    TEST_DB=pg_textsearch_tenant_loadtest
    SOCK_DIR="${DATA_DIR}"
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log() { echo -e "${GREEN}[$(date '+%H:%M:%S')] $1${NC}"; }
warn() {
    echo -e "${YELLOW}[$(date '+%H:%M:%S')] WARNING: $1${NC}"
}
error() {
    echo -e "${RED}[$(date '+%H:%M:%S')] ERROR: $1${NC}"
    exit 1
}
info() { echo -e "${BLUE}[$(date '+%H:%M:%S')] $1${NC}"; }

# ms-precision timing on macOS (no date +%s.%N)
now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }

cleanup() {
    local exit_code=$?
    log "Cleaning up..."
    jobs -p | xargs -r kill 2>/dev/null || true
    if [ "$MODE" = "local" ]; then
        # Save PG log on failure for diagnosis
        if [ $exit_code -ne 0 ]; then
            local pg_log="${DATA_DIR}/log/postgresql.log"
            if [ -f "$pg_log" ]; then
                warn "PG log (last 50 lines):"
                tail -50 "$pg_log" 2>/dev/null || true
            fi
        fi
        if [ -f "${DATA_DIR}/postmaster.pid" ]; then
            pg_ctl stop -D "${DATA_DIR}" -m immediate \
                &>/dev/null || true
        fi
        rm -rf "${DATA_DIR}"
    else
        # Drop test table in external mode
        run_sql -c \
            "DROP TABLE IF EXISTS tenant_docs CASCADE;" \
            >/dev/null 2>&1 || true
    fi
    exit $exit_code
}

trap cleanup EXIT INT TERM

run_sql() {
    psql -h "${SOCK_DIR}" -p "${TEST_PORT}" \
        -d "${TEST_DB}" "$@"
}

run_sql_value() {
    psql -h "${SOCK_DIR}" -p "${TEST_PORT}" \
        -d "${TEST_DB}" -tAc "$1" 2>/dev/null
}

# ============================================================
# Phase 1: Setup PostgreSQL instance
# ============================================================
setup_test_db() {
    if [ "$MODE" = "external" ]; then
        log "Using external PostgreSQL on port ${TEST_PORT}..."
        run_sql -c \
            "CREATE EXTENSION IF NOT EXISTS pg_textsearch;" \
            >/dev/null 2>&1
        log "Connected to external PG on port ${TEST_PORT}"
        return
    fi

    log "Setting up dedicated PostgreSQL instance..."

    command -v pg_ctl >/dev/null 2>&1 \
        || error "pg_ctl not found in PATH"
    command -v psql >/dev/null 2>&1 \
        || error "psql not found in PATH"

    rm -rf "${DATA_DIR}"
    mkdir -p "${DATA_DIR}"

    initdb -D "${DATA_DIR}" \
        --auth-local=trust --auth-host=trust \
        >/dev/null 2>&1

    cat >> "${DATA_DIR}/postgresql.conf" << EOF
port = ${TEST_PORT}
max_connections = 20
shared_buffers = 128MB
work_mem = 32MB
maintenance_work_mem = 128MB
unix_socket_directories = '${DATA_DIR}'
listen_addresses = 'localhost'
log_min_messages = warning
max_parallel_maintenance_workers = 4
logging_collector = on
log_directory = 'log'
log_filename = 'postgresql.log'
EOF

    pg_ctl start -D "${DATA_DIR}" -l "${LOGFILE}" -w \
        || error "Failed to start PostgreSQL"

    createdb -h "${DATA_DIR}" -p "${TEST_PORT}" "${TEST_DB}"
    run_sql -c "CREATE EXTENSION pg_textsearch;" >/dev/null

    log "PostgreSQL running on port ${TEST_PORT}"
}

# ============================================================
# Phase 2: Run setup SQL (data gen + index build)
# ============================================================
run_setup() {
    log "Running setup: data generation + index build..."

    local t_start
    t_start=$(now_ms)

    local rc=0
    run_sql -v num_tenants="${NUM_TENANTS}" \
        -v docs_multiplier="${DOCS_MULTIPLIER}" \
        -f "${SQL_DIR}/01-setup.sql" 2>&1 | \
        while IFS= read -r line; do
            if echo "$line" | grep -qE \
                "^Time:|===|---|info|Rows|Table|Index|Tenant|Documents|entries_spilled|Inserted|ERROR|FATAL"; then
                info "  $line"
            fi
        done
    # Check if psql succeeded via PIPESTATUS
    rc=${PIPESTATUS[0]:-0}

    local t_end
    t_end=$(now_ms)
    SETUP_TOTAL_SEC=$(( (t_end - t_start) / 1000 ))

    # Check if PG is still alive after setup
    if [ "$MODE" = "local" ] && \
       [ ! -f "${DATA_DIR}/postmaster.pid" ]; then
        local pg_log="${DATA_DIR}/log/postgresql.log"
        if [ -f "$pg_log" ]; then
            warn "PG crashed during setup. Last 30 log lines:"
            tail -30 "$pg_log" 2>/dev/null || true
        fi
        error "PostgreSQL crashed during setup"
    fi

    local row_count
    row_count=$(run_sql_value \
        "SELECT count(*) FROM tenant_docs;") || true
    if [ -z "$row_count" ] || [ "$row_count" = "0" ]; then
        error "Setup failed: no rows in tenant_docs"
    fi
    info "Total rows in table: ${row_count}"
}

# ============================================================
# Phase 3: Cold query measurement (fresh psql sessions)
# ============================================================
run_cold_queries() {
    log "Running cold query benchmarks (5 fresh sessions)..."

    local cold_single=()
    local cold_multi=()

    for i in $(seq 1 5); do
        # Single-term cold query
        local t_start t_end elapsed
        t_start=$(now_ms)
        printf '%s\n' \
            "SET enable_seqscan = off;" \
            "SET pg_textsearch.log_bmw_stats = on;" \
            "SELECT id FROM tenant_docs" \
            "WHERE content <@> to_bm25query('database'," \
            "  'tenant_docs_bm25_idx') < 0" \
            "  AND tenant_id = 1" \
            "ORDER BY content <@> to_bm25query('database'," \
            "  'tenant_docs_bm25_idx')" \
            "LIMIT 10;" | \
            run_sql -tA >/dev/null 2>&1
        t_end=$(now_ms)
        elapsed=$((t_end - t_start))
        cold_single+=("$elapsed")

        # Multi-term cold query
        t_start=$(now_ms)
        printf '%s\n' \
            "SET enable_seqscan = off;" \
            "SET pg_textsearch.log_bmw_stats = on;" \
            "SELECT id FROM tenant_docs" \
            "WHERE content <@>" \
            "  to_bm25query('algorithm database optimization'," \
            "  'tenant_docs_bm25_idx') < 0" \
            "  AND tenant_id = 1" \
            "ORDER BY content <@>" \
            "  to_bm25query('algorithm database optimization'," \
            "  'tenant_docs_bm25_idx')" \
            "LIMIT 10;" | \
            run_sql -tA >/dev/null 2>&1
        t_end=$(now_ms)
        elapsed=$((t_end - t_start))
        cold_multi+=("$elapsed")
    done

    # Compute medians (sort and take middle value)
    COLD_SINGLE_MEDIAN=$(printf '%s\n' \
        "${cold_single[@]}" | sort -n | sed -n '3p')
    COLD_MULTI_MEDIAN=$(printf '%s\n' \
        "${cold_multi[@]}" | sort -n | sed -n '3p')

    info "Cold single-term median: ${COLD_SINGLE_MEDIAN}ms"
    info "Cold multi-term median:  ${COLD_MULTI_MEDIAN}ms"
}

# ============================================================
# Phase 4: Insert benchmarks
# ============================================================
run_insert_bench() {
    log "Running insert benchmarks..."

    local rc=0
    INSERT_OUTPUT=$(run_sql \
        -f "${SQL_DIR}/02-insert-bench.sql" 2>&1) || rc=$?
    if [ $rc -ne 0 ]; then
        error "Insert benchmark failed (exit $rc):\n${INSERT_OUTPUT}"
    fi

    SINGLE_INSERT=$(echo "$INSERT_OUTPUT" | \
        grep "SINGLE_INSERT:" | \
        sed -E 's/.*SINGLE_INSERT: //')
    BATCH_INSERT=$(echo "$INSERT_OUTPUT" | \
        grep "BATCH_INSERT:" | \
        sed -E 's/.*BATCH_INSERT: //')

    info "Single insert: ${SINGLE_INSERT}"
    info "Batch insert:  ${BATCH_INSERT}"
}

# ============================================================
# Phase 5: Query benchmarks
# ============================================================
run_query_bench() {
    log "Running query benchmarks..."

    local rc=0
    QUERY_OUTPUT=$(run_sql \
        -f "${SQL_DIR}/03-query-bench.sql" 2>&1) || rc=$?
    if [ $rc -ne 0 ]; then
        error "Query benchmark failed (exit $rc):\n${QUERY_OUTPUT}"
    fi

    # Print relevant output lines
    echo "$QUERY_OUTPUT" | \
        grep -E "test|Benchmark|---" | \
        while IFS= read -r line; do
            info "  $line"
        done
}

# ============================================================
# Phase 6: Extract BMW stats from postgres log
# ============================================================
extract_bmw_stats() {
    log "Extracting BMW block skip stats from log..."

    local pg_log="${DATA_DIR}/log/postgresql.log"
    if [ ! -f "$pg_log" ]; then
        pg_log="${LOGFILE}"
    fi

    if [ -f "$pg_log" ]; then
        BMW_STATS=$(grep "BMW stats:" "$pg_log" 2>/dev/null \
            | tail -20 \
            | sed -E 's/.*blocks: ([0-9]+) scanned, ([0-9]+) skipped, ([0-9.]+)%.*/  scanned=\1 skipped=\2 skip=\3%/' \
            | tail -5)
    fi

    if [ -n "$BMW_STATS" ]; then
        info "BMW stats (last 5):"
        echo "$BMW_STATS" | while IFS= read -r line; do
            info "  $line"
        done
    else
        warn "No BMW stats found in log"
    fi
}

# ============================================================
# Phase 7: Print summary
# ============================================================
print_summary() {
    echo ""
    echo "========================================================"
    echo "=== MULTI-TENANT LOAD TEST RESULTS ==="
    echo "========================================================"
    echo "Dataset: ${NUM_TENANTS} tenants, docs_multiplier=${DOCS_MULTIPLIER}, 100 tok/chunk"
    echo ""
    echo "--- Setup ---"
    echo "Total setup time: ${SETUP_TOTAL_SEC}s"
    echo ""
    echo "--- Insert Latency ---"
    echo "Single row:   ${SINGLE_INSERT}"
    echo "Batch (200):  ${BATCH_INSERT}"
    echo ""
    echo "--- Cold Query (fresh session, LIMIT 10) ---"
    echo "Single-term tenant=1:  median=${COLD_SINGLE_MEDIAN}ms"
    echo "Multi-term tenant=1:   median=${COLD_MULTI_MEDIAN}ms"
    echo ""
    echo "--- Warm Query Results (LIMIT 10, from SQL output) ---"
    echo "$QUERY_OUTPUT" | grep -E "^ " | head -30
    echo ""
    if [ -n "$BMW_STATS" ]; then
        echo "--- BMW Block Skip Stats (last 5 from log) ---"
        echo "$BMW_STATS"
        echo ""
    fi
    echo "========================================================"
}

# ============================================================
# Main
# ============================================================
main() {
    log "Multi-Tenant Load Test Benchmark"
    log "================================="

    setup_test_db
    run_setup
    run_cold_queries
    run_insert_bench
    run_query_bench
    extract_bmw_stats
    print_summary

    log "Benchmark complete!"
}

main "$@"
