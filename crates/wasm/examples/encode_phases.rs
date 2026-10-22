//! Times the result encoding of the JavaScript boundary natively (the
//! PERF measurements, `docs/architecture.md`, "Vague 5"): for each file
//! (`path<TAB>password` lines on stdin), `pdfplumber.open` with its first
//! 20 pages, then for each page `page.objects` (not timed) and the `PJB1`
//! block of what the `page_objects` operation returns (`objects_value`),
//! written `PASSES` times. Writes, per file, `path pages encode bytes`
//! (seconds, bytes of one pass).
//!
//! ```sh
//! cargo run --release -p pdfplumber-wasm --example encode_phases -- CMAP_DIR [PASSES] < list.tsv
//! ```

use std::io::BufRead;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use pdfminer::cmapdb::{CMapDB, DirCMapProvider};
use pdfplumber::container::objects_value;
use pdfplumber::obj::PyValue;
use pdfplumber::pdf::{OpenOptions, Source, PDF};
use pdfplumber_wasm::wire::binary::encode;
use pycompat::PyStr;

fn main() {
    let dir = std::env::args().nth(1).expect("the CMap directory");
    CMapDB::set_default_provider(Some(Arc::new(
        DirCMapProvider::new(std::path::Path::new(&dir)).expect("a CMap directory"),
    )));
    std::thread::Builder::new().stack_size(1 << 30).spawn(run).unwrap().join().unwrap();
}

fn run() {
    let passes: usize = std::env::args().nth(2).map_or(1, |n| n.parse().expect("PASSES"));
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let (path, password) = line.split_once('\t').unwrap_or((&line, ""));
        let data: Rc<[u8]> = Rc::from(std::fs::read(path).unwrap());
        let mut o = OpenOptions::new();
        if !password.is_empty() {
            o.password = Some(PyValue::str(password));
        }
        o.pages = Some(PyValue::list((1..=20).map(PyValue::from)));
        let Ok(pdf) = PDF::open(Source::Path { data, path: PyStr::new("") }, o) else {
            println!("{path}\t0\t0\t0");
            continue;
        };
        let pages = pdf.pages().unwrap_or_default();
        let (mut t, mut bytes) = (0f64, 0usize);
        for p in &pages {
            let Ok(objects) = p.objects() else { continue };
            let value = objects_value(&objects);
            for pass in 0..passes {
                let s = Instant::now();
                let block = encode(&value);
                t += s.elapsed().as_secs_f64();
                if pass == 0 {
                    bytes += block.map_or(0, |b| b.len());
                }
            }
        }
        println!("{path}\t{}\t{t:.4}\t{bytes}", pages.len());
    }
}
