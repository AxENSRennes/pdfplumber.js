//! Port of `pdfplumber/utils/geometry.py`.
//!
//! Values are read with Python's operators on [`PyValue`]: coordinates keep
//! their `int`/`float` type, and the evaluation order (hence the first
//! exception raised) is upstream's.

use pycompat::{exc, PyResult};

use crate::obj::{bbox_repr, Bbox, Hot, Obj, PyValue};
use crate::utils::clustering::cluster_objects;

/// `max(a, b)`: `b` only when `b > a`.
pub fn max2(a: &PyValue, b: &PyValue) -> PyResult<PyValue> {
    Ok(if b.gt(a)? { b.clone() } else { a.clone() })
}

/// `min(a, b)`: `b` only when `b < a`.
pub fn min2(a: &PyValue, b: &PyValue) -> PyResult<PyValue> {
    Ok(if b.lt(a)? { b.clone() } else { a.clone() })
}

// pdfplumber/utils/geometry.py:9 objects_to_rect
pub fn objects_to_rect(objects: &[Obj]) -> PyResult<Obj> {
    Ok(bbox_to_rect(&objects_to_bbox(objects)?))
}

// pdfplumber/utils/geometry.py:18 objects_to_bbox
pub fn objects_to_bbox(objects: &[Obj]) -> PyResult<Bbox> {
    let bboxes = objects.iter().map(obj_to_bbox).collect::<PyResult<Vec<Bbox>>>()?;
    merge_bboxes(&bboxes)
}

/// [`objects_to_bbox`] over borrowed objects.
pub fn objects_to_bbox_refs<'a>(objects: impl IntoIterator<Item = &'a Obj>) -> PyResult<Bbox> {
    let bboxes = objects.into_iter().map(obj_to_bbox).collect::<PyResult<Vec<Bbox>>>()?;
    merge_bboxes(&bboxes)
}

// pdfplumber/utils/geometry.py:26 bbox_getter (itemgetter("x0", "top", "x1", "bottom"))
// + :29 obj_to_bbox
pub fn obj_to_bbox(obj: &Obj) -> PyResult<Bbox> {
    Ok([obj.hot(Hot::X0)?.clone(), obj.hot(Hot::Top)?.clone(), obj.hot(Hot::X1)?.clone(), obj.hot(Hot::Bottom)?.clone()])
}

// pdfplumber/utils/geometry.py:37 bbox_to_rect
pub fn bbox_to_rect(bbox: &Bbox) -> Obj {
    Obj::from_pairs([
        ("x0", bbox[0].clone()),
        ("top", bbox[1].clone()),
        ("x1", bbox[2].clone()),
        ("bottom", bbox[3].clone()),
    ])
}

// pdfplumber/utils/geometry.py:45 merge_bboxes
pub fn merge_bboxes(bboxes: &[Bbox]) -> PyResult<Bbox> {
    if bboxes.is_empty() {
        // x0, top, x1, bottom = zip(*[])
        return Err(exc::ValueError("not enough values to unpack (expected 4, got 0)"));
    }
    let col = |i: usize| bboxes.iter().map(move |b| &b[i]);
    Ok([crate::obj::py_min(col(0))?, crate::obj::py_min(col(1))?, crate::obj::py_max(col(2))?, crate::obj::py_max(col(3))?])
}

// pdfplumber/utils/geometry.py:54 get_bbox_overlap
pub fn get_bbox_overlap(a: &Bbox, b: &Bbox) -> PyResult<Option<Bbox>> {
    let [a_left, a_top, a_right, a_bottom] = a;
    let [b_left, b_top, b_right, b_bottom] = b;
    let o_left = max2(a_left, b_left)?;
    let o_right = min2(a_right, b_right)?;
    let o_bottom = min2(a_bottom, b_bottom)?;
    let o_top = max2(a_top, b_top)?;
    let o_width = o_right.sub(&o_left)?;
    let o_height = o_bottom.sub(&o_top)?;
    let zero = PyValue::from(0i64);
    if o_height.ge(&zero)? && o_width.ge(&zero)? && o_height.add(&o_width)?.gt(&zero)? {
        Ok(Some([o_left, o_top, o_right, o_bottom]))
    } else {
        Ok(None)
    }
}

// pdfplumber/utils/geometry.py:69 calculate_area
pub fn calculate_area(bbox: &Bbox) -> PyResult<PyValue> {
    let [left, top, right, bottom] = bbox;
    if left.gt(right)? || top.gt(bottom)? {
        return Err(exc::ValueError(format!("{} has a negative width or height.", bbox_repr(bbox))));
    }
    right.sub(left)?.mul(&bottom.sub(top)?)
}

// pdfplumber/utils/geometry.py:76 clip_obj
pub fn clip_obj(obj: &Obj, bbox: &Bbox) -> PyResult<Option<Obj>> {
    let Some(overlap) = get_bbox_overlap(&obj_to_bbox(obj)?, bbox)? else {
        return Ok(None);
    };
    let mut copy = obj.clone();
    for (attr, v) in ["x0", "top", "x1", "bottom"].iter().zip(overlap.iter()) {
        copy.set(attr, v.clone());
    }
    let diff = overlap[1].sub(obj.hot(Hot::Top)?)?;
    if copy.contains("doctop") {
        copy.set("doctop", obj.hot(Hot::Doctop)?.add(&diff)?);
    }
    let width = copy.hot(Hot::X1)?.sub(copy.hot(Hot::X0)?)?;
    copy.set("width", width);
    let height = copy.hot(Hot::Bottom)?.sub(copy.hot(Hot::Top)?)?;
    copy.set("height", height);
    Ok(Some(copy))
}

// pdfplumber/utils/geometry.py:96 intersects_bbox
pub fn intersects_bbox(objs: &[Obj], bbox: &Bbox) -> PyResult<Vec<Obj>> {
    let mut out = Vec::new();
    for obj in objs {
        if get_bbox_overlap(&obj_to_bbox(obj)?, bbox)?.is_some() {
            out.push(obj.clone());
        }
    }
    Ok(out)
}

/// Tuple equality of two bboxes.
fn bbox_eq(a: &Bbox, b: &Bbox) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| x.py_eq(y))
}

// pdfplumber/utils/geometry.py:103 within_bbox
pub fn within_bbox(objs: &[Obj], bbox: &Bbox) -> PyResult<Vec<Obj>> {
    let mut out = Vec::new();
    for obj in objs {
        let overlap = get_bbox_overlap(&obj_to_bbox(obj)?, bbox)?;
        if let Some(o) = overlap {
            if bbox_eq(&o, &obj_to_bbox(obj)?) {
                out.push(obj.clone());
            }
        }
    }
    Ok(out)
}

// pdfplumber/utils/geometry.py:114 outside_bbox
pub fn outside_bbox(objs: &[Obj], bbox: &Bbox) -> PyResult<Vec<Obj>> {
    let mut out = Vec::new();
    for obj in objs {
        if get_bbox_overlap(&obj_to_bbox(obj)?, bbox)?.is_none() {
            out.push(obj.clone());
        }
    }
    Ok(out)
}

// pdfplumber/utils/geometry.py:121 crop_to_bbox
pub fn crop_to_bbox(objs: &[Obj], bbox: &Bbox) -> PyResult<Vec<Obj>> {
    let mut out = Vec::new();
    for obj in objs {
        if let Some(c) = clip_obj(obj, bbox)? {
            // filter(None, ...): a clipped object always has its four keys
            out.push(c);
        }
    }
    Ok(out)
}

/// The axis of [`move_object`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    H,
    V,
}

// pdfplumber/utils/geometry.py:129 move_object
pub fn move_object(obj: &Obj, axis: Axis, value: &PyValue) -> PyResult<Obj> {
    let mut new_items: Vec<(&str, PyValue)> = Vec::with_capacity(4);
    match axis {
        Axis::H => {
            new_items.push(("x0", obj.hot(Hot::X0)?.add(value)?));
            new_items.push(("x1", obj.hot(Hot::X1)?.add(value)?));
        }
        Axis::V => {
            new_items.push(("top", obj.hot(Hot::Top)?.add(value)?));
            new_items.push(("bottom", obj.hot(Hot::Bottom)?.add(value)?));
            if obj.contains("doctop") {
                new_items.push(("doctop", obj.hot(Hot::Doctop)?.add(value)?));
            }
            if obj.contains("y0") {
                new_items.push(("y0", obj.hot(Hot::Y0)?.sub(value)?));
                new_items.push(("y1", obj.hot(Hot::Y1)?.sub(value)?));
            }
        }
    }
    // obj.__class__(tuple(obj.items()) + tuple(new_items))
    let mut out = obj.clone();
    out.update(new_items);
    Ok(out)
}

/// The attributes [`snap_objects`] accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapAttr {
    X0,
    X1,
    Top,
    Bottom,
}

impl SnapAttr {
    fn hot(self) -> Hot {
        match self {
            SnapAttr::X0 => Hot::X0,
            SnapAttr::X1 => Hot::X1,
            SnapAttr::Top => Hot::Top,
            SnapAttr::Bottom => Hot::Bottom,
        }
    }
}

// pdfplumber/utils/geometry.py:151 snap_objects
pub fn snap_objects(objs: &[Obj], attr: SnapAttr, tolerance: &PyValue) -> PyResult<Vec<Obj>> {
    let axis = match attr {
        SnapAttr::X0 | SnapAttr::X1 => Axis::H,
        SnapAttr::Top | SnapAttr::Bottom => Axis::V,
    };
    let h = attr.hot();
    let clusters = cluster_objects(objs, |o| Ok(o.hot(h)?.clone()), tolerance, false)?;
    let mut avgs = Vec::with_capacity(clusters.len());
    for cluster in &clusters {
        let values = cluster.iter().map(|o| o.hot(h)).collect::<PyResult<Vec<_>>>()?;
        let total = crate::obj::py_sum(values)?;
        avgs.push(total.truediv(&PyValue::from(cluster.len() as i64))?);
    }
    let mut out = Vec::with_capacity(objs.len());
    for (cluster, avg) in clusters.iter().zip(avgs.iter()) {
        for obj in cluster {
            out.push(move_object(obj, axis, &avg.sub(obj.hot(h)?)?)?);
        }
    }
    Ok(out)
}

// pdfplumber/utils/geometry.py:163 resize_object
pub fn resize_object(obj: &Obj, key: SnapAttr, value: &PyValue) -> PyResult<Obj> {
    let h = key.hot();
    let old_value = obj.hot(h)?;
    let diff = value.sub(old_value)?;
    let mut new_items: Vec<(&str, PyValue)> = vec![(h.name(), value.clone())];
    let assertion = || exc::AssertionError("");
    match key {
        SnapAttr::X0 => {
            if !value.le(obj.hot(Hot::X1)?)? {
                return Err(assertion());
            }
            new_items.push(("width", obj.hot(Hot::X1)?.sub(value)?));
        }
        SnapAttr::X1 => {
            if !value.ge(obj.hot(Hot::X0)?)? {
                return Err(assertion());
            }
            new_items.push(("width", value.sub(obj.hot(Hot::X0)?)?));
        }
        SnapAttr::Top => {
            if !value.le(obj.hot(Hot::Bottom)?)? {
                return Err(assertion());
            }
            new_items.push(("doctop", obj.hot(Hot::Doctop)?.add(&diff)?));
            new_items.push(("height", obj.hot(Hot::Height)?.sub(&diff)?));
            if obj.contains("y1") {
                new_items.push(("y1", obj.hot(Hot::Y1)?.sub(&diff)?));
            }
        }
        SnapAttr::Bottom => {
            if !value.ge(obj.hot(Hot::Top)?)? {
                return Err(assertion());
            }
            new_items.push(("height", obj.hot(Hot::Height)?.add(&diff)?));
            if obj.contains("y0") {
                new_items.push(("y0", obj.hot(Hot::Y0)?.sub(&diff)?));
            }
        }
    }
    let mut out = obj.clone();
    out.update(new_items);
    Ok(out)
}

/// `p[i]` for a point of a curve's `pts` (a `tuple` or `list`).
fn point_item(p: &PyValue, i: usize) -> PyResult<&PyValue> {
    match p.as_seq() {
        Some(items) => items.get(i).ok_or_else(|| {
            exc::IndexError(if matches!(p, PyValue::Tuple(_)) { "tuple index out of range" } else { "list index out of range" })
        }),
        None => Err(exc::TypeError(format!("'{}' object is not subscriptable", p.type_name()))),
    }
}

// pdfplumber/utils/geometry.py:190 curve_to_edges
pub fn curve_to_edges(curve: &Obj) -> PyResult<Vec<Obj>> {
    let pts = curve.item("pts")?;
    let Some(pts) = pts.as_seq() else {
        return Err(not_subscriptable(pts));
    };
    let mut out = Vec::new();
    for pair in pts.windows(2) {
        let (p0, p1) = (&pair[0], &pair[1]);
        let x0 = min2(point_item(p0, 0)?, point_item(p1, 0)?)?;
        let x1 = max2(point_item(p0, 0)?, point_item(p1, 0)?)?;
        let top = min2(point_item(p0, 1)?, point_item(p1, 1)?)?;
        let doctop =
            min2(point_item(p0, 1)?, point_item(p1, 1)?)?.add(&curve.hot(Hot::Doctop)?.sub(curve.hot(Hot::Top)?)?)?;
        let bottom = max2(point_item(p0, 1)?, point_item(p1, 1)?)?;
        let width = point_item(p0, 0)?.sub(point_item(p1, 0)?)?.abs()?;
        let height = point_item(p0, 1)?.sub(point_item(p1, 1)?)?.abs()?;
        let orientation = if point_item(p0, 0)?.py_eq(point_item(p1, 0)?) {
            PyValue::str("v")
        } else if point_item(p0, 1)?.py_eq(point_item(p1, 1)?) {
            PyValue::str("h")
        } else {
            PyValue::None
        };
        out.push(Obj::from_pairs([
            ("object_type", PyValue::str("curve_edge")),
            ("x0", x0),
            ("x1", x1),
            ("top", top),
            ("doctop", doctop),
            ("bottom", bottom),
            ("width", width),
            ("height", height),
            ("orientation", orientation),
        ]));
    }
    Ok(out)
}

/// `curve["pts"][1:]` on a value that cannot be sliced.
fn not_subscriptable(v: &PyValue) -> pycompat::PyErr {
    exc::TypeError(format!("'{}' object is not subscriptable", v.type_name()))
}

// pdfplumber/utils/geometry.py:208 rect_to_edges
pub fn rect_to_edges(rect: &Obj) -> PyResult<Vec<Obj>> {
    let (mut top, mut bottom, mut left, mut right) = (rect.clone(), rect.clone(), rect.clone(), rect.clone());
    let zero = PyValue::from(0i64);
    let rect_edge = PyValue::str("rect_edge");
    top.update([
        ("object_type", rect_edge.clone()),
        ("height", zero.clone()),
        ("y0", rect.hot(Hot::Y1)?.clone()),
        ("bottom", rect.hot(Hot::Top)?.clone()),
        ("orientation", PyValue::str("h")),
    ]);
    let b_y1 = rect.hot(Hot::Y0)?.clone();
    let b_top = rect.hot(Hot::Top)?.add(rect.hot(Hot::Height)?)?;
    let b_doctop = rect.hot(Hot::Doctop)?.add(rect.hot(Hot::Height)?)?;
    bottom.update([
        ("object_type", rect_edge.clone()),
        ("height", zero.clone()),
        ("y1", b_y1),
        ("top", b_top),
        ("doctop", b_doctop),
        ("orientation", PyValue::str("h")),
    ]);
    left.update([
        ("object_type", rect_edge.clone()),
        ("width", zero.clone()),
        ("x1", rect.hot(Hot::X0)?.clone()),
        ("orientation", PyValue::str("v")),
    ]);
    right.update([
        ("object_type", rect_edge),
        ("width", zero),
        ("x0", rect.hot(Hot::X1)?.clone()),
        ("orientation", PyValue::str("v")),
    ]);
    Ok(vec![top, bottom, left, right])
}

// pdfplumber/utils/geometry.py:248 line_to_edge
pub fn line_to_edge(line: &Obj) -> PyResult<Obj> {
    let mut edge = line.clone();
    let h = line.hot(Hot::Top)?.py_eq(line.hot(Hot::Bottom)?);
    edge.set("orientation", PyValue::str(if h { "h" } else { "v" }));
    Ok(edge)
}

// pdfplumber/utils/geometry.py:254 obj_to_edges
pub fn obj_to_edges(obj: &Obj) -> PyResult<Vec<Obj>> {
    let t = obj.hot(Hot::ObjectType)?;
    let Some(name) = t.as_str() else {
        return Err(exc::TypeError(format!("argument of type '{}' is not iterable", t.type_name())));
    };
    if name.contains("_edge") {
        Ok(vec![obj.clone()])
    } else if name == "line" {
        Ok(vec![line_to_edge(obj)?])
    } else if name == "rect" {
        rect_to_edges(obj)
    } else if name == "curve" {
        curve_to_edges(obj)
    } else {
        Err(exc::KeyError(t.repr()))
    }
}

// pdfplumber/utils/geometry.py:264 filter_edges
pub fn filter_edges(
    edges: &[Obj],
    orientation: Option<&str>,
    edge_type: Option<&PyValue>,
    min_length: &PyValue,
) -> PyResult<Vec<Obj>> {
    if !matches!(orientation, None | Some("v") | Some("h")) {
        return Err(exc::ValueError("Orientation must be 'v' or 'h'"));
    }
    let mut out = Vec::new();
    for e in edges {
        let o = e.hot(Hot::Orientation)?;
        let is_v = o.as_str() == Some("v");
        let dim = if is_v { Hot::Height } else { Hot::Width };
        let et_correct = match edge_type {
            Some(t) => e.hot(Hot::ObjectType)?.py_eq(t),
            None => true,
        };
        let orient_correct = match orientation {
            None => true,
            Some(want) => o.as_str() == Some(want),
        };
        if et_correct && orient_correct && e.hot(dim)?.ge(min_length)? {
            out.push(e.clone());
        }
    }
    Ok(out)
}
