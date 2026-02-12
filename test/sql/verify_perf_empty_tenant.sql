-- Verification: 4 tenants x 1M docs + 1 empty tenant
\timing on

CREATE EXTENSION IF NOT EXISTS pg_textsearch;
DROP TABLE IF EXISTS verify_tenant CASCADE;

CREATE TABLE verify_tenant (
    id SERIAL PRIMARY KEY,
    content TEXT,
    tenant_id INTEGER NOT NULL
);

-- Tenant 1: 1M docs
INSERT INTO verify_tenant (content, tenant_id)
SELECT
    CASE (i % 5)
        WHEN 0 THEN 'database query optimization performance'
        WHEN 1 THEN 'search engine ranking algorithm design'
        WHEN 2 THEN 'machine learning neural network training'
        WHEN 3 THEN 'distributed systems cloud computing scale'
        WHEN 4 THEN 'natural language processing text analysis'
    END || ' document ' || i,
    1
FROM generate_series(1, 1000000) i;

-- Tenant 2: 1M docs
INSERT INTO verify_tenant (content, tenant_id)
SELECT
    CASE (i % 5)
        WHEN 0 THEN 'database indexing storage management'
        WHEN 1 THEN 'search relevance scoring retrieval'
        WHEN 2 THEN 'machine learning model evaluation'
        WHEN 3 THEN 'web application framework development'
        WHEN 4 THEN 'cloud infrastructure monitoring alerting'
    END || ' document ' || i,
    2
FROM generate_series(1, 1000000) i;

-- Tenant 3: 1M docs
INSERT INTO verify_tenant (content, tenant_id)
SELECT
    CASE (i % 4)
        WHEN 0 THEN 'database administration backup recovery'
        WHEN 1 THEN 'search engine optimization strategy'
        WHEN 2 THEN 'cloud infrastructure monitoring alerting'
        WHEN 3 THEN 'network security firewall protection'
    END || ' document ' || i,
    3
FROM generate_series(1, 1000000) i;

-- Tenant 4: 1M docs
INSERT INTO verify_tenant (content, tenant_id)
SELECT
    CASE (i % 4)
        WHEN 0 THEN 'database performance tuning optimization'
        WHEN 1 THEN 'search index building inverted lists'
        WHEN 2 THEN 'machine learning deep reinforcement'
        WHEN 3 THEN 'distributed consensus raft protocol'
    END || ' document ' || i,
    4
FROM generate_series(1, 1000000) i;

-- Tenant 5: EMPTY (0 docs)

SELECT tenant_id, COUNT(*) AS doc_count
FROM verify_tenant
GROUP BY tenant_id
ORDER BY tenant_id;

SET max_parallel_maintenance_workers = 0;
ANALYZE verify_tenant;

CREATE INDEX verify_tenant_idx ON verify_tenant
    USING bm25(content)
    WITH (text_config='english', tenant_column='tenant_id');

RESET max_parallel_maintenance_workers;

SET pg_textsearch.log_bmw_stats = on;
SET enable_seqscan = off;

-- TEST 1: Empty tenant, single term
\echo '=== TEST 1: Empty tenant (5), single term "database" ==='
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM verify_tenant
WHERE content <@> to_bm25query('database', 'verify_tenant_idx') < 0
    AND tenant_id = 5
ORDER BY content <@> to_bm25query('database', 'verify_tenant_idx')
LIMIT 10;

-- TEST 2: Empty tenant, multi-term
\echo '=== TEST 2: Empty tenant (5), multi-term "database search machine" ==='
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM verify_tenant
WHERE content <@> to_bm25query('database search machine', 'verify_tenant_idx') < 0
    AND tenant_id = 5
ORDER BY content <@> to_bm25query('database search machine', 'verify_tenant_idx')
LIMIT 10;

-- TEST 3: Populated tenant for comparison
\echo '=== TEST 3: Populated tenant (1), single term "database" ==='
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM verify_tenant
WHERE content <@> to_bm25query('database', 'verify_tenant_idx') < 0
    AND tenant_id = 1
ORDER BY content <@> to_bm25query('database', 'verify_tenant_idx')
LIMIT 10;

-- TEST 4: Correctness
\echo '=== TEST 4: Correctness ==='
SELECT COUNT(*) AS empty_tenant_hits FROM verify_tenant
WHERE content <@> to_bm25query('database', 'verify_tenant_idx') < 0
    AND tenant_id = 5;

SELECT COUNT(*) AS tenant1_hits FROM verify_tenant
WHERE content <@> to_bm25query('database', 'verify_tenant_idx') < 0
    AND tenant_id = 1;

-- TEST 5: Term exists only in tenant 3, query tenant 1
\echo '=== TEST 5: "firewall" only in tenant 3, query tenant 1 ==='
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM verify_tenant
WHERE content <@> to_bm25query('firewall', 'verify_tenant_idx') < 0
    AND tenant_id = 1
ORDER BY content <@> to_bm25query('firewall', 'verify_tenant_idx')
LIMIT 10;

\echo '=== TEST 5b: "firewall" in its actual tenant 3 ==='
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM verify_tenant
WHERE content <@> to_bm25query('firewall', 'verify_tenant_idx') < 0
    AND tenant_id = 3
ORDER BY content <@> to_bm25query('firewall', 'verify_tenant_idx')
LIMIT 10;

RESET pg_textsearch.log_bmw_stats;
RESET enable_seqscan;

DROP TABLE verify_tenant CASCADE;
DROP EXTENSION pg_textsearch CASCADE;
