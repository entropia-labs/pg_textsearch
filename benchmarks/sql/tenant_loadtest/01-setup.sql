\set ON_ERROR_STOP on
\timing on

\echo '=== Multi-Tenant Load Test: Setup Phase ==='
\echo '============================================='

-- Clean slate
DROP TABLE IF EXISTS tenant_docs CASCADE;

-- Schema
CREATE TABLE tenant_docs (
    id BIGSERIAL PRIMARY KEY,
    doc_id INTEGER NOT NULL,
    chunk_id INTEGER NOT NULL,
    content TEXT NOT NULL,
    tenant_id INTEGER NOT NULL
);

\echo ''
\echo '--- Step 1: Content generation function ---'

CREATE OR REPLACE FUNCTION generate_content(seed int)
RETURNS text AS $$
DECLARE
    words text[];
    result text := '';
    idx int;
BEGIN
    words := ARRAY[
        -- Medical (50 words)
        'diagnosis', 'treatment', 'patient', 'clinical', 'therapy',
        'symptom', 'pathology', 'surgical', 'chronic', 'acute',
        'oncology', 'cardiac', 'respiratory', 'neurological',
        'pharmaceutical', 'dosage', 'prognosis', 'remission',
        'biopsy', 'radiology', 'anesthesia', 'orthopedic',
        'pediatric', 'obstetric', 'dermatology', 'immunology',
        'hematology', 'endocrine', 'gastric', 'renal',
        'hepatic', 'pulmonary', 'vascular', 'metabolic',
        'infectious', 'autoimmune', 'congenital', 'malignant',
        'benign', 'bilateral', 'intravenous', 'subcutaneous',
        'antibiotic', 'analgesic', 'sedative', 'vaccine',
        'triage', 'ventilator', 'transfusion', 'rehabilitation',
        -- Technology (50 words)
        'algorithm', 'database', 'network', 'protocol', 'server',
        'encryption', 'bandwidth', 'latency', 'throughput',
        'architecture', 'microservice', 'container', 'kubernetes',
        'virtualization', 'middleware', 'firmware', 'compiler',
        'runtime', 'bytecode', 'executable', 'interface',
        'abstraction', 'polymorphism', 'inheritance', 'recursion',
        'concurrency', 'parallelism', 'synchronization', 'mutex',
        'semaphore', 'deadlock', 'pipeline', 'cache', 'buffer',
        'register', 'interrupt', 'kernel', 'syscall', 'daemon',
        'socket', 'packet', 'router', 'firewall', 'proxy',
        'gateway', 'cluster', 'replica', 'shard', 'partition',
        'checkpoint',
        -- Finance (50 words)
        'portfolio', 'dividend', 'equity', 'derivative',
        'commodity', 'futures', 'options', 'hedge', 'leverage',
        'arbitrage', 'liquidity', 'solvency', 'amortization',
        'depreciation', 'valuation', 'capitalization', 'yield',
        'maturity', 'coupon', 'principal', 'collateral',
        'underwriting', 'securities', 'treasury', 'fiscal',
        'monetary', 'inflation', 'deflation', 'recession',
        'surplus', 'deficit', 'revenue', 'expenditure', 'margin',
        'premium', 'deductible', 'annuity', 'fiduciary',
        'benchmark', 'volatility', 'correlation', 'regression',
        'stochastic', 'actuarial', 'insolvency', 'acquisition',
        'merger', 'divestiture', 'syndicate', 'prospectus',
        -- Legal (50 words)
        'jurisdiction', 'statute', 'precedent', 'litigation',
        'arbitration', 'mediation', 'plaintiff', 'defendant',
        'prosecution', 'indictment', 'subpoena', 'deposition',
        'testimony', 'verdict', 'acquittal', 'sentencing',
        'probation', 'parole', 'injunction', 'restraining',
        'fiduciary', 'negligence', 'liability', 'tort',
        'breach', 'remedy', 'damages', 'restitution',
        'compliance', 'regulatory', 'statutory', 'constitutional',
        'amendment', 'ratification', 'jurisprudence', 'appellate',
        'magistrate', 'tribunal', 'counsel', 'attorney',
        'solicitor', 'barrister', 'notary', 'affidavit',
        'covenant', 'easement', 'lien', 'encumbrance',
        'conveyance', 'adjudication',
        -- Education (50 words)
        'curriculum', 'pedagogy', 'syllabus', 'assessment',
        'evaluation', 'accreditation', 'enrollment', 'tuition',
        'scholarship', 'fellowship', 'dissertation', 'thesis',
        'seminar', 'lecture', 'tutorial', 'practicum',
        'internship', 'residency', 'certification', 'diploma',
        'transcript', 'prerequisite', 'elective', 'compulsory',
        'remedial', 'gifted', 'inclusive', 'bilingual',
        'montessori', 'waldorf', 'constructivist', 'behaviorist',
        'cognitivist', 'metacognition', 'scaffolding', 'rubric',
        'formative', 'summative', 'standardized', 'diagnostic',
        'literacy', 'numeracy', 'competency', 'proficiency',
        'matriculation', 'commencement', 'valedictorian',
        'salutatorian', 'tenure', 'sabbatical',
        -- Science (50 words)
        'hypothesis', 'experiment', 'observation', 'variable',
        'constant', 'replication', 'methodology', 'empirical',
        'theoretical', 'quantitative', 'qualitative', 'specimen',
        'catalyst', 'reagent', 'isotope', 'molecule',
        'compound', 'element', 'photosynthesis', 'mitosis',
        'meiosis', 'genome', 'chromosome', 'mutation',
        'evolution', 'adaptation', 'ecosystem', 'biodiversity',
        'taxonomy', 'phylogenetic', 'entropy', 'thermodynamic',
        'kinetic', 'potential', 'electromagnetic', 'quantum',
        'relativity', 'particle', 'neutron', 'proton',
        'electron', 'orbital', 'covalent', 'ionic',
        'oxidation', 'reduction', 'equilibrium', 'diffusion',
        'osmosis', 'centrifugal',
        -- Engineering (50 words)
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
        -- Agriculture (50 words)
        'irrigation', 'fertilizer', 'pesticide', 'herbicide',
        'germination', 'pollination', 'cultivation', 'harvest',
        'rotation', 'fallow', 'compost', 'mulch',
        'aquaculture', 'hydroponics', 'aeroponics', 'permaculture',
        'monoculture', 'polyculture', 'agroforestry', 'silviculture',
        'viticulture', 'horticulture', 'floriculture', 'sericulture',
        'apiculture', 'livestock', 'poultry', 'husbandry',
        'breeding', 'genetics', 'transgenic', 'hybrid',
        'cultivar', 'rootstock', 'grafting', 'propagation',
        'dormancy', 'vernalization', 'photoperiod', 'senescence',
        'pathogen', 'nematode', 'fungicide', 'insecticide',
        'biological', 'integrated', 'sustainable', 'organic',
        'tillage', 'conservation',
        -- Manufacturing (50 words)
        'assembly', 'automation', 'conveyor', 'inventory',
        'logistics', 'procurement', 'supply', 'warehouse',
        'distribution', 'fulfillment', 'quality', 'inspection',
        'defect', 'tolerance', 'specification', 'compliance',
        'certification', 'standardization', 'optimization',
        'throughput', 'bottleneck', 'downtime', 'maintenance',
        'preventive', 'predictive', 'corrective', 'reliability',
        'availability', 'utilization', 'efficiency', 'lean',
        'kaizen', 'kanban', 'ergonomic', 'safety',
        'hazardous', 'ventilation', 'filtration', 'sterilization',
        'packaging', 'labeling', 'traceability', 'serialization',
        'batch', 'continuous', 'discrete', 'additive',
        'subtractive', 'formative', 'prototype',
        -- Real Estate (50 words)
        'appraisal', 'assessment', 'mortgage', 'refinance',
        'foreclosure', 'escrow', 'closing', 'settlement',
        'conveyance', 'easement', 'encumbrance', 'zoning',
        'variance', 'subdivision', 'annexation', 'eminent',
        'condemnation', 'depreciation', 'appreciation',
        'capitalization', 'amortization', 'occupancy', 'vacancy',
        'leasehold', 'freehold', 'tenancy', 'landlord',
        'commercial', 'residential', 'industrial', 'mixed',
        'condominium', 'cooperative', 'townhouse', 'duplex',
        'brownstone', 'bungalow', 'ranch', 'colonial',
        'contemporary', 'renovation', 'restoration', 'demolition',
        'construction', 'foundation', 'framing', 'insulation',
        'plumbing', 'electrical', 'landscaping'
    ];

    FOR pos IN 1..100 LOOP
        idx := 1 + abs(hashint8(seed::bigint + pos)) % 500;
        IF pos > 1 THEN
            result := result || ' ';
        END IF;
        result := result || words[idx];
    END LOOP;

    RETURN result;
END;
$$ LANGUAGE plpgsql IMMUTABLE;

\echo '--- Step 2: Pre-generate content pool (10,000 strings) ---'

CREATE TEMP TABLE content_pool AS
SELECT i AS pool_id,
       generate_content(i)
           || ' database algorithm optimization' AS content
FROM generate_series(1, 10000) i;

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

\echo '--- Step 4: Document assignments ---'

CREATE TEMP TABLE doc_assignments AS
SELECT row_number() OVER () AS doc_id, tenant_id
FROM tenant_config,
     generate_series(1, num_docs);

SELECT 'Documents assigned:' AS info, count(*) AS total
FROM doc_assignments;

\echo '--- Step 5: Bulk insert ~10M rows (batched) ---'

-- Batch in groups of 40 chunks to avoid WAL pressure
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
          ON cp.pool_id = 1 + (
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

\echo '--- Step 6: ANALYZE ---'
ANALYZE tenant_docs;

\echo '--- Step 7: Create BM25 index ---'

CREATE INDEX tenant_docs_bm25_idx ON tenant_docs
    USING bm25(content)
    WITH (text_config='english', tenant_column='tenant_id');

SELECT 'Index size:' AS info,
       pg_size_pretty(pg_relation_size('tenant_docs_bm25_idx'))
           AS index_size;

\echo '--- Step 8: Spill memtable (skip if parallel build) ---'

-- Parallel build already spills to segments; spill may fail on
-- empty memtable after parallel build, so we tolerate errors here.
DO $$
BEGIN
    PERFORM bm25_spill_index('tenant_docs_bm25_idx');
    RAISE NOTICE 'Spill completed';
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE 'Spill skipped (parallel build): %', SQLERRM;
END $$;

\echo '--- Step 9: Index summary ---'

SELECT * FROM bm25_summarize_index('tenant_docs_bm25_idx');

\echo ''
\echo '=== Setup complete ==='
