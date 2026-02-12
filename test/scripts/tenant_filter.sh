#!/bin/bash
#
# Cross-session tenant filter tests for pg_textsearch
#
# Verifies that BMW block-skip optimization works correctly when
# queries run in a different psql session than the one that created
# the index. This catches regressions where tenant filter push-down
# only works within the CREATE INDEX session.
#
# Key metric: blocks_scanned from BMW stats log output.
# With 1M docs across 100 tenants, a universal term has ~7800 blocks.
# With tenant ordering, one tenant (10K docs) should scan ~78 blocks.
#

set -e

# Use PG_BIN_DIR if set (from Makefile) to find correct PG binaries
if [ -n "${PG_BIN_DIR}" ]; then
    export PATH="${PG_BIN_DIR}:${PATH}"
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_PORT=55437
TEST_DB=pg_textsearch_tenant_filter_test
DATA_DIR="${SCRIPT_DIR}/../tmp_tenant_filter_test"
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
    log "Cleaning up tenant filter test environment..."
    jobs -p | xargs -r kill 2>/dev/null || true
    if [ -f "${DATA_DIR}/postmaster.pid" ]; then
        pg_ctl stop -D "${DATA_DIR}" -m immediate &>/dev/null || true
    fi
    rm -rf "${DATA_DIR}"
    exit $exit_code
}

trap cleanup EXIT INT TERM

setup_test_db() {
    log "Setting up tenant filter test PostgreSQL instance..."

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
max_parallel_maintenance_workers = 0
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

# Run a query in a fresh psql session with BMW stats enabled.
# Captures both stdout (results) and stderr (LOG messages).
run_fresh_bmw_query() {
    local sql="$1"
    printf '%s\n' \
        "SET pg_textsearch.log_bmw_stats = on;" \
        "SET client_min_messages = log;" \
        "SET enable_seqscan = off;" \
        "$sql" | \
        psql -h "${DATA_DIR}" -p "${TEST_PORT}" -d "${TEST_DB}" \
            2>&1
}

# Parse blocks_scanned from BMW stats log line
parse_blocks_scanned() {
    echo "$1" | grep "BMW stats" | head -1 | \
        sed -E 's/.*blocks: ([0-9]+) scanned.*/\1/'
}

# Parse skip percentage from BMW stats log line
parse_skip_pct() {
    echo "$1" | grep "BMW stats" | head -1 | \
        sed -E 's/.* ([0-9]+\.[0-9])% skip.*/\1/'
}

# Assert: integer value >= threshold
assert_ge() {
    local actual="$1" threshold="$2" msg="$3"
    if ! [[ "$actual" =~ ^[0-9]+$ ]]; then
        error "Assertion failed: $msg (got '$actual', not a number)"
    fi
    if [ "$actual" -lt "$threshold" ]; then
        error "Assertion failed: $msg (actual=$actual, expected>=$threshold)"
    fi
}

# Assert: integer value <= threshold
assert_le() {
    local actual="$1" threshold="$2" msg="$3"
    if ! [[ "$actual" =~ ^[0-9]+$ ]]; then
        error "Assertion failed: $msg (got '$actual', not a number)"
    fi
    if [ "$actual" -gt "$threshold" ]; then
        error "Assertion failed: $msg (actual=$actual, expected<=$threshold)"
    fi
}

# Assert: value == expected
assert_eq() {
    local actual="$1" expected="$2" msg="$3"
    if [ "$actual" != "$expected" ]; then
        error "Assertion failed: $msg (actual='$actual', expected='$expected')"
    fi
}

setup_test_data() {
    log "Creating test data: 100 tenants x 10K docs = 1M rows..."

    run_sql_quiet "CREATE TABLE t (
        id SERIAL PRIMARY KEY,
        content TEXT,
        tenant_id INTEGER NOT NULL
    );"

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
        (i % 100) + 1
    FROM generate_series(1, 1000000) i;"

    local total
    total=$(run_sql_value "SELECT COUNT(*) FROM t;")
    local tenants
    tenants=$(run_sql_value "SELECT COUNT(DISTINCT tenant_id) FROM t;")
    info "Inserted $total docs across $tenants tenants"

    run_sql_quiet "ANALYZE t;"

    log "Creating BM25 index with tenant_column (no parallelism)..."
    run_sql_quiet "CREATE INDEX idx ON t
        USING bm25(content)
        WITH (text_config='english', tenant_column='tenant_id');"

    log "Spilling memtable to disk segment..."
    run_sql_quiet "SELECT bm25_spill_index('idx');"

    info "Setup complete"
}

#
# Test 1: Cross-session tenant filter (the critical regression test)
#
# Previously, BMW queries only achieved O(K) block scanning in the
# same session as CREATE INDEX, but scanned all N*K blocks in a new
# session due to tenant filter not being pushed down.
#
test_cross_session_tenant_filter() {
    log "Test 1: Cross-session tenant filter push-down"
    info "Querying tenant_id=1, term='document' in fresh session..."

    local output
    output=$(run_fresh_bmw_query "
        SELECT id FROM t
        WHERE content <@> to_bm25query('document', 'idx') < 0
            AND tenant_id = 1
        ORDER BY content <@> to_bm25query('document', 'idx')
        LIMIT 10;")

    local blocks
    blocks=$(parse_blocks_scanned "$output")
    local skip_pct
    skip_pct=$(parse_skip_pct "$output")

    if [ -z "$blocks" ] || [ -z "$skip_pct" ]; then
        error "Failed to parse BMW stats from output: $output"
    fi

    local skip_int=${skip_pct%.*}
    info "blocks_scanned=$blocks, skip=$skip_pct%"

    assert_ge "$skip_int" 90 "skip percentage should be >= 90%"
    assert_le "$blocks" 300 "blocks_scanned should be <= 300"

    log "Test 1 passed: cross-session tenant filter works" \
        "(blocks=$blocks, skip=$skip_pct%)"
}

#
# Test 2: Empty tenant early exit
#
# Tenant 999 has 0 documents. The query should return 0 rows
# immediately via the early-exit path (before BMW scoring).
#
test_empty_tenant_early_exit() {
    log "Test 2: Empty tenant early exit"
    info "Querying tenant_id=999 (not in data) in fresh session..."

    local row_count
    row_count=$(printf '%s\n' \
        "SET enable_seqscan = off;" \
        "SELECT COUNT(*) FROM t" \
        "WHERE content <@> to_bm25query('document', 'idx') < 0" \
        "    AND tenant_id = 999;" | \
        psql -h "${DATA_DIR}" -p "${TEST_PORT}" \
            -d "${TEST_DB}" -tA 2>/dev/null | \
        grep -E '^[0-9]+$' | tail -1)
    assert_eq "$row_count" "0" "empty tenant should return 0 rows"

    log "Test 2 passed: empty tenant returns 0 rows"
}

#
# Test 3: Consistent O(K) scaling across tenants
#
# Tenants 1, 50, 100 each have 10K docs. All should show similar
# block counts and high skip percentages, proving the optimization
# doesn't depend on tenant_id value.
#
test_scaling_across_tenants() {
    log "Test 3: Consistent O(K) scaling across tenants 1, 50, 100"

    for tid in 1 50 100; do
        info "Querying tenant_id=$tid in fresh session..."

        local output
        output=$(run_fresh_bmw_query "
            SELECT id FROM t
            WHERE content <@> to_bm25query('document', 'idx') < 0
                AND tenant_id = $tid
            ORDER BY content <@> to_bm25query('document', 'idx')
            LIMIT 10;")

        local blocks
        blocks=$(parse_blocks_scanned "$output")
        local skip_pct
        skip_pct=$(parse_skip_pct "$output")

        if [ -z "$blocks" ] || [ -z "$skip_pct" ]; then
            error "Failed to parse BMW stats for tenant $tid"
        fi

        local skip_int=${skip_pct%.*}
        info "  tenant=$tid: blocks_scanned=$blocks, skip=$skip_pct%"

        assert_ge "$skip_int" 90 \
            "tenant $tid skip percentage should be >= 90%"
    done

    log "Test 3 passed: all tenants show consistent O(K) scanning"
}

#
# Test 4: No-filter baseline proves optimization is effective
#
# Same query without tenant filter should scan many more blocks,
# proving the tenant skip optimization is actually doing something.
#
test_no_filter_baseline() {
    log "Test 4: No-filter baseline comparison"
    info "Querying without tenant filter in fresh session..."

    local output
    output=$(run_fresh_bmw_query "
        SELECT id FROM t
        WHERE content <@> to_bm25query('document', 'idx') < 0
        ORDER BY content <@> to_bm25query('document', 'idx')
        LIMIT 10;")

    local blocks
    blocks=$(parse_blocks_scanned "$output")

    if [ -z "$blocks" ]; then
        error "Failed to parse BMW stats for no-filter query"
    fi

    info "No-filter blocks_scanned=$blocks"
    assert_ge "$blocks" 2000 \
        "no-filter should scan >= 2000 blocks"

    log "Test 4 passed: no-filter scans many more blocks ($blocks)"
}

run_tenant_filter_tests() {
    log "Starting cross-session tenant filter tests"

    setup_test_data

    test_cross_session_tenant_filter
    test_empty_tenant_early_exit
    test_scaling_across_tenants
    test_no_filter_baseline
}

main() {
    log "Starting pg_textsearch tenant filter testing..."

    command -v pg_ctl >/dev/null 2>&1 || error "pg_ctl not found"
    command -v psql >/dev/null 2>&1 || error "psql not found"

    setup_test_db
    run_tenant_filter_tests

    log "All cross-session tenant filter tests passed!"
    exit 0
}

if [ "${BASH_SOURCE[0]}" == "${0}" ]; then
    main "$@"
fi
