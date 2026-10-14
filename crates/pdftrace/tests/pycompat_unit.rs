//! Replays the cases of `oracle/unit/pycompat.py` (CPython 3.12 results)
//! through `pycompat`. Each test fails with the first mismatches.
//!
//! Floats compare by bit pattern, except that any NaN matches any NaN (the
//! payload of a NaN is not observable through the Python code ported).

use pdftrace::unit::UnitData;
use pdftrace::value::Value;
use pycompat::builtins::{py_max, py_min, py_round, py_round_ndigits, py_sum};
use pycompat::float::{float_from_bytes, float_from_str};
use pycompat::io::BytesIO;
use pycompat::repr::{bytes_repr, float_repr, json_dumps_str, str_repr};
use pycompat::text::{self, tables};
use pycompat::{ExcKind, Num, PyInt, PyResult};

fn data() -> UnitData {
    UnitData::open("pycompat")
}

/// Exact comparison, with any NaN equal to any NaN.
fn matches(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
        (Value::List(x), Value::List(y)) | (Value::Tuple(x), Value::Tuple(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| matches(p, q))
        }
        _ => a.same(b),
    }
}

/// Runs `f` on every case of `name` and fails with the mismatches.
fn replay(name: &str, mut f: impl FnMut(&[Value]) -> Value) {
    let cases = data().cases(name);
    assert!(!cases.is_empty(), "{name}: no cases");
    let mut failures = Vec::new();
    for (args, expected) in &cases {
        let actual = f(args);
        if !matches(&actual, expected) {
            failures.push(format!("{args:?}\n    python: {expected:?}\n    rust:   {actual:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{name}: {}/{} cases differ; first ones:\n{}",
        failures.len(),
        cases.len(),
        failures.iter().take(15).cloned().collect::<Vec<_>>().join("\n")
    );
}

fn num(v: &Value) -> Num {
    match v {
        Value::Int(i) => Num::Int(i.clone()),
        Value::Float(f) => Num::Float(*f),
        Value::Bool(b) => Num::Int(PyInt::Small(i64::from(*b))),
        other => panic!("not a number: {other:?}"),
    }
}

fn num_value(n: Num) -> Value {
    match n {
        Num::Int(i) => Value::Int(i),
        Num::Float(f) => Value::Float(f),
    }
}

fn result<T>(r: PyResult<T>, ok: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => ok(v),
        Err(e) => Value::from(&e),
    }
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(PyInt::Small(i)) => *i,
        other => panic!("not a small int: {other:?}"),
    }
}

fn str_arg(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a str: {v:?}"))
}

fn bytes_arg(v: &Value) -> &[u8] {
    match v {
        Value::Bytes(b) => b,
        other => panic!("not bytes: {other:?}"),
    }
}

#[test]
fn exc_hierarchy() {
    // Every builtin/stdlib kind of ExcKind must exist in CPython with the
    // same name and bases.
    let cases = data().cases("exc_hierarchy");
    let qualified = |v: &Value| match v {
        Value::List(p) => format!("{}.{}", str_arg(&p[0]), str_arg(&p[1])),
        other => panic!("{other:?}"),
    };
    let mut checked = 0;
    for kind in ExcKind::ALL {
        if !matches!(kind.module(), "builtins" | "struct" | "zlib") {
            continue;
        }
        let key = format!("{}.{}", kind.module(), kind.qualname());
        let (_, out) = cases
            .iter()
            .find(|(args, _)| format!("{}.{}", str_arg(&args[0]), str_arg(&args[1])) == key)
            .unwrap_or_else(|| panic!("{key} is not a CPython exception class"));
        let Value::List(out) = out else { panic!() };
        assert_eq!(str_arg(&out[0]), kind.name(), "{key}: __name__");
        let Value::List(bases) = &out[1] else { panic!() };
        let python: Vec<String> = bases.iter().map(qualified).filter(|b| b != "builtins.object").collect();
        let rust: Vec<String> = kind.bases().iter().map(|b| format!("{}.{}", b.module(), b.qualname())).collect();
        assert_eq!(rust, python, "{key}: __bases__");
        checked += 1;
    }
    assert!(checked > 20);
}

#[test]
fn int_parse() {
    replay("int_parse", |a| {
        let base = int(&a[1]) as u32;
        let base = if int(&a[1]) < 0 { u32::MAX } else { base };
        let r = match &a[0] {
            Value::Str(s) => PyInt::parse_str(s.as_str().expect("no surrogates"), base),
            Value::Bytes(b) => PyInt::parse_bytes(b, base),
            other => panic!("{other:?}"),
        };
        result(r, Value::Int)
    });
}

#[test]
fn int_str() {
    replay("int_str", |a| {
        let Value::Int(i) = &a[0] else { panic!() };
        result(i.py_str(), Value::str)
    });
}

#[test]
fn int_to_float() {
    replay("int_to_float", |a| {
        let Value::Int(i) = &a[0] else { panic!() };
        result(i.to_f64(), Value::Float)
    });
}

#[test]
fn binop() {
    replay("binop", |a| {
        let (x, y) = (num(&a[1]), num(&a[2]));
        let r = match str_arg(&a[0]) {
            "+" => x.add(&y),
            "-" => x.sub(&y),
            "*" => x.mul(&y),
            "/" => x.truediv(&y),
            "//" => x.floordiv(&y),
            "%" => x.modulo(&y),
            "**" => x.pow(&y),
            op => panic!("{op}"),
        };
        result(r, num_value)
    });
}

#[test]
fn compare() {
    replay("compare", |a| {
        let (x, y) = (num(&a[0]), num(&a[1]));
        let ne = !x.py_eq(&y);
        Value::List(
            [x.py_lt(&y), x.py_le(&y), x.py_eq(&y), ne, x.py_gt(&y), x.py_ge(&y)]
                .into_iter()
                .map(Value::Bool)
                .collect(),
        )
    });
}

#[test]
fn sum() {
    replay("sum", |a| {
        let Value::List(items) = &a[0] else { panic!() };
        let start = a.get(1).map_or(Num::Int(PyInt::ZERO), num);
        result(py_sum(items.iter().map(num), start), num_value)
    });
}

#[test]
fn minmax() {
    replay("minmax", |a| {
        let Value::List(items) = &a[0] else { panic!() };
        let items: Vec<Num> = items.iter().map(num).collect();
        Value::List(vec![result(py_min(items.clone()), num_value), result(py_max(items), num_value)])
    });
}

#[test]
fn float_repr_() {
    replay("float_repr", |a| {
        let Value::Float(f) = &a[0] else { panic!() };
        Value::str(float_repr(*f))
    });
}

#[test]
fn round() {
    replay("round", |a| {
        let x = num(&a[0]);
        match a.get(1) {
            None => result(py_round(&x), Value::Int),
            Some(n) => result(py_round_ndigits(&x, int(n)), num_value),
        }
    });
}

#[test]
fn float_parse() {
    replay("float_parse", |a| match &a[0] {
        Value::Str(s) => result(float_from_str(s.as_str().expect("no surrogates")), Value::Float),
        Value::Bytes(b) => result(float_from_bytes(b), Value::Float),
        other => panic!("{other:?}"),
    });
}

#[test]
fn str_repr_() {
    replay("str_repr", |a| Value::str(str_repr(str_arg(&a[0]))));
}

#[test]
fn json_str() {
    replay("json_str", |a| Value::str(json_dumps_str(str_arg(&a[0]))));
}

#[test]
fn bytes_repr_() {
    replay("bytes_repr", |a| Value::str(bytes_repr(bytes_arg(&a[0]))));
}

#[test]
fn utf8_ignore() {
    replay("utf8_ignore", |a| Value::str(text::decode_utf8_ignore(bytes_arg(&a[0]))));
}

fn range_list(ranges: &[(u32, u32)]) -> Value {
    Value::List(
        ranges
            .iter()
            .map(|&(lo, hi)| Value::List(vec![Value::Int(i64::from(lo).into()), Value::Int(i64::from(hi).into())]))
            .collect(),
    )
}

#[test]
fn unicode_tables() {
    replay("unicode", |a| match str_arg(&a[0]) {
        "unidata_version" => Value::str(text::UNICODE_VERSION.to_owned()),
        "nonprintable" => range_list(tables::NONPRINTABLE),
        "space" => range_list(tables::SPACE),
        "decimal" => Value::List(
            (0..=0x10ffffu32)
                .filter_map(char::from_u32)
                .filter_map(|c| text::decimal_value(c).map(|d| (c, d)))
                .map(|(c, d)| {
                    Value::List(vec![Value::Int(i64::from(c as u32).into()), Value::Int(i64::from(d).into())])
                })
                .collect(),
        ),
        "bytes_isspace" | "bytes_re_s" => Value::List(
            (0..=255u8).filter(|&c| text::bytes_isspace(c)).map(|c| Value::Int(i64::from(c).into())).collect(),
        ),
        other => panic!("{other}"),
    });
    // The class functions agree with the tables on every scalar value.
    for c in (0..=0x10ffffu32).filter_map(char::from_u32) {
        let in_table = |t: &[(u32, u32)]| t.iter().any(|&(lo, hi)| (lo..=hi).contains(&(c as u32)));
        assert_eq!(text::str_isprintable(c), !in_table(tables::NONPRINTABLE), "{c:?}");
        assert_eq!(text::str_isspace(c), in_table(tables::SPACE), "{c:?}");
    }
}

#[test]
fn bytesio() {
    replay("bytesio", |a| {
        let mut f = BytesIO::new(bytes_arg(&a[0]).into());
        let Value::List(ops) = &a[1] else { panic!() };
        let results = ops
            .iter()
            .map(|op| {
                let Value::List(op) = op else { panic!() };
                match str_arg(&op[0]) {
                    "seek" => result(f.seek_whence(int(&op[1]), int(&op[2]) as i32), |p| Value::Int(p.into())),
                    "read" => {
                        let n = match &op[1] {
                            Value::None => None,
                            v if int(v) < 0 => None,
                            v => Some(int(v) as usize),
                        };
                        Value::Bytes(f.read_bytes(n).to_vec())
                    }
                    "tell" => Value::Int(f.tell().into()),
                    other => panic!("{other}"),
                }
            })
            .collect();
        Value::List(results)
    });
}
