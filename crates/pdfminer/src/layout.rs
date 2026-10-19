//! Port of `pdfminer/layout.py`: the layout objects (`LT*`) and the layout
//! analysis (`LAParams`, `LTLayoutContainer.analyze`).
//!
//! # Objects
//!
//! Each `LT*` class is a struct whose public fields are the instance
//! attributes, declared in the order of the instance `__dict__` (pdfplumber
//! builds its objects from `obj.__dict__`); [`LTItem::dict_keys`] and the
//! `DICT_KEYS` constants give that order. `bbox` is not stored: upstream it
//! is always the tuple `(x0, y0, x1, y1)` of the other four attributes
//! ([`LTComponent::bbox`]). Classes that only differ by their methods share
//! a struct with a kind (`LTCurve`/`LTLine`/`LTRect`,
//! `LTTextLineHorizontal`/`LTTextLineVertical`, ...).
//!
//! Python objects are shared and mutable: a text box is both in the page's
//! `_objs` and in a group of `page.groups`, and the analysis mutates it
//! (`index`, the order of its lines). Containers are therefore
//! `Rc<RefCell<..>>`, immutable objects (`LTChar`, `LTCurve`, `LTImage`)
//! `Rc<..>`, and identity (`is`, `set`, `dict` keys) is the `Rc` pointer.
//! The attributes pdfplumber sets on raw objects after their creation
//! (`mcid`, `tag`) are kept in [`ExtraAttrs`], which takes `&self`.
//!
//! Coordinates are [`Num`]: they are floats in practice, but ints appear
//! (`LTPage` starts at `(0, 0)`, an empty container's box is
//! `(INF, INF, -INF, -INF)`, kept by `min`/`max` against NaN or huge
//! coordinates) and show in the attributes.
//!
//! The objects the interpreter hands to the layout without the layout
//! looking into them (the graphic state of a char, the colors of a curve,
//! the font name) are types of a [`LTPayload`] (the interpreter's types, or
//! the test harness's dumped values); the font is seen through
//! [`LTCharFont`], the three `PDFFont` methods `LTChar.__init__` calls.
//!
//! # Analysis and object identity
//!
//! `group_textboxes` keeps its candidate pairs in a heap of tuples
//! `(skip_isany, dist, id(obj1), id(obj2), obj1, obj2)`: pairs at the same
//! distance are popped in the order of Python's `id()` of the objects, that
//! is of their memory addresses, which depend on the allocator's state.
//! pdfminer's result is therefore not reproducible in general when two
//! pairs tie (measured: on a third of the pages of pdfplumber's test PDFs a
//! tie happens, and the same page analyzed after other allocations gives
//! different groups, sometimes a different text box order). The port takes
//! the `id()` of boxes and groups from an [`IdSource`]: by default
//! [`CreationOrder`] (increasing in order of first use, the order pymalloc
//! hands out fresh blocks), and the differential tests give the ids Python
//! had.
//!
//! # Recursion
//!
//! Groups nest as deep as the merges chain, so `analyze`, `get_text` and
//! `IndexAssigner.run` recurse as upstream and take the Python frames
//! (and the C units of calls made from C: sort keys, generators resumed by
//! `list()`/`str.join`) that CPython takes on these paths
//! (`pycompat::recursion`), so a `RecursionError` happens where it happens
//! in Python. The iterative parts (`group_objects`, `group_textlines`,
//! `group_textboxes` and the methods they call) take one frame each and do
//! not count their callees call by call: they run at the depth of a page's
//! or a figure's `analyze`, which pdfminer's interpreter never reaches near
//! the limit.
//!
//! The constructors of the raw objects, which `converter` calls as deep as
//! Form XObjects nest ([`LTChar::new`], [`LTCurve::new`],
//! [`LTCurve::new_line`], [`LTCurve::new_rect`], [`LTImage::new`],
//! [`LTFigure::new_obj`], [`LTPage::new`]), take the
//! frames of the class call: `_PyObject_MakeTpCall`'s C unit (`" while
//! calling a Python object"`) and the evaluation loop entry of `__init__`
//! (three C units in all, measured by `oracle/unit/converter.py`'s probes),
//! the `__init__` frame and those of the base `__init__`s it calls
//! (`LTComponent.__init__` -> `set_bbox`; `LTItem.__init__` and
//! `LTText.__init__` are `object.__init__`, C), and the leaf frames of the
//! Python functions they call (`apply_matrix_rect` -> `apply_matrix_pt`,
//! `get_bound`, `PDFStream.get_any`, `font.is_vertical()`), each where
//! upstream calls it, so that the outcome at the recursion limit (a
//! `RecursionError` or an exception raised earlier) is Python's.
//! `oracle/unit/converter.py` checks them at the boundary (its `depth`
//! cases). The analysis's own constructors (`LTTextLineHorizontal`...) are
//! not counted (see above).

use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use pycompat::pystr::PyStrBuilder;
use pycompat::recursion::{c_call, eval_entry, py_frame, Guard};
use pycompat::{exc, Num, PyInt, PyResult, PyStr};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::casting::IterItem;
use crate::pdfcolor::PDFColorSpace;
use crate::pdftypes::{PDFList, PDFObject, PDFStream};
use crate::utils::{
    apply_matrix_rect, format_fixed, fsplit, get_bound, inf, neg_inf, py_max2, py_min2, uniq, unpack, Matrix,
    ObjMatrix, Operand, Plane, PlaneItem, Point, Rect,
};

// ---------------------------------------------------------------------------
// Helpers

fn int(v: i64) -> Num {
    Num::Int(PyInt::Small(v))
}

/// `bool(x)` of a number.
fn truthy(x: &Num) -> bool {
    match x {
        Num::Int(i) => !i.is_zero(),
        Num::Float(f) => *f != 0.0,
    }
}

/// A Python function called from C (a sort key, a generator resumed by
/// `list()` or `str.join`): the evaluation loop entry, then its frame.
fn c_called_frame() -> PyResult<(Guard, Guard)> {
    let entry = eval_entry()?;
    Ok((entry, py_frame()?))
}

/// A class call, `Cls(...)` (see the module documentation, "Recursion"):
/// `_PyObject_MakeTpCall`'s C unit, then the evaluation loop entry of
/// `__init__` (called from C), held for the call; the caller then takes
/// the `__init__` frame.
pub fn class_call() -> PyResult<(Guard, Guard)> {
    let unit = c_call(" while calling a Python object")?;
    Ok((unit, eval_entry()?))
}

/// `apply_matrix_rect(m, rect)` with its frames (`utils` does not count
/// them): its own, and the leaf of each `apply_matrix_pt` (all at the same
/// depth, before its arithmetic: the first check decides).
pub fn apply_matrix_rect_counted(m: &Matrix, rect: &Rect) -> PyResult<Rect> {
    let _frame = py_frame()?;
    leaf_frame()?; // apply_matrix_pt
    apply_matrix_rect(m, rect)
}

/// A leaf call of a Python function that returns at once (its frame is
/// only checked).
fn leaf_frame() -> PyResult<()> {
    drop(py_frame()?);
    Ok(())
}

/// `list.sort(key=...)` with the keys already computed and Python's `<` on
/// them (`lt`): CPython 3.12's timsort (`pycompat::listsort`, which makes
/// the comparisons CPython makes, in its order), so the result is
/// CPython's even when `lt` is not a total order (NaN keys). The layout's
/// comparisons never raise (numbers and tuples of numbers).
///
/// In the layout, only the two children of a group can have NaN keys;
/// the lists of 64 or more are box lines by `-y1`, boxes by their index or
/// by their coordinates, never NaN (an expandable container's bounds come
/// from `min`/`max` against `INF`, which a NaN never replaces, see
/// `LTExpandableContainer.add`). Public for the differential test against
/// Python's `list.sort` (`objects` cases `list_sort`).
pub fn list_sort<T, K>(items: &mut Vec<T>, keys: Vec<K>, lt: impl Fn(&K, &K) -> bool) {
    let n = items.len();
    if n < 2 {
        return;
    }
    let order = pycompat::listsort::sort_permutation(n, |i, j| Ok(lt(&keys[i], &keys[j])))
        .expect("the layout's comparisons never raise");
    let mut slots: Vec<Option<T>> = items.drain(..).map(Some).collect();
    items.extend(order.into_iter().map(|i| slots[i].take().expect("each index once")));
}

/// Python's `<` on two tuples of numbers of the same length (the first
/// differing pair decides; `==` and `<` are Python's, so a NaN never equals
/// anything and is never less). Python's `==` first checks identity: the
/// layout's keys are new float objects (`-obj.x1`), so a NaN is never the
/// same object in two keys.
pub fn tuple_lt(a: &[Num], b: &[Num]) -> bool {
    for (x, y) in a.iter().zip(b) {
        if !x.py_eq(y) {
            return x.py_lt(y);
        }
    }
    // equal tuples (the keys all have the same length)
    false
}

// ---------------------------------------------------------------------------
// LAParams

/// pdfminer/layout.py:48 `LAParams`: parameters for layout analysis (see
/// the upstream docstring). The margins are numbers (an int stays an int,
/// as a user's `LAParams(char_margin=3)`); `detect_vertical` and
/// `all_texts` are only tested for truth.
#[derive(Clone, Debug)]
pub struct LAParams {
    pub line_overlap: Num,
    pub char_margin: Num,
    pub line_margin: Num,
    pub word_margin: Num,
    pub boxes_flow: Option<Num>,
    pub detect_vertical: bool,
    pub all_texts: bool,
}

impl Default for LAParams {
    /// `LAParams()`
    fn default() -> LAParams {
        LAParams {
            line_overlap: Num::Float(0.5),
            char_margin: Num::Float(2.0),
            line_margin: Num::Float(0.5),
            word_margin: Num::Float(0.1),
            boxes_flow: Some(Num::Float(0.5)),
            detect_vertical: false,
            all_texts: false,
        }
    }
}

impl LAParams {
    // pdfminer/layout.py:76 LAParams.__init__
    pub fn new(
        line_overlap: Num,
        char_margin: Num,
        line_margin: Num,
        word_margin: Num,
        boxes_flow: Option<Num>,
        detect_vertical: bool,
        all_texts: bool,
    ) -> PyResult<LAParams> {
        let laparams =
            LAParams { line_overlap, char_margin, line_margin, word_margin, boxes_flow, detect_vertical, all_texts };
        laparams.validate()?;
        Ok(laparams)
    }

    // pdfminer/layout.py:96 LAParams._validate
    /// `boxes_flow` must be `None` or a number in `[-1, 1]` (a number
    /// argument always passes the type check here).
    fn validate(&self) -> PyResult<()> {
        if let Some(boxes_flow) = &self.boxes_flow {
            if !(int(-1).py_le(boxes_flow) && boxes_flow.py_le(&int(1))) {
                return Err(exc::PDFValueError("LAParam boxes_flow should be None, or a number between -1 and +1"));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Payload and font

/// The types of the objects the interpreter attaches to layout objects and
/// that the layout only stores (see the module documentation).
pub trait LTPayload: Clone + 'static {
    /// `LTChar.fontname`: `PDFFont.fontname`, a `str` or whatever object
    /// the font descriptor's `/FontName` holds.
    type FontName: Clone;
    /// `LTChar.graphicstate`: the interpreter's `PDFGraphicState` copy.
    type GraphicState: Clone;
    /// `LTCurve.stroking_color`, `LTCurve.non_stroking_color`: the
    /// interpreter's `Color`.
    type Color: Clone;
    /// The value of an attribute set on an object after its creation
    /// ([`ExtraAttrs`]): pdfplumber's `mcid` (the `MCID` of the
    /// marked-content properties, any PDF object, or `None`) and `tag` (a
    /// `str` or `None`).
    type Attr: Clone;
}

/// The attributes set on a layout object after its creation, in the order
/// they were first set, as they follow the class's own attributes in
/// `__dict__`. pdfplumber's aggregator
/// (pdfplumber/page.py:130 `PDFPageAggregatorWithMarkedContent.tag_cur_item`)
/// sets `mcid`, then `tag`, on the last object of the current page or
/// figure after each `render_char`, `render_image` and `paint_path`: the
/// new object, or an older one (an `LTFigure`, or any object when
/// `paint_path` added nothing), whose values
/// are then replaced in place. Only raw objects (chars, curves, images,
/// figures) are tagged: the analysis runs later. The names are never those
/// of the class's own attributes.
#[derive(Clone)]
pub struct ExtraAttrs<V>(RefCell<Vec<(&'static str, V)>>);

impl<V> Default for ExtraAttrs<V> {
    fn default() -> ExtraAttrs<V> {
        ExtraAttrs(RefCell::new(Vec::new()))
    }
}

impl<V: Clone> ExtraAttrs<V> {
    /// `setattr(obj, name, value)`: an attribute already set keeps its
    /// place in `__dict__`, a new one comes last.
    pub fn set(&self, name: &'static str, value: V) {
        let mut attrs = self.0.borrow_mut();
        match attrs.iter_mut().find(|(k, _)| *k == name) {
            Some((_, v)) => *v = value,
            None => attrs.push((name, value)),
        }
    }

    /// The names and values, in `__dict__` order.
    pub fn items(&self) -> Vec<(&'static str, V)> {
        self.0.borrow().clone()
    }
}

/// The `PDFFont` methods `LTChar.__init__` calls.
pub trait LTCharFont<P: LTPayload> {
    /// `font.fontname`
    fn fontname(&self) -> P::FontName;
    /// `font.is_vertical()`
    fn is_vertical(&self) -> bool;
    /// `font.get_descent()`
    fn get_descent(&self) -> PyResult<Num>;
}

/// The `textdisp` argument of `LTChar`: `font.char_disp(cid)`, a number for
/// a horizontal font, a `(vx, vy)` tuple for a vertical one.
///
/// `vx` and `vy` are whatever `pdffont` found, unchecked: the items of a
/// `/W2` array (any PDF object), `vy` of `(vy, w) = resolve1(DW2)` (an
/// item of any iterable: a dict key is a `str`, a byte of a bytes string an
/// int), and `vx` is `None` ([`PDFObject::Null`]) for the default
/// displacement. `LTChar.__init__` does the arithmetic and raises Python's
/// `TypeError` on a value that is not a number.
#[derive(Clone, Debug)]
pub enum CharDisp {
    Scalar(Num),
    Vertical(IterItem, IterItem),
}

/// `x * y` where `y` is a number and `x` any value, as far as
/// `LTChar.__init__` needs it: a number gives the product; a sequence
/// (`bytes`, `str`, `list`) is repeated by an int (the result is a
/// sequence again: `None`) and raises with a float; any other type raises
/// the `TypeError` of an unsupported operand.
///
/// A repetition is not built: its only use is a further `* 0.001`, which
/// raises. Only the size checks CPython makes before allocating are
/// reproduced (`OverflowError` for a count beyond `Py_ssize_t` or a too
/// long result, `MemoryError` for a list or a `str` too long for its
/// allocation size); a repetition that fits these checks but not in memory
/// raises `MemoryError` in Python, depending on the machine, and not here.
/// The interpreter passes a float `fontsize`, or the int `0` of a text
/// state without `Tf`, which repeats nothing.
fn item_mul(x: &IterItem, y: &Num) -> PyResult<Option<Num>> {
    if let IterItem::Obj(o) = x {
        if let Some(n) = o.as_num() {
            return n.mul(y).map(Some);
        }
    }
    let len = match x {
        IterItem::Obj(PDFObject::Bytes(b)) => b.len(),
        IterItem::Obj(PDFObject::List(l)) => l.borrow().len(),
        IterItem::Str(s) => s.chars().count(),
        _ => {
            let y_name = if y.is_float() { "float" } else { "int" };
            return Err(exc::TypeError(format!(
                "unsupported operand type(s) for *: '{}' and '{y_name}'",
                x.type_name()
            )));
        }
    };
    let Num::Int(count) = y else {
        return Err(exc::TypeError("can't multiply sequence by non-int of type 'float'"));
    };
    // sequence_repeat: PyNumber_AsSsize_t(count, OverflowError), then the
    // repeat function of the type
    let Some(count) = count.as_i64() else {
        return Err(exc::OverflowError("cannot fit 'int' into an index-sized integer"));
    };
    if count <= 0 || len == 0 {
        return Ok(None);
    }
    let len = i128::from(i64::try_from(len).unwrap_or(i64::MAX));
    let max = i128::from(i64::MAX);
    if len > max / i128::from(count) {
        return Err(match x {
            IterItem::Obj(PDFObject::Bytes(_)) => exc::OverflowError("repeated bytes are too long"),
            IterItem::Str(_) => exc::OverflowError("repeated string is too long"),
            _ => exc::MemoryError(""),
        });
    }
    // list_new_prealloc: PyMem_New(PyObject *, size)
    if matches!(x, IterItem::Obj(PDFObject::List(_))) && len * i128::from(count) > max / 8 {
        return Err(exc::MemoryError(""));
    }
    // unicode_repeat's PyUnicode_New(len * count, maxchar): the header
    // (sizeof(PyASCIIObject) for an ASCII str, else
    // sizeof(PyCompactUnicodeObject), on 64 bits) and the characters, of 1,
    // 2 or 4 bytes as the largest one needs, then a NUL, within
    // PY_SSIZE_T_MAX bytes
    if let IterItem::Str(s) = x {
        let (struct_size, char_size) = match s.chars().map(u32::from).max().unwrap_or(0) {
            0..=0x7f => (40, 1),
            0x80..=0xff => (56, 1),
            0x100..=0xffff => (56, 2),
            _ => (56, 4),
        };
        if len * i128::from(count) > (max - struct_size) / char_size - 1 {
            return Err(exc::MemoryError(""));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// LTComponent

/// pdfminer/layout.py:133 `LTComponent`: the bounding box attributes of an
/// object, `x0, y0, x1, y1, width, height` (then `bbox`, their tuple).
#[derive(Clone, Debug)]
pub struct LTComponent {
    pub x0: Num,
    pub y0: Num,
    pub x1: Num,
    pub y1: Num,
    pub width: Num,
    pub height: Num,
}

/// `__dict__` keys set by `LTComponent.set_bbox`, in order.
pub const COMPONENT_KEYS: [&str; 7] = ["x0", "y0", "x1", "y1", "width", "height", "bbox"];

impl LTComponent {
    // pdfminer/layout.py:136 LTComponent.__init__
    pub fn new(bbox: Rect) -> PyResult<LTComponent> {
        let [x0, y0, x1, y1] = bbox;
        let width = x1.sub(&x0)?;
        let height = y1.sub(&y0)?;
        Ok(LTComponent { x0, y0, x1, y1, width, height })
    }

    // pdfminer/layout.py:136 LTComponent.__init__
    /// `LTComponent.__init__(self, bbox)` called by a subclass's
    /// `__init__`: its frame and that of `set_bbox`, which it calls.
    pub fn init(bbox: Rect) -> PyResult<LTComponent> {
        let _frame = py_frame()?;
        let _set_bbox = py_frame()?;
        LTComponent::new(bbox)
    }

    // pdfminer/layout.py:156 LTComponent.set_bbox
    /// Sets the four coordinates, then `width` and `height` (an
    /// `OverflowError` of the subtraction leaves the coordinates set, as in
    /// Python).
    pub fn set_bbox(&mut self, bbox: Rect) -> PyResult<()> {
        let [x0, y0, x1, y1] = bbox;
        self.x0 = x0;
        self.y0 = y0;
        self.x1 = x1;
        self.y1 = y1;
        self.width = self.x1.sub(&self.x0)?;
        self.height = self.y1.sub(&self.y0)?;
        Ok(())
    }

    /// `self.bbox`: the tuple `(x0, y0, x1, y1)`.
    pub fn bbox(&self) -> Rect {
        [self.x0.clone(), self.y0.clone(), self.x1.clone(), self.y1.clone()]
    }

    // pdfminer/layout.py:166 LTComponent.is_empty
    pub fn is_empty(&self) -> bool {
        self.width.py_le(&int(0)) || self.height.py_le(&int(0))
    }

    // pdfminer/layout.py:169 LTComponent.is_hoverlap
    pub fn is_hoverlap(&self, obj: &LTComponent) -> bool {
        obj.x0.py_le(&self.x1) && self.x0.py_le(&obj.x1)
    }

    // pdfminer/layout.py:173 LTComponent.hdistance
    pub fn hdistance(&self, obj: &LTComponent) -> PyResult<Num> {
        if self.is_hoverlap(obj) {
            Ok(int(0))
        } else {
            Ok(py_min2(&self.x0.sub(&obj.x1)?.abs(), &self.x1.sub(&obj.x0)?.abs()))
        }
    }

    // pdfminer/layout.py:180 LTComponent.hoverlap
    pub fn hoverlap(&self, obj: &LTComponent) -> PyResult<Num> {
        if self.is_hoverlap(obj) {
            Ok(py_min2(&self.x0.sub(&obj.x1)?.abs(), &self.x1.sub(&obj.x0)?.abs()))
        } else {
            Ok(int(0))
        }
    }

    // pdfminer/layout.py:187 LTComponent.is_voverlap
    pub fn is_voverlap(&self, obj: &LTComponent) -> bool {
        obj.y0.py_le(&self.y1) && self.y0.py_le(&obj.y1)
    }

    // pdfminer/layout.py:191 LTComponent.vdistance
    pub fn vdistance(&self, obj: &LTComponent) -> PyResult<Num> {
        if self.is_voverlap(obj) {
            Ok(int(0))
        } else {
            Ok(py_min2(&self.y0.sub(&obj.y1)?.abs(), &self.y1.sub(&obj.y0)?.abs()))
        }
    }

    // pdfminer/layout.py:198 LTComponent.voverlap
    pub fn voverlap(&self, obj: &LTComponent) -> PyResult<Num> {
        if self.is_voverlap(obj) {
            Ok(py_min2(&self.y0.sub(&obj.y1)?.abs(), &self.y1.sub(&obj.y0)?.abs()))
        } else {
            Ok(int(0))
        }
    }

    /// `(min(self.x0, obj.x0), min(self.y0, obj.y0), max(self.x1, obj.x1),
    /// max(self.y1, obj.y1))`
    fn union(&self, obj: &LTComponent) -> Rect {
        [py_min2(&self.x0, &obj.x0), py_min2(&self.y0, &obj.y0), py_max2(&self.x1, &obj.x1), py_max2(&self.y1, &obj.y1)]
    }

    /// The empty box of an expandable container, `(+INF, +INF, -INF, -INF)`.
    fn expandable() -> PyResult<LTComponent> {
        LTComponent::new([inf(), inf(), neg_inf(), neg_inf()])
    }
}

// ---------------------------------------------------------------------------
// Leaf objects

/// An element of `LTCurve.original_path`: the operator and its points
/// mapped through the CTM, the tuple `(op, (x1, y1), ...)` that
/// `PDFLayoutAnalyzer.paint_path` builds (not the `utils.PathSegment` of the
/// interpreter's path, whose coordinates are flat).
pub type OriginalPathSegment = (char, Vec<Point>);

/// pdfminer/layout.py:206 `LTCurve`, :242 `LTLine`, :275 `LTRect`: which
/// class a [`LTCurve`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveKind {
    Curve,
    Line,
    Rect,
}

/// pdfminer/layout.py:206 `LTCurve` (and `LTLine`, `LTRect`, see
/// [`CurveKind`]): a path, with the original path segments and the dash
/// style.
#[derive(Clone)]
pub struct LTCurve<P: LTPayload> {
    pub kind: CurveKind,
    pub comp: LTComponent,
    pub pts: Vec<Point>,
    pub linewidth: Num,
    pub stroke: bool,
    pub fill: bool,
    pub evenodd: bool,
    pub stroking_color: Option<P::Color>,
    pub non_stroking_color: Option<P::Color>,
    pub original_path: Option<Vec<OriginalPathSegment>>,
    /// `(dash, phase)`, the raw operands of `d`.
    pub dashing_style: Option<(PDFObject, PDFObject)>,
    /// Attributes set later (`mcid`, `tag`), after the others in `__dict__`.
    pub attrs: ExtraAttrs<P::Attr>,
}

impl<P: LTPayload> LTCurve<P> {
    /// `__dict__` keys, in order.
    pub const DICT_KEYS: [&'static str; 16] = [
        "x0",
        "y0",
        "x1",
        "y1",
        "width",
        "height",
        "bbox",
        "pts",
        "linewidth",
        "stroke",
        "fill",
        "evenodd",
        "stroking_color",
        "non_stroking_color",
        "original_path",
        "dashing_style",
    ];

    // pdfminer/layout.py:215 LTCurve.__init__
    /// `LTCurve(...)`: the class call, then `__init__`.
    pub fn new(
        linewidth: Num,
        pts: Vec<Point>,
        stroke: bool,
        fill: bool,
        evenodd: bool,
        stroking_color: Option<P::Color>,
        non_stroking_color: Option<P::Color>,
        original_path: Option<Vec<OriginalPathSegment>>,
        dashing_style: Option<(PDFObject, PDFObject)>,
    ) -> PyResult<LTCurve<P>> {
        let _call = class_call()?;
        LTCurve::init(
            CurveKind::Curve,
            linewidth,
            pts,
            stroke,
            fill,
            evenodd,
            stroking_color,
            non_stroking_color,
            original_path,
            dashing_style,
        )
    }

    // pdfminer/layout.py:215 LTCurve.__init__
    /// The body of `LTCurve.__init__` (its frame), for `kind`'s class:
    /// `LTComponent.__init__(self, get_bound(pts))`, then the attributes.
    fn init(
        kind: CurveKind,
        linewidth: Num,
        pts: Vec<Point>,
        stroke: bool,
        fill: bool,
        evenodd: bool,
        stroking_color: Option<P::Color>,
        non_stroking_color: Option<P::Color>,
        original_path: Option<Vec<OriginalPathSegment>>,
        dashing_style: Option<(PDFObject, PDFObject)>,
    ) -> PyResult<LTCurve<P>> {
        let _frame = py_frame()?;
        leaf_frame()?; // get_bound
        let comp = LTComponent::init(get_bound(&pts))?;
        Ok(LTCurve {
            kind,
            comp,
            pts,
            linewidth,
            stroke,
            fill,
            evenodd,
            stroking_color,
            non_stroking_color,
            original_path,
            dashing_style,
            attrs: ExtraAttrs::default(),
        })
    }

    // pdfminer/layout.py:248 LTLine.__init__
    /// `LTLine(...)`: the class call, `LTLine.__init__`, which calls
    /// `LTCurve.__init__` with the points `[p0, p1]`.
    pub fn new_line(
        linewidth: Num,
        p0: Point,
        p1: Point,
        stroke: bool,
        fill: bool,
        evenodd: bool,
        stroking_color: Option<P::Color>,
        non_stroking_color: Option<P::Color>,
        original_path: Option<Vec<OriginalPathSegment>>,
        dashing_style: Option<(PDFObject, PDFObject)>,
    ) -> PyResult<LTCurve<P>> {
        let _call = class_call()?;
        let _frame = py_frame()?;
        LTCurve::init(
            CurveKind::Line,
            linewidth,
            vec![p0, p1],
            stroke,
            fill,
            evenodd,
            stroking_color,
            non_stroking_color,
            original_path,
            dashing_style,
        )
    }

    // pdfminer/layout.py:281 LTRect.__init__
    /// `LTRect(...)`: the class call, `LTRect.__init__`, which calls
    /// `LTCurve.__init__` with the four corners of `bbox`.
    pub fn new_rect(
        linewidth: Num,
        bbox: Rect,
        stroke: bool,
        fill: bool,
        evenodd: bool,
        stroking_color: Option<P::Color>,
        non_stroking_color: Option<P::Color>,
        original_path: Option<Vec<OriginalPathSegment>>,
        dashing_style: Option<(PDFObject, PDFObject)>,
    ) -> PyResult<LTCurve<P>> {
        let _call = class_call()?;
        let _frame = py_frame()?;
        let [x0, y0, x1, y1] = bbox;
        let pts = vec![[x0.clone(), y0.clone()], [x1.clone(), y0], [x1, y1.clone()], [x0, y1]];
        LTCurve::init(
            CurveKind::Rect,
            linewidth,
            pts,
            stroke,
            fill,
            evenodd,
            stroking_color,
            non_stroking_color,
            original_path,
            dashing_style,
        )
    }

    // pdfminer/layout.py:238 LTCurve.get_pts
    pub fn get_pts(&self) -> PyResult<String> {
        let mut parts = Vec::with_capacity(self.pts.len());
        for [x, y] in &self.pts {
            parts.push(format!("{},{}", format_fixed(x, 3)?, format_fixed(y, 3)?));
        }
        Ok(parts.join(","))
    }

    /// The Python class name.
    pub fn class_name(&self) -> &'static str {
        match self.kind {
            CurveKind::Curve => "LTCurve",
            CurveKind::Line => "LTLine",
            CurveKind::Rect => "LTRect",
        }
    }
}

/// pdfminer/layout.py:308 `LTImage`: an image object.
#[derive(Clone)]
pub struct LTImage<P: LTPayload> {
    pub comp: LTComponent,
    pub name: String,
    pub stream: Rc<PDFStream>,
    pub srcsize: (PDFObject, PDFObject),
    pub imagemask: PDFObject,
    pub bits: PDFObject,
    /// Always a list: the stream's list, or a new list around its value.
    pub colorspace: PDFObject,
    /// Attributes set later (`mcid`, `tag`), after the others in `__dict__`.
    pub attrs: ExtraAttrs<P::Attr>,
}

impl<P: LTPayload> LTImage<P> {
    /// `__dict__` keys, in order.
    pub const DICT_KEYS: [&'static str; 13] = [
        "x0",
        "y0",
        "x1",
        "y1",
        "width",
        "height",
        "bbox",
        "name",
        "stream",
        "srcsize",
        "imagemask",
        "bits",
        "colorspace",
    ];

    // pdfminer/layout.py:314 LTImage.__init__
    /// `LTImage(name, stream, bbox)`: the class call, `__init__`; the four
    /// `stream.get_any(...)` are leaf calls (methods that do not resolve).
    pub fn new(name: String, stream: Rc<PDFStream>, bbox: Rect) -> PyResult<LTImage<P>> {
        let _call = class_call()?;
        let _frame = py_frame()?;
        let comp = LTComponent::init(bbox)?;
        leaf_frame()?; // PDFStream.get_any
        let srcsize =
            (stream.get_any(&["W", "Width"], PDFObject::Null), stream.get_any(&["H", "Height"], PDFObject::Null));
        let imagemask = stream.get_any(&["IM", "ImageMask"], PDFObject::Null);
        let bits = stream.get_any(&["BPC", "BitsPerComponent"], PDFObject::Int(PyInt::Small(1)));
        let mut colorspace = stream.get_any(&["CS", "ColorSpace"], PDFObject::Null);
        if !matches!(colorspace, PDFObject::List(_)) {
            colorspace = PDFObject::List(PDFList::new(vec![colorspace]));
        }
        Ok(LTImage { comp, name, stream, srcsize, imagemask, bits, colorspace, attrs: ExtraAttrs::default() })
    }
}

/// pdfminer/layout.py:332 `LTAnno`: a "virtual" character inserted by the
/// analysis (a space between words, a newline after a line).
#[derive(Clone, Debug)]
pub struct LTAnno {
    pub _text: PyStr,
}

impl LTAnno {
    /// `__dict__` keys.
    pub const DICT_KEYS: [&'static str; 1] = ["_text"];

    // pdfminer/layout.py:340 LTAnno.__init__
    pub fn new(text: &str) -> LTAnno {
        LTAnno { _text: PyStr::new(text) }
    }

    // pdfminer/layout.py:343 LTAnno.get_text
    pub fn get_text(&self) -> &PyStr {
        &self._text
    }
}

/// pdfminer/layout.py:347 `LTChar`: an actual letter of the text.
#[derive(Clone)]
pub struct LTChar<P: LTPayload> {
    /// A Python `str`: `chr(cid)` of an `IdentityUnicodeMap` can be a lone
    /// surrogate.
    pub _text: PyStr,
    pub matrix: Matrix,
    pub fontname: P::FontName,
    pub ncs: Rc<PDFColorSpace>,
    pub graphicstate: P::GraphicState,
    pub adv: Num,
    pub upright: bool,
    pub comp: LTComponent,
    pub size: Num,
    /// Attributes set later (`mcid`, `tag`), after the others in `__dict__`.
    pub attrs: ExtraAttrs<P::Attr>,
}

impl<P: LTPayload> LTChar<P> {
    /// `__dict__` keys, in order.
    pub const DICT_KEYS: [&'static str; 15] = [
        "_text",
        "matrix",
        "fontname",
        "ncs",
        "graphicstate",
        "adv",
        "upright",
        "x0",
        "y0",
        "x1",
        "y1",
        "width",
        "height",
        "bbox",
        "size",
    ];

    // pdfminer/layout.py:350 LTChar.__init__
    /// `LTChar(...)`: the class call, then `__init__`. `textwidth` is what
    /// `font.char_width(cid)` returned (a number, or a sequence for a Type 3
    /// font whose `/FontMatrix` holds one, which `* fontsize * scaling`
    /// rejects with Python's exception: the interpreter's `scaling` is a
    /// float, `textstate.scaling * 0.01`). `font.is_vertical()` is a leaf
    /// call (a method), `font.get_descent()` takes its frames itself.
    pub fn new(
        matrix: Matrix,
        font: &dyn LTCharFont<P>,
        fontsize: Num,
        scaling: Num,
        rise: Num,
        text: impl Into<PyStr>,
        textwidth: Operand,
        textdisp: CharDisp,
        ncs: Rc<PDFColorSpace>,
        graphicstate: P::GraphicState,
    ) -> PyResult<LTChar<P>> {
        let _call = class_call()?;
        let _frame = py_frame()?;
        let fontname = font.fontname();
        let adv = match textwidth.mul(&Operand::Num(fontsize.clone()))?.mul(&Operand::Num(scaling.clone()))? {
            Operand::Num(adv) => adv,
            // a sequence repeated by an int font size and an int scaling,
            // which the interpreter never passes (its scaling is a float)
            other => {
                return Err(exc::TypeError(format!(
                    "LTChar.adv of type '{}': an int scaling is not supported",
                    other.type_name()
                )))
            }
        };
        // compute the boundary rectangle.
        leaf_frame()?; // font.is_vertical()
        let bbox: Rect = if font.is_vertical() {
            // vertical
            let CharDisp::Vertical(vx, vy) = textdisp else {
                return Err(exc::AssertionError(""));
            };
            let milli = Num::Float(0.001);
            let vx = match vx {
                IterItem::Obj(PDFObject::Null) => fontsize.mul(&Num::Float(0.5))?,
                vx => match item_mul(&vx, &fontsize)? {
                    Some(product) => product.mul(&milli)?,
                    // a repeated sequence times a float
                    None => return Err(exc::TypeError("can't multiply sequence by non-int of type 'float'")),
                },
            };
            let Some(vy_num) = (match &vy {
                IterItem::Obj(o) => o.as_num(),
                IterItem::Str(_) => None,
            }) else {
                return Err(exc::TypeError(format!(
                    "unsupported operand type(s) for -: 'int' and '{}'",
                    vy.type_name()
                )));
            };
            let vy = int(1000).sub(&vy_num)?.mul(&fontsize)?.mul(&milli)?;
            [vx.neg(), vy.add(&rise)?.add(&adv)?, vx.neg().add(&fontsize)?, vy.add(&rise)?]
        } else {
            // horizontal
            let descent = font.get_descent()?.mul(&fontsize)?;
            [int(0), descent.add(&rise)?, adv.clone(), descent.add(&rise)?.add(&fontsize)?]
        };
        let [a, b, c, d, _e, _f] = &matrix;
        let upright = a.mul(d)?.mul(&scaling)?.py_gt(&int(0)) && b.mul(c)?.py_le(&int(0));
        let [mut x0, mut y0, mut x1, mut y1] = apply_matrix_rect_counted(&matrix, &bbox)?;
        if x1.py_lt(&x0) {
            std::mem::swap(&mut x0, &mut x1);
        }
        if y1.py_lt(&y0) {
            std::mem::swap(&mut y0, &mut y1);
        }
        let comp = LTComponent::init([x0, y0, x1, y1])?;
        leaf_frame()?; // font.is_vertical()
        let size = if font.is_vertical() { comp.width.clone() } else { comp.height.clone() };
        Ok(LTChar {
            _text: text.into(),
            matrix,
            fontname,
            ncs,
            graphicstate,
            adv,
            upright,
            comp,
            size,
            attrs: ExtraAttrs::default(),
        })
    }

    // pdfminer/layout.py:404 LTChar.get_text
    pub fn get_text(&self) -> &PyStr {
        &self._text
    }
}

// ---------------------------------------------------------------------------
// Text containers

/// An element of a text line: pdfminer/layout.py:465 `TextLineElement`.
pub enum TextLineElement<P: LTPayload> {
    Char(Rc<LTChar<P>>),
    Anno(LTAnno),
}

impl<P: LTPayload> Clone for TextLineElement<P> {
    fn clone(&self) -> Self {
        match self {
            TextLineElement::Char(c) => TextLineElement::Char(c.clone()),
            TextLineElement::Anno(a) => TextLineElement::Anno(a.clone()),
        }
    }
}

impl<P: LTPayload> TextLineElement<P> {
    /// `obj.get_text()`, a frame (`LTChar.get_text`/`LTAnno.get_text`).
    fn get_text_into(&self, out: &mut PyStrBuilder) -> PyResult<()> {
        leaf_frame()?;
        out.push_str(match self {
            TextLineElement::Char(c) => c.get_text(),
            TextLineElement::Anno(a) => a.get_text(),
        });
        Ok(())
    }
}

/// Which `LTTextLine` subclass, with its private attribute.
#[derive(Clone, Debug)]
pub enum TextLineKind {
    /// pdfminer/layout.py:498 `LTTextLineHorizontal`: `_x1`, the right edge
    /// of the last char added.
    Horizontal { _x1: Num },
    /// pdfminer/layout.py:561 `LTTextLineVertical`: `_y0`, the bottom edge
    /// of the last char added.
    Vertical { _y0: Num },
}

/// pdfminer/layout.py:468 `LTTextLine`: the chars of a single text line
/// (and the `LTAnno` spaces and newline the analysis inserts).
pub struct LTTextLine<P: LTPayload> {
    pub comp: LTComponent,
    pub _objs: Vec<TextLineElement<P>>,
    pub word_margin: Num,
    pub kind: TextLineKind,
}

/// A shared text line.
pub type LineRef<P> = Rc<RefCell<LTTextLine<P>>>;

impl<P: LTPayload> LTTextLine<P> {
    /// `__dict__` keys of `LTTextLineHorizontal`, in order (`_y0` instead
    /// of `_x1` for `LTTextLineVertical`).
    pub const DICT_KEYS_HORIZONTAL: [&'static str; 10] =
        ["x0", "y0", "x1", "y1", "width", "height", "bbox", "_objs", "word_margin", "_x1"];
    pub const DICT_KEYS_VERTICAL: [&'static str; 10] =
        ["x0", "y0", "x1", "y1", "width", "height", "bbox", "_objs", "word_margin", "_y0"];

    // pdfminer/layout.py:499 LTTextLineHorizontal.__init__
    pub fn new_horizontal(word_margin: Num) -> PyResult<LTTextLine<P>> {
        let _frame = py_frame()?;
        Ok(LTTextLine {
            comp: LTComponent::expandable()?,
            _objs: Vec::new(),
            word_margin,
            kind: TextLineKind::Horizontal { _x1: inf() },
        })
    }

    // pdfminer/layout.py:562 LTTextLineVertical.__init__
    pub fn new_vertical(word_margin: Num) -> PyResult<LTTextLine<P>> {
        let _frame = py_frame()?;
        Ok(LTTextLine {
            comp: LTComponent::expandable()?,
            _objs: Vec::new(),
            word_margin,
            kind: TextLineKind::Vertical { _y0: neg_inf() },
        })
    }

    pub fn is_horizontal(&self) -> bool {
        matches!(self.kind, TextLineKind::Horizontal { .. })
    }

    /// The Python class name.
    pub fn class_name(&self) -> &'static str {
        match self.kind {
            TextLineKind::Horizontal { .. } => "LTTextLineHorizontal",
            TextLineKind::Vertical { .. } => "LTTextLineVertical",
        }
    }

    // pdfminer/layout.py:505 LTTextLineHorizontal.add
    // + :568 LTTextLineVertical.add
    /// Adds a char, after an `LTAnno(" ")` when it is farther from the
    /// previous char than `word_margin` times its size.
    pub fn add(&mut self, obj: &Rc<LTChar<P>>) -> PyResult<()> {
        let _frame = py_frame()?;
        let o = &obj.comp;
        let space = if truthy(&self.word_margin) {
            let margin = self.word_margin.mul(&py_max2(&o.width, &o.height))?;
            match &self.kind {
                TextLineKind::Horizontal { _x1 } => _x1.py_lt(&o.x0.sub(&margin)?),
                TextLineKind::Vertical { _y0 } => o.y1.add(&margin)?.py_lt(_y0),
            }
        } else {
            false
        };
        if space {
            self._objs.push(TextLineElement::Anno(LTAnno::new(" ")));
        }
        match &mut self.kind {
            TextLineKind::Horizontal { _x1 } => *_x1 = o.x1.clone(),
            TextLineKind::Vertical { _y0 } => *_y0 = o.y0.clone(),
        }
        // LTExpandableContainer.add
        self._objs.push(TextLineElement::Char(obj.clone()));
        let bbox = self.comp.union(o);
        self.comp.set_bbox(bbox)
    }

    // pdfminer/layout.py:459 LTTextContainer.get_text
    /// The text of the elements, with the frames of `get_text`, of
    /// `__iter__`, of the generator expression (resumed by `str.join`, from
    /// C) and of the `cast()` and `get_text()` it calls.
    pub fn get_text(&self) -> PyResult<PyStr> {
        let _frame = py_frame()?;
        leaf_frame()?; // LTContainer.__iter__
        leaf_frame()?; // creating the generator
        let mut out = PyStrBuilder::new();
        for obj in &self._objs {
            let _gen = c_called_frame()?;
            leaf_frame()?; // typing.cast
            obj.get_text_into(&mut out)?;
        }
        drop(c_called_frame()?); // StopIteration
        Ok(out.finish())
    }

    // pdfminer/layout.py:494 LTTextLine.is_empty
    pub fn is_empty(&self) -> PyResult<bool> {
        let _frame = py_frame()?;
        leaf_frame()?; // LTComponent.is_empty
        if self.comp.is_empty() {
            return Ok(true);
        }
        Ok(self.get_text()?.isspace())
    }

    // pdfminer/layout.py:482 LTTextLine.analyze
    /// Analyzes the elements (nothing to do) and appends `LTAnno("\n")`.
    pub fn analyze(&mut self, _laparams: &LAParams) -> PyResult<()> {
        let _frame = py_frame()?;
        for _obj in &self._objs {
            leaf_frame()?; // LTItem.analyze
        }
        leaf_frame()?; // LTAnno.__init__
        leaf_frame()?; // LTContainer.add
        self._objs.push(TextLineElement::Anno(LTAnno::new("\n")));
        Ok(())
    }

    // pdfminer/layout.py:513 LTTextLineHorizontal.find_neighbors
    // + :576 LTTextLineVertical.find_neighbors
    /// The lines of the plane of the same kind as this one, of the same
    /// height (width) within `ratio` times this line's height (width), and
    /// aligned on one edge or on the center within that tolerance.
    pub fn find_neighbors(&self, plane: &Plane<LineRef<P>>, ratio: &Num) -> PyResult<Vec<LineRef<P>>> {
        let _frame = py_frame()?;
        let s = &self.comp;
        let horizontal = self.is_horizontal();
        let (d, bbox) = if horizontal {
            let d = ratio.mul(&s.height)?;
            let bbox = [s.x0.clone(), s.y0.sub(&d)?, s.x1.clone(), s.y1.add(&d)?];
            (d, bbox)
        } else {
            let d = ratio.mul(&s.width)?;
            let bbox = [s.x0.sub(&d)?, s.y0.clone(), s.x1.add(&d)?, s.y1.clone()];
            (d, bbox)
        };
        let two = int(2);
        let mut out = Vec::new();
        for obj in plane.find(&bbox)? {
            let line = obj.borrow();
            if line.is_horizontal() != horizontal {
                continue;
            }
            let o = &line.comp;
            let keep = if horizontal {
                // _is_same_height_as and (_is_left_aligned_with or
                // _is_right_aligned_with or _is_centrally_aligned_with)
                o.height.sub(&s.height)?.abs().py_le(&d)
                    && (o.x0.sub(&s.x0)?.abs().py_le(&d)
                        || o.x1.sub(&s.x1)?.abs().py_le(&d)
                        || o.x0.add(&o.x1)?.truediv(&two)?.sub(&s.x0.add(&s.x1)?.truediv(&two)?)?.abs().py_le(&d))
            } else {
                // _is_same_width_as and (_is_lower_aligned_with or
                // _is_upper_aligned_with or _is_centrally_aligned_with)
                o.width.sub(&s.width)?.abs().py_le(&d)
                    && (o.y0.sub(&s.y0)?.abs().py_le(&d)
                        || o.y1.sub(&s.y1)?.abs().py_le(&d)
                        || o.y0.add(&o.y1)?.truediv(&two)?.sub(&s.y0.add(&s.y1)?.truediv(&two)?)?.abs().py_le(&d))
            };
            drop(line);
            if keep {
                out.push(obj);
            }
        }
        Ok(out)
    }
}

impl<P: LTPayload> PlaneItem for LineRef<P> {
    fn plane_bbox(&self) -> Rect {
        self.borrow().comp.bbox()
    }

    fn plane_id(&self) -> usize {
        Rc::as_ptr(self) as *const () as usize
    }
}

/// pdfminer/layout.py:624 `LTTextBox` (`LTTextBoxHorizontal`,
/// `LTTextBoxVertical`): text lines grouped by geometry.
pub struct LTTextBox<P: LTPayload> {
    pub comp: LTComponent,
    pub _objs: Vec<LineRef<P>>,
    /// The position assigned by `IndexAssigner` (-1 before).
    pub index: i64,
    pub vertical: bool,
}

/// A shared text box.
pub type BoxRef<P> = Rc<RefCell<LTTextBox<P>>>;

impl<P: LTPayload> LTTextBox<P> {
    /// `__dict__` keys, in order.
    pub const DICT_KEYS: [&'static str; 9] = ["x0", "y0", "x1", "y1", "width", "height", "bbox", "_objs", "index"];

    // pdfminer/layout.py:632 LTTextBox.__init__
    pub fn new(vertical: bool) -> PyResult<LTTextBox<P>> {
        let _frame = py_frame()?;
        Ok(LTTextBox { comp: LTComponent::expandable()?, _objs: Vec::new(), index: -1, vertical })
    }

    /// The Python class name.
    pub fn class_name(&self) -> &'static str {
        if self.vertical {
            "LTTextBoxVertical"
        } else {
            "LTTextBoxHorizontal"
        }
    }

    // pdfminer/layout.py:442 LTExpandableContainer.add
    pub fn add(&mut self, obj: &LineRef<P>) -> PyResult<()> {
        let _frame = py_frame()?;
        self._objs.push(obj.clone());
        let bbox = self.comp.union(&obj.borrow().comp);
        self.comp.set_bbox(bbox)
    }

    // pdfminer/layout.py:642 LTTextBox.get_writing_mode
    // + :651 LTTextBoxHorizontal.get_writing_mode and :660 LTTextBoxVertical.get_writing_mode
    pub fn get_writing_mode(&self) -> &'static str {
        if self.vertical {
            "tb-rl"
        } else {
            "lr-tb"
        }
    }

    // pdfminer/layout.py:459 LTTextContainer.get_text
    pub fn get_text(&self) -> PyResult<PyStr> {
        let _frame = py_frame()?;
        leaf_frame()?; // LTContainer.__iter__
        leaf_frame()?; // creating the generator
        let mut out = PyStrBuilder::new();
        for line in &self._objs {
            let _gen = c_called_frame()?;
            leaf_frame()?; // typing.cast
            out.push_str(&line.borrow().get_text()?);
        }
        drop(c_called_frame()?);
        Ok(out.finish())
    }

    // pdfminer/layout.py:647 LTTextBoxHorizontal.analyze
    // + :656 LTTextBoxVertical.analyze
    /// Analyzes the lines (`LTContainer.analyze`), then orders them from
    /// top to bottom (right to left for a vertical box).
    pub fn analyze(&mut self, laparams: &LAParams) -> PyResult<()> {
        let _frame = py_frame()?;
        {
            let _frame = py_frame()?; // LTContainer.analyze
            for line in &self._objs {
                line.borrow_mut().analyze(laparams)?;
            }
        }
        let mut keys = Vec::with_capacity(self._objs.len());
        for line in &self._objs {
            let _key = c_called_frame()?;
            let l = line.borrow();
            keys.push(if self.vertical { l.comp.x1.neg() } else { l.comp.y1.neg() });
        }
        list_sort(&mut self._objs, keys, |a, b| a.py_lt(b));
        Ok(())
    }
}

/// An element of a text group: pdfminer/layout.py:664 `TextGroupElement`.
pub enum TextGroupElement<P: LTPayload> {
    Box(BoxRef<P>),
    Group(GroupRef<P>),
}

impl<P: LTPayload> Clone for TextGroupElement<P> {
    fn clone(&self) -> Self {
        match self {
            TextGroupElement::Box(b) => TextGroupElement::Box(b.clone()),
            TextGroupElement::Group(g) => TextGroupElement::Group(g.clone()),
        }
    }
}

impl<P: LTPayload> TextGroupElement<P> {
    /// The bounding box attributes.
    pub fn comp(&self) -> Ref<'_, LTComponent> {
        match self {
            TextGroupElement::Box(b) => Ref::map(b.borrow(), |b| &b.comp),
            TextGroupElement::Group(g) => Ref::map(g.borrow(), |g| &g.comp),
        }
    }

    /// `isinstance(obj, (LTTextBoxVertical, LTTextGroupTBRL))`
    fn is_tbrl(&self) -> bool {
        match self {
            TextGroupElement::Box(b) => b.borrow().vertical,
            TextGroupElement::Group(g) => g.borrow().tbrl,
        }
    }

    /// `obj.analyze(laparams)`
    pub fn analyze(&self, laparams: &LAParams) -> PyResult<()> {
        match self {
            TextGroupElement::Box(b) => b.borrow_mut().analyze(laparams),
            TextGroupElement::Group(g) => g.borrow_mut().analyze(laparams),
        }
    }

    /// `obj.get_text()`
    pub fn get_text(&self) -> PyResult<PyStr> {
        match self {
            TextGroupElement::Box(b) => b.borrow().get_text(),
            TextGroupElement::Group(g) => g.borrow().get_text(),
        }
    }
}

impl<P: LTPayload> PlaneItem for TextGroupElement<P> {
    fn plane_bbox(&self) -> Rect {
        self.comp().bbox()
    }

    fn plane_id(&self) -> usize {
        match self {
            TextGroupElement::Box(b) => Rc::as_ptr(b) as *const () as usize,
            TextGroupElement::Group(g) => Rc::as_ptr(g) as *const () as usize,
        }
    }
}

/// pdfminer/layout.py:667 `LTTextGroup` (`LTTextGroupLRTB`,
/// `LTTextGroupTBRL`): two boxes or groups merged by `group_textboxes`.
pub struct LTTextGroup<P: LTPayload> {
    pub comp: LTComponent,
    pub _objs: Vec<TextGroupElement<P>>,
    /// `LTTextGroupTBRL` (else `LTTextGroupLRTB`).
    pub tbrl: bool,
}

/// A shared text group.
pub type GroupRef<P> = Rc<RefCell<LTTextGroup<P>>>;

impl<P: LTPayload> LTTextGroup<P> {
    /// `__dict__` keys, in order.
    pub const DICT_KEYS: [&'static str; 8] = ["x0", "y0", "x1", "y1", "width", "height", "bbox", "_objs"];

    // pdfminer/layout.py:668 LTTextGroup.__init__
    pub fn new(objs: Vec<TextGroupElement<P>>, tbrl: bool) -> PyResult<LTTextGroup<P>> {
        let _frame = py_frame()?;
        let mut group = LTTextGroup { comp: LTComponent::expandable()?, _objs: Vec::with_capacity(objs.len()), tbrl };
        for obj in objs {
            // LTExpandableContainer.add
            let bbox = group.comp.union(&obj.comp());
            group._objs.push(obj);
            group.comp.set_bbox(bbox)?;
        }
        Ok(group)
    }

    /// The Python class name.
    pub fn class_name(&self) -> &'static str {
        if self.tbrl {
            "LTTextGroupTBRL"
        } else {
            "LTTextGroupLRTB"
        }
    }

    // pdfminer/layout.py:459 LTTextContainer.get_text
    pub fn get_text(&self) -> PyResult<PyStr> {
        let _frame = py_frame()?;
        leaf_frame()?; // LTContainer.__iter__
        leaf_frame()?; // creating the generator
        let mut out = PyStrBuilder::new();
        for obj in &self._objs {
            let _gen = c_called_frame()?;
            leaf_frame()?; // typing.cast
            out.push_str(&obj.get_text()?);
        }
        drop(c_called_frame()?);
        Ok(out.finish())
    }

    // pdfminer/layout.py:674 LTTextGroupLRTB.analyze
    // + :686 LTTextGroupTBRL.analyze
    /// Analyzes the elements, then orders them from top-left to
    /// bottom-right (LRTB) or from top-right to bottom-left (TBRL),
    /// weighting the two directions by `boxes_flow`.
    pub fn analyze(&mut self, laparams: &LAParams) -> PyResult<()> {
        let _frame = py_frame()?;
        {
            let _frame = py_frame()?; // LTContainer.analyze
            for obj in &self._objs {
                obj.analyze(laparams)?;
            }
        }
        let Some(boxes_flow) = &laparams.boxes_flow else {
            return Err(exc::AssertionError(""));
        };
        let one = int(1);
        let mut keys = Vec::with_capacity(self._objs.len());
        for obj in &self._objs {
            let _key = c_called_frame()?;
            let o = obj.comp();
            keys.push(if self.tbrl {
                // -(1 + boxes_flow) * (obj.x0 + obj.x1) - (1 - boxes_flow) * obj.y1
                one.add(boxes_flow)?.neg().mul(&o.x0.add(&o.x1)?)?.sub(&one.sub(boxes_flow)?.mul(&o.y1)?)?
            } else {
                // (1 - boxes_flow) * obj.x0 - (1 + boxes_flow) * (obj.y0 + obj.y1)
                one.sub(boxes_flow)?.mul(&o.x0)?.sub(&one.add(boxes_flow)?.mul(&o.y0.add(&o.y1)?)?)?
            });
        }
        list_sort(&mut self._objs, keys, |a, b| a.py_lt(b));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Figures and pages

/// A layout object: what a page or a figure holds in `_objs`.
pub enum LTItem<P: LTPayload> {
    Char(Rc<LTChar<P>>),
    Curve(Rc<LTCurve<P>>),
    Image(Rc<LTImage<P>>),
    Figure(Rc<RefCell<LTFigure<P>>>),
    TextLine(LineRef<P>),
    TextBox(BoxRef<P>),
}

impl<P: LTPayload> Clone for LTItem<P> {
    fn clone(&self) -> Self {
        match self {
            LTItem::Char(x) => LTItem::Char(x.clone()),
            LTItem::Curve(x) => LTItem::Curve(x.clone()),
            LTItem::Image(x) => LTItem::Image(x.clone()),
            LTItem::Figure(x) => LTItem::Figure(x.clone()),
            LTItem::TextLine(x) => LTItem::TextLine(x.clone()),
            LTItem::TextBox(x) => LTItem::TextBox(x.clone()),
        }
    }
}

impl<P: LTPayload> LTItem<P> {
    /// The Python class name.
    pub fn class_name(&self) -> &'static str {
        match self {
            LTItem::Char(_) => "LTChar",
            LTItem::Curve(c) => c.class_name(),
            LTItem::Image(_) => "LTImage",
            LTItem::Figure(_) => "LTFigure",
            LTItem::TextLine(l) => l.borrow().class_name(),
            LTItem::TextBox(b) => b.borrow().class_name(),
        }
    }

    /// The keys of the object's `__dict__`, in order: the class's
    /// attributes, then those set later ([`ExtraAttrs`]).
    pub fn dict_keys(&self) -> Vec<&'static str> {
        let (keys, extra): (&[&str], _) = match self {
            LTItem::Char(c) => (&LTChar::<P>::DICT_KEYS, c.attrs.items()),
            LTItem::Curve(c) => (&LTCurve::<P>::DICT_KEYS, c.attrs.items()),
            LTItem::Image(i) => (&LTImage::<P>::DICT_KEYS, i.attrs.items()),
            LTItem::Figure(f) => (&LTFigure::<P>::DICT_KEYS, f.borrow().attrs.items()),
            LTItem::TextLine(l) => {
                if l.borrow().is_horizontal() {
                    (&LTTextLine::<P>::DICT_KEYS_HORIZONTAL, Vec::new())
                } else {
                    (&LTTextLine::<P>::DICT_KEYS_VERTICAL, Vec::new())
                }
            }
            LTItem::TextBox(_) => (&LTTextBox::<P>::DICT_KEYS, Vec::new()),
        };
        keys.iter().copied().chain(extra.into_iter().map(|(k, _)| k)).collect()
    }

    /// `setattr(obj, name, value)` for an attribute that is not one of the
    /// class's (pdfplumber's `cur_obj.mcid = ...`, `cur_obj.tag = ...`): on
    /// the raw objects of a page or a figure, the only ones pdfplumber tags.
    pub fn set_attr(&self, name: &'static str, value: P::Attr) {
        match self {
            LTItem::Char(c) => c.attrs.set(name, value),
            LTItem::Curve(c) => c.attrs.set(name, value),
            LTItem::Image(i) => i.attrs.set(name, value),
            LTItem::Figure(f) => f.borrow().attrs.set(name, value),
            LTItem::TextLine(_) | LTItem::TextBox(_) => {
                unreachable!("pdfplumber tags the objects of a page before its analysis")
            }
        }
    }

    /// `obj.analyze(laparams)`, as the page's analysis calls it on the
    /// objects that are not chars.
    fn analyze(&self, laparams: &LAParams, ids: &mut dyn IdSource<P>) -> PyResult<()> {
        match self {
            // LTItem.analyze: nothing to do
            LTItem::Char(_) | LTItem::Curve(_) | LTItem::Image(_) => leaf_frame(),
            LTItem::Figure(f) => f.borrow_mut().analyze_with_ids(laparams, ids),
            LTItem::TextLine(l) => l.borrow_mut().analyze(laparams),
            LTItem::TextBox(b) => b.borrow_mut().analyze(laparams),
        }
    }
}

/// Python's `id()` of the boxes and groups `group_textboxes` compares (see
/// the module documentation): the same value for the same object during
/// one analysis, a different one for each object alive at the same time.
pub trait IdSource<P: LTPayload> {
    fn id(&mut self, obj: &TextGroupElement<P>) -> u64;
}

/// The default [`IdSource`]: ids increase in the order objects are first
/// seen (a new group is seen when it is created).
#[derive(Default)]
pub struct CreationOrder {
    ids: HashMap<usize, u64>,
}

impl<P: LTPayload> IdSource<P> for CreationOrder {
    fn id(&mut self, obj: &TextGroupElement<P>) -> u64 {
        let n = self.ids.len() as u64;
        *self.ids.entry(obj.plane_id()).or_insert(n)
    }
}

/// pdfminer/layout.py:35 `IndexAssigner`: numbers the text boxes of a group
/// tree depth first.
pub struct IndexAssigner {
    pub index: i64,
}

impl IndexAssigner {
    // pdfminer/layout.py:36 IndexAssigner.__init__
    pub fn new(index: i64) -> IndexAssigner {
        IndexAssigner { index }
    }

    // pdfminer/layout.py:39 IndexAssigner.run
    pub fn run<P: LTPayload>(&mut self, obj: &TextGroupElement<P>) -> PyResult<()> {
        let _frame = py_frame()?;
        match obj {
            TextGroupElement::Box(b) => {
                b.borrow_mut().index = self.index;
                self.index += 1;
            }
            TextGroupElement::Group(g) => {
                leaf_frame()?; // LTContainer.__iter__
                let children = g.borrow()._objs.clone();
                for x in &children {
                    self.run(x)?;
                }
            }
        }
        Ok(())
    }
}

/// pdfminer/layout.py:697 `LTLayoutContainer`: the analysis of a page or a
/// figure.
pub struct LTLayoutContainer<P: LTPayload> {
    pub comp: LTComponent,
    pub _objs: Vec<LTItem<P>>,
    /// The top-level groups of the last analysis with `boxes_flow` (`None`
    /// before or without).
    pub groups: Option<Vec<TextGroupElement<P>>>,
}

/// The heap entries of `group_textboxes`:
/// `(skip_isany, d, id(obj1), id(obj2), obj1, obj2)`.
struct HeapEntry<P: LTPayload> {
    skip_isany: bool,
    d: Num,
    id1: u64,
    id2: u64,
    obj1: TextGroupElement<P>,
    obj2: TextGroupElement<P>,
}

impl<P: LTPayload> HeapEntry<P> {
    /// Python's `<` on the tuples: the first unequal field decides. Two
    /// entries never have the same pair of ids (each pair of objects is
    /// pushed once, or popped and pushed again), so the objects themselves,
    /// whose `<` raises `PDFValueError`, are never compared.
    fn lt(&self, other: &HeapEntry<P>) -> bool {
        if self.skip_isany != other.skip_isany {
            return !self.skip_isany;
        }
        if !self.d.py_eq(&other.d) {
            return self.d.py_lt(&other.d);
        }
        if self.id1 != other.id1 {
            return self.id1 < other.id1;
        }
        debug_assert!(self.id2 != other.id2, "two heap entries for one pair of objects");
        self.id2 < other.id2
    }
}

// The heap functions of CPython's _heapq module (the same algorithm as
// heapq.py).

/// `_siftdown(heap, startpos, pos)`
fn heap_siftdown<P: LTPayload>(heap: &mut [HeapEntry<P>], startpos: usize, mut pos: usize) {
    while pos > startpos {
        let parentpos = (pos - 1) >> 1;
        if !heap[pos].lt(&heap[parentpos]) {
            break;
        }
        heap.swap(pos, parentpos);
        pos = parentpos;
    }
}

/// `_siftup(heap, pos)`
fn heap_siftup<P: LTPayload>(heap: &mut [HeapEntry<P>], mut pos: usize) {
    let endpos = heap.len();
    let startpos = pos;
    let limit = endpos >> 1;
    while pos < limit {
        let mut childpos = 2 * pos + 1;
        if childpos + 1 < endpos && !heap[childpos].lt(&heap[childpos + 1]) {
            childpos += 1;
        }
        heap.swap(pos, childpos);
        pos = childpos;
    }
    heap_siftdown(heap, startpos, pos)
}

/// `heapq.heapify`: `_siftup` from each parent, the last one first. For more
/// than 2500 entries CPython's `heapify` visits the parents in another order
/// (`cache_friendly_heapify`, a subtree as soon as its children's subtrees
/// are heaps), with the same result: `_siftup(i)` only moves entries within
/// the subtree of `i`, whose children's subtrees are already heaps in both
/// orders, so each subtree ends in the same state.
///
/// Sifting up from a leaf (`i >= len / 2`) does nothing (`limit = len / 2`,
/// then `_siftdown` from its own position), which makes a larger bound
/// (`len * 2`) an equivalent mutant.
fn heapify<P: LTPayload>(heap: &mut [HeapEntry<P>]) {
    for i in (0..heap.len() / 2).rev() {
        heap_siftup(heap, i);
    }
}

/// `heapq.heappush`
fn heappush<P: LTPayload>(heap: &mut Vec<HeapEntry<P>>, item: HeapEntry<P>) {
    heap.push(item);
    let last = heap.len() - 1;
    heap_siftdown(heap, 0, last)
}

/// `heapq.heappop` (of a non-empty heap)
fn heappop<P: LTPayload>(heap: &mut Vec<HeapEntry<P>>) -> HeapEntry<P> {
    let lastelt = heap.pop().expect("a non-empty heap");
    if heap.is_empty() {
        return lastelt;
    }
    let returnitem = std::mem::replace(&mut heap[0], lastelt);
    heap_siftup(heap, 0);
    returnitem
}

/// `dist(obj1, obj2)` of `group_textboxes`: the area of the bounding
/// rectangle of both, less their own areas (may be negative).
fn dist(obj1: &LTComponent, obj2: &LTComponent) -> PyResult<Num> {
    let _frame = py_frame()?;
    let x0 = py_min2(&obj1.x0, &obj2.x0);
    let y0 = py_min2(&obj1.y0, &obj2.y0);
    let x1 = py_max2(&obj1.x1, &obj2.x1);
    let y1 = py_max2(&obj1.y1, &obj2.y1);
    x1.sub(&x0)?.mul(&y1.sub(&y0)?)?.sub(&obj1.width.mul(&obj1.height)?)?.sub(&obj2.width.mul(&obj2.height)?)
}

impl<P: LTPayload> LTLayoutContainer<P> {
    // pdfminer/layout.py:698 LTLayoutContainer.__init__
    pub fn new(bbox: Rect) -> PyResult<LTLayoutContainer<P>> {
        Ok(LTLayoutContainer { comp: LTComponent::new(bbox)?, _objs: Vec::new(), groups: None })
    }

    // pdfminer/layout.py:698 LTLayoutContainer.__init__
    /// `LTLayoutContainer.__init__(self, bbox)` called by a subclass's
    /// `__init__` (`LTFigure`, `LTPage`): its frame, `LTContainer.__init__`'s
    /// and `LTComponent.__init__`'s.
    pub fn init(bbox: Rect) -> PyResult<LTLayoutContainer<P>> {
        let _frame = py_frame()?;
        let _container = py_frame()?; // LTContainer.__init__
        Ok(LTLayoutContainer { comp: LTComponent::init(bbox)?, _objs: Vec::new(), groups: None })
    }

    /// `self.add(obj)` (`LTContainer.add`), without its frame (see
    /// [`Self::add_counted`]).
    pub fn add(&mut self, obj: LTItem<P>) {
        self._objs.push(obj);
    }

    // pdfminer/layout.py:424 LTContainer.add
    /// `self.add(obj)` called from Python code (the converter): a leaf
    /// frame, then the append.
    pub fn add_counted(&mut self, obj: LTItem<P>) -> PyResult<()> {
        leaf_frame()?;
        self._objs.push(obj);
        Ok(())
    }

    // pdfminer/layout.py:703 LTLayoutContainer.group_objects
    /// Groups the chars into text lines: consecutive chars that are aligned
    /// horizontally (vertically, with `detect_vertical`) and close enough
    /// join the same line.
    pub fn group_objects(&self, laparams: &LAParams, objs: &[Rc<LTChar<P>>]) -> PyResult<Vec<LineRef<P>>> {
        let _gen = c_called_frame()?; // a generator consumed by list()
        let mut out = Vec::new();
        let mut obj0: Option<&Rc<LTChar<P>>> = None;
        let mut line: Option<LTTextLine<P>> = None;
        for obj1 in objs {
            if let Some(obj0) = obj0 {
                let (c0, c1) = (&obj0.comp, &obj1.comp);
                // halign: obj0 and obj1 is horizontally aligned.
                let halign = c0.is_voverlap(c1)
                    && py_min2(&c0.height, &c1.height).mul(&laparams.line_overlap)?.py_lt(&c0.voverlap(c1)?)
                    && c0.hdistance(c1)?.py_lt(&py_max2(&c0.width, &c1.width).mul(&laparams.char_margin)?);
                // valign: obj0 and obj1 is vertically aligned.
                let valign = laparams.detect_vertical
                    && c0.is_hoverlap(c1)
                    && py_min2(&c0.width, &c1.width).mul(&laparams.line_overlap)?.py_lt(&c0.hoverlap(c1)?)
                    && c0.vdistance(c1)?.py_lt(&py_max2(&c0.height, &c1.height).mul(&laparams.char_margin)?);
                let extends = match &line {
                    Some(l) => (halign && l.is_horizontal()) || (valign && !l.is_horizontal()),
                    None => false,
                };
                if extends {
                    line.as_mut().expect("a line").add(obj1)?;
                } else if let Some(l) = line.take() {
                    out.push(Rc::new(RefCell::new(l)));
                } else if valign && !halign {
                    let mut l = LTTextLine::new_vertical(laparams.word_margin.clone())?;
                    l.add(obj0)?;
                    l.add(obj1)?;
                    line = Some(l);
                } else if halign && !valign {
                    let mut l = LTTextLine::new_horizontal(laparams.word_margin.clone())?;
                    l.add(obj0)?;
                    l.add(obj1)?;
                    line = Some(l);
                } else {
                    let mut l = LTTextLine::new_horizontal(laparams.word_margin.clone())?;
                    l.add(obj0)?;
                    out.push(Rc::new(RefCell::new(l)));
                }
            }
            obj0 = Some(obj1);
        }
        let line = match line {
            Some(l) => l,
            None => {
                let mut l = LTTextLine::new_horizontal(laparams.word_margin.clone())?;
                let Some(obj0) = obj0 else {
                    return Err(exc::AssertionError(""));
                };
                l.add(obj0)?;
                l
            }
        };
        out.push(Rc::new(RefCell::new(line)));
        Ok(out)
    }

    // pdfminer/layout.py:780 LTLayoutContainer.group_textlines
    /// Groups neighboring lines into text boxes (the empty ones are left
    /// out), in the order of their first line.
    pub fn group_textlines(&self, laparams: &LAParams, lines: &[LineRef<P>]) -> PyResult<Vec<BoxRef<P>>> {
        let _gen = c_called_frame()?; // a generator consumed by list()
        let mut plane: Plane<LineRef<P>> = Plane::new(&self.comp.bbox(), 50);
        plane.extend(lines)?;
        let mut boxes: FxHashMap<usize, BoxRef<P>> = FxHashMap::default();
        for line in lines {
            let neighbors = line.borrow().find_neighbors(&plane, &laparams.line_margin)?;
            let mut members = vec![line.clone()];
            for obj1 in neighbors {
                members.push(obj1.clone());
                if let Some(b) = boxes.remove(&obj1.plane_id()) {
                    members.extend(b.borrow()._objs.iter().cloned());
                }
            }
            let vertical = !line.borrow().is_horizontal();
            let b = Rc::new(RefCell::new(LTTextBox::new(vertical)?));
            for obj in uniq(members, PlaneItem::plane_id) {
                b.borrow_mut().add(&obj)?;
                boxes.insert(obj.plane_id(), b.clone());
            }
        }
        let mut done = FxHashSet::default();
        let mut out = Vec::new();
        for line in lines {
            let Some(b) = boxes.get(&line.plane_id()) else { continue };
            if !done.insert(Rc::as_ptr(b) as *const () as usize) {
                continue;
            }
            if !b.borrow().comp.is_empty() {
                out.push(b.clone());
            }
        }
        Ok(out)
    }

    // pdfminer/layout.py:814 LTLayoutContainer.group_textboxes
    /// Groups the text boxes hierarchically: the pair at the smallest
    /// distance (see [`dist`]) with nothing in between is merged first, and
    /// the new group then counts as one object. Returns the top-level
    /// groups (one, unless there was no box).
    pub fn group_textboxes(
        &self,
        _laparams: &LAParams,
        boxes: &[BoxRef<P>],
        ids: &mut dyn IdSource<P>,
    ) -> PyResult<Vec<TextGroupElement<P>>> {
        let _frame = py_frame()?;
        let mut plane: Plane<TextGroupElement<P>> = Plane::new(&self.comp.bbox(), 50);
        let boxes: Vec<TextGroupElement<P>> = boxes.iter().cloned().map(TextGroupElement::Box).collect();

        // isany: is there any other object between obj1 and obj2?
        let isany = |plane: &Plane<TextGroupElement<P>>,
                     obj1: &TextGroupElement<P>,
                     obj2: &TextGroupElement<P>|
         -> PyResult<bool> {
            let _frame = py_frame()?;
            let bbox = obj1.comp().union(&obj2.comp());
            let (id1, id2) = (obj1.plane_id(), obj2.plane_id());
            let mut found = false;
            plane.find_each(&bbox, |obj| {
                found = obj.plane_id() != id1 && obj.plane_id() != id2;
                Ok(!found)
            })?;
            Ok(found)
        };

        let mut dists: Vec<HeapEntry<P>> = Vec::new();
        for (i, box1) in boxes.iter().enumerate() {
            for box2 in &boxes[i + 1..] {
                let d = dist(&box1.comp(), &box2.comp())?;
                let (id1, id2) = (ids.id(box1), ids.id(box2));
                dists.push(HeapEntry { skip_isany: false, d, id1, id2, obj1: box1.clone(), obj2: box2.clone() });
            }
        }
        heapify(&mut dists);

        plane.extend(&boxes)?;
        let mut done: FxHashSet<u64> = FxHashSet::default();
        while !dists.is_empty() {
            let HeapEntry { skip_isany, d, id1, id2, obj1, obj2 } = heappop(&mut dists);
            // Skip objects that are already merged
            if !done.contains(&id1) && !done.contains(&id2) {
                if !skip_isany && isany(&plane, &obj1, &obj2)? {
                    heappush(&mut dists, HeapEntry { skip_isany: true, d, id1, id2, obj1, obj2 });
                    continue;
                }
                let tbrl = obj1.is_tbrl() || obj2.is_tbrl();
                let group = TextGroupElement::Group(Rc::new(RefCell::new(LTTextGroup::new(
                    vec![obj1.clone(), obj2.clone()],
                    tbrl,
                )?)));
                plane.remove(&obj1)?;
                plane.remove(&obj2)?;
                done.insert(id1);
                done.insert(id2);
                let others: Vec<TextGroupElement<P>> = plane.iter().cloned().collect();
                for other in others {
                    let d = dist(&group.comp(), &other.comp())?;
                    let (gid, oid) = (ids.id(&group), ids.id(&other));
                    heappush(
                        &mut dists,
                        HeapEntry { skip_isany: false, d, id1: gid, id2: oid, obj1: group.clone(), obj2: other },
                    );
                }
                plane.add(&group)?;
            }
        }
        Ok(plane.iter().cloned().collect())
    }

    // pdfminer/layout.py:907 LTLayoutContainer.analyze
    /// The layout analysis: chars grouped into lines, lines into boxes,
    /// boxes into groups (with `boxes_flow`) whose order numbers the boxes;
    /// `_objs` becomes the boxes, then the other objects (analyzed), then
    /// the empty lines.
    pub fn analyze(&mut self, laparams: &LAParams) -> PyResult<()> {
        self.analyze_with_ids(laparams, &mut CreationOrder::default())
    }

    /// [`Self::analyze`] with the `id()` of the boxes and groups taken from
    /// `ids`.
    pub fn analyze_with_ids(&mut self, laparams: &LAParams, ids: &mut dyn IdSource<P>) -> PyResult<()> {
        let _frame = py_frame()?;
        // textobjs is a list of LTChar objects, i.e. it has all the
        // individual characters in the page.
        let (textobjs, otherobjs) = {
            let _frame = py_frame()?; // fsplit
            leaf_frame()?; // LTContainer.__iter__
            fsplit(
                |obj| {
                    leaf_frame()?; // the lambda
                    Ok(matches!(obj, LTItem::Char(_)))
                },
                self._objs.iter().cloned(),
            )?
        };
        for obj in &otherobjs {
            obj.analyze(laparams, ids)?;
        }
        if textobjs.is_empty() {
            return Ok(());
        }
        let textobjs: Vec<Rc<LTChar<P>>> = textobjs
            .into_iter()
            .map(|obj| match obj {
                LTItem::Char(c) => c,
                _ => unreachable!("fsplit kept the chars"),
            })
            .collect();
        let textlines = self.group_objects(laparams, &textobjs)?;
        let (empties, textlines) = {
            let _frame = py_frame()?; // fsplit
            fsplit(
                |obj: &LineRef<P>| {
                    let _frame = py_frame()?; // the lambda
                    obj.borrow().is_empty()
                },
                textlines,
            )?
        };
        for obj in &empties {
            obj.borrow_mut().analyze(laparams)?;
        }
        let mut textboxes = self.group_textlines(laparams, &textlines)?;
        if laparams.boxes_flow.is_none() {
            for textbox in &textboxes {
                textbox.borrow_mut().analyze(laparams)?;
            }
            let mut keys = Vec::with_capacity(textboxes.len());
            for b in &textboxes {
                let _key = c_called_frame()?; // getkey
                let b = b.borrow();
                keys.push(if b.vertical {
                    [int(0), b.comp.x1.neg(), b.comp.y0.neg()]
                } else {
                    [int(1), b.comp.y0.neg(), b.comp.x0.clone()]
                });
            }
            list_sort(&mut textboxes, keys, |a, b| tuple_lt(a, b));
        } else {
            let groups = self.group_textboxes(laparams, &textboxes, ids)?;
            self.groups = Some(groups.clone());
            let mut assigner = IndexAssigner::new(0);
            for group in &groups {
                group.analyze(laparams)?;
                assigner.run(group)?;
            }
            let mut keys = Vec::with_capacity(textboxes.len());
            for b in &textboxes {
                let _key = c_called_frame()?;
                keys.push(b.borrow().index);
            }
            list_sort(&mut textboxes, keys, |a, b| a < b);
        }
        let mut objs: Vec<LTItem<P>> = textboxes.into_iter().map(LTItem::TextBox).collect();
        objs.extend(otherobjs);
        objs.extend(empties.into_iter().map(LTItem::TextLine));
        self._objs = objs;
        Ok(())
    }
}

/// pdfminer/layout.py:945 `LTFigure`: the area of a Form XObject (or an
/// image XObject), which can contain other figures.
pub struct LTFigure<P: LTPayload> {
    pub name: String,
    pub matrix: Matrix,
    pub layout: LTLayoutContainer<P>,
    /// Attributes set later (`mcid`, `tag`), after the others in `__dict__`.
    pub attrs: ExtraAttrs<P::Attr>,
}

impl<P: LTPayload> LTFigure<P> {
    /// `__dict__` keys, in order.
    pub const DICT_KEYS: [&'static str; 11] =
        ["name", "matrix", "x0", "y0", "x1", "y1", "width", "height", "bbox", "_objs", "groups"];

    // pdfminer/layout.py:953 LTFigure.__init__
    /// `bbox` is `(x, y, w, h)`, the form's box, mapped through `matrix`.
    /// For numbers, without the frames of the class call: the converter
    /// makes the figure of a Form XObject with [`Self::new_obj`], which
    /// counts them; this one rebuilds the figures of a dumped page
    /// (`tests/layout_unit.rs`).
    pub fn new(name: String, bbox: [Num; 4], matrix: Matrix) -> PyResult<LTFigure<P>> {
        let [x, y, w, h] = bbox;
        let rect = [x.clone(), y.clone(), x.add(&w)?, y.add(&h)?];
        let bbox = apply_matrix_rect(&matrix, &rect)?;
        Ok(LTFigure { name, matrix, layout: LTLayoutContainer::new(bbox)?, attrs: ExtraAttrs::default() })
    }

    // pdfminer/layout.py:953 LTFigure.__init__
    /// The figure of a Form XObject, as `PDFLayoutAnalyzer.begin_figure`
    /// makes it from what `pdfinterp.do_Do` hands over unchecked:
    /// `LTFigure(name, list_value(xobj["BBox"]), mult_matrix(matrix, ctm))`,
    /// `matrix` being [`crate::utils::mult_matrix_obj`]'s. Python's
    /// `ValueError` for a box of another length than 4, and the `TypeError`,
    /// `OverflowError` or `MemoryError` of the arithmetic on items that are
    /// not numbers, are raised where Python raises them. When it succeeds,
    /// the matrix and the box are numbers (see [`ObjMatrix`]).
    pub fn new_obj(name: String, bbox: &[PDFObject], matrix: &ObjMatrix) -> PyResult<LTFigure<P>> {
        let _call = class_call()?;
        let _frame = py_frame()?;
        let [x, y, w, h] = unpack::<_, 4>(bbox)?.map(|v| Operand::from_obj(&v));
        let rect = [x.clone(), y.clone(), x.add(&w)?, y.add(&h)?];
        let bbox = {
            let _frame = py_frame()?; // apply_matrix_rect
            leaf_frame()?; // apply_matrix_pt
            matrix.apply_rect(&rect)?
        };
        let matrix = matrix.to_matrix().expect("apply_matrix_pt raises on a matrix item that is not a number");
        Ok(LTFigure { name, matrix, layout: LTLayoutContainer::init(bbox)?, attrs: ExtraAttrs::default() })
    }

    // pdfminer/layout.py:968 LTFigure.analyze
    /// Analyzes the figure's contents only with `all_texts`, with the ids of
    /// the page's analysis (the figure is analyzed as part of its page).
    pub fn analyze_with_ids(&mut self, laparams: &LAParams, ids: &mut dyn IdSource<P>) -> PyResult<()> {
        let _frame = py_frame()?;
        if !laparams.all_texts {
            return Ok(());
        }
        self.layout.analyze_with_ids(laparams, ids)
    }
}

/// pdfminer/layout.py:974 `LTPage`: an entire page.
pub struct LTPage<P: LTPayload> {
    pub layout: LTLayoutContainer<P>,
    pub pageid: PyInt,
    pub rotate: Num,
}

impl<P: LTPayload> LTPage<P> {
    /// `__dict__` keys, in order.
    pub const DICT_KEYS: [&'static str; 11] =
        ["x0", "y0", "x1", "y1", "width", "height", "bbox", "_objs", "groups", "pageid", "rotate"];

    // pdfminer/layout.py:981 LTPage.__init__
    /// `LTPage(pageid, bbox, rotate)`: the class call, then `__init__`.
    pub fn new(pageid: PyInt, bbox: Rect, rotate: Num) -> PyResult<LTPage<P>> {
        let _call = class_call()?;
        let _frame = py_frame()?;
        Ok(LTPage { layout: LTLayoutContainer::init(bbox)?, pageid, rotate })
    }

    /// `page.analyze(laparams)` (`LTLayoutContainer.analyze`).
    pub fn analyze(&mut self, laparams: &LAParams) -> PyResult<()> {
        self.layout.analyze(laparams)
    }

    /// [`Self::analyze`] with the ids of an [`IdSource`].
    pub fn analyze_with_ids(&mut self, laparams: &LAParams, ids: &mut dyn IdSource<P>) -> PyResult<()> {
        self.layout.analyze_with_ids(laparams, ids)
    }
}
