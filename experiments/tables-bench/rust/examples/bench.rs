//! Native (non-wasm) timing: cargo run --release --example bench
use std::time::Instant;
use tables::{find_tables, read_job};

fn main() {
    for name in ["corpus", "fuzz"] {
        let data = std::fs::read(format!("{}/../out/{name}.bin", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let n = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        let mut jobs = Vec::with_capacity(n);
        let mut off = 8;
        for _ in 0..n {
            let (s, edges, len) = read_job(&data[off..]);
            off += len;
            jobs.push((s, edges));
        }
        let mut times = Vec::new();
        let mut tables = 0;
        for _ in 0..6 {
            let t0 = Instant::now();
            tables = 0;
            for (s, edges) in &jobs {
                tables += std::hint::black_box(find_tables(edges, s)).tables.len();
            }
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.remove(0);
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("native rust [{name}]: {tables} tables, median {:.0} ms, runs {:?}", times[times.len() / 2], times.iter().map(|t| t.round()).collect::<Vec<_>>());
    }
}
