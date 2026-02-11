/*
 * Copyright (c) 2025-2026 Tiger Data, Inc.
 * Licensed under the PostgreSQL License. See LICENSE for details.
 *
 * dictionary.rs - Term dictionary encoding and lookup
 *
 * The dictionary is a sorted array of string offsets enabling binary
 * search. Each string entry stores: [length:u32][text:bytes][offset:u32]
 * where offset points to the corresponding TpDictEntry.
 */

use crate::segment::format::TpDictEntry;
use std::ffi::CStr;
use std::os::raw::c_char;

/// Encode a sorted list of terms into dictionary format.
///
/// Returns (dictionary_bytes, strings_bytes, entries_bytes):
/// - dictionary_bytes: num_terms (u32) + string_offsets (u32[])
/// - strings_bytes: length-prefixed strings with dict entry offsets
/// - entries_bytes: placeholder TpDictEntry[] (filled later by caller)
pub fn encode_dictionary(terms: &[&str]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let num_terms = terms.len() as u32;

    // Build string pool and compute offsets
    let mut strings_bytes = Vec::new();
    let mut string_offsets: Vec<u32> = Vec::with_capacity(terms.len());
    let entries_size =
        terms.len() * std::mem::size_of::<TpDictEntry>();

    for (i, term) in terms.iter().enumerate() {
        string_offsets.push(strings_bytes.len() as u32);

        // Write: [length:u32][text:bytes][dict_entry_offset:u32]
        let len = term.len() as u32;
        strings_bytes.extend_from_slice(&len.to_ne_bytes());
        strings_bytes.extend_from_slice(term.as_bytes());
        let entry_offset = (i * std::mem::size_of::<TpDictEntry>()) as u32;
        strings_bytes.extend_from_slice(&entry_offset.to_ne_bytes());
    }

    // Build dictionary header: num_terms + offsets array
    let mut dict_bytes =
        Vec::with_capacity(4 + terms.len() * 4);
    dict_bytes.extend_from_slice(&num_terms.to_ne_bytes());
    for offset in &string_offsets {
        dict_bytes.extend_from_slice(&offset.to_ne_bytes());
    }

    // Placeholder entries
    let entries_bytes = vec![0u8; entries_size];

    (dict_bytes, strings_bytes, entries_bytes)
}

/// Look up a term in the dictionary using binary search.
///
/// `data` contains the full segment data starting from the segment header.
/// `dict_offset` is the offset to the dictionary section.
/// `strings_offset` is the offset to the strings section.
/// `entries_offset` is the offset to the dict entries section.
///
/// Returns Some(TpDictEntry) if found, None otherwise.
pub fn lookup_term(
    data: &[u8],
    dict_offset: u32,
    strings_offset: u32,
    entries_offset: u32,
    term: &str,
) -> Option<TpDictEntry> {
    let dict_off = dict_offset as usize;
    if dict_off + 4 > data.len() {
        return None;
    }

    let num_terms =
        u32::from_ne_bytes(data[dict_off..dict_off + 4].try_into().ok()?)
            as usize;

    if num_terms == 0 {
        return None;
    }

    let offsets_start = dict_off + 4;
    let strings_off = strings_offset as usize;
    let entries_off = entries_offset as usize;

    // Binary search
    let mut lo = 0usize;
    let mut hi = num_terms - 1;

    while lo <= hi {
        let mid = lo + (hi - lo) / 2;

        // Read string offset for mid
        let off_pos = offsets_start + mid * 4;
        let str_rel_off = u32::from_ne_bytes(
            data[off_pos..off_pos + 4].try_into().ok()?,
        ) as usize;
        let str_pos = strings_off + str_rel_off;

        // Read string length
        let str_len = u32::from_ne_bytes(
            data[str_pos..str_pos + 4].try_into().ok()?,
        ) as usize;

        // Compare
        let str_data = &data[str_pos + 4..str_pos + 4 + str_len];
        let cmp = str_data.cmp(term.as_bytes());

        match cmp {
            std::cmp::Ordering::Equal => {
                // Read dict entry offset from after the string
                let entry_off_pos = str_pos + 4 + str_len;
                let entry_rel_off = u32::from_ne_bytes(
                    data[entry_off_pos..entry_off_pos + 4]
                        .try_into()
                        .ok()?,
                ) as usize;
                let entry_pos = entries_off + entry_rel_off;

                // Read TpDictEntry
                let entry_size = std::mem::size_of::<TpDictEntry>();
                if entry_pos + entry_size > data.len() {
                    return None;
                }
                let entry_bytes = &data[entry_pos..entry_pos + entry_size];
                let entry: TpDictEntry =
                    unsafe { std::ptr::read(entry_bytes.as_ptr().cast()) };
                return Some(entry);
            }
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => {
                if mid == 0 {
                    return None;
                }
                hi = mid - 1;
            }
        }
    }

    None
}

// --- FFI exports ---

/// Look up a term in dictionary data.
///
/// Returns true if found, writing the entry to `out`.
///
/// # Safety
/// All pointers must be valid. `data` must contain at least `data_len` bytes.
/// `term` must be a null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn tp_rust_dict_lookup(
    data: *const u8,
    data_len: u32,
    dict_offset: u32,
    strings_offset: u32,
    entries_offset: u32,
    term: *const c_char,
    out: *mut TpDictEntry,
) -> bool {
    if data.is_null() || term.is_null() || out.is_null() {
        return false;
    }

    let data_slice =
        unsafe { std::slice::from_raw_parts(data, data_len as usize) };
    let term_cstr = unsafe { CStr::from_ptr(term) };
    let term_str = match term_cstr.to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };

    match lookup_term(
        data_slice,
        dict_offset,
        strings_offset,
        entries_offset,
        term_str,
    ) {
        Some(entry) => {
            unsafe { *out = entry };
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_dictionary() {
        let terms = vec!["alpha", "beta", "gamma"];
        let (dict, strings, entries) = encode_dictionary(&terms);

        // Check num_terms
        let num =
            u32::from_ne_bytes(dict[0..4].try_into().unwrap());
        assert_eq!(num, 3);

        // Check entries placeholder size
        assert_eq!(
            entries.len(),
            3 * std::mem::size_of::<TpDictEntry>()
        );

        // Strings should contain all terms
        assert!(strings.len() > 0);
    }

    #[test]
    fn test_lookup_in_encoded_dictionary() {
        let terms = vec!["alpha", "beta", "gamma", "delta"];
        let (dict, strings, mut entries) = encode_dictionary(&terms);

        // Fill in dict entries with test data
        for i in 0..4 {
            let entry = TpDictEntry {
                skip_index_offset: (i * 100) as u32,
                block_count: (i + 1) as u16,
                reserved: 0,
                doc_freq: (i * 10 + 5) as u32,
            };
            let offset = i * std::mem::size_of::<TpDictEntry>();
            let bytes: [u8; 12] = unsafe { std::mem::transmute(entry) };
            entries[offset..offset + 12].copy_from_slice(&bytes);
        }

        // Assemble a fake segment data buffer
        // Layout: [padding][dict][strings][entries]
        let dict_offset = 100u32; // arbitrary offset
        let strings_offset = dict_offset + dict.len() as u32;
        let _entries_offset = strings_offset + strings.len() as u32;

        let mut data = vec![0u8; dict_offset as usize];
        data.extend_from_slice(&dict);
        data.extend_from_slice(&strings);
        data.extend_from_slice(&entries);

        // Look up existing terms (terms must be sorted for binary search)
        let mut sorted_terms = terms.clone();
        sorted_terms.sort();
        let (dict_s, strings_s, mut entries_s) =
            encode_dictionary(&sorted_terms);

        // Fill entries
        for i in 0..4 {
            let entry = TpDictEntry {
                skip_index_offset: (i * 100) as u32,
                block_count: (i + 1) as u16,
                reserved: 0,
                doc_freq: (i * 10 + 5) as u32,
            };
            let offset = i * std::mem::size_of::<TpDictEntry>();
            let bytes: [u8; 12] = unsafe { std::mem::transmute(entry) };
            entries_s[offset..offset + 12].copy_from_slice(&bytes);
        }

        let dict_offset2 = 100u32;
        let strings_offset2 = dict_offset2 + dict_s.len() as u32;
        let entries_offset2 = strings_offset2 + strings_s.len() as u32;

        let mut data2 = vec![0u8; dict_offset2 as usize];
        data2.extend_from_slice(&dict_s);
        data2.extend_from_slice(&strings_s);
        data2.extend_from_slice(&entries_s);

        // Look up "beta" (should be at index 1 in sorted order)
        let result = lookup_term(
            &data2,
            dict_offset2,
            strings_offset2,
            entries_offset2,
            "beta",
        );
        assert!(result.is_some());
        let entry = result.unwrap();
        assert_eq!(entry.block_count, 2); // index 1 → block_count = 2

        // Look up non-existent term
        let result = lookup_term(
            &data2,
            dict_offset2,
            strings_offset2,
            entries_offset2,
            "zeta",
        );
        assert!(result.is_none());
    }
}
