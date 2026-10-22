//! CPython 3.12 `json.dumps(obj, indent=indent)` with the default options
//! (`ensure_ascii`, `allow_nan`, `check_circular`, no `default`), what
//! pdfplumber/container.py:108 `Container.to_json` calls on the values its
//! `Serializer` produced.
//!
//! Two encoders, as in CPython (Lib/json/encoder.py
//! `JSONEncoder.iterencode`):
//!
//! - `indent=None`: the C accelerator (`Modules/_json.c`
//!   `encoder_listencode_obj`), which takes one C recursion unit per list,
//!   tuple or dict it enters (`" while encoding a JSON object"`), and one for
//!   the call of the encoder object itself (`_PyObject_MakeTpCall`);
//! - any other `indent`: the Python encoder (`_make_iterencode`), whose
//!   generators nest one frame per container (`_iterencode_list`,
//!   `_iterencode_dict`, below `_iterencode`), resumed by `list()` from C in
//!   `JSONEncoder.encode`; a float is written by `floatstr`, a frame more.
//!
//! Output: floats as `float.__repr__` (`NaN`, `Infinity`, `-Infinity`), ints
//! as `int.__repr__` (`ValueError` past 4 300 digits), strings escaped to
//! ASCII (`\uXXXX`, surrogate pairs, a lone surrogate as itself), item
//! separator `", "` (`","` with an indent), key separator `": "`. A value
//! of another type raises `TypeError("Object of type X is not JSON
//! serializable")`. The caller gives the values through [`JsonValue`].

use crate::err::{exc, PyResult};
use crate::int::PyInt;
use crate::pystr::PyStr;
use crate::recursion::{c_call, class_call_leaf, eval_entry, leaf, py_frame};
use crate::repr::float_repr;

/// What a value is for the encoder.
pub enum JsonKind<'a, T> {
    None,
    Bool(bool),
    Int(&'a PyInt),
    Float(f64),
    Str(&'a PyStr),
    /// A `list` or a `tuple`.
    List(&'a [T]),
    /// A `dict` with `str` keys.
    Dict(&'a [(PyStr, T)]),
    /// Any other type (its `__name__`).
    Other(&'static str),
}

/// A value the encoder can walk.
pub trait JsonValue: Sized {
    fn json_kind(&self) -> JsonKind<'_, Self>;
}

/// The `indent` of `json.dumps`: `None` (the C encoder), or the string
/// each level repeats (`' ' * indent` for an int, the `str` itself).
#[derive(Clone, Debug)]
pub enum JsonIndent {
    None,
    Str(String),
}

const WHILE_ENCODING: &str = " while encoding a JSON object";

/// `float.__repr__` and the specials of `floatstr`.
fn float_text(x: f64) -> String {
    if x.is_nan() {
        "NaN".to_owned()
    } else if x == f64::INFINITY {
        "Infinity".to_owned()
    } else if x == f64::NEG_INFINITY {
        "-Infinity".to_owned()
    } else {
        float_repr(x)
    }
}

fn type_error(name: &str) -> crate::err::PyErr {
    exc::TypeError(format!("Object of type {name} is not JSON serializable"))
}

// cpython-3.12.3/Lib/json/__init__.py:183 dumps
/// `json.dumps(obj, indent=indent)`. `indent` is what `_make_iterencode`
/// made of the caller's value (see [`JsonIndent`]); the caller raises the
/// `TypeError` of `' ' * indent` for an indent that is neither an int nor a
/// `str` (from the frame of `_make_iterencode`: [`dumps_indent_error`]).
pub fn dumps<T: JsonValue>(obj: &T, indent: &JsonIndent) -> PyResult<String> {
    let _dumps = py_frame()?;
    match indent {
        JsonIndent::None => {
            // _default_encoder.encode(obj)
            let _encode = py_frame()?;
            if let JsonKind::Str(s) = obj.json_kind() {
                return Ok(s.json());
            }
            // self.iterencode(o, _one_shot=True): c_make_encoder(...) then
            // the call of the encoder object (_PyObject_MakeTpCall)
            let _iterencode = py_frame()?;
            let _call = c_call(" while calling a Python object")?;
            let mut out = String::new();
            c_encode(obj, &mut out)?;
            Ok(out)
        }
        JsonIndent::Str(indent) => {
            // JSONEncoder(indent=indent, ...): the class call, __init__
            class_call_leaf()?;
            let _encode = py_frame()?;
            if let JsonKind::Str(s) = obj.json_kind() {
                return Ok(s.json());
            }
            // iterencode -> _make_iterencode: return the generator function
            leaf(2)?;
            // list(chunks): the generator _iterencode resumed from C
            let _entry = eval_entry()?;
            let _iterencode = py_frame()?;
            let mut out = String::new();
            let mut enc = PyEncoder { indent, out: &mut out };
            enc.value(obj, 0)?;
            Ok(out)
        }
    }
}

/// `json.dumps(obj, indent=indent)` for an `indent` that `' ' * indent`
/// rejects: the `TypeError` of `_make_iterencode`, three frames below
/// `dumps` (`encode`, `iterencode`, `_make_iterencode`) after the class
/// call of `JSONEncoder`.
pub fn dumps_indent_error(type_name: &str) -> PyResult<String> {
    let _dumps = py_frame()?;
    class_call_leaf()?;
    leaf(3)?;
    Err(exc::TypeError(format!("can't multiply sequence by non-int of type '{type_name}'")))
}

/// The C encoder: `encoder_listencode_obj`, iterative over containers.
fn c_encode<T: JsonValue>(obj: &T, out: &mut String) -> PyResult<()> {
    enum Task<'a, T> {
        Value(&'a T),
        Text(&'static str),
        Owned(String),
        Leave(crate::recursion::Guard),
    }
    let mut tasks: Vec<Task<'_, T>> = vec![Task::Value(obj)];
    while let Some(task) = tasks.pop() {
        let v = match task {
            Task::Value(v) => v,
            Task::Text(t) => {
                out.push_str(t);
                continue;
            }
            Task::Owned(t) => {
                out.push_str(&t);
                continue;
            }
            Task::Leave(g) => {
                drop(g);
                continue;
            }
        };
        match v.json_kind() {
            JsonKind::None => out.push_str("null"),
            JsonKind::Bool(b) => out.push_str(if b { "true" } else { "false" }),
            JsonKind::Int(i) => out.push_str(&i.py_str()?),
            JsonKind::Float(x) => out.push_str(&float_text(x)),
            JsonKind::Str(s) => out.push_str(&s.json()),
            JsonKind::List(items) => {
                let unit = c_call(WHILE_ENCODING)?;
                if items.is_empty() {
                    out.push_str("[]");
                    continue;
                }
                out.push('[');
                tasks.push(Task::Leave(unit));
                tasks.push(Task::Text("]"));
                for (i, x) in items.iter().enumerate().rev() {
                    tasks.push(Task::Value(x));
                    if i > 0 {
                        tasks.push(Task::Text(", "));
                    }
                }
            }
            JsonKind::Dict(items) => {
                let unit = c_call(WHILE_ENCODING)?;
                if items.is_empty() {
                    out.push_str("{}");
                    continue;
                }
                out.push('{');
                tasks.push(Task::Leave(unit));
                tasks.push(Task::Text("}"));
                for (i, (k, x)) in items.iter().enumerate().rev() {
                    tasks.push(Task::Value(x));
                    tasks.push(Task::Owned(format!("{}: ", k.json())));
                    if i > 0 {
                        tasks.push(Task::Text(", "));
                    }
                }
            }
            JsonKind::Other(name) => return Err(type_error(name)),
        }
    }
    Ok(())
}

/// The Python encoder (`_make_iterencode`, with an indent).
struct PyEncoder<'a> {
    indent: &'a str,
    out: &'a mut String,
}

impl PyEncoder<'_> {
    /// `_iterencode(o, level)` (its frame is the caller's) for a value that
    /// is not in a container, or the body of a container's item.
    fn value<T: JsonValue>(&mut self, v: &T, level: usize) -> PyResult<()> {
        match v.json_kind() {
            JsonKind::None => self.out.push_str("null"),
            JsonKind::Bool(b) => self.out.push_str(if b { "true" } else { "false" }),
            JsonKind::Int(i) => self.out.push_str(&i.py_str()?),
            JsonKind::Float(x) => {
                // _floatstr(value): a Python function
                leaf(1)?;
                self.out.push_str(&float_text(x));
            }
            JsonKind::Str(s) => self.out.push_str(&s.json()),
            JsonKind::List(items) => self.list(items, level)?,
            JsonKind::Dict(items) => self.dict(items, level)?,
            JsonKind::Other(name) => return Err(type_error(name)),
        }
        Ok(())
    }

    fn newline(&mut self, level: usize) {
        self.out.push('\n');
        for _ in 0..level {
            self.out.push_str(self.indent);
        }
    }

    // cpython-3.12.3/Lib/json/encoder.py:278 _iterencode_list
    fn list<T: JsonValue>(&mut self, items: &[T], level: usize) -> PyResult<()> {
        let _frame = py_frame()?;
        if items.is_empty() {
            self.out.push_str("[]");
            return Ok(());
        }
        self.out.push('[');
        let level = level + 1;
        for (i, v) in items.iter().enumerate() {
            if i > 0 {
                self.out.push(',');
            }
            self.newline(level);
            self.value(v, level)?;
        }
        self.newline(level - 1);
        self.out.push(']');
        Ok(())
    }

    // cpython-3.12.3/Lib/json/encoder.py:334 _iterencode_dict
    fn dict<T: JsonValue>(&mut self, items: &[(PyStr, T)], level: usize) -> PyResult<()> {
        let _frame = py_frame()?;
        if items.is_empty() {
            self.out.push_str("{}");
            return Ok(());
        }
        self.out.push('{');
        let level = level + 1;
        for (i, (k, v)) in items.iter().enumerate() {
            if i > 0 {
                self.out.push(',');
            }
            self.newline(level);
            self.out.push_str(&k.json());
            self.out.push_str(": ");
            self.value(v, level)?;
        }
        self.newline(level - 1);
        self.out.push('}');
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    enum V {
        N(f64),
        L(Vec<V>),
        D(Vec<(PyStr, V)>),
        S(PyStr),
    }

    impl JsonValue for V {
        fn json_kind(&self) -> JsonKind<'_, V> {
            match self {
                V::N(x) => JsonKind::Float(*x),
                V::L(l) => JsonKind::List(l),
                V::D(d) => JsonKind::Dict(d),
                V::S(s) => JsonKind::Str(s),
            }
        }
    }

    #[test]
    fn both_encoders() {
        let v = V::D(vec![
            (PyStr::new("a"), V::L(vec![V::N(1.5), V::N(f64::NAN), V::L(vec![])])),
            (PyStr::new("é"), V::S(PyStr::new("x\u{e9}"))),
        ]);
        assert_eq!(dumps(&v, &JsonIndent::None).unwrap(), r#"{"a": [1.5, NaN, []], "\u00e9": "x\u00e9"}"#);
        assert_eq!(
            dumps(&v, &JsonIndent::Str("  ".into())).unwrap(),
            "{\n  \"a\": [\n    1.5,\n    NaN,\n    []\n  ],\n  \"\\u00e9\": \"x\\u00e9\"\n}"
        );
    }
}
