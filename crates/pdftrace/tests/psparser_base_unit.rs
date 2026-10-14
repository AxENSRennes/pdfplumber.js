//! Replays the cases of `oracle/unit/psparser_base.py`: `PSStackParser`
//! over a `BytesIO` (`nexttoken`, `nextline`, `revreadlines`, `seek`,
//! `nextobject`, `pop`), compared with pdfminer.

use std::rc::Rc;

use pdfminer::psparser::{PSBaseParser, PSStackEntry, PSStackParser};
use pdftrace::unit::UnitData;
use pdftrace::value::Value;
use pycompat::io::BytesIO;
use pycompat::{PyInt, PyResult};

fn entry(e: &PSStackEntry) -> Value {
    Value::Tuple(vec![Value::Int(PyInt::from(e.0)), Value::from(&e.1)])
}

fn result<T>(r: PyResult<T>, ok: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => ok(v),
        Err(e) => Value::from(&e),
    }
}

/// Calls `next` until it raises; the exception ends the list.
fn until_error<T>(mut next: impl FnMut() -> PyResult<T>, mut value: impl FnMut(T) -> Value) -> Value {
    let mut items = Vec::new();
    loop {
        match next() {
            Ok(v) => items.push(value(v)),
            Err(e) => {
                items.push(Value::from(&e));
                return Value::List(items);
            }
        }
    }
}

fn line(l: (usize, Vec<u8>)) -> Value {
    Value::Tuple(vec![Value::Int(PyInt::from(l.0)), Value::Bytes(l.1)])
}

fn run(data: &[u8], ops: &[Value]) -> Value {
    let fp = BytesIO::new(Rc::from(data));
    let mut p: PSBaseParser = PSStackParser::init(Some(fp), ()).expect("a BytesIO parser opens");
    let mut out = Vec::new();
    for op in ops {
        let Value::List(op) = op else { panic!("{op:?}") };
        let Value::Str(kind) = &op[0] else { panic!() };
        let arg = || match op.get(1) {
            Some(Value::Int(i)) => i.as_i64().expect("small int"),
            other => panic!("{other:?}"),
        };
        out.push(match kind.as_str() {
            "seek" => result(p.seek(arg()), |()| Value::None),
            "nexttoken" => result(p.nexttoken(), |t| entry(&t)),
            "tokens" => until_error(|| p.nexttoken(), |t| entry(&t)),
            "nextline" => result(p.nextline(), line),
            "lines" => until_error(|| p.nextline(), line),
            "revreadlines" => {
                result(p.revreadlines().map(|lines| lines.collect::<Vec<_>>()), |l| {
                    Value::List(l.into_iter().map(Value::Bytes).collect())
                })
            }
            "nextobject" => result(p.nextobject(), |e| entry(&e)),
            "pop" => {
                let popped = p.pop(arg() as usize);
                Value::List(popped.iter().map(entry).collect())
            }
            "stack" => Value::List(vec![
                p.curtype.map_or(Value::None, |t| Value::Str(t.to_owned())),
                Value::List(p.curstack.iter().map(entry).collect()),
                Value::Int(PyInt::from(p.context.len())),
            ]),
            other => panic!("unknown op {other}"),
        });
    }
    Value::List(out)
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

#[test]
fn base_parser() {
    let cases = UnitData::open("psparser_base").cases("cases");
    assert!(cases.len() >= 3000);
    let mut failures = Vec::new();
    for (k, (args, expected)) in cases.iter().enumerate() {
        let Value::Bytes(data) = &args[0] else { panic!() };
        let Value::List(ops) = &args[1] else { panic!() };
        let actual = run(data, ops);
        if !matches(&actual, expected) {
            failures.push(format!("case {k}: ops {ops:?}\n    python: {expected:?}\n    rust:   {actual:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{}/{} cases differ; first ones:\n{}",
        failures.len(),
        cases.len(),
        failures.iter().take(5).map(|f| f.chars().take(3000).collect::<String>()).collect::<Vec<_>>().join("\n")
    );
}
