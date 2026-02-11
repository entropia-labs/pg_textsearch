/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * scoring.rs - BM25 scoring functions
 *
 * Implements the BM25 ranking function components:
 * - IDF (Inverse Document Frequency) calculation
 * - Per-document BM25 score computation
 * - Block max score upper bound for BMW optimization
 */

use crate::fieldnorm::decode_fieldnorm;

/// Calculate IDF (Inverse Document Frequency) for a term.
///
/// Formula: IDF = ln(1 + (N - df + 0.5) / (df + 0.5))
///
/// Where N = total_docs and df = doc_freq.
/// The +0.5 smoothing ensures IDF is always non-negative.
pub fn calculate_idf(doc_freq: i32, total_docs: i32) -> f32 {
    let numerator = (total_docs - doc_freq) as f64 + 0.5;
    let denominator = doc_freq as f64 + 0.5;
    let ratio = numerator / denominator;
    (1.0_f64 + ratio).ln() as f32
}

/// Compute BM25 score for a single document-term pair.
///
/// Formula:
///   score = IDF * (tf * (k1 + 1)) / (tf + k1 * (1 - b + b * dl/avgdl))
///
/// Parameters:
/// - `idf`: pre-computed IDF for the term
/// - `tf`: term frequency in the document
/// - `dl`: document length (word count)
/// - `k1`: term frequency saturation (default 1.2)
/// - `b`: length normalization (default 0.75)
/// - `avgdl`: average document length across the index
pub fn bm25_score(idf: f32, tf: i32, dl: i32, k1: f32, b: f32, avgdl: f32) -> f32 {
    let tf_f = tf as f32;
    let dl_f = dl as f32;
    let len_norm = 1.0 - b + b * (dl_f / avgdl);
    let tf_component = (tf_f * (k1 + 1.0)) / (tf_f + k1 * len_norm);
    idf * tf_component
}

/// Compute block maximum BM25 score from skip entry metadata.
///
/// Uses the block's maximum TF and the fieldnorm of the
/// shortest document to compute a tight upper bound on any
/// document's score in the block. Used by BMW for pruning.
pub fn block_max_score(
    max_tf: u16,
    max_norm: u8,
    idf: f32,
    k1: f32,
    b: f32,
    avgdl: f32,
) -> f32 {
    let tf = max_tf as f32;
    let dl = decode_fieldnorm(max_norm) as f32;
    let len_norm = 1.0 - b + b * (dl / avgdl);
    let tf_component = (tf * (k1 + 1.0)) / (tf + k1 * len_norm);
    idf * tf_component
}

/// Compute per-tenant average document length.
///
/// Returns the average doc length for documents belonging to the
/// specified tenant. Used when multi-tenant indexes need tenant-
/// specific BM25 scoring.
pub fn tenant_avgdl(tenant_total_tokens: u64, tenant_num_docs: u32) -> f32 {
    if tenant_num_docs == 0 {
        return 0.0;
    }
    tenant_total_tokens as f32 / tenant_num_docs as f32
}

/// Compute per-tenant IDF.
///
/// Uses tenant-specific corpus stats instead of global stats:
/// IDF = ln(1 + (N_tenant - df_tenant + 0.5) / (df_tenant + 0.5))
pub fn tenant_idf(tenant_doc_freq: u32, tenant_total_docs: u32) -> f32 {
    calculate_idf(tenant_doc_freq as i32, tenant_total_docs as i32)
}

// --- FFI exports ---

#[no_mangle]
pub extern "C" fn tp_rust_calculate_idf(doc_freq: i32, total_docs: i32) -> f32 {
    calculate_idf(doc_freq, total_docs)
}

#[no_mangle]
pub extern "C" fn tp_rust_bm25_score(
    idf: f32,
    tf: i32,
    dl: i32,
    k1: f32,
    b: f32,
    avgdl: f32,
) -> f32 {
    bm25_score(idf, tf, dl, k1, b, avgdl)
}

#[no_mangle]
pub extern "C" fn tp_rust_block_max_score(
    max_tf: u16,
    max_norm: u8,
    idf: f32,
    k1: f32,
    b: f32,
    avgdl: f32,
) -> f32 {
    block_max_score(max_tf, max_norm, idf, k1, b, avgdl)
}

#[no_mangle]
pub extern "C" fn tp_rust_tenant_avgdl(
    tenant_total_tokens: u64,
    tenant_num_docs: u32,
) -> f32 {
    tenant_avgdl(tenant_total_tokens, tenant_num_docs)
}

#[no_mangle]
pub extern "C" fn tp_rust_tenant_idf(
    tenant_doc_freq: u32,
    tenant_total_docs: u32,
) -> f32 {
    tenant_idf(tenant_doc_freq, tenant_total_docs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idf_basic() {
        // Term in half the documents
        let idf = calculate_idf(50, 100);
        assert!(idf > 0.0, "IDF should be positive");
        // ln(1 + (100-50+0.5)/(50+0.5)) = ln(1 + 50.5/50.5) = ln(2)
        assert!((idf - (2.0_f32).ln()).abs() < 0.001);
    }

    #[test]
    fn test_idf_rare_term() {
        // Very rare term (in 1 of 10000 docs)
        let idf = calculate_idf(1, 10000);
        assert!(idf > 5.0, "Rare term should have high IDF");
    }

    #[test]
    fn test_idf_common_term() {
        // Very common term (in all docs)
        let idf = calculate_idf(100, 100);
        // ln(1 + (100-100+0.5)/(100+0.5)) = ln(1 + 0.5/100.5) ≈ 0.00497
        assert!(idf > 0.0, "IDF should still be positive");
        assert!(idf < 0.01, "Very common term should have very low IDF");
    }

    #[test]
    fn test_idf_always_positive() {
        // IDF should be non-negative for all valid inputs
        for total in [1, 10, 100, 1000, 10000] {
            for df in 1..=total {
                let idf = calculate_idf(df, total);
                assert!(
                    idf >= 0.0,
                    "IDF should be >= 0 for df={}, total={}",
                    df,
                    total
                );
            }
        }
    }

    #[test]
    fn test_bm25_score_basic() {
        let idf = calculate_idf(10, 1000);
        let score = bm25_score(idf, 3, 100, 1.2, 0.75, 200.0);
        assert!(score > 0.0, "BM25 score should be positive");
    }

    #[test]
    fn test_bm25_higher_tf_higher_score() {
        let idf = calculate_idf(10, 1000);
        let score_low = bm25_score(idf, 1, 100, 1.2, 0.75, 100.0);
        let score_high = bm25_score(idf, 5, 100, 1.2, 0.75, 100.0);
        assert!(
            score_high > score_low,
            "Higher TF should give higher score"
        );
    }

    #[test]
    fn test_bm25_tf_saturation() {
        let idf = calculate_idf(10, 1000);
        let score_10 = bm25_score(idf, 10, 100, 1.2, 0.75, 100.0);
        let score_100 = bm25_score(idf, 100, 100, 1.2, 0.75, 100.0);
        // Due to saturation, 10x more TF should NOT give 10x more score
        assert!(
            score_100 / score_10 < 2.0,
            "TF saturation should limit score growth"
        );
    }

    #[test]
    fn test_bm25_length_normalization() {
        let idf = calculate_idf(10, 1000);
        let score_short = bm25_score(idf, 3, 50, 1.2, 0.75, 100.0);
        let score_long = bm25_score(idf, 3, 500, 1.2, 0.75, 100.0);
        assert!(
            score_short > score_long,
            "Shorter doc should score higher (same TF)"
        );
    }

    #[test]
    fn test_tenant_avgdl() {
        assert_eq!(tenant_avgdl(1000, 10), 100.0);
        assert_eq!(tenant_avgdl(0, 0), 0.0);
        assert_eq!(tenant_avgdl(500, 5), 100.0);
    }

    #[test]
    fn test_tenant_scoring_differs_from_global() {
        // Tenant A: 100 short docs (avgdl=50)
        // Tenant B: 1000 long docs (avgdl=500)
        // Global: 1100 docs, avgdl ≈ 459
        let global_avgdl = tenant_avgdl(100 * 50 + 1000 * 500, 1100);
        let tenant_a_avgdl = tenant_avgdl(100 * 50, 100);
        let tenant_b_avgdl = tenant_avgdl(1000 * 500, 1000);

        // Same doc, same TF, but scores differ based on corpus
        let idf = calculate_idf(10, 1100);
        let score_global = bm25_score(idf, 3, 50, 1.2, 0.75, global_avgdl);
        let score_tenant_a =
            bm25_score(idf, 3, 50, 1.2, 0.75, tenant_a_avgdl);

        // With tenant A's short avgdl, a 50-word doc is average,
        // but with global avgdl ~459, it's much shorter than average
        // so global score should be higher (shorter = better)
        assert!(
            score_global > score_tenant_a,
            "Global score {} should differ from tenant score {}",
            score_global,
            score_tenant_a
        );

        // Verify tenant B avgdl is much larger
        assert!(tenant_b_avgdl > tenant_a_avgdl * 5.0);
    }

    #[test]
    fn test_block_max_score_upper_bound() {
        let idf = calculate_idf(10, 1000);
        let k1 = 1.2_f32;
        let b = 0.75_f32;
        let avgdl = 100.0_f32;

        // Block with max_tf=5, max_norm for length=50
        let max_tf: u16 = 5;
        let max_norm = crate::fieldnorm::encode_fieldnorm(50);
        let block_max = block_max_score(max_tf, max_norm, idf, k1, b, avgdl);

        // Any individual document in this block should score <= block_max
        let doc_score = bm25_score(idf, 3, 80, k1, b, avgdl);
        assert!(
            doc_score <= block_max,
            "Individual doc score {} should be <= block max {}",
            doc_score,
            block_max
        );
    }
}
