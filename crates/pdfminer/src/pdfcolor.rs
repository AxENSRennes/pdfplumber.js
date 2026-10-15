//! Port of `pdfminer/pdfcolor.py`: color spaces.
//!
//! `PREDEFINED_COLORSPACE` holds shared instances in Python (the
//! interpreter hands the same object to every char); here each thread has
//! its own table of `Rc<PDFColorSpace>`, so [`predefined_colorspace`]
//! returns the same `Rc` each time, as Python returns the same object.

use std::rc::Rc;
use std::sync::LazyLock;

use indexmap::IndexMap;
use pycompat::PyInt;

use crate::pdftypes::PDFObject;
use crate::psparser::{PSLiteral, LIT};

// pdfminer/pdfcolor.py:5-11 LITERAL_DEVICE_GRAY, LITERAL_DEVICE_RGB, LITERAL_DEVICE_CMYK, LITERAL_INLINE_DEVICE_GRAY,
// LITERAL_INLINE_DEVICE_RGB, LITERAL_INLINE_DEVICE_CMYK
pub static LITERAL_DEVICE_GRAY: LazyLock<PSLiteral> = LazyLock::new(|| LIT("DeviceGray"));
pub static LITERAL_DEVICE_RGB: LazyLock<PSLiteral> = LazyLock::new(|| LIT("DeviceRGB"));
pub static LITERAL_DEVICE_CMYK: LazyLock<PSLiteral> = LazyLock::new(|| LIT("DeviceCMYK"));
// Abbreviations for inline images
pub static LITERAL_INLINE_DEVICE_GRAY: LazyLock<PSLiteral> = LazyLock::new(|| LIT("G"));
pub static LITERAL_INLINE_DEVICE_RGB: LazyLock<PSLiteral> = LazyLock::new(|| LIT("RGB"));
pub static LITERAL_INLINE_DEVICE_CMYK: LazyLock<PSLiteral> = LazyLock::new(|| LIT("CMYK"));

/// pdfminer/pdfcolor.py:14 `PDFColorSpace`.
#[derive(Clone, Debug)]
pub struct PDFColorSpace {
    /// `self.name`: a `str` (from `literal_name`).
    pub name: String,
    /// `self.ncomponents`: an int for the predefined spaces and `DeviceN`,
    /// the raw `/N` entry of the stream for `ICCBased` (any object, even an
    /// unresolved reference).
    pub ncomponents: PDFObject,
}

impl PDFColorSpace {
    // pdfminer/pdfcolor.py:15 PDFColorSpace.__init__
    pub fn new(name: impl Into<String>, ncomponents: PDFObject) -> PDFColorSpace {
        PDFColorSpace { name: name.into(), ncomponents }
    }

    // pdfminer/pdfcolor.py:19 PDFColorSpace.__repr__
    pub fn repr(&self) -> pycompat::PyResult<String> {
        Ok(format!("<PDFColorSpace: {}, ncomponents={}>", self.name, self.ncomponents.py_str()?))
    }
}

/// The names and component counts of `PREDEFINED_COLORSPACE`, in order
/// (`DeviceGray`, the default, first).
// pdfminer/pdfcolor.py:23 PREDEFINED_COLORSPACE
pub static PREDEFINED_COLORSPACE_SPECS: LazyLock<IndexMap<&'static str, i64>> = LazyLock::new(|| {
    [
        ("DeviceGray", 1), // default value first
        ("CalRGB", 3),
        ("CalGray", 1),
        ("Lab", 3),
        ("DeviceRGB", 3),
        ("DeviceCMYK", 4),
        ("Separation", 1),
        ("Indexed", 1),
        ("Pattern", 1),
    ]
    .into_iter()
    .collect()
});

thread_local! {
    static PREDEFINED: IndexMap<&'static str, Rc<PDFColorSpace>> = PREDEFINED_COLORSPACE_SPECS
        .iter()
        .map(|(&name, &n)| (name, Rc::new(PDFColorSpace::new(name, PDFObject::Int(PyInt::Small(n))))))
        .collect();
}

/// `PREDEFINED_COLORSPACE.get(name)`: the shared instance.
pub fn predefined_colorspace(name: &str) -> Option<Rc<PDFColorSpace>> {
    PREDEFINED.with(|m| m.get(name).cloned())
}

/// `PREDEFINED_COLORSPACE` in order.
pub fn predefined_colorspaces() -> Vec<Rc<PDFColorSpace>> {
    PREDEFINED.with(|m| m.values().cloned().collect())
}
