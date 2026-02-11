/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * segment/ - Disk-based segment format handling
 *
 * This module handles the byte-level encoding/decoding of segment
 * data. The C side handles Postgres page I/O; Rust handles format
 * serialization.
 */

pub mod dictionary;
pub mod docmap;
pub mod format;
pub mod merge;
pub mod reader;
pub mod writer;
