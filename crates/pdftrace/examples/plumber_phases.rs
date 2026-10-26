//! Times the phases of pdfplumber's page layer, without any trace: for each
//! file (`path<TAB>password` lines on stdin), `pdfplumber.open`, then for
//! each of its first 20 pages `page.layout` (the interpretation and the
//! device), `page.objects` (`process_object` of each layout object),
//! `extract_text()`, `extract_words()`, `find_tables()`, `extract_tables()`
//! (which finds the tables again, as pdfplumber does). Writes, per file,
//! `path pages open layout objects text words tables extract_tables`
//! (seconds). The Python
//! twin is `oracle`'s way of measuring the same calls (the join's report,
//! `docs/architecture.md`).
//!
//! ```sh
//! cargo run --release -p pdftrace --example plumber_phases -- CMAP_DIR < list.tsv
//! ```
//!
//! `PLUMBER_PHASES=1,2` runs only the phases listed (1 `layout`, 2
//! `objects`, 3 text, 4 words, 5 tables, 6 `extract_tables`; a later phase still computes
//! what it needs), to count what a phase allocates.

use std::io::BufRead;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use pdfminer::cmapdb::{CMapDB, DirCMapProvider};
use pdfplumber::obj::Obj;
use pdfplumber::pdf::{OpenOptions, Source, PDF};
use pycompat::PyStr;

fn main() {
    let dir = std::env::args().nth(1).expect("the CMap directory");
    CMapDB::set_default_provider(Some(Arc::new(
        DirCMapProvider::new(std::path::Path::new(&dir)).expect("a CMap directory"),
    )));
    std::thread::Builder::new().stack_size(1 << 30).spawn(run).unwrap().join().unwrap();
}

fn run() {
    // PLUMBER_PHASES=1,2 (say) runs only those phases (1 layout, 2 objects,
    // 3 text, 4 words, 5 tables, 6 extract_tables): the allocation counts
    // of a phase
    let phases: Vec<bool> = match std::env::var("PLUMBER_PHASES") {
        Ok(v) => (0..7).map(|i| i == 0 || v.split(',').any(|p| p == i.to_string())).collect(),
        Err(_) => vec![true; 7],
    };
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let (path, password) = line.split_once('\t').unwrap_or((&line, ""));
        let data: Rc<[u8]> = Rc::from(std::fs::read(path).unwrap());
        let mut t = [0f64; 7];
        let started = Instant::now();
        let mut o = OpenOptions::new();
        if !password.is_empty() {
            o.password = Some(pdfplumber::obj::PyValue::str(password));
        }
        o.pages = Some(pdfplumber::obj::PyValue::list((1..=20).map(pdfplumber::obj::PyValue::from)));
        let pdf = match PDF::open(Source::Path { data, path: PyStr::new("") }, o) {
            Ok(pdf) => pdf,
            Err(_) => {
                println!("{path}\t0\t{:.4}\t0\t0\t0\t0\t0\t0", started.elapsed().as_secs_f64());
                continue;
            }
        };
        let pages = pdf.pages().unwrap_or_default();
        t[0] = started.elapsed().as_secs_f64();
        let kw = Obj::new();
        for p in &pages {
            let mut lap = |i: usize, f: &mut dyn FnMut()| {
                if !phases.get(i).copied().unwrap_or(true) {
                    return;
                }
                let s = Instant::now();
                f();
                t[i] += s.elapsed().as_secs_f64();
            };
            lap(1, &mut || drop(p.layout()));
            lap(2, &mut || drop(p.objects()));
            lap(3, &mut || drop(p.extract_text(&kw)));
            lap(4, &mut || drop(p.extract_words(&kw)));
            lap(5, &mut || drop(p.find_tables(&pdfplumber::obj::PyValue::None)));
            lap(6, &mut || drop(p.extract_tables(&pdfplumber::obj::PyValue::None)));
        }
        let t: Vec<String> = t.iter().map(|x| format!("{x:.4}")).collect();
        println!("{path}\t{}\t{}", pages.len(), t.join("\t"));
    }
}
