/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * merge.rs - Segment compaction/merge logic
 *
 * Implements N-way merge of segment data. The merge combines
 * multiple segments into one by merging their sorted posting
 * lists term by term.
 *
 * The C side orchestrates segment opens/closes and page I/O.
 * Rust handles the merge algorithm on byte-level data.
 */

use crate::segment::format::{
    PostingEntry, TpTenantDocFreq, TpTenantStats,
};
use std::collections::BTreeMap;

/// Merge posting lists from multiple sources for a single term.
///
/// Each source provides a sorted (by doc_id) posting list.
/// The merge produces a single sorted list.
///
/// `doc_id_maps[i]` maps old doc_ids from source i to new doc_ids
/// in the merged segment. Pass None if doc_ids don't need remapping.
pub fn merge_postings(
    sources: &[&[PostingEntry]],
    doc_id_maps: Option<&[&[u32]]>,
) -> Vec<PostingEntry> {
    let total: usize = sources.iter().map(|s| s.len()).sum();
    let mut merged = Vec::with_capacity(total);

    // Collect all postings with remapped doc_ids
    for (src_idx, source) in sources.iter().enumerate() {
        for posting in *source {
            let new_doc_id = if let Some(maps) = doc_id_maps {
                maps[src_idx][posting.doc_id as usize]
            } else {
                posting.doc_id
            };

            merged.push(PostingEntry {
                doc_id: new_doc_id,
                frequency: posting.frequency,
                fieldnorm: posting.fieldnorm,
                tenant_id: posting.tenant_id,
            });
        }
    }

    // Sort by doc_id for the merged segment
    merged.sort_by_key(|p| p.doc_id);
    merged
}

/// N-way merge of sorted term lists from multiple segments.
///
/// Returns a merged sorted list of unique terms with their
/// combined posting lists.
pub fn merge_term_lists(
    sources: &[Vec<(String, Vec<PostingEntry>)>],
    doc_id_maps: Option<&[&[u32]]>,
) -> Vec<(String, Vec<PostingEntry>)> {
    // Collect all unique terms
    let mut all_terms: Vec<String> = sources
        .iter()
        .flat_map(|src| src.iter().map(|(term, _)| term.clone()))
        .collect();
    all_terms.sort();
    all_terms.dedup();

    let mut result = Vec::with_capacity(all_terms.len());

    for term in &all_terms {
        let mut term_sources: Vec<&[PostingEntry]> = Vec::new();
        let mut source_indices: Vec<usize> = Vec::new();

        for (src_idx, source) in sources.iter().enumerate() {
            if let Some((_, postings)) =
                source.iter().find(|(t, _)| t == term)
            {
                term_sources.push(postings);
                source_indices.push(src_idx);
            }
        }

        if term_sources.is_empty() {
            continue;
        }

        let merged = if let Some(maps) = doc_id_maps {
            let src_maps: Vec<&[u32]> = source_indices
                .iter()
                .map(|&idx| maps[idx])
                .collect();
            merge_postings(&term_sources, Some(&src_maps))
        } else {
            merge_postings(&term_sources, None)
        };

        result.push((term.clone(), merged));
    }

    result
}

/// Merge posting lists with tenant-aware sorting.
///
/// Like `merge_postings` but sorts by (tenant_id, doc_id) instead
/// of just doc_id. This naturally groups postings by tenant,
/// creating single-tenant blocks that enable O(1) skip in BMW.
pub fn merge_postings_tenant(
    sources: &[&[PostingEntry]],
    doc_id_maps: Option<&[&[u32]]>,
) -> Vec<PostingEntry> {
    let total: usize = sources.iter().map(|s| s.len()).sum();
    let mut merged = Vec::with_capacity(total);

    for (src_idx, source) in sources.iter().enumerate() {
        for posting in *source {
            let new_doc_id = if let Some(maps) = doc_id_maps {
                maps[src_idx][posting.doc_id as usize]
            } else {
                posting.doc_id
            };
            merged.push(PostingEntry {
                doc_id: new_doc_id,
                frequency: posting.frequency,
                fieldnorm: posting.fieldnorm,
                tenant_id: posting.tenant_id,
            });
        }
    }

    // Sort by (tenant_id, doc_id) for tenant-segregated blocks
    merged.sort_by(|a, b| {
        a.tenant_id
            .cmp(&b.tenant_id)
            .then(a.doc_id.cmp(&b.doc_id))
    });
    merged
}

/// Aggregate per-tenant statistics from multiple source segments.
///
/// Combines TpTenantStats from each source, summing num_docs and
/// total_tokens per tenant.
pub fn merge_tenant_stats(
    sources: &[&[TpTenantStats]],
) -> Vec<TpTenantStats> {
    let mut by_tenant: BTreeMap<u32, (u32, u64)> = BTreeMap::new();

    for source in sources {
        for stat in *source {
            let entry = by_tenant
                .entry(stat.tenant_id)
                .or_insert((0, 0));
            entry.0 += stat.num_docs;
            entry.1 += stat.total_tokens;
        }
    }

    by_tenant
        .into_iter()
        .map(|(tenant_id, (num_docs, total_tokens))| {
            TpTenantStats {
                tenant_id,
                num_docs,
                total_tokens,
            }
        })
        .collect()
}

/// Compute per-tenant doc_freq for a merged posting list.
///
/// Given a posting list sorted by (tenant_id, doc_id), counts
/// the number of documents per tenant.
pub fn compute_tenant_docfreqs(
    postings: &[PostingEntry],
) -> Vec<TpTenantDocFreq> {
    let mut by_tenant: BTreeMap<u32, u32> = BTreeMap::new();

    for posting in postings {
        *by_tenant.entry(posting.tenant_id).or_insert(0) += 1;
    }

    by_tenant
        .into_iter()
        .map(|(tenant_id, doc_freq)| TpTenantDocFreq {
            tenant_id,
            doc_freq,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(doc_id: u32, frequency: u16, fieldnorm: u8) -> PostingEntry {
        PostingEntry {
            doc_id,
            frequency,
            fieldnorm,
            tenant_id: 0,
        }
    }

    #[test]
    fn test_merge_postings_single_source() {
        let postings = vec![p(1, 3, 42), p(5, 1, 55)];
        let merged = merge_postings(&[&postings], None);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].doc_id, 1);
        assert_eq!(merged[1].doc_id, 5);
    }

    #[test]
    fn test_merge_postings_two_sources() {
        let src1 = vec![p(1, 3, 42), p(5, 1, 55)];
        let src2 = vec![p(2, 2, 30), p(7, 4, 60)];

        let merged = merge_postings(&[&src1, &src2], None);
        assert_eq!(merged.len(), 4);
        assert_eq!(merged[0].doc_id, 1);
        assert_eq!(merged[1].doc_id, 2);
        assert_eq!(merged[2].doc_id, 5);
        assert_eq!(merged[3].doc_id, 7);
    }

    #[test]
    fn test_merge_postings_with_remap() {
        let src1 = vec![p(0, 3, 42)];
        let src2 = vec![p(0, 1, 55)];

        let map1 = vec![10u32];
        let map2 = vec![20u32];
        let maps: Vec<&[u32]> = vec![&map1, &map2];

        let merged = merge_postings(&[&src1, &src2], Some(&maps));
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].doc_id, 10);
        assert_eq!(merged[1].doc_id, 20);
    }

    #[test]
    fn test_merge_term_lists() {
        let src1 = vec![
            ("alpha".to_string(), vec![p(0, 1, 42)]),
            ("beta".to_string(), vec![p(1, 2, 42)]),
        ];
        let src2 = vec![
            ("beta".to_string(), vec![p(0, 3, 55)]),
            ("gamma".to_string(), vec![p(1, 1, 30)]),
        ];

        let merged = merge_term_lists(&[src1, src2], None);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].0, "alpha");
        assert_eq!(merged[1].0, "beta");
        assert_eq!(merged[1].1.len(), 2);
        assert_eq!(merged[2].0, "gamma");
    }

    fn pt(
        doc_id: u32,
        frequency: u16,
        fieldnorm: u8,
        tenant_id: u32,
    ) -> PostingEntry {
        PostingEntry {
            doc_id,
            frequency,
            fieldnorm,
            tenant_id,
        }
    }

    #[test]
    fn test_merge_postings_tenant_sorted() {
        let src1 = vec![pt(0, 1, 42, 10), pt(1, 2, 42, 20)];
        let src2 = vec![pt(0, 3, 55, 10), pt(1, 1, 30, 20)];

        let merged = merge_postings_tenant(&[&src1, &src2], None);
        assert_eq!(merged.len(), 4);
        // Should be sorted by (tenant_id, doc_id)
        assert_eq!(merged[0].tenant_id, 10);
        assert_eq!(merged[1].tenant_id, 10);
        assert_eq!(merged[2].tenant_id, 20);
        assert_eq!(merged[3].tenant_id, 20);
    }

    #[test]
    fn test_merge_tenant_stats() {
        let src1 = vec![
            TpTenantStats {
                tenant_id: 1,
                num_docs: 100,
                total_tokens: 5000,
            },
            TpTenantStats {
                tenant_id: 2,
                num_docs: 50,
                total_tokens: 2000,
            },
        ];
        let src2 = vec![TpTenantStats {
            tenant_id: 1,
            num_docs: 200,
            total_tokens: 10000,
        }];

        let merged = merge_tenant_stats(&[&src1, &src2]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].tenant_id, 1);
        assert_eq!(merged[0].num_docs, 300);
        assert_eq!(merged[0].total_tokens, 15000);
        assert_eq!(merged[1].tenant_id, 2);
        assert_eq!(merged[1].num_docs, 50);
    }

    #[test]
    fn test_compute_tenant_docfreqs() {
        let postings = vec![
            pt(0, 1, 42, 10),
            pt(1, 2, 42, 10),
            pt(2, 1, 55, 10),
            pt(3, 3, 30, 20),
            pt(4, 1, 42, 20),
        ];
        let freqs = compute_tenant_docfreqs(&postings);
        assert_eq!(freqs.len(), 2);
        assert_eq!(freqs[0].tenant_id, 10);
        assert_eq!(freqs[0].doc_freq, 3);
        assert_eq!(freqs[1].tenant_id, 20);
        assert_eq!(freqs[1].doc_freq, 2);
    }
}
