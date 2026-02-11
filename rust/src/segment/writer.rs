/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * writer.rs - Segment writer producing byte streams
 *
 * The writer serializes segment data into a byte buffer. The C side
 * takes these bytes and writes them to Postgres pages via the
 * existing tp_segment_writer_write() infrastructure.
 */

use crate::compression;
use crate::fieldnorm::encode_fieldnorm;
use crate::segment::format::*;
use crate::types::{TpBlockPosting, TP_MAX_COMPRESSED_BLOCK_SIZE};

/// Accumulates segment data as bytes during a build.
///
/// After all postings are written, call `finish()` to get the
/// final TpSegmentHeader with correct offsets.
pub struct SegmentBuilder {
    /// Running byte buffer for the segment.
    buffer: Vec<u8>,

    /// Accumulated skip entries across all terms.
    skip_entries: Vec<TpSkipEntry>,

    /// Dictionary entries parallel to terms.
    dict_entries: Vec<TpDictEntry>,

    /// Number of terms written.
    num_terms: u32,

    /// Number of documents in segment.
    num_docs: u32,

    /// Total tokens across all documents.
    total_tokens: u64,

    /// Whether compression is enabled.
    compress: bool,

    /// Section offsets (filled during build).
    dictionary_offset: u32,
    strings_offset: u32,
    entries_offset: u32,
    postings_offset: u32,
}

impl SegmentBuilder {
    pub fn new(compress: bool) -> Self {
        SegmentBuilder {
            buffer: Vec::new(),
            skip_entries: Vec::new(),
            dict_entries: Vec::new(),
            num_terms: 0,
            num_docs: 0,
            total_tokens: 0,
            compress,
            dictionary_offset: 0,
            strings_offset: 0,
            entries_offset: 0,
            postings_offset: 0,
        }
    }

    /// Write the segment header placeholder.
    /// Returns the header offset (always 0).
    pub fn write_header_placeholder(&mut self) -> usize {
        let header_size = std::mem::size_of::<TpSegmentHeader>();
        self.buffer.resize(header_size, 0);
        0
    }

    /// Write the dictionary section (term offsets).
    pub fn write_dictionary(&mut self, terms: &[&str]) {
        self.dictionary_offset = self.buffer.len() as u32;
        self.num_terms = terms.len() as u32;

        // Write num_terms
        self.buffer
            .extend_from_slice(&self.num_terms.to_ne_bytes());

        // Write string offsets (relative to strings section)
        let mut string_offset = 0u32;
        for term in terms {
            self.buffer
                .extend_from_slice(&string_offset.to_ne_bytes());
            // Each string entry: [len:4][text:len][entry_offset:4]
            string_offset += 4 + term.len() as u32 + 4;
        }
    }

    /// Write the string pool section.
    pub fn write_strings(&mut self, terms: &[&str]) {
        self.strings_offset = self.buffer.len() as u32;

        for (i, term) in terms.iter().enumerate() {
            let len = term.len() as u32;
            self.buffer.extend_from_slice(&len.to_ne_bytes());
            self.buffer.extend_from_slice(term.as_bytes());
            let entry_offset =
                (i * std::mem::size_of::<TpDictEntry>()) as u32;
            self.buffer
                .extend_from_slice(&entry_offset.to_ne_bytes());
        }
    }

    /// Reserve space for dictionary entries (filled after postings).
    pub fn reserve_entries(&mut self, num_terms: usize) {
        self.entries_offset = self.buffer.len() as u32;
        let size = num_terms * std::mem::size_of::<TpDictEntry>();
        self.buffer.resize(self.buffer.len() + size, 0);
    }

    /// Write posting blocks for a term.
    ///
    /// Postings must be sorted by doc_id. Groups into blocks of
    /// TP_BLOCK_SIZE, compresses if enabled, and accumulates skip
    /// entries.
    pub fn write_term_postings(
        &mut self,
        postings: &[PostingEntry],
    ) -> TpDictEntry {
        if self.postings_offset == 0 {
            self.postings_offset = self.buffer.len() as u32;
        }

        let mut block_count = 0u16;
        let skip_start = self.skip_entries.len();
        let mut block = Vec::with_capacity(TP_BLOCK_SIZE as usize);

        for posting in postings {
            block.push(TpBlockPosting {
                doc_id: posting.doc_id,
                frequency: posting.frequency,
                fieldnorm: posting.fieldnorm,
                reserved: 0,
            });

            if block.len() == TP_BLOCK_SIZE as usize {
                self.flush_block(&block);
                block.clear();
                block_count += 1;
            }
        }

        // Flush remaining partial block
        if !block.is_empty() {
            self.flush_block(&block);
            block_count += 1;
        }

        let entry = TpDictEntry {
            skip_index_offset: 0, // Set later when skip index is written
            block_count,
            reserved: 0,
            doc_freq: postings.len() as u32,
        };
        self.dict_entries.push(entry);

        // Update skip_index_offset for this term's first skip entry
        let mut result = entry;
        result.skip_index_offset =
            (skip_start * std::mem::size_of::<TpSkipEntry>()) as u32;
        result
    }

    /// Write the skip index section.
    pub fn write_skip_index(&mut self) -> u32 {
        let offset = self.buffer.len() as u32;
        for skip in &self.skip_entries {
            let bytes: [u8; 16] = unsafe { std::mem::transmute(*skip) };
            self.buffer.extend_from_slice(&bytes);
        }
        offset
    }

    /// Write the fieldnorm table.
    pub fn write_fieldnorms(&mut self, fieldnorms: &[u8]) -> u32 {
        let offset = self.buffer.len() as u32;
        self.buffer.extend_from_slice(fieldnorms);
        offset
    }

    /// Write the CTID pages array (BlockNumber per doc).
    pub fn write_ctid_pages(&mut self, pages: &[u32]) -> u32 {
        let offset = self.buffer.len() as u32;
        for &page in pages {
            self.buffer.extend_from_slice(&page.to_ne_bytes());
        }
        offset
    }

    /// Write the CTID offsets array (OffsetNumber per doc).
    pub fn write_ctid_offsets(&mut self, offsets: &[u16]) -> u32 {
        let offset = self.buffer.len() as u32;
        for &off in offsets {
            self.buffer.extend_from_slice(&off.to_ne_bytes());
        }
        offset
    }

    /// Write the tenant_id table (uint32 per doc). V4 only.
    pub fn write_tenant_map(&mut self, tenant_ids: &[u32]) -> u32 {
        let offset = self.buffer.len() as u32;
        for &tid in tenant_ids {
            self.buffer.extend_from_slice(&tid.to_ne_bytes());
        }
        offset
    }

    /// Write per-tenant stats section. V4 only.
    /// Format: num_tenants (u32) + TpTenantStats[num_tenants]
    pub fn write_tenant_stats(
        &mut self,
        stats: &[TpTenantStats],
    ) -> u32 {
        let offset = self.buffer.len() as u32;
        let count = stats.len() as u32;
        self.buffer.extend_from_slice(&count.to_ne_bytes());
        for stat in stats {
            let bytes: [u8; 16] = unsafe { std::mem::transmute(*stat) };
            self.buffer.extend_from_slice(&bytes);
        }
        offset
    }

    /// Write per-term per-tenant doc_freq section. V4 only.
    /// For each term (in dictionary order):
    ///   num_tenant_entries (u16) + TpTenantDocFreq[]
    pub fn write_tenant_docfreqs(
        &mut self,
        per_term_entries: &[Vec<TpTenantDocFreq>],
    ) -> u32 {
        let offset = self.buffer.len() as u32;
        for entries in per_term_entries {
            let count = entries.len() as u16;
            self.buffer.extend_from_slice(&count.to_ne_bytes());
            for entry in entries {
                let bytes: [u8; 8] =
                    unsafe { std::mem::transmute(*entry) };
                self.buffer.extend_from_slice(&bytes);
            }
        }
        offset
    }

    /// Finalize the segment: fill in header and dict entries.
    pub fn finish(
        &mut self,
        level: u32,
        next_segment: u32,
        skip_index_offset: u32,
        fieldnorm_offset: u32,
        ctid_pages_offset: u32,
        ctid_offsets_offset: u32,
    ) -> &[u8] {
        // Update dictionary entries in the buffer
        let entries_off = self.entries_offset as usize;
        let entry_size = std::mem::size_of::<TpDictEntry>();
        let mut skip_offset = 0u32;

        for (i, entry) in self.dict_entries.iter().enumerate() {
            let mut e = *entry;
            e.skip_index_offset = skip_offset;
            skip_offset +=
                entry.block_count as u32 * std::mem::size_of::<TpSkipEntry>() as u32;

            let pos = entries_off + i * entry_size;
            let bytes: [u8; 12] = unsafe { std::mem::transmute(e) };
            self.buffer[pos..pos + entry_size].copy_from_slice(&bytes);
        }

        // Write header at position 0
        let header = TpSegmentHeader {
            magic: TP_SEGMENT_MAGIC,
            version: TP_SEGMENT_FORMAT_VERSION,
            created_at: 0, // Set by C side
            num_pages: 0,  // Set by C side after page allocation
            data_size: self.buffer.len() as u32,
            level,
            next_segment,
            dictionary_offset: self.dictionary_offset,
            strings_offset: self.strings_offset,
            entries_offset: self.entries_offset,
            postings_offset: self.postings_offset,
            skip_index_offset,
            fieldnorm_offset,
            ctid_pages_offset,
            ctid_offsets_offset,
            num_terms: self.num_terms,
            num_docs: self.num_docs,
            total_tokens: self.total_tokens,
            page_index: 0, // Set by C side
            // V4 extensions
            tenant_map_offset: 0,
            tenant_stats_offset: 0,
            tenant_docfreq_offset: 0,
            flags: 0,
        };

        let header_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                &header as *const TpSegmentHeader as *const u8,
                std::mem::size_of::<TpSegmentHeader>(),
            )
        };
        let header_size = std::mem::size_of::<TpSegmentHeader>();
        self.buffer[..header_size].copy_from_slice(header_bytes);

        &self.buffer
    }

    /// Get the current buffer contents.
    pub fn data(&self) -> &[u8] {
        &self.buffer
    }

    /// Set corpus statistics.
    pub fn set_stats(&mut self, num_docs: u32, total_tokens: u64) {
        self.num_docs = num_docs;
        self.total_tokens = total_tokens;
    }

    // --- Internal helpers ---

    fn flush_block(&mut self, block: &[TpBlockPosting]) {
        let posting_offset = self.buffer.len() as u32;
        let count = block.len() as u8;

        // Compute block max TF and max norm
        let mut max_tf: u16 = 0;
        let mut max_norm: u8 = 0;
        let mut last_doc_id: u32 = 0;
        for posting in block {
            if posting.frequency > max_tf {
                max_tf = posting.frequency;
            }
            if posting.fieldnorm > max_norm {
                max_norm = posting.fieldnorm;
            }
            last_doc_id = posting.doc_id;
        }

        // Compress or write raw
        let flags = if self.compress && block.len() > 1 {
            let mut compressed = vec![0u8; TP_MAX_COMPRESSED_BLOCK_SIZE];
            let compressed_len =
                compression::compress_block(block, &mut compressed);
            self.buffer
                .extend_from_slice(&compressed[..compressed_len]);
            TP_BLOCK_FLAG_DELTA
        } else {
            // Write raw TpBlockPosting array
            for posting in block {
                let bytes: [u8; 8] =
                    unsafe { std::mem::transmute(*posting) };
                self.buffer.extend_from_slice(&bytes);
            }
            TP_BLOCK_FLAG_UNCOMPRESSED
        };

        // Create skip entry
        self.skip_entries.push(TpSkipEntry {
            last_doc_id,
            doc_count: count,
            block_max_tf: max_tf,
            block_max_norm: max_norm,
            posting_offset,
            flags,
            reserved: [0; 3],
        });
    }
}

/// Encode a document length into a fieldnorm byte.
pub fn encode_doc_length(length: u32) -> u8 {
    encode_fieldnorm(length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_segment_builder_basic() {
        let mut builder = SegmentBuilder::new(true);

        let terms = vec!["hello", "world"];
        builder.write_header_placeholder();
        builder.write_dictionary(&terms);
        builder.write_strings(&terms);
        builder.reserve_entries(terms.len());
        builder.set_stats(10, 500);

        // Write postings for "hello"
        let postings = vec![
            PostingEntry {
                doc_id: 0,
                frequency: 3,
                fieldnorm: 42,
                tenant_id: 0,
            },
            PostingEntry {
                doc_id: 5,
                frequency: 1,
                fieldnorm: 55,
                tenant_id: 0,
            },
        ];
        builder.write_term_postings(&postings);

        // Write postings for "world"
        let postings2 = vec![PostingEntry {
            doc_id: 2,
            frequency: 2,
            fieldnorm: 42,
            tenant_id: 0,
        }];
        builder.write_term_postings(&postings2);

        let skip_off = builder.write_skip_index();
        let fnorm_off = builder.write_fieldnorms(&[42, 55, 42, 0, 0, 5, 0, 0, 0, 0]);
        let ctid_pages_off = builder.write_ctid_pages(&[1, 1, 1, 2, 2, 2, 3, 3, 3, 3]);
        let ctid_off_off = builder.write_ctid_offsets(&[1, 2, 3, 1, 2, 3, 1, 2, 3, 4]);

        let data = builder.finish(0, 0, skip_off, fnorm_off, ctid_pages_off, ctid_off_off);
        assert!(!data.is_empty());

        // Verify header magic
        let magic = u32::from_ne_bytes(data[0..4].try_into().unwrap());
        assert_eq!(magic, TP_SEGMENT_MAGIC);
    }
}
