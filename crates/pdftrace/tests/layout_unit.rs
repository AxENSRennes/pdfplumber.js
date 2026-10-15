//! Replays the cases of `oracle/unit/layout.py` (pdfminer results) through
//! `pdfminer::layout`.
//!
//! A layout case is a raw page dumped by the generator: the Rust objects are
//! rebuilt from the dumped attributes (bypassing the font), analyzed from
//! the depth Python analyzed them at, with the `id()` order Python's boxes
//! and groups had, and the analyzed tree is written as the generator writes
//! it (`dump_tree`) and compared through `canon`.

mod utils_layout_common;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use pdfminer::layout::{
    CharDisp, CurveKind, IdSource, LAParams, LTChar, LTCharFont, LTComponent, LTCurve, LTFigure, LTImage, LTItem,
    LTLayoutContainer, LTPage, LTPayload, LTTextBox, LTTextLine, TextGroupElement, TextLineElement, TextLineKind,
};
use pdfminer::pdfcolor::PDFColorSpace;
use pdfminer::utils::PlaneItem;
use pdfminer::pdftypes::{PDFObject, PDFResolver, PDFStream};
use pdftrace::canon::{canon, canon_exc};
use pdftrace::unit::UnitData;
use pdftrace::value::{PyStr, Value};
use pycompat::recursion;
use pycompat::{Num, PyInt, PyResult};
use utils_layout_common::*;

/// The test payload: dumped values.
#[derive(Clone)]
struct TP;

impl LTPayload for TP {
    type FontName = Value;
    type GraphicState = ();
    type Color = Value;
}

// ---------------------------------------------------------------------------
// Dumped values -> objects

fn fields(v: &Value) -> (&str, &[(Value, Value)]) {
    match v {
        Value::Named { name, fields } => (name, fields),
        other => panic!("not a named object: {other:?}"),
    }
}

fn field<'a>(f: &'a [(Value, Value)], key: &str) -> &'a Value {
    f.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v).unwrap_or_else(|| panic!("no {key}"))
}

fn component(f: &[(Value, Value)]) -> LTComponent {
    LTComponent {
        x0: num(field(f, "x0")),
        y0: num(field(f, "y0")),
        x1: num(field(f, "x1")),
        y1: num(field(f, "y1")),
        width: num(field(f, "width")),
        height: num(field(f, "height")),
    }
}

fn opt(v: &Value) -> Option<Value> {
    match v {
        Value::None => None,
        v => Some(v.clone()),
    }
}

fn text(v: &Value) -> String {
    str_arg(v).to_owned()
}

fn path_segments(v: &Value) -> Option<Vec<(char, Vec<Num>)>> {
    match v {
        Value::None => None,
        v => Some(
            items(v)
                .iter()
                .map(|seg| {
                    let seg = items(seg);
                    let op = str_arg(&seg[0]).chars().next().expect("an operator");
                    (op, seg[1..].iter().map(num).collect())
                })
                .collect(),
        ),
    }
}

/// Builds the raw objects; `order` receives their identities, depth first.
struct Builder {
    weak: Weak<dyn PDFResolver>,
    order: Vec<usize>,
}

fn ptr<T>(rc: &Rc<T>) -> usize {
    Rc::as_ptr(rc) as *const () as usize
}

impl Builder {
    fn char(&self, f: &[(Value, Value)]) -> LTChar<TP> {
        let (_, ncs) = fields(field(f, "ncs"));
        LTChar {
            _text: text(field(f, "_text")),
            matrix: nums(field(f, "matrix")),
            fontname: field(f, "fontname").clone(),
            ncs: Rc::new(PDFColorSpace::new(text(field(ncs, "name")), build(field(ncs, "ncomponents"), &self.weak))),
            graphicstate: (),
            adv: num(field(f, "adv")),
            upright: matches!(field(f, "upright"), Value::Bool(true)),
            comp: component(f),
            size: num(field(f, "size")),
        }
    }

    fn curve(&self, name: &str, f: &[(Value, Value)]) -> LTCurve<TP> {
        let dash = match field(f, "dashing_style") {
            Value::None => None,
            v => {
                let d = items(v);
                Some((build(&d[0], &self.weak), build(&d[1], &self.weak)))
            }
        };
        LTCurve {
            kind: match name {
                "LTCurve" => CurveKind::Curve,
                "LTLine" => CurveKind::Line,
                _ => CurveKind::Rect,
            },
            comp: component(f),
            pts: items(field(f, "pts")).iter().map(nums).collect(),
            linewidth: num(field(f, "linewidth")),
            stroke: matches!(field(f, "stroke"), Value::Bool(true)),
            fill: matches!(field(f, "fill"), Value::Bool(true)),
            evenodd: matches!(field(f, "evenodd"), Value::Bool(true)),
            stroking_color: opt(field(f, "stroking_color")),
            non_stroking_color: opt(field(f, "non_stroking_color")),
            original_path: path_segments(field(f, "original_path")),
            dashing_style: dash,
        }
    }

    fn image(&self, f: &[(Value, Value)]) -> LTImage {
        let PDFObject::Stream(stream) = build(field(f, "stream"), &self.weak) else { panic!("a stream") };
        let srcsize = items(field(f, "srcsize"));
        LTImage {
            comp: component(f),
            name: text(field(f, "name")),
            stream,
            srcsize: (build(&srcsize[0], &self.weak), build(&srcsize[1], &self.weak)),
            imagemask: build(field(f, "imagemask"), &self.weak),
            bits: build(field(f, "bits"), &self.weak),
            colorspace: build(field(f, "colorspace"), &self.weak),
        }
    }

    fn item(&mut self, v: &Value) -> LTItem<TP> {
        let (name, f) = fields(v);
        match name {
            "LTChar" => {
                let c = Rc::new(self.char(f));
                self.order.push(ptr(&c));
                LTItem::Char(c)
            }
            "LTCurve" | "LTLine" | "LTRect" => {
                let c = Rc::new(self.curve(name, f));
                self.order.push(ptr(&c));
                LTItem::Curve(c)
            }
            "LTImage" => {
                let i = Rc::new(self.image(f));
                self.order.push(ptr(&i));
                LTItem::Image(i)
            }
            "LTFigure" => {
                let fig = Rc::new(RefCell::new(LTFigure {
                    name: text(field(f, "name")),
                    matrix: nums(field(f, "matrix")),
                    layout: LTLayoutContainer { comp: component(f), _objs: Vec::new(), groups: None },
                }));
                self.order.push(ptr(&fig));
                let objs: Vec<LTItem<TP>> = items(field(f, "_objs")).iter().map(|c| self.item(c)).collect();
                fig.borrow_mut().layout._objs = objs;
                LTItem::Figure(fig)
            }
            other => panic!("raw object {other}"),
        }
    }

    fn page(&mut self, v: &Value) -> LTPage<TP> {
        let (name, f) = fields(v);
        assert_eq!(name, "LTPage");
        let objs = items(field(f, "_objs")).iter().map(|c| self.item(c)).collect();
        let Value::Int(pageid) = field(f, "pageid") else { panic!("pageid") };
        LTPage {
            layout: LTLayoutContainer { comp: component(f), _objs: objs, groups: None },
            pageid: pageid.clone(),
            rotate: num(field(f, "rotate")),
        }
    }
}

// ---------------------------------------------------------------------------
// Objects -> dumped values

fn s(k: &str) -> Value {
    Value::str(k)
}

fn named(name: &str, fields: Vec<(&str, Value)>) -> Value {
    Value::Named { name: name.to_owned(), fields: fields.into_iter().map(|(k, v)| (s(k), v)).collect() }
}

fn comp_fields(c: &LTComponent) -> Vec<(&'static str, Value)> {
    vec![
        ("x0", num_value(&c.x0)),
        ("y0", num_value(&c.y0)),
        ("x1", num_value(&c.x1)),
        ("y1", num_value(&c.y1)),
        ("width", num_value(&c.width)),
        ("height", num_value(&c.height)),
        ("bbox", tuple(&c.bbox())),
    ]
}

fn raw_char(c: &LTChar<TP>) -> Value {
    let mut f = vec![
        ("_text", Value::str(c._text.clone())),
        ("matrix", tuple(&c.matrix)),
        ("fontname", c.fontname.clone()),
        (
            "ncs",
            named(
                "PDFColorSpace",
                vec![("name", Value::str(c.ncs.name.clone())), ("ncomponents", Value::from(&c.ncs.ncomponents))],
            ),
        ),
        ("graphicstate", Value::None),
        ("adv", num_value(&c.adv)),
        ("upright", Value::Bool(c.upright)),
    ];
    f.extend(comp_fields(&c.comp));
    f.push(("size", num_value(&c.size)));
    named("LTChar", f)
}

fn obj_value(o: &PDFObject) -> Value {
    Value::from(o)
}

fn raw_curve(c: &LTCurve<TP>) -> Value {
    let mut f = comp_fields(&c.comp);
    f.push(("pts", Value::List(c.pts.iter().map(|p| tuple(p)).collect())));
    f.push(("linewidth", num_value(&c.linewidth)));
    f.push(("stroke", Value::Bool(c.stroke)));
    f.push(("fill", Value::Bool(c.fill)));
    f.push(("evenodd", Value::Bool(c.evenodd)));
    f.push(("stroking_color", c.stroking_color.clone().unwrap_or(Value::None)));
    f.push(("non_stroking_color", c.non_stroking_color.clone().unwrap_or(Value::None)));
    f.push((
        "original_path",
        match &c.original_path {
            None => Value::None,
            Some(path) => Value::List(
                path.iter()
                    .map(|(op, args)| {
                        let mut t = vec![Value::str(op.to_string())];
                        t.extend(args.iter().map(num_value));
                        Value::Tuple(t)
                    })
                    .collect(),
            ),
        },
    ));
    f.push((
        "dashing_style",
        match &c.dashing_style {
            None => Value::None,
            Some((a, b)) => Value::Tuple(vec![obj_value(a), obj_value(b)]),
        },
    ));
    named(c.class_name(), f)
}

fn raw_image(i: &LTImage) -> Value {
    let mut f = comp_fields(&i.comp);
    f.push(("name", Value::str(i.name.clone())));
    f.push(("stream", obj_value(&PDFObject::Stream(i.stream.clone()))));
    f.push(("srcsize", Value::Tuple(vec![obj_value(&i.srcsize.0), obj_value(&i.srcsize.1)])));
    f.push(("imagemask", obj_value(&i.imagemask)));
    f.push(("bits", obj_value(&i.bits)));
    f.push(("colorspace", obj_value(&i.colorspace)));
    named("LTImage", f)
}

fn raw_item(item: &LTItem<TP>) -> Value {
    match item {
        LTItem::Char(c) => raw_char(c),
        LTItem::Curve(c) => raw_curve(c),
        LTItem::Image(i) => raw_image(i),
        LTItem::Figure(fig) => {
            let fig = fig.borrow();
            let mut f = vec![("name", Value::str(fig.name.clone())), ("matrix", tuple(&fig.matrix))];
            f.extend(comp_fields(&fig.layout.comp));
            f.push(("_objs", Value::List(fig.layout._objs.iter().map(raw_item).collect())));
            f.push(("groups", Value::None));
            named("LTFigure", f)
        }
        _ => panic!("not a raw object"),
    }
}

fn raw_page(p: &LTPage<TP>) -> Value {
    let mut f = comp_fields(&p.layout.comp);
    f.push(("_objs", Value::List(p.layout._objs.iter().map(raw_item).collect())));
    f.push(("groups", Value::None));
    f.push(("pageid", Value::Int(p.pageid.clone())));
    f.push(("rotate", num_value(&p.rotate)));
    named("LTPage", f)
}

/// `dump_tree` of the generator.
struct TreeDumper<'a> {
    index: &'a HashMap<usize, i64>,
}

fn text_value(r: PyResult<String>) -> Value {
    result(r, Value::str)
}

impl TreeDumper<'_> {
    fn raw_ref(&self, id: usize) -> Option<Value> {
        self.index.get(&id).map(|i| named("#", vec![("i", Value::Int(PyInt::Small(*i)))]))
    }

    fn line(&self, l: &LTTextLine<TP>) -> Value {
        let mut f = comp_fields(&l.comp);
        let objs = l
            ._objs
            .iter()
            .map(|o| match o {
                TextLineElement::Char(c) => self.raw_ref(ptr(c)).expect("a raw char"),
                TextLineElement::Anno(a) => named("LTAnno", vec![("_text", Value::str(a._text.clone()))]),
            })
            .collect();
        f.push(("_objs", Value::List(objs)));
        f.push(("word_margin", num_value(&l.word_margin)));
        match &l.kind {
            TextLineKind::Horizontal { _x1 } => f.push(("_x1", num_value(_x1))),
            TextLineKind::Vertical { _y0 } => f.push(("_y0", num_value(_y0))),
        }
        f.push(("get_text()", text_value(l.get_text())));
        named(l.class_name(), f)
    }

    fn textbox(&self, b: &LTTextBox<TP>) -> Value {
        let mut f = comp_fields(&b.comp);
        f.push(("_objs", Value::List(b._objs.iter().map(|l| self.line(&l.borrow())).collect())));
        f.push(("index", Value::Int(PyInt::Small(b.index))));
        f.push(("get_text()", text_value(b.get_text())));
        named(b.class_name(), f)
    }

    fn item(&self, item: &LTItem<TP>) -> Value {
        match item {
            LTItem::Char(c) => self.raw_ref(ptr(c)).expect("a raw char"),
            LTItem::Curve(c) => self.raw_ref(ptr(c)).expect("a raw curve"),
            LTItem::Image(i) => self.raw_ref(ptr(i)).expect("a raw image"),
            LTItem::Figure(fig) => {
                let fig = fig.borrow();
                let mut f = vec![("name", Value::str(fig.name.clone())), ("matrix", tuple(&fig.matrix))];
                f.extend(self.container(&fig.layout));
                named("LTFigure", f)
            }
            LTItem::TextLine(l) => self.line(&l.borrow()),
            LTItem::TextBox(b) => self.textbox(&b.borrow()),
        }
    }

    fn group(&self, g: &TextGroupElement<TP>, positions: &HashMap<usize, i64>) -> Value {
        match g {
            TextGroupElement::Box(b) => named("box#", vec![("i", Value::Int(PyInt::Small(positions[&ptr(b)])))]),
            TextGroupElement::Group(g) => {
                let g = g.borrow();
                let mut f = comp_fields(&g.comp);
                f.push(("_objs", Value::List(g._objs.iter().map(|c| self.group(c, positions)).collect())));
                named(g.class_name(), f)
            }
        }
    }

    /// The container's fields: the box, `_objs`, `groups`.
    fn container(&self, c: &LTLayoutContainer<TP>) -> Vec<(&'static str, Value)> {
        let mut positions = HashMap::new();
        for (i, o) in c._objs.iter().enumerate() {
            if let LTItem::TextBox(b) = o {
                positions.insert(ptr(b), i as i64);
            }
        }
        let mut f = comp_fields(&c.comp);
        f.push(("_objs", Value::List(c._objs.iter().map(|o| self.item(o)).collect())));
        f.push((
            "groups",
            match &c.groups {
                None => Value::None,
                Some(groups) => Value::List(groups.iter().map(|g| self.group(g, &positions)).collect()),
            },
        ));
        f
    }

    fn page(&self, p: &LTPage<TP>) -> Value {
        let mut f = self.container(&p.layout);
        f.push(("pageid", Value::Int(p.pageid.clone())));
        f.push(("rotate", num_value(&p.rotate)));
        named("LTPage", f)
    }
}

// ---------------------------------------------------------------------------
// ids

/// The `id()` order Python's boxes and groups had, by their smallest char
/// index and number of boxes.
struct RecordedIds<'a> {
    ranks: HashMap<(i64, i64), u64>,
    index: &'a HashMap<usize, i64>,
    /// The ids already given, by object (the contents of a box or group
    /// never change once it is made).
    given: HashMap<usize, u64>,
}

impl RecordedIds<'_> {
    fn key(&self, obj: &TextGroupElement<TP>) -> (i64, i64) {
        match obj {
            TextGroupElement::Box(b) => {
                let b = b.borrow();
                let min = b
                    ._objs
                    .iter()
                    .flat_map(|l| {
                        l.borrow()
                            ._objs
                            .iter()
                            .filter_map(|o| match o {
                                TextLineElement::Char(c) => Some(self.index[&ptr(c)]),
                                TextLineElement::Anno(_) => None,
                            })
                            .collect::<Vec<_>>()
                    })
                    .min()
                    .expect("a box has chars");
                (min, 1)
            }
            TextGroupElement::Group(g) => {
                let keys: Vec<(i64, i64)> = g.borrow()._objs.iter().map(|c| self.key(c)).collect();
                (keys.iter().map(|k| k.0).min().expect("children"), keys.iter().map(|k| k.1).sum())
            }
        }
    }
}

impl IdSource<TP> for RecordedIds<'_> {
    fn id(&mut self, obj: &TextGroupElement<TP>) -> u64 {
        if let Some(id) = self.given.get(&obj.plane_id()) {
            return *id;
        }
        let key = self.key(obj);
        let id = *self.ranks.get(&key).unwrap_or_else(|| panic!("no Python id for the box or group {key:?}"));
        self.given.insert(obj.plane_id(), id);
        id
    }
}

// ---------------------------------------------------------------------------
// LAParams

fn laparams(v: &Value) -> LAParams {
    let Value::Dict(items) = v else { panic!("laparams: {v:?}") };
    let mut lp = LAParams::default();
    for (k, v) in items {
        match str_arg(k) {
            "line_overlap" => lp.line_overlap = num(v),
            "char_margin" => lp.char_margin = num(v),
            "line_margin" => lp.line_margin = num(v),
            "word_margin" => lp.word_margin = num(v),
            "boxes_flow" => lp.boxes_flow = if matches!(v, Value::None) { None } else { Some(num(v)) },
            "detect_vertical" => lp.detect_vertical = matches!(v, Value::Bool(true)),
            "all_texts" => lp.all_texts = matches!(v, Value::Bool(true)),
            other => panic!("{other}"),
        }
    }
    LAParams::new(
        lp.line_overlap,
        lp.char_margin,
        lp.line_margin,
        lp.word_margin,
        lp.boxes_flow,
        lp.detect_vertical,
        lp.all_texts,
    )
    .expect("valid laparams")
}

/// The first difference of two canonical texts, with some context.
fn first_difference(a: &str, b: &str) -> String {
    let i = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    let lo = i.saturating_sub(300);
    format!(
        "at byte {i}:\n    python: ...{}\n    rust:   ...{}",
        &a[lo..(i + 200).min(a.len())],
        &b[lo..(i + 200).min(b.len())]
    )
}

/// Replays the layout cases of `name`.
fn replay_layout(name: &str) {
    let cases = UnitData::open("layout").cases(name);
    assert!(!cases.is_empty());
    let mut failures = Vec::new();
    let mut runs_total = 0;
    for (args, expected) in &cases {
        let case = str_arg(&args[0]);
        let (_doc, weak) = doc();
        // the dump of the raw page rebuilds identically
        {
            let mut b = Builder { weak: weak.clone(), order: Vec::new() };
            let page = b.page(&args[1]);
            let again = canon(&raw_page(&page));
            let orig = canon(&args[1]);
            if again != orig {
                failures.push(format!("{case}: raw page {}", first_difference(&orig, &again)));
                continue;
            }
        }
        let runs = items(&args[2]);
        let expected = items(expected);
        for (run, expected) in runs.iter().zip(expected) {
            runs_total += 1;
            let run = items(run);
            let lp = laparams(&run[0]);
            let mut b = Builder { weak: weak.clone(), order: Vec::new() };
            let mut page = b.page(&args[1]);
            let index: HashMap<usize, i64> = b.order.iter().enumerate().map(|(i, p)| (*p, i as i64)).collect();
            let ranks = items(&run[2])
                .iter()
                .map(|r| {
                    let r = items(r);
                    ((int_arg(&r[0]), int_arg(&r[1])), int_arg(&r[2]) as u64)
                })
                .collect();
            let mut ids = RecordedIds { ranks, index: &index, given: HashMap::new() };
            let out = recursion::with_depth(depth(&run[1]), || page.analyze_with_ids(&lp, &mut ids));
            let actual = match out {
                Ok(()) => canon(&TreeDumper { index: &index }.page(&page)),
                Err(e) => canon_exc(&e),
            };
            let expected = str_arg(expected);
            if actual != expected {
                failures.push(format!("{case} {:?}: {}", run[0], first_difference(expected, &actual)));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{name}: {} of {runs_total} runs differ; first ones:\n{}",
        failures.len(),
        failures.iter().take(3).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn adversarial() {
    replay_layout("adversarial");
}

#[test]
fn real() {
    replay_layout("real");
}

// ---------------------------------------------------------------------------
// Objects

/// The test font of the generator.
struct TestFont {
    fontname: Value,
    vertical: bool,
    descent: Num,
}

impl LTCharFont<TP> for TestFont {
    fn fontname(&self) -> Value {
        self.fontname.clone()
    }

    fn is_vertical(&self) -> bool {
        self.vertical
    }

    fn get_descent(&self) -> PyResult<Num> {
        Ok(self.descent.clone())
    }
}

fn gray() -> Rc<PDFColorSpace> {
    pdfminer::pdfcolor::predefined_colorspace("DeviceGray").expect("DeviceGray")
}

#[test]
fn ltchar() {
    replay("layout", "ltchar", |a| {
        let f = items(&a[1]);
        let font =
            TestFont { fontname: f[0].clone(), vertical: matches!(f[1], Value::Bool(true)), descent: num(&f[2]) };
        let textdisp = match &a[7] {
            Value::Tuple(t) => {
                CharDisp::Vertical(if matches!(t[0], Value::None) { None } else { Some(num(&t[0])) }, num(&t[1]))
            }
            v => CharDisp::Scalar(num(v)),
        };
        let c = LTChar::<TP>::new(
            nums(&a[0]),
            &font,
            num(&a[2]),
            num(&a[3]),
            num(&a[4]),
            text(&a[5]),
            num(&a[6]),
            textdisp,
            gray(),
            (),
        );
        result(c, |c| raw_char(&c))
    });
}

fn bool_value(b: bool) -> Value {
    Value::Bool(b)
}

#[test]
fn objects() {
    let (_doc, weak) = doc();
    replay("layout", "objects", |a| match str_arg(&a[0]) {
        "LTCurve" => {
            let pts = items(&a[2]).iter().map(nums).collect();
            let c = LTCurve::<TP>::new(
                num(&a[1]),
                pts,
                true,
                false,
                true,
                Some(Value::Tuple(vec![Value::Float(0.5)])),
                Some(Value::str("P1")),
                Some(vec![('h', vec![])]),
                Some((
                    PDFObject::List(pdfminer::pdftypes::PDFList::new(vec![PDFObject::Int(PyInt::Small(1))])),
                    PDFObject::Int(PyInt::Small(2)),
                )),
            );
            result(c, |c| raw_curve(&c))
        }
        "LTLine" => {
            let pts = items(&a[2]);
            let c = LTCurve::<TP>::new_line(
                num(&a[1]),
                nums(&pts[0]),
                nums(&pts[1]),
                false,
                false,
                false,
                None,
                None,
                None,
                None,
            );
            result(c, |c| raw_curve(&c))
        }
        "LTRect" => {
            let c = LTCurve::<TP>::new_rect(num(&a[1]), nums(&a[2]), false, false, false, None, None, None, None);
            result(c, |c| raw_curve(&c))
        }
        "LTFigure" => {
            let f = LTFigure::<TP>::new("F".to_owned(), nums(&a[1]), nums(&a[2]));
            result(f, |f| raw_item(&LTItem::Figure(Rc::new(RefCell::new(f)))))
        }
        "LTPage" => {
            let p = LTPage::<TP>::new(PyInt::Small(3), nums(&a[1]), num(&a[2]));
            result(p, |p| raw_page(&p))
        }
        "LTImage" => {
            let PDFObject::Dict(attrs) = build(&a[1], &weak) else { panic!() };
            let stream = Rc::new(PDFStream::new(attrs, Rc::from(&b""[..]), None));
            let i = LTImage::new("Im0".to_owned(), stream, nums(&a[2]));
            result(i, |i| raw_image(&i))
        }
        "LAParams" => {
            let bf = if matches!(a[1], Value::None) { None } else { Some(num(&a[1])) };
            let lp =
                LAParams::new(Num::Float(0.5), Num::Float(2.0), Num::Float(0.5), Num::Float(0.1), bf, false, false);
            // the value given back as it was given (a bool stays a bool)
            result(lp, |_| a[1].clone())
        }
        "component" => {
            let c1 = LTComponent::new(nums(&a[1])).expect("a component");
            let c2 = LTComponent::new(nums(&a[2])).expect("a component");
            Value::List(vec![
                bool_value(c1.is_empty()),
                bool_value(c1.is_hoverlap(&c2)),
                result(c1.hdistance(&c2), |n| num_value(&n)),
                result(c1.hoverlap(&c2), |n| num_value(&n)),
                bool_value(c1.is_voverlap(&c2)),
                result(c1.vdistance(&c2), |n| num_value(&n)),
                result(c1.voverlap(&c2), |n| num_value(&n)),
            ])
        }
        other => panic!("{other}"),
    });
}

#[test]
fn textline() {
    let (_doc, weak) = doc();
    replay("layout", "textline", |a| {
        let b = Builder { weak: weak.clone(), order: Vec::new() };
        let chars: Vec<Rc<LTChar<TP>>> = items(&a[2]).iter().map(|c| Rc::new(b.char(fields(c).1))).collect();
        let index: HashMap<usize, i64> = chars.iter().enumerate().map(|(i, c)| (ptr(c), i as i64)).collect();
        let wm = num(&a[1]);
        let mut line = if matches!(a[0], Value::Bool(true)) {
            LTTextLine::new_vertical(wm).expect("a line")
        } else {
            LTTextLine::new_horizontal(wm).expect("a line")
        };
        for c in &chars {
            line.add(c).expect("add");
        }
        let empty = line.is_empty().expect("is_empty");
        let line = Rc::new(RefCell::new(line));
        let mut page = LTPage::<TP>::new(PyInt::Small(1), [int(0), int(0), int(1), int(1)], int(0)).expect("a page");
        page.layout._objs = vec![LTItem::TextLine(line)];
        let tree = canon(&TreeDumper { index: &index }.page(&page));
        Value::List(vec![Value::Str(PyStr::Text(tree)), Value::Bool(empty)])
    });
}

fn int(v: i64) -> Num {
    Num::Int(PyInt::Small(v))
}
