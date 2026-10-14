//! Native timing of `PDFContentParser` over the real content-stream corpus
//! written by `oracle/unit/psparser.py`:
//!
//! ```sh
//! PDFPLUMBER_ROOT=<main checkout> cargo run --profile dist -p pdftrace --example content_parser_bench
//! ```
//!
//! Every job is parsed to its end, each object being dropped as the
//! interpreter's operand stack would after use (`TOKENS=1` times
//! `nexttoken` alone). Prints the median of `RUNS` runs (default 5) after
//! a warm-up.

use std::hint::black_box;
use std::time::Instant;

use pdfminer::pdfinterp::PDFContentParser;
use pdftrace::unit::{pdfplumber_root, read_content_jobs};

fn main() {
    let path = pdfplumber_root().join(".cache/unit/psparser/corpus.bin");
    let jobs = read_content_jobs(&path);
    let bytes: usize = jobs.iter().map(|job| job.bytes()).sum();
    let runs: usize = std::env::var("RUNS").ok().and_then(|r| r.parse().ok()).unwrap_or(5);
    let tokens_only = std::env::var_os("TOKENS").is_some();
    let mut times = Vec::new();
    let mut total = 0;
    for _ in 0..=runs {
        let t0 = Instant::now();
        total = 0;
        for job in &jobs {
            let Ok(mut parser) = PDFContentParser::new(job.stream_objects()) else { continue };
            if tokens_only {
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
        "{} jobs, {:.1} MB, {total} {}: median {median:.0} ms ({:.1} MB/s), runs {times:.0?}",
        jobs.len(),
        bytes as f64 / 1e6,
        if tokens_only { "tokens" } else { "objects" },
        bytes as f64 / 1e6 / (median / 1000.0)
    );
}
