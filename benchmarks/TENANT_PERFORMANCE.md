# Multi-Tenant Performance: Tenant-Aware vs. Post-Filter Indexing

## Overview

pg_textsearch supports two approaches for multi-tenant full-text search:

1. **Post-filter (naive)**: Create a standard BM25 index. Add
   `WHERE tenant_id = X` to queries. The index returns all matching documents
   across all tenants; the executor discards non-matching rows.

2. **Tenant-aware**: Create a BM25 index with `tenant_column='tenant_id'`.
   The planner injects the tenant predicate into the index scan. The index
   filters at the posting level and uses per-tenant BM25 statistics.

The tenant-aware approach is faster (less work amplification) and produces
more accurate BM25 scores (per-tenant IDF and average document length).

## How Tenant-Aware Indexing Works

### Index Creation

Specify the tenant column when creating the index:

```sql
CREATE INDEX idx ON docs USING bm25(content)
    WITH (text_config='english', tenant_column='tenant_id');
```

The `tenant_column` must reference an integer column on the same table.

### Planner Injection

When the planner sees a query like:

```sql
SELECT * FROM docs
WHERE content <@> 'search terms'::bm25query AND tenant_id = 42
ORDER BY content <@> 'search terms'::bm25query
LIMIT 10;
```

It detects the equality predicate on the tenant column and injects
`tenant_id = 42` into the `bm25query` constant. The index scan receives
the tenant ID and filters internally.

### Posting-Level Filtering

During the index scan, each posting entry carries a tenant ID. The scan
skips entries that don't match the requested tenant before scoring them.
This means the index never scores documents that will be discarded later.

### Per-Tenant Statistics

With tenant-aware indexing, BM25 scoring uses statistics scoped to the
tenant:

| Statistic | Global (post-filter) | Per-tenant |
|-----------|---------------------|------------|
| `N` (total docs) | All documents in the index | Only the tenant's documents |
| `avgdl` (avg doc length) | Average across all tenants | Average within the tenant |
| `df` (document frequency) | How many docs globally contain the term | How many tenant docs contain the term |

## Performance Comparison

### Work Amplification

The naive approach suffers from work amplification: the index must return
enough results to satisfy the LIMIT after post-filtering.

Consider a table with 1M documents, tenant A has 1,000 docs (0.1%
selectivity), and you want the top 10 results:

| Approach | Documents scored by index | Documents returned to executor | Waste |
|----------|--------------------------|-------------------------------|-------|
| Post-filter | ~10,000 (10 / 0.001) | 10,000 (executor keeps 10) | 9,990 |
| Tenant-aware | ~10 | 10 | 0 |

The amplification factor is `1 / selectivity`. For a tenant that owns
0.1% of the data, the index does 1000x more work than necessary.

With BMW optimization enabled (default), the savings compound: tenant-aware
scans skip more blocks because they only track scores for the tenant's
documents.

### When It Matters Most

The performance gap is largest when:
- **Small tenants + small LIMIT**: A tenant with 100 docs in a 10M-doc
  index requesting top-10 results
- **Many tenants**: More tenants means smaller average tenant selectivity
- **Skewed distributions**: If one tenant has most documents, smaller
  tenants pay the amplification cost

### When It Matters Least

- Single-tenant tables (no filtering needed)
- Queries without LIMIT (must scan everything anyway)
- Tenants that own most of the data (selectivity near 1.0)

## BM25 Score Accuracy

### Why Global Stats Give Wrong Scores

BM25 uses three corpus statistics: total documents (N), average document
length (avgdl), and document frequency (df). When tenants share an index,
global stats distort scoring for individual tenants.

**Example**: A table with two tenants:
- Tenant A: 100 short product descriptions (avg 5 words)
- Tenant B: 10,000 long research papers (avg 500 words)

Global stats: N=10,100, avgdl=490 words.

When querying tenant A, BM25 thinks documents are abnormally short
(5 vs 490 avgdl), boosting length normalization incorrectly. The IDF
is also wrong because df reflects term popularity across all tenants.

**Per-tenant stats fix this**: Tenant A sees N=100, avgdl=5, and gets
accurate scores as if it had its own dedicated index.

### Concrete Scoring Difference

```sql
-- Setup: tenant 1 has 3 short docs, tenant 2 has 100 long docs
CREATE TABLE demo (id SERIAL PRIMARY KEY, content TEXT, tenant_id INT);
CREATE INDEX demo_idx ON demo USING bm25(content)
    WITH (text_config='english', tenant_column='tenant_id');

INSERT INTO demo (content, tenant_id) VALUES
    ('search engine', 1),
    ('search results', 1),
    ('web browser', 1);
INSERT INTO demo (content, tenant_id)
SELECT 'alpha beta gamma delta epsilon zeta eta theta iota'
    || ' kappa lambda mu document ' || i, 2
FROM generate_series(1, 100) i;

-- Per-tenant score: uses N=3, avgdl=2, df=2
SELECT id, content <@> 'search'::bm25query AS score
FROM demo
WHERE content <@> 'search'::bm25query < 0 AND tenant_id = 1
ORDER BY content <@> 'search'::bm25query;

-- Global score: uses N=103, avgdl~13, df=2
-- Same documents get very different (more negative) scores
SELECT id, content <@> 'search'::bm25query AS score
FROM demo
WHERE content <@> 'search'::bm25query < 0
ORDER BY content <@> 'search'::bm25query;
```

The global query gives "search" a much higher IDF (2/103 vs 2/3), producing
inflated scores. For ranking within a single tenant, this distortion can
change the relative order of results.

## How to Measure

### EXPLAIN ANALYZE

Compare execution plans with and without tenant-aware indexing:

```sql
-- Tenant-aware: index filters internally
EXPLAIN ANALYZE
SELECT id FROM docs
WHERE content <@> 'search terms'::bm25query AND tenant_id = 42
ORDER BY content <@> 'search terms'::bm25query
LIMIT 10;

-- Look for:
-- Index Scan using docs_idx on docs
--   Order By: (content <@> 'search terms'::bm25query)
--   Filter: (tenant_id = 42)
-- Rows Removed by Filter: 0  <-- tenant-aware: no post-filtering
```

Without tenant-aware indexing, you'll see `Rows Removed by Filter: N`
where N can be very large for small tenants.

### BMW Stats Logging

Enable Block-Max WAND statistics to see how many documents and blocks
are processed:

```sql
SET pg_textsearch.log_bmw_stats = true;

-- Run your query, then check the Postgres log for output like:
-- BMW stats: memtable=50 docs, segments=100 docs
--   (blocks: 8 scanned, 12 skipped, 60.0% skip),
--   seeks=4, results=10
```

With tenant-aware indexing, you'll see fewer documents scored and a
higher block skip percentage.

### Score Comparison

To verify per-tenant statistics are being used, compare scores for the
same document with and without tenant filtering:

```sql
-- Score with per-tenant stats
SELECT id, content <@> 'query'::bm25query AS tenant_score
FROM docs
WHERE content <@> 'query'::bm25query < 0 AND tenant_id = 1
ORDER BY content <@> 'query'::bm25query
LIMIT 5;

-- Score with global stats (no tenant filter)
SELECT id, content <@> 'query'::bm25query AS global_score
FROM docs
WHERE content <@> 'query'::bm25query < 0
ORDER BY content <@> 'query'::bm25query
LIMIT 5;
```

If per-tenant stats are working, the same document will have different
scores in the two queries (because N, avgdl, and df differ).

### Benchmarking with Varying Tenant Sizes

To measure the performance impact on your data:

1. Create a table with skewed tenant distribution:

```sql
CREATE TABLE bench (
    id SERIAL PRIMARY KEY,
    content TEXT,
    tenant_id INTEGER NOT NULL
);

-- Large tenant: 100,000 docs
INSERT INTO bench (content, tenant_id)
SELECT 'document about topic ' || (i % 500) || ' with term '
    || (i % 1000), 1
FROM generate_series(1, 100000) i;

-- Small tenants: 100 docs each
INSERT INTO bench (content, tenant_id)
SELECT 'document about topic ' || (i % 50), t
FROM generate_series(1, 100) i,
     generate_series(2, 100) t;
```

2. Create a tenant-aware index and benchmark queries at different tenant
   sizes. The speedup for small tenants (high amplification) will be
   significantly larger than for the large tenant.

3. Use `\timing` in psql and run each query multiple times to get stable
   measurements.
