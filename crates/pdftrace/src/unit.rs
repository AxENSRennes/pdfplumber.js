//! Access to the unit differential test data that `oracle/unit/<module>.py`
//! writes under `$PDFPLUMBER_ROOT/.cache/unit/<module>/`.
//!
//! Each generator also writes `meta.json` with the SHA-256 of its own
//! source; [`UnitData::open`] refuses data produced by another version of
//! the generator, and panics with the command that regenerates it when the
//! data is missing or stale.

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use pdfminer::pdftypes::{PDFDict, PDFObject, PDFStream};
use pdfminer::psparser::LIT;

use crate::canon::sha256_hex;
use crate::typed_json::{self, Json};
use crate::value::Value;

/// The checkout this crate belongs to (where `oracle/` is).
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("workspace root")
}

/// `$PDFPLUMBER_ROOT`, else the workspace root: where `corpus/`,
/// `upstream/`, `.venv` and `.cache` are. In a Git worktree, set it to the
/// main checkout.
pub fn pdfplumber_root() -> PathBuf {
    match std::env::var_os("PDFPLUMBER_ROOT") {
        Some(root) if !root.is_empty() => PathBuf::from(root),
        _ => workspace_root(),
    }
}

/// The unit test data of one oracle module.
pub struct UnitData {
    pub module: String,
    pub dir: PathBuf,
}

impl UnitData {
    /// Opens `.cache/unit/<module>/`, checking that `oracle/unit/<module>.py`
    /// generated it. Panics with instructions when it did not.
    pub fn open(module: &str) -> UnitData {
        let root = pdfplumber_root();
        let dir = root.join(".cache/unit").join(module);
        let generator = workspace_root().join("oracle/unit").join(format!("{module}.py"));
        let how = format!(
            "generate it with:\n    PDFPLUMBER_ROOT={root} {root}/.venv/bin/python {generator}\n\
             (PDFPLUMBER_ROOT is the checkout holding .venv, corpus/ and .cache/)",
            root = root.display(),
            generator = generator.display(),
        );
        let meta_path = dir.join("meta.json");
        let meta = fs::read_to_string(&meta_path)
            .unwrap_or_else(|_| panic!("missing unit test data {}: {how}", meta_path.display()));
        let meta = typed_json::parse_json(&meta).expect("meta.json is JSON");
        let source = fs::read(&generator).unwrap_or_else(|e| panic!("cannot read {}: {e}", generator.display()));
        let expected = sha256_hex(&source);
        let recorded = match meta.get("generator_sha256") {
            Some(Json::String(s)) => s.clone(),
            _ => String::new(),
        };
        assert_eq!(recorded, expected, "stale unit test data in {}: {how}", dir.display());
        UnitData { module: module.to_owned(), dir }
    }

    /// The path of one data file.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// The lines of `<name>.jsonl`, parsed as plain JSON.
    pub fn json_lines(&self, name: &str) -> Vec<Json> {
        let path = self.path(&format!("{name}.jsonl"));
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        text.lines()
            .enumerate()
            .map(|(i, line)| {
                typed_json::parse_json(line).unwrap_or_else(|e| panic!("{}:{}: {e}", path.display(), i + 1))
            })
            .collect()
    }

    /// The `(in, out)` cases of `<name>.jsonl`, whose lines are
    /// `{"in": [typed args...], "out": typed result}`.
    pub fn cases(&self, name: &str) -> Vec<(Vec<Value>, Value)> {
        self.json_lines(name)
            .iter()
            .map(|line| {
                let args = match typed_json::from_json(line.get("in").expect("\"in\"")) {
                    Ok(Value::List(args)) => args,
                    other => panic!("{name}: \"in\" is not a list: {other:?}"),
                };
                let out = typed_json::from_json(line.get("out").expect("\"out\"")).expect("typed \"out\"");
                (args, out)
            })
            .collect()
    }
}

/// A content-stream job of `oracle/unit/psparser.py`: the streams given to
/// `PDFContentParser`, each a kind (`STREAM_KINDS` of the generator) and
/// bytes.
pub struct ContentJob {
    pub streams: Vec<(u8, Rc<[u8]>)>,
}

impl ContentJob {
    /// Total length of the stream bytes.
    pub fn bytes(&self) -> usize {
        self.streams.iter().map(|(_, data)| data.len()).sum()
    }

    /// The objects the generator builds (`STREAM_KINDS`): 0 a stream of
    /// decoded data, 1 a stream with the unsupported filter `/FooDecode`,
    /// 2 a stream with `/DCTDecode` (data returned as is), 3 the bytes
    /// themselves (not a stream), 4 a stream with `/Crypt`.
    pub fn stream_objects(&self) -> Vec<PDFObject> {
        let filtered = |filter: &str, data: &Rc<[u8]>| {
            let attrs = PDFDict::new();
            attrs.insert("Filter".into(), PDFObject::Literal(LIT(filter)));
            PDFObject::Stream(Rc::new(PDFStream::new(attrs, data.clone(), None)))
        };
        self.streams
            .iter()
            .map(|(kind, data)| match kind {
                0 => PDFObject::Stream(Rc::new(PDFStream::new(PDFDict::new(), data.clone(), None))),
                1 => filtered("FooDecode", data),
                2 => filtered("DCTDecode", data),
                3 => PDFObject::Bytes(data.clone()),
                4 => filtered("Crypt", data),
                other => panic!("unknown stream kind {other}"),
            })
            .collect()
    }
}

/// Reads a content-stream corpus `*.bin` of `oracle/unit/psparser.py`
/// (`b"PSC2" u32 njobs`, then per job `u32 nstreams` and per stream
/// `u8 kind`, `u32 len` + bytes, little-endian).
pub fn read_content_jobs(path: &Path) -> Vec<ContentJob> {
    let data = fs::read(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    assert_eq!(&data[..4], b"PSC2", "{}: not a PSC2 file (regenerate it)", path.display());
    let rd = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().expect("4 bytes")) as usize;
    let mut off = 8;
    let mut jobs = Vec::with_capacity(rd(4));
    for _ in 0..rd(4) {
        let n = rd(off);
        off += 4;
        let mut streams = Vec::with_capacity(n);
        for _ in 0..n {
            let kind = data[off];
            let len = rd(off + 1);
            off += 5;
            streams.push((kind, Rc::from(&data[off..off + len])));
            off += len;
        }
        jobs.push(ContentJob { streams });
    }
    assert_eq!(off, data.len(), "{}: trailing bytes", path.display());
    jobs
}
