//! Port of `pdfminer/_saslprep.py`: RFC 4013 SASLprep, which
//! `PDFStandardSecurityHandlerV5` applies to revision 6 passwords.
//!
//! Upstream calls the `stringprep` module and
//! `unicodedata.ucd_3_2_0.normalize("NFKC", ...)`, CPython's Unicode 3.2.0
//! view of its database. Their data is generated from the oracle's CPython
//! by `oracle/codegen/saslprep_tables.py` into [`crate::saslprep_tables`]:
//! one range table per `stringprep.in_table_*` predicate, and the tables of
//! the normalization, which [`ucd_3_2_0_normalize_nfkc`] ports from
//! CPython's `Modules/unicodedata.c` (`nfd_nfkd`, `nfc_nfkc`).
//!
//! The input is a Rust `str`, so it holds no lone surrogate (the
//! `stringprep.in_table_c5` table, surrogates, is kept for fidelity but can
//! match nothing). `saslprep` runs while a document is opened, at a fixed
//! shallow depth: it takes the frame of its upstream function, nothing finer.

use std::cmp::Ordering;

use pycompat::recursion;
use pycompat::{exc, PyResult};

use crate::saslprep_tables as tables;

/// Whether `c` is in a table of inclusive ranges.
fn in_ranges(ranges: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    ranges
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                Ordering::Less
            } else if lo > c {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// A `stringprep.in_table_*` predicate, by its generated table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringprepTable {
    A1,
    B1,
    C12,
    C21C22,
    C3,
    C4,
    C5,
    C6,
    C7,
    C8,
    C9,
    D1,
    D2,
}

impl StringprepTable {
    /// The generated ranges of the predicate.
    pub fn ranges(self) -> &'static [(u32, u32)] {
        match self {
            StringprepTable::A1 => tables::A1,
            StringprepTable::B1 => tables::B1,
            StringprepTable::C12 => tables::C12,
            StringprepTable::C21C22 => tables::C21_C22,
            StringprepTable::C3 => tables::C3,
            StringprepTable::C4 => tables::C4,
            StringprepTable::C5 => tables::C5,
            StringprepTable::C6 => tables::C6,
            StringprepTable::C7 => tables::C7,
            StringprepTable::C8 => tables::C8,
            StringprepTable::C9 => tables::C9,
            StringprepTable::D1 => tables::D1,
            StringprepTable::D2 => tables::D2,
        }
    }

    /// `stringprep.in_table_<name>(ch)`
    pub fn contains(self, ch: char) -> bool {
        in_ranges(self.ranges(), ch)
    }
}

// pdfminer/_saslprep.py:30 _PROHIBITED
/// RFC4013 section 2.3 prohibited output.
const PROHIBITED: [StringprepTable; 9] = [
    // A strict reading of RFC 4013 requires table c12 here, but
    // characters from it are mapped to SPACE in the Map step. Can
    // normalization reintroduce them somehow?
    StringprepTable::C12,
    StringprepTable::C21C22,
    StringprepTable::C3,
    StringprepTable::C4,
    StringprepTable::C5,
    StringprepTable::C6,
    StringprepTable::C7,
    StringprepTable::C8,
    StringprepTable::C9,
];

// pdfminer/_saslprep.py:46 saslprep
/// An implementation of RFC4013 SASLprep: the SASLprep'ed version of
/// `data`; `prohibit_unassigned_code_points` (upstream's default is true)
/// also rejects the code points unassigned in Unicode 3.2.0 (table A.1).
///
/// Raises `PDFValueError` when a check fails, and `IndexError` when the
/// mapping leaves an empty string (`data[0]`).
pub fn saslprep(data: &str, prohibit_unassigned_code_points: bool) -> PyResult<String> {
    let _frame = recursion::py_frame()?;
    let mut prohibited: Vec<StringprepTable> = PROHIBITED.to_vec();
    if prohibit_unassigned_code_points {
        prohibited.push(StringprepTable::A1);
    }

    // RFC3454 section 2, step 1 - Map
    // RFC4013 section 2.1 mappings
    // Map Non-ASCII space characters to SPACE (U+0020). Map
    // commonly mapped to nothing characters to, well, nothing.
    let mapped: Vec<char> = data
        .chars()
        .filter(|&elt| !StringprepTable::B1.contains(elt))
        .map(|elt| if StringprepTable::C12.contains(elt) { '\u{20}' } else { elt })
        .collect();

    // RFC3454 section 2, step 2 - Normalize
    // RFC4013 section 2.2 normalization
    let data = ucd_3_2_0_normalize_nfkc(&mapped);

    let (Some(&first), Some(&last)) = (data.first(), data.last()) else {
        return Err(exc::IndexError("string index out of range"));
    };
    if StringprepTable::D1.contains(first) {
        if !StringprepTable::D1.contains(last) {
            // RFC3454, Section 6, #3. If a string contains any
            // RandALCat character, the first and last characters
            // MUST be RandALCat characters.
            return Err(exc::PDFValueError("SASLprep: failed bidirectional check"));
        }
        // RFC3454, Section 6, #2. If a string contains any RandALCat
        // character, it MUST NOT contain any LCat character.
        prohibited.push(StringprepTable::D2);
    } else {
        // RFC3454, Section 6, #3. Following the logic of #3, if
        // the first character is not a RandALCat, no other character
        // can be either.
        prohibited.push(StringprepTable::D1);
    }

    // RFC3454 section 2, step 3 and 4 - Prohibit and check bidi
    for &char in &data {
        if prohibited.iter().any(|in_table| in_table.contains(char)) {
            return Err(exc::PDFValueError("SASLprep: failed prohibited character check"));
        }
    }

    Ok(data.into_iter().collect())
}

// ---------------------------------------------------------------------------
// unicodedata.ucd_3_2_0.normalize("NFKC", s): CPython 3.12.3
// Modules/unicodedata.c, `unicodedata_UCD_normalize_impl` with a UCD 3.2.0
// object (no quick check), `nfc_nfkc(self, input, k=1)` and
// `nfd_nfkd(self, input, k=1)`.

// Modules/unicodedata.c: Hangul syllables (decomposed and composed by
// arithmetic).
const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

/// `_getrecord_ex(code)->combining`: the canonical combining class (the
/// current database, also for UCD 3.2.0).
fn combining(code: u32) -> u8 {
    let i = tables::COMBINING.partition_point(|&(_, hi, _)| hi < code);
    match tables::COMBINING.get(i) {
        Some(&(lo, _, class)) if lo <= code => class,
        _ => 0,
    }
}

/// `get_decomp_record` of UCD 3.2.0: the code points of the decomposition,
/// canonical or compatibility (NFKC takes both: the table's `prefix` flag,
/// which NFD reads, is not needed), or `None` for no decomposition (also for
/// a code point unassigned in 3.2.0).
fn decomposition(code: u32) -> Option<&'static [u32]> {
    tables::DECOMPOSITION.binary_search_by_key(&code, |&(c, _, _)| c).ok().map(|i| tables::DECOMPOSITION[i].2)
}

/// `PreviousDBVersion.normalization(code)` of UCD 3.2.0 (0: none).
fn normalization_correction(code: u32) -> Option<u32> {
    tables::NORMALIZATION_CORRECTIONS
        .binary_search_by_key(&code, |&(c, _)| c)
        .ok()
        .map(|i| tables::NORMALIZATION_CORRECTIONS[i].1)
}

/// `comp_data[...]` of the pair, or `None` (0). CPython first looks both
/// characters up in `nfc_first`/`nfc_last` (`find_nfc_index`): a character
/// that is the first (last) of no pair composes with nothing, so those
/// look-ups only shortcut what this one finds (see `nfc_nfkc`).
fn compose(first: u32, last: u32) -> Option<u32> {
    tables::COMPOSITION.binary_search_by(|&(a, b, _)| (a, b).cmp(&(first, last))).ok().map(|i| tables::COMPOSITION[i].2)
}

/// CPython's `Py_UCS4 stack[20]` of `nfd_nfkd` ("Longest decomposition in
/// Unicode 3.2: U+FDFA"): the decomposition stack never holds more.
const STACK_SIZE: usize = 20;

/// `buffer[o++] = code` into a buffer of `size` code points (CPython's fixed
/// buffers): a bug that would write past it panics instead of growing.
fn put(buffer: &mut Vec<u32>, size: usize, code: u32) {
    assert!(buffer.len() < size, "normalization buffer overflow");
    buffer.push(code);
}

/// `nfd_nfkd(ucd_3_2_0, input, k=1)`: decomposition, then canonical
/// ordering.
fn nfd_nfkd(input: &[u32]) -> Vec<u32> {
    let mut output: Vec<u32> = Vec::with_capacity(input.len());
    let mut stack: Vec<u32> = Vec::new();
    for &c in input {
        stack.push(c);
        while let Some(code) = stack.pop() {
            // Hangul Decomposition.
            if (S_BASE..S_BASE + S_COUNT).contains(&code) {
                let s_index = code - S_BASE;
                output.push(L_BASE + s_index / N_COUNT);
                output.push(V_BASE + (s_index % N_COUNT) / T_COUNT);
                let t = T_BASE + s_index % T_COUNT;
                if t != T_BASE {
                    output.push(t);
                }
                continue;
            }
            // normalization changes
            if let Some(value) = normalization_correction(code) {
                put(&mut stack, STACK_SIZE, value);
                continue;
            }
            // Other decompositions.
            match decomposition(code) {
                // Copy character if it is not decomposable (or has a
                // compatibility decomposition, but we do NFD: never here).
                None => output.push(code),
                // Copy decomposition onto the stack, in reverse order.
                Some(codes) => {
                    for &code in codes.iter().rev() {
                        put(&mut stack, STACK_SIZE, code);
                    }
                }
            }
        }
    }
    // Sort canonically.
    if output.is_empty() {
        return output;
    }
    let mut prev = combining(output[0]);
    for i in 1..output.len() {
        let cur = combining(output[i]);
        if prev == 0 || cur == 0 || prev <= cur {
            prev = cur;
            continue;
        }
        // Non-canonical order. Need to switch *i with previous.
        let mut o = i - 1;
        loop {
            output.swap(o, o + 1);
            if o == 0 {
                break;
            }
            o -= 1;
            prev = combining(output[o]);
            if prev == 0 || prev <= cur {
                break;
            }
        }
        prev = combining(output[i]);
    }
    output
}

/// `nfc_nfkc(ucd_3_2_0, input, k=1)`: `nfd_nfkd`, then canonical
/// composition.
fn nfc_nfkc(input: &[u32]) -> Vec<u32> {
    let data = nfd_nfkd(input);
    let len = data.len();
    let mut output: Vec<u32> = Vec::with_capacity(len);
    // indices of characters already composed into an earlier one
    let mut skipped: Vec<usize> = Vec::new();
    let mut i = 0;
    'again: while i < len {
        if let Some(index) = skipped.iter().position(|&s| s == i) {
            // *i character is skipped. Remove from list.
            skipped.swap_remove(index);
            i += 1;
            continue 'again;
        }
        // Hangul Composition. We don't need to check for <LV,T>
        // pairs, since we always have decomposed data.
        let mut code = data[i];
        if (L_BASE..L_BASE + L_COUNT).contains(&code)
            && i + 1 < len
            && (V_BASE..V_BASE + V_COUNT).contains(&data[i + 1])
        {
            let l_index = code - L_BASE;
            let v_index = data[i + 1] - V_BASE;
            code = S_BASE + (l_index * V_COUNT + v_index) * T_COUNT;
            i += 2;
            if i < len && T_BASE < data[i] && data[i] < T_BASE + T_COUNT {
                code += data[i] - T_BASE;
                i += 1;
            }
            put(&mut output, len, code);
            continue;
        }
        // code is still input[i] here. CPython outputs it at once when it is
        // the first of no pair (`find_nfc_index(nfc_first, code) == -1`):
        // the loop below then composes nothing and outputs it the same way.
        // Find next unblocked character.
        let mut i1 = i + 1;
        let mut comb: u8 = 0;
        // output base character for now; might be updated later.
        let mut current = code;
        while i1 < len {
            let code1 = data[i1];
            let comb1 = combining(code1);
            if comb != 0 {
                if comb1 == 0 {
                    break;
                }
                if comb >= comb1 {
                    // Character is blocked.
                    i1 += 1;
                    continue;
                }
            }
            // (find_nfc_index(nfc_last, code1) == -1 also means no pair)
            let Some(composed) = compose(current, code1) else {
                // not_combinable: i1 cannot be combined with i. If i1 is a
                // starter, we don't need to look further. Otherwise, record
                // the combining class.
                if comb1 == 0 {
                    break;
                }
                comb = comb1;
                i1 += 1;
                continue;
            };
            // Replace the original character.
            current = composed;
            // Mark the second character unused.
            skipped.push(i1);
            i1 += 1;
            // CPython stops here when the composed character is the first of
            // no pair: going on composes nothing more.
        }
        // Output character was already written. Just advance the indices.
        put(&mut output, len, current);
        i += 1;
    }
    output
}

/// `unicodedata.ucd_3_2_0.normalize("NFKC", s)`, on the code points of `s`
/// (the empty string is returned as is).
pub fn ucd_3_2_0_normalize_nfkc(s: &[char]) -> Vec<char> {
    if s.is_empty() {
        return Vec::new();
    }
    let codes: Vec<u32> = s.iter().map(|&c| c as u32).collect();
    nfc_nfkc(&codes)
        .into_iter()
        .map(|c| char::from_u32(c).expect("normalization maps scalar values to scalar values"))
        .collect()
}
