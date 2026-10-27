//! The oracle hooks of the harness that writes stage traces through the
//! public JavaScript API (`js/trace/`, `docs/verification.md`, "Parity
//! Through The JavaScript API"): what a Python stage reads that the public
//! semantics intentionally do not give, and nothing else. Compiled only
//! with the `oracle-hooks` feature (`node scripts/build-wasm.mjs --oracle`:
//! `dist/pdfplumber-oracle.wasm`, never shipped); every operation is
//! prefixed `oracle_`.
//!
//! - `oracle_doc_open` `[options, ids, path]`: `doc_open` (`path` `None`)
//!   or `doc_open_file`, whose layout analyses rank their boxes and groups
//!   by Python's `id()` order, the `ids` lines of the oracle's
//!   `plumber_objects` trace (`ids`: page number -> `[[key, rank], ...]`,
//!   `pdftrace::stages::plumber_objects::ids_factory`); the library and the
//!   public API keep `CreationOrder`.
//! - `oracle_page_ids` (a page): `content_ids(page.layout)` of
//!   `oracle/stages/plumber_objects.py`, the `ids` line of the page: the
//!   ranking its analysis used, keyed by content.
//! - `oracle_object_indices` `[parent, kind]` (a derived page): for each
//!   object of `derived.objects[kind]`, its index in `parent.objects[kind]`
//!   by identity (`plumber_text.py` `run_op` of `dedupe_chars`): JavaScript
//!   results are copies, Python's derived pages hold the parent's dicts.
//!
//! Python's `re` spans (`plumber_text`'s `spans` lines) need no hook here:
//! the harness gives them to `page_search` as JavaScript's spans
//! (`js/src/search.ts`).

use std::collections::HashMap;
use std::rc::Rc;

use pdfminer::pdfparser::FileKind;
use pdfplumber::obj::PyValue;
use pdfplumber::pdf::Source;
use pdftrace::stages::plumber_objects::{content_ids, ids_factory, IdsRegistry};
use pdftrace::value::Value;
use pycompat::recursion::leaf;
use pycompat::specialize::AttrSite;
use pycompat::{exc, PyInt, PyResult};

use crate::ops::{arg, handle, seq, str_arg, Core, DocState, Document, Object};
use crate::pages::PageObj;

static PAGE_IDS_LAYOUT: AttrSite = AttrSite::new("js oracle_page_ids page.layout");

/// The `ids` argument: page number -> `[(key, rank), ...]`.
fn oracle_ids(v: &PyValue) -> PyResult<HashMap<i64, Vec<(String, u64)>>> {
    let mut out = HashMap::new();
    let PyValue::Dict(d) = v else { return Err(exc::TypeError("ids: expected a dict")) };
    for (k, ranks) in d.items() {
        let n: i64 = k.as_str().and_then(|s| s.parse().ok()).ok_or_else(|| exc::ValueError("ids: a page number"))?;
        let mut items = Vec::new();
        for pair in seq(ranks)? {
            let pair = seq(pair)?;
            let key = str_arg(arg(pair, 0)?)?.to_owned();
            let PyValue::Int(rank) = arg(pair, 1)? else { return Err(exc::TypeError("ids: a rank")) };
            let rank =
                rank.as_i64().and_then(|r| u64::try_from(r).ok()).ok_or_else(|| exc::ValueError("ids: a rank"))?;
            items.push((key, rank));
        }
        out.insert(n, items);
    }
    Ok(out)
}

/// A value of the trace model as a pdfplumber value (`content_ids` writes
/// lists of `[str, int]`).
fn py_value(v: &Value) -> PyResult<PyValue> {
    pdftrace::plumber::from_value(v).map_err(exc::ValueError)
}

impl Core {
    /// The oracle operations; `None` for any other name.
    pub(crate) fn oracle_op(
        &mut self,
        name: &str,
        h: u32,
        args: &PyValue,
        data: Option<Rc<[u8]>>,
    ) -> PyResult<Option<PyValue>> {
        let a = || seq(args);
        Ok(Some(match name {
            "oracle_doc_open" => {
                let data = data.ok_or_else(|| exc::TypeError("oracle_doc_open needs the PDF bytes"))?;
                let a = a()?;
                let options = arg(a, 0)?.clone();
                let (factory, registry) = ids_factory(&oracle_ids(arg(a, 1)?)?);
                let source = match arg(a, 2)? {
                    PyValue::None => Source::Stream { data: data.clone(), kind: FileKind::BytesIO },
                    PyValue::Str(path) => Source::Path { data: data.clone(), path: path.clone() },
                    other => return Err(exc::TypeError(format!("expected a str path, got {}", other.type_name()))),
                };
                let pdf = {
                    let _call = pycompat::recursion::eval_entry()?;
                    crate::pages::open_source(source, &options, Some(factory))?
                };
                let state = DocState { pdf: Some(Rc::new(pdf)), hooks: Some(Rc::new(registry)), ..DocState::default() };
                let h = self.insert(Object::Document(Document {
                    data,
                    options,
                    state,
                    unrecorded: false,
                    scanned: false,
                }))?;
                self.reach(handle(&h)?)?;
                h
            }
            "oracle_page_ids" => {
                let (doc, page) = match self.handles.get(h) {
                    Some(Object::Page(PageObj { doc, page })) => (*doc, page.clone()),
                    _ => return Err(exc::ValueError("invalid handle")),
                };
                self.reach(doc)?;
                let page = page.ok_or_else(|| exc::ValueError("the page was not rebuilt"))?;
                let registry = match self.handles.get(doc) {
                    Some(Object::Document(d)) => d.state.hooks.clone(),
                    _ => None,
                };
                let registry = registry.and_then(|r| r.downcast::<IdsRegistry>().ok()).ok_or_else(|| {
                    exc::ValueError("oracle_page_ids: the document was not opened by oracle_doc_open")
                })?;
                // content_ids(page.layout): the property, then the harness's
                // function (no recursion in it)
                let layout = PAGE_IDS_LAYOUT.get(page.class_name(), || page.layout())?;
                leaf(1)?;
                let n = page.page_number.as_i64().ok_or_else(|| exc::ValueError("a page number"))?;
                let ids = registry.borrow().get(&n).cloned().unwrap_or_default();
                let value = content_ids(&layout, &mut ids.borrow_mut());
                py_value(&value)?
            }
            "oracle_object_indices" => {
                let a = a()?;
                let page_of = |core: &Core, h: u32| match core.handles.get(h) {
                    Some(Object::Page(PageObj { doc, page })) => Ok((*doc, page.clone())),
                    _ => Err(exc::ValueError("invalid handle")),
                };
                let (doc, derived) = page_of(self, h)?;
                let (_, parent) = page_of(self, handle(arg(a, 0)?)?)?;
                self.reach(doc)?;
                let kind = str_arg(arg(a, 1)?)?.to_owned();
                let (derived, parent) = match (derived, parent) {
                    (Some(d), Some(p)) => (d, p),
                    _ => return Err(exc::ValueError("the pages were not rebuilt")),
                };
                let parent_objects = parent.objects()?;
                let index: HashMap<usize, usize> = parent_objects
                    .get(&kind)
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                    .map(|(i, o)| (o.id(), i))
                    .collect();
                let derived_objects = derived.objects()?;
                let mut out = Vec::new();
                for o in derived_objects.get(&kind).unwrap_or_default() {
                    let i = index.get(&o.id()).ok_or_else(|| exc::KeyError("an object not in the parent"))?;
                    out.push(PyValue::Int(PyInt::from(*i as i64)));
                }
                PyValue::list(out)
            }
            _ => return Ok(None),
        }))
    }
}
