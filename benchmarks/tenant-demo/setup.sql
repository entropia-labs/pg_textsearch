\set ON_ERROR_STOP on
\timing on

\echo '=== Tenant Demo: Setup ==='
\echo '=========================='

-- Clean slate
DROP TABLE IF EXISTS tenant_docs CASCADE;

CREATE TABLE tenant_docs (
    id BIGSERIAL PRIMARY KEY,
    doc_id INTEGER NOT NULL,
    chunk_id INTEGER NOT NULL,
    content TEXT NOT NULL,
    tenant_id INTEGER NOT NULL
);

-- --------------------------------------------------------
-- Step 1: Domain word lists (10 domains, 50 words each)
-- --------------------------------------------------------
\echo ''
\echo '--- Step 1: Content generation function ---'

CREATE OR REPLACE FUNCTION generate_content(
    seed int, domain_a int, domain_b int
) RETURNS text AS $$
DECLARE
    -- 10 domains x 50 words = 500 words total
    -- domain index 0-9 maps to words[(d*50+1)..(d*50+50)]
    words text[];
    result text := '';
    idx int;
    domain_offset int;
BEGIN
    words := ARRAY[
        -- Domain 0: Medical (50 words)
        'diagnosis', 'treatment', 'patient', 'clinical',
        'therapy', 'symptom', 'pathology', 'surgical',
        'chronic', 'acute', 'oncology', 'cardiac',
        'respiratory', 'neurological', 'pharmaceutical',
        'dosage', 'prognosis', 'remission', 'biopsy',
        'radiology', 'anesthesia', 'orthopedic', 'pediatric',
        'obstetric', 'dermatology', 'immunology', 'hematology',
        'endocrine', 'gastric', 'renal', 'hepatic',
        'pulmonary', 'vascular', 'metabolic', 'infectious',
        'autoimmune', 'congenital', 'malignant', 'benign',
        'bilateral', 'intravenous', 'subcutaneous',
        'antibiotic', 'analgesic', 'sedative', 'vaccine',
        'triage', 'ventilator', 'transfusion', 'rehabilitation',
        -- Domain 1: Technology (50 words)
        'algorithm', 'database', 'network', 'protocol',
        'server', 'encryption', 'bandwidth', 'latency',
        'throughput', 'architecture', 'microservice',
        'container', 'kubernetes', 'virtualization',
        'middleware', 'firmware', 'compiler', 'runtime',
        'bytecode', 'executable', 'interface', 'abstraction',
        'polymorphism', 'inheritance', 'recursion',
        'concurrency', 'parallelism', 'synchronization',
        'mutex', 'semaphore', 'deadlock', 'pipeline',
        'cache', 'buffer', 'register', 'interrupt', 'kernel',
        'syscall', 'daemon', 'socket', 'packet', 'router',
        'firewall', 'proxy', 'gateway', 'cluster', 'replica',
        'shard', 'partition', 'checkpoint',
        -- Domain 2: Finance (50 words)
        'portfolio', 'dividend', 'equity', 'derivative',
        'commodity', 'futures', 'options', 'hedge',
        'leverage', 'arbitrage', 'liquidity', 'solvency',
        'amortization', 'depreciation', 'valuation',
        'capitalization', 'yield', 'maturity', 'coupon',
        'principal', 'collateral', 'underwriting',
        'securities', 'treasury', 'fiscal', 'monetary',
        'inflation', 'deflation', 'recession', 'surplus',
        'deficit', 'revenue', 'expenditure', 'margin',
        'premium', 'deductible', 'annuity', 'fiduciary',
        'benchmark', 'volatility', 'correlation', 'regression',
        'stochastic', 'actuarial', 'insolvency', 'acquisition',
        'merger', 'divestiture', 'syndicate', 'prospectus',
        -- Domain 3: Legal (50 words)
        'jurisdiction', 'statute', 'precedent', 'litigation',
        'arbitration', 'mediation', 'plaintiff', 'defendant',
        'prosecution', 'indictment', 'subpoena', 'deposition',
        'testimony', 'verdict', 'acquittal', 'sentencing',
        'probation', 'parole', 'injunction', 'restraining',
        'fiduciary', 'negligence', 'liability', 'tort',
        'breach', 'remedy', 'damages', 'restitution',
        'compliance', 'regulatory', 'statutory',
        'constitutional', 'amendment', 'ratification',
        'jurisprudence', 'appellate', 'magistrate', 'tribunal',
        'counsel', 'attorney', 'solicitor', 'barrister',
        'notary', 'affidavit', 'covenant', 'easement', 'lien',
        'encumbrance', 'conveyance', 'adjudication',
        -- Domain 4: Education (50 words)
        'curriculum', 'pedagogy', 'syllabus', 'assessment',
        'evaluation', 'accreditation', 'enrollment', 'tuition',
        'scholarship', 'fellowship', 'dissertation', 'thesis',
        'seminar', 'lecture', 'tutorial', 'practicum',
        'internship', 'residency', 'certification', 'diploma',
        'transcript', 'prerequisite', 'elective', 'compulsory',
        'remedial', 'gifted', 'inclusive', 'bilingual',
        'montessori', 'waldorf', 'constructivist',
        'behaviorist', 'cognitivist', 'metacognition',
        'scaffolding', 'rubric', 'formative', 'summative',
        'standardized', 'diagnostic', 'literacy', 'numeracy',
        'competency', 'proficiency', 'matriculation',
        'commencement', 'valedictorian', 'salutatorian',
        'tenure', 'sabbatical',
        -- Domain 5: Science (50 words)
        'hypothesis', 'experiment', 'observation', 'variable',
        'constant', 'replication', 'methodology', 'empirical',
        'theoretical', 'quantitative', 'qualitative',
        'specimen', 'catalyst', 'reagent', 'isotope',
        'molecule', 'compound', 'element', 'photosynthesis',
        'mitosis', 'meiosis', 'genome', 'chromosome',
        'mutation', 'evolution', 'adaptation', 'ecosystem',
        'biodiversity', 'taxonomy', 'phylogenetic', 'entropy',
        'thermodynamic', 'kinetic', 'potential',
        'electromagnetic', 'quantum', 'relativity', 'particle',
        'neutron', 'proton', 'electron', 'orbital', 'covalent',
        'ionic', 'oxidation', 'reduction', 'equilibrium',
        'diffusion', 'osmosis', 'centrifugal',
        -- Domain 6: Engineering (50 words)
        'structural', 'mechanical', 'hydraulic', 'pneumatic',
        'thermal', 'aerospace', 'automotive', 'biomedical',
        'chemical', 'civil', 'electrical', 'industrial',
        'manufacturing', 'materials', 'nuclear', 'petroleum',
        'robotics', 'semiconductor', 'telecommunications',
        'turbine', 'combustion', 'propulsion', 'aerodynamic',
        'hydrodynamic', 'geotechnical', 'seismic', 'acoustic',
        'optical', 'photonic', 'nanoscale', 'composite',
        'alloy', 'ceramic', 'polymer', 'elastomer',
        'viscosity', 'tensile', 'fatigue', 'corrosion',
        'welding', 'machining', 'casting', 'forging',
        'extrusion', 'injection', 'calibration', 'tolerance',
        'precision', 'actuator', 'transducer',
        -- Domain 7: Agriculture (50 words)
        'irrigation', 'fertilizer', 'pesticide', 'herbicide',
        'germination', 'pollination', 'cultivation', 'harvest',
        'rotation', 'fallow', 'compost', 'mulch',
        'aquaculture', 'hydroponics', 'aeroponics',
        'permaculture', 'monoculture', 'polyculture',
        'agroforestry', 'silviculture', 'viticulture',
        'horticulture', 'floriculture', 'sericulture',
        'apiculture', 'livestock', 'poultry', 'husbandry',
        'breeding', 'genetics', 'transgenic', 'hybrid',
        'cultivar', 'rootstock', 'grafting', 'propagation',
        'dormancy', 'vernalization', 'photoperiod',
        'senescence', 'pathogen', 'nematode', 'fungicide',
        'insecticide', 'biological', 'integrated',
        'sustainable', 'organic', 'tillage', 'conservation',
        -- Domain 8: Manufacturing (50 words)
        'assembly', 'automation', 'conveyor', 'inventory',
        'logistics', 'procurement', 'supply', 'warehouse',
        'distribution', 'fulfillment', 'quality', 'inspection',
        'defect', 'tolerance', 'specification', 'compliance',
        'certification', 'standardization', 'optimization',
        'throughput', 'bottleneck', 'downtime', 'maintenance',
        'preventive', 'predictive', 'corrective', 'reliability',
        'availability', 'utilization', 'efficiency', 'lean',
        'kaizen', 'kanban', 'ergonomic', 'safety', 'hazardous',
        'ventilation', 'filtration', 'sterilization',
        'packaging', 'labeling', 'traceability',
        'serialization', 'batch', 'continuous', 'discrete',
        'additive', 'subtractive', 'formative', 'prototype',
        -- Domain 9: Real Estate (50 words)
        'appraisal', 'assessment', 'mortgage', 'refinance',
        'foreclosure', 'escrow', 'closing', 'settlement',
        'conveyance', 'easement', 'encumbrance', 'zoning',
        'variance', 'subdivision', 'annexation', 'eminent',
        'condemnation', 'depreciation', 'appreciation',
        'capitalization', 'amortization', 'occupancy',
        'vacancy', 'leasehold', 'freehold', 'tenancy',
        'landlord', 'commercial', 'residential', 'industrial',
        'mixed', 'condominium', 'cooperative', 'townhouse',
        'duplex', 'brownstone', 'bungalow', 'ranch',
        'colonial', 'contemporary', 'renovation', 'restoration',
        'demolition', 'construction', 'foundation', 'framing',
        'insulation', 'plumbing', 'electrical', 'landscaping'
    ];

    FOR pos IN 1..100 LOOP
        -- Alternate between the two domains
        IF pos % 2 = 1 THEN
            domain_offset := domain_a * 50;
        ELSE
            domain_offset := domain_b * 50;
        END IF;
        idx := domain_offset
             + 1 + abs(hashint8(seed::bigint + pos)) % 50;
        IF pos > 1 THEN
            result := result || ' ';
        END IF;
        result := result || words[idx];
    END LOOP;

    RETURN result;
END;
$$ LANGUAGE plpgsql IMMUTABLE;

-- --------------------------------------------------------
-- Step 2: Assign each tenant two random domains (0-9)
-- --------------------------------------------------------
\echo '--- Step 2: Tenant domain assignments ---'

CREATE TEMP TABLE domain_names (
    id int PRIMARY KEY, name text
);
INSERT INTO domain_names VALUES
    (0, 'Medical'), (1, 'Technology'), (2, 'Finance'),
    (3, 'Legal'), (4, 'Education'), (5, 'Science'),
    (6, 'Engineering'), (7, 'Agriculture'),
    (8, 'Manufacturing'), (9, 'Real Estate');

-- Deterministic but varied: use hash of tenant_id to pick domains
CREATE TEMP TABLE tenant_domains AS
SELECT t AS tenant_id,
       abs(hashint4(t)) % 10 AS domain_a,
       abs(hashint4(t * 7 + 13)) % 10 AS domain_b
FROM generate_series(1, :'num_tenants') t;

-- Ensure domains are different; if collision, shift domain_b
UPDATE tenant_domains
SET domain_b = (domain_b + 1) % 10
WHERE domain_a = domain_b;

SELECT td.tenant_id,
       da.name AS domain_1,
       db.name AS domain_2
FROM tenant_domains td
JOIN domain_names da ON da.id = td.domain_a
JOIN domain_names db ON db.id = td.domain_b
ORDER BY td.tenant_id;

-- --------------------------------------------------------
-- Step 3: Tenant row counts
-- --------------------------------------------------------
\echo '--- Step 3: Tenant configuration ---'

CREATE TEMP TABLE tenant_config AS
SELECT t AS tenant_id,
       (80 + (20 * (1 + sin(t * 0.4)))::int)
           * :'docs_multiplier' AS num_docs
FROM generate_series(1, :'num_tenants') t;

SELECT 'Tenant distribution:' AS info,
       min(num_docs) AS min_docs,
       round(avg(num_docs)) AS avg_docs,
       max(num_docs) AS max_docs,
       sum(num_docs) AS total_docs
FROM tenant_config;

-- --------------------------------------------------------
-- Step 4: Document assignments
-- --------------------------------------------------------
\echo '--- Step 4: Document assignments ---'

CREATE TEMP TABLE doc_assignments AS
SELECT row_number() OVER () AS doc_id, tenant_id
FROM tenant_config,
     generate_series(1, num_docs);

SELECT 'Documents assigned:' AS info, count(*) AS total
FROM doc_assignments;

-- --------------------------------------------------------
-- Step 5: Generate content pool per tenant domain pair
-- --------------------------------------------------------
\echo '--- Step 5: Content pool generation ---'

CREATE TEMP TABLE content_pool AS
SELECT td.tenant_id, i AS pool_id,
       generate_content(i, td.domain_a, td.domain_b) AS content
FROM tenant_domains td
CROSS JOIN generate_series(1, 10000) i;

-- --------------------------------------------------------
-- Step 6: Bulk insert
-- --------------------------------------------------------
\echo '--- Step 6: Bulk insert ---'

DO $$
DECLARE
    batch_start int;
    batch_size int := 40;
BEGIN
    FOR batch_start IN 1..200 BY batch_size LOOP
        INSERT INTO tenant_docs
            (doc_id, chunk_id, content, tenant_id)
        SELECT da.doc_id, c.chunk_id, cp.content,
               da.tenant_id
        FROM doc_assignments da
        CROSS JOIN generate_series(
            batch_start,
            least(batch_start + batch_size - 1, 200)
        ) AS c(chunk_id)
        JOIN content_pool cp
          ON cp.tenant_id = da.tenant_id
         AND cp.pool_id = 1 + (
              (da.doc_id * 200 + c.chunk_id) % 10000
          );
        RAISE NOTICE 'Inserted chunks %–%',
            batch_start, batch_start + batch_size - 1;
    END LOOP;
END $$;

SELECT 'Rows inserted:' AS info,
       count(*) AS total_rows,
       count(DISTINCT tenant_id) AS tenants,
       count(DISTINCT doc_id) AS documents
FROM tenant_docs;

SELECT 'Table size:' AS info,
       pg_size_pretty(pg_relation_size('tenant_docs')) AS heap_size;

-- --------------------------------------------------------
-- Step 7: ANALYZE + Index
-- --------------------------------------------------------
\echo '--- Step 7: ANALYZE ---'
ANALYZE tenant_docs;

\echo '--- Step 8: Create BM25 index ---'

CREATE INDEX tenant_docs_bm25_idx ON tenant_docs
    USING bm25(content)
    WITH (text_config='english', tenant_column='tenant_id');

SELECT 'Index size:' AS info,
       pg_size_pretty(pg_relation_size('tenant_docs_bm25_idx'))
           AS index_size;

\echo '--- Step 9: Spill memtable ---'

DO $$
BEGIN
    PERFORM bm25_spill_index('tenant_docs_bm25_idx');
    RAISE NOTICE 'Spill completed';
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE 'Spill skipped (parallel build): %', SQLERRM;
END $$;

\echo '--- Step 10: Index summary ---'

SELECT * FROM bm25_summarize_index('tenant_docs_bm25_idx');

\echo ''
\echo '=== Setup complete ==='
