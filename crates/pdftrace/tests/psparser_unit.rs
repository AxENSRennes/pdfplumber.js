//! Replays the content-stream corpora of `oracle/unit/psparser.py` through
//! `pdfminer::pdfinterp::PDFContentParser` and compares, job by job, the
//! SHA-256 of the canonical serialization with pdfminer's.
//!
//! A job serializes to one line `<pos> <canon(obj)>` per object returned by
//! `nextobject()`, then the line of the exception that ended it.
//!
//! A job stops as soon as it yields more objects than pdfminer did (the
//! serialization then differs anyway): a broken parser that keeps returning
//! objects without consuming its input fails at once instead of filling
//! the memory. Loops inside the parser are caught by
//! `psparser::ProgressGuard` in builds with debug assertions.
//!
//! To investigate a mismatch, set `PSPARSER_DUMP_DIR`: the Rust
//! serialization of each differing job is written there as
//! `<corpus>-<index>.txt`, to compare with
//! `oracle/unit/psparser.py --dump <index> --corpus <corpus>`.

use std::fs;

use pdfminer::pdfinterp::PDFContentParser;
use pdftrace::canon::{canon_exc, sha256_hex, write_canon_obj};
use pdftrace::typed_json::Json;
use pdftrace::unit::{read_content_jobs, ContentJob, UnitData};

/// The canonical serialization of a job, as the oracle writes it, and the
/// number of objects; stops after `max_objects + 1` objects.
fn serialize(job: &ContentJob, max_objects: usize) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;
    let exc = match PDFContentParser::new(job.stream_objects()) {
        Err(e) => e,
        Ok(mut parser) => loop {
            if count > max_objects {
                out.push_str("!too many objects\n");
                return (out, count);
            }
            match parser.nextobject() {
                Ok((pos, obj)) => {
                    out.push_str(&pos.to_string());
                    out.push(' ');
                    write_canon_obj(&mut out, &obj);
                    out.push('\n');
                    count += 1;
                }
                Err(e) => break e,
            }
        },
    };
    out.push_str(&canon_exc(&exc));
    out.push('\n');
    (out, count)
}

fn replay(corpus: &str, min_jobs: usize) {
    let data = UnitData::open("psparser");
    let oracle_path = data.path(&format!("{corpus}.json"));
    let oracle = fs::read_to_string(&oracle_path).unwrap_or_else(|_| {
        panic!(
            "missing {}: generate it with oracle/unit/psparser.py{}",
            oracle_path.display(),
            if corpus == "fuzz" { " --fuzz 4000" } else { "" }
        )
    });
    let oracle = pdftrace::typed_json::parse_json(&oracle).expect("oracle JSON");
    let recorded = oracle.get("generator_sha256");
    let meta = fs::read_to_string(data.path("meta.json")).unwrap();
    let meta = pdftrace::typed_json::parse_json(&meta).unwrap();
    assert_eq!(recorded, meta.get("generator_sha256"), "{corpus}.json comes from another version of the generator");
    let Some(Json::Array(records)) = oracle.get("records") else { panic!("no records") };
    let jobs = read_content_jobs(&data.path(&format!("{corpus}.bin")));
    assert_eq!(jobs.len(), records.len());
    assert!(jobs.len() >= min_jobs, "{corpus}: only {} jobs", jobs.len());

    let dump_dir = std::env::var_os("PSPARSER_DUMP_DIR");
    let mut differ = Vec::new();
    let mut objects = 0;
    for (k, (job, rec)) in jobs.iter().zip(records).enumerate() {
        let expected_objects = match rec.get("objects") {
            Some(Json::Number(n)) => n.parse::<usize>().expect("object count"),
            other => panic!("record without object count: {other:?}"),
        };
        let (text, count) = serialize(job, expected_objects);
        objects += count;
        let Some(Json::String(expected)) = rec.get("sha256") else { panic!("record without sha256") };
        if &sha256_hex(text.as_bytes()) != expected {
            let Some(Json::String(id)) = rec.get("id") else { panic!() };
            differ.push(format!("#{k} {id} (python: {expected_objects} objects, {:?})", rec.get("outcome")));
            if let Some(dir) = &dump_dir {
                fs::create_dir_all(dir).unwrap();
                fs::write(std::path::Path::new(dir).join(format!("{corpus}-{k}.txt")), &text).unwrap();
            }
        }
    }
    println!("{corpus}: {}/{} jobs identical, {objects} objects", jobs.len() - differ.len(), jobs.len());
    assert!(
        differ.is_empty(),
        "{corpus}: {}/{} jobs differ:\n{}",
        differ.len(),
        jobs.len(),
        differ.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn adversarial_corpus() {
    replay("fuzz", 4000);
}

#[test]
fn real_corpus() {
    replay("corpus", 3000);
}
