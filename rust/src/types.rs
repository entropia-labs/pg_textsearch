/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * Shared types that mirror C structures across the FFI boundary.
 */

/// Block posting entry - 8 bytes, matches C TpBlockPosting.
///
/// Used in uncompressed blocks. doc_id is segment-local.
/// Fieldnorm is stored inline to avoid per-posting buffer lookups.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TpBlockPosting {
    pub doc_id: u32,
    pub frequency: u16,
    pub fieldnorm: u8,
    pub reserved: u8,
}

/// Compressed block header - 2 bytes, matches C TpCompressedBlockHeader.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TpCompressedBlockHeader {
    pub doc_id_bits: u8,
    pub freq_bits: u8,
}

/// Maximum documents per block.
pub const TP_BLOCK_SIZE: u32 = 128;

/// Maximum compressed block size (for buffer allocation).
/// Header (2) + max doc_id bits (32*128/8=512) + max freq bits
/// (16*128/8=256) + fieldnorms (128) = 898 bytes.
pub const TP_MAX_COMPRESSED_BLOCK_SIZE: usize = 898;
