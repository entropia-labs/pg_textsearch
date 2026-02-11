/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * bmw/mod.rs - Block-Max WAND query optimization
 *
 * Ports the core BMW algorithm from C to Rust:
 * - Top-K min-heap for threshold management
 * - Single-term BMW with block-level skipping
 * - Multi-term WAND with pivot selection and block-max refinement
 *
 * The algorithm operates through callbacks: C provides block/posting
 * access via function pointers; Rust handles the scoring logic.
 */

pub mod term_state;

use crate::scoring::{block_max_score, bm25_score};
use term_state::TermState;

/// Statistics for BMW scoring, mirrors C TpBMWStats.
#[derive(Clone, Debug, Default)]
pub struct BMWStats {
    pub blocks_scanned: u64,
    pub blocks_skipped: u64,
    pub memtable_docs: u64,
    pub segment_docs_scored: u64,
    pub docs_in_results: u64,
    pub seeks_performed: u64,
}

/// A scored document result. CTID resolution is deferred for
/// segment results (seg_block != u32::MAX).
#[derive(Clone, Copy, Debug)]
pub struct ScoredDoc {
    /// Heap tuple page number (0 for segment results pre-resolution).
    pub ctid_page: u32,
    /// Heap tuple offset (0 for segment results pre-resolution).
    pub ctid_offset: u16,
    /// Segment root block (u32::MAX = memtable result).
    pub seg_block: u32,
    /// Segment-local doc ID (for deferred CTID lookup).
    pub doc_id: u32,
    /// BM25 score.
    pub score: f32,
}

/// Min-heap maintaining top-k results by score.
///
/// Heap property: parent.score <= child.score (minimum at root).
/// When full, root.score is the threshold — any doc scoring below
/// cannot enter the top-k.
pub struct TopKHeap {
    entries: Vec<ScoredDoc>,
    capacity: usize,
}

impl TopKHeap {
    pub fn new(k: usize) -> Self {
        TopKHeap {
            entries: Vec::with_capacity(k),
            capacity: k,
        }
    }

    /// Current threshold: minimum score to enter top-k.
    /// Returns 0.0 if heap not yet full.
    pub fn threshold(&self) -> f32 {
        if self.entries.len() >= self.capacity {
            self.entries[0].score
        } else {
            0.0
        }
    }

    /// Check if a score is dominated (cannot enter top-k).
    pub fn dominated(&self, score: f32) -> bool {
        self.entries.len() >= self.capacity && score < self.entries[0].score
    }

    /// Add a memtable result (CTID known immediately).
    pub fn add_memtable(
        &mut self,
        ctid_page: u32,
        ctid_offset: u16,
        score: f32,
    ) {
        self.add(ScoredDoc {
            ctid_page,
            ctid_offset,
            seg_block: u32::MAX,
            doc_id: 0,
            score,
        });
    }

    /// Add a segment result (CTID resolved later).
    pub fn add_segment(
        &mut self,
        seg_block: u32,
        doc_id: u32,
        score: f32,
    ) {
        self.add(ScoredDoc {
            ctid_page: 0,
            ctid_offset: 0,
            seg_block,
            doc_id,
            score,
        });
    }

    /// Add a scored doc to the heap.
    fn add(&mut self, doc: ScoredDoc) {
        if self.entries.len() < self.capacity {
            // Heap not full: push and sift up
            self.entries.push(doc);
            self.sift_up(self.entries.len() - 1);
        } else if self.beats_root(doc.score, &doc) {
            // Replace root (minimum) with new entry
            self.entries[0] = doc;
            self.sift_down(0);
        }
    }

    /// Check if a candidate beats the root (weakest entry).
    fn beats_root(&self, score: f32, _doc: &ScoredDoc) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        score > self.entries[0].score
    }

    /// Extract sorted results (descending by score).
    /// After extraction, heap is empty.
    pub fn extract_sorted(&mut self) -> Vec<ScoredDoc> {
        let mut results: Vec<ScoredDoc> = self.entries.drain(..).collect();
        // Sort by score descending, then CTID ascending for tie-breaking
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.ctid_page
                        .cmp(&b.ctid_page)
                        .then(a.ctid_offset.cmp(&b.ctid_offset))
                })
        });
        results
    }

    /// Number of entries currently in the heap.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the heap is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    // --- Heap operations ---

    fn sift_up(&mut self, mut idx: usize) {
        while idx > 0 {
            let parent = (idx - 1) / 2;
            if self.heap_less(idx, parent) {
                self.entries.swap(idx, parent);
                idx = parent;
            } else {
                break;
            }
        }
    }

    fn sift_down(&mut self, mut idx: usize) {
        let len = self.entries.len();
        loop {
            let left = 2 * idx + 1;
            let right = 2 * idx + 2;
            let mut smallest = idx;

            if left < len && self.heap_less(left, smallest) {
                smallest = left;
            }
            if right < len && self.heap_less(right, smallest) {
                smallest = right;
            }
            if smallest == idx {
                break;
            }
            self.entries.swap(idx, smallest);
            idx = smallest;
        }
    }

    /// Min-heap comparison: lower score = closer to root.
    /// Tie-break: higher CTID (page,offset) is weaker.
    fn heap_less(&self, a: usize, b: usize) -> bool {
        let sa = self.entries[a].score;
        let sb = self.entries[b].score;
        if sa != sb {
            return sa < sb;
        }
        // Equal scores: higher CTID is weaker (closer to root)
        let pa = (self.entries[a].ctid_page, self.entries[a].ctid_offset);
        let pb = (self.entries[b].ctid_page, self.entries[b].ctid_offset);
        pa > pb
    }
}

/// Block metadata loaded from skip entries.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockInfo {
    pub last_doc_id: u32,
    pub doc_count: u8,
    pub block_max_tf: u16,
    pub block_max_norm: u8,
    pub posting_offset: u32,
    pub flags: u8,
    pub tenant_mode: u8,
    pub tenant_id_low16: u16,
}

/// A posting entry from a decoded block.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockPosting {
    pub doc_id: u32,
    pub frequency: u16,
    pub fieldnorm: u8,
}

/// Score a single-term query against pre-loaded block data.
///
/// `blocks` contains skip entry metadata for each block.
/// `load_block` is called to decode postings for a block.
///
/// Returns the number of scored documents.
#[allow(clippy::too_many_arguments)]
pub fn score_single_term_bmw(
    heap: &mut TopKHeap,
    blocks: &[BlockInfo],
    idf: f32,
    k1: f32,
    b: f32,
    avgdl: f32,
    seg_block: u32,
    load_block: &mut dyn FnMut(usize) -> Vec<BlockPosting>,
    stats: &mut BMWStats,
) {
    // Pre-compute block max scores
    let block_max_scores: Vec<f32> = blocks
        .iter()
        .map(|bi| block_max_score(bi.block_max_tf, bi.block_max_norm, idf, k1, b, avgdl))
        .collect();

    for (block_idx, block_info) in blocks.iter().enumerate() {
        let threshold = heap.threshold();

        // Skip block if its upper bound can't beat threshold
        if block_max_scores[block_idx] <= threshold {
            stats.blocks_skipped += 1;
            continue;
        }

        stats.blocks_scanned += 1;

        // Load and score postings in this block
        let postings = load_block(block_idx);
        for posting in &postings {
            let dl = crate::fieldnorm::decode_fieldnorm(posting.fieldnorm);
            let score = bm25_score(
                idf,
                posting.frequency as i32,
                dl as i32,
                k1,
                b,
                avgdl,
            );

            if score > 0.0 && !heap.dominated(score) {
                heap.add_segment(seg_block, posting.doc_id, score);
            }
            stats.segment_docs_scored += 1;
        }

        // Re-check: if threshold increased, skip remaining blocks
        // whose max can't beat it
        let _ = block_info; // silence unused warning
    }
}

/// WAND pivot selection for multi-term queries.
///
/// Given terms sorted by current doc_id, walks from lowest doc_id
/// accumulating each term's max_score. When the sum exceeds
/// threshold, returns the pivot position and doc_id.
pub fn find_wand_pivot(
    terms: &[TermState],
    threshold: f32,
) -> Option<(usize, u32)> {
    let mut accumulated = 0.0_f32;

    for (i, term) in terms.iter().enumerate() {
        let doc_id = term.current_doc_id();
        if doc_id == u32::MAX {
            break; // No more active terms
        }

        accumulated += term.max_score;
        if accumulated > threshold {
            // Found pivot. Include subsequent terms at same doc_id.
            let pivot_doc = doc_id;
            let mut pivot_len = i + 1;
            while pivot_len < terms.len()
                && terms[pivot_len].current_doc_id() == pivot_doc
            {
                pivot_len += 1;
            }
            return Some((pivot_len, pivot_doc));
        }
    }

    None // Can't beat threshold
}

/// Compute block-max upper bound at pivot.
///
/// After WAND pivot selection, refine with actual block-level
/// upper bounds. Only considers terms [0..pivot_len-1].
pub fn compute_block_max_at_pivot(
    terms: &[TermState],
    pivot_len: usize,
) -> f32 {
    let mut upper_bound = 0.0_f32;

    for term in terms.iter().take(pivot_len) {
        if term.finished || term.block_max_scores.is_empty() {
            continue;
        }

        let block = term.current_block as usize;
        if block < term.block_max_scores.len() {
            upper_bound +=
                term.block_max_scores[block] * term.query_freq as f32;
        }
    }

    upper_bound
}

/// Score a pivot document by accumulating BM25 contributions.
///
/// All pivot terms must be positioned at pivot_doc_id.
pub fn score_pivot_document(
    terms: &[TermState],
    pivot_len: usize,
    k1: f32,
    b: f32,
    avgdl: f32,
) -> f32 {
    let mut doc_score = 0.0_f32;

    for term in terms.iter().take(pivot_len) {
        if term.finished {
            continue;
        }

        if let Some(posting) = term.current_posting() {
            let dl = crate::fieldnorm::decode_fieldnorm(posting.fieldnorm);
            let term_score = bm25_score(
                term.idf,
                posting.frequency as i32,
                dl as i32,
                k1,
                b,
                avgdl,
            ) * term.query_freq as f32;
            doc_score += term_score;
        }
    }

    doc_score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topk_empty() {
        let heap = TopKHeap::new(10);
        assert_eq!(heap.threshold(), 0.0);
        assert!(!heap.dominated(1.0));
        assert!(heap.is_empty());
    }

    #[test]
    fn test_topk_add_below_capacity() {
        let mut heap = TopKHeap::new(3);
        heap.add_memtable(1, 1, 5.0);
        heap.add_memtable(2, 1, 3.0);

        assert_eq!(heap.len(), 2);
        assert_eq!(heap.threshold(), 0.0); // Not full yet
    }

    #[test]
    fn test_topk_threshold_when_full() {
        let mut heap = TopKHeap::new(3);
        heap.add_memtable(1, 1, 5.0);
        heap.add_memtable(2, 1, 3.0);
        heap.add_memtable(3, 1, 7.0);

        assert_eq!(heap.len(), 3);
        assert_eq!(heap.threshold(), 3.0); // Min score
    }

    #[test]
    fn test_topk_eviction() {
        let mut heap = TopKHeap::new(3);
        heap.add_memtable(1, 1, 5.0);
        heap.add_memtable(2, 1, 3.0);
        heap.add_memtable(3, 1, 7.0);

        // Add higher score - should evict 3.0
        heap.add_memtable(4, 1, 10.0);

        assert_eq!(heap.len(), 3);
        assert_eq!(heap.threshold(), 5.0);
    }

    #[test]
    fn test_topk_dominated() {
        let mut heap = TopKHeap::new(2);
        heap.add_memtable(1, 1, 5.0);
        heap.add_memtable(2, 1, 3.0);

        assert!(heap.dominated(2.0));
        assert!(!heap.dominated(4.0));
    }

    #[test]
    fn test_topk_extract_sorted() {
        let mut heap = TopKHeap::new(5);
        heap.add_memtable(1, 1, 5.0);
        heap.add_memtable(2, 1, 3.0);
        heap.add_memtable(3, 1, 7.0);
        heap.add_memtable(4, 1, 1.0);

        let results = heap.extract_sorted();
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].score, 7.0);
        assert_eq!(results[1].score, 5.0);
        assert_eq!(results[2].score, 3.0);
        assert_eq!(results[3].score, 1.0);
    }

    #[test]
    fn test_topk_segment_results() {
        let mut heap = TopKHeap::new(3);
        heap.add_segment(100, 0, 5.0);
        heap.add_segment(100, 1, 3.0);
        heap.add_memtable(1, 1, 7.0);

        let results = heap.extract_sorted();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].score, 7.0);
        assert_eq!(results[0].seg_block, u32::MAX); // memtable
        assert_eq!(results[1].seg_block, 100); // segment
    }

    #[test]
    fn test_find_wand_pivot_basic() {
        let terms = vec![
            TermState::test_new(10, 3.0, 1),
            TermState::test_new(20, 2.0, 1),
            TermState::test_new(30, 1.5, 1),
        ];

        // Threshold 4.0: terms[0](3.0) + terms[1](2.0) = 5.0 > 4.0
        let result = find_wand_pivot(&terms, 4.0);
        assert!(result.is_some());
        let (pivot_len, pivot_doc) = result.unwrap();
        assert_eq!(pivot_len, 2);
        assert_eq!(pivot_doc, 20);
    }

    #[test]
    fn test_find_wand_pivot_no_match() {
        let terms = vec![
            TermState::test_new(10, 1.0, 1),
            TermState::test_new(20, 1.0, 1),
        ];

        // Threshold too high
        let result = find_wand_pivot(&terms, 10.0);
        assert!(result.is_none());
    }

    #[test]
    fn test_score_single_term_bmw_basic() {
        let mut heap = TopKHeap::new(10);
        let blocks = vec![
            BlockInfo {
                last_doc_id: 127,
                doc_count: 128,
                block_max_tf: 5,
                block_max_norm: 42,
                ..Default::default()
            },
            BlockInfo {
                last_doc_id: 255,
                doc_count: 128,
                block_max_tf: 1,
                block_max_norm: 42,
                ..Default::default()
            },
        ];

        let idf = crate::scoring::calculate_idf(10, 1000);
        let mut stats = BMWStats::default();

        score_single_term_bmw(
            &mut heap,
            &blocks,
            idf,
            1.2,
            0.75,
            100.0,
            1, // seg_block
            &mut |_block_idx| {
                vec![
                    BlockPosting {
                        doc_id: 0,
                        frequency: 3,
                        fieldnorm: 42,
                    },
                    BlockPosting {
                        doc_id: 1,
                        frequency: 1,
                        fieldnorm: 55,
                    },
                ]
            },
            &mut stats,
        );

        assert!(stats.blocks_scanned >= 1);
        assert!(heap.len() >= 2);
    }
}
