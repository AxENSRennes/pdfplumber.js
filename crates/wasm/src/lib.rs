//! The WebAssembly boundary of the port: the coarse-grained API the
//! JavaScript package (`js/`) calls (`docs/architecture.md`, "JavaScript
//! Boundary"; `docs/js-runtime.md` for the ABI, the result format and
//! its measurements, the CMap loading and the error and warning channels).
//!
//! - `abi`: the exported functions (`pp_alloc`, `pp_call`...), no
//!   `wasm-bindgen`.
//! - `ops`: requests, responses and operations; the handle table
//!   (`handles`) of the objects JavaScript holds; `pages`: the operations
//!   of documents and pages (pdfplumber's `PDF` and `Page`); `structs`:
//!   structure trees (`PDFStructTree`); `streams`: the streams results
//!   hold by reference.
//! - `wire`: the result encodings (binary `PJB1`, JSON) and the argument
//!   decoding; `strings`: Python `str` (lone surrogates included) to UTF-16.
//! - `errors`: a Python exception as JavaScript receives it.
//! - `cmaps`: predefined CMaps handed over by JavaScript, loaded lazily;
//!   `cmap_scan`: the ones a document will probably ask for (preloading).
//!
//! The crate is also an `rlib`, so its logic is tested natively
//! (`cargo test -p pdfplumber-wasm`), and in every browser by `js/test`.

pub mod abi;
pub mod cmap_scan;
pub mod cmaps;
pub mod errors;
pub mod handles;
pub mod ops;
#[cfg(feature = "oracle-hooks")]
pub mod oracle;
pub mod pages;
pub mod streams;
pub mod strings;
pub mod structs;
pub mod wire;
