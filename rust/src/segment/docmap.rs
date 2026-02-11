/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * docmap.rs - Document ID remapping for segment merges
 *
 * During segment merge, each source segment has its own doc_id
 * space (0..N-1). The merged segment needs a unified doc_id space.
 * This module builds the remapping tables.
 *
 * Note: The build-time CTID → doc_id mapping stays in C
 * (uses Postgres hash tables and memory contexts). This module
 * handles only the merge-time remapping logic.
 */

/// Build doc_id remapping tables for an N-way segment merge.
///
/// Given the number of documents in each source segment,
/// returns a vector of maps where `maps[i][old_id]` = new_id
/// in the merged segment.
///
/// Documents are assigned new IDs sequentially:
/// segment 0 docs get 0..n0-1, segment 1 gets n0..n0+n1-1, etc.
pub fn build_merge_remap(doc_counts: &[u32]) -> Vec<Vec<u32>> {
    let mut maps = Vec::with_capacity(doc_counts.len());
    let mut next_id: u32 = 0;

    for &count in doc_counts {
        let map: Vec<u32> = (next_id..next_id + count).collect();
        maps.push(map);
        next_id += count;
    }

    maps
}

/// Total number of documents across all segments.
pub fn total_docs(doc_counts: &[u32]) -> u32 {
    doc_counts.iter().sum()
}

// --- FFI exports ---

/// Build doc_id remap tables for merging `num_sources` segments.
///
/// # Safety
/// `doc_counts` must point to `num_sources` u32 values.
/// `out_maps` must point to `num_sources` pointers, each with space
/// for `doc_counts[i]` u32 values.
#[no_mangle]
pub unsafe extern "C" fn tp_rust_build_merge_remap(
    doc_counts: *const u32,
    num_sources: u32,
    out_maps: *mut *mut u32,
) -> u32 {
    if doc_counts.is_null() || out_maps.is_null() || num_sources == 0 {
        return 0;
    }

    let counts = unsafe {
        std::slice::from_raw_parts(doc_counts, num_sources as usize)
    };
    let maps = build_merge_remap(counts);

    let out_slices = unsafe {
        std::slice::from_raw_parts_mut(out_maps, num_sources as usize)
    };

    for (i, map) in maps.iter().enumerate() {
        if !out_slices[i].is_null() {
            let out = unsafe {
                std::slice::from_raw_parts_mut(
                    out_slices[i],
                    map.len(),
                )
            };
            out.copy_from_slice(map);
        }
    }

    total_docs(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_merge_remap_single() {
        let maps = build_merge_remap(&[5]);
        assert_eq!(maps.len(), 1);
        assert_eq!(maps[0], vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_build_merge_remap_multiple() {
        let maps = build_merge_remap(&[3, 2, 4]);
        assert_eq!(maps.len(), 3);
        assert_eq!(maps[0], vec![0, 1, 2]);
        assert_eq!(maps[1], vec![3, 4]);
        assert_eq!(maps[2], vec![5, 6, 7, 8]);
    }

    #[test]
    fn test_build_merge_remap_empty() {
        let maps = build_merge_remap(&[]);
        assert!(maps.is_empty());
    }

    #[test]
    fn test_build_merge_remap_with_zero() {
        let maps = build_merge_remap(&[2, 0, 3]);
        assert_eq!(maps.len(), 3);
        assert_eq!(maps[0], vec![0, 1]);
        assert!(maps[1].is_empty());
        assert_eq!(maps[2], vec![2, 3, 4]);
    }

    #[test]
    fn test_total_docs() {
        assert_eq!(total_docs(&[3, 2, 4]), 9);
        assert_eq!(total_docs(&[]), 0);
        assert_eq!(total_docs(&[0, 0, 0]), 0);
    }
}
