//! `unicodedata.normalize` of CPython 3.12 (Unicode 15.0), on any `str`
//! (lone surrogates included: they have no decomposition and combining
//! class 0, so they pass through as starters).
//!
//! A port of `Modules/unicodedata.c`: `unicodedata_UCD_normalize_impl`,
//! `nfd_nfkd` and `nfc_nfkc`, over tables read from the oracle's
//! `unicodedata` by `oracle/codegen/unicodedata_tables.py`
//! ([`crate::unicodedata_tables`]). CPython first runs a quick check
//! (`is_normalized_quickcheck`) and returns the input itself when it is
//! certainly normalized; the full algorithm returns the same text for such
//! an input, so the port always runs it (the differential cases of
//! `oracle/unit/pycompat.py`, `normalize`, hold normalized inputs too).
//! pdfplumber normalizes the text of each char when `unicode_norm` is set
//! (`pdfplumber/page.py`, `Page.process_object`).
//!
//! Mutation testing (`cargo mutants -f crates/pycompat/src/unicodedata.rs
//! --profile mutants -j 1 -- --package=pdftrace --test=pycompat_unit
//! --test=canon_unit`, review of wave 2): 159 mutants, every one caught
//! or timed out except these, equivalent for every input:
//!
//! - `is_nfc_first` and `is_nfc_last` made `true`: they only skip a
//!   [`compose`] look-up, and the composition table holds exactly the
//!   pairs whose first is an NFC first and whose last an NFC last (CPython's
//!   `comp_data` is 0 for every other pair), so the look-up finds nothing
//!   where they are false; a non-first then never composes, and the scan
//!   that follows it only advances `i1`.
//! - `i1 += 1` made `*= 1` in the not-combinable branch of `nfc_nfkc`: the
//!   loop meets the same mark again with `comb == comb1`, which is the
//!   blocked branch, and that one advances `i1`.
//!
//! The trailing-jamo bounds of the Hangul composition (`T_BASE < c` and
//! `c < T_BASE + T_COUNT` made `<=`) are killed by the cases `가`
//! followed by `ᆧ` or `ᇃ`.

use crate::pystr::{PyStr, PyStrBuilder};
use crate::unicodedata_tables as tables;
use crate::{exc, PyResult};

pub use crate::unicodedata_tables::UNIDATA_VERSION;

/// The normalization forms of `unicodedata.normalize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NormalizationForm {
    Nfc,
    Nfkc,
    Nfd,
    Nfkd,
}

impl NormalizationForm {
    /// The form named `name` (`"NFC"`, `"NFKC"`, `"NFD"`, `"NFKD"`); other
    /// names raise `ValueError("invalid normalization form")` in Python
    /// (when the input is not empty).
    pub fn parse(name: &str) -> Option<NormalizationForm> {
        Some(match name {
            "NFC" => NormalizationForm::Nfc,
            "NFKC" => NormalizationForm::Nfkc,
            "NFD" => NormalizationForm::Nfd,
            "NFKD" => NormalizationForm::Nfkd,
            _ => return None,
        })
    }
}

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

/// `_getrecord_ex(code)->combining`: the canonical combining class.
pub fn combining(code: u32) -> u8 {
    let i = tables::COMBINING.partition_point(|&(_, hi, _)| hi < code);
    match tables::COMBINING.get(i) {
        Some(&(lo, _, class)) if lo <= code => class,
        _ => 0,
    }
}

/// `get_decomp_record`: `(compatibility?, code points)`, or `None` (`count
/// == 0`).
fn decomposition(code: u32) -> Option<(bool, &'static [u32])> {
    tables::DECOMPOSITION.binary_search_by_key(&code, |&(c, _, _)| c).ok().map(|i| {
        let (_, compat, codes) = tables::DECOMPOSITION[i];
        (compat, codes)
    })
}

/// `comp_data[...]` of the pair (`find_nfc_index` of both, then the
/// table), or `None`.
fn compose(first: u32, last: u32) -> Option<u32> {
    tables::COMPOSITION.binary_search_by(|&(a, b, _)| (a, b).cmp(&(first, last))).ok().map(|i| tables::COMPOSITION[i].2)
}

/// `find_nfc_index(nfc_first, code) != -1`: `code` is the first of a pair.
fn is_nfc_first(code: u32) -> bool {
    let i = tables::COMPOSITION.partition_point(|&(a, _, _)| a < code);
    tables::COMPOSITION.get(i).is_some_and(|&(a, _, _)| a == code)
}

/// `find_nfc_index(nfc_last, code) != -1`: `code` is the last of a pair.
fn is_nfc_last(code: u32) -> bool {
    tables::COMPOSITION_LAST.binary_search(&code).is_ok()
}

/// `nfd_nfkd(self, input, k)`: decomposition (compatibility ones too when
/// `k`), then canonical ordering.
fn nfd_nfkd(input: impl Iterator<Item = u32>, k: bool) -> Vec<u32> {
    let mut output: Vec<u32> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    for c in input {
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
            // Other decompositions.
            match decomposition(code) {
                // Copy character if it is not decomposable, or has a
                // compatibility decomposition, but we do NFD.
                None => output.push(code),
                Some((true, _)) if !k => output.push(code),
                // Copy decomposition onto the stack, in reverse order.
                Some((_, codes)) => stack.extend(codes.iter().rev()),
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

/// `nfc_nfkc(self, input, k)`: `nfd_nfkd`, then canonical composition.
fn nfc_nfkc(input: impl Iterator<Item = u32>, k: bool) -> Vec<u32> {
    let data = nfd_nfkd(input, k);
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
        // Hangul Composition. We don't need to check for <LV,T> pairs,
        // since we always have decomposed data.
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
            output.push(code);
            continue;
        }
        // code is still input[i] here
        if !is_nfc_first(code) {
            output.push(code);
            i += 1;
            continue;
        }
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
            let composed = if is_nfc_last(code1) { compose(current, code1) } else { None };
            let Some(composed) = composed else {
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
            if !is_nfc_first(current) {
                break;
            }
        }
        // Output character was already written. Just advance the indices.
        output.push(current);
        i += 1;
    }
    output
}

/// `unicodedata.normalize(form, s)` (`unicodedata_UCD_normalize_impl`).
pub fn normalize(form: NormalizationForm, s: &PyStr) -> PyStr {
    if s.is_empty() {
        return s.clone();
    }
    let out = match form {
        NormalizationForm::Nfc => nfc_nfkc(s.code_points(), false),
        NormalizationForm::Nfkc => nfc_nfkc(s.code_points(), true),
        NormalizationForm::Nfd => nfd_nfkd(s.code_points(), false),
        NormalizationForm::Nfkd => nfd_nfkd(s.code_points(), true),
    };
    let mut b = PyStrBuilder::with_capacity(out.len());
    for c in out {
        b.push(c);
    }
    b.finish()
}

/// `unicodedata.normalize(form, s)` with the form by name: an empty `s` is
/// returned before the form is looked at; an unknown form raises
/// `ValueError("invalid normalization form")`.
pub fn normalize_named(form: &str, s: &PyStr) -> PyResult<PyStr> {
    if s.is_empty() {
        return Ok(s.clone());
    }
    match NormalizationForm::parse(form) {
        Some(form) => Ok(normalize(form, s)),
        None => Err(exc::ValueError("invalid normalization form")),
    }
}
