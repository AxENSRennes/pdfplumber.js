//! Replays the cases of `oracle/unit/psparser_base.py`: `PSStackParser`
//! over a `BytesIO` (`nexttoken`, `nextline`, `revreadlines`, `seek`,
//! `nextobject`, `pop`), compared with pdfminer.

use std::rc::Rc;

use pdfminer::psparser::{LIT_bytes, PSBaseParser, PSStackEntry, PSStackParser, KWD, LIT};
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

/// Calls `next` until it raises; the exception ends the list. Stops after
/// `limit` items: a parser cannot return more tokens or lines than its input
/// has bytes, so a broken one that loops without consuming input fails at
/// once instead of filling the memory.
fn until_error<T>(limit: usize, mut next: impl FnMut() -> PyResult<T>, mut value: impl FnMut(T) -> Value) -> Value {
    let mut items = Vec::new();
    loop {
        if items.len() > limit {
            items.push(Value::Str("too many items".into()));
            return Value::List(items);
        }
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
    // At most one token or line per byte, plus the end.
    let limit = data.len() + 2;
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
            "tokens" => until_error(limit, || p.nexttoken(), |t| entry(&t)),
            "nextline" => result(p.nextline(), line),
            "lines" => until_error(limit, || p.nextline(), line),
            "revreadlines" => result(p.revreadlines().map(|lines| lines.take(limit).collect::<Vec<_>>()), |l| {
                Value::List(l.into_iter().map(Value::Bytes).collect())
            }),
            "nextobject" => result(p.nextobject(), |e| entry(&e)),
            "pop" => {
                let popped = p.pop(arg() as usize);
                Value::List(popped.iter().map(entry).collect())
            }
            "popall" => Value::List(p.popall().iter().map(entry).collect()),
            "add_results" => {
                let popped = p.pop(arg() as usize);
                p.add_results(popped);
                Value::List(p.results.iter().map(entry).collect())
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

/// Interned symbols compare by identity, like Python's `is` on the objects
/// of `PSLiteralTable`/`PSKeywordTable`: the same name gives the same
/// symbol, in any thread, and a `str` name differs from a `bytes` one.
#[test]
fn interning_is_process_wide() {
    let intern =
        || (KWD(b"Tj"), LIT("F1"), LIT_bytes(b"\xff"), KWD(b"a-long-keyword"), LIT("a-long-literal-name"), KWD(b""));
    let here = intern();
    let there = std::thread::spawn(intern).join().expect("thread");
    assert_eq!(here, there);
    assert_eq!(here, intern());
    assert_ne!(LIT("F1"), LIT_bytes(b"F1"));
    assert_ne!(KWD(b"Tj"), KWD(b"TJ"));
    assert_ne!(KWD(b""), KWD(b"\x00"));
}
