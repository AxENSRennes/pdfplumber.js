//! Replays the cases of `oracle/unit/zlib.py` (CPython 3.12 `zlib` on zlib
//! 1.3) through `pycompat::zlib`: `zlib.decompress` and
//! `zlib.decompressobj().decompress` fed in pieces, outputs and error
//! messages compared exactly.

use pdftrace::canon::sha256_hex;
use pdftrace::unit::UnitData;
use pdftrace::value::Value;
use pycompat::zlib::{decompress_with, Decompress};
use pycompat::{PyInt, PyResult};

/// `BLOB_MAX` of the generator.
const BLOB_MAX: usize = 2048;

/// The generator's `blob(b)`.
fn blob(b: &[u8]) -> Value {
    if b.len() <= BLOB_MAX {
        Value::Bytes(b.to_vec())
    } else {
        Value::Tuple(vec![Value::str("sha256"), Value::Int(PyInt::from(b.len() as u64)), Value::str(sha256_hex(b))])
    }
}

fn result(r: PyResult<Vec<u8>>) -> Value {
    match r {
        Ok(b) => blob(&b),
        Err(e) => Value::from(&e),
    }
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(i) => i.as_i64().expect("an i64"),
        other => panic!("not an int: {other:?}"),
    }
}

fn bytes(v: &Value) -> &[u8] {
    match v {
        Value::Bytes(b) => b,
        other => panic!("not bytes: {other:?}"),
    }
}

fn check(name: &str, min_cases: usize, run: impl Fn(&[Value]) -> Value) {
    let data = UnitData::open("zlib");
    let cases = data.cases(name);
    assert!(cases.len() >= min_cases, "{name}: only {} cases", cases.len());
    let mut differ = Vec::new();
    for (k, (args, expected)) in cases.iter().enumerate() {
        let got = run(args);
        if !got.same(expected) {
            differ.push(format!("#{k}: expected {expected:?}\n     got {got:?}"));
        }
    }
    println!("{name}: {}/{} identical", cases.len() - differ.len(), cases.len());
    assert!(
        differ.is_empty(),
        "{name}: {} differ:\n{}",
        differ.len(),
        differ.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn decompress() {
    check("decompress", 10_000, |args| {
        let data = bytes(&args[0]);
        result(decompress_with(data, int(&args[1]) as i32, int(&args[2]) as isize))
    });
}

/// One `zlib.decompressobj(wbits)` fed by `calls` (the generator's `feed`).
fn feed(data: &[u8], wbits: i32, calls: &[Value]) -> Value {
    let mut d = match Decompress::new(wbits) {
        Ok(d) => d,
        Err(e) => {
            return Value::List(vec![
                Value::Int(PyInt::Small(0)),
                Value::Bytes(Vec::new()),
                Value::str(""),
                Value::from(&e),
                Value::Bytes(Vec::new()),
                Value::Bytes(Vec::new()),
                Value::Bool(false),
            ])
        }
    };
    let mut output = Vec::new();
    let mut lengths = Vec::new();
    let mut exc = Value::None;
    let mut pos = 0usize;
    'calls: for c in calls {
        let Value::List(c) = c else { panic!("a call") };
        let n = int(&c[0]) as usize;
        for _ in 0..int(&c[2]) {
            let mut chunk = d.unconsumed_tail.clone();
            chunk.extend_from_slice(&data[pos.min(data.len())..(pos + n).min(data.len())]);
            pos += n;
            match d.decompress_max(&chunk, int(&c[1]) as isize) {
                Ok(out) => {
                    lengths.push(out.len().to_string());
                    output.extend_from_slice(&out);
                }
                Err(e) => {
                    exc = Value::from(&e);
                    break 'calls;
                }
            }
        }
    }
    Value::List(vec![
        Value::Int(PyInt::from(lengths.len() as u64)),
        blob(&output),
        Value::str(sha256_hex(lengths.join(",").as_bytes())),
        exc,
        blob(&d.unused_data),
        blob(&d.unconsumed_tail),
        Value::Bool(d.eof),
    ])
}

#[test]
fn decompressobj() {
    check("decompressobj", 4_000, |args| {
        let data = bytes(&args[0]);
        let Value::List(feeds) = &args[2] else { panic!("feeds") };
        Value::List(
            feeds
                .iter()
                .map(|calls| {
                    let Value::List(calls) = calls else { panic!("calls") };
                    feed(data, int(&args[1]) as i32, calls)
                })
                .collect(),
        )
    });
}
