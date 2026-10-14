//! The typed JSON exchange format of `docs/verification.md` ("Format
//! d'échange typé"), shared with `oracle/typed_json.py`: Python values
//! encoded in JSON without loss, keeping int/float and tuple/list apart.
//!
//! | Python | JSON |
//! |---|---|
//! | `None`, `True`, `False`, `str` | `null`, `true`, `false`, string |
//! | `int` | `{"i": "<decimal>"}` |
//! | `float` | `{"f": "<16 hex digits of the IEEE-754 bits, big-endian>"}` |
//! | `bytes` | `{"b": "<hex>"}` |
//! | `list` | array |
//! | `tuple` | `{"t": [...]}` |
//! | `dict` | `{"d": [[key, value], ...]}` (insertion order) |
//! | `PSLiteral`, `PSKeyword` | `{"lit": <str or {"b": ...}>}`, `{"kw": "<hex>"}` |
//! | `PDFObjRef` | `{"ref": objid}` |
//! | exception | `{"exc": "<class>", "msg": "<str(e)>"}` |
//!
//! [`to_json`] writes what Python's `json.dumps` writes by default
//! (`ensure_ascii=True`, separators `", "` and `": "`). A Python `str` with a
//! lone surrogate cannot be a Rust `String` and is rejected by [`parse`].

use std::fmt::Write;

use pycompat::repr::write_json_str;
use pycompat::PyInt;

use crate::canon::hex;
use crate::value::{NameValue, Value};

/// Parses one typed JSON value.
pub fn parse(text: &str) -> Result<Value, String> {
    let json = parse_json(text)?;
    from_json(&json)
}

/// Writes a value as typed JSON (streams and named objects have no typed
/// JSON form).
pub fn to_json(v: &Value) -> Result<String, String> {
    let mut out = String::new();
    write_value(&mut out, v)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Plain JSON

/// A parsed JSON document. Numbers keep their source text.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// The value of `key` in an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Parses a JSON document (RFC 8259).
pub fn parse_json(text: &str) -> Result<Json, String> {
    let mut p = JsonParser { s: text.as_bytes(), i: 0 };
    let v = p.value()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(p.error("trailing characters"));
    }
    Ok(v)
}

struct JsonParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl JsonParser<'_> {
    fn error(&self, what: &str) -> String {
        format!("invalid JSON at byte {}: {what}", self.i)
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Result<(), String> {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected {:?}", c as char)))
        }
    }

    fn literal(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.error("unknown literal"))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        match self.s.get(self.i) {
            None => Err(self.error("unexpected end")),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'"') => Ok(Json::String(self.string()?)),
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Array(items));
                        }
                        _ => return Err(self.error("expected ',' or ']'")),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Object(items));
                }
                loop {
                    self.ws();
                    let key = self.string()?;
                    self.eat(b':')?;
                    items.push((key, self.value()?));
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Object(items));
                        }
                        _ => return Err(self.error("expected ',' or '}'")),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => {
                let start = self.i;
                while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    self.i += 1;
                }
                Ok(Json::Number(String::from_utf8_lossy(&self.s[start..self.i]).into_owned()))
            }
            Some(_) => Err(self.error("unexpected character")),
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.s.get(self.i..self.i + 4).ok_or_else(|| self.error("short \\u escape"))?;
        let text = std::str::from_utf8(digits).map_err(|_| self.error("bad \\u escape"))?;
        let v = u32::from_str_radix(text, 16).map_err(|_| self.error("bad \\u escape"))?;
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        if self.s.get(self.i) != Some(&b'"') {
            return Err(self.error("expected a string"));
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.s.len() && self.s[self.i] != b'"' && self.s[self.i] != b'\\' {
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.s[start..self.i]).map_err(|_| self.error("invalid UTF-8"))?);
            match self.s.get(self.i) {
                None => return Err(self.error("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(_) => {
                    self.i += 1;
                    let c = *self.s.get(self.i).ok_or_else(|| self.error("unterminated escape"))?;
                    self.i += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&hi) {
                                if self.s.get(self.i..self.i + 2) != Some(b"\\u") {
                                    return Err(self.error("lone surrogate"));
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return Err(self.error("lone surrogate"));
                                }
                                0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                            } else {
                                hi
                            };
                            out.push(char::from_u32(code).ok_or_else(|| self.error("lone surrogate"))?);
                        }
                        _ => return Err(self.error("unknown escape")),
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Typed values

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err(format!("odd-length hex {s:?}"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| format!("bad hex {s:?}")))
        .collect()
}

fn as_str(j: &Json) -> Result<&str, String> {
    match j {
        Json::String(s) => Ok(s),
        other => Err(format!("expected a JSON string, got {other:?}")),
    }
}

fn as_array(j: &Json) -> Result<&[Json], String> {
    match j {
        Json::Array(items) => Ok(items),
        other => Err(format!("expected a JSON array, got {other:?}")),
    }
}

fn parse_int(text: &str) -> Result<PyInt, String> {
    PyInt::from_decimal(text).ok_or_else(|| format!("bad int {text:?}"))
}

/// Decodes a parsed JSON value as a typed value.
pub fn from_json(j: &Json) -> Result<Value, String> {
    Ok(match j {
        Json::Null => Value::None,
        Json::Bool(b) => Value::Bool(*b),
        Json::String(s) => Value::Str(s.clone()),
        Json::Number(n) => return Err(format!("bare JSON number {n} (ints are {{\"i\": ...}})")),
        Json::Array(items) => Value::List(items.iter().map(from_json).collect::<Result<_, _>>()?),
        Json::Object(fields) => {
            let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
            match keys.as_slice() {
                ["i"] => Value::Int(parse_int(as_str(&fields[0].1)?)?),
                ["f"] => {
                    let bits = as_str(&fields[0].1)?;
                    if bits.len() != 16 {
                        return Err(format!("float bits {bits:?} are not 16 hex digits"));
                    }
                    Value::Float(f64::from_bits(
                        u64::from_str_radix(bits, 16).map_err(|_| format!("bad float bits {bits:?}"))?,
                    ))
                }
                ["b"] => Value::Bytes(unhex(as_str(&fields[0].1)?)?),
                ["t"] => Value::Tuple(as_array(&fields[0].1)?.iter().map(from_json).collect::<Result<_, _>>()?),
                ["d"] => {
                    let mut items = Vec::new();
                    for pair in as_array(&fields[0].1)? {
                        match as_array(pair)? {
                            [k, v] => items.push((from_json(k)?, from_json(v)?)),
                            _ => return Err("a dict item is not a [key, value] pair".into()),
                        }
                    }
                    Value::Dict(items)
                }
                ["lit"] => match &fields[0].1 {
                    Json::String(s) => Value::Literal(NameValue::Str(s.clone())),
                    other => match from_json(other)? {
                        Value::Bytes(b) => Value::Literal(NameValue::Bytes(b)),
                        _ => return Err("a literal name is a str or {\"b\": ...}".into()),
                    },
                },
                ["kw"] => Value::Keyword(unhex(as_str(&fields[0].1)?)?),
                ["ref"] => match &fields[0].1 {
                    Json::Number(n) => Value::Ref(parse_int(n)?),
                    _ => return Err("a ref is a JSON number".into()),
                },
                ["exc", "msg"] => Value::Exception {
                    class: as_str(&fields[0].1)?.to_owned(),
                    message: as_str(&fields[1].1)?.to_owned(),
                },
                _ => return Err(format!("unknown typed JSON object with keys {keys:?}")),
            }
        }
    })
}

fn write_tagged(out: &mut String, tag: &str, f: impl FnOnce(&mut String) -> Result<(), String>) -> Result<(), String> {
    out.push_str("{\"");
    out.push_str(tag);
    out.push_str("\": ");
    f(out)?;
    out.push('}');
    Ok(())
}

fn write_items(out: &mut String, items: &[Value]) -> Result<(), String> {
    out.push('[');
    for (i, x) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_value(out, x)?;
    }
    out.push(']');
    Ok(())
}

fn write_value(out: &mut String, v: &Value) -> Result<(), String> {
    match v {
        Value::None => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Str(s) => write_json_str(out, s),
        Value::Int(i) => write_tagged(out, "i", |out| {
            let _ = write!(out, "\"{i}\"");
            Ok(())
        })?,
        Value::Float(f) => write_tagged(out, "f", |out| {
            let _ = write!(out, "\"{:016x}\"", f.to_bits());
            Ok(())
        })?,
        Value::Bytes(b) => write_tagged(out, "b", |out| {
            let _ = write!(out, "\"{}\"", hex(b));
            Ok(())
        })?,
        Value::List(items) => write_items(out, items)?,
        Value::Tuple(items) => write_tagged(out, "t", |out| write_items(out, items))?,
        Value::Dict(items) => write_tagged(out, "d", |out| {
            out.push('[');
            for (i, (k, v)) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push('[');
                write_value(out, k)?;
                out.push_str(", ");
                write_value(out, v)?;
                out.push(']');
            }
            out.push(']');
            Ok(())
        })?,
        Value::Literal(name) => write_tagged(out, "lit", |out| match name {
            NameValue::Str(s) => {
                write_json_str(out, s);
                Ok(())
            }
            NameValue::Bytes(b) => write_value(out, &Value::Bytes(b.clone())),
        })?,
        Value::Keyword(name) => write_tagged(out, "kw", |out| {
            let _ = write!(out, "\"{}\"", hex(name));
            Ok(())
        })?,
        Value::Ref(objid) => write_tagged(out, "ref", |out| {
            let _ = write!(out, "{objid}");
            Ok(())
        })?,
        Value::Exception { class, message } => {
            out.push_str("{\"exc\": ");
            write_json_str(out, class);
            out.push_str(", \"msg\": ");
            write_json_str(out, message);
            out.push('}');
        }
        Value::Stream(_) => return Err("a PDFStream has no typed JSON form".into()),
        Value::Named { name, .. } => return Err(format!("a named object ({name}) has no typed JSON form")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! The examples of the table in docs/verification.md.
    use super::*;

    fn roundtrip(text: &str) -> Value {
        let v = parse(text).unwrap();
        assert_eq!(to_json(&v).unwrap(), text);
        v
    }

    #[test]
    fn doc_table_examples() {
        assert!(matches!(roundtrip("null"), Value::None));
        assert!(matches!(roundtrip("true"), Value::Bool(true)));
        assert!(matches!(roundtrip("\"caf\\u00e9\""), Value::Str(s) if s == "café"));
        assert!(matches!(roundtrip("{\"i\": \"-12\"}"), Value::Int(PyInt::Small(-12))));
        let big = roundtrip("{\"i\": \"123456789012345678901234567890\"}");
        assert!(matches!(big, Value::Int(PyInt::Big(_))));
        assert!(matches!(roundtrip("{\"f\": \"3ff0000000000000\"}"), Value::Float(f) if f == 1.0));
        assert!(matches!(roundtrip("{\"f\": \"8000000000000000\"}"), Value::Float(f) if f.to_bits() == 1 << 63));
        assert!(matches!(roundtrip("{\"b\": \"0d0a\"}"), Value::Bytes(b) if b == b"\r\n"));
        assert!(matches!(roundtrip("[{\"i\": \"1\"}, null]"), Value::List(v) if v.len() == 2));
        assert!(matches!(roundtrip("{\"t\": []}"), Value::Tuple(v) if v.is_empty()));
        assert!(matches!(roundtrip("{\"d\": [[\"Type\", {\"lit\": \"Page\"}]]}"), Value::Dict(v) if v.len() == 1));
        assert!(matches!(roundtrip("{\"lit\": {\"b\": \"ff\"}}"), Value::Literal(NameValue::Bytes(b)) if b == [0xff]));
        assert!(matches!(roundtrip("{\"kw\": \"4254\"}"), Value::Keyword(k) if k == b"BT"));
        assert!(matches!(roundtrip("{\"ref\": 12}"), Value::Ref(PyInt::Small(12))));
        assert!(matches!(
            roundtrip("{\"exc\": \"PSEOF\", \"msg\": \"Unexpected EOF\"}"),
            Value::Exception { class, message } if class == "PSEOF" && message == "Unexpected EOF"
        ));
        assert!(matches!(roundtrip("\"\\ud83d\\ude00\""), Value::Str(s) if s == "😀"));
        assert!(parse("\"\\ud83d\"").is_err());
        assert!(parse("{\"x\": 1}").is_err());
        assert!(parse("1").is_err());
    }
}
