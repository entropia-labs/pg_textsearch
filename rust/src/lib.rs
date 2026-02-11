/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * pg_textsearch Rust core library.
 *
 * This crate is compiled as a static library and linked into the
 * pg_textsearch PostgreSQL extension. It provides Rust implementations
 * of performance-critical components behind a C FFI boundary.
 */

pub mod bmw;
pub mod compression;
pub mod fieldnorm;
pub mod scoring;
pub mod segment;
pub mod topk;
pub mod types;

/// Return the Rust library version as a packed integer.
/// Major * 10000 + Minor * 100 + Patch (e.g. 0.1.0 = 100).
#[no_mangle]
pub extern "C" fn tp_rust_version() -> u32 {
    let major: u32 = env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap_or(0);
    let minor: u32 = env!("CARGO_PKG_VERSION_MINOR").parse().unwrap_or(0);
    let patch: u32 = env!("CARGO_PKG_VERSION_PATCH").parse().unwrap_or(0);
    major * 10000 + minor * 100 + patch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert_eq!(tp_rust_version(), 100); // 0.1.0
    }
}
