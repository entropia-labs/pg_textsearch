/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * bmw/term_state.rs - Per-term iteration state for WAND traversal
 *
 * Each term being scored maintains:
 * - Pre-computed block max scores and last_doc_ids
 * - Current position within the posting list
 * - Global max_score used for WAND pivot selection
 */

use super::BlockPosting;

/// Per-term state for multi-term WAND scoring.
///
/// Terms are sorted by current doc_id during traversal.
/// The max_score field is used for WAND pivot selection.
pub struct TermState {
    /// Pre-computed IDF for this term.
    pub idf: f32,
    /// Query term frequency (for multi-word queries).
    pub query_freq: i32,
    /// Global maximum score across all blocks (* query_freq).
    pub max_score: f32,
    /// Whether the term was found in the current segment.
    pub found: bool,
    /// Whether iteration is exhausted.
    pub finished: bool,

    /// Pre-computed per-block max BM25 scores.
    pub block_max_scores: Vec<f32>,
    /// Cached last_doc_id per block (for binary search seeking).
    pub block_last_doc_ids: Vec<u32>,

    /// Current block index (0-based).
    pub current_block: u16,
    /// Position within current block.
    pub current_in_block: u16,
    /// Total block count for this term.
    pub block_count: u16,

    /// Current block's decoded postings.
    pub current_postings: Vec<BlockPosting>,
}

impl TermState {
    /// Get the current doc_id. Returns u32::MAX if exhausted.
    pub fn current_doc_id(&self) -> u32 {
        if self.finished || self.current_postings.is_empty() {
            return u32::MAX;
        }
        let idx = self.current_in_block as usize;
        if idx < self.current_postings.len() {
            self.current_postings[idx].doc_id
        } else {
            u32::MAX
        }
    }

    /// Get the current posting entry, if any.
    pub fn current_posting(&self) -> Option<&BlockPosting> {
        if self.finished {
            return None;
        }
        let idx = self.current_in_block as usize;
        self.current_postings.get(idx)
    }

    /// Advance to the next posting in the current block.
    /// Returns true if there's another posting, false if block
    /// is exhausted (caller should load next block).
    pub fn advance_in_block(&mut self) -> bool {
        self.current_in_block += 1;
        (self.current_in_block as usize) < self.current_postings.len()
    }

    /// Find the block containing target_doc_id using binary search
    /// on the cached block_last_doc_ids array.
    ///
    /// Returns the block index, or None if target is past all blocks.
    pub fn find_block_for_doc(&self, target_doc_id: u32) -> Option<u16> {
        if self.block_last_doc_ids.is_empty() {
            return None;
        }

        // Binary search: find first block whose last_doc_id >= target
        let mut left = self.current_block as usize;
        let mut right = self.block_last_doc_ids.len();

        while left < right {
            let mid = left + (right - left) / 2;
            if self.block_last_doc_ids[mid] < target_doc_id {
                left = mid + 1;
            } else {
                right = mid;
            }
        }

        if left < self.block_last_doc_ids.len() {
            Some(left as u16)
        } else {
            None // Past all blocks
        }
    }

    /// Create a test TermState with a given current doc_id.
    #[cfg(test)]
    pub fn test_new(
        current_doc_id: u32,
        max_score: f32,
        query_freq: i32,
    ) -> Self {
        TermState {
            idf: 1.0,
            query_freq,
            max_score,
            found: true,
            finished: false,
            block_max_scores: vec![max_score],
            block_last_doc_ids: vec![u32::MAX],
            current_block: 0,
            current_in_block: 0,
            block_count: 1,
            current_postings: vec![BlockPosting {
                doc_id: current_doc_id,
                frequency: 1,
                fieldnorm: 42,
            }],
        }
    }
}

/// Sort term states by current doc_id.
/// Used once after initialization for WAND traversal.
pub fn sort_terms_by_doc_id(terms: &mut [TermState]) {
    terms.sort_by_key(|t| t.current_doc_id());
}

/// Restore sorted order after term at position `ord` advanced.
/// The term's doc_id increased, so it may need to move right.
/// Uses linear insertion — O(1) typical, O(T) worst case.
pub fn restore_ordering(terms: &mut [TermState], ord: usize) {
    let doc_id = terms[ord].current_doc_id();
    let term_count = terms.len();

    // Find where this term should go
    let mut target = ord + 1;
    while target < term_count
        && terms[target].current_doc_id() < doc_id
    {
        target += 1;
    }

    // Rotate the term from ord to target-1
    if target > ord + 1 {
        terms[ord..target].rotate_left(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_doc_id() {
        let ts = TermState::test_new(42, 1.0, 1);
        assert_eq!(ts.current_doc_id(), 42);
    }

    #[test]
    fn test_current_doc_id_finished() {
        let mut ts = TermState::test_new(42, 1.0, 1);
        ts.finished = true;
        assert_eq!(ts.current_doc_id(), u32::MAX);
    }

    #[test]
    fn test_advance_in_block() {
        let mut ts = TermState {
            current_postings: vec![
                BlockPosting {
                    doc_id: 1,
                    frequency: 1,
                    fieldnorm: 42,
                },
                BlockPosting {
                    doc_id: 5,
                    frequency: 2,
                    fieldnorm: 42,
                },
            ],
            ..TermState::test_new(1, 1.0, 1)
        };

        assert_eq!(ts.current_doc_id(), 1);
        assert!(ts.advance_in_block());
        assert_eq!(ts.current_doc_id(), 5);
        assert!(!ts.advance_in_block());
    }

    #[test]
    fn test_find_block_for_doc() {
        let ts = TermState {
            block_last_doc_ids: vec![127, 255, 383, 500],
            current_block: 0,
            ..TermState::test_new(0, 1.0, 1)
        };

        assert_eq!(ts.find_block_for_doc(0), Some(0));
        assert_eq!(ts.find_block_for_doc(127), Some(0));
        assert_eq!(ts.find_block_for_doc(128), Some(1));
        assert_eq!(ts.find_block_for_doc(400), Some(3));
        assert_eq!(ts.find_block_for_doc(501), None);
    }

    #[test]
    fn test_sort_terms_by_doc_id() {
        let mut terms = vec![
            TermState::test_new(30, 1.0, 1),
            TermState::test_new(10, 2.0, 1),
            TermState::test_new(20, 1.5, 1),
        ];

        sort_terms_by_doc_id(&mut terms);

        assert_eq!(terms[0].current_doc_id(), 10);
        assert_eq!(terms[1].current_doc_id(), 20);
        assert_eq!(terms[2].current_doc_id(), 30);
    }

    #[test]
    fn test_restore_ordering() {
        let mut terms = vec![
            TermState::test_new(10, 1.0, 1),
            TermState::test_new(20, 1.0, 1),
            TermState::test_new(30, 1.0, 1),
        ];

        // Advance term[0] past term[1]
        terms[0].current_postings[0].doc_id = 25;
        restore_ordering(&mut terms, 0);

        assert_eq!(terms[0].current_doc_id(), 20);
        assert_eq!(terms[1].current_doc_id(), 25);
        assert_eq!(terms[2].current_doc_id(), 30);
    }

    #[test]
    fn test_restore_ordering_no_move() {
        let mut terms = vec![
            TermState::test_new(10, 1.0, 1),
            TermState::test_new(20, 1.0, 1),
        ];

        // term[0] still less than term[1] — no move needed
        terms[0].current_postings[0].doc_id = 15;
        restore_ordering(&mut terms, 0);

        assert_eq!(terms[0].current_doc_id(), 15);
        assert_eq!(terms[1].current_doc_id(), 20);
    }

    #[test]
    fn test_restore_ordering_move_to_end() {
        let mut terms = vec![
            TermState::test_new(10, 1.0, 1),
            TermState::test_new(20, 1.0, 1),
            TermState::test_new(30, 1.0, 1),
        ];

        // Advance term[0] past all others
        terms[0].current_postings[0].doc_id = 50;
        restore_ordering(&mut terms, 0);

        assert_eq!(terms[0].current_doc_id(), 20);
        assert_eq!(terms[1].current_doc_id(), 30);
        assert_eq!(terms[2].current_doc_id(), 50);
    }
}
