//! The report of a font's attributes and results that `oracle/unit/fonts.py`
//! (`font_attrs`) and the `fonts` stages (`oracle/stages/fonts.py`, which
//! imports the same report) write, shared by `tests/fonts_unit.rs` and
//! `stages::fonts`.

use pdfminer::casting::IterItem;
use pdfminer::cmapdb::{AttrValue, CMapBase, Unichr, UnicodeMap};
use pdfminer::layout::CharDisp;
use pdfminer::pdffont::{DefaultDisp, FontKind, FontName, PDFFont, WidthKey};
use pdfminer::utils::{Operand, SeqKind};
use pycompat::recursion::py_frame;
use pycompat::{Num, NumKey, PyInt, PyResult};

use crate::value::{PyStr, Value};

/// A number.
pub fn num(n: &Num) -> Value {
    match n {
        Num::Int(i) => Value::Int(i.clone()),
        Num::Float(f) => Value::Float(*f),
    }
}

/// `seqval(x)` of the generator: a number, a `bool`, or a repeated
/// sequence as `seq{"type": .., "len": ..}`.
pub fn operand(x: &Operand) -> Value {
    match x {
        Operand::Num(n) => num(n),
        Operand::Bool(b) => Value::Bool(*b),
        Operand::Seq(kind, len) => Value::Named {
            name: "seq".into(),
            fields: vec![
                (Value::str("type"), Value::str(if *kind == SeqKind::Bytes { "bytes" } else { "list" })),
                (Value::str("len"), Value::Int(PyInt::Small(i64::try_from(*len).expect("a length")))),
            ],
        },
        Operand::Other(name) => panic!("an operand of type {name} is never a result"),
    }
}

/// A font's text for a code.
pub fn unichr(u: &Unichr) -> Value {
    match u {
        Unichr::Text(s) => Value::str(s.to_string()),
        Unichr::Surrogate(c) => Value::Str(PyStr::from_code_points(vec![*c]).expect("a surrogate")),
    }
}

/// An item of an iterated PDF object.
pub fn item(x: &IterItem) -> Value {
    match x {
        IterItem::Obj(o) => Value::from(o),
        IterItem::Str(s) => Value::str(s.to_string()),
    }
}

/// A number used as a dict key.
pub fn numkey(k: &NumKey) -> Value {
    num(k.num())
}

/// A CMap attribute.
pub fn attr(a: &AttrValue) -> Value {
    match a {
        AttrValue::Str(s) => Value::str(s.clone()),
        AttrValue::Obj(o) => Value::from(o),
    }
}

/// `dict(m.attrs)`
pub fn attrs(m: &dyn CMapBase) -> Value {
    Value::Dict(m.attrs().iter().map(|(k, v)| (Value::str(k.clone()), attr(v))).collect())
}

/// A result, or the exception.
pub fn res<T>(r: PyResult<T>, f: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => f(v),
        Err(e) => Value::from(&e),
    }
}

/// `Named(name, {fields})`
pub fn named(name: &str, fields: Vec<(&str, Value)>) -> Value {
    Value::Named { name: name.to_owned(), fields: fields.into_iter().map(|(k, v)| (Value::str(k), v)).collect() }
}

/// A tuple.
pub fn tuple(items: Vec<Value>) -> Value {
    Value::Tuple(items)
}

/// `at_depth(f, ...)` of the generator: `f` (a Python function) takes its
/// frame.
pub fn in_frame<T>(f: impl FnOnce() -> PyResult<T>) -> PyResult<T> {
    let _frame = py_frame()?;
    f()
}

/// `_umap(m)`
pub fn umap_report(m: Option<&UnicodeMap>) -> Value {
    let Some(m) = m else { return Value::None };
    named(
        m.class_name(),
        vec![
            ("attrs", attrs(m)),
            (
                "items",
                Value::List(m.cid2unichr.iter().map(|(k, v)| tuple(vec![Value::Int(k.clone()), unichr(v)])).collect()),
            ),
        ],
    )
}

/// `_cmap(m)`
pub fn cmap_report(m: &dyn CMapBase) -> Value {
    let mut fields = vec![("attrs", attrs(m))];
    if let Some(c) = m.as_cmap() {
        fields.push(("code2cid", Value::Int(PyInt::Small(c.code2cid.len() as i64))));
    }
    named(m.class_name(), fields)
}

/// A font's `fontname`.
pub fn fontname(n: &FontName) -> Value {
    match n {
        FontName::Str(s) => Value::str(s.clone()),
        FontName::Obj(o) => Value::from(o),
    }
}

/// `font.char_disp(cid)`
pub fn char_disp(d: &CharDisp) -> Value {
    match d {
        CharDisp::Scalar(n) => num(n),
        CharDisp::Vertical(vx, vy) => tuple(vec![item(vx), item(vy)]),
    }
}

/// A method called through `call_seq` (one more frame: `attempt`'s).
pub fn call_seq(f: impl FnOnce() -> PyResult<Operand>) -> Value {
    res(in_frame(f), |x| operand(&x))
}

/// `font_attrs(font)`: the attributes of a font, in a fixed order (absent
/// ones omitted), then `get_ascent`, `get_descent`, `get_width`,
/// `get_height` (through `call_seq`).
pub fn font_attrs(font: &PDFFont) -> Value {
    let mut fields: Vec<(&str, Value)> = Vec::new();
    let basefont = match &font.kind {
        FontKind::Simple(s) => s.basefont.clone(),
        FontKind::CID(c) => Some(c.basefont.clone()),
    };
    if let Some(b) = basefont {
        fields.push(("basefont", Value::str(b)));
    }
    fields.push(("fontname", fontname(&font.fontname)));
    fields.push(("flags", Value::from(&font.flags)));
    fields.push(("ascent", operand(&font.ascent)));
    fields.push(("descent", operand(&font.descent)));
    fields.push(("italic_angle", operand(&font.italic_angle)));
    fields.push(("default_width", item(&font.default_width)));
    fields.push(("leading", operand(&font.leading)));
    fields.push(("bbox", tuple(font.bbox.iter().map(|&f| Value::Float(f)).collect())));
    fields.push(("hscale", operand(&font.hscale)));
    fields.push(("vscale", operand(&font.vscale)));
    let widths = font
        .widths
        .iter()
        .map(|(k, v)| {
            let k = match k {
                WidthKey::Num(n) => numkey(n),
                WidthKey::Str(s) => Value::str(s.to_string()),
            };
            tuple(vec![k, Value::from(v)])
        })
        .collect();
    fields.push(("widths", Value::List(widths)));
    match &font.kind {
        FontKind::Simple(s) => {
            let items = s.cid2unicode.iter().map(|(k, v)| tuple(vec![Value::Int(k.clone()), Value::str(v.clone())]));
            fields.push(("cid2unicode", Value::List(items.collect())));
            fields.push(("unicode_map", umap_report(s.unicode_map.as_deref())));
            if s.fontfile.is_some() {
                fields.push(("fontfile", Value::Bool(true)));
            }
            if let Some(m) = &s.matrix {
                fields.push(("matrix", tuple(m.iter().map(Value::from).collect())));
            }
        }
        FontKind::CID(c) => {
            fields.push(("unicode_map", umap_report(c.unicode_map.as_deref())));
            if c.fontfile.is_some() {
                fields.push(("fontfile", Value::Bool(true)));
            }
            fields.push(("cidcoding", Value::str(c.cidcoding.clone())));
            fields.push(("cmap", cmap_report(&*c.cmap)));
            fields.push(("vertical", Value::Bool(c.vertical)));
            let disps = c
                .disps
                .iter()
                .map(|(k, (vx, vy))| tuple(vec![numkey(k), tuple(vec![Value::from(vx), Value::from(vy)])]));
            fields.push(("disps", Value::List(disps.collect())));
            fields.push((
                "default_disp",
                match &c.default_disp {
                    DefaultDisp::Zero => Value::Int(PyInt::ZERO),
                    DefaultDisp::Vertical(vy) => tuple(vec![Value::None, item(vy)]),
                },
            ));
        }
    }
    fields.push(("is_vertical", Value::Bool(font.is_vertical())));
    fields.push(("is_multibyte", Value::Bool(font.is_multibyte())));
    fields.push(("repr", Value::str(font.repr())));
    // called through call_seq from font_attrs, at the depth of the queries
    fields.push(("get_ascent", call_seq(|| font.get_ascent())));
    fields.push(("get_descent", call_seq(|| font.get_descent())));
    fields.push(("get_width", call_seq(|| font.get_width())));
    fields.push(("get_height", call_seq(|| font.get_height())));
    named(font.class_name(), fields)
}
