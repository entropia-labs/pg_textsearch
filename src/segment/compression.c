/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * compression.c - Block compression for posting lists
 *
 * Thin C wrapper that delegates to the Rust implementation.
 * The actual delta encoding + bitpacking logic lives in
 * rust/src/compression.rs.
 */
#include <postgres.h>

#include "compression.h"

/* Rust FFI declarations (implemented in rust/src/compression.rs) */
extern uint32 tp_rust_compress_block(
		const TpBlockPosting *postings, uint32 count, uint8 *out_buf);
extern void tp_rust_decompress_block(
		const uint8	   *compressed,
		uint32			count,
		uint32			first_doc_id,
		TpBlockPosting *out_postings);
extern uint8 tp_rust_compute_bit_width(uint32 max_value);
extern uint32
tp_rust_compressed_block_size(const uint8 *compressed, uint32 count);

uint8
tp_compute_bit_width(uint32 max_value)
{
	return tp_rust_compute_bit_width(max_value);
}

uint32
tp_compress_block(TpBlockPosting *postings, uint32 count, uint8 *out_buf)
{
	return tp_rust_compress_block(postings, count, out_buf);
}

void
tp_decompress_block(
		const uint8	   *compressed,
		uint32			count,
		uint32			first_doc_id,
		TpBlockPosting *out_postings)
{
	tp_rust_decompress_block(compressed, count, first_doc_id, out_postings);
}

uint32
tp_compressed_block_size(const uint8 *compressed, uint32 count)
{
	return tp_rust_compressed_block_size(compressed, count);
}
