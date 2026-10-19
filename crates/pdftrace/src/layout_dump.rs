//! The lines of the `layout` stage (`oracle/stages/layout.py`) for a page
//! of pdfplumber's device (`pdfplumber::page::PlumberPayload`): the raw
//! page (`_RawDump`), the analyzed page (`_analyzed`) and the `id()`
//! ranking of its analysis (`id_ranks`); and [`RecordedIds`], the
//! [`IdSource`] that gives the boxes and groups of an analysis the `id()`
//! order Python's had. Shared by the stage (`stages::layout`) and the
//! replay of `oracle/unit/converter.py` (`tests/converter_unit.rs`);
//! `tests/layout_unit.rs` uses [`RecordedIds`].
//!
//! Every function here is the harness's, not a port: they take no Python
//! frame (the oracle's dumps run far from the recursion limit) and walk the
//! trees without recursion.

use std::collections::HashMap;
use std::rc::Rc;

use pdfminer::layout::{
    IdSource, LTChar, LTComponent, LTCurve, LTImage, LTItem, LTPage, LTPayload, LTTextBox, LTTextLine,
    TextGroupElement, TextLineElement, TextLineKind,
};
use pdfminer::pdfcolor::PDFColorSpace;
use pdfminer::pdffont::FontName;
use pdfminer::pdfinterp::{Color, PDFGraphicState, StandardColor};
use pdfminer::pdftypes::PDFObject;
use pdfminer::utils::Point;
use pdfplumber::page::{MarkedContentAttr, PlumberPayload};
use pycompat::{Num, PyInt, PyResult, PyStr};

use crate::canon::canon;
use crate::value::Value;

type P = PlumberPayload;

/// The identity of an `Rc` (Python's `id()` for the dumps).
pub fn ptr<T: ?Sized>(rc: &Rc<T>) -> usize {
    Rc::as_ptr(rc) as *const () as usize
}

/// The identity of a layout object.
pub fn item_ptr<Q: LTPayload>(item: &LTItem<Q>) -> usize {
    match item {
        LTItem::Char(c) => ptr(c),
        LTItem::Curve(c) => ptr(c),
        LTItem::Image(i) => ptr(i),
        LTItem::Figure(f) => ptr(f),
        LTItem::TextLine(l) => ptr(l),
        LTItem::TextBox(b) => ptr(b),
    }
}

/// `line(label, *values)`: the label and the canonical texts of the
/// values, separated by spaces.
pub fn line(label: &str, values: &[Value]) -> String {
    let mut out = label.to_owned();
    for v in values {
        out.push(' ');
        out.push_str(&canon(v));
    }
    out
}

fn s(k: &str) -> Value {
    Value::str(k)
}

fn int(i: i64) -> Value {
    Value::Int(PyInt::Small(i))
}

fn named(name: &str, fields: Vec<(&str, Value)>) -> Value {
    Value::Named { name: name.to_owned(), fields: fields.into_iter().map(|(k, v)| (s(k), v)).collect() }
}

/// A number.
pub fn num(n: &Num) -> Value {
    match n {
        Num::Int(i) => Value::Int(i.clone()),
        Num::Float(f) => Value::Float(*f),
    }
}

/// A tuple of numbers.
pub fn tuple(ns: &[Num]) -> Value {
    Value::Tuple(ns.iter().map(num).collect())
}

fn obj(o: &PDFObject) -> Value {
    Value::from(o)
}

/// `PDFColorSpace{"name": .., "ncomponents": ..}` (`interp.colorspace`).
pub fn colorspace(cs: &PDFColorSpace) -> Value {
    named("PDFColorSpace", vec![("name", Value::str(cs.name.clone())), ("ncomponents", obj(&cs.ncomponents))])
}

fn standard_color(c: &StandardColor) -> Value {
    match c {
        StandardColor::Gray(g) => num(g),
        StandardColor::Rgb(rgb) => Value::Tuple(rgb.iter().map(|f| Value::Float(*f)).collect()),
        StandardColor::Cmyk(cmyk) => Value::Tuple(cmyk.iter().map(|f| Value::Float(*f)).collect()),
    }
}

/// A color as Python holds it (a number, a tuple, a pattern name, or
/// `(base_color, pattern_name)`).
pub fn color(c: &Color) -> Value {
    match c {
        Color::Standard(c) => standard_color(c),
        Color::Pattern(name) => Value::str(name.clone()),
        Color::UncoloredPattern(c, name) => Value::Tuple(vec![standard_color(c), Value::str(name.clone())]),
    }
}

/// `Named("PDFGraphicState", state_fields(vars(gs)))` (`interp.state_fields`:
/// the color spaces as values).
pub fn graphic_state(gs: &PDFGraphicState) -> Value {
    named(
        "PDFGraphicState",
        vec![
            ("linewidth", num(&gs.linewidth)),
            ("linecap", obj(&gs.linecap)),
            ("linejoin", obj(&gs.linejoin)),
            ("miterlimit", obj(&gs.miterlimit)),
            ("dash", gs.dash.as_ref().map_or(Value::None, |(d, p)| Value::Tuple(vec![obj(d), obj(p)]))),
            ("intent", obj(&gs.intent)),
            ("flatness", obj(&gs.flatness)),
            ("scolor", color(&gs.scolor)),
            ("scs", colorspace(&gs.scs)),
            ("ncolor", color(&gs.ncolor)),
            ("ncs", colorspace(&gs.ncs)),
        ],
    )
}

/// `LTChar.fontname`.
pub fn fontname(f: &FontName) -> Value {
    match f {
        FontName::Str(s) => Value::str(s.clone()),
        FontName::Obj(o) => obj(o),
    }
}

/// The value of `mcid` or `tag`.
pub fn attr(a: &MarkedContentAttr) -> Value {
    match a {
        MarkedContentAttr::Mcid(o) => obj(o),
        MarkedContentAttr::Tag(t) => t.as_ref().map_or(Value::None, |t| Value::str(t.clone())),
    }
}

fn comp_fields(c: &LTComponent) -> Vec<(&'static str, Value)> {
    vec![
        ("x0", num(&c.x0)),
        ("y0", num(&c.y0)),
        ("x1", num(&c.x1)),
        ("y1", num(&c.y1)),
        ("width", num(&c.width)),
        ("height", num(&c.height)),
    ]
}

fn point(p: &Point) -> Value {
    tuple(p)
}

fn text_result(r: PyResult<PyStr>) -> Value {
    match r {
        Ok(t) => Value::py_str(&t),
        Err(e) => Value::from(&e),
    }
}

/// `(obj, depth)` of the raw objects of a container, depth-first, figures
/// before their contents (`_raw_objects`).
pub fn raw_objects<Q: LTPayload>(objs: &[LTItem<Q>]) -> Vec<(LTItem<Q>, i64)> {
    let mut out = Vec::new();
    let mut stack: Vec<(LTItem<Q>, i64)> = objs.iter().rev().map(|o| (o.clone(), 0)).collect();
    while let Some((o, depth)) = stack.pop() {
        if let LTItem::Figure(f) = &o {
            stack.extend(f.borrow().layout._objs.iter().rev().map(|c| (c.clone(), depth + 1)));
        }
        out.push((o, depth));
    }
    out
}

/// `_RawDump`: the raw objects of a page, their raw index, the inline
/// images' numbers, and the `raw` lines.
pub struct RawDump {
    pub objects: Vec<(LTItem<P>, i64)>,
    /// raw index by object identity
    pub index: HashMap<usize, i64>,
    /// an inline image's name -> its number
    inline: HashMap<String, i64>,
    /// the canonical text of each shared value -> its number, for `gstate`
    /// and `colorspace`
    gstates: HashMap<String, i64>,
    colorspaces: HashMap<String, i64>,
    keys: HashMap<&'static str, Vec<String>>,
    pub lines: Vec<String>,
}

impl RawDump {
    /// The dump of the raw page `objs`, inline images being the images
    /// whose name `is_inline` (Python: `name == str(id(stream))`).
    pub fn new(objs: &[LTItem<P>], is_inline: &dyn Fn(&str) -> bool) -> RawDump {
        let objects = raw_objects(objs);
        let index = objects.iter().enumerate().map(|(i, (o, _))| (item_ptr(o), i as i64)).collect();
        let mut inline = HashMap::new();
        for (o, _) in &objects {
            if let LTItem::Image(img) = o {
                if is_inline(&img.name) && !inline.contains_key(&img.name) {
                    let n = inline.len() as i64;
                    inline.insert(img.name.clone(), n);
                }
            }
        }
        RawDump {
            objects,
            index,
            inline,
            gstates: HashMap::new(),
            colorspaces: HashMap::new(),
            keys: HashMap::new(),
            lines: Vec::new(),
        }
    }

    fn inline_name(&self, n: i64) -> Value {
        named("inline", vec![("n", int(n))])
    }

    /// `self.name(obj, name)`
    fn image_name(&self, img: &LTImage<P>) -> Value {
        match self.inline.get(&img.name) {
            Some(n) => self.inline_name(*n),
            None => Value::str(img.name.clone()),
        }
    }

    fn figure_name(&self, fig: &pdfminer::layout::LTFigure<P>) -> Value {
        if let Some(n) = self.inline.get(&fig.name) {
            let has_image = fig.layout._objs.iter().any(|c| matches!(c, LTItem::Image(img) if img.name == fig.name));
            if has_image {
                return self.inline_name(*n);
            }
        }
        Value::str(fig.name.clone())
    }

    /// `self.ref(kind, value, short)`
    fn shared(&mut self, gstate: bool, value: Value) -> Value {
        let text = canon(&value);
        let (numbers, kind, short) =
            if gstate { (&mut self.gstates, "gstate", "gs") } else { (&mut self.colorspaces, "colorspace", "cs") };
        let k = match numbers.get(&text) {
            Some(k) => *k,
            None => {
                let k = numbers.len() as i64;
                numbers.insert(text.clone(), k);
                self.lines.push(format!("{kind} {k} {text}"));
                k
            }
        };
        named(short, vec![("n", int(k))])
    }

    /// `self.values(obj)`: the attribute names (`bbox` left out) and
    /// values.
    fn values(&mut self, item: &LTItem<P>) -> (Vec<String>, Vec<Value>) {
        let mut f: Vec<(&'static str, Value)> = Vec::new();
        let extra: Vec<(&'static str, MarkedContentAttr)> = match item {
            LTItem::Char(c) => {
                let ch: &LTChar<P> = c;
                f.push(("_text", Value::py_str(&ch._text)));
                f.push(("matrix", tuple(&ch.matrix)));
                f.push(("fontname", fontname(&ch.fontname)));
                let ncs = self.shared(false, colorspace(&ch.ncs));
                f.push(("ncs", ncs));
                let gs = self.shared(true, graphic_state(&ch.graphicstate));
                f.push(("graphicstate", gs));
                f.push(("adv", num(&ch.adv)));
                f.push(("upright", Value::Bool(ch.upright)));
                f.extend(comp_fields(&ch.comp));
                f.push(("size", num(&ch.size)));
                ch.attrs.items()
            }
            LTItem::Curve(c) => {
                let cv: &LTCurve<P> = c;
                f.extend(comp_fields(&cv.comp));
                f.push(("pts", Value::List(cv.pts.iter().map(point).collect())));
                f.push(("linewidth", num(&cv.linewidth)));
                f.push(("stroke", Value::Bool(cv.stroke)));
                f.push(("fill", Value::Bool(cv.fill)));
                f.push(("evenodd", Value::Bool(cv.evenodd)));
                f.push(("stroking_color", cv.stroking_color.as_ref().map_or(Value::None, color)));
                f.push(("non_stroking_color", cv.non_stroking_color.as_ref().map_or(Value::None, color)));
                f.push((
                    "original_path",
                    match &cv.original_path {
                        None => Value::None,
                        Some(path) => Value::List(
                            path.iter()
                                .map(|(op, pts)| {
                                    let mut t = vec![Value::str(op.to_string())];
                                    t.extend(pts.iter().map(point));
                                    Value::Tuple(t)
                                })
                                .collect(),
                        ),
                    },
                ));
                f.push((
                    "dashing_style",
                    cv.dashing_style.as_ref().map_or(Value::None, |(d, p)| Value::Tuple(vec![obj(d), obj(p)])),
                ));
                cv.attrs.items()
            }
            LTItem::Image(i) => {
                f.extend(comp_fields(&i.comp));
                f.push(("name", self.image_name(i)));
                f.push(("stream", obj(&PDFObject::Stream(i.stream.clone()))));
                f.push(("srcsize", Value::Tuple(vec![obj(&i.srcsize.0), obj(&i.srcsize.1)])));
                f.push(("imagemask", obj(&i.imagemask)));
                f.push(("bits", obj(&i.bits)));
                f.push(("colorspace", obj(&i.colorspace)));
                i.attrs.items()
            }
            LTItem::Figure(fig) => {
                let fig = fig.borrow();
                f.push(("name", self.figure_name(&fig)));
                f.push(("matrix", tuple(&fig.matrix)));
                f.extend(comp_fields(&fig.layout.comp));
                f.push(("_objs", int(fig.layout._objs.len() as i64)));
                f.push(("groups", Value::None));
                fig.attrs.items()
            }
            LTItem::TextLine(_) | LTItem::TextBox(_) => panic!("a raw page holds no text line or box"),
        };
        for (k, v) in &extra {
            f.push((k, attr(v)));
        }
        let keys = f.iter().map(|(k, _)| (*k).to_owned()).collect();
        (keys, f.into_iter().map(|(_, v)| v).collect())
    }

    /// `write(page)`: the `raw` lines of the page and its objects (with the
    /// `gstate`, `colorspace` and `class` lines they need).
    pub fn write(mut self, page: &LTPage<P>) -> Vec<String> {
        let mut f = comp_fields(&page.layout.comp);
        f.push(("_objs", int(page.layout._objs.len() as i64)));
        f.push(("groups", Value::None));
        f.push(("pageid", Value::Int(page.pageid.clone())));
        f.push(("rotate", num(&page.rotate)));
        self.lines.push(line("raw", &[named("LTPage", f)]));
        let objects = std::mem::take(&mut self.objects);
        for (i, (o, depth)) in objects.iter().enumerate() {
            let cls = o.class_name();
            let (keys, values) = self.values(o);
            if self.keys.get(cls) != Some(&keys) {
                self.lines.push(line("class", &[s(cls), Value::List(keys.iter().map(|k| s(k)).collect())]));
                self.keys.insert(cls, keys);
            }
            self.lines.push(line("raw", &[int(i as i64), int(*depth), s(cls), Value::Tuple(values)]));
        }
        self.objects = objects;
        self.lines
    }
}

/// The heights of the group trees (a box is 0), by identity.
fn heights(groups: &[TextGroupElement<P>]) -> HashMap<usize, i64> {
    let mut out = HashMap::new();
    let mut stack: Vec<(TextGroupElement<P>, bool)> = groups.iter().map(|g| (g.clone(), false)).collect();
    while let Some((g, done)) = stack.pop() {
        match &g {
            TextGroupElement::Box(b) => {
                out.insert(ptr(b), 0);
            }
            TextGroupElement::Group(grp) => {
                if done {
                    let h = 1 + grp.borrow()._objs.iter().map(|c| out[&c.plane_id_dump()]).max().unwrap_or(0);
                    out.insert(ptr(grp), h);
                } else {
                    stack.push((g.clone(), true));
                    stack.extend(grp.borrow()._objs.iter().map(|c| (c.clone(), false)));
                }
            }
        }
    }
    out
}

/// The identity of a group element.
trait PlaneIdDump {
    fn plane_id_dump(&self) -> usize;
}

impl<Q: LTPayload> PlaneIdDump for TextGroupElement<Q> {
    fn plane_id_dump(&self) -> usize {
        match self {
            TextGroupElement::Box(b) => ptr(b),
            TextGroupElement::Group(g) => ptr(g),
        }
    }
}

fn raw_ref(raw: &RawDump, id: usize) -> Value {
    named("#", vec![("i", int(raw.index[&id]))])
}

fn text_line(l: &LTTextLine<P>, raw: &RawDump) -> Value {
    let mut f = comp_fields(&l.comp);
    let objs = l
        ._objs
        .iter()
        .map(|o| match o {
            TextLineElement::Char(c) => raw_ref(raw, ptr(c)),
            TextLineElement::Anno(a) => named("LTAnno", vec![("_text", Value::py_str(&a._text))]),
        })
        .collect();
    f.push(("_objs", Value::List(objs)));
    f.push(("word_margin", num(&l.word_margin)));
    match &l.kind {
        TextLineKind::Horizontal { _x1 } => f.push(("_x1", num(_x1))),
        TextLineKind::Vertical { _y0 } => f.push(("_y0", num(_y0))),
    }
    f.push(("get_text()", text_result(l.get_text())));
    named(l.class_name(), f)
}

fn text_box(b: &LTTextBox<P>) -> Value {
    let mut f = comp_fields(&b.comp);
    f.push(("_objs", int(b._objs.len() as i64)));
    f.push(("index", int(b.index)));
    f.push(("get_text()", text_result(b.get_text())));
    named(b.class_name(), f)
}

/// `_analyzed(page, raw)`: the `analyzed`, `obj` and `group` lines.
pub fn analyzed(page: &LTPage<P>, raw: &RawDump) -> Vec<String> {
    let mut out = Vec::new();
    let objs = &page.layout._objs;
    let positions: HashMap<usize, i64> = objs.iter().enumerate().map(|(i, o)| (item_ptr(o), i as i64)).collect();
    let mut f = comp_fields(&page.layout.comp);
    f.push(("_objs", int(objs.len() as i64)));
    f.push(("groups", page.layout.groups.as_ref().map_or(Value::None, |g| int(g.len() as i64))));
    f.push(("pageid", Value::Int(page.pageid.clone())));
    f.push(("rotate", num(&page.rotate)));
    out.push(line("analyzed", &[named("LTPage", f)]));
    for (i, o) in objs.iter().enumerate() {
        let i = int(i as i64);
        if raw.index.contains_key(&item_ptr(o)) {
            out.push(line("obj", &[Value::Tuple(vec![i]), raw_ref(raw, item_ptr(o))]));
            continue;
        }
        match o {
            LTItem::TextLine(l) => out.push(line("obj", &[Value::Tuple(vec![i]), text_line(&l.borrow(), raw)])),
            LTItem::TextBox(b) => {
                let b = b.borrow();
                out.push(line("obj", &[Value::Tuple(vec![i.clone()]), text_box(&b)]));
                for (j, tl) in b._objs.iter().enumerate() {
                    out.push(line(
                        "obj",
                        &[Value::Tuple(vec![i.clone(), int(j as i64)]), text_line(&tl.borrow(), raw)],
                    ));
                }
            }
            other => panic!("an analyzed page holds a {} that is not raw", other.class_name()),
        }
    }
    if let Some(groups) = &page.layout.groups {
        let heights = heights(groups);
        let mut stack: Vec<(TextGroupElement<P>, Vec<i64>)> =
            groups.iter().enumerate().rev().map(|(i, g)| (g.clone(), vec![i as i64])).collect();
        while let Some((g, path)) = stack.pop() {
            let path_value = Value::Tuple(path.iter().map(|i| int(*i)).collect());
            match &g {
                TextGroupElement::Box(b) => {
                    out.push(line("group", &[path_value, named("box#", vec![("i", int(positions[&ptr(b)]))])]));
                }
                TextGroupElement::Group(grp) => {
                    let gr = grp.borrow();
                    let mut f = comp_fields(&gr.comp);
                    f.push(("_objs", int(gr._objs.len() as i64)));
                    let name = gr.class_name();
                    let children: Vec<TextGroupElement<P>> = gr._objs.clone();
                    drop(gr);
                    if heights[&ptr(grp)] <= GROUP_TEXT_HEIGHT {
                        f.push(("get_text()", text_result(g.get_text())));
                    }
                    out.push(line("group", &[path_value, named(name, f)]));
                    for (j, c) in children.into_iter().enumerate().rev() {
                        let mut p = path.clone();
                        p.push(j as i64);
                        stack.push((c, p));
                    }
                }
            }
        }
    }
    out
}

/// `GROUP_TEXT_HEIGHT` of the stage: the highest group whose `get_text()`
/// is written.
pub const GROUP_TEXT_HEIGHT: i64 = 100;

/// The key of a box or group (`id_ranks`): the smallest raw index of its
/// chars and its number of boxes.
pub fn element_key<Q: LTPayload>(obj: &TextGroupElement<Q>, index: &HashMap<usize, i64>) -> (i64, i64) {
    // post-order without recursion
    let mut keys: HashMap<usize, (i64, i64)> = HashMap::new();
    let mut stack: Vec<(TextGroupElement<Q>, bool)> = vec![(obj.clone(), false)];
    while let Some((g, done)) = stack.pop() {
        match &g {
            TextGroupElement::Box(b) => {
                let min = b
                    .borrow()
                    ._objs
                    .iter()
                    .flat_map(|l| {
                        l.borrow()
                            ._objs
                            .iter()
                            .filter_map(|o| match o {
                                TextLineElement::Char(c) => Some(index[&ptr(c)]),
                                TextLineElement::Anno(_) => None,
                            })
                            .collect::<Vec<_>>()
                    })
                    .min()
                    .expect("a box has chars");
                keys.insert(ptr(b), (min, 1));
            }
            TextGroupElement::Group(grp) => {
                if done {
                    let ks: Vec<(i64, i64)> = grp.borrow()._objs.iter().map(|c| keys[&c.plane_id_dump()]).collect();
                    let key = (ks.iter().map(|k| k.0).min().expect("children"), ks.iter().map(|k| k.1).sum());
                    keys.insert(ptr(grp), key);
                } else {
                    stack.push((g.clone(), true));
                    stack.extend(grp.borrow()._objs.iter().map(|c| (c.clone(), false)));
                }
            }
        }
    }
    keys[&obj.plane_id_dump()]
}

/// `id_ranks(page, index)`: `[min_char, n_boxes, rank]` of every box and
/// group in the groups of the page and of its figures, ranked by the `id()`
/// that `ids` gives them (the ids of the analysis).
pub fn id_ranks<Q: LTPayload>(
    page: &LTPage<Q>,
    index: &HashMap<usize, i64>,
    ids: &mut dyn IdSource<Q>,
) -> Vec<[i64; 3]> {
    // the page, then its figures depth first (`containers[:0] = ...`)
    let mut roots: Vec<TextGroupElement<Q>> = page.layout.groups.clone().unwrap_or_default();
    let mut containers: Vec<LTItem<Q>> =
        page.layout._objs.iter().filter(|o| matches!(o, LTItem::Figure(_))).cloned().collect();
    while !containers.is_empty() {
        let c = containers.remove(0);
        let LTItem::Figure(fig) = &c else { unreachable!() };
        let fig = fig.borrow();
        let figures: Vec<LTItem<Q>> =
            fig.layout._objs.iter().filter(|o| matches!(o, LTItem::Figure(_))).cloned().collect();
        containers.splice(0..0, figures);
        roots.extend(fig.layout.groups.clone().unwrap_or_default());
    }
    let mut found: Vec<(u64, i64, i64)> = Vec::new();
    for root in &roots {
        // post-order, children in order
        let mut stack: Vec<(TextGroupElement<Q>, bool)> = vec![(root.clone(), false)];
        while let Some((g, done)) = stack.pop() {
            match &g {
                TextGroupElement::Box(_) => {
                    let (m, n) = element_key(&g, index);
                    found.push((ids.id(&g), m, n));
                }
                TextGroupElement::Group(grp) => {
                    if done {
                        let (m, n) = element_key(&g, index);
                        found.push((ids.id(&g), m, n));
                    } else {
                        stack.push((g.clone(), true));
                        stack.extend(grp.borrow()._objs.iter().rev().map(|c| (c.clone(), false)));
                    }
                }
            }
        }
    }
    found.sort();
    found.into_iter().enumerate().map(|(rank, (_, m, n))| [m, n, rank as i64]).collect()
}

/// The `ids` line.
pub fn ids_line(ranks: &[[i64; 3]]) -> String {
    let list = Value::List(ranks.iter().map(|r| Value::List(r.iter().map(|x| int(*x)).collect())).collect());
    line("ids", &[list])
}

/// The `id()` order Python's boxes and groups had, by their smallest char
/// index and number of boxes (the `ids` of the oracle: `[min_char, n_boxes,
/// rank]`). A box or group that Python's ranking does not have (the
/// analysis built another one) gets an id after every recorded one, in the
/// order it is first asked for: the result then differs from Python's, at
/// the latest on the `ids` line.
pub struct RecordedIds {
    ranks: HashMap<(i64, i64), u64>,
    /// the raw index of each char, by identity
    index: HashMap<usize, i64>,
    /// the ids already given, by object (the contents of a box or group
    /// never change once it is made)
    given: HashMap<usize, u64>,
    next: u64,
}

impl RecordedIds {
    /// The ids of `ranks`, for the chars numbered by `index`.
    pub fn new(ranks: &[[i64; 3]], index: HashMap<usize, i64>) -> RecordedIds {
        let next = ranks.iter().map(|r| r[2] as u64 + 1).max().unwrap_or(0);
        RecordedIds {
            ranks: ranks.iter().map(|r| ((r[0], r[1]), r[2] as u64)).collect(),
            index,
            given: HashMap::new(),
            next,
        }
    }
}

impl<Q: LTPayload> IdSource<Q> for RecordedIds {
    fn id(&mut self, obj: &TextGroupElement<Q>) -> u64 {
        let key = obj.plane_id_dump();
        if let Some(id) = self.given.get(&key) {
            return *id;
        }
        let id = match self.ranks.get(&element_key(obj, &self.index)) {
            Some(id) => *id,
            None => {
                self.next += 1;
                self.next - 1
            }
        };
        self.given.insert(key, id);
        id
    }
}

/// The raw index of the chars of a raw page (what [`RecordedIds`] keys
/// by), by identity.
pub fn raw_index<Q: LTPayload>(objs: &[LTItem<Q>]) -> HashMap<usize, i64> {
    raw_objects(objs).iter().enumerate().map(|(i, (o, _))| (item_ptr(o), i as i64)).collect()
}
