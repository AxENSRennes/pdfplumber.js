//! Cross-test of `canon.rs` and `typed_json.rs` against `oracle/canon.py`
//! and `oracle/typed_json.py`, on the cases of `oracle/unit/canon.py`
//! (edge cases of every row of the canon table, then random values).
//!
//! Each line is `{"in":[<typed JSON value>],"out":"<canon(value)>"}`. For
//! every case, the Rust side must
//! - decode the value with `typed_json::from_json`,
//! - write exactly the Python canon with `canon::canon`,
//! - re-encode exactly the Python text with `typed_json::to_json`.

use std::fs;

use pdftrace::canon::{canon, sha256_hex};
use pdftrace::typed_json::{from_json, parse_json, to_json, Json};
use pdftrace::unit::UnitData;
use pdftrace::value::Value;

/// The text of the typed value in a line written by `oracle/unit/_common.py`
/// (`{"in":[` value `],"out":` json string `}`). The `out` string is
/// escaped JSON, so the last `],"out":"` of the line is the separator.
fn value_text(line: &str) -> &str {
    let start = "{\"in\":[".len();
    let end = line.rfind("],\"out\":\"").expect("an \"out\" string");
    &line[start..end]
}

#[test]
fn canon_and_typed_json_match_python() {
    let data = UnitData::open("canon");
    let path = data.path("cases.jsonl");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut failures = Vec::new();
    let mut count = 0;
    for (n, line) in text.lines().enumerate() {
        let json = parse_json(line).unwrap_or_else(|e| panic!("line {}: {e}", n + 1));
        let Some(Json::Array(args)) = json.get("in") else { panic!("line {}: no \"in\"", n + 1) };
        let Some(Json::String(expected)) = json.get("out") else { panic!("line {}: no \"out\"", n + 1) };
        count += 1;
        let value: Value = match from_json(&args[0]) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("line {}: typed JSON not decoded: {e}", n + 1));
                continue;
            }
        };
        let written = canon(&value);
        if &written != expected {
            failures.push(format!("line {}: canon\n    python: {expected}\n    rust:   {written}", n + 1));
        }
        let encoded = to_json(&value);
        if encoded != value_text(line) {
            failures.push(format!(
                "line {}: typed JSON\n    python: {}\n    rust:   {encoded}",
                n + 1,
                value_text(line)
            ));
        }
    }
    assert!(count > 6000, "only {count} cases");
    assert!(
        failures.is_empty(),
        "{}/{count} cases differ; first ones:\n{}",
        failures.len(),
        failures.iter().take(15).cloned().collect::<Vec<_>>().join("\n")
    );
}

/// Values nested 2*10^5 to 10^6 deep (`deep.jsonl`): parsed, decoded,
/// written by `canon`, re-encoded, compared and freed without recursion
/// (as `oracle/canon.py` writes them).
#[test]
fn deep_values() {
    let data = UnitData::open("canon");
    let path = data.path("deep.jsonl");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut count = 0;
    for (n, line) in text.lines().enumerate() {
        let json = parse_json(line).unwrap_or_else(|e| panic!("line {}: {e}", n + 1));
        let Some(Json::Array(args)) = json.get("in") else { panic!("line {}: no \"in\"", n + 1) };
        let Some(Json::String(expected)) = json.get("out") else { panic!("line {}: no \"out\"", n + 1) };
        let value = from_json(&args[0]).unwrap_or_else(|e| panic!("line {}: {e}", n + 1));
        assert_eq!(&sha256_hex(canon(&value).as_bytes()), expected, "line {}: canon", n + 1);
        assert!(to_json(&value) == value_text(line), "line {}: typed JSON", n + 1);
        let again = from_json(&parse_json(&to_json(&value)).expect("JSON")).expect("typed JSON");
        assert!(value.same(&again), "line {}: round trip", n + 1);
        count += 1;
    }
    assert_eq!(count, 4);
}
