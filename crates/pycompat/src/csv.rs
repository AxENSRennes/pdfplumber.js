//! CPython 3.12's `csv.writer(stream, quoting=csv.QUOTE_MINIMAL,
//! escapechar="\\")` (default dialect `excel`: delimiter `,`, quotechar
//! `"`, `doublequote`, line terminator `\r\n`), as
//! pdfplumber/container.py:132 `Container.to_csv` writes through
//! `csv.DictWriter` (`Modules/_csv.c` `csv_writerow`, `join_append_data`).
//!
//! A field is `None` (written empty) or the `str` of the value (the caller
//! computes `str()` of what is not a `str`). A character that is the
//! delimiter, the quotechar or a line terminator character quotes the
//! field; the quotechar is doubled; the escapechar (`\`) is escaped by
//! itself (`\\`) without quoting the field. A row of a single empty field
//! is written `""`.

use crate::pystr::PyStr;

/// `writer.writerow(fields)`: the row and its `\r\n`, appended to `out`
/// (code points; a lone surrogate is written as it is).
pub fn write_row(out: &mut Vec<u32>, fields: &[Option<PyStr>]) {
    let start = out.len();
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push(u32::from(','));
        }
        let Some(field) = field else { continue };
        let mut quoted = false;
        let mut body: Vec<u32> = Vec::with_capacity(field.len());
        for c in field.code_points() {
            match char::from_u32(c) {
                Some('"') => {
                    // doublequote
                    body.push(c);
                    quoted = true;
                }
                Some('\\') => body.push(c), // escaped below
                Some(',' | '\r' | '\n') => quoted = true,
                _ => {}
            }
            body.push(c);
        }
        if quoted {
            out.push(u32::from('"'));
        }
        out.extend(body);
        if quoted {
            out.push(u32::from('"'));
        }
    }
    // a single empty field: quoted, so that the row is not empty
    if !fields.is_empty() && out.len() == start {
        out.extend([u32::from('"'), u32::from('"')]);
    }
    out.extend([u32::from('\r'), u32::from('\n')]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(fields: &[Option<&str>]) -> String {
        let mut out = Vec::new();
        write_row(&mut out, &fields.iter().map(|f| f.map(PyStr::new)).collect::<Vec<_>>());
        out.into_iter().map(|c| char::from_u32(c).unwrap()).collect()
    }

    #[test]
    fn quoting() {
        assert_eq!(row(&[Some("")]), "\"\"\r\n");
        assert_eq!(row(&[None]), "\"\"\r\n");
        assert_eq!(row(&[None, None]), ",\r\n");
        assert_eq!(row(&[Some("a,b")]), "\"a,b\"\r\n");
        assert_eq!(row(&[Some("a\"b")]), "\"a\"\"b\"\r\n");
        assert_eq!(row(&[Some("a\\b")]), "a\\\\b\r\n");
        assert_eq!(row(&[Some("\\\"")]), "\"\\\\\"\"\"\r\n");
        assert_eq!(row(&[Some("a\rb"), Some(" x")]), "\"a\rb\", x\r\n");
    }
}
