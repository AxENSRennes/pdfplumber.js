//! Writes the corpus of the result-encoding benchmark (`js/bench/`,
//! `docs/js-runtime.md`): the `chars` and `edges` of the real pages of
//! `oracle/unit/text_tables_real.py` (pdfplumber's own objects, from the
//! oracle), as one `PJB1` block without tables (a list of
//! `{"chars": [...], "edges": [...]}`), which the `bench_load` operation
//! reads.
//!
//! ```sh
//! PDFPLUMBER_ROOT=<main checkout> cargo run --release -p pdfplumber-wasm --example bench_corpus -- OUT.pjb [MAX_PAGES]
//! ```

use std::io::{BufRead, BufReader};

use pdfplumber::obj::{Obj, PyValue};
use pdfplumber_wasm::wire::binary::BinWriter;
use pdfplumber_wasm::wire::write_value;
use pdftrace::typed_json;
use pdftrace::unit::UnitData;
use pdftrace::value::Value;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).expect("usage: bench_corpus OUT.pjb [MAX_PAGES]");
    let max: usize = args.get(2).map_or(usize::MAX, |n| n.parse().expect("MAX_PAGES"));
    let data = UnitData::open("text_tables_real");
    let path = data.path("pages_real.jsonl");
    let file = std::fs::File::open(&path).expect("pages_real.jsonl");
    let mut pages = Vec::new();
    let (mut chars, mut edges) = (0, 0);
    for line in BufReader::new(file).lines().take(max) {
        let json = typed_json::parse_json(&line.expect("a line")).expect("JSON");
        let input = typed_json::from_json(json.get("in").expect("in")).expect("typed");
        let Value::List(args) = &input else { panic!("\"in\" is not a list") };
        let record = pdftrace::plumber::from_value(&args[0]).expect("a record");
        let PyValue::Dict(record) = &record else { panic!("a record is a dict") };
        let mut page = Obj::new();
        for key in ["chars", "edges"] {
            let v = record.get(key).expect("chars and edges").clone();
            let n = v.as_seq().map_or(0, <[PyValue]>::len);
            if key == "chars" {
                chars += n;
            } else {
                edges += n;
            }
            page.set(key, v);
        }
        pages.push(PyValue::Dict(page));
    }
    let mut w = BinWriter::without_tables();
    write_value(&mut w, &PyValue::list(pages.iter().cloned())).expect("encodable");
    let block = w.finish();
    std::fs::write(out, &block).expect("write");
    eprintln!("{out}: {} pages, {chars} chars, {edges} edges, {} bytes", pages.len(), block.len());
    // how many lists are tables (uniform dicts) in the result format
    let mut shapes = std::collections::BTreeMap::<String, usize>::new();
    for p in &pages {
        let PyValue::Dict(d) = p else { continue };
        for (k, v) in d.items() {
            let items = v.as_seq().unwrap_or(&[]);
            let what = if items.len() < 2 {
                "short"
            } else if let Some(s) = pdfplumber_wasm::wire::shapes(items) {
                if s.first.len() == 1 {
                    "table"
                } else {
                    "tables"
                }
            } else {
                "mixed"
            };
            *shapes.entry(format!("{k} {what}")).or_default() += 1;
        }
    }
    eprintln!("{shapes:?}");
}
