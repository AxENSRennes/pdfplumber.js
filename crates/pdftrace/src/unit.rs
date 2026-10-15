//! Access to the unit differential test data that `oracle/unit/<module>.py`
//! writes under `<cache>/unit/<module>/<fingerprint>/` (`<cache>` is
//! `$PDFPLUMBER_CACHE`, else `$PDFPLUMBER_ROOT/.cache`).
//!
//! Each generator also writes `meta.json`, whose `sources` gives the SHA-256
//! of every `oracle/` module it loaded (`oracle/unit/_common.py`). Several
//! checkouts share the cache, each generator version in its own fingerprint
//! directory; [`UnitData::open`] takes the most recent directory whose
//! sources are those of this checkout (`python -m oracle.unit` marks the one
//! of the current oracle environment as the most recent), and panics with
//! the command that regenerates the data when there is none.

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use pdfminer::pdftypes::{PDFDict, PDFObject, PDFStream};
use pdfminer::psparser::LIT;

use crate::canon::sha256_hex;
use crate::typed_json::{self, Json};
use crate::value::Value;

/// The checkout this crate belongs to (where `oracle/` is): two levels up
/// from this crate (not canonicalized: WASI has no `realpath`).
pub fn workspace_root() -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().and_then(Path::parent).expect("crates/pdftrace is two levels below the root").to_path_buf()
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

/// `$PDFPLUMBER_CACHE`, else `<pdfplumber_root>/.cache` (as
/// `oracle/corpus.py` `cache_dir`).
pub fn cache_dir() -> PathBuf {
    match std::env::var_os("PDFPLUMBER_CACHE") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => pdfplumber_root().join(".cache"),
    }
}

/// The unit test data of one oracle module.
pub struct UnitData {
    pub module: String,
    pub dir: PathBuf,
}

impl UnitData {
    /// Opens the data of `module`: the most recent
    /// `<cache>/unit/<module>/<fingerprint>/` whose `meta.json` was written
    /// by the current `oracle/unit/<module>.py` (and the oracle modules it
    /// uses). Panics with instructions when there is none.
    pub fn open(module: &str) -> UnitData {
        UnitData::open_in(&cache_dir().join("unit").join(module), module)
    }

    /// [`UnitData::open`] on the fingerprint directories under `base`.
    pub fn open_in(base: &Path, module: &str) -> UnitData {
        let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(base)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter_map(|dir| Some((fs::metadata(dir.join("meta.json")).ok()?.modified().ok()?, dir)))
                    .collect()
            })
            .unwrap_or_default();
        candidates.sort_by_key(|c| std::cmp::Reverse(c.0));
        let mut reasons = Vec::new();
        for (_, dir) in candidates {
            let data = UnitData { module: module.to_owned(), dir };
            let meta_path = data.path("meta.json");
            let reason = match fs::read_to_string(&meta_path).map_err(|e| e.to_string()).and_then(|meta| {
                let meta = typed_json::parse_json(&meta)?;
                data.sources_reason(&meta_path, meta.get("sources")).map_or(Ok(()), Err)
            }) {
                Ok(()) => return data,
                Err(reason) => reason,
            };
            reasons.push(reason);
        }
        let data = UnitData { module: module.to_owned(), dir: base.to_path_buf() };
        match reasons.first() {
            None => panic!("missing unit test data in {}: {}", base.display(), data.how()),
            Some(reason) => panic!("no current unit test data in {} ({reason}): {}", base.display(), data.how()),
        }
    }

    /// How to regenerate the data.
    fn how(&self) -> String {
        format!(
            "generate it with:\n    cd {workspace} && PDFPLUMBER_ROOT={root} {root}/.venv/bin/python -m oracle.unit.{module}\n\
             (PDFPLUMBER_ROOT is the checkout holding .venv, corpus/ and .cache/; `python -m oracle.unit` \
             regenerates every missing or stale module)",
            workspace = workspace_root().display(),
            root = pdfplumber_root().display(),
            module = self.module,
        )
    }

    /// Why the `sources` recorded in `file` (`{repo path: sha256}` of the
    /// oracle modules that wrote it) do not match this checkout: the
    /// generator of this module must be listed, and every file must be
    /// unchanged. `None` when they match.
    fn sources_reason(&self, file: &Path, sources: Option<&Json>) -> Option<String> {
        let Some(Json::Object(sources)) = sources else {
            return Some(format!("{} has no \"sources\" (an older generator wrote it)", file.display()));
        };
        let generator = format!("oracle/unit/{}.py", self.module);
        if !sources.iter().any(|(path, _)| *path == generator) {
            return Some(format!("{} does not list {generator}", file.display()));
        }
        let workspace = workspace_root();
        for (path, recorded) in sources {
            let Ok(source) = fs::read(workspace.join(path)) else {
                return Some(format!("{path} no longer exists"));
            };
            if *recorded != Json::String(sha256_hex(&source)) {
                return Some(format!("{path} changed since {} was written", file.display()));
            }
        }
        None
    }

    /// Checks the `sources` recorded in another file of the data (a corpus
    /// written with `meta.json`); panics with instructions if they do not
    /// match this checkout.
    pub fn check_sources(&self, file: &Path, sources: Option<&Json>) {
        if let Some(reason) = self.sources_reason(file, sources) {
            panic!("stale unit test data ({reason}): {}", self.how());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a fingerprint directory whose `meta.json` records `sources`.
    fn data_dir(base: &Path, name: &str, sources: &str) -> PathBuf {
        let dir = base.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("meta.json"), format!("{{\"sources\": {sources}}}")).unwrap();
        dir
    }

    #[test]
    #[cfg_attr(target_family = "wasm", ignore = "panics abort on WebAssembly")]
    fn open_picks_the_directory_of_this_checkout() {
        let base = std::env::temp_dir().join(format!("pdftrace-unitdata-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let generator = workspace_root().join("oracle/unit/canon.py");
        let sha = sha256_hex(&fs::read(&generator).unwrap());
        // another checkout's generator, written last: never picked
        data_dir(&base, "aaaa", &format!("{{\"oracle/unit/canon.py\": \"{sha}\"}}"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        data_dir(&base, "bbbb", "{\"oracle/unit/canon.py\": \"0000\"}");
        assert_eq!(UnitData::open_in(&base, "canon").dir, base.join("aaaa"));
        let missing = std::panic::catch_unwind(|| UnitData::open_in(&base.join("none"), "canon"));
        assert!(missing.is_err());
        fs::remove_dir_all(&base).unwrap();
    }
}
