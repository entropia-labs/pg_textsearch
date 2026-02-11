/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * topk.rs - Top-K min-heap for BM25 query results
 *
 * Maintains the top-k highest-scoring entries using a min-heap.
 * The root always holds the minimum score, enabling O(1) threshold
 * checks and O(log k) insertions.
 *
 * Entries are stored as (score, index) pairs. The index is an opaque
 * u32 that the caller uses to map back to their own data structures
 * (e.g., CTIDs, segment blocks, doc IDs).
 */

/// A min-heap of (score, index) pairs for maintaining top-k results.
///
/// Heap property: parent score <= child scores (minimum at root).
/// When the heap is full, only entries scoring above the root can
/// enter. This naturally maintains the top-k highest-scoring entries.
pub struct TopKHeap {
    scores: Vec<f32>,
    indices: Vec<u32>,
    capacity: usize,
    size: usize,
}

impl TopKHeap {
    /// Create a new top-k heap with the given capacity.
    pub fn new(capacity: usize) -> Self {
        TopKHeap {
            scores: vec![0.0; capacity],
            indices: vec![0; capacity],
            capacity,
            size: 0,
        }
    }

    /// Get the current threshold score.
    /// Returns 0 if the heap is not yet full.
    #[inline]
    pub fn threshold(&self) -> f32 {
        if self.size >= self.capacity {
            self.scores[0]
        } else {
            0.0
        }
    }

    /// Check if a score is strictly dominated (cannot enter top-k).
    #[inline]
    pub fn dominated(&self, score: f32) -> bool {
        self.size >= self.capacity && score < self.scores[0]
    }

    /// Try to add an entry to the heap.
    ///
    /// Returns true if the entry was added (either heap not full,
    /// or score beats the current minimum).
    pub fn add(&mut self, index: u32, score: f32) -> bool {
        if self.size < self.capacity {
            // Heap not full - append and sift up
            let pos = self.size;
            self.scores[pos] = score;
            self.indices[pos] = index;
            self.size += 1;
            self.sift_up(pos);
            true
        } else if score > self.scores[0] {
            // Beats root - replace and sift down
            self.scores[0] = score;
            self.indices[0] = index;
            self.sift_down(0);
            true
        } else {
            false
        }
    }

    /// Extract sorted results (descending by score).
    /// Returns the number of entries extracted.
    /// After extraction, the heap is empty.
    pub fn extract_sorted(
        &mut self,
        out_indices: &mut [u32],
        out_scores: &mut [f32],
    ) -> usize {
        let count = self.size;
        if count == 0 {
            return 0;
        }

        // Collect entries and sort by score descending
        let mut entries: Vec<(f32, u32)> = (0..count)
            .map(|i| (self.scores[i], self.indices[i]))
            .collect();

        entries.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        });

        for (i, (score, index)) in entries.iter().enumerate() {
            out_scores[i] = *score;
            out_indices[i] = *index;
        }

        self.size = 0;
        count
    }

    /// Current number of entries in the heap.
    pub fn len(&self) -> usize {
        self.size
    }

    /// Check if heap is empty.
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    // --- Internal heap operations ---

    fn sift_up(&mut self, mut i: usize) {
        while i > 0 {
            let parent = (i - 1) / 2;
            if self.scores[i] >= self.scores[parent] {
                break;
            }
            self.swap(i, parent);
            i = parent;
        }
    }

    fn sift_down(&mut self, mut i: usize) {
        loop {
            let left = 2 * i + 1;
            let right = 2 * i + 2;
            let mut smallest = i;

            if left < self.size && self.scores[left] < self.scores[smallest] {
                smallest = left;
            }
            if right < self.size && self.scores[right] < self.scores[smallest]
            {
                smallest = right;
            }

            if smallest == i {
                break;
            }

            self.swap(i, smallest);
            i = smallest;
        }
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.scores.swap(i, j);
        self.indices.swap(i, j);
    }
}

// --- FFI exports ---

/// Create a new top-k heap.
///
/// Returns a heap pointer that must be freed with `tp_rust_topk_free`.
#[no_mangle]
pub extern "C" fn tp_rust_topk_create(capacity: i32) -> *mut TopKHeap {
    let heap = Box::new(TopKHeap::new(capacity as usize));
    Box::into_raw(heap)
}

/// Get the current threshold score (0 if heap not full).
///
/// # Safety
/// `heap` must be a valid pointer from `tp_rust_topk_create`.
#[no_mangle]
pub unsafe extern "C" fn tp_rust_topk_threshold(
    heap: *const TopKHeap,
) -> f32 {
    unsafe { (*heap).threshold() }
}

/// Check if a score is dominated (cannot enter top-k).
///
/// # Safety
/// `heap` must be a valid pointer from `tp_rust_topk_create`.
#[no_mangle]
pub unsafe extern "C" fn tp_rust_topk_dominated(
    heap: *const TopKHeap,
    score: f32,
) -> bool {
    unsafe { (*heap).dominated(score) }
}

/// Add an entry to the top-k heap.
/// Returns true if the entry was added.
///
/// # Safety
/// `heap` must be a valid pointer from `tp_rust_topk_create`.
#[no_mangle]
pub unsafe extern "C" fn tp_rust_topk_add(
    heap: *mut TopKHeap,
    index: u32,
    score: f32,
) -> bool {
    unsafe { (*heap).add(index, score) }
}

/// Extract sorted results (descending by score).
/// Returns number of entries extracted.
///
/// # Safety
/// `heap` must be a valid pointer from `tp_rust_topk_create`.
/// `out_indices` and `out_scores` must have space for at least
/// `capacity` entries.
#[no_mangle]
pub unsafe extern "C" fn tp_rust_topk_extract(
    heap: *mut TopKHeap,
    out_indices: *mut u32,
    out_scores: *mut f32,
) -> i32 {
    let h = unsafe { &mut *heap };
    let cap = h.capacity;
    let indices_slice =
        unsafe { std::slice::from_raw_parts_mut(out_indices, cap) };
    let scores_slice =
        unsafe { std::slice::from_raw_parts_mut(out_scores, cap) };
    h.extract_sorted(indices_slice, scores_slice) as i32
}

/// Get the current number of entries in the heap.
///
/// # Safety
/// `heap` must be a valid pointer from `tp_rust_topk_create`.
#[no_mangle]
pub unsafe extern "C" fn tp_rust_topk_size(heap: *const TopKHeap) -> i32 {
    unsafe { (*heap).len() as i32 }
}

/// Free a top-k heap.
///
/// # Safety
/// `heap` must be a valid pointer from `tp_rust_topk_create`,
/// or null (in which case this is a no-op).
#[no_mangle]
pub unsafe extern "C" fn tp_rust_topk_free(heap: *mut TopKHeap) {
    if !heap.is_null() {
        unsafe {
            drop(Box::from_raw(heap));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_heap() {
        let heap = TopKHeap::new(5);
        assert_eq!(heap.threshold(), 0.0);
        assert_eq!(heap.len(), 0);
        assert!(heap.is_empty());
        assert!(!heap.dominated(0.0));
    }

    #[test]
    fn test_add_below_capacity() {
        let mut heap = TopKHeap::new(3);
        assert!(heap.add(0, 5.0));
        assert!(heap.add(1, 3.0));
        assert!(heap.add(2, 7.0));
        assert_eq!(heap.len(), 3);
        // Threshold should be the minimum score
        assert_eq!(heap.threshold(), 3.0);
    }

    #[test]
    fn test_add_above_threshold() {
        let mut heap = TopKHeap::new(3);
        heap.add(0, 5.0);
        heap.add(1, 3.0);
        heap.add(2, 7.0);

        // This should replace the 3.0 entry
        assert!(heap.add(3, 4.0));
        assert_eq!(heap.threshold(), 4.0);
    }

    #[test]
    fn test_add_below_threshold() {
        let mut heap = TopKHeap::new(3);
        heap.add(0, 5.0);
        heap.add(1, 3.0);
        heap.add(2, 7.0);

        // This should be rejected (below threshold of 3.0)
        assert!(!heap.add(3, 2.0));
        assert_eq!(heap.len(), 3);
    }

    #[test]
    fn test_dominated() {
        let mut heap = TopKHeap::new(2);
        heap.add(0, 5.0);
        heap.add(1, 3.0);

        assert!(heap.dominated(2.0)); // below threshold
        assert!(!heap.dominated(3.0)); // at threshold (equal)
        assert!(!heap.dominated(4.0)); // above threshold
    }

    #[test]
    fn test_extract_sorted() {
        let mut heap = TopKHeap::new(5);
        heap.add(0, 3.0);
        heap.add(1, 7.0);
        heap.add(2, 1.0);
        heap.add(3, 5.0);
        heap.add(4, 9.0);

        let mut indices = vec![0u32; 5];
        let mut scores = vec![0.0f32; 5];
        let count = heap.extract_sorted(&mut indices, &mut scores);

        assert_eq!(count, 5);
        // Should be descending by score
        assert_eq!(scores[0], 9.0);
        assert_eq!(scores[1], 7.0);
        assert_eq!(scores[2], 5.0);
        assert_eq!(scores[3], 3.0);
        assert_eq!(scores[4], 1.0);

        // Indices should match
        assert_eq!(indices[0], 4); // score 9.0
        assert_eq!(indices[1], 1); // score 7.0
        assert_eq!(indices[2], 3); // score 5.0
        assert_eq!(indices[3], 0); // score 3.0
        assert_eq!(indices[4], 2); // score 1.0

        // Heap should be empty after extraction
        assert!(heap.is_empty());
    }

    #[test]
    fn test_extract_with_evictions() {
        let mut heap = TopKHeap::new(3);
        heap.add(0, 1.0);
        heap.add(1, 2.0);
        heap.add(2, 3.0);
        heap.add(3, 4.0); // evicts 1.0
        heap.add(4, 5.0); // evicts 2.0

        let mut indices = vec![0u32; 3];
        let mut scores = vec![0.0f32; 3];
        let count = heap.extract_sorted(&mut indices, &mut scores);

        assert_eq!(count, 3);
        assert_eq!(scores[0], 5.0);
        assert_eq!(scores[1], 4.0);
        assert_eq!(scores[2], 3.0);
    }

    #[test]
    fn test_capacity_one() {
        let mut heap = TopKHeap::new(1);
        heap.add(0, 5.0);
        assert_eq!(heap.threshold(), 5.0);

        heap.add(1, 3.0); // rejected
        assert_eq!(heap.threshold(), 5.0);

        heap.add(2, 7.0); // replaces 5.0
        assert_eq!(heap.threshold(), 7.0);

        let mut indices = vec![0u32; 1];
        let mut scores = vec![0.0f32; 1];
        let count = heap.extract_sorted(&mut indices, &mut scores);
        assert_eq!(count, 1);
        assert_eq!(scores[0], 7.0);
        assert_eq!(indices[0], 2);
    }
}
