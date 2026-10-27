//! Streams by reference (`docs/public-api.md`, "PDFStream";
//! `docs/js-runtime.md`, "Documents And Pages"): with `set_stream_refs`
//! (what the JavaScript runtime asks for), the `PDFStream`s of a result
//! cross as a handle, `objid`, `genno` and `attrs`, and JavaScript reads
//! their data on demand, instead of the core copying every image's bytes
//! into every result that holds it (`objects`, `images`, `to_dict`...).
//!
//! A result names a stream by the first live handle of that stream (the
//! same `Rc`), else a new one, which the operation makes (so that a replay
//! makes it again, `crate::ops`, "Void calls"), in the order the value is
//! written, like the pages it holds ([`Core::label_values`]). A stream
//! handle belongs to the stream's document and lives as long as it: freed
//! with it (JavaScript keeps one `PDFStream` per handle while it is
//! alive). The typed format of the harness writes streams by value.
//!
//! The operations on a stream handle are the statements of
//! `oracle/unit/js_pages_run.py`:
//!
//! | operation | statement | result |
//! |---|---|---|
//! | `stream_rawdata` | `obj.rawdata` | `bytes` or `None` (once decoded) |
//! | `stream_data` | `obj.data` | `bytes` or `None` (before decoding) |
//! | `stream_get_data` | `obj.get_data()` | `bytes` (decodes the stream) |
//! | `stream_get_rawdata` | `obj.get_rawdata()` | `bytes` or `None` |
//!
//! Each reaches the stream's document (`get_data()` changes it: a replay
//! must decode the stream again). An attribute read takes no frame; a
//! method called without arguments is a Python-to-Python call (its frame
//! is the method's own, counted by `pdfminer::pdftypes`). After
//! `PDF.close()`, a stream keeps its bytes, as Python's does (they were
//! read when it was parsed).

use std::rc::Rc;

use pdfminer::pdftypes::{PDFObject, PDFStream};
use pdfplumber::obj::PyValue;
use pycompat::{exc, ExcKind, PyErr, PyResult};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::ops::{seq, Core, Object, Out};
use crate::pages::PageObj;

/// A stream handle.
pub struct StreamObj {
    pub doc: u32,
    /// `None` between a reset and the replay that refills it.
    pub stream: Option<Rc<PDFStream>>,
}

/// `PyValue::Bytes`, or `None`.
fn opt_bytes(b: Option<Rc<[u8]>>) -> PyValue {
    b.map_or(PyValue::None, PyValue::Bytes)
}

impl Core {
    /// The stream of the handle `h` (the operation reaches its document).
    fn stream(&mut self, h: u32) -> PyResult<Rc<PDFStream>> {
        let (doc, stream) = match self.handles.get(h) {
            Some(Object::Stream(s)) => (s.doc, s.stream.clone()),
            _ => return Err(exc::ValueError("invalid handle")),
        };
        self.reach(doc)?;
        stream.ok_or_else(|| PyErr::new(ExcKind::RuntimeError, "the stream was not rebuilt"))
    }

    /// The operations of streams (see the module documentation); `None`
    /// for another name.
    pub(crate) fn stream_op(&mut self, name: &str, h: u32, args: &PyValue) -> PyResult<Option<Out>> {
        let value = match name {
            "stream_rawdata" => opt_bytes(self.stream(h)?.rawdata()),
            "stream_data" => opt_bytes(self.stream(h)?.data()),
            "stream_get_data" => {
                let s = self.stream(h)?;
                no_arguments(args, "PDFStream.get_data")?;
                PyValue::Bytes(s.get_data()?)
            }
            "stream_get_rawdata" => {
                let s = self.stream(h)?;
                no_arguments(args, "PDFStream.get_rawdata")?;
                // a method of one statement: its frame
                pycompat::recursion::leaf(1)?;
                opt_bytes(s.get_rawdata())
            }
            _ => return Ok(None),
        };
        Ok(Some(Out::Value(value)))
    }

    /// Gives a handle to each page and, with `set_stream_refs` (and not in
    /// the typed format), each stream a result holds, in the order the
    /// value is written (see the module documentation and
    /// `crate::pages`): the handles the writer names them by
    /// ([`crate::wire::with_value_handles`]).
    pub(crate) fn label_values(&mut self, value: &PyValue) -> PyResult<()> {
        enum Item<'a> {
            Py(&'a PyValue),
            Pdf(PDFObject),
        }
        let streams = self.stream_refs && self.format != crate::ops::Format::Typed;
        let mut stack = vec![Item::Py(value)];
        // the PDF containers walked (a PDF value may hold itself)
        let mut seen: FxHashSet<usize> = FxHashSet::default();
        // the live stream handles of the document, by stream (built once)
        let mut live: Option<FxHashMap<usize, u32>> = None;
        while let Some(item) = stack.pop() {
            match item {
                Item::Py(v) => match v {
                    PyValue::List(items) | PyValue::Tuple(items) => stack.extend(items.iter().rev().map(Item::Py)),
                    PyValue::Dict(d) => stack.extend(d.items().iter().rev().map(|(_, v)| Item::Py(v))),
                    PyValue::Page(r) => self.label_page(r)?,
                    PyValue::Pdf(o) if streams => stack.push(Item::Pdf(o.clone())),
                    _ => {}
                },
                Item::Pdf(o) => match o {
                    PDFObject::List(l) => {
                        if seen.insert(std::ptr::from_ref(&*l.borrow()) as usize) {
                            let items = l.borrow().clone();
                            stack.extend(items.into_iter().rev().map(Item::Pdf));
                        }
                    }
                    PDFObject::Dict(d) => {
                        if seen.insert(std::ptr::from_ref(&*d.borrow()) as usize) {
                            let items: Vec<PDFObject> = d.borrow().iter().map(|(_, v)| v.clone()).collect();
                            stack.extend(items.into_iter().rev().map(Item::Pdf));
                        }
                    }
                    PDFObject::Stream(s) => {
                        let addr = Rc::as_ptr(&s) as usize;
                        if seen.insert(addr) {
                            let live = live.get_or_insert_with(|| self.live_streams());
                            if !self.stream_values.contains_key(&addr) {
                                let h = match live.get(&addr) {
                                    Some(&h) => h,
                                    None => {
                                        let doc = self.op_doc.ok_or_else(|| {
                                            PyErr::new(ExcKind::RuntimeError, "a stream without a document")
                                        })?;
                                        let h =
                                            self.insert(Object::Stream(StreamObj { doc, stream: Some(s.clone()) }))?;
                                        let h = crate::pages::int_handle(&h)?;
                                        let h = u32::try_from(h).map_err(|_| exc::ValueError("invalid handle"))?;
                                        live.insert(addr, h);
                                        h
                                    }
                                };
                                self.stream_values.insert(addr, h);
                            }
                            // its attrs are written with it
                            stack.push(Item::Pdf(PDFObject::Dict(s.attrs.clone())));
                        }
                    }
                    _ => {}
                },
            }
        }
        Ok(())
    }

    /// The live handle of each stream of the running operation's document,
    /// by stream (the first one: the handles in order).
    fn live_streams(&self) -> FxHashMap<usize, u32> {
        let mut out = FxHashMap::default();
        // (an operation that reached no document: a stream it was given)
        let doc = self.op_doc;
        let mut handles: Vec<(u64, u32, usize)> = self
            .handles
            .iter()
            .filter_map(|(h, o)| match o {
                Object::Stream(StreamObj { doc: d, stream: Some(s) }) if doc.is_none_or(|doc| *d == doc) => {
                    Some((self.oids.get(&h).copied().unwrap_or(u64::MAX), h, Rc::as_ptr(s) as usize))
                }
                _ => None,
            })
            .collect();
        handles.sort_unstable();
        for (_, h, addr) in handles {
            out.entry(addr).or_insert(h);
        }
        out
    }

    /// The handle of a page a result holds (`annot["data"]["P"]`,
    /// `getattr(pdf, "pages")`): the first live handle of that page, else a
    /// new one (made by the operation, so that a replay makes it again).
    fn label_page(&mut self, r: &pdfplumber::obj::PageRef) -> PyResult<()> {
        if self.page_values.contains_key(&r.id()) {
            return Ok(());
        }
        let doc = self.op_doc.ok_or_else(|| PyErr::new(ExcKind::RuntimeError, "a page without a document"))?;
        let live = self
            .handles
            .iter()
            .filter_map(|(h, o)| match o {
                Object::Page(PageObj { doc: d, page: Some(p) }) if *d == doc && p.as_value().ptr_eq(r) => {
                    Some((self.oids.get(&h).copied().unwrap_or(u64::MAX), h))
                }
                _ => None,
            })
            .min();
        let h = match live {
            Some((_, h)) => h,
            None => {
                let pdf = match self.handles.get(doc) {
                    Some(Object::Document(d)) => d.state.pdf.clone(),
                    _ => None,
                };
                let page = pdf
                    .map(|pdf| pdf.cached_pages())
                    .unwrap_or_default()
                    .into_iter()
                    .find(|p| p.as_value().ptr_eq(r))
                    .ok_or_else(|| PyErr::new(ExcKind::RuntimeError, "a page value of no page"))?;
                let h = self.insert(Object::Page(PageObj { doc, page: Some(page) }))?;
                u32::try_from(crate::pages::int_handle(&h)?).map_err(|_| exc::ValueError("invalid handle"))?
            }
        };
        self.page_values.insert(r.id(), h);
        Ok(())
    }
}

/// `obj.method()`: a method that takes no argument (`TypeError` as
/// CPython words it otherwise).
fn no_arguments(args: &PyValue, qualname: &str) -> PyResult<()> {
    let n = match args {
        PyValue::None => 0,
        v => seq(v)?.len(),
    };
    if n == 0 {
        Ok(())
    } else {
        Err(exc::TypeError(format!("{qualname}() takes 1 positional argument but {} were given", n + 1)))
    }
}
