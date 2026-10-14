//! Native (non-wasm) timing of the same parser: cargo run --release --example bench
use psparser::{Interner, Parser};
use std::time::Instant;

fn main() {
    let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../out/corpus.bin")).unwrap();
    let rd = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap()) as usize;
    let mut off = 8;
    let mut jobs: Vec<Vec<&[u8]>> = Vec::new();
    for _ in 0..rd(4) {
        let ns = rd(off);
        off += 4;
        let mut streams = Vec::new();
        for _ in 0..ns {
            let len = rd(off);
            off += 4;
            streams.push(&data[off..off + len]);
            off += len;
        }
        jobs.push(streams);
    }
    let bytes: usize = jobs.iter().flatten().map(|s| s.len()).sum();
    let mut interner = Interner::new();
    let mut times = Vec::new();
    let mut total = 0;
    for _ in 0..(if std::env::var("LONG").is_ok() { 40 } else { 6 }) {
        let t0 = Instant::now();
        total = 0;
        for job in &jobs {
            let Ok(mut p) = Parser::new(&mut interner, job.clone()) else { continue };
            while let Ok(obj) = p.nextobject() {
                std::hint::black_box(&obj);
                total += 1;
            }
        }
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times.remove(0); // warm-up
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = times[times.len() / 2];
    println!("native rust: {total} objects, median {median:.0} ms, {:.1} MB/s, runs {times:?}", bytes as f64 / 1e6 / (median / 1000.0));
}
