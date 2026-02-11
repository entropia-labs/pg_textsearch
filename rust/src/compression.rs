/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * compression.rs - Block compression for posting lists
 *
 * Implements delta encoding + bitpacking for posting list compression.
 * This is a scalar implementation; SIMD optimization can be added later.
 */

use crate::types::{TpBlockPosting, TpCompressedBlockHeader, TP_BLOCK_SIZE};

/// Compute minimum bits needed to represent a value.
/// Returns 1 for 0 (need at least 1 bit), otherwise ceil(log2(value+1)).
pub fn compute_bit_width(max_value: u32) -> u8 {
    if max_value == 0 {
        return 1;
    }
    let mut bits: u8 = 1;
    while bits < 32 && (1u32 << bits) <= max_value {
        bits += 1;
    }
    bits
}

/// Pack an array of values into a bit stream.
/// Returns number of bytes written.
fn bitpack_encode(values: &[u32], bits: u8, out: &mut [u8]) -> usize {
    let mut buffer: u64 = 0;
    let mut buf_bits: u32 = 0;
    let mut out_pos: usize = 0;
    let mask: u32 = if bits == 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    };

    for &val in values {
        buffer |= ((val & mask) as u64) << buf_bits;
        buf_bits += bits as u32;

        while buf_bits >= 8 {
            out[out_pos] = (buffer & 0xFF) as u8;
            out_pos += 1;
            buffer >>= 8;
            buf_bits -= 8;
        }
    }

    if buf_bits > 0 {
        out[out_pos] = (buffer & 0xFF) as u8;
        out_pos += 1;
    }

    out_pos
}

/// Unpack a bit stream into an array of values.
fn bitpack_decode(input: &[u8], count: usize, bits: u8, out: &mut [u32]) {
    let mut buffer: u64 = 0;
    let mut buf_bits: u32 = 0;
    let mut in_pos: usize = 0;
    let mask: u32 = if bits == 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    };

    for item in out.iter_mut().take(count) {
        while buf_bits < bits as u32 {
            buffer |= (input[in_pos] as u64) << buf_bits;
            in_pos += 1;
            buf_bits += 8;
        }

        *item = (buffer as u32) & mask;
        buffer >>= bits;
        buf_bits -= bits as u32;
    }
}

/// Compress a block of postings.
///
/// Steps:
/// 1. Delta-encode doc IDs (first doc ID stored as-is, rest as deltas)
/// 2. Find max delta and max frequency to determine bit widths
/// 3. Bitpack deltas and frequencies
/// 4. Copy fieldnorms as-is
///
/// Returns number of bytes written to `out_buf`.
pub fn compress_block(postings: &[TpBlockPosting], out_buf: &mut [u8]) -> usize {
    let count = postings.len();
    assert!(count as u32 <= TP_BLOCK_SIZE);

    if count == 0 {
        return 0;
    }

    let mut doc_deltas = vec![0u32; count];
    let mut frequencies = vec![0u32; count];
    let mut max_delta: u32 = 0;
    let mut max_freq: u32 = 0;
    let mut prev_doc: u32 = 0;

    for i in 0..count {
        let doc_id = postings[i].doc_id;
        let delta = doc_id - prev_doc;

        doc_deltas[i] = delta;
        frequencies[i] = postings[i].frequency as u32;

        if delta > max_delta {
            max_delta = delta;
        }
        if frequencies[i] > max_freq {
            max_freq = frequencies[i];
        }

        prev_doc = doc_id;
    }

    // Write header
    let doc_id_bits = compute_bit_width(max_delta);
    let freq_bits = compute_bit_width(max_freq);
    out_buf[0] = doc_id_bits;
    out_buf[1] = freq_bits;
    let mut out_pos = std::mem::size_of::<TpCompressedBlockHeader>();

    // Bitpack doc ID deltas
    out_pos += bitpack_encode(&doc_deltas, doc_id_bits, &mut out_buf[out_pos..]);

    // Bitpack frequencies
    out_pos += bitpack_encode(&frequencies, freq_bits, &mut out_buf[out_pos..]);

    // Copy fieldnorms as-is (1 byte each)
    for posting in postings.iter().take(count) {
        out_buf[out_pos] = posting.fieldnorm;
        out_pos += 1;
    }

    out_pos
}

/// Decompress a block of postings.
///
/// `first_doc_id` is the base for delta decoding. For the first block
/// of a term, pass 0. The first delta IS the first absolute doc ID.
pub fn decompress_block(
    compressed: &[u8],
    count: usize,
    first_doc_id: u32,
    out_postings: &mut [TpBlockPosting],
) {
    assert!(count as u32 <= TP_BLOCK_SIZE);

    if count == 0 {
        return;
    }

    let doc_id_bits = compressed[0];
    let freq_bits = compressed[1];

    debug_assert!((1..=32).contains(&doc_id_bits));
    debug_assert!((1..=16).contains(&freq_bits));

    let header_size = std::mem::size_of::<TpCompressedBlockHeader>();

    let mut doc_deltas = vec![0u32; count];
    let mut frequencies = vec![0u32; count];

    let doc_id_bytes = (count as u32 * doc_id_bits as u32).div_ceil(8);
    let freq_bytes = (count as u32 * freq_bits as u32).div_ceil(8);

    // Decode doc ID deltas
    bitpack_decode(
        &compressed[header_size..],
        count,
        doc_id_bits,
        &mut doc_deltas,
    );
    let freq_start = header_size + doc_id_bytes as usize;

    // Decode frequencies
    bitpack_decode(&compressed[freq_start..], count, freq_bits, &mut frequencies);
    let fieldnorm_start = freq_start + freq_bytes as usize;

    // Reconstruct postings with absolute doc IDs
    let mut prev_doc = first_doc_id;
    for i in 0..count {
        let doc_id = prev_doc + doc_deltas[i];
        out_postings[i] = TpBlockPosting {
            doc_id,
            frequency: frequencies[i] as u16,
            fieldnorm: compressed[fieldnorm_start + i],
            reserved: 0,
        };
        prev_doc = doc_id;
    }
}

/// Get the size of compressed data without decompressing.
pub fn compressed_block_size(compressed: &[u8], count: u32) -> u32 {
    if count == 0 {
        return 0;
    }

    let doc_id_bits = compressed[0] as u32;
    let freq_bits = compressed[1] as u32;

    let header_size = std::mem::size_of::<TpCompressedBlockHeader>() as u32;
    let doc_id_bytes = (count * doc_id_bits).div_ceil(8);
    let freq_bytes = (count * freq_bits).div_ceil(8);

    header_size + doc_id_bytes + freq_bytes + count
}

// --- FFI exports ---

/// Compress a block of postings (FFI entry point).
///
/// # Safety
/// `postings` must point to `count` valid TpBlockPosting entries.
/// `out_buf` must have at least TP_MAX_COMPRESSED_BLOCK_SIZE bytes.
#[no_mangle]
pub unsafe extern "C" fn tp_rust_compress_block(
    postings: *const TpBlockPosting,
    count: u32,
    out_buf: *mut u8,
) -> u32 {
    if count == 0 || postings.is_null() || out_buf.is_null() {
        return 0;
    }
    let postings_slice =
        unsafe { std::slice::from_raw_parts(postings, count as usize) };
    let out_slice = unsafe {
        std::slice::from_raw_parts_mut(
            out_buf,
            crate::types::TP_MAX_COMPRESSED_BLOCK_SIZE,
        )
    };
    compress_block(postings_slice, out_slice) as u32
}

/// Decompress a block of postings (FFI entry point).
///
/// # Safety
/// `compressed` must point to valid compressed data.
/// `out_postings` must have space for `count` TpBlockPosting entries.
#[no_mangle]
pub unsafe extern "C" fn tp_rust_decompress_block(
    compressed: *const u8,
    count: u32,
    first_doc_id: u32,
    out_postings: *mut TpBlockPosting,
) {
    if count == 0 || compressed.is_null() || out_postings.is_null() {
        return;
    }
    // Upper bound on compressed size for the slice
    let max_compressed_len = crate::types::TP_MAX_COMPRESSED_BLOCK_SIZE;
    let compressed_slice =
        unsafe { std::slice::from_raw_parts(compressed, max_compressed_len) };
    let out_slice =
        unsafe { std::slice::from_raw_parts_mut(out_postings, count as usize) };
    decompress_block(compressed_slice, count as usize, first_doc_id, out_slice);
}

/// Compute minimum bits needed to represent a value (FFI entry point).
#[no_mangle]
pub extern "C" fn tp_rust_compute_bit_width(max_value: u32) -> u8 {
    compute_bit_width(max_value)
}

/// Get compressed block size (FFI entry point).
///
/// # Safety
/// `compressed` must point to valid compressed data (at least 2 bytes).
#[no_mangle]
pub unsafe extern "C" fn tp_rust_compressed_block_size(
    compressed: *const u8,
    count: u32,
) -> u32 {
    if count == 0 || compressed.is_null() {
        return 0;
    }
    let compressed_slice = unsafe { std::slice::from_raw_parts(compressed, 2) };
    compressed_block_size(compressed_slice, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TP_MAX_COMPRESSED_BLOCK_SIZE;

    #[test]
    fn test_compute_bit_width() {
        assert_eq!(compute_bit_width(0), 1);
        assert_eq!(compute_bit_width(1), 1);
        assert_eq!(compute_bit_width(2), 2);
        assert_eq!(compute_bit_width(3), 2);
        assert_eq!(compute_bit_width(4), 3);
        assert_eq!(compute_bit_width(7), 3);
        assert_eq!(compute_bit_width(8), 4);
        assert_eq!(compute_bit_width(255), 8);
        assert_eq!(compute_bit_width(256), 9);
        assert_eq!(compute_bit_width(u32::MAX), 32);
    }

    #[test]
    fn test_bitpack_roundtrip() {
        let values = [1u32, 3, 7, 2, 5, 0, 6, 4];
        let bits = 3u8;
        let mut packed = vec![0u8; 10];
        let written = bitpack_encode(&values, bits, &mut packed);
        assert!(written > 0);

        let mut decoded = vec![0u32; 8];
        bitpack_decode(&packed, 8, bits, &mut decoded);
        assert_eq!(decoded, values);
    }

    #[test]
    fn test_compress_decompress_roundtrip() {
        let postings: Vec<TpBlockPosting> = (0..128)
            .map(|i| TpBlockPosting {
                doc_id: i * 3 + 1,
                frequency: (i % 10 + 1) as u16,
                fieldnorm: (i % 200) as u8,
                reserved: 0,
            })
            .collect();

        let mut compressed = vec![0u8; TP_MAX_COMPRESSED_BLOCK_SIZE];
        let compressed_len = compress_block(&postings, &mut compressed);
        assert!(compressed_len > 0);
        assert!(compressed_len < TP_MAX_COMPRESSED_BLOCK_SIZE);

        let mut decompressed = vec![TpBlockPosting::default(); 128];
        decompress_block(&compressed, 128, 0, &mut decompressed);

        for (i, (orig, dec)) in
            postings.iter().zip(decompressed.iter()).enumerate()
        {
            assert_eq!(
                orig.doc_id, dec.doc_id,
                "doc_id mismatch at index {}",
                i
            );
            assert_eq!(
                orig.frequency, dec.frequency,
                "frequency mismatch at index {}",
                i
            );
            assert_eq!(
                orig.fieldnorm, dec.fieldnorm,
                "fieldnorm mismatch at index {}",
                i
            );
        }
    }

    #[test]
    fn test_compress_small_block() {
        let postings = vec![
            TpBlockPosting {
                doc_id: 5,
                frequency: 3,
                fieldnorm: 42,
                reserved: 0,
            },
            TpBlockPosting {
                doc_id: 10,
                frequency: 1,
                fieldnorm: 55,
                reserved: 0,
            },
        ];

        let mut compressed = vec![0u8; TP_MAX_COMPRESSED_BLOCK_SIZE];
        let compressed_len = compress_block(&postings, &mut compressed);
        assert!(compressed_len > 0);

        let mut decompressed = vec![TpBlockPosting::default(); 2];
        decompress_block(&compressed, 2, 0, &mut decompressed);

        assert_eq!(decompressed[0].doc_id, 5);
        assert_eq!(decompressed[0].frequency, 3);
        assert_eq!(decompressed[0].fieldnorm, 42);
        assert_eq!(decompressed[1].doc_id, 10);
        assert_eq!(decompressed[1].frequency, 1);
        assert_eq!(decompressed[1].fieldnorm, 55);
    }

    #[test]
    fn test_compressed_block_size() {
        let postings: Vec<TpBlockPosting> = (0..64)
            .map(|i| TpBlockPosting {
                doc_id: i * 2,
                frequency: 1,
                fieldnorm: 10,
                reserved: 0,
            })
            .collect();

        let mut compressed = vec![0u8; TP_MAX_COMPRESSED_BLOCK_SIZE];
        let actual_len = compress_block(&postings, &mut compressed);
        let computed_len = compressed_block_size(&compressed, 64);

        assert_eq!(actual_len as u32, computed_len);
    }

    #[test]
    fn test_compress_empty() {
        let mut compressed = vec![0u8; TP_MAX_COMPRESSED_BLOCK_SIZE];
        assert_eq!(compress_block(&[], &mut compressed), 0);
    }

    #[test]
    fn test_compress_single_posting() {
        let postings = vec![TpBlockPosting {
            doc_id: 42,
            frequency: 7,
            fieldnorm: 100,
            reserved: 0,
        }];

        let mut compressed = vec![0u8; TP_MAX_COMPRESSED_BLOCK_SIZE];
        let compressed_len = compress_block(&postings, &mut compressed);
        assert!(compressed_len > 0);

        let mut decompressed = vec![TpBlockPosting::default(); 1];
        decompress_block(&compressed, 1, 0, &mut decompressed);

        assert_eq!(decompressed[0].doc_id, 42);
        assert_eq!(decompressed[0].frequency, 7);
        assert_eq!(decompressed[0].fieldnorm, 100);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::types::TP_MAX_COMPRESSED_BLOCK_SIZE;
    use proptest::prelude::*;

    prop_compose! {
        fn arb_posting(max_doc_id: u32)(
            doc_id in 0..=max_doc_id,
            frequency in 1..=1000u16,
            fieldnorm in 0..=255u8,
        ) -> TpBlockPosting {
            TpBlockPosting { doc_id, frequency, fieldnorm, reserved: 0 }
        }
    }

    fn arb_sorted_postings(
        count: usize,
    ) -> impl Strategy<Value = Vec<TpBlockPosting>> {
        prop::collection::vec(
            (1..=100u32, 1..=1000u16, 0..=255u8),
            count..=count,
        )
        .prop_map(|tuples| {
            let mut doc_id = 0u32;
            tuples
                .into_iter()
                .map(|(gap, freq, norm)| {
                    doc_id += gap;
                    TpBlockPosting {
                        doc_id,
                        frequency: freq,
                        fieldnorm: norm,
                        reserved: 0,
                    }
                })
                .collect()
        })
    }

    proptest! {
        #[test]
        fn roundtrip_any_block(postings in arb_sorted_postings(128)) {
            let mut compressed = vec![0u8; TP_MAX_COMPRESSED_BLOCK_SIZE];
            let len = compress_block(&postings, &mut compressed);
            prop_assert!(len > 0);

            let mut decompressed = vec![TpBlockPosting::default(); 128];
            decompress_block(&compressed, 128, 0, &mut decompressed);

            for (i, (orig, dec)) in postings.iter().zip(decompressed.iter()).enumerate() {
                prop_assert_eq!(orig.doc_id, dec.doc_id, "doc_id at {}", i);
                prop_assert_eq!(orig.frequency, dec.frequency, "freq at {}", i);
                prop_assert_eq!(orig.fieldnorm, dec.fieldnorm, "norm at {}", i);
            }
        }

        #[test]
        fn roundtrip_partial_block(
            count in 1..=128usize,
        ) {
            let postings: Vec<TpBlockPosting> = (0..count).map(|i| {
                TpBlockPosting {
                    doc_id: (i as u32) * 5 + 1,
                    frequency: ((i % 10) + 1) as u16,
                    fieldnorm: (i % 256) as u8,
                    reserved: 0,
                }
            }).collect();

            let mut compressed = vec![0u8; TP_MAX_COMPRESSED_BLOCK_SIZE];
            let len = compress_block(&postings, &mut compressed);
            prop_assert!(len > 0);

            let computed_size = compressed_block_size(&compressed, count as u32);
            prop_assert_eq!(len as u32, computed_size);

            let mut decompressed = vec![TpBlockPosting::default(); count];
            decompress_block(&compressed, count, 0, &mut decompressed);

            for (i, (orig, dec)) in postings.iter().zip(decompressed.iter()).enumerate() {
                prop_assert_eq!(orig.doc_id, dec.doc_id, "doc_id at {}", i);
                prop_assert_eq!(orig.frequency, dec.frequency, "freq at {}", i);
                prop_assert_eq!(orig.fieldnorm, dec.fieldnorm, "norm at {}", i);
            }
        }

        #[test]
        fn bit_width_correct(value in 0..=u32::MAX) {
            let bits = compute_bit_width(value);
            prop_assert!(bits >= 1);
            prop_assert!(bits <= 32);
            if bits < 32 {
                prop_assert!(value < (1u32 << bits), "value {} >= 2^{}", value, bits);
            }
        }
    }
}
