//! The typed JSON exchange format of `docs/verification.md` ("Typed
//! JSON"), shared with `oracle/typed_json.py`: Python values
//! encoded in JSON without loss, keeping int/float and tuple/list apart.
//!
//! | Python | JSON |
//! |---|---|
//! | `None`, `True`, `False`, `str` | `null`, `true`, `false`, string |
//! | `str` holding a surrogate | `{"s": [<code point>, ...]}` |
//! | `int` | `{"i": "<decimal>"}` |
//! | `float` | `{"f": "<16 hex digits of the IEEE-754 bits, big-endian>"}` |
//! | `bytes` | `{"b": "<hex>"}` |
//! | `list` | array |
//! | `tuple` | `{"t": [...]}` |
//! | `dict` | `{"d": [[key, value], ...]}` (insertion order) |
//! | `PSLiteral`, `PSKeyword` | `{"lit": <str or {"b": ...}>}`, `{"kw": "<hex>"}` |
//! | `PDFObjRef` | `{"ref": objid}` (a JSON integer when it fits in an i64, else `{"i": ...}`) |
//! | `PDFStream` | `{"stream": {"objid": v, "genno": v, "attrs": {"d": ...}, "rawdata": v, "data": v}}` |
//! | exception | `{"exc": "<class>", "msg": <str(e)>}` |
//! | named object | `{"named": "<name>", "d": [[key, value], ...]}` |
//!
//! [`to_json`] writes exactly what `oracle/typed_json.py` `dumps` writes
//! (compact, ASCII, keys in the order above), so a re-encoding can be
//! compared with the Python text.

use std::fmt::Write;

use pycompat::repr::write_json_str;
use pycompat::PyInt;

use crate::canon::hex;
use crate::value::{NameValue, PyStr, StreamValue, Value};

/// Parses one typed JSON value.
pub fn parse(text: &str) -> Result<Value, String> {
    let json = parse_json(text)?;
    from_json(&json)
}

/// Writes a value as typed JSON, as `oracle/typed_json.py` `dumps` does.
pub fn to_json(v: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, v);
    out
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

impl Drop for Json {
    /// Drops nested arrays and objects with a work list, not by recursion.
    fn drop(&mut self) {
        let mut stack: Vec<Json> = Vec::new();
        let take = |j: &mut Json, stack: &mut Vec<Json>| match j {
            Json::Array(items) => stack.append(items),
            Json::Object(fields) => stack.extend(fields.drain(..).map(|(_, v)| v)),
            _ => {}
        };
        take(self, &mut stack);
        while let Some(mut j) = stack.pop() {
            take(&mut j, &mut stack);
        }
    }
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

    /// One JSON value, parsed with an explicit stack of the arrays and
    /// objects being read (any depth).
    fn value(&mut self) -> Result<Json, String> {
        enum Open {
            Array(Vec<Json>),
            /// The items read so far and the key of the value being read.
            Object(Vec<(String, Json)>, String),
        }
        let mut open: Vec<Open> = Vec::new();
        loop {
            self.ws();
            let mut value = match self.s.get(self.i) {
                None => return Err(self.error("unexpected end")),
                Some(b'n') => self.literal("null", Json::Null)?,
                Some(b't') => self.literal("true", Json::Bool(true))?,
                Some(b'f') => self.literal("false", Json::Bool(false))?,
                Some(b'"') => Json::String(self.string()?),
                Some(b'[') => {
                    self.i += 1;
                    self.ws();
                    if self.s.get(self.i) == Some(&b']') {
                        self.i += 1;
                        Json::Array(Vec::new())
                    } else {
                        open.push(Open::Array(Vec::new()));
                        continue;
                    }
                }
                Some(b'{') => {
                    self.i += 1;
                    self.ws();
                    if self.s.get(self.i) == Some(&b'}') {
                        self.i += 1;
                        Json::Object(Vec::new())
                    } else {
                        let key = self.key()?;
                        open.push(Open::Object(Vec::new(), key));
                        continue;
                    }
                }
                Some(b'-' | b'0'..=b'9') => {
                    let start = self.i;
                    while self.i < self.s.len()
                        && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                    {
                        self.i += 1;
                    }
                    Json::Number(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
                }
                Some(_) => return Err(self.error("unexpected character")),
            };
            // Add the value to the innermost container; close the containers
            // that end here.
            loop {
                self.ws();
                match open.last_mut() {
                    None => return Ok(value),
                    Some(Open::Array(items)) => {
                        items.push(value);
                        match self.s.get(self.i) {
                            Some(b',') => {
                                self.i += 1;
                                break;
                            }
                            Some(b']') => {
                                self.i += 1;
                                let Some(Open::Array(items)) = open.pop() else { unreachable!() };
                                value = Json::Array(items);
                            }
                            _ => return Err(self.error("expected ',' or ']'")),
                        }
                    }
                    Some(Open::Object(items, key)) => {
                        items.push((std::mem::take(key), value));
                        match self.s.get(self.i) {
                            Some(b',') => {
                                self.i += 1;
                                *key = self.key()?;
                                break;
                            }
                            Some(b'}') => {
                                self.i += 1;
                                let Some(Open::Object(items, _)) = open.pop() else { unreachable!() };
                                value = Json::Object(items);
                            }
                            _ => return Err(self.error("expected ',' or '}'")),
                        }
                    }
                }
            }
        }
    }

    /// An object key and its `:`.
    fn key(&mut self) -> Result<String, String> {
        self.ws();
        let key = self.string()?;
        self.eat(b':')?;
        Ok(key)
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

/// A `str`: a JSON string, or `{"s": [code points]}`.
fn py_str(j: &Json) -> Result<PyStr, String> {
    match j {
        Json::String(s) => Ok(PyStr::Text(s.clone())),
        Json::Object(fields) if fields.len() == 1 && fields[0].0 == "s" => {
            let code_points = as_array(&fields[0].1)?
                .iter()
                .map(|c| match c {
                    Json::Number(n) => n.parse::<u32>().map_err(|_| format!("bad code point {n}")),
                    other => Err(format!("bad code point {other:?}")),
                })
                .collect::<Result<Vec<u32>, String>>()?;
            PyStr::from_code_points(code_points).ok_or_else(|| "code point above U+10FFFF".to_owned())
        }
        other => Err(format!("expected a str, got {other:?}")),
    }
}

/// The `[key, value]` pairs of a `{"d": ...}` (or `named`) array.
fn pair_list(j: &Json) -> Result<Vec<&Json>, String> {
    let mut out = Vec::new();
    for pair in as_array(j)? {
        match as_array(pair)? {
            [k, v] => out.extend([k, v]),
            _ => return Err("a dict item is not a [key, value] pair".to_owned()),
        }
    }
    Ok(out)
}

/// Pairs from a flat key, value, key, value... list.
fn into_pairs(flat: Vec<Value>) -> Vec<(Value, Value)> {
    let mut pairs = Vec::with_capacity(flat.len() / 2);
    let mut flat = flat.into_iter();
    while let (Some(k), Some(v)) = (flat.next(), flat.next()) {
        pairs.push((k, v));
    }
    pairs
}

/// A container of the typed JSON being decoded, completed when its children are.
enum Build {
    List(usize),
    Tuple(usize),
    Dict(usize),
    Named(String, usize),
    /// `objid`, `genno`, the attrs pairs, `rawdata`, `data`.
    Stream(usize),
    Ref,
}

/// Decodes a parsed JSON value as a typed value (any depth: children are
/// decoded with an explicit stack, then their containers built).
pub fn from_json(j: &Json) -> Result<Value, String> {
    enum Step<'a> {
        Enter(&'a Json),
        Build(Build),
    }
    let mut steps = vec![Step::Enter(j)];
    let mut done: Vec<Value> = Vec::new();
    while let Some(step) = steps.pop() {
        let (build, children): (Build, Vec<&Json>) = match step {
            Step::Enter(j) => match leaf_or_container(j)? {
                Ok(leaf) => {
                    done.push(leaf);
                    continue;
                }
                Err(container) => container,
            },
            Step::Build(build) => {
                let value = match build {
                    Build::List(n) => Value::List(done.split_off(done.len() - n)),
                    Build::Tuple(n) => Value::Tuple(done.split_off(done.len() - n)),
                    Build::Dict(n) => Value::Dict(into_pairs(done.split_off(done.len() - n))),
                    Build::Named(name, n) => Value::Named { name, fields: into_pairs(done.split_off(done.len() - n)) },
                    Build::Stream(n) => {
                        let mut fields = done.split_off(done.len() - n).into_iter();
                        let (objid, genno) = (fields.next().expect("objid"), fields.next().expect("genno"));
                        let mut rest: Vec<Value> = fields.collect();
                        let data = rest.pop().expect("data");
                        let raw = rest.pop().expect("rawdata");
                        Value::Stream(Box::new(StreamValue { objid, genno, attrs: into_pairs(rest), raw, data }))
                    }
                    Build::Ref => match done.pop().expect("objid") {
                        Value::Int(ref i) => Value::Ref(i.clone()),
                        v => return Err(format!("a ref objid that is not an int: {v:?}")),
                    },
                };
                done.push(value);
                continue;
            }
        };
        steps.push(Step::Build(build));
        steps.extend(children.into_iter().rev().map(Step::Enter));
    }
    Ok(done.pop().expect("one value"))
}

/// A scalar typed value, or the container to build and its children.
#[allow(clippy::type_complexity)]
fn leaf_or_container(j: &Json) -> Result<Result<Value, (Build, Vec<&Json>)>, String> {
    Ok(Ok(match j {
        Json::Null => Value::None,
        Json::Bool(b) => Value::Bool(*b),
        Json::String(s) => Value::Str(PyStr::Text(s.clone())),
        Json::Number(n) => return Err(format!("bare JSON number {n} (ints are {{\"i\": ...}})")),
        Json::Array(items) => return Ok(Err((Build::List(items.len()), items.iter().collect()))),
        Json::Object(fields) => {
            let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
            let Some((_, first)) = fields.first() else {
                return Err("empty JSON object".into());
            };
            match keys.as_slice() {
                ["i"] => Value::Int(parse_int(as_str(first)?)?),
                ["f"] => {
                    let bits = as_str(first)?;
                    if bits.len() != 16 {
                        return Err(format!("float bits {bits:?} are not 16 hex digits"));
                    }
                    Value::Float(f64::from_bits(
                        u64::from_str_radix(bits, 16).map_err(|_| format!("bad float bits {bits:?}"))?,
                    ))
                }
                ["s"] => Value::Str(py_str(j)?),
                ["b"] => Value::Bytes(unhex(as_str(first)?)?),
                ["t"] => {
                    let items = as_array(first)?;
                    return Ok(Err((Build::Tuple(items.len()), items.iter().collect())));
                }
                ["d"] => {
                    let flat = pair_list(first)?;
                    return Ok(Err((Build::Dict(flat.len()), flat)));
                }
                ["lit"] => match first {
                    Json::Object(f) if f.len() == 1 && f[0].0 == "b" => {
                        Value::Literal(NameValue::Bytes(unhex(as_str(&f[0].1)?)?))
                    }
                    other => Value::Literal(NameValue::Str(py_str(other)?)),
                },
                ["kw"] => Value::Keyword(unhex(as_str(first)?)?),
                ["ref"] => match first {
                    Json::Number(n) => Value::Ref(parse_int(n)?),
                    other => return Ok(Err((Build::Ref, vec![other]))),
                },
                ["stream"] => {
                    let field = |name: &str| first.get(name).ok_or_else(|| format!("a stream has no {name:?}"));
                    let keys: Vec<&str> = match first {
                        Json::Object(fields) => fields.iter().map(|(k, _)| k.as_str()).collect(),
                        _ => Vec::new(),
                    };
                    if keys != ["objid", "genno", "attrs", "rawdata", "data"] {
                        return Err(format!("stream keys {keys:?}"));
                    }
                    let attrs = match field("attrs")? {
                        Json::Object(f) if f.len() == 1 && f[0].0 == "d" => pair_list(&f[0].1)?,
                        other => return Err(format!("stream attrs {other:?}")),
                    };
                    let mut children = vec![field("objid")?, field("genno")?];
                    children.extend(attrs);
                    children.extend([field("rawdata")?, field("data")?]);
                    return Ok(Err((Build::Stream(children.len()), children)));
                }
                ["exc", "msg"] => Value::Exception { class: as_str(first)?.to_owned(), message: py_str(&fields[1].1)? },
                ["named", "d"] => {
                    let flat = pair_list(&fields[1].1)?;
                    return Ok(Err((Build::Named(as_str(first)?.to_owned(), flat.len()), flat)));
                }
                _ => return Err(format!("unknown typed JSON object with keys {keys:?}")),
            }
        }
    }))
}

fn write_py_str(out: &mut String, s: &PyStr) {
    match s {
        PyStr::Text(s) => write_json_str(out, s),
        PyStr::CodePoints(c) => {
            out.push_str("{\"s\":[");
            for (i, cp) in c.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{cp}");
            }
            out.push_str("]}");
        }
    }
}

fn write_int(out: &mut String, i: &PyInt) {
    let _ = write!(out, "{{\"i\":\"{i}\"}}");
}

/// A step of [`write_value`].
enum Out<'a> {
    Value(&'a Value),
    Text(&'static str),
}

/// Pushes the steps that write `[a,b,...]`.
fn items<'a>(steps: &mut Vec<Out<'a>>, items: &'a [Value]) {
    steps.push(Out::Text("]"));
    for (i, x) in items.iter().enumerate().rev() {
        steps.push(Out::Value(x));
        if i > 0 {
            steps.push(Out::Text(","));
        }
    }
    steps.push(Out::Text("["));
}

/// Pushes the steps that write `[[k,v],...]`.
fn pairs<'a>(steps: &mut Vec<Out<'a>>, pairs: &'a [(Value, Value)]) {
    steps.push(Out::Text("]"));
    for (i, (k, v)) in pairs.iter().enumerate().rev() {
        steps.extend([Out::Text("]"), Out::Value(v), Out::Text(","), Out::Value(k), Out::Text("[")]);
        if i > 0 {
            steps.push(Out::Text(","));
        }
    }
    steps.push(Out::Text("["));
}

/// Writes `v` as typed JSON, with an explicit stack (any depth).
fn write_value(out: &mut String, v: &Value) {
    let mut steps = vec![Out::Value(v)];
    while let Some(step) = steps.pop() {
        let v = match step {
            Out::Text(t) => {
                out.push_str(t);
                continue;
            }
            Out::Value(v) => v,
        };
        match v {
            Value::None => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Str(s) => write_py_str(out, s),
            Value::Int(i) => write_int(out, i),
            Value::Float(f) => {
                let _ = write!(out, "{{\"f\":\"{:016x}\"}}", f.to_bits());
            }
            Value::Bytes(b) => {
                let _ = write!(out, "{{\"b\":\"{}\"}}", hex(b));
            }
            Value::List(list) => items(&mut steps, list),
            Value::Tuple(list) => {
                out.push_str("{\"t\":");
                steps.push(Out::Text("}"));
                items(&mut steps, list);
            }
            Value::Dict(list) => {
                out.push_str("{\"d\":");
                steps.push(Out::Text("}"));
                pairs(&mut steps, list);
            }
            Value::Literal(name) => {
                out.push_str("{\"lit\":");
                match name {
                    NameValue::Str(s) => write_py_str(out, s),
                    NameValue::Bytes(b) => {
                        let _ = write!(out, "{{\"b\":\"{}\"}}", hex(b));
                    }
                }
                out.push('}');
            }
            Value::Keyword(name) => {
                let _ = write!(out, "{{\"kw\":\"{}\"}}", hex(name));
            }
            Value::Ref(objid) => {
                out.push_str("{\"ref\":");
                match objid {
                    PyInt::Small(i) => {
                        let _ = write!(out, "{i}");
                    }
                    PyInt::Big(_) => write_int(out, objid),
                }
                out.push('}');
            }
            Value::Stream(s) => {
                out.push_str("{\"stream\":{\"objid\":");
                steps.extend([Out::Text("}}"), Out::Value(&s.data), Out::Text(",\"data\":")]);
                steps.extend([Out::Value(&s.raw), Out::Text("},\"rawdata\":")]);
                pairs(&mut steps, &s.attrs);
                steps.extend([Out::Text(",\"attrs\":{\"d\":"), Out::Value(&s.genno), Out::Text(",\"genno\":")]);
                steps.push(Out::Value(&s.objid));
            }
            Value::Exception { class, message } => {
                out.push_str("{\"exc\":");
                write_json_str(out, class);
                out.push_str(",\"msg\":");
                write_py_str(out, message);
                out.push('}');
            }
            Value::Named { name, fields } => {
                out.push_str("{\"named\":");
                write_json_str(out, name);
                out.push_str(",\"d\":");
                steps.push(Out::Text("}"));
                pairs(&mut steps, fields);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! The examples of the table in docs/verification.md ("Typed JSON");
    //! the exhaustive cross-test against `oracle/typed_json.py` is
    //! `tests/canon_unit.rs`.
    use super::*;

    fn roundtrip(text: &str) -> Value {
        let v = parse(text).unwrap();
        assert_eq!(to_json(&v), text);
        v
    }

    #[test]
    fn doc_table_examples() {
        assert!(matches!(&roundtrip("null"), Value::None));
        assert!(matches!(&roundtrip("true"), Value::Bool(true)));
        assert!(matches!(&roundtrip("\"caf\\u00e9\""), Value::Str(s) if s == "café"));
        assert!(matches!(&roundtrip("{\"i\":\"-12\"}"), Value::Int(PyInt::Small(-12))));
        let big = roundtrip("{\"i\":\"123456789012345678901234567890\"}");
        assert!(matches!(&big, Value::Int(PyInt::Big(_))));
        assert!(matches!(&roundtrip("{\"f\":\"3ff0000000000000\"}"), Value::Float(f) if *f == 1.0));
        assert!(matches!(&roundtrip("{\"f\":\"8000000000000000\"}"), Value::Float(f) if f.to_bits() == 1 << 63));
        assert!(matches!(&roundtrip("{\"b\":\"0d0a\"}"), Value::Bytes(b) if b == b"\r\n"));
        assert!(matches!(&roundtrip("[{\"i\":\"1\"},null]"), Value::List(v) if v.len() == 2));
        assert!(matches!(&roundtrip("{\"t\":[]}"), Value::Tuple(v) if v.is_empty()));
        assert!(matches!(&roundtrip("{\"d\":[[\"Type\",{\"lit\":\"Page\"}]]}"), Value::Dict(v) if v.len() == 1));
        assert!(matches!(&roundtrip("{\"lit\":{\"b\":\"ff\"}}"), Value::Literal(NameValue::Bytes(b)) if *b == [0xff]));
        assert!(matches!(&roundtrip("{\"kw\":\"4254\"}"), Value::Keyword(k) if k == b"BT"));
        assert!(matches!(&roundtrip("{\"ref\":12}"), Value::Ref(PyInt::Small(12))));
        assert!(matches!(&roundtrip("{\"ref\":{\"i\":\"9223372036854775808\"}}"), Value::Ref(PyInt::Big(_))));
        assert!(matches!(
            &roundtrip("{\"exc\":\"PSEOF\",\"msg\":\"Unexpected EOF\"}"),
            Value::Exception { class, message } if class == "PSEOF" && message == "Unexpected EOF"
        ));
        assert!(matches!(&roundtrip("\"\\ud83d\\ude00\""), Value::Str(s) if s == "😀"));
        // A str holding surrogates; two code points that form a pair stay two.
        assert!(matches!(&roundtrip("{\"s\":[120,55296]}"), Value::Str(PyStr::CodePoints(c)) if *c == [120, 0xd800]));
        assert!(matches!(&roundtrip("{\"s\":[55357,56832]}"), Value::Str(PyStr::CodePoints(c)) if c.len() == 2));
        assert!(matches!(
            &roundtrip("{\"lit\":{\"s\":[56320]}}"),
            Value::Literal(NameValue::Str(PyStr::CodePoints(_)))
        ));
        roundtrip(
            "{\"stream\":{\"objid\":{\"i\":\"5\"},\"genno\":{\"i\":\"0\"},\"attrs\":{\"d\":[[\"Length\",{\"i\":\"3\"}]]},\
             \"rawdata\":null,\"data\":{\"b\":\"616263\"}}}",
        );
        roundtrip("{\"named\":\"LTChar\",\"d\":[[\"x0\",{\"f\":\"3ff0000000000000\"}]]}");
        assert!(parse("\"\\ud83d\"").is_err());
        assert!(parse("{\"x\":1}").is_err());
        assert!(parse("1").is_err());
    }
}
