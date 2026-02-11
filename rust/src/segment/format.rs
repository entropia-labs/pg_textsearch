/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * format.rs - Segment format structure definitions
 *
 * Mirrors the C structures in segment.h for FFI compatibility.
 * All structs use #[repr(C)] to match C memory layout exactly.
 *
 * Supports both V3 (single-tenant) and V4 (multi-tenant) formats.
 * V3 segments remain readable; V4 adds tenant-aware sections.
 */

/// Segment magic number: "TPSG" in ASCII.
pub const TP_SEGMENT_MAGIC: u32 = 0x5450_5347;

/// Segment format V3: block compression.
pub const TP_SEGMENT_FORMAT_V3: u32 = 3;

/// Segment format V4: multi-tenant support.
pub const TP_SEGMENT_FORMAT_V4: u32 = 4;

/// Current segment format version.
pub const TP_SEGMENT_FORMAT_VERSION: u32 = TP_SEGMENT_FORMAT_V4;

/// Documents per block.
pub const TP_BLOCK_SIZE: u32 = 128;

/// Block compression flags.
pub const TP_BLOCK_FLAG_UNCOMPRESSED: u8 = 0x00;
pub const TP_BLOCK_FLAG_DELTA: u8 = 0x01;

/// Tenant mode values stored in skip entry reserved[0].
pub const TP_TENANT_MODE_NONE: u8 = 0;
pub const TP_TENANT_MODE_SINGLE: u8 = 1;
pub const TP_TENANT_MODE_MIXED: u8 = 2;

/// V4 header flags.
pub const TP_FLAG_HAS_TENANT_DATA: u32 = 0x01;

/// Segment header V3 - stored on the first page.
/// Matches C TpSegmentHeader exactly for V3 format.
#[repr(C)]
#[derive(Clone, Debug)]
pub struct TpSegmentHeader {
    pub magic: u32,
    pub version: u32,
    pub created_at: i64, // TimestampTz = int64
    pub num_pages: u32,
    pub data_size: u32,
    pub level: u32,
    pub next_segment: u32, // BlockNumber

    // Section offsets in logical file
    pub dictionary_offset: u32,
    pub strings_offset: u32,
    pub entries_offset: u32,
    pub postings_offset: u32,
    pub skip_index_offset: u32,
    pub fieldnorm_offset: u32,
    pub ctid_pages_offset: u32,
    pub ctid_offsets_offset: u32,

    // Corpus statistics
    pub num_terms: u32,
    pub num_docs: u32,
    pub total_tokens: u64,

    // Page index reference
    pub page_index: u32, // BlockNumber

    // --- V4 extensions (appended after V3 fields) ---
    /// Offset to tenant_id table: uint32[num_docs].
    pub tenant_map_offset: u32,
    /// Offset to per-tenant stats: TpTenantStats[].
    pub tenant_stats_offset: u32,
    /// Offset to per-term per-tenant doc_freq table.
    pub tenant_docfreq_offset: u32,
    /// Flags: bit 0 = has_tenant_data.
    pub flags: u32,
}

impl TpSegmentHeader {
    /// True if this segment contains tenant data.
    pub fn has_tenant_data(&self) -> bool {
        self.flags & TP_FLAG_HAS_TENANT_DATA != 0
    }

    /// True if this is a V4 (or later) segment.
    pub fn is_v4(&self) -> bool {
        self.version >= TP_SEGMENT_FORMAT_V4
    }
}

/// Dictionary entry - 12 bytes per term.
/// Points to skip index and stores doc frequency.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TpDictEntry {
    pub skip_index_offset: u32,
    pub block_count: u16,
    pub reserved: u16,
    pub doc_freq: u32,
}

/// Skip index entry - 16 bytes per block.
/// Stores block metadata for BMW optimization.
///
/// In V4, the reserved[3] bytes encode tenant info:
///   reserved[0] = tenant_mode (0=none, 1=single, 2=mixed)
///   reserved[1..2] = tenant_id_low16 (lower 16 bits of tenant_id)
///
/// When tenant_mode == SINGLE and tenant_id matches the query filter,
/// the entire block belongs to that tenant. When tenant_mode == MIXED,
/// per-doc filtering via the tenant_map table is required.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TpSkipEntry {
    pub last_doc_id: u32,
    pub doc_count: u8,
    pub block_max_tf: u16,
    pub block_max_norm: u8,
    pub posting_offset: u32,
    pub flags: u8,
    pub reserved: [u8; 3],
}

impl TpSkipEntry {
    /// Get tenant mode from reserved bytes.
    pub fn tenant_mode(&self) -> u8 {
        self.reserved[0]
    }

    /// Get lower 16 bits of tenant_id from reserved bytes.
    pub fn tenant_id_low16(&self) -> u16 {
        u16::from_le_bytes([self.reserved[1], self.reserved[2]])
    }

    /// Set tenant info in reserved bytes.
    pub fn set_tenant_info(&mut self, mode: u8, tenant_id: u32) {
        self.reserved[0] = mode;
        let low16 = (tenant_id & 0xFFFF) as u16;
        let bytes = low16.to_le_bytes();
        self.reserved[1] = bytes[0];
        self.reserved[2] = bytes[1];
    }

    /// Check if this block can be skipped for a tenant filter.
    /// Returns true if the block definitely has no docs for target.
    pub fn can_skip_for_tenant(&self, target_tenant_id: u32) -> bool {
        let mode = self.tenant_mode();
        if mode == TP_TENANT_MODE_SINGLE {
            let low16 = self.tenant_id_low16();
            // If target tenant's low 16 bits differ, block is
            // definitely for a different tenant.
            (target_tenant_id & 0xFFFF) as u16 != low16
        } else {
            // NONE or MIXED: can't skip
            false
        }
    }
}

/// Per-tenant corpus statistics.
/// Stored in the tenant_stats section of V4 segments.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TpTenantStats {
    pub tenant_id: u32,
    pub num_docs: u32,
    pub total_tokens: u64,
}

/// Per-term per-tenant document frequency entry.
/// Stored in the tenant_docfreq section of V4 segments.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TpTenantDocFreq {
    pub tenant_id: u32,
    pub doc_freq: u32,
}

/// Information about a term during segment building.
#[derive(Clone, Debug)]
pub struct TermInfo {
    pub term: String,
    pub postings: Vec<PostingEntry>,
    pub doc_freq: u32,
}

/// A posting entry before block grouping.
#[derive(Clone, Copy, Debug)]
pub struct PostingEntry {
    pub doc_id: u32,
    pub frequency: u16,
    pub fieldnorm: u8,
    /// Tenant ID (0 if no tenancy). V4 only.
    pub tenant_id: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dict_entry_size() {
        assert_eq!(std::mem::size_of::<TpDictEntry>(), 12);
    }

    #[test]
    fn test_skip_entry_size() {
        assert_eq!(std::mem::size_of::<TpSkipEntry>(), 16);
    }

    #[test]
    fn test_tenant_stats_size() {
        assert_eq!(std::mem::size_of::<TpTenantStats>(), 16);
    }

    #[test]
    fn test_tenant_docfreq_size() {
        assert_eq!(std::mem::size_of::<TpTenantDocFreq>(), 8);
    }

    #[test]
    fn test_skip_entry_tenant_info() {
        let mut skip = TpSkipEntry::default();
        skip.set_tenant_info(TP_TENANT_MODE_SINGLE, 42);
        assert_eq!(skip.tenant_mode(), TP_TENANT_MODE_SINGLE);
        assert_eq!(skip.tenant_id_low16(), 42);
        assert!(skip.can_skip_for_tenant(99));
        assert!(!skip.can_skip_for_tenant(42));
    }

    #[test]
    fn test_skip_entry_tenant_mixed_no_skip() {
        let mut skip = TpSkipEntry::default();
        skip.set_tenant_info(TP_TENANT_MODE_MIXED, 0);
        assert!(!skip.can_skip_for_tenant(42));
    }

    #[test]
    fn test_header_v4_flags() {
        let mut header = TpSegmentHeader {
            magic: TP_SEGMENT_MAGIC,
            version: TP_SEGMENT_FORMAT_V4,
            created_at: 0,
            num_pages: 0,
            data_size: 0,
            level: 0,
            next_segment: 0,
            dictionary_offset: 0,
            strings_offset: 0,
            entries_offset: 0,
            postings_offset: 0,
            skip_index_offset: 0,
            fieldnorm_offset: 0,
            ctid_pages_offset: 0,
            ctid_offsets_offset: 0,
            num_terms: 0,
            num_docs: 0,
            total_tokens: 0,
            page_index: 0,
            tenant_map_offset: 0,
            tenant_stats_offset: 0,
            tenant_docfreq_offset: 0,
            flags: 0,
        };
        assert!(header.is_v4());
        assert!(!header.has_tenant_data());

        header.flags = TP_FLAG_HAS_TENANT_DATA;
        assert!(header.has_tenant_data());
    }
}
