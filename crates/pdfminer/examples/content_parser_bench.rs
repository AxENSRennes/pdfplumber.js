//! Native timing of `PDFContentParser` over the real content-stream corpus
//! written by `oracle/unit/psparser.py`:
//!
//! ```sh
//! PDFPLUMBER_ROOT=<main checkout> cargo run --release -p pdfminer --example content_parser_bench
//! ```
//!
//! Every job is parsed to its end, each object being dropped as the
//! interpreter's operand stack would after use. Prints the median of the
//! runs after a warm-up.

use std::hint::black_box;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use pdfminer::pdfinterp::PDFContentParser;
use pdfminer::pdftypes::{PDFDict, PDFObject, PDFStream};

fn main() {
    let root = std::env::var_os("PDFPLUMBER_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    let path = root.join(".cache/unit/psparser/corpus.bin");
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e} (run oracle/unit/psparser.py)", path.display()));
    let rd = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap()) as usize;
    let mut off = 8;
    let mut jobs: Vec<Vec<Rc<[u8]>>> = Vec::new();
    for _ in 0..rd(4) {
        let n = rd(off);
        off += 4;
        let mut streams = Vec::new();
        for _ in 0..n {
            let len = rd(off);
            off += 4;
            streams.push(Rc::from(&data[off..off + len]));
            off += len;
        }
        jobs.push(streams);
    }
    let bytes: usize = jobs.iter().flatten().map(|s| s.len()).sum();
    let runs: usize = std::env::var("RUNS").ok().and_then(|r| r.parse().ok()).unwrap_or(5);
    let mut times = Vec::new();
    let mut total = 0;
    for _ in 0..=runs {
        let t0 = Instant::now();
        total = 0;
        for job in &jobs {
            let streams = job
                .iter()
                .map(|s| PDFObject::Stream(Rc::new(PDFStream::new(PDFDict::new(), s.clone(), None))))
                .collect();
            let Ok(mut parser) = PDFContentParser::new(streams) else { continue };
            if std::env::var_os("TOKENS").is_some() {
                while let Ok(tok) = parser.nexttoken() {
                    black_box(&tok);
                    total += 1;
                }
            } else {
                while let Ok(obj) = parser.nextobject() {
                    black_box(&obj);
                    total += 1;
                }
            }
        }
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times.remove(0); // warm-up
    times.sort_by(f64::total_cmp);
    let median = times[times.len() / 2];
    println!(
        "{} jobs, {:.1} MB, {total} objects: median {median:.0} ms ({:.1} MB/s), runs {times:.0?}",
        jobs.len(),
        bytes as f64 / 1e6,
        bytes as f64 / 1e6 / (median / 1000.0)
    );
}
