//! The text of pdfminer's log records (`docs/js-runtime.md`,
//! "Diagnostics"): what `record.getMessage()` gives, `msg % args`, for the
//! `log.warning(...)` calls the port makes (`pycompat::logging::Logger::log`).
//! Not a Python module: the formatting pieces the call sites share (`%r`
//! of a tuple or list of operands, `%d`, `%s`), which the port computes at
//! the call, only when a collector listens; the frames of each call stay
//! those of logging disabled, the reference.

use pycompat::{exc, PyResult};

use crate::pdftypes::PDFObject;

/// `%r` of an object.
pub fn r(x: &PDFObject) -> PyResult<String> {
    x.repr()
}

/// `%r` of a tuple of objects (`(a, b)`; `(a,)` for one).
pub fn tuple(items: &[&PDFObject]) -> PyResult<String> {
    let parts = items.iter().map(|x| x.repr()).collect::<PyResult<Vec<_>>>()?;
    Ok(if parts.len() == 1 { format!("({},)", parts[0]) } else { format!("({})", parts.join(", ")) })
}

/// `%r` of a tuple whose first item is a `str` (`("m", x, y)`, a path
/// segment).
pub fn tagged_tuple(tag: &str, items: &[&PDFObject]) -> PyResult<String> {
    let mut parts = vec![pycompat::repr::str_repr(tag)];
    for x in items {
        parts.push(x.repr()?);
    }
    Ok(format!("({})", parts.join(", ")))
}

/// `%r` of a list of objects.
pub fn list(items: &[PDFObject]) -> PyResult<String> {
    let parts = items.iter().map(PDFObject::repr).collect::<PyResult<Vec<_>>>()?;
    Ok(format!("[{}]", parts.join(", ")))
}

/// `%d` of a number (`int(x)` of a float, `1`/`0` of a bool); another
/// value raises (`TypeError`: the record is not emitted).
pub fn d(x: &PDFObject) -> PyResult<String> {
    match x {
        PDFObject::Int(i) => Ok(i.to_string()),
        PDFObject::Bool(b) => Ok(u8::from(*b).to_string()),
        PDFObject::Float(f) if f.is_finite() => Ok(format!("{}", f.trunc() as i128)),
        PDFObject::Float(_) => Err(exc::ValueError("cannot convert float to integer")),
        other => Err(exc::TypeError(format!("%d format: a real number is required, not {}", other.type_name()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pycompat::PyInt;

    #[test]
    fn tuples_lists_and_numbers_as_python_formats_them() {
        let one = PDFObject::Int(PyInt::from(1));
        let half = PDFObject::Float(0.5);
        assert_eq!(tuple(&[&one]).unwrap(), "(1,)");
        assert_eq!(tuple(&[&one, &half]).unwrap(), "(1, 0.5)");
        assert_eq!(tagged_tuple("m", &[&one, &half]).unwrap(), "('m', 1, 0.5)");
        assert_eq!(list(&[one.clone(), half.clone()]).unwrap(), "[1, 0.5]");
        assert_eq!(list(&[]).unwrap(), "[]");
        assert_eq!(d(&one).unwrap(), "1");
        assert_eq!(d(&PDFObject::Float(-2.7)).unwrap(), "-2");
        assert_eq!(d(&PDFObject::Bool(true)).unwrap(), "1");
        assert!(d(&PDFObject::Float(f64::NAN)).is_err());
        assert!(d(&PDFObject::Null).is_err());
    }
}
