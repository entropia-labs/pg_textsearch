/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * fieldnorm.h - Document length quantization for BM25 scoring
 *
 * Uses Lucene's SmallFloat encoding scheme which maps document lengths
 * to a single byte (256 buckets). This reduces storage while maintaining
 * good BM25 ranking quality because:
 * - BM25 uses the ratio dl/avgdl, not absolute length
 * - Small errors become smaller in the ratio
 * - The b parameter (0.75) further dampens length's influence
 *
 * Key properties:
 * - Lengths 0-39 stored exactly (covers most short documents)
 * - Larger lengths use 4-bit mantissa (~6% relative error max)
 * - 256 buckets cover lengths from 0 to 2+ billion
 *
 * Implementation lives in rust/src/fieldnorm.rs.
 */
#pragma once

#include "postgres.h"

/* Rust FFI declarations (implemented in rust/src/fieldnorm.rs) */
extern uint8  tp_rust_encode_fieldnorm(uint32 length);
extern uint32 tp_rust_decode_fieldnorm(uint8 norm_id);

/*
 * Encode document length to fieldnorm byte.
 *
 * Finds the largest index i where decode_table[i] <= length.
 * This matches Tantivy's fieldnorm_to_id implementation.
 */
static inline uint8
encode_fieldnorm(uint32 length)
{
	return tp_rust_encode_fieldnorm(length);
}

/*
 * Decode fieldnorm byte back to approximate document length
 */
static inline uint32
decode_fieldnorm(uint8 norm_id)
{
	return tp_rust_decode_fieldnorm(norm_id);
}
