-- Multi-tenant support tests for pg_textsearch
-- Tests tenant_column index option and tenant-isolated queries

-- Setup
CREATE EXTENSION pg_textsearch;

-- ============================================================
-- SECTION 1: tenant_column reloption parsing
-- ============================================================

-- Test: Create index with tenant_column option
CREATE TABLE tenant_basic (
    id SERIAL PRIMARY KEY,
    content TEXT,
    tenant_id INTEGER NOT NULL
);

CREATE INDEX tenant_basic_idx ON tenant_basic USING bm25(content)
    WITH (text_config='english', tenant_column='tenant_id');

-- Verify index was created with the option
SELECT indexrelid::regclass FROM pg_index
WHERE indrelid = 'tenant_basic'::regclass;

-- Verify reloption is stored
SELECT unnest(reloptions) FROM pg_class
WHERE relname = 'tenant_basic_idx';

DROP TABLE tenant_basic CASCADE;

-- ============================================================
-- SECTION 2: Basic insert and query with tenant-enabled index
-- ============================================================

CREATE TABLE tenant_docs (
    id SERIAL PRIMARY KEY,
    content TEXT,
    tenant_id INTEGER NOT NULL
);

CREATE INDEX tenant_docs_idx ON tenant_docs USING bm25(content)
    WITH (text_config='english', tenant_column='tenant_id');

-- Insert documents for different tenants
INSERT INTO tenant_docs (content, tenant_id) VALUES
    ('database design principles', 1),
    ('database query optimization', 1),
    ('database normalization forms', 1),
    ('web application framework', 2),
    ('web service architecture', 2),
    ('web server configuration', 2),
    ('machine learning algorithms', 3),
    ('machine learning models', 3),
    ('machine learning training', 3);

-- Test: Basic query without tenant filter should return all matches
SELECT id, content <@> 'database'::bm25query as score
FROM tenant_docs
WHERE content <@> 'database'::bm25query < 0
ORDER BY score LIMIT 10;

-- Test: Query for a term that spans multiple tenants
SELECT id, content <@> 'machine'::bm25query as score
FROM tenant_docs
WHERE content <@> 'machine'::bm25query < 0
ORDER BY score LIMIT 10;

DROP TABLE tenant_docs CASCADE;

-- ============================================================
-- SECTION 3: Tenant-enabled index with segment spill
-- ============================================================

CREATE TABLE tenant_segment (
    id SERIAL PRIMARY KEY,
    content TEXT,
    tenant_id INTEGER NOT NULL
);

SET pg_textsearch.index_memory_limit = '64kB';

-- Insert enough data to force a segment spill
INSERT INTO tenant_segment (content, tenant_id)
SELECT
    CASE (i % 3)
        WHEN 0 THEN 'alpha beta gamma document ' || i
        WHEN 1 THEN 'delta epsilon zeta document ' || i
        WHEN 2 THEN 'alpha delta document ' || i
    END,
    (i % 5) + 1  -- tenant_ids 1-5
FROM generate_series(1, 500) i;

CREATE INDEX tenant_segment_idx ON tenant_segment USING bm25(content)
    WITH (text_config='english', tenant_column='tenant_id');

RESET pg_textsearch.index_memory_limit;

-- Test: Query should work correctly after segment spill
SELECT COUNT(*) FROM tenant_segment
WHERE content <@> to_bm25query('alpha', 'tenant_segment_idx') < 0;

-- Test: Results should include docs from multiple tenants
SELECT id, content <@> 'alpha'::bm25query as score
FROM tenant_segment
WHERE content <@> 'alpha'::bm25query < 0
ORDER BY score LIMIT 10;

DROP TABLE tenant_segment CASCADE;

-- ============================================================
-- SECTION 4: Index without tenant_column still works
-- ============================================================

CREATE TABLE tenant_none (
    id SERIAL PRIMARY KEY,
    content TEXT,
    tenant_id INTEGER NOT NULL
);

-- No tenant_column specified - should work as before
CREATE INDEX tenant_none_idx ON tenant_none USING bm25(content)
    WITH (text_config='english');

INSERT INTO tenant_none (content, tenant_id) VALUES
    ('search engine optimization', 1),
    ('search engine ranking', 2),
    ('search engine indexing', 3);

SELECT id, content <@> 'search'::bm25query as score
FROM tenant_none
WHERE content <@> 'search'::bm25query < 0
ORDER BY score LIMIT 10;

DROP TABLE tenant_none CASCADE;

-- ============================================================
-- Cleanup
-- ============================================================
DROP EXTENSION pg_textsearch CASCADE;
