//! Port of `pdfminer/encodingdb.py`: Adobe glyph names to Unicode
//! (`name2unicode`) and the standard Latin encodings (`EncodingDB`).
//!
//! `name2unicode` never returns a lone surrogate (it rejects U+D800..U+DFFF),
//! so its result is a Rust `String`.
//!
//! The dicts `EncodingDB` returns map Python ints to `str`
//! ([`Cid2Unicode`], insertion-ordered like a dict). A `Differences` entry
//! `true`/`false` sets the code to `1`/`0`: Python would keep `True` as the
//! key object when the dict has no `1` yet, which only `repr()` of the dict
//! could show (`True == 1` and `hash(True) == hash(1)`, so every lookup is
//! the same).

use std::borrow::Cow;
use std::sync::LazyLock;

use indexmap::IndexMap;
use pycompat::repr::{bytes_repr, str_repr};
use pycompat::{exc, ExcKind, PyInt, PyResult};
use rustc_hash::FxBuildHasher;

use crate::glyphlist::glyphname2unicode;
use crate::latin_enc::ENCODING;
use crate::pdftypes::PDFObject;
use crate::psparser::LiteralName;

/// A `dict[int, str]` from character codes to text.
pub type Cid2Unicode = IndexMap<PyInt, String, FxBuildHasher>;

/// `HEXADECIMAL.match(s)` with `HEXADECIMAL = re.compile(r"[0-9a-fA-F]+")`:
/// the string starts with an ASCII hex digit.
fn hexadecimal_match(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_hexdigit())
}

/// `s.strip(chars)`: removes the characters of `chars` from both ends.
fn py_strip<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}

/// `chr(i)`: the code point (a surrogate is returned as is), or
/// `OverflowError` outside the C `int` range and `ValueError` outside
/// `range(0x110000)`.
pub fn py_chr(i: &PyInt) -> PyResult<u32> {
    let v = match i.as_i64() {
        Some(v) if i32::try_from(v).is_ok() => v,
        _ => return Err(exc::OverflowError("Python int too large to convert to C int")),
    };
    match u32::try_from(v) {
        Ok(c) if c < 0x110000 => Ok(c),
        _ => Err(exc::ValueError("chr() arg not in range(0x110000)")),
    }
}

/// `chr(i)` for a code point that is not a surrogate (checked by the caller).
fn chr_char(i: &PyInt) -> PyResult<char> {
    let c = py_chr(i)?;
    Ok(char::from_u32(c).expect("not a surrogate"))
}

// pdfminer/encodingdb.py:16 name2unicode
/// Converts an Adobe glyph name to Unicode: the text, or `PDFKeyError` when
/// the name matches nothing (and the `ValueError` of `int()`/`chr()` on a
/// malformed `uniXXXX`/`uXXXX` name).
pub fn name2unicode(name: &str) -> PyResult<String> {
    let name = name.split('.').next().expect("split yields at least one part");
    let components: Vec<&str> = name.split('_').collect();

    if components.len() > 1 {
        let mut out = String::new();
        for component in components {
            out.push_str(&name2unicode(component)?);
        }
        return Ok(out);
    } else if let Some(text) = glyphname2unicode.get(name) {
        return Ok((*text).to_owned());
    } else if name.starts_with("uni") {
        let name_without_uni = py_strip(name, "uni");
        let chars: Vec<char> = name_without_uni.chars().collect();
        if hexadecimal_match(name_without_uni) && chars.len().is_multiple_of(4) {
            let unicode_digits = chars
                .chunks(4)
                .map(|chunk| PyInt::parse_str(&chunk.iter().collect::<String>(), 16))
                .collect::<PyResult<Vec<PyInt>>>()?;
            for digit in &unicode_digits {
                raise_key_error_for_invalid_unicode(digit)?;
            }
            return unicode_digits.iter().map(chr_char).collect();
        }
    } else if name.starts_with('u') {
        let name_without_u = py_strip(name, "u");
        if hexadecimal_match(name_without_u) && (4..=6).contains(&name_without_u.chars().count()) {
            let unicode_digit = PyInt::parse_str(name_without_u, 16)?;
            raise_key_error_for_invalid_unicode(&unicode_digit)?;
            return Ok(chr_char(&unicode_digit)?.to_string());
        }
    }

    Err(exc::PDFKeyError(str_repr(&format!(
        "Could not convert unicode name \"{name}\" to character because it does not match specification"
    ))))
}

// pdfminer/encodingdb.py:16 name2unicode
/// `name2unicode(name)` for the name of a literal: a `bytes` name (not valid
/// UTF-8) is not a `str`, which raises `PDFKeyError`.
pub fn name2unicode_literal(name: LiteralName) -> PyResult<String> {
    match name {
        LiteralName::Str(s) => name2unicode(s),
        LiteralName::Bytes(b) => Err(exc::PDFKeyError(str_repr(&format!(
            "Could not convert unicode name \"{}\" to character because it should be of type str but is of type \
             <class 'bytes'>",
            bytes_repr(b)
        )))),
    }
}

// pdfminer/encodingdb.py:72 raise_key_error_for_invalid_unicode
/// Unicode values should not be in the range D800 through DFFF because that
/// is used for surrogate pairs in UTF-16: raises `PDFKeyError` for them.
pub fn raise_key_error_for_invalid_unicode(unicode_digit: &PyInt) -> PyResult<()> {
    if *unicode_digit > PyInt::Small(55295) && *unicode_digit < PyInt::Small(57344) {
        return Err(exc::PDFKeyError(str_repr(&format!(
            "Unicode digit {unicode_digit} is invalid because it is in the range D800 through DFFF"
        ))));
    }
    Ok(())
}

/// pdfminer/encodingdb.py:85 `EncodingDB`: the class attributes (built
/// once, like the class body).
pub struct EncodingDB {
    pub std2unicode: Cid2Unicode,
    pub mac2unicode: Cid2Unicode,
    pub win2unicode: Cid2Unicode,
    pub pdf2unicode: Cid2Unicode,
}

static ENCODING_DB: LazyLock<EncodingDB> = LazyLock::new(EncodingDB::build);

impl EncodingDB {
    // pdfminer/encodingdb.py:86-89 EncodingDB.std2unicode, EncodingDB.mac2unicode, EncodingDB.win2unicode,
    // EncodingDB.pdf2unicode
    /// The class body: one entry per encoding that has a (truthy) code for
    /// the name.
    fn build() -> EncodingDB {
        let mut db = EncodingDB {
            std2unicode: Cid2Unicode::default(),
            mac2unicode: Cid2Unicode::default(),
            win2unicode: Cid2Unicode::default(),
            pdf2unicode: Cid2Unicode::default(),
        };
        for &(name, std, mac, win, pdf) in ENCODING {
            let c = name2unicode(name).expect("every name of latin_enc is a glyph name");
            for (code, table) in [
                (std, &mut db.std2unicode),
                (mac, &mut db.mac2unicode),
                (win, &mut db.win2unicode),
                (pdf, &mut db.pdf2unicode),
            ] {
                if let Some(code) = code.filter(|&code| code != 0) {
                    table.insert(PyInt::Small(code), c.clone());
                }
            }
        }
        db
    }

    /// The class (its attributes).
    pub fn get() -> &'static EncodingDB {
        &ENCODING_DB
    }

    // pdfminer/encodingdb.py:101 EncodingDB.encodings
    /// `EncodingDB.encodings.get(name)`.
    pub fn encodings(name: &str) -> Option<&'static Cid2Unicode> {
        let db = EncodingDB::get();
        match name {
            "StandardEncoding" => Some(&db.std2unicode),
            "MacRomanEncoding" => Some(&db.mac2unicode),
            "WinAnsiEncoding" => Some(&db.win2unicode),
            "PDFDocEncoding" => Some(&db.pdf2unicode),
            _ => None,
        }
    }

    // pdfminer/encodingdb.py:109 EncodingDB.get_encoding
    /// The encoding `name` (`StandardEncoding` when unknown), with the
    /// `Differences` array `diff` applied: the class dict itself
    /// (`Cow::Borrowed`) when `diff` is `None` or empty, else a copy.
    pub fn get_encoding(name: &str, diff: Option<&[PDFObject]>) -> PyResult<Cow<'static, Cid2Unicode>> {
        let base = EncodingDB::encodings(name).unwrap_or(&EncodingDB::get().std2unicode);
        let Some(diff) = diff.filter(|diff| !diff.is_empty()) else {
            return Ok(Cow::Borrowed(base));
        };
        let mut cid2unicode = base.clone();
        let mut cid = PyInt::ZERO;
        for x in diff {
            match x {
                PDFObject::Int(i) => cid = i.clone(),
                PDFObject::Bool(b) => cid = PyInt::Small(i64::from(*b)),
                PDFObject::Literal(lit) => {
                    match name2unicode_literal(lit.name()) {
                        Ok(text) => {
                            cid2unicode.insert(cid.clone(), text);
                        }
                        // log.debug(str(e)); name2unicode raises nothing else
                        // (an equivalent mutant: see tests/cmaps_unit.rs)
                        Err(e) if e.is_instance(ExcKind::KeyError) || e.is_instance(ExcKind::ValueError) => {}
                        Err(e) => return Err(e),
                    }
                    cid = cid.add(&PyInt::Small(1));
                }
                _ => {}
            }
        }
        Ok(Cow::Owned(cid2unicode))
    }
}
