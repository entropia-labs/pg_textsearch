/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * reader.rs - Segment reader for byte-level data access
 *
 * Provides functions to read and parse segment data from byte slices.
 * The C side handles page I/O and provides contiguous byte buffers
 * for the sections that need parsing.
 */

use crate::compression;
use crate::segment::format::*;
use crate::types::TpBlockPosting;

/// Read a TpSegmentHeader from a byte slice.
///
/// # Safety
/// `data` must contain at least `size_of::<TpSegmentHeader>()` bytes.
pub fn read_header(data: &[u8]) -> Option<TpSegmentHeader> {
    let size = std::mem::size_of::<TpSegmentHeader>();
    if data.len() < size {
        return None;
    }
    let header: TpSegmentHeader =
        unsafe { std::ptr::read(data.as_ptr().cast()) };

    if header.magic != TP_SEGMENT_MAGIC {
        return None;
    }
    Some(header)
}

/// Read a skip entry from a byte slice.
pub fn read_skip_entry(data: &[u8], offset: usize) -> Option<TpSkipEntry> {
    let size = std::mem::size_of::<TpSkipEntry>();
    if offset + size > data.len() {
        return None;
    }
    let entry: TpSkipEntry =
        unsafe { std::ptr::read(data[offset..].as_ptr().cast()) };
    Some(entry)
}

/// Read a posting block (possibly compressed).
///
/// `block_data` is the raw bytes at the posting_offset.
/// `skip` describes the block (count, flags, etc.).
///
/// Returns the decompressed block postings.
pub fn read_posting_block(
    block_data: &[u8],
    skip: &TpSkipEntry,
) -> Vec<TpBlockPosting> {
    let count = skip.doc_count as usize;
    let mut postings = vec![TpBlockPosting::default(); count];

    if skip.flags == TP_BLOCK_FLAG_DELTA {
        // Compressed block
        compression::decompress_block(block_data, count, 0, &mut postings);
    } else {
        // Uncompressed: read raw TpBlockPosting array
        let entry_size = std::mem::size_of::<TpBlockPosting>();
        for (i, posting) in postings.iter_mut().enumerate().take(count) {
            let offset = i * entry_size;
            if offset + entry_size <= block_data.len() {
                *posting = unsafe {
                    std::ptr::read(block_data[offset..].as_ptr().cast())
                };
            }
        }
    }

    postings
}

/// Read a dict entry from segment data at the given offset.
pub fn read_dict_entry(data: &[u8], offset: usize) -> Option<TpDictEntry> {
    let size = std::mem::size_of::<TpDictEntry>();
    if offset + size > data.len() {
        return None;
    }
    let entry: TpDictEntry =
        unsafe { std::ptr::read(data[offset..].as_ptr().cast()) };
    Some(entry)
}

// --- FFI exports ---

/// Read a posting block from segment data.
///
/// # Safety
/// `data` must point to valid block data of sufficient length.
/// `out` must have space for `doc_count` TpBlockPosting entries.
#[no_mangle]
pub unsafe extern "C" fn tp_rust_read_posting_block(
    data: *const u8,
    data_len: u32,
    doc_count: u8,
    flags: u8,
    out: *mut TpBlockPosting,
) -> u32 {
    if data.is_null() || out.is_null() || doc_count == 0 {
        return 0;
    }

    let data_slice =
        unsafe { std::slice::from_raw_parts(data, data_len as usize) };
    let count = doc_count as usize;

    let skip = TpSkipEntry {
        last_doc_id: 0,
        doc_count,
        block_max_tf: 0,
        block_max_norm: 0,
        posting_offset: 0,
        flags,
        reserved: [0; 3],
    };

    let postings = read_posting_block(data_slice, &skip);
    let out_slice =
        unsafe { std::slice::from_raw_parts_mut(out, count) };
    let n = postings.len().min(count);
    out_slice[..n].copy_from_slice(&postings[..n]);

    n as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_header_invalid() {
        let data = vec![0u8; 10];
        assert!(read_header(&data).is_none());
    }

    #[test]
    fn test_read_skip_entry() {
        let entry = TpSkipEntry {
            last_doc_id: 42,
            doc_count: 10,
            block_max_tf: 5,
            block_max_norm: 30,
            posting_offset: 1000,
            flags: TP_BLOCK_FLAG_DELTA,
            reserved: [0; 3],
        };
        let bytes: [u8; 16] = unsafe { std::mem::transmute(entry) };

        let result = read_skip_entry(&bytes, 0);
        assert!(result.is_some());
        let e = result.unwrap();
        let last_doc_id = e.last_doc_id;
        let doc_count = e.doc_count;
        let flags = e.flags;
        assert_eq!(last_doc_id, 42);
        assert_eq!(doc_count, 10);
        assert_eq!(flags, TP_BLOCK_FLAG_DELTA);
    }
}
