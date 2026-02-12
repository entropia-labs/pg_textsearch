-- Tenant benchmark: skewed distribution with up to 100k rows per tenant
-- Demonstrates that tenant-aware indexing avoids work amplification
-- and produces correct per-tenant BM25 scores at scale.

CREATE EXTENSION pg_textsearch;

-- ============================================================
-- SECTION 1: Setup - skewed tenant distribution
-- ============================================================
-- Tenant 1: 100,000 docs (large tenant)
-- Tenant 2:  10,000 docs (medium tenant)
-- Tenant 3:   1,000 docs (small tenant)
-- Tenant 4:     100 docs (tiny tenant)
-- Total: 111,100 docs

CREATE TABLE tenant_bench (
    id SERIAL PRIMARY KEY,
    content TEXT,
    tenant_id INTEGER NOT NULL
);

-- Tenant 1: 100k docs with varied vocabulary
INSERT INTO tenant_bench (content, tenant_id)
SELECT
    CASE (i % 5)
        WHEN 0 THEN 'database query optimization performance'
        WHEN 1 THEN 'search engine ranking algorithm design'
        WHEN 2 THEN 'machine learning neural network training'
        WHEN 3 THEN 'distributed systems cloud computing scale'
        WHEN 4 THEN 'natural language processing text analysis'
    END || ' document ' || i,
    1
FROM generate_series(1, 100000) i;

-- Tenant 2: 10k docs
INSERT INTO tenant_bench (content, tenant_id)
SELECT
    CASE (i % 4)
        WHEN 0 THEN 'database indexing storage management'
        WHEN 1 THEN 'search relevance scoring retrieval'
        WHEN 2 THEN 'machine learning model evaluation'
        WHEN 3 THEN 'web application framework development'
    END || ' document ' || i,
    2
FROM generate_series(1, 10000) i;

-- Tenant 3: 1k docs
INSERT INTO tenant_bench (content, tenant_id)
SELECT
    CASE (i % 3)
        WHEN 0 THEN 'database administration backup recovery'
        WHEN 1 THEN 'search engine optimization strategy'
        WHEN 2 THEN 'cloud infrastructure monitoring alerting'
    END || ' document ' || i,
    3
FROM generate_series(1, 1000) i;

-- Tenant 4: 100 docs (tiny tenant, high amplification)
INSERT INTO tenant_bench (content, tenant_id)
SELECT 'rare unique specialized niche document ' || i,
    4
FROM generate_series(1, 100) i;

-- ============================================================
-- SECTION 2: Create tenant-aware index
-- ============================================================

-- Disable parallel build to isolate tenant-aware performance testing
SET max_parallel_maintenance_workers = 0;
ANALYZE tenant_bench;

CREATE INDEX tenant_bench_idx ON tenant_bench USING bm25(content)
    WITH (text_config='english', tenant_column='tenant_id');

RESET max_parallel_maintenance_workers;

-- Verify row counts per tenant
SELECT tenant_id, COUNT(*) AS doc_count
FROM tenant_bench
GROUP BY tenant_id
ORDER BY tenant_id;

-- ============================================================
-- SECTION 3: Verify tenant-aware filtering correctness
-- ============================================================

-- "database" appears in tenants 1, 2, 3 but NOT tenant 4

-- Tenant 1: should return only tenant 1 docs
SELECT COUNT(*) AS t1_database_hits FROM tenant_bench
WHERE content <@> to_bm25query('database', 'tenant_bench_idx') < 0
    AND tenant_id = 1;

-- Tenant 2: count hits (medium tenant, ~2500 matching docs)
SELECT COUNT(*) AS t2_database_hits FROM tenant_bench
WHERE content <@> to_bm25query('database', 'tenant_bench_idx') < 0
    AND tenant_id = 2;

-- Tenant 3: count hits (small tenant, ~333 matching docs)
SELECT COUNT(*) AS t3_database_hits FROM tenant_bench
WHERE content <@> to_bm25query('database', 'tenant_bench_idx') < 0
    AND tenant_id = 3;

-- Tenant 4: no docs contain "database"
SELECT COUNT(*) AS t4_database_hits FROM tenant_bench
WHERE content <@> to_bm25query('database', 'tenant_bench_idx') < 0
    AND tenant_id = 4;

-- ============================================================
-- SECTION 4: Verify query plan uses index scan
-- ============================================================

SET enable_seqscan = off;

-- Tenant-aware query should use index scan with no rows removed
EXPLAIN (COSTS OFF)
SELECT id FROM tenant_bench
WHERE content <@> to_bm25query('database', 'tenant_bench_idx') < 0
    AND tenant_id = 2
ORDER BY content <@> to_bm25query('database', 'tenant_bench_idx')
LIMIT 10;

RESET enable_seqscan;

-- ============================================================
-- SECTION 5: Per-tenant scores differ from global scores
-- ============================================================
-- "database" is common in tenant 1 (20k/100k = 20%) but tenant 4
-- has no "database" docs. Per-tenant IDF for "database" within
-- tenant 1 is lower than global IDF because the term is common
-- in tenant 1 relative to its size.

-- Global top-3 for "search"
SELECT id, tenant_id,
    ROUND((content <@> to_bm25query('search', 'tenant_bench_idx'))::numeric, 4) AS score
FROM tenant_bench
WHERE content <@> to_bm25query('search', 'tenant_bench_idx') < 0
ORDER BY content <@> to_bm25query('search', 'tenant_bench_idx'), id
LIMIT 3;

-- Tenant 1 top-3 for "search" (per-tenant stats: N=100k)
SELECT id, tenant_id,
    ROUND((content <@> to_bm25query('search', 'tenant_bench_idx'))::numeric, 4) AS score
FROM tenant_bench
WHERE content <@> to_bm25query('search', 'tenant_bench_idx') < 0
    AND tenant_id = 1
ORDER BY content <@> to_bm25query('search', 'tenant_bench_idx'), id
LIMIT 3;

-- Tenant 3 top-3 for "search" (per-tenant stats: N=1k)
SELECT id, tenant_id,
    ROUND((content <@> to_bm25query('search', 'tenant_bench_idx'))::numeric, 4) AS score
FROM tenant_bench
WHERE content <@> to_bm25query('search', 'tenant_bench_idx') < 0
    AND tenant_id = 3
ORDER BY content <@> to_bm25query('search', 'tenant_bench_idx'), id
LIMIT 3;

-- ============================================================
-- SECTION 6: Tiny tenant queries work correctly
-- ============================================================
-- Tenant 4 has only 100 docs - maximum amplification scenario.
-- The tenant-aware index returns exactly what's needed.

SELECT id, tenant_id,
    ROUND((content <@> to_bm25query('rare', 'tenant_bench_idx'))::numeric, 4) AS score
FROM tenant_bench
WHERE content <@> to_bm25query('rare', 'tenant_bench_idx') < 0
    AND tenant_id = 4
ORDER BY content <@> to_bm25query('rare', 'tenant_bench_idx'), id
LIMIT 5;

-- "rare" only exists in tenant 4, so global = tenant results
SELECT COUNT(*) AS global_rare_hits FROM tenant_bench
WHERE content <@> to_bm25query('rare', 'tenant_bench_idx') < 0;

SELECT COUNT(*) AS t4_rare_hits FROM tenant_bench
WHERE content <@> to_bm25query('rare', 'tenant_bench_idx') < 0
    AND tenant_id = 4;

-- ============================================================
-- SECTION 7: Multi-term queries with tenant filtering
-- ============================================================

-- Multi-term query scoped to tenant 2
SELECT id, tenant_id,
    ROUND((content <@> to_bm25query('search relevance scoring', 'tenant_bench_idx'))::numeric, 4) AS score
FROM tenant_bench
WHERE content <@> to_bm25query('search relevance scoring', 'tenant_bench_idx') < 0
    AND tenant_id = 2
ORDER BY content <@> to_bm25query('search relevance scoring', 'tenant_bench_idx'), id
LIMIT 5;

-- Same query, global
SELECT id, tenant_id,
    ROUND((content <@> to_bm25query('search relevance scoring', 'tenant_bench_idx'))::numeric, 4) AS score
FROM tenant_bench
WHERE content <@> to_bm25query('search relevance scoring', 'tenant_bench_idx') < 0
ORDER BY content <@> to_bm25query('search relevance scoring', 'tenant_bench_idx'), id
LIMIT 5;

-- ============================================================
-- SECTION 8: Cross-tenant term distribution
-- ============================================================
-- Verify that terms shared across tenants return correct counts

-- "machine" appears in tenants 1 and 2
SELECT tenant_id, COUNT(*) AS hits
FROM tenant_bench
WHERE content <@> to_bm25query('machine', 'tenant_bench_idx') < 0
GROUP BY tenant_id
ORDER BY tenant_id;

-- "cloud" appears in tenants 1 and 3
SELECT tenant_id, COUNT(*) AS hits
FROM tenant_bench
WHERE content <@> to_bm25query('cloud', 'tenant_bench_idx') < 0
GROUP BY tenant_id
ORDER BY tenant_id;

-- ============================================================
-- Cleanup
-- ============================================================
DROP TABLE tenant_bench CASCADE;
DROP EXTENSION pg_textsearch CASCADE;
