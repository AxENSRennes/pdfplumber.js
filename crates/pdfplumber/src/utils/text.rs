//! Port of `pdfplumber/utils/text.py`: words, text maps and text extraction
//! from `char` objects.
//!
//! Keyword arguments: upstream passes options as `**kwargs` and filters them
//! by signature (`TEXTMAP_KWARGS`, `WORD_EXTRACTOR_KWARGS`). The port keeps
//! that interface ([`Kwargs`], an ordered `str`-keyed dict of values, as a
//! JS options object maps to it) next to typed constructors.
//!
//! Search: [`TextMap::search`] takes the matches of a regular expression
//! computed by the caller over [`TextMap::as_string`]. To reproduce
//! `TextMap.search(pattern, regex, case, ...)` exactly, the matches must be
//! those of Python's `re` (pdfplumber/utils/text.py:172 TextMap.search):
//! `compiled = re.compile(pattern if regex else re.escape(pattern), re.I if
//! case is False else 0)`, then for each `m` of `re.finditer(compiled,
//! textmap.as_string)`, `[m.span(g) if m.start(g) >= 0 else None for g in
//! range(compiled.groups + 1)]` as a [`RegexMatch`] (spans are indices of
//! code points, i.e. of the Python `str`). Filtering (empty and
//! whitespace-only matches) and the result dicts are computed here. The two
//! fixed patterns of `extract_text_lines` are matched natively
//! ([`text_line_matches`]). A JavaScript `RegExp` does not give these
//! matches: `finditer` retries a non-empty match where an empty one was
//! found (`x*|abc` on `abc`), `\w`, `\d`, `\s` and `\b` are Unicode-aware,
//! the syntax differs (`(?P<name>...)`, inline flags, `$` before a final
//! newline), and JS indices count UTF-16 units. A matcher with Python's
//! semantics for the browser is an open item of the JavaScript API
//! (`docs/architecture.md`, "Frontière JavaScript"); the replay feeds the
//! spans Python computed.
//!
//! Upstream's `logger.warning` calls (deprecated `vertical_ttb`,
//! `horizontal_ltr`) have no effect on results and are not reproduced.
//!
//! `HashKey` holds `Rc`-shared values, which clippy takes for interior
//! mutability; a key's hash only reads immutable parts, so
//! `mutable_key_type` is allowed.
#![allow(clippy::mutable_key_type)]

use rustc_hash::FxHashMap as HashMap;

use pycompat::listsort::sort_by_key;
use pycompat::pystr::PyStrBuilder;
use pycompat::text::str_isspace_cp;
use pycompat::{exc, ExcKind, PyErr, PyInt, PyResult, PyStr};

use crate::obj::{check_hashable, py_contains, py_iter, HashKey, Hot, Obj, PyValue};
use crate::typing::T_dir;
use crate::utils::clustering::cluster_objects;
use crate::utils::geometry::objects_to_bbox_refs;

/// Options passed as `**kwargs`: an ordered `str`-keyed dict.
pub type Kwargs = Obj;

// pdfplumber/utils/text.py:27 DEFAULT_X_TOLERANCE
pub const DEFAULT_X_TOLERANCE: i64 = 3;
// pdfplumber/utils/text.py:28 DEFAULT_Y_TOLERANCE
pub const DEFAULT_Y_TOLERANCE: i64 = 3;
// pdfplumber/utils/text.py:29 DEFAULT_X_DENSITY
pub const DEFAULT_X_DENSITY: f64 = 7.25;
// pdfplumber/utils/text.py:30 DEFAULT_Y_DENSITY
pub const DEFAULT_Y_DENSITY: i64 = 13;
// pdfplumber/utils/text.py:31 DEFAULT_LINE_DIR
pub const DEFAULT_LINE_DIR: T_dir = T_dir::Ttb;
// pdfplumber/utils/text.py:32 DEFAULT_CHAR_DIR
pub const DEFAULT_CHAR_DIR: T_dir = T_dir::Ltr;

// pdfplumber/utils/text.py:34 LIGATURES
pub const LIGATURES: [(&str, &str); 7] = [
    ("\u{fb00}", "ff"),
    ("\u{fb03}", "ffi"),
    ("\u{fb04}", "ffl"),
    ("\u{fb01}", "fi"),
    ("\u{fb02}", "fl"),
    ("\u{fb06}", "st"),
    ("\u{fb05}", "st"),
];

/// `string.punctuation`
pub const PUNCTUATION: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";

/// `LIGATURES.get(text)`
fn ligature(text: &str) -> Option<&'static str> {
    LIGATURES.iter().find(|(k, _)| *k == text).map(|(_, v)| *v)
}

/// The resource guard of repeated items (`[x] * n`, a loop over `range(n)`
/// that appends): a count no real page reaches. Python would try to
/// allocate (`MemoryError` when it cannot, which is certain from about
/// 2**44 items on, beyond any address space) or loop until memory runs out.
const MAX_REPEAT: i64 = 10_000_000;

fn guarded_count(v: i64) -> PyResult<usize> {
    match v {
        ..=0 => Ok(0),
        1..=MAX_REPEAT => Ok(v as usize),
        _ => Err(exc::MemoryError("")),
    }
}

/// `[x] * n` for an `int` `n` (`list_repeat`): `n` must fit a `Py_ssize_t`
/// (64 bits); a negative count gives an empty list.
fn list_repeat_count(n: &PyInt) -> PyResult<usize> {
    match n.as_i64() {
        Some(v) => guarded_count(v),
        None => Err(exc::OverflowError("cannot fit 'int' into an index-sized integer")),
    }
}

/// The number of iterations of `for _ in range(n)` for an `int` `n`.
fn range_len(n: &PyInt) -> PyResult<usize> {
    match n.as_i64() {
        Some(v) => guarded_count(v),
        None if n.is_negative() => Ok(0),
        None => Err(exc::MemoryError("")),
    }
}

/// `[x] * n` with a dynamic `n`.
fn seq_repeat_count(n: &PyValue) -> PyResult<usize> {
    match n {
        PyValue::Int(i) => list_repeat_count(i),
        PyValue::Bool(b) => Ok(usize::from(*b)),
        other => Err(exc::TypeError(format!("can't multiply sequence by non-int of type '{}'", other.type_name()))),
    }
}

/// `range(n)` with a dynamic `n`: its length.
fn range_count(n: &PyValue) -> PyResult<usize> {
    match n {
        PyValue::Int(i) => range_len(i),
        PyValue::Bool(b) => Ok(usize::from(*b)),
        other => Err(exc::TypeError(format!("'{}' object cannot be interpreted as an integer", other.type_name()))),
    }
}

/// `round(x)` of a number (`int` result).
fn py_round_value(x: &PyValue) -> PyResult<PyInt> {
    match x.as_num() {
        Some(n) => pycompat::py_round(&n),
        None => Err(exc::TypeError(format!("type {} doesn't define __round__ method", x.type_name()))),
    }
}

/// The text of a char (`char["text"]`) for the operations that need a `str`.
fn char_text(c: &Obj) -> PyResult<&PyValue> {
    c.hot(Hot::Text)
}

/// `expansions.get(text, text)` of a `str` (a ligature is never a
/// surrogate).
fn expansion(s: &PyStr, expand: bool) -> Option<&'static str> {
    if expand {
        s.as_str().and_then(ligature)
    } else {
        None
    }
}

/// `for letter in expansions.get(char["text"], char["text"]):
/// _textmap.append((letter, char))`; returns the number of letters.
///
/// The chars of a WordMap that pdfplumber makes went through
/// `WordExtractor.merge_chars`, whose `"".join(...)` raises `TypeError` for
/// a text that is not a `str`: such a char reaches a text map only through
/// a `WordMap` built by hand (`WordMap(tuples)`, which pdfplumber's `Page`
/// and module functions never do, and the port's API does not offer), so
/// that case is refused.
fn push_letters(textmap: &mut Vec<(u32, Option<Obj>)>, c: &Obj, expand: bool) -> PyResult<i64> {
    let PyValue::Str(s) = char_text(c)? else {
        return Err(PyErr::not_ported("a hand-made WordMap char whose text is not a str"));
    };
    let before = textmap.len();
    match expansion(s, expand) {
        Some(e) => textmap.extend(e.chars().map(|ch| (ch as u32, Some(c.clone())))),
        None => textmap.extend(s.code_points().map(|cp| (cp, Some(c.clone())))),
    }
    Ok((textmap.len() - before) as i64)
}

/// The `str` of code points (all at most U+10FFFF).
fn cps_str(cps: &[u32]) -> PyStr {
    let mut b = PyStrBuilder::with_capacity(cps.len());
    for &c in cps {
        b.push(c);
    }
    b.finish()
}

/// A `ValueError` (or other) whose message ends with a value's `str()`
/// (it may hold a lone surrogate).
fn err_ending_with(kind: ExcKind, prefix: &str, value: &PyStr, suffix: &str) -> PyErr {
    let mut b = PyStrBuilder::new();
    b.push_rust_str(prefix);
    b.push_str(value);
    b.push_rust_str(suffix);
    PyErr::with_message(kind, &b.finish())
}

// -- directions -----------------------------------------------------------------

// pdfplumber/utils/text.py:45 get_line_cluster_key
pub fn line_cluster_key(line_dir: T_dir, x: &Obj) -> PyResult<PyValue> {
    match line_dir {
        T_dir::Ttb => Ok(x.hot(Hot::Top)?.clone()),
        T_dir::Btt => x.hot(Hot::Bottom)?.neg(),
        T_dir::Ltr => Ok(x.hot(Hot::X0)?.clone()),
        T_dir::Rtl => x.hot(Hot::X1)?.neg(),
    }
}

/// A sort key of two values, compared as a `tuple`.
pub type PairKey = (PyValue, PyValue);

/// `a < b` for two pairs (`tuplerichcompare`: the first items that differ
/// by `PyObject_RichCompareBool`, which has the identity shortcut).
pub fn pair_lt(a: &PairKey, b: &PairKey) -> PyResult<bool> {
    if !a.0.eq_bool(&b.0) {
        return a.0.lt(&b.0);
    }
    if !a.1.eq_bool(&b.1) {
        return a.1.lt(&b.1);
    }
    Ok(false)
}

// pdfplumber/utils/text.py:54 get_char_sort_key
pub fn char_sort_key(char_dir: T_dir, x: &Obj) -> PyResult<PairKey> {
    match char_dir {
        T_dir::Ttb => Ok((x.hot(Hot::Top)?.clone(), x.hot(Hot::Bottom)?.clone())),
        T_dir::Btt => {
            let a = x.hot(Hot::Top)?.add(x.hot(Hot::Height)?)?.neg()?;
            Ok((a, x.hot(Hot::Top)?.neg()?))
        }
        T_dir::Ltr => Ok((x.hot(Hot::X0)?.clone(), x.hot(Hot::X0)?.clone())),
        T_dir::Rtl => Ok((x.hot(Hot::X1)?.neg()?, x.hot(Hot::X0)?.neg()?)),
    }
}

// pdfplumber/utils/text.py:63 BBOX_ORIGIN_KEYS
fn bbox_origin(dir: T_dir, bbox: &PyValue) -> PyResult<PyValue> {
    let i = match dir {
        T_dir::Ttb => 1,
        T_dir::Btt => 3,
        T_dir::Ltr => 0,
        T_dir::Rtl => 2,
    };
    crate::obj::py_getitem_index(bbox, i)
}

// pdfplumber/utils/text.py:70 POSITION_KEYS
fn position_key(dir: T_dir, x: &Obj) -> PyResult<PyValue> {
    Ok(match dir {
        T_dir::Ttb => x.hot(Hot::Top)?,
        T_dir::Btt => x.hot(Hot::Bottom)?,
        T_dir::Ltr => x.hot(Hot::X0)?,
        T_dir::Rtl => x.hot(Hot::X1)?,
    }
    .clone())
}

/// `{"ttb": ..., ...}[d]` for a direction given as a value (`KeyError`
/// otherwise).
fn dir_key(d: &PyValue) -> PyResult<T_dir> {
    check_hashable(d)?;
    d.as_str().and_then(T_dir::parse).ok_or_else(|| exc::KeyError(d.repr()))
}

/// `str(valid_dirs)` of `validate_directions`. Python prints a `set`, whose
/// order depends on string hashing (random per process): the port prints
/// the order of `POSITION_KEYS`, and the oracle normalizes its message to
/// it.
pub const VALID_DIRS_REPR: &str = "{'ttb', 'btt', 'ltr', 'rtl'}";

/// One direction checked by [`validate_directions`].
fn valid_dir(d: &PyValue, name: &str, suffix: &str) -> PyResult<T_dir> {
    let invalid = || {
        let prefix = format!("{name}{suffix} must be one of {VALID_DIRS_REPR}, not ");
        err_ending_with(ExcKind::ValueError, &prefix, &d.py_str(), "")
    };
    // `d not in valid_dirs` (a set) hashes `d`
    check_hashable(d)?;
    d.as_str().and_then(T_dir::parse).ok_or_else(invalid)
}

// pdfplumber/utils/text.py:78 validate_directions
pub fn validate_directions(line_dir: &PyValue, char_dir: &PyValue, suffix: &str) -> PyResult<(T_dir, T_dir)> {
    let ld = valid_dir(line_dir, "line_dir", suffix)?;
    let cd = valid_dir(char_dir, "char_dir", suffix)?;
    let axis = |d: T_dir| matches!(d, T_dir::Ttb | T_dir::Btt);
    // set(line_dir) == set(char_dir): "ttb"/"btt" and "ltr"/"rtl" have the
    // same letters
    if axis(ld) == axis(cd) {
        return Err(exc::ValueError(format!(
            "line_dir{suffix}={} is incompatible with char_dir{suffix}={}",
            ld.name(),
            cd.name()
        )));
    }
    Ok((ld, cd))
}

fn dir_value(d: T_dir) -> PyValue {
    PyValue::str(d.name())
}

// -- TextMap --------------------------------------------------------------------

/// The spans of one regular-expression match over [`TextMap::as_string`]:
/// `spans[g]` is `m.span(g)` in code points, `None` for a group that did
/// not participate (`spans[0]` is the whole match).
#[derive(Clone, Debug)]
pub struct RegexMatch {
    pub spans: Vec<Option<(usize, usize)>>,
}

// pdfplumber/utils/text.py:95 TextMap
#[derive(Clone, Debug)]
pub struct TextMap {
    /// `(text, char)`: one code point each (`ord` of the one-character
    /// `str`), `None` for layout whitespace.
    pub tuples: Vec<(u32, Option<Obj>)>,
    pub line_dir_render: T_dir,
    pub char_dir_render: T_dir,
    pub as_string: PyStr,
}

/// `"\n"` and `" "` as code points.
const NEWLINE: u32 = 0x0a;
const SPACE: u32 = 0x20;

impl TextMap {
    // pdfplumber/utils/text.py:101 TextMap.__init__
    pub fn new(
        tuples: Vec<(u32, Option<Obj>)>,
        line_dir_render: &PyValue,
        char_dir_render: &PyValue,
    ) -> PyResult<TextMap> {
        let (ld, cd) = validate_directions(line_dir_render, char_dir_render, "_render")?;
        let mut tm = TextMap { tuples, line_dir_render: ld, char_dir_render: cd, as_string: PyStr::empty() };
        tm.as_string = tm.to_string();
        Ok(tm)
    }

    // pdfplumber/utils/text.py:113 TextMap.to_string
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> PyStr {
        let cd = self.char_dir_render;
        let ld = self.line_dir_render;
        let base: Vec<u32> = self.tuples.iter().map(|t| t.0).collect();
        if cd == T_dir::Ltr && ld == T_dir::Ttb {
            return cps_str(&base);
        }
        let mut lines: Vec<Vec<u32>> = base.split(|&c| c == NEWLINE).map(<[u32]>::to_vec).collect();
        if matches!(ld, T_dir::Btt | T_dir::Rtl) {
            lines.reverse();
        }
        if cd == T_dir::Rtl {
            for line in &mut lines {
                line.reverse();
            }
        }
        if matches!(ld, T_dir::Rtl | T_dir::Ltr) {
            let max_line_length = lines.iter().map(Vec::len).max().unwrap_or(0);
            for line in &mut lines {
                let pad = max_line_length - line.len();
                if cd == T_dir::Btt {
                    line.splice(0..0, std::iter::repeat_n(SPACE, pad));
                } else {
                    line.extend(std::iter::repeat_n(SPACE, pad));
                }
            }
            let mut out = Vec::new();
            for i in 0..max_line_length {
                if i > 0 {
                    out.push(NEWLINE);
                }
                out.extend(lines.iter().map(|line| line[i]));
            }
            cps_str(&out)
        } else {
            let mut out = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                if i > 0 {
                    out.push(NEWLINE);
                }
                out.extend(line.iter());
            }
            cps_str(&out)
        }
    }

    // pdfplumber/utils/text.py:145 TextMap.match_to_dict
    /// For a match whose `main_group` took part, with spans within `string`
    /// (`search` checks both).
    fn match_to_dict(
        &self,
        string: &[u32],
        m: &RegexMatch,
        main_group: usize,
        return_groups: bool,
        return_chars: bool,
    ) -> PyResult<Obj> {
        let (start, end) = m.spans[main_group].expect("a group that took part");
        // self.tuples[m.start(g):m.end(g)], clamped like a Python slice
        let n = self.tuples.len();
        let a = start.min(n);
        let subset = &self.tuples[a..end.min(n).max(a)];
        let chars: Vec<Obj> = subset.iter().filter_map(|(_, c)| c.clone()).collect();
        let [x0, top, x1, bottom] = objects_to_bbox_refs(chars.iter())?;
        let mut result = Obj::from_pairs([
            ("text", group_value(string, m.spans[main_group])),
            ("x0", x0),
            ("top", top),
            ("x1", x1),
            ("bottom", bottom),
        ]);
        if return_groups {
            result.set("groups", PyValue::tuple(m.spans[1..].iter().map(|s| group_value(string, *s))));
        }
        if return_chars {
            result.set("chars", PyValue::list(chars.into_iter().map(PyValue::Dict)));
        }
        Ok(result)
    }

    // pdfplumber/utils/text.py:172 TextMap.search
    /// `search` with the matches computed by the caller (see the module
    /// documentation).
    pub fn search(
        &self,
        matches: &[RegexMatch],
        return_groups: bool,
        return_chars: bool,
        main_group: usize,
    ) -> PyResult<Vec<Obj>> {
        let string: Vec<u32> = self.as_string.code_points().collect();
        // Matches come from the caller: a span outside the string cannot
        // come from a regular expression run over it.
        let valid = |s: &Option<(usize, usize)>| s.is_none_or(|(a, b)| a <= b && b <= string.len());
        if !matches.iter().all(|m| m.spans.iter().all(valid)) {
            return Err(exc::ValueError("match span outside TextMap.as_string"));
        }
        let mut out = Vec::new();
        for m in matches {
            // filter(lambda m: bool(m.group(main_group).strip()), gen)
            let span = match m.spans.get(main_group) {
                Some(Some(span)) => *span,
                Some(None) => return Err(exc::AttributeError("'NoneType' object has no attribute 'strip'")),
                None => return Err(exc::IndexError("no such group")),
            };
            if string[span.0..span.1].iter().all(|&c| str_isspace_cp(c)) {
                continue;
            }
            out.push(self.match_to_dict(&string, m, main_group, return_groups, return_chars)?);
        }
        Ok(out)
    }

    // pdfplumber/utils/text.py:212 TextMap.extract_text_lines
    pub fn extract_text_lines(&self, strip: bool, return_chars: bool) -> PyResult<Vec<Obj>> {
        let string: Vec<u32> = self.as_string.code_points().collect();
        let matches = text_line_matches(&string, strip);
        self.search(&matches, false, return_chars, 1)
    }
}

/// `m.group(g)`: the text of a span, `None` for a group that did not
/// participate.
fn group_value(string: &[u32], span: Option<(usize, usize)>) -> PyValue {
    match span {
        Some((a, b)) => PyValue::Str(cps_str(&string[a..b])),
        None => PyValue::None,
    }
}

/// `re.finditer` of the two patterns of `extract_text_lines` over `string`
/// (`r" *([^\n]+?) *(\n|$)"` when `strip`, else `r"([^\n]+)"`), with
/// Python's matching semantics, as [`RegexMatch`]es.
pub fn text_line_matches(s: &[u32], strip: bool) -> Vec<RegexMatch> {
    let n = s.len();
    let mut out = Vec::new();
    if !strip {
        let mut i = 0;
        while i < n {
            let start = i;
            if s[i] == NEWLINE {
                i += 1;
            } else {
                while i < n && s[i] != NEWLINE {
                    i += 1;
                }
                out.push(RegexMatch { spans: vec![Some((start, i)), Some((start, i))] });
            }
            debug_assert!(i > start, "progress guard (debug assertions: the `mutants` profile)");
        }
        return out;
    }
    let spaces_from = |i: usize| s[i..].iter().take_while(|&&c| c == SPACE).count();
    let mut pos = 0;
    while pos <= n {
        // search: the first start position where the pattern matches
        let mut found = None;
        for start in pos..=n {
            if let Some(m) = match_text_line_at(s, start, &spaces_from) {
                found = Some(m);
                break;
            }
        }
        let Some(m) = found else { break };
        let end = m.spans[0].expect("whole match").1;
        out.push(m);
        debug_assert!(end > pos, "progress guard (debug assertions: the `mutants` profile)");
        pos = end; // never an empty match: group 1 takes a character
    }
    out
}

/// The match of `" *([^\n]+?) *(\n|$)"` at `start` (backtracking as `re`
/// does: greedy leading spaces first, then the shortest group 1).
fn match_text_line_at(s: &[u32], start: usize, spaces_from: &impl Fn(usize) -> usize) -> Option<RegexMatch> {
    let n = s.len();
    let k_max = spaces_from(start);
    for k in (0..=k_max).rev() {
        let g1 = start + k;
        if g1 >= n || s[g1] == NEWLINE {
            continue;
        }
        let mut g1_end = g1 + 1;
        loop {
            // greedy ` *`: only the longest run can be followed by \n or $
            let q = g1_end + spaces_from(g1_end);
            if q == n {
                return Some(RegexMatch { spans: vec![Some((start, q)), Some((g1, g1_end)), Some((q, q))] });
            }
            if s[q] == NEWLINE {
                return Some(RegexMatch { spans: vec![Some((start, q + 1)), Some((g1, g1_end)), Some((q, q + 1))] });
            }
            // `[^\n]+?` takes one more character: there is one, and it is
            // not a newline (`q` would have stopped at it or at the end)
            g1_end += 1;
        }
    }
    None
}

// -- WordMap --------------------------------------------------------------------

/// The options of `WordMap.to_textmap`, as values (their defaults are
/// upstream's).
#[derive(Clone, Debug)]
pub struct TextMapOptions {
    pub layout: bool,
    pub layout_width: PyValue,
    pub layout_height: PyValue,
    pub layout_width_chars: PyValue,
    pub layout_height_chars: PyValue,
    pub layout_bbox: PyValue,
    pub x_density: PyValue,
    pub y_density: PyValue,
    pub x_shift: PyValue,
    pub y_shift: PyValue,
    pub y_tolerance: PyValue,
    pub line_dir: PyValue,
    pub char_dir: PyValue,
    pub line_dir_rotated: PyValue,
    pub char_dir_rotated: PyValue,
    pub char_dir_render: PyValue,
    pub line_dir_render: PyValue,
    pub use_text_flow: bool,
    pub presorted: bool,
    pub expand_ligatures: bool,
}

impl Default for TextMapOptions {
    // pdfplumber/utils/text.py:241 WordMap.to_textmap (defaults)
    fn default() -> TextMapOptions {
        let zero = PyValue::from(0i64);
        TextMapOptions {
            layout: false,
            layout_width: zero.clone(),
            layout_height: zero.clone(),
            layout_width_chars: zero.clone(),
            layout_height_chars: zero.clone(),
            layout_bbox: PyValue::tuple([zero.clone(), zero.clone(), zero.clone(), zero.clone()]),
            x_density: PyValue::from(DEFAULT_X_DENSITY),
            y_density: PyValue::from(DEFAULT_Y_DENSITY),
            x_shift: zero.clone(),
            y_shift: zero,
            y_tolerance: PyValue::from(DEFAULT_Y_TOLERANCE),
            line_dir: dir_value(DEFAULT_LINE_DIR),
            char_dir: dir_value(DEFAULT_CHAR_DIR),
            line_dir_rotated: PyValue::None,
            char_dir_rotated: PyValue::None,
            char_dir_render: PyValue::None,
            line_dir_render: PyValue::None,
            use_text_flow: false,
            presorted: false,
            expand_ligatures: true,
        }
    }
}

// pdfplumber/utils/text.py:701 TEXTMAP_KWARGS
pub const TEXTMAP_KWARGS: [&str; 21] = [
    "self",
    "layout",
    "layout_width",
    "layout_height",
    "layout_width_chars",
    "layout_height_chars",
    "layout_bbox",
    "x_density",
    "y_density",
    "x_shift",
    "y_shift",
    "y_tolerance",
    "line_dir",
    "char_dir",
    "line_dir_rotated",
    "char_dir_rotated",
    "char_dir_render",
    "line_dir_render",
    "use_text_flow",
    "presorted",
    "expand_ligatures",
];

impl TextMapOptions {
    /// `WordMap.to_textmap(**kwargs)`: with `strict`, an unknown keyword
    /// raises `TypeError` as Python's call does; otherwise only the keys of
    /// `TEXTMAP_KWARGS` are taken (`{k: kwargs[k] for k in TEXTMAP_KWARGS
    /// if k in kwargs}`, where `self` is one of them).
    pub fn from_kwargs(kwargs: &Kwargs, strict: bool) -> PyResult<TextMapOptions> {
        let mut o = TextMapOptions::default();
        if kwargs.contains("self") {
            return Err(exc::TypeError("WordMap.to_textmap() got multiple values for argument 'self'"));
        }
        for (k, v) in kwargs.items() {
            let v = v.clone();
            match k.as_str().unwrap_or("") {
                "layout" => o.layout = v.truthy(),
                "layout_width" => o.layout_width = v,
                "layout_height" => o.layout_height = v,
                "layout_width_chars" => o.layout_width_chars = v,
                "layout_height_chars" => o.layout_height_chars = v,
                "layout_bbox" => o.layout_bbox = v,
                "x_density" => o.x_density = v,
                "y_density" => o.y_density = v,
                "x_shift" => o.x_shift = v,
                "y_shift" => o.y_shift = v,
                "y_tolerance" => o.y_tolerance = v,
                "line_dir" => o.line_dir = v,
                "char_dir" => o.char_dir = v,
                "line_dir_rotated" => o.line_dir_rotated = v,
                "char_dir_rotated" => o.char_dir_rotated = v,
                "char_dir_render" => o.char_dir_render = v,
                "line_dir_render" => o.line_dir_render = v,
                "use_text_flow" => o.use_text_flow = v.truthy(),
                "presorted" => o.presorted = v.truthy(),
                "expand_ligatures" => o.expand_ligatures = v.truthy(),
                _ if strict => {
                    return Err(err_ending_with(
                        ExcKind::TypeError,
                        "WordMap.to_textmap() got an unexpected keyword argument '",
                        k,
                        "'",
                    ))
                }
                _ => {}
            }
        }
        Ok(o)
    }
}

/// `a or b`
fn or_value<'a>(a: &'a PyValue, b: &'a PyValue) -> &'a PyValue {
    if a.truthy() {
        a
    } else {
        b
    }
}

/// `int(round(a / b))`
fn round_div(a: &PyValue, b: &PyValue) -> PyResult<PyValue> {
    Ok(PyValue::Int(py_round_value(&a.truediv(b)?)?))
}

/// `max(a, b)` of two ints.
fn max_int(a: PyInt, b: PyInt) -> PyInt {
    if b > a {
        b
    } else {
        a
    }
}

// pdfplumber/utils/text.py:233 WordMap
#[derive(Clone, Debug, Default)]
pub struct WordMap {
    pub tuples: Vec<(Obj, Vec<Obj>)>,
}

impl WordMap {
    // pdfplumber/utils/text.py:238 WordMap.__init__
    pub fn new(tuples: Vec<(Obj, Vec<Obj>)>) -> WordMap {
        WordMap { tuples }
    }

    // pdfplumber/utils/text.py:241 WordMap.to_textmap
    pub fn to_textmap(&self, o: &TextMapOptions) -> PyResult<TextMap> {
        let mut textmap: Vec<(u32, Option<Obj>)> = Vec::new();

        if self.tuples.is_empty() {
            return TextMap::new(
                textmap,
                or_value(&o.line_dir_render, &o.line_dir),
                or_value(&o.char_dir_render, &o.char_dir),
            );
        }

        let expand = o.expand_ligatures;
        let mut layout_width_chars = o.layout_width_chars.clone();
        let mut layout_height_chars = o.layout_height_chars.clone();
        let blank_line: usize = if o.layout {
            if layout_width_chars.truthy() {
                if o.layout_width.truthy() {
                    return Err(exc::ValueError("`layout_width` and `layout_width_chars` cannot both be set."));
                }
            } else {
                layout_width_chars = round_div(&o.layout_width, &o.x_density)?;
            }
            if layout_height_chars.truthy() {
                if o.layout_height.truthy() {
                    return Err(exc::ValueError("`layout_height` and `layout_height_chars` cannot both be set."));
                }
            } else {
                layout_height_chars = round_div(&o.layout_height, &o.y_density)?;
            }
            seq_repeat_count(&layout_width_chars)?
        } else {
            0
        };
        let push_blank_line =
            |tm: &mut Vec<(u32, Option<Obj>)>| tm.extend(std::iter::repeat_n((SPACE, None), blank_line));

        let mut num_newlines = PyInt::ZERO;

        let line_dir = dir_key(&o.line_dir)?;
        let char_dir = dir_key(&o.char_dir)?;
        let y_origin = bbox_origin(line_dir, &o.layout_bbox)?;
        let x_origin = bbox_origin(char_dir, &o.layout_bbox)?;

        let keep_order = o.presorted || o.use_text_flow;
        let mut words_sorted_line_dir: Vec<usize> = (0..self.tuples.len()).collect();
        if !keep_order {
            sort_by_key(
                &mut words_sorted_line_dir,
                |&i| line_cluster_key(line_dir, &self.tuples[i].0),
                |a, b| a.lt(b),
            )?;
        }

        let tuples_by_line = cluster_objects(
            &words_sorted_line_dir,
            |&i| line_cluster_key(line_dir, &self.tuples[i].0),
            &o.y_tolerance,
            keep_order,
        )?;

        let one = PyValue::from(1i64);
        let minus_one = PyValue::from(-1i64);
        for (i, line_tuples) in tuples_by_line.iter().enumerate() {
            let y_dist = if o.layout {
                let line_position = position_key(line_dir, &self.tuples[line_tuples[0]].0)?;
                let y_dist_raw = line_position.sub(&y_origin.add(&o.y_shift)?)?;
                let adj = if matches!(line_dir, T_dir::Btt | T_dir::Rtl) { &minus_one } else { &one };
                y_dist_raw.mul(adj)?.truediv(&o.y_density)?
            } else {
                PyValue::from(0i64)
            };
            let num_newlines_prepend =
                max_int(PyInt::Small(i64::from(i > 0)), py_round_value(&y_dist)?.sub(&num_newlines));

            for _ in 0..range_len(&num_newlines_prepend)? {
                if textmap.last().is_none_or(|t| t.0 == NEWLINE) {
                    push_blank_line(&mut textmap);
                }
                textmap.push((NEWLINE, None));
            }

            num_newlines = num_newlines.add(&num_newlines_prepend);

            let mut line_len: i64 = 0;

            let mut line_tuples_sorted: Vec<usize> = line_tuples.clone();
            if !keep_order {
                sort_by_key(&mut line_tuples_sorted, |&i| char_sort_key(char_dir, &self.tuples[i].0), pair_lt)?;
            }

            for &w in &line_tuples_sorted {
                let (word, chars) = &self.tuples[w];
                let x_dist = if o.layout {
                    let char_position = position_key(char_dir, word)?;
                    let x_dist_raw = char_position.sub(&x_origin.add(&o.x_shift)?)?;
                    let adj = if matches!(char_dir, T_dir::Btt | T_dir::Rtl) { &minus_one } else { &one };
                    x_dist_raw.mul(adj)?.truediv(&o.x_density)?
                } else {
                    PyValue::from(0i64)
                };
                let num_spaces_prepend =
                    max_int(PyInt::Small(line_len.min(1)), py_round_value(&x_dist)?.sub(&PyInt::Small(line_len)));
                let n = list_repeat_count(&num_spaces_prepend)?;
                textmap.extend(std::iter::repeat_n((SPACE, None), n));
                line_len = line_len
                    .checked_add(num_spaces_prepend.as_i64().ok_or_else(|| exc::MemoryError(""))?)
                    .ok_or_else(|| exc::MemoryError(""))?;

                for c in chars {
                    line_len += push_letters(&mut textmap, c, expand)?;
                }
            }

            // Append spaces at end of line
            if o.layout {
                let n = seq_repeat_count(&layout_width_chars.sub(&PyValue::from(line_len))?)?;
                textmap.extend(std::iter::repeat_n((SPACE, None), n));
            }
        }

        // Append blank lines at end of text
        if o.layout {
            let num_newlines_append = layout_height_chars.sub(&PyValue::Int(num_newlines.add(&PyInt::Small(1))))?;
            for i in 0..range_count(&num_newlines_append)? {
                if i > 0 {
                    push_blank_line(&mut textmap);
                }
                textmap.push((NEWLINE, None));
            }

            // Remove terminal newline
            match textmap.last() {
                None => return Err(exc::IndexError("list index out of range")),
                Some((NEWLINE, None)) => {
                    textmap.pop();
                }
                Some(_) => {}
            }
        }

        TextMap::new(textmap, or_value(&o.line_dir_render, &o.line_dir), or_value(&o.char_dir_render, &o.char_dir))
    }
}

// -- WordExtractor ----------------------------------------------------------------

/// `extra_attrs` and `split_at_punctuation` are kept as given, as upstream
/// does: `extra_attrs` is iterated when words are extracted (`itemgetter`),
/// `text in split_at_punctuation` is Python's `in` (a substring test for a
/// `str`, membership for a `list`...).
#[derive(Clone, Debug)]
pub struct WordExtractor {
    pub x_tolerance: PyValue,
    pub y_tolerance: PyValue,
    pub x_tolerance_ratio: PyValue,
    pub y_tolerance_ratio: PyValue,
    pub keep_blank_chars: bool,
    pub use_text_flow: bool,
    pub horizontal_ltr: bool,
    pub vertical_ttb: bool,
    pub line_dir: T_dir,
    pub char_dir: T_dir,
    pub line_dir_rotated: T_dir,
    pub char_dir_rotated: T_dir,
    /// `[] if extra_attrs is None else extra_attrs`
    pub extra_attrs: PyValue,
    pub split_at_punctuation: PyValue,
    pub expand_ligatures: bool,
}

/// The arguments of `WordExtractor(...)`, as values (defaults are
/// upstream's).
#[derive(Clone, Debug)]
pub struct WordExtractorArgs {
    pub x_tolerance: PyValue,
    pub y_tolerance: PyValue,
    pub x_tolerance_ratio: PyValue,
    pub y_tolerance_ratio: PyValue,
    pub keep_blank_chars: PyValue,
    pub use_text_flow: PyValue,
    pub vertical_ttb: PyValue,
    pub horizontal_ltr: PyValue,
    pub line_dir: PyValue,
    pub char_dir: PyValue,
    pub line_dir_rotated: PyValue,
    pub char_dir_rotated: PyValue,
    pub extra_attrs: PyValue,
    pub split_at_punctuation: PyValue,
    pub expand_ligatures: PyValue,
}

impl Default for WordExtractorArgs {
    // pdfplumber/utils/text.py:424 WordExtractor.__init__ (defaults)
    fn default() -> WordExtractorArgs {
        WordExtractorArgs {
            x_tolerance: PyValue::from(DEFAULT_X_TOLERANCE),
            y_tolerance: PyValue::from(DEFAULT_Y_TOLERANCE),
            x_tolerance_ratio: PyValue::None,
            y_tolerance_ratio: PyValue::None,
            keep_blank_chars: PyValue::from(false),
            use_text_flow: PyValue::from(false),
            vertical_ttb: PyValue::from(true),
            horizontal_ltr: PyValue::from(true),
            line_dir: dir_value(DEFAULT_LINE_DIR),
            char_dir: dir_value(DEFAULT_CHAR_DIR),
            line_dir_rotated: PyValue::None,
            char_dir_rotated: PyValue::None,
            extra_attrs: PyValue::None,
            split_at_punctuation: PyValue::from(false),
            expand_ligatures: PyValue::from(true),
        }
    }
}

// pdfplumber/utils/text.py:702 WORD_EXTRACTOR_KWARGS
pub const WORD_EXTRACTOR_KWARGS: [&str; 15] = [
    "x_tolerance",
    "y_tolerance",
    "x_tolerance_ratio",
    "y_tolerance_ratio",
    "keep_blank_chars",
    "use_text_flow",
    "vertical_ttb",
    "horizontal_ltr",
    "line_dir",
    "char_dir",
    "line_dir_rotated",
    "char_dir_rotated",
    "extra_attrs",
    "split_at_punctuation",
    "expand_ligatures",
];

impl WordExtractorArgs {
    /// `WordExtractor(**kwargs)`: with `strict`, an unknown keyword raises
    /// `TypeError` as Python's call does; otherwise only the keys of
    /// `WORD_EXTRACTOR_KWARGS` are taken (`{k: kwargs[k] for k in
    /// WORD_EXTRACTOR_KWARGS if k in kwargs}`).
    pub fn from_kwargs(kwargs: &Kwargs, strict: bool) -> PyResult<WordExtractorArgs> {
        let mut a = WordExtractorArgs::default();
        for (k, v) in kwargs.items() {
            let v = v.clone();
            match k.as_str().unwrap_or("") {
                "x_tolerance" => a.x_tolerance = v,
                "y_tolerance" => a.y_tolerance = v,
                "x_tolerance_ratio" => a.x_tolerance_ratio = v,
                "y_tolerance_ratio" => a.y_tolerance_ratio = v,
                "keep_blank_chars" => a.keep_blank_chars = v,
                "use_text_flow" => a.use_text_flow = v,
                "vertical_ttb" => a.vertical_ttb = v,
                "horizontal_ltr" => a.horizontal_ltr = v,
                "line_dir" => a.line_dir = v,
                "char_dir" => a.char_dir = v,
                "line_dir_rotated" => a.line_dir_rotated = v,
                "char_dir_rotated" => a.char_dir_rotated = v,
                "extra_attrs" => a.extra_attrs = v,
                "split_at_punctuation" => a.split_at_punctuation = v,
                "expand_ligatures" => a.expand_ligatures = v,
                _ if strict => {
                    return Err(err_ending_with(
                        ExcKind::TypeError,
                        "WordExtractor.__init__() got an unexpected keyword argument '",
                        k,
                        "'",
                    ))
                }
                _ => {}
            }
        }
        Ok(a)
    }
}

/// The items of `*v` in a call (`itemgetter("upright", *extra_attrs)`):
/// any iterable (a `str` gives its characters, a `dict` its keys).
fn star_items(v: &PyValue) -> PyResult<Vec<PyValue>> {
    match py_iter(v) {
        Some(items) => items,
        None => Err(exc::TypeError(format!("Value after * must be an iterable, not {}", v.type_name()))),
    }
}

/// `obj[key]` for a key of any type: a `str` is looked up (its own
/// `KeyError`), another hashable value is no key of a pdfplumber object
/// (`KeyError(repr(key))`), an unhashable one raises `TypeError`.
pub fn item_any<'a>(obj: &'a Obj, key: &PyValue) -> PyResult<&'a PyValue> {
    match key {
        PyValue::Str(k) => obj.item(k),
        other => {
            check_hashable(other)?;
            Err(exc::KeyError(other.repr()))
        }
    }
}

impl WordExtractor {
    // pdfplumber/utils/text.py:424 WordExtractor.__init__
    pub fn new(a: &WordExtractorArgs) -> PyResult<WordExtractor> {
        // Default is to "flip" the directions for rotated text
        let line_dir_rotated = or_value(&a.line_dir_rotated, &a.char_dir);
        let char_dir_rotated = or_value(&a.char_dir_rotated, &a.line_dir);
        let (line_dir, char_dir) = validate_directions(&a.line_dir, &a.char_dir, "")?;
        let (line_dir_rotated, char_dir_rotated) = validate_directions(line_dir_rotated, char_dir_rotated, "_rotated")?;
        let extra_attrs = if a.extra_attrs.is_none() { PyValue::list([]) } else { a.extra_attrs.clone() };
        // `string.punctuation if split_at_punctuation is True else
        // (split_at_punctuation or "")`
        let split_at_punctuation = match &a.split_at_punctuation {
            PyValue::Bool(true) => PyValue::str(PUNCTUATION),
            v if !v.truthy() => PyValue::str(""),
            v => v.clone(),
        };
        Ok(WordExtractor {
            x_tolerance: a.x_tolerance.clone(),
            y_tolerance: a.y_tolerance.clone(),
            x_tolerance_ratio: a.x_tolerance_ratio.clone(),
            y_tolerance_ratio: a.y_tolerance_ratio.clone(),
            keep_blank_chars: a.keep_blank_chars.truthy(),
            use_text_flow: a.use_text_flow.truthy(),
            horizontal_ltr: a.horizontal_ltr.truthy(),
            vertical_ttb: a.vertical_ttb.truthy(),
            line_dir,
            char_dir,
            line_dir_rotated,
            char_dir_rotated,
            extra_attrs,
            split_at_punctuation,
            expand_ligatures: a.expand_ligatures.truthy(),
        })
    }

    /// `WordExtractor(**kwargs)` (see [`WordExtractorArgs::from_kwargs`]).
    pub fn from_kwargs(kwargs: &Kwargs, strict: bool) -> PyResult<WordExtractor> {
        WordExtractor::new(&WordExtractorArgs::from_kwargs(kwargs, strict)?)
    }

    // pdfplumber/utils/text.py:478 WordExtractor.get_char_dir
    pub fn get_char_dir(&self, upright: &PyValue) -> T_dir {
        let upright = upright.truthy();
        if !upright && !self.vertical_ttb {
            return T_dir::Btt;
        } else if upright && !self.horizontal_ltr {
            return T_dir::Rtl;
        }
        if upright {
            self.char_dir
        } else {
            self.char_dir_rotated
        }
    }

    // pdfplumber/utils/text.py:490 WordExtractor.merge_chars
    pub fn merge_chars(&self, ordered_chars: &[Obj]) -> PyResult<Obj> {
        let [x0, top, x1, bottom] = objects_to_bbox_refs(ordered_chars.iter())?;
        let first = &ordered_chars[0];
        let doctop_adj = first.hot(Hot::Doctop)?.sub(first.hot(Hot::Top)?)?;
        let upright = first.hot(Hot::Upright)?.clone();
        let char_dir = self.get_char_dir(&upright);

        // `"".join(<generator>)`: PySequence_Fast first runs the whole
        // generator (each `c["text"]`, KeyError, then `expansions.get`,
        // which hashes its key even in the empty dict of
        // expand_ligatures=False), and only then checks that every item is
        // a `str` (unicode_join: "sequence item i").
        let mut texts = Vec::with_capacity(ordered_chars.len());
        for c in ordered_chars {
            let t = char_text(c)?;
            check_hashable(t)?;
            texts.push(t);
        }
        let mut text = PyStrBuilder::new();
        for (i, t) in texts.into_iter().enumerate() {
            match t {
                PyValue::Str(s) => match expansion(s, self.expand_ligatures) {
                    Some(e) => text.push_rust_str(e),
                    None => text.push_str(s),
                },
                other => {
                    return Err(exc::TypeError(format!(
                        "sequence item {i}: expected str instance, {} found",
                        other.type_name()
                    )))
                }
            }
        }
        let doctop = top.add(&doctop_adj)?;
        let height = bottom.sub(&top)?;
        let width = x1.sub(&x0)?;
        let mut word = Obj::from_pairs([
            ("text", PyValue::Str(text.finish())),
            ("x0", x0),
            ("x1", x1),
            ("top", top),
            ("doctop", doctop),
            ("bottom", bottom),
            ("upright", upright),
            ("height", height),
            ("width", width),
            ("direction", dir_value(char_dir)),
        ]);
        for key in star_items(&self.extra_attrs)? {
            let value = item_any(first, &key)?.clone();
            let PyValue::Str(key) = key else { unreachable!("only str keys are found") };
            word.set(&key, value);
        }
        Ok(word)
    }

    // pdfplumber/utils/text.py:516 WordExtractor.char_begins_new_word
    pub fn char_begins_new_word(
        &self,
        prev_char: &Obj,
        curr_char: &Obj,
        direction: T_dir,
        x_tolerance: &PyValue,
        y_tolerance: &PyValue,
    ) -> PyResult<bool> {
        let (x, y, ay, cy, ax, bx, cx);
        if matches!(direction, T_dir::Ltr | T_dir::Rtl) {
            x = x_tolerance;
            y = y_tolerance;
            ay = prev_char.hot(Hot::Top)?.clone();
            cy = curr_char.hot(Hot::Top)?.clone();
            if direction == T_dir::Ltr {
                ax = prev_char.hot(Hot::X0)?.clone();
                bx = prev_char.hot(Hot::X1)?.clone();
                cx = curr_char.hot(Hot::X0)?.clone();
            } else {
                ax = prev_char.hot(Hot::X1)?.neg()?;
                bx = prev_char.hot(Hot::X0)?.neg()?;
                cx = curr_char.hot(Hot::X1)?.neg()?;
            }
        } else {
            x = y_tolerance;
            y = x_tolerance;
            ay = prev_char.hot(Hot::X0)?.clone();
            cy = curr_char.hot(Hot::X0)?.clone();
            if direction == T_dir::Ttb {
                ax = prev_char.hot(Hot::Top)?.clone();
                bx = prev_char.hot(Hot::Bottom)?.clone();
                cx = curr_char.hot(Hot::Top)?.clone();
            } else {
                ax = prev_char.hot(Hot::Bottom)?.neg()?;
                bx = prev_char.hot(Hot::Top)?.neg()?;
                cx = curr_char.hot(Hot::Bottom)?.neg()?;
            }
        }
        Ok(cx.lt(&ax)? || cx.gt(&bx.add(x)?)? || cy.sub(&ay)?.abs()?.gt(y)?)
    }

    /// The tolerance of one comparison: `t if ratio is None else ratio *
    /// prev["size"]`.
    fn tolerance(t: &PyValue, ratio: &PyValue, prev: &Obj) -> PyResult<PyValue> {
        if ratio.is_none() {
            Ok(t.clone())
        } else {
            ratio.mul(prev.hot(Hot::Size)?)
        }
    }

    // pdfplumber/utils/text.py:593 WordExtractor.iter_chars_to_words
    /// Words of `ordered_chars`, each handed to `emit` as soon as Python's
    /// generator would yield it (so that errors happen in the same order).
    pub fn iter_chars_to_words(
        &self,
        ordered_chars: impl IntoIterator<Item = PyResult<Obj>>,
        direction: T_dir,
        emit: &mut impl FnMut(Vec<Obj>) -> PyResult<()>,
    ) -> PyResult<()> {
        let mut current_word: Vec<Obj> = Vec::new();
        // start_next_word (nested in iter_chars_to_words)
        fn start_next_word(
            current_word: &mut Vec<Obj>,
            new_char: Option<Obj>,
            emit: &mut impl FnMut(Vec<Obj>) -> PyResult<()>,
        ) -> PyResult<()> {
            let next = new_char.map_or_else(Vec::new, |c| vec![c]);
            let word = std::mem::replace(current_word, next);
            if !word.is_empty() {
                emit(word)?;
            }
            Ok(())
        }

        for char in ordered_chars {
            let char = char?;
            let text = char_text(&char)?;
            let is_blank = if self.keep_blank_chars {
                false
            } else {
                match text {
                    PyValue::Str(s) => s.isspace(),
                    // bytes.isspace()
                    PyValue::Bytes(b) => !b.is_empty() && b.iter().all(|&c| pycompat::text::bytes_isspace(c)),
                    other => {
                        return Err(exc::AttributeError(format!(
                            "'{}' object has no attribute 'isspace'",
                            other.type_name()
                        )))
                    }
                }
            };
            if is_blank {
                start_next_word(&mut current_word, None, emit)?;
                continue;
            }
            let in_punct = py_contains(&self.split_at_punctuation, text)?;
            if in_punct {
                start_next_word(&mut current_word, Some(char), emit)?;
                start_next_word(&mut current_word, None, emit)?;
            } else if let Some(prev) = current_word.last() {
                let xt = WordExtractor::tolerance(&self.x_tolerance, &self.x_tolerance_ratio, prev)?;
                let yt = WordExtractor::tolerance(&self.y_tolerance, &self.y_tolerance_ratio, prev)?;
                if self.char_begins_new_word(prev, &char, direction, &xt, &yt)? {
                    start_next_word(&mut current_word, Some(char), emit)?;
                } else {
                    current_word.push(char);
                }
            } else {
                current_word.push(char);
            }
        }

        // Finally, after all chars processed
        if !current_word.is_empty() {
            emit(current_word)?;
        }
        Ok(())
    }

    // pdfplumber/utils/text.py:641 WordExtractor.iter_chars_to_lines
    pub fn iter_chars_to_lines(&self, chars: Vec<Obj>) -> PyResult<Vec<(Vec<Obj>, T_dir)>> {
        let first = chars.first().ok_or_else(|| exc::IndexError("list index out of range"))?;
        let upright = first.hot(Hot::Upright)?.clone();
        let line_dir = if upright.truthy() { self.line_dir } else { self.line_dir_rotated };
        let char_dir = self.get_char_dir(&upright);

        // Cluster by line
        let tolerance = if matches!(line_dir, T_dir::Ttb | T_dir::Btt) { &self.y_tolerance } else { &self.x_tolerance };
        let subclusters = cluster_objects(&chars, |c| line_cluster_key(line_dir, c), tolerance, false)?;

        let mut out = Vec::with_capacity(subclusters.len());
        for mut sc in subclusters {
            // Sort within line
            sort_by_key(&mut sc, |c| char_sort_key(char_dir, c), pair_lt)?;
            out.push((sc, char_dir));
        }
        Ok(out)
    }

    /// `itemgetter("upright", *self.extra_attrs)(char)`: one value, or a
    /// tuple with extra attributes (`extra` are the items of
    /// `self.extra_attrs`).
    fn grouping_key(c: &Obj, extra: &[PyValue]) -> PyResult<PyValue> {
        let upright = c.hot(Hot::Upright)?.clone();
        if extra.is_empty() {
            return Ok(upright);
        }
        let mut items = vec![upright];
        for k in extra {
            items.push(item_any(c, k)?.clone());
        }
        Ok(PyValue::tuple(items))
    }

    // pdfplumber/utils/text.py:664 WordExtractor.iter_extract_tuples
    /// Each `(word, chars)` handed to `emit` in Python's order (the
    /// `itertools.groupby` keys are computed one char ahead, as CPython's
    /// `groupby` does).
    pub fn iter_extract_tuples(
        &self,
        chars: &[Obj],
        emit: &mut impl FnMut(Obj, Vec<Obj>) -> PyResult<()>,
    ) -> PyResult<()> {
        let mut emit_word = |word_chars: Vec<Obj>| -> PyResult<()> {
            let word = self.merge_chars(&word_chars)?;
            emit(word, word_chars)
        };
        // grouping_key = itemgetter("upright", *self.extra_attrs)
        let extra = star_items(&self.extra_attrs)?;
        let mut i = 0;
        let mut next_key: Option<PyValue> = None;
        while i < chars.len() {
            let group_start = i;
            // groupby: the key of the group's first char
            let tgtkey = match next_key.take() {
                Some(k) => k,
                None => WordExtractor::grouping_key(&chars[i], &extra)?,
            };
            if self.use_text_flow {
                // The group is consumed char by char by iter_chars_to_words:
                // the key of char j + 1 is computed after char j is
                // processed.
                let start = i;
                let mut pending_key: Option<PyValue> = None;
                let mut end_reached = false;
                let iter = std::iter::from_fn(|| {
                    if end_reached {
                        return None;
                    }
                    if i == start {
                        i += 1;
                        return Some(Ok(chars[start].clone()));
                    }
                    if i >= chars.len() {
                        end_reached = true;
                        return None;
                    }
                    let key = match WordExtractor::grouping_key(&chars[i], &extra) {
                        Ok(k) => k,
                        Err(e) => {
                            end_reached = true;
                            return Some(Err(e));
                        }
                    };
                    if key.py_eq(&tgtkey) {
                        i += 1;
                        Some(Ok(chars[i - 1].clone()))
                    } else {
                        pending_key = Some(key);
                        end_reached = true;
                        None
                    }
                });
                self.iter_chars_to_words(iter, self.char_dir, &mut emit_word)?;
                next_key = pending_key;
            } else {
                let start = i;
                i += 1;
                while i < chars.len() {
                    let key = WordExtractor::grouping_key(&chars[i], &extra)?;
                    if !key.py_eq(&tgtkey) {
                        next_key = Some(key);
                        break;
                    }
                    i += 1;
                }
                for (line_chars, direction) in self.iter_chars_to_lines(chars[start..i].to_vec())? {
                    self.iter_chars_to_words(line_chars.into_iter().map(Ok), direction, &mut emit_word)?;
                }
            }
            debug_assert!(i > group_start, "progress guard (debug assertions: the `mutants` profile)");
        }
        Ok(())
    }

    // pdfplumber/utils/text.py:680 WordExtractor.extract_wordmap
    pub fn extract_wordmap(&self, chars: &[Obj]) -> PyResult<WordMap> {
        let mut tuples = Vec::new();
        self.iter_extract_tuples(chars, &mut |w, c| {
            tuples.push((w, c));
            Ok(())
        })?;
        Ok(WordMap::new(tuples))
    }

    // pdfplumber/utils/text.py:683 WordExtractor.extract_words
    pub fn extract_words(&self, chars: &[Obj], return_chars: bool) -> PyResult<Vec<Obj>> {
        let mut words = Vec::new();
        self.iter_extract_tuples(chars, &mut |mut word, word_chars| {
            if return_chars {
                word.set("chars", PyValue::list(word_chars.into_iter().map(PyValue::Dict)));
            }
            words.push(word);
            Ok(())
        })?;
        Ok(words)
    }
}

// -- module functions -----------------------------------------------------------

// pdfplumber/utils/text.py:695 extract_words
pub fn extract_words(chars: &[Obj], return_chars: bool, kwargs: &Kwargs) -> PyResult<Vec<Obj>> {
    WordExtractor::from_kwargs(kwargs, true)?.extract_words(chars, return_chars)
}

/// Binds the named parameters of a call `f(chars, **kwargs)`: removes them
/// from `kwargs` and returns their values (`None` when absent).
fn bind_named(func: &str, kwargs: &Kwargs, names: &[&str]) -> PyResult<(Vec<Option<PyValue>>, Kwargs)> {
    if kwargs.contains("chars") {
        return Err(exc::TypeError(format!("{func}() got multiple values for argument 'chars'")));
    }
    let mut rest = kwargs.clone();
    let mut values = Vec::with_capacity(names.len());
    for name in names {
        values.push(if rest.contains(name) { Some(rest.remove(name)?) } else { None });
    }
    Ok((values, rest))
}

/// `extract_words(chars, **kwargs)` (`return_chars` may be in `kwargs`).
pub fn extract_words_kwargs(chars: &[Obj], kwargs: &Kwargs) -> PyResult<Vec<Obj>> {
    let (values, rest) = bind_named("extract_words", kwargs, &["return_chars"])?;
    let return_chars = values[0].as_ref().is_some_and(PyValue::truthy);
    extract_words(chars, return_chars, &rest)
}

/// `extract_text(chars, **kwargs)` (`line_dir_render`, `char_dir_render`
/// may be in `kwargs`).
pub fn extract_text_kwargs(chars: &[Obj], kwargs: &Kwargs) -> PyResult<PyStr> {
    let (values, rest) = bind_named("extract_text", kwargs, &["line_dir_render", "char_dir_render"])?;
    let none = PyValue::None;
    extract_text(chars, values[0].as_ref().unwrap_or(&none), values[1].as_ref().unwrap_or(&none), &rest)
}

// pdfplumber/utils/text.py:705 chars_to_textmap
pub fn chars_to_textmap(chars: &[Obj], kwargs: &Kwargs) -> PyResult<TextMap> {
    if kwargs.contains("chars") {
        return Err(exc::TypeError("chars_to_textmap() got multiple values for argument 'chars'"));
    }
    let mut kwargs = kwargs.clone();
    let layout_bbox = match kwargs.get("layout_bbox") {
        Some(b) if b.truthy() => b.clone(),
        _ => crate::obj::bbox_value(&objects_to_bbox_refs(chars.iter())?),
    };
    kwargs.set("presorted", PyValue::from(true));
    kwargs.set("layout_bbox", layout_bbox);

    let extractor = WordExtractor::from_kwargs(&kwargs, false)?;
    let wordmap = extractor.extract_wordmap(chars)?;
    wordmap.to_textmap(&TextMapOptions::from_kwargs(&kwargs, false)?)
}

// pdfplumber/utils/text.py:723 extract_text
pub fn extract_text(
    chars: &[Obj],
    line_dir_render: &PyValue,
    char_dir_render: &PyValue,
    kwargs: &Kwargs,
) -> PyResult<PyStr> {
    let chars = &crate::utils::generic::to_list(chars)[..];
    if chars.is_empty() {
        return Ok(PyStr::empty());
    }

    if kwargs.get("layout").is_some_and(PyValue::truthy) {
        let mut textmap_kwargs = kwargs.clone();
        textmap_kwargs.set("line_dir_render", line_dir_render.clone());
        textmap_kwargs.set("char_dir_render", char_dir_render.clone());
        Ok(chars_to_textmap(chars, &textmap_kwargs)?.as_string)
    } else {
        let extractor = WordExtractor::from_kwargs(kwargs, false)?;
        let words = extractor.extract_words(chars, false)?;

        let ld_value = dir_value(extractor.line_dir);
        let cd_value = dir_value(extractor.char_dir);
        let line_dir_render = or_value(line_dir_render, &ld_value).clone();
        let char_dir_render = or_value(char_dir_render, &cd_value).clone();

        let line_dir = extractor.line_dir;

        let x_tolerance = kwargs.get("x_tolerance").cloned().unwrap_or(PyValue::from(DEFAULT_X_TOLERANCE));
        let y_tolerance = kwargs.get("y_tolerance").cloned().unwrap_or(PyValue::from(DEFAULT_Y_TOLERANCE));

        let vertical = ["ttb", "btt"].iter().any(|d| line_dir_render.py_eq(&PyValue::str(d)));
        let lines = cluster_objects(
            &words,
            |w| line_cluster_key(line_dir, w),
            if vertical { &y_tolerance } else { &x_tolerance },
            false,
        )?;

        let mut text = PyStrBuilder::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                text.push(NEWLINE);
            }
            for (j, word) in line.iter().enumerate() {
                if j > 0 {
                    text.push(SPACE);
                }
                text.push_str(word.hot(Hot::Text)?.as_pystr().expect("words have str text"));
            }
        }
        let tuples = text.finish().code_points().map(|c| (c, None)).collect();
        Ok(TextMap::new(tuples, &line_dir_render, &char_dir_render)?.as_string)
    }
}

// pdfplumber/utils/text.py:771 collate_line
pub fn collate_line(line_chars: &[Obj], tolerance: &PyValue) -> PyResult<PyStr> {
    let mut coll = PyStrBuilder::new();
    let mut last_x1: Option<PyValue> = None;
    let mut sorted = line_chars.to_vec();
    sort_by_key(&mut sorted, |c| Ok(c.hot(Hot::X0)?.clone()), |a, b| a.lt(b))?;
    for char in &sorted {
        if let Some(last) = &last_x1 {
            if char.hot(Hot::X0)?.gt(&last.add(tolerance)?)? {
                coll.push(SPACE);
            }
        }
        last_x1 = Some(char.hot(Hot::X1)?.clone());
        match char.hot(Hot::Text)? {
            PyValue::Str(s) => coll.push_str(s),
            other => {
                return Err(exc::TypeError(format!("can only concatenate str (not \"{}\") to str", other.type_name())))
            }
        }
    }
    Ok(coll.finish())
}

// pdfplumber/utils/text.py:785 extract_text_simple
pub fn extract_text_simple(chars: &[Obj], x_tolerance: &PyValue, y_tolerance: &PyValue) -> PyResult<PyStr> {
    let clustered = cluster_objects(chars, |c| Ok(c.hot(Hot::Doctop)?.clone()), y_tolerance, false)?;
    let mut out = PyStrBuilder::new();
    for (i, c) in clustered.iter().enumerate() {
        if i > 0 {
            out.push(NEWLINE);
        }
        out.push_str(&collate_line(c, x_tolerance)?);
    }
    Ok(out.finish())
}

// pdfplumber/utils/text.py:794 dedupe_chars
/// `extra_attrs`: the value given (upstream's default is `("fontname",
/// "size")`; a false value, `None` included, for no extra attribute).
pub fn dedupe_chars(chars: &[Obj], tolerance: &PyValue, extra_attrs: &PyValue) -> PyResult<Vec<Obj>> {
    // key = itemgetter(*("upright", "text"), *(extra_attrs or tuple()))
    let extra = if extra_attrs.truthy() { star_items(extra_attrs)? } else { Vec::new() };
    let key = |c: &Obj| -> PyResult<PyValue> {
        let mut items = vec![c.hot(Hot::Upright)?.clone(), c.hot(Hot::Text)?.clone()];
        for k in &extra {
            items.push(item_any(c, k)?.clone());
        }
        Ok(PyValue::tuple(items))
    };
    // pos_key = itemgetter("doctop", "x0")
    let pos_key = |c: &Obj| -> PyResult<PairKey> { Ok((c.hot(Hot::Doctop)?.clone(), c.hot(Hot::X0)?.clone())) };

    // yield_unique_chars (nested in dedupe_chars), with each char's position
    let mut indexed: Vec<(usize, Obj)> = chars.iter().cloned().enumerate().collect();
    sort_by_key(&mut indexed, |(_, c)| key(c), |a, b| a.lt(b))?;
    let mut deduped: Vec<(usize, Obj)> = Vec::new();
    let mut g = 0;
    while g < indexed.len() {
        // itertools.groupby(sorted_chars, key=key)
        let k = key(&indexed[g].1)?;
        let mut end = g + 1;
        while end < indexed.len() && key(&indexed[end].1)?.py_eq(&k) {
            end += 1;
        }
        let grp_chars = &indexed[g..end];
        for y_cluster in cluster_objects(grp_chars, |(_, c)| Ok(c.hot(Hot::Doctop)?.clone()), tolerance, false)? {
            for mut x_cluster in cluster_objects(&y_cluster, |(_, c)| Ok(c.hot(Hot::X0)?.clone()), tolerance, false)? {
                sort_by_key(&mut x_cluster, |(_, c)| pos_key(c), pair_lt)?;
                deduped.push(x_cluster.swap_remove(0));
            }
        }
        debug_assert!(end > g, "progress guard (debug assertions: the `mutants` profile)");
        g = end;
    }
    // sorted(deduped, key=chars.index): the first char equal to each one
    let index_of = IndexOf::new(chars);
    let mut keyed = Vec::with_capacity(deduped.len());
    for (pos, c) in deduped {
        keyed.push((index_of.index(pos), c));
    }
    sort_by_key(&mut keyed, |(i, _)| Ok(*i), |a, b| Ok(a < b))?;
    Ok(keyed.into_iter().map(|(_, c)| c).collect())
}

/// `chars.index(c)` for `c = chars[pos]`: the first position holding a
/// dict equal to `c` (`list.index` compares with `==`, after an identity
/// check that makes `pos` itself match). Equal dicts have equal `text`,
/// `doctop` and `x0` (every char has them here: the sort keys read them),
/// so only the chars of the same `(text, doctop, x0)` bucket are compared.
struct IndexOf<'a> {
    chars: &'a [Obj],
    buckets: Option<HashMap<[HashKey; 3], Vec<usize>>>,
}

impl<'a> IndexOf<'a> {
    fn new(chars: &'a [Obj]) -> IndexOf<'a> {
        let mut buckets: HashMap<[HashKey; 3], Vec<usize>> = HashMap::default();
        for (i, c) in chars.iter().enumerate() {
            match IndexOf::bucket(c) {
                Some(b) => buckets.entry(b).or_default().push(i),
                None => return IndexOf { chars, buckets: None },
            }
        }
        IndexOf { chars, buckets: Some(buckets) }
    }

    fn bucket(c: &Obj) -> Option<[HashKey; 3]> {
        let h = |k: Hot| c.get_hot(k).and_then(|v| HashKey::new(v.clone()).ok());
        Some([h(Hot::Text)?, h(Hot::Doctop)?, h(Hot::X0)?])
    }

    fn index(&self, pos: usize) -> usize {
        let c = &self.chars[pos];
        let candidates: Box<dyn Iterator<Item = usize> + '_> = match (&self.buckets, IndexOf::bucket(c)) {
            (Some(buckets), Some(b)) => Box::new(buckets.get(&b).into_iter().flatten().copied()),
            _ => Box::new(0..pos),
        };
        candidates.take_while(|&i| i < pos).find(|&i| self.chars[i].py_eq(c)).unwrap_or(pos)
    }
}
