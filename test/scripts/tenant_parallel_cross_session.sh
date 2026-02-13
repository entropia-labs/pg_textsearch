#!/bin/bash
#
# Cross-session tenant query after parallel index build
#
# Regression test for a bug where tenant stats pages were truncated
# during parallel build. The truncation step only considered segment
# pages when calculating max_used, so tenant stats pages (written at
# P_NEW beyond segments) were removed. New sessions then found an
# empty tenant stats dshash and returned 0 rows for tenant queries.
#

set -e

# Use PG_BIN_DIR if set (from Makefile) to find correct PG binaries
if [ -n "${PG_BIN_DIR}" ]; then
    export PATH="${PG_BIN_DIR}:${PATH}"
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_PORT=55438
TEST_DB=pg_textsearch_tenant_parallel_test
DATA_DIR="${SCRIPT_DIR}/../tmp_tenant_parallel_test"
LOGFILE="${DATA_DIR}/postgres.log"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log() { echo -e "${GREEN}[$(date '+%H:%M:%S')] $1${NC}"; }
warn() { echo -e "${YELLOW}[$(date '+%H:%M:%S')] WARNING: $1${NC}"; }
error() { echo -e "${RED}[$(date '+%H:%M:%S')] ERROR: $1${NC}"; exit 1; }
info() { echo -e "${BLUE}[$(date '+%H:%M:%S')] $1${NC}"; }

cleanup() {
    local exit_code=$?
    log "Cleaning up..."
    jobs -p | xargs -r kill 2>/dev/null || true
    if [ -f "${DATA_DIR}/postmaster.pid" ]; then
        pg_ctl stop -D "${DATA_DIR}" -m immediate &>/dev/null || true
    fi
    rm -rf "${DATA_DIR}"
    exit $exit_code
}

trap cleanup EXIT INT TERM

setup_test_db() {
    log "Setting up PostgreSQL instance..."

    rm -rf "${DATA_DIR}"
    mkdir -p "${DATA_DIR}"

    initdb -D "${DATA_DIR}" \
        --auth-local=trust --auth-host=trust >/dev/null 2>&1

    cat >> "${DATA_DIR}/postgresql.conf" << EOF
port = ${TEST_PORT}
max_connections = 20
shared_buffers = 256MB
unix_socket_directories = '${DATA_DIR}'
listen_addresses = 'localhost'
log_min_messages = warning
max_parallel_maintenance_workers = 4
EOF

    pg_ctl start -D "${DATA_DIR}" -l "${LOGFILE}" -w \
        || error "Failed to start PostgreSQL"

    createdb -h "${DATA_DIR}" -p "${TEST_PORT}" "${TEST_DB}"
    psql -h "${DATA_DIR}" -p "${TEST_PORT}" -d "${TEST_DB}" \
        -c "CREATE EXTENSION pg_textsearch;" >/dev/null
}

run_sql_quiet() {
    psql -h "${DATA_DIR}" -p "${TEST_PORT}" -d "${TEST_DB}" \
        -c "$1" >/dev/null 2>&1
}

run_sql_value() {
    psql -h "${DATA_DIR}" -p "${TEST_PORT}" -d "${TEST_DB}" \
        -tAc "$1" 2>/dev/null
}

# Run multi-statement SQL and return the last non-empty line
run_sql_last_value() {
    printf '%s\n' "$1" | \
        psql -h "${DATA_DIR}" -p "${TEST_PORT}" -d "${TEST_DB}" \
            -tA 2>/dev/null | \
        grep -E '^[0-9]+$' | tail -1
}

# Run a query in a fresh psql session (new backend = cold caches)
run_fresh_query_value() {
    printf '%s\n' "$1" | \
        psql -h "${DATA_DIR}" -p "${TEST_PORT}" -d "${TEST_DB}" \
            -tA 2>/dev/null | \
        grep -E '^[0-9]+$' | tail -1
}

assert_gt() {
    local actual="$1" threshold="$2" msg="$3"
    if ! [[ "$actual" =~ ^[0-9]+$ ]]; then
        error "Assertion failed: $msg (got '$actual', not a number)"
    fi
    if [ "$actual" -le "$threshold" ]; then
        error "Assertion failed: $msg" \
            "(actual=$actual, expected>$threshold)"
    fi
}

assert_eq() {
    local actual="$1" expected="$2" msg="$3"
    if [ "$actual" != "$expected" ]; then
        error "Assertion failed: $msg" \
            "(actual='$actual', expected='$expected')"
    fi
}

setup_test_data() {
    log "Creating test data: 5 tenants x ~24K docs = ~120K rows..."

    run_sql_quiet "CREATE TABLE t (
        id SERIAL PRIMARY KEY,
        content TEXT,
        tenant_id INTEGER NOT NULL
    );"

    # 120K rows across 5 tenants — enough to trigger parallel build
    run_sql_quiet "INSERT INTO t (content, tenant_id)
    SELECT
        'document ' ||
        CASE (i % 5)
            WHEN 0 THEN 'database query optimization'
            WHEN 1 THEN 'search engine ranking'
            WHEN 2 THEN 'machine learning training'
            WHEN 3 THEN 'distributed systems cloud'
            WHEN 4 THEN 'natural language processing'
        END || ' entry ' || i,
        (i % 5) + 1
    FROM generate_series(1, 120000) i;"

    run_sql_quiet "ANALYZE t;"

    log "Creating BM25 index with tenant_column (parallel build)..."
    local notice
    notice=$(psql -h "${DATA_DIR}" -p "${TEST_PORT}" -d "${TEST_DB}" \
        -c "CREATE INDEX idx ON t USING bm25(content)
            WITH (text_config='english', tenant_column='tenant_id');" \
        2>&1)

    # Verify parallel build was used
    if echo "$notice" | grep -q "parallel index build"; then
        local workers
        workers=$(echo "$notice" | grep "parallel index build" | \
            sed -E 's/.*launched ([0-9]+) of.*/\1/')
        info "Parallel build used $workers workers"
    else
        warn "Parallel build was NOT triggered — test may not" \
            "exercise the bug path"
    fi
}

#
# Test 1: Same-session tenant query (baseline — should always work)
#
test_same_session() {
    log "Test 1: Same-session tenant query after parallel build"

    local count
    count=$(run_sql_last_value "
SET enable_seqscan = off;
SELECT COUNT(*) FROM t
WHERE content <@> to_bm25query('database', 'idx') < 0
    AND tenant_id = 1;")

    info "Same-session count for tenant 1: $count"
    assert_gt "$count" 0 "same-session tenant query should return rows"

    log "Test 1 passed"
}

#
# Test 2: Cross-session tenant query (the regression being fixed)
#
test_cross_session() {
    log "Test 2: Cross-session tenant query after parallel build"

    local count
    count=$(run_fresh_query_value "
        SET enable_seqscan = off;
        SELECT COUNT(*) FROM t
        WHERE content <@> to_bm25query('database', 'idx') < 0
            AND tenant_id = 1;")

    info "Cross-session count for tenant 1: $count"
    assert_gt "$count" 0 \
        "cross-session tenant query should return rows"

    log "Test 2 passed"
}

#
# Test 3: Cross-session query for all tenants
#
test_cross_session_all_tenants() {
    log "Test 3: Cross-session query for each tenant"

    for tid in 1 2 3 4 5; do
        local count
        count=$(run_fresh_query_value "
            SET enable_seqscan = off;
            SELECT COUNT(*) FROM t
            WHERE content <@> to_bm25query('document', 'idx') < 0
                AND tenant_id = $tid;")

        info "  tenant $tid: $count rows"
        assert_gt "$count" 0 \
            "cross-session query for tenant $tid should return rows"
    done

    log "Test 3 passed"
}

#
# Test 4: Cross-session global query (no tenant filter) as sanity check
#
test_cross_session_global() {
    log "Test 4: Cross-session global query (no tenant filter)"

    local count
    count=$(run_fresh_query_value "
        SET enable_seqscan = off;
        SELECT COUNT(*) FROM t
        WHERE content <@> to_bm25query('database', 'idx') < 0;")

    info "Cross-session global count: $count"
    assert_gt "$count" 0 \
        "cross-session global query should return rows"

    log "Test 4 passed"
}

main() {
    log "Starting parallel build + cross-session tenant query test"

    command -v pg_ctl >/dev/null 2>&1 || error "pg_ctl not found"
    command -v psql >/dev/null 2>&1 || error "psql not found"

    setup_test_db
    setup_test_data

    test_same_session
    test_cross_session
    test_cross_session_all_tenants
    test_cross_session_global

    log "All tests passed!"
    exit 0
}

if [ "${BASH_SOURCE[0]}" == "${0}" ]; then
    main "$@"
fi
