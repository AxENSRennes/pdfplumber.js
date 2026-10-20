//! Stage `plumber_text`: `extract_text` (simple, `layout=True`, option
//! sets), `extract_text_simple`, `extract_words`, `extract_text_lines`,
//! `search`, `dedupe_chars`, on the page and on derived pages. The
//! traversal and the line format are those of
//! `oracle/stages/plumber_text.py`.
//!
//! The ops of `plumber_ops::TEXT_BASE_OPS` and a `select` of
//! `TEXT_VARIANT_OPS` (`TEXT_VARIANTS_PER_PAGE`), then `TEXT_DERIVED_OPS`
//! on a `select` of `TEXT_DERIVED` (`TEXT_DERIVED_PER_PAGE`), each op
//! through the `Page` methods; a derivation through `crop`,
//! `within_bbox`, `outside_bbox`, `filter` (the predicates `upright`,
//! `not-rect`, `small` of the Python stage, as `FilterFn`s) and
//! `dedupe_chars`, the box of a step computed with `PyValue` arithmetic as
//! `derive_bbox` does. `search`: `re` is not ported; the matches are the
//! spans of the oracle's `spans <i>` line for that op
//! (`StageInput::oracle`, `ORACLE_INPUT`), copied to the trace, and the
//! text map is computed first as `search_spans` computes it (it fills the
//! page's `get_textmap` cache). Without the oracle's trace (the Wasm depth
//! gate), a search has no match and its `spans` line is `[]`.
//!
//! The Python stage's functions are generators or helpers (`ops_lines`,
//! `run_op`, `search_spans`, `derive`, `derive_bbox`): a frame each, so
//! that pdfplumber's code runs at the depth Python runs it.

use std::collections::HashMap;
use std::io::{self, Write};
use std::rc::Rc;

use pdfplumber::container::Container;
use pdfplumber::obj::{Obj, PyValue};
use pdfplumber::page::{CropFn, FilterFn, Page, SearchArgs};
use pdfplumber::utils::text::{Kwargs, RegexMatch};
use pycompat::recursion::py_frame;
use pycompat::{PyInt, PyResult, PyStr};

use super::common::{terminated, Lines};
use super::plumber_common::{bounded, exc as exc_line, open_pdf, select, table};
use super::plumber_ops::{
    TEXT_BASE_OPS, TEXT_DERIVED, TEXT_DERIVED_OPS, TEXT_DERIVED_PER_PAGE, TEXT_VARIANTS_PER_PAGE, TEXT_VARIANT_OPS,
};
use super::{Stage, StageInput};
use crate::canon::{canon, sha256_hex};
use crate::plumber::{from_value, obj_value};
use crate::value::Value;

pub const STAGE: Stage = Stage { name: "plumber_text", run: Some(run), oracle_input: true };

/// The `page.search` arguments that are not text map arguments.
const SEARCH_ARGS: [&str; 5] = ["regex", "case", "main_group", "return_chars", "return_groups"];

/// The `spans <i> <spans>` lines of an oracle trace: (page number, op
/// index) -> the text after the index.
pub fn oracle_spans(trace: &str) -> HashMap<(String, String), String> {
    let mut out = HashMap::new();
    let mut page = String::new();
    for line in trace.lines() {
        if let Some(n) = line.strip_prefix("page ") {
            page = n.to_owned();
        } else if let Some(rest) = line.strip_prefix("spans ") {
            if let Some((i, spans)) = rest.split_once(' ') {
                out.insert((page.clone(), i.to_owned()), spans.to_owned());
            }
        }
    }
    out
}

/// The matches of a `spans` text: `None`, or `[[(a, b) | None, ...], ...]`.
pub fn parse_spans(text: &str) -> Result<Option<Vec<RegexMatch>>, String> {
    if text == "None" {
        return Ok(None);
    }
    let b = text.as_bytes();
    let mut i = 0;
    let err = |i: usize| format!("spans: unexpected text at {i}: {text}");
    let ws = |i: &mut usize| {
        while *i < b.len() && b[*i] == b' ' {
            *i += 1;
        }
    };
    let num = |i: &mut usize| -> Result<usize, String> {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        text[start..*i].parse::<usize>().map_err(|_| err(start))
    };
    let expect = |i: &mut usize, c: u8| -> Result<(), String> {
        if *i < b.len() && b[*i] == c {
            *i += 1;
            Ok(())
        } else {
            Err(err(*i))
        }
    };
    let mut out = Vec::new();
    expect(&mut i, b'[')?;
    loop {
        ws(&mut i);
        if i < b.len() && b[i] == b']' {
            i += 1;
            break;
        }
        expect(&mut i, b'[')?;
        let mut spans = Vec::new();
        loop {
            ws(&mut i);
            if i < b.len() && b[i] == b']' {
                i += 1;
                break;
            }
            if text[i..].starts_with("None") {
                spans.push(None);
                i += 4;
            } else {
                expect(&mut i, b'(')?;
                let a = num(&mut i)?;
                expect(&mut i, b',')?;
                ws(&mut i);
                let e = num(&mut i)?;
                expect(&mut i, b')')?;
                spans.push(Some((a, e)));
            }
            ws(&mut i);
            if i < b.len() && b[i] == b',' {
                i += 1;
            }
        }
        out.push(RegexMatch { spans });
        ws(&mut i);
        if i < b.len() && b[i] == b',' {
            i += 1;
        }
    }
    if i != b.len() {
        return Err(err(i));
    }
    Ok(Some(out))
}

/// The keyword arguments of an op (a typed dict of the table).
pub fn kwargs_of(v: &Value) -> Kwargs {
    match from_value(v) {
        Ok(PyValue::Dict(ref d)) => d.clone(),
        other => panic!("op kwargs: {other:?}"),
    }
}

fn str_of(v: &Value) -> &str {
    v.as_str().expect("a str in an op table")
}

// oracle/stages/plumber_text.py PREDICATES
/// The predicates of `filter` steps: `upright` (`obj.get("upright",
/// True)`), `not-rect` (`obj["object_type"] != "rect"`), `small`
/// (`obj.get("size", 0) < 10`).
pub fn predicate(name: &str) -> FilterFn {
    match name {
        "upright" => Rc::new(|o: &Obj| Ok(o.get("upright").cloned().unwrap_or(PyValue::Bool(true)))),
        "not-rect" => Rc::new(|o: &Obj| Ok(PyValue::Bool(!o.item("object_type")?.py_eq(&PyValue::str("rect"))))),
        "small" => Rc::new(|o: &Obj| {
            let size = o.get("size").cloned().unwrap_or(PyValue::from(0i64));
            Ok(PyValue::Bool(size.lt(&PyValue::from(10i64))?))
        }),
        other => panic!("unknown predicate {other}"),
    }
}

/// `derive_bbox(page, fractions, relative)`: a frame.
pub fn derive_bbox(page: &Page, fractions: &Value, relative: bool) -> PyResult<PyValue> {
    let _frame = py_frame()?;
    let [x0, top, x1, bottom] = pdfplumber::obj::bbox_from_value(&page.bbox)?;
    let w = x1.sub(&x0)?;
    let h = bottom.sub(&top)?;
    let Value::List(f) = fractions else { panic!("fractions: {fractions:?}") };
    let f: Vec<PyValue> = f.iter().map(|v| from_value(v).expect("a number")).collect();
    if relative {
        return Ok(PyValue::tuple([w.mul(&f[0])?, h.mul(&f[1])?, w.mul(&f[2])?, h.mul(&f[3])?]));
    }
    Ok(PyValue::tuple([
        x0.add(&w.mul(&f[0])?)?,
        top.add(&h.mul(&f[1])?)?,
        x0.add(&w.mul(&f[2])?)?,
        top.add(&h.mul(&f[3])?)?,
    ]))
}

/// `derive(page, steps)`: a frame.
pub fn derive(page: &Rc<Page>, steps: &Value) -> PyResult<Rc<Page>> {
    let _frame = py_frame()?;
    let Value::List(steps) = steps else { panic!("steps: {steps:?}") };
    let mut page = page.clone();
    for step in steps {
        let Value::List(step) = step else { panic!("step: {step:?}") };
        let name = str_of(&step[0]);
        page = match name {
            "crop" | "within_bbox" | "outside_bbox" => {
                let kwargs = kwargs_of(&step[2]);
                let relative = kwargs.get("relative").is_some_and(PyValue::truthy);
                let bbox = derive_bbox(&page, &step[1], relative)?;
                let strict = kwargs.get("strict").is_none_or(PyValue::truthy);
                let crop_fn = match name {
                    "crop" => CropFn::CropToBbox,
                    "within_bbox" => CropFn::WithinBbox,
                    _ => CropFn::OutsideBbox,
                };
                page.crop(bbox, crop_fn, relative, strict)?
            }
            "filter" => page.filter(predicate(str_of(&step[1])))?,
            "dedupe_chars" => page.dedupe_chars(&kwargs_of(&step[1]))?,
            other => panic!("unknown step {other}"),
        };
    }
    Ok(page)
}

/// The text map keyword arguments of a search op.
fn textmap_kwargs(kwargs: &Kwargs) -> Kwargs {
    let mut out = Kwargs::new();
    for (k, v) in kwargs.items() {
        if !SEARCH_ARGS.iter().any(|a| *k == **a) {
            out.set(k, v.clone());
        }
    }
    out
}

/// `run_op(page, op)`: a frame; the value of the op.
pub fn run_op(page: &Rc<Page>, op: &[Value], spans: Option<Vec<RegexMatch>>) -> PyResult<Value> {
    let _frame = py_frame()?;
    let name = str_of(&op[0]);
    let objs = |v: Vec<Obj>| Value::List(v.iter().map(obj_value).collect());
    Ok(match name {
        "search" => {
            let kwargs = kwargs_of(&op[2]);
            let truthy = |k: &str, default: bool| kwargs.get(k).map_or(default, PyValue::truthy);
            let main_group = match kwargs.get("main_group") {
                None => 0,
                Some(v) => match v.as_num().and_then(|n| match n {
                    pycompat::Num::Int(i) => i.as_i64(),
                    _ => None,
                }) {
                    Some(g) if g >= 0 => g as usize,
                    _ => panic!("main_group: {v:?}"),
                },
            };
            let args = SearchArgs {
                main_group,
                return_chars: truthy("return_chars", true),
                return_groups: truthy("return_groups", true),
            };
            let mut matches = |_: &PyStr| -> PyResult<Vec<RegexMatch>> {
                // the oracle's matches (a text map raising there raises
                // here too, before this is called)
                Ok(spans.clone().unwrap_or_default())
            };
            objs(page.search(&mut matches, args, &textmap_kwargs(&kwargs))?)
        }
        "dedupe_chars" => {
            let deduped = page.dedupe_chars(&kwargs_of(&op[1]))?;
            let deduped_chars = Container::chars(&*deduped)?;
            let chars = Container::chars(&**page)?;
            Value::List(
                deduped_chars
                    .iter()
                    .map(|c| {
                        let i = chars.iter().position(|x| x.ptr_eq(c)).expect("a char of the page");
                        Value::Int(PyInt::Small(i as i64))
                    })
                    .collect(),
            )
        }
        "extract_text" => Value::py_str(&page.extract_text(&kwargs_of(&op[1]))?),
        "extract_text_simple" => Value::py_str(&page.extract_text_simple(&kwargs_of(&op[1]))?),
        "extract_words" => objs(page.extract_words(&kwargs_of(&op[1]))?),
        "extract_text_lines" => {
            let mut kwargs = kwargs_of(&op[1]);
            let strip = if kwargs.contains("strip") { kwargs.remove("strip")?.truthy() } else { true };
            let return_chars =
                if kwargs.contains("return_chars") { kwargs.remove("return_chars")?.truthy() } else { true };
            objs(page.extract_text_lines(strip, return_chars, &kwargs)?)
        }
        other => panic!("unknown op {other}"),
    })
}

/// `ops_lines(page, ops, counter)`: a generator (a frame).
fn ops_lines(
    lines: &mut Lines,
    page: &Rc<Page>,
    ops: &[&Value],
    counter: &mut usize,
    spans: &HashMap<(String, String), String>,
    has_oracle: bool,
) -> PyResult<()> {
    let _gen = py_frame()?;
    for op in ops {
        let i = *counter;
        *counter += 1;
        lines.push(format!("op {i} {}", canon(op)));
        let Value::List(op) = op else { panic!("op: {op:?}") };
        let mut matches = None;
        if str_of(&op[0]) == "search" {
            // search_spans(page, pattern, kwargs): a frame, the text map
            // computed (and cached) as Python's does
            {
                let _frame = py_frame()?;
                let _ = pdfplumber::page::tuplify_list_kwargs(&textmap_kwargs(&kwargs_of(&op[2])))
                    .and_then(|kw| page.get_textmap(&kw));
            }
            let text = if has_oracle {
                spans.get(&(page.page_number.to_string(), i.to_string())).cloned().unwrap_or_else(|| "None".to_owned())
            } else {
                "[]".to_owned()
            };
            lines.push(format!("spans {i} {text}"));
            matches = match parse_spans(&text) {
                Ok(m) => m,
                Err(e) => panic!("the oracle's trace: {e}"),
            };
        }
        match run_op(page, op, matches) {
            Err(e) => lines.push(format!("out {i} {}", exc_line(&e))),
            Ok(value) => lines.push(bounded("out", &i.to_string(), &canon(&value))),
        }
    }
    Ok(())
}

fn run(input: &StageInput<'_>, out: &mut dyn Write) -> io::Result<()> {
    let spans = input.oracle.map(oracle_spans).unwrap_or_default();
    let has_oracle = input.oracle.is_some();
    let sha = sha256_hex(input.pdf);
    let base_ops = table(TEXT_BASE_OPS);
    let variant_ops = table(TEXT_VARIANT_OPS);
    let derived_all = table(TEXT_DERIVED);
    let derived_ops = table(TEXT_DERIVED_OPS);
    terminated(out, |lines: &mut Lines| {
        let pdf = match open_pdf(input, |_| {}) {
            Ok(pdf) => pdf,
            Err(e) => {
                lines.push(format!("open {}", exc_line(&e)));
                return Ok(());
            }
        };
        let pages = match pdf.pages() {
            Ok(pages) => pages,
            Err(e) => {
                lines.push(format!("pages {}", exc_line(&e)));
                return Ok(());
            }
        };
        for (index, page) in pages.iter().enumerate() {
            lines.push(format!("page {}", page.page_number));
            if let Err(e) = Container::chars(&**page) {
                lines.push(format!("chars {}", exc_line(&e)));
                continue;
            }
            let mut counter = 0;
            let mut ops: Vec<&Value> = base_ops.iter().collect();
            ops.extend(select(&sha, index, &variant_ops, TEXT_VARIANTS_PER_PAGE));
            ops_lines(lines, page, &ops, &mut counter, &spans, has_oracle)?;
            for steps in select(&sha, index, &derived_all, TEXT_DERIVED_PER_PAGE) {
                lines.push(format!("derived {}", canon(steps)));
                let derived = match derive(page, steps) {
                    Ok(d) => d,
                    Err(e) => {
                        lines.push(format!("derive {}", exc_line(&e)));
                        continue;
                    }
                };
                let ops: Vec<&Value> = derived_ops.iter().collect();
                ops_lines(lines, &derived, &ops, &mut counter, &spans, has_oracle)?;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_parse() {
        assert!(parse_spans("None").unwrap().is_none());
        assert!(parse_spans("[]").unwrap().unwrap().is_empty());
        let m = parse_spans("[[(0, 3), None], [(4, 5), (4, 5)]]").unwrap().unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].spans, vec![Some((0, 3)), None]);
        assert_eq!(m[1].spans, vec![Some((4, 5)), Some((4, 5))]);
        assert!(parse_spans("[[(0 3)]]").is_err());
    }

    #[test]
    fn reads_spans_by_page_and_op() {
        let s = oracle_spans("page 1\nop 0 x\nspans 0 []\npage 2\nspans 0 None\n");
        assert_eq!(s[&("1".into(), "0".into())], "[]");
        assert_eq!(s[&("2".into(), "0".into())], "None");
    }
}
