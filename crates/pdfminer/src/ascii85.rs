//! Port of `pdfminer/ascii85.py`: the `/ASCII85Decode` and
//! `/ASCIIHexDecode` filters.
//!
//! The three regular expressions of the module are applied by hand, with
//! the semantics of `re.sub` on these exact patterns (`\s` of a bytes
//! pattern is `[ \t\n\r\f\v]`, `pycompat::text::bytes_isspace`):
//!
//! - `start_re` `^\s*<?\s*~\s*`: anchored at 0 (no `MULTILINE`), so at
//!   most one match. Backtracking cannot help: `\s*` must take the whole
//!   first run of whitespace (a shorter run leaves a space where `<` or
//!   `~` is needed), then an optional `<`, a run of whitespace, a `~`, a
//!   run of whitespace.
//! - `end_re` `\s*~\s*>?\s*$`: what follows the `~` of a match is
//!   `\s* >? \s*` up to the end (`$` also matches before a final `\n`, but
//!   that `\n` is whitespace the greedy `\s*` takes). That text holds no
//!   `~`, so the `~` is the last one of the data, and the leftmost match
//!   starts at the run of whitespace before it; it ends at the end of the
//!   data, so there is no second match.
//! - `bws_re` `\s`: every whitespace byte removed.

use pycompat::base64::a85decode;
use pycompat::binascii::unhexlify;
use pycompat::text::bytes_isspace;
use pycompat::PyResult;

/// `start_re.sub(b"", data)`
fn strip_start(data: &[u8]) -> &[u8] {
    let ws = |i: usize| data[i..].iter().take_while(|&&c| bytes_isspace(c)).count();
    let mut i = ws(0);
    if data.get(i) == Some(&b'<') {
        i += 1;
        i += ws(i);
    }
    if data.get(i) != Some(&b'~') {
        return data;
    }
    i += 1;
    &data[i + ws(i)..]
}

/// `end_re.sub(b"", data)`
fn strip_end(data: &[u8]) -> &[u8] {
    let Some(tilde) = data.iter().rposition(|&c| c == b'~') else { return data };
    let mut rest = data[tilde + 1..].iter().skip_while(|&&c| bytes_isspace(c)).peekable();
    if rest.peek() == Some(&&b'>') {
        rest.next();
    }
    if !rest.all(|&c| bytes_isspace(c)) {
        return data;
    }
    let start = data[..tilde].iter().rposition(|&c| !bytes_isspace(c)).map_or(0, |i| i + 1);
    &data[..start]
}

// pdfminer/ascii85.py:11 ascii85decode
/// In ASCII85 encoding, every four bytes are encoded with five ASCII
/// letters. Leading `<~` or `~` and trailing `~` or `~>` are stripped,
/// then `base64.a85decode` decodes the rest (`ValueError` on bad input).
pub fn ascii85decode(data: &[u8]) -> PyResult<Vec<u8>> {
    let _frame = pycompat::recursion::py_frame()?;
    let data = strip_start(data);
    let data = strip_end(data);
    // base64.a85decode (a Python function, which calls
    // _bytes_from_decode_data first)
    let _a85decode = pycompat::recursion::py_frame()?;
    pycompat::recursion::leaf(1)?;
    a85decode(data)
}

// pdfminer/ascii85.py:33 asciihexdecode
/// ASCIIHexDecode filter: whitespace ignored, `>` ends the data (an odd
/// number of digits before it gets a `0`), `binascii.unhexlify` decodes
/// (`binascii.Error` on an odd length or a non-hexadecimal digit).
pub fn asciihexdecode(data: &[u8]) -> PyResult<Vec<u8>> {
    let _frame = pycompat::recursion::py_frame()?;
    let mut data: Vec<u8> = data.iter().copied().filter(|&c| !bytes_isspace(c)).collect();
    if let Some(idx) = data.iter().position(|&c| c == b'>') {
        data.truncate(idx);
        if idx % 2 == 1 {
            data.push(b'0');
        }
    }
    unhexlify(&data)
}
