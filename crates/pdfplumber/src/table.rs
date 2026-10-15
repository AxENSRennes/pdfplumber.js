//! Port of `pdfplumber/table.py`: table settings, edges, intersections,
//! cells and tables.
//!
//! `TableFinder` and `Table.extract` read the page through [`TablePage`]
//! (`page.chars`, `page.edges`, `page.bbox`, `page.extract_words`), which
//! `page.rs` implements.
//!
//! Optimizations (the results are upstream's, see the comments at each):
//! `edges_to_intersections` only visits the horizontal edges whose `top`
//! can satisfy the predicate (binary search, when the coordinates are
//! ordered numbers);
//! `intersections_to_cells` and `cells_to_tables` find equal points and
//! shared edges through hash maps (Python `==`/`hash`) instead of scanning
//! lists.
//!
//! `HashKey` holds `Rc`-shared values, which clippy takes for interior
//! mutability; a key's hash only reads immutable parts (numbers, strings,
//! tuples), so `mutable_key_type` is allowed.
#![allow(clippy::mutable_key_type)]

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use pycompat::listsort::sort_by_key;
use pycompat::pystr::PyStrBuilder;
use pycompat::{exc, ExcKind, PyErr, PyResult, PyStr};

use crate::obj::{bbox_value, py_contains, py_iter, Bbox, HashKey, Hot, Obj, PyValue};
use crate::utils::clustering::cluster_objects;
use crate::utils::geometry::{
    bbox_to_rect, filter_edges, get_bbox_overlap, obj_to_bbox, obj_to_edges, objects_to_bbox, objects_to_rect,
    resize_object, snap_objects, SnapAttr,
};
use crate::utils::text::{extract_text_kwargs, extract_words_kwargs, pair_lt, Kwargs};

// pdfplumber/table.py:9 DEFAULT_SNAP_TOLERANCE
pub const DEFAULT_SNAP_TOLERANCE: i64 = 3;
// pdfplumber/table.py:10 DEFAULT_JOIN_TOLERANCE
pub const DEFAULT_JOIN_TOLERANCE: i64 = 3;
// pdfplumber/table.py:11 DEFAULT_MIN_WORDS_VERTICAL
pub const DEFAULT_MIN_WORDS_VERTICAL: i64 = 3;
// pdfplumber/table.py:12 DEFAULT_MIN_WORDS_HORIZONTAL
pub const DEFAULT_MIN_WORDS_HORIZONTAL: i64 = 1;

/// What `TableFinder` and `Table.extract` read from a page.
pub trait TablePage {
    /// `page.chars`
    fn chars(&self) -> PyResult<Vec<Obj>>;
    /// `page.edges`
    fn edges(&self) -> PyResult<Vec<Obj>>;
    /// `page.bbox`
    fn bbox(&self) -> PyResult<Bbox>;
    /// `page.extract_words(**kwargs)` (pdfplumber/page.py:535 Page.extract_words)
    fn extract_words(&self, kwargs: &Kwargs) -> PyResult<Vec<Obj>> {
        extract_words_kwargs(&self.chars()?, kwargs)
    }
}

/// `e["orientation"]` as a `str`, if it is one.
fn orientation(e: &Obj) -> PyResult<&PyValue> {
    e.hot(Hot::Orientation)
}

// pdfplumber/table.py:21 snap_edges
pub fn snap_edges(edges: &[Obj], x_tolerance: &PyValue, y_tolerance: &PyValue) -> PyResult<Vec<Obj>> {
    let mut v = Vec::new();
    let mut h = Vec::new();
    for e in edges {
        let o = orientation(e)?;
        match o.as_str() {
            Some("v") => v.push(e.clone()),
            Some("h") => h.push(e.clone()),
            _ => {
                return Err(match o {
                    PyValue::List(_) | PyValue::Dict(_) => {
                        exc::TypeError(format!("unhashable type: '{}'", o.type_name()))
                    }
                    _ => exc::KeyError(o.repr()),
                })
            }
        }
    }
    let mut snapped = snap_objects(&v, SnapAttr::X0, x_tolerance)?;
    snapped.extend(snap_objects(&h, SnapAttr::Top, y_tolerance)?);
    Ok(snapped)
}

/// The orientation argument of [`join_edge_group`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    H,
    V,
}

// pdfplumber/table.py:39 join_edge_group
pub fn join_edge_group(edges: Vec<Obj>, orientation: Orientation, tolerance: &PyValue) -> PyResult<Vec<Obj>> {
    let (min_prop, max_prop) = match orientation {
        Orientation::H => (Hot::X0, SnapAttr::X1),
        Orientation::V => (Hot::Top, SnapAttr::Bottom),
    };
    let max_hot = match max_prop {
        SnapAttr::X1 => Hot::X1,
        _ => Hot::Bottom,
    };
    let mut sorted_edges = edges;
    sort_by_key(&mut sorted_edges, |e| Ok(e.hot(min_prop)?.clone()), |a, b| a.lt(b))?;
    let mut iter = sorted_edges.into_iter();
    let first = iter.next().ok_or_else(|| exc::IndexError("list index out of range"))?;
    let mut joined = vec![first];
    for e in iter {
        let last = joined.last().expect("one edge");
        if e.hot(min_prop)?.le(&last.hot(max_hot)?.add(tolerance)?)? {
            if e.hot(max_hot)?.gt(last.hot(max_hot)?)? {
                // Extend current edge to new extremity
                let resized = resize_object(last, max_prop, e.hot(max_hot)?)?;
                *joined.last_mut().expect("one edge") = resized;
            }
        } else {
            // Edge is separate from previous edges
            joined.push(e);
        }
    }
    Ok(joined)
}

// pdfplumber/table.py:68 merge_edges
pub fn merge_edges(
    edges: Vec<Obj>,
    snap_x_tolerance: &PyValue,
    snap_y_tolerance: &PyValue,
    join_x_tolerance: &PyValue,
    join_y_tolerance: &PyValue,
) -> PyResult<Vec<Obj>> {
    // get_group (nested in merge_edges)
    fn get_group(edge: &Obj) -> PyResult<PyValue> {
        if orientation(edge)?.as_str() == Some("h") {
            Ok(PyValue::tuple([PyValue::str("h"), edge.hot(Hot::Top)?.clone()]))
        } else {
            Ok(PyValue::tuple([PyValue::str("v"), edge.hot(Hot::X0)?.clone()]))
        }
    }

    let zero = PyValue::from(0i64);
    let mut edges = edges;
    if snap_x_tolerance.gt(&zero)? || snap_y_tolerance.gt(&zero)? {
        edges = snap_edges(&edges, snap_x_tolerance, snap_y_tolerance)?;
    }

    let mut keyed = Vec::with_capacity(edges.len());
    for e in edges {
        keyed.push((get_group(&e)?, e));
    }
    sort_by_key(&mut keyed, |(k, _)| Ok(k.clone()), |a, b| a.lt(b))?;
    let mut out = Vec::new();
    let mut i = 0;
    while i < keyed.len() {
        let mut j = i + 1;
        while j < keyed.len() && keyed[j].0.py_eq(&keyed[i].0) {
            j += 1;
        }
        let horizontal = keyed[i].0.as_seq().expect("a group key")[0].as_str() == Some("h");
        let items: Vec<Obj> = keyed[i..j].iter().map(|(_, e)| e.clone()).collect();
        let (o, tol) = if horizontal { (Orientation::H, join_x_tolerance) } else { (Orientation::V, join_y_tolerance) };
        out.extend(join_edge_group(items, o, tol)?);
        debug_assert!(j > i, "progress guard (debug assertions: the `mutants` profile)");
        i = j;
    }
    Ok(out)
}

/// `len(x) >= word_threshold`
fn len_ge(n: usize, threshold: &PyValue) -> PyResult<bool> {
    PyValue::from(n as i64).ge(threshold)
}

// pdfplumber/table.py:101 words_to_edges_h
pub fn words_to_edges_h(words: &[Obj], word_threshold: &PyValue) -> PyResult<Vec<Obj>> {
    let by_top = cluster_objects(words, |w| Ok(w.hot(Hot::Top)?.clone()), &PyValue::from(1i64), false)?;
    let mut rects = Vec::new();
    for cluster in by_top {
        if len_ge(cluster.len(), word_threshold)? {
            rects.push(objects_to_rect(&cluster)?);
        }
    }
    if rects.is_empty() {
        return Ok(Vec::new());
    }
    let x0s = rects.iter().map(|r| r.hot(Hot::X0)).collect::<PyResult<Vec<_>>>()?;
    let min_x0 = crate::obj::py_min(x0s)?;
    let x1s = rects.iter().map(|r| r.hot(Hot::X1)).collect::<PyResult<Vec<_>>>()?;
    let max_x1 = crate::obj::py_max(x1s)?;

    let mut edges = Vec::with_capacity(rects.len() * 2);
    for r in &rects {
        for key in [Hot::Top, Hot::Bottom] {
            let y = r.hot(key)?.clone();
            edges.push(Obj::from_pairs([
                ("x0", min_x0.clone()),
                ("x1", max_x1.clone()),
                ("top", y.clone()),
                ("bottom", y),
                ("width", max_x1.sub(&min_x0)?),
                ("orientation", PyValue::str("h")),
            ]));
        }
    }
    Ok(edges)
}

// pdfplumber/table.py:144 words_to_edges_v
pub fn words_to_edges_v(words: &[Obj], word_threshold: &PyValue) -> PyResult<Vec<Obj>> {
    let one = PyValue::from(1i64);
    // Find words that share the same left, right, or centerpoints
    let by_x0 = cluster_objects(words, |w| Ok(w.hot(Hot::X0)?.clone()), &one, false)?;
    let by_x1 = cluster_objects(words, |w| Ok(w.hot(Hot::X1)?.clone()), &one, false)?;
    // get_center (nested in words_to_edges_v)
    let get_center = |w: &Obj| w.hot(Hot::X0)?.add(w.hot(Hot::X1)?)?.to_float()?.truediv(&PyValue::from(2i64));
    let by_center = cluster_objects(words, get_center, &one, false)?;
    let mut clusters = by_x0;
    clusters.extend(by_x1);
    clusters.extend(by_center);

    // Find the points that align with the most words
    sort_by_key(&mut clusters, |c| Ok(-(c.len() as i64)), |a, b| Ok(a < b))?;

    // For each of those points, find the bboxes fitting all matching words
    let mut bboxes = Vec::new();
    for c in &clusters {
        if len_ge(c.len(), word_threshold)? {
            bboxes.push(objects_to_bbox(c)?);
        }
    }

    // Iterate through those bboxes, condensing overlapping bboxes
    let mut condensed_bboxes: Vec<Bbox> = Vec::new();
    for bbox in bboxes {
        let mut overlap = false;
        for c in &condensed_bboxes {
            if get_bbox_overlap(&bbox, c)?.is_some() {
                overlap = true;
                break;
            }
        }
        if !overlap {
            condensed_bboxes.push(bbox);
        }
    }

    if condensed_bboxes.is_empty() {
        return Ok(Vec::new());
    }

    let mut sorted_rects: Vec<Obj> = condensed_bboxes.iter().map(bbox_to_rect).collect();
    sort_by_key(&mut sorted_rects, |r| Ok(r.hot(Hot::X0)?.clone()), |a, b| a.lt(b))?;

    let col = |h: Hot| sorted_rects.iter().map(|r| r.hot(h)).collect::<PyResult<Vec<_>>>();
    let max_x1 = crate::obj::py_max(col(Hot::X1)?)?;
    let min_top = crate::obj::py_min(col(Hot::Top)?)?;
    let max_bottom = crate::obj::py_max(col(Hot::Bottom)?)?;

    let edge = |x: PyValue| -> PyResult<Obj> {
        Ok(Obj::from_pairs([
            ("x0", x.clone()),
            ("x1", x),
            ("top", min_top.clone()),
            ("bottom", max_bottom.clone()),
            ("height", max_bottom.sub(&min_top)?),
            ("orientation", PyValue::str("v")),
        ]))
    };
    let mut out = Vec::with_capacity(sorted_rects.len() + 1);
    for b in &sorted_rects {
        out.push(edge(b.hot(Hot::X0)?.clone())?);
    }
    out.push(edge(max_x1.clone())?);
    Ok(out)
}

/// A point `(x, top)` of [`Intersections`].
pub type Point = (PyValue, PyValue);

/// `T_intersections`: the vertices, in insertion order, with the edges that
/// meet there.
#[derive(Clone, Debug, Default)]
pub struct Intersections {
    pub points: Vec<(Point, Vec<Obj>, Vec<Obj>)>,
    index: HashMap<HashKey, usize>,
}

fn point_key(p: &Point) -> PyResult<HashKey> {
    HashKey::new(PyValue::tuple([p.0.clone(), p.1.clone()]))
}

impl Intersections {
    /// The position of a point (`point in intersections`).
    pub fn find(&self, p: &Point) -> PyResult<Option<usize>> {
        Ok(self.index.get(&point_key(p)?).copied())
    }
}

/// A number whose additions, subtractions and comparisons with other such
/// numbers cannot raise and are monotone: a float that is not a NaN, an
/// `int` in the `i64` range (its conversion to float cannot overflow), a
/// `bool`.
fn ordered_number(v: &PyValue) -> bool {
    match v {
        PyValue::Float(x) => !x.is_nan(),
        PyValue::Int(pycompat::PyInt::Small(_)) | PyValue::Bool(_) => true,
        _ => false,
    }
}

// pdfplumber/table.py:207 edges_to_intersections
/// A vertex `(v["x0"], h["top"])` never holds a NaN: `v["x0"]` is compared
/// by `v["x0"] >= h["x0"] - x_tolerance` and `h["top"]` by `v["top"] <=
/// h["top"] + y_tolerance`, and a NaN on either side (or a NaN tolerance)
/// makes the comparison false. The cells (`intersections_to_cells`, made
/// of vertices), the tables and their rows and columns therefore never
/// hold one either: `Table.rows` and `Table.columns`, which look cell
/// coordinates up in a dict, never meet a NaN through `TableFinder`
/// (`no_nan_vertex` in `crates/pdftrace/tests/text_tables_unit.rs` checks
/// this on edges with NaN at every coordinate; a `Table` built by hand
/// from NaN cells would still look them up by identity, as Python does).
pub fn edges_to_intersections(edges: &[Obj], x_tolerance: &PyValue, y_tolerance: &PyValue) -> PyResult<Intersections> {
    let mut intersections = Intersections::default();
    let mut v_edges = Vec::new();
    let mut h_edges = Vec::new();
    for e in edges {
        if orientation(e)?.py_eq(&PyValue::str("v")) {
            v_edges.push(e.clone());
        }
    }
    for e in edges {
        if orientation(e)?.py_eq(&PyValue::str("h")) {
            h_edges.push(e.clone());
        }
    }
    let key2 = |e: &Obj, a: Hot, b: Hot| -> PyResult<(PyValue, PyValue)> { Ok((e.hot(a)?.clone(), e.hot(b)?.clone())) };
    sort_by_key(&mut v_edges, |e| key2(e, Hot::X0, Hot::Top), pair_lt)?;
    if v_edges.is_empty() {
        return Ok(intersections);
    }
    // sorted(h_edges, ...) runs once per v edge upstream, with the same
    // result each time: sorting once is equivalent.
    sort_by_key(&mut h_edges, |e| key2(e, Hot::Top, Hot::X0), pair_lt)?;

    // Search window: when the y tolerance, the v edge's top and bottom and
    // every h edge's top are ordered numbers (`ordered_number`), the h edges
    // are sorted by top, `v.top <= h.top + ytol` is false then true along
    // them and `v.bottom >= h.top - ytol` true then false (rounded addition
    // is monotone), so the h edges that satisfy both form one range, found
    // by binary search with the same expressions. Python evaluates the
    // predicate on every h edge; outside the range it is false without
    // raising, so visiting the range only, in order, gives the same result.
    let tops_ordered =
        ordered_number(y_tolerance) && h_edges.iter().all(|h| h.get_hot(Hot::Top).is_some_and(ordered_number));

    for v in &v_edges {
        let mut range = 0..h_edges.len();
        if let (true, Some(vt), Some(vb)) = (tops_ordered, v.get_hot(Hot::Top), v.get_hot(Hot::Bottom)) {
            if ordered_number(vt) && ordered_number(vb) {
                let above = |h: &Obj| -> PyResult<bool> { Ok(!vt.le(&h.hot(Hot::Top)?.add(y_tolerance)?)?) };
                let reaches = |h: &Obj| -> PyResult<bool> { vb.ge(&h.hot(Hot::Top)?.sub(y_tolerance)?) };
                let lo = partition_point(&h_edges, above)?;
                let hi = lo + partition_point(&h_edges[lo..], reaches)?;
                range = lo..hi;
            }
        }
        for h in &h_edges[range] {
            if v.hot(Hot::Top)?.le(&h.hot(Hot::Top)?.add(y_tolerance)?)?
                && v.hot(Hot::Bottom)?.ge(&h.hot(Hot::Top)?.sub(y_tolerance)?)?
                && v.hot(Hot::X0)?.ge(&h.hot(Hot::X0)?.sub(x_tolerance)?)?
                && v.hot(Hot::X0)?.le(&h.hot(Hot::X1)?.add(x_tolerance)?)?
            {
                let vertex = (v.hot(Hot::X0)?.clone(), h.hot(Hot::Top)?.clone());
                let key = point_key(&vertex)?;
                let k = match intersections.index.get(&key) {
                    Some(&k) => k,
                    None => {
                        intersections.points.push((vertex, Vec::new(), Vec::new()));
                        intersections.index.insert(key, intersections.points.len() - 1);
                        intersections.points.len() - 1
                    }
                };
                intersections.points[k].1.push(v.clone());
                intersections.points[k].2.push(h.clone());
            }
        }
    }
    Ok(intersections)
}

/// `slice.partition_point(pred)` with a fallible predicate: the first
/// index where `pred` is false (it must be true then false along `items`).
fn partition_point(items: &[Obj], mut pred: impl FnMut(&Obj) -> PyResult<bool>) -> PyResult<usize> {
    let (mut lo, mut hi) = (0, items.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if pred(&items[mid])? {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Ok(lo)
}

/// `edges_to_set(intersections[p]["v"])` and `["h"]` (nested in
/// `intersections_to_cells`) as sorted ids of distinct bboxes, computed the
/// first time `edge_connects` needs them, so that its `KeyError` (a missing
/// coordinate) or `TypeError` (an unhashable one) is raised where Python
/// raises it, or not at all. Two edges have the same id when their bboxes
/// are equal as set members: CPython compares tuple items with an identity
/// shortcut, so a bbox holding a NaN is equal to a bbox holding the same
/// NaN object (the edge's own, or one it shares with another edge: see
/// "NaN identity" in `obj.rs`).
struct EdgeSets<'a> {
    points: &'a [(Point, Vec<Obj>, Vec<Obj>)],
    ids: HashMap<[HashKey; 4], u32>,
    /// `[v, h]` of each point, once computed
    sets: Vec<[Option<Vec<u32>>; 2]>,
}

impl<'a> EdgeSets<'a> {
    fn new(points: &'a [(Point, Vec<Obj>, Vec<Obj>)]) -> Self {
        EdgeSets { points, ids: HashMap::default(), sets: vec![[None, None]; points.len()] }
    }

    // edges_to_set (nested in edge_connects)
    fn compute(&mut self, p: usize, axis: usize) -> PyResult<()> {
        if self.sets[p][axis].is_some() {
            return Ok(());
        }
        let edges = if axis == 0 { &self.points[p].1 } else { &self.points[p].2 };
        let mut ids = Vec::with_capacity(edges.len());
        for e in edges {
            let b = obj_to_bbox(e)?;
            let key = [
                HashKey::new(b[0].clone())?,
                HashKey::new(b[1].clone())?,
                HashKey::new(b[2].clone())?,
                HashKey::new(b[3].clone())?,
            ];
            let next = self.ids.len() as u32;
            ids.push(*self.ids.entry(key).or_insert(next));
        }
        ids.sort_unstable();
        ids.dedup();
        self.sets[p][axis] = Some(ids);
        Ok(())
    }

    /// `len(edges_to_set(p1 edges).intersection(edges_to_set(p2 edges)))`
    /// along `axis` (0: "v", 1: "h")
    fn share(&mut self, p1: usize, p2: usize, axis: usize) -> PyResult<bool> {
        self.compute(p1, axis)?;
        self.compute(p2, axis)?;
        let a = self.sets[p1][axis].as_deref().expect("computed");
        let b = self.sets[p2][axis].as_deref().expect("computed");
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            match a[i].cmp(&b[j]) {
                std::cmp::Ordering::Equal => return Ok(true),
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
            }
        }
        Ok(false)
    }

    // edge_connects (nested in intersections_to_cells)
    /// On positions in `intersections`.
    fn edge_connects(&mut self, p1: usize, p2: usize) -> PyResult<bool> {
        if self.points[p1].0 .0.py_eq(&self.points[p2].0 .0) && self.share(p1, p2, 0)? {
            return Ok(true);
        }
        Ok(self.points[p1].0 .1.py_eq(&self.points[p2].0 .1) && self.share(p1, p2, 1)?)
    }
}

// pdfplumber/table.py:234 intersections_to_cells
pub fn intersections_to_cells(intersections: &Intersections) -> PyResult<Vec<Bbox>> {
    let n = intersections.points.len();
    let mut sets = EdgeSets::new(&intersections.points);
    let pt = |k: usize| &intersections.points[k].0;

    // points = list(sorted(intersections.keys()))
    let mut points: Vec<usize> = (0..n).collect();
    sort_by_key(&mut points, |&k| Ok(pt(k).clone()), pair_lt)?;

    // Points of equal x (resp. y), by position in `points`.
    let mut by_x: HashMap<HashKey, Vec<usize>> = HashMap::default();
    let mut by_y: HashMap<HashKey, Vec<usize>> = HashMap::default();
    for (i, &k) in points.iter().enumerate() {
        by_x.entry(HashKey::new(pt(k).0.clone())?).or_default().push(i);
        by_y.entry(HashKey::new(pt(k).1.clone())?).or_default().push(i);
    }
    let after = |list: &[usize], i: usize| -> Vec<usize> {
        let from = list.partition_point(|&j| j <= i);
        list[from..].to_vec()
    };

    let mut cells = Vec::new();
    // find_smallest_cell (nested in intersections_to_cells)
    for i in 0..n {
        if i == n - 1 {
            continue;
        }
        let p = points[i];
        let (x, y) = pt(p);
        // Get all the points directly below and directly right (`==`, the
        // operator: a NaN equals no point, not even itself; an intersection
        // never holds one, see `edges_to_intersections`)
        let group = |by: &HashMap<HashKey, Vec<usize>>, v: &PyValue| {
            if v.py_eq(v) {
                by.get(&HashKey(v.clone())).map_or_else(Vec::new, |l| after(l, i))
            } else {
                Vec::new()
            }
        };
        let below = group(&by_x, x);
        let right = group(&by_y, y);
        'search: for &b in &below {
            let below_pt = points[b];
            if !sets.edge_connects(p, below_pt)? {
                continue;
            }
            for &r in &right {
                let right_pt = points[r];
                if !sets.edge_connects(p, right_pt)? {
                    continue;
                }
                let bottom_right = (pt(right_pt).0.clone(), pt(below_pt).1.clone());
                if let Some(br) = intersections.find(&bottom_right)? {
                    if sets.edge_connects(br, right_pt)? && sets.edge_connects(br, below_pt)? {
                        cells.push([x.clone(), y.clone(), bottom_right.0, bottom_right.1]);
                        break 'search;
                    }
                }
            }
        }
    }
    Ok(cells)
}

// pdfplumber/table.py:297 cells_to_tables
pub fn cells_to_tables(cells: &[Bbox]) -> PyResult<Vec<Vec<Bbox>>> {
    // bbox_to_corners (nested in cells_to_tables), as ids of distinct points
    let mut corner_ids: HashMap<HashKey, usize> = HashMap::default();
    let mut corners_of = Vec::with_capacity(cells.len());
    for cell in cells {
        let [x0, top, x1, bottom] = cell;
        let mut ids = [0usize; 4];
        for (k, (a, b)) in [(x0, top), (x0, bottom), (x1, top), (x1, bottom)].into_iter().enumerate() {
            let key = HashKey::new(PyValue::tuple([a.clone(), b.clone()]))?;
            let next = corner_ids.len();
            ids[k] = *corner_ids.entry(key).or_insert(next);
        }
        corners_of.push(ids);
    }

    // current_corners as marks: a corner is in the set when its mark is the
    // current generation.
    let mut mark = vec![0u32; corner_ids.len()];
    let mut generation = 1u32;
    let mut remaining_cells: Vec<usize> = (0..cells.len()).collect();
    let mut current_cells: Vec<usize> = Vec::new();
    let mut tables: Vec<Vec<usize>> = Vec::new();
    let mut passes = 0usize;
    while !remaining_cells.is_empty() {
        // each pass assigns a cell or closes a table: at most 2n + 1 passes
        passes += 1;
        debug_assert!(passes <= 2 * cells.len() + 1, "progress guard (debug assertions: the `mutants` profile)");
        let initial_cell_count = current_cells.len();
        let mut next_remaining = Vec::with_capacity(remaining_cells.len());
        for &cell in &remaining_cells {
            let cell_corners = corners_of[cell];
            if current_cells.is_empty() || cell_corners.iter().any(|&c| mark[c] == generation) {
                for &c in &cell_corners {
                    mark[c] = generation;
                }
                current_cells.push(cell);
            } else {
                next_remaining.push(cell);
            }
        }
        remaining_cells = next_remaining;
        // If this iteration did not find any more cells to append...
        if current_cells.len() == initial_cell_count {
            tables.push(std::mem::take(&mut current_cells));
            generation += 1;
        }
    }
    if !current_cells.is_empty() {
        tables.push(current_cells);
    }

    // Sort the tables top-to-bottom-left-to-right based on the value of the
    // topmost-and-then-leftmost coordinate of a table.
    let mut keyed: Vec<((PyValue, PyValue), Vec<usize>)> = Vec::with_capacity(tables.len());
    for t in tables {
        // min((c[1], c[0]) for c in t)
        let mut best: Option<(PyValue, PyValue)> = None;
        for &c in &t {
            let k = (cells[c][1].clone(), cells[c][0].clone());
            best = match best {
                None => Some(k),
                Some(b) => Some(if pair_lt(&k, &b)? { k } else { b }),
            };
        }
        keyed.push((best.ok_or_else(|| exc::ValueError("min() iterable argument is empty"))?, t));
    }
    sort_by_key(&mut keyed, |(k, _)| Ok(k.clone()), pair_lt)?;
    Ok(keyed
        .into_iter()
        .filter(|(_, t)| t.len() > 1)
        .map(|(_, t)| t.into_iter().map(|c| cells[c].clone()).collect())
        .collect())
}

/// pdfplumber/table.py:358 CellGroup (and :369 Row, :373 Column)
#[derive(Clone, Debug)]
pub struct CellGroup {
    pub cells: Vec<Option<Bbox>>,
    pub bbox: Bbox,
}

impl CellGroup {
    // pdfplumber/table.py:359 CellGroup.__init__
    pub fn new(cells: Vec<Option<Bbox>>) -> PyResult<CellGroup> {
        let col = |i: usize| cells.iter().flatten().map(move |c| &c[i]);
        let bbox = [
            crate::obj::py_min(col(0))?,
            crate::obj::py_min(col(1))?,
            crate::obj::py_max(col(2))?,
            crate::obj::py_max(col(3))?,
        ];
        Ok(CellGroup { cells, bbox })
    }
}

/// `Row` or `Column`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellGroupKind {
    Row,
    Column,
}

/// pdfplumber/table.py:377 Table
#[derive(Clone, Debug)]
pub struct Table {
    pub cells: Vec<Bbox>,
}

impl Table {
    // pdfplumber/table.py:378 Table.__init__
    pub fn new(cells: Vec<Bbox>) -> Table {
        Table { cells }
    }

    // pdfplumber/table.py:383 Table.bbox
    pub fn bbox(&self) -> PyResult<Bbox> {
        let col = |i: usize| self.cells.iter().map(move |c| &c[i]);
        Ok([
            crate::obj::py_min(col(0))?,
            crate::obj::py_min(col(1))?,
            crate::obj::py_max(col(2))?,
            crate::obj::py_max(col(3))?,
        ])
    }

    // pdfplumber/table.py:392 Table._get_rows_or_cols
    pub fn get_rows_or_cols(&self, kind: CellGroupKind) -> PyResult<Vec<CellGroup>> {
        let axis = if kind == CellGroupKind::Row { 0 } else { 1 };
        let antiaxis = 1 - axis;

        // Sort first by top/x0, then by x0/top
        let mut sorted = self.cells.clone();
        sort_by_key(&mut sorted, |c| Ok((c[antiaxis].clone(), c[axis].clone())), pair_lt)?;

        // Sort get all x0s/tops: sorted(set(...))
        let mut seen: HashSet<HashKey> = HashSet::default();
        let mut xs = Vec::new();
        for c in &self.cells {
            if seen.insert(HashKey::new(c[axis].clone())?) {
                xs.push(c[axis].clone());
            }
        }
        pycompat::listsort::sort_by_lt(&mut xs, |a, b| a.lt(b))?;

        // Group by top/x0
        let mut rows = Vec::new();
        let mut i = 0;
        while i < sorted.len() {
            let mut j = i + 1;
            while j < sorted.len() && sorted[j][antiaxis].py_eq(&sorted[i][antiaxis]) {
                j += 1;
            }
            // xdict = {cell[axis]: cell for cell in row_cells}
            let mut xdict: HashMap<HashKey, &Bbox> = HashMap::default();
            for cell in &sorted[i..j] {
                xdict.insert(HashKey::new(cell[axis].clone())?, cell);
            }
            let row_cells = xs
                .iter()
                .map(|x| Ok(xdict.get(&HashKey::new(x.clone())?).map(|c| (*c).clone())))
                .collect::<PyResult<Vec<_>>>()?;
            rows.push(CellGroup::new(row_cells)?);
            debug_assert!(j > i, "progress guard (debug assertions: the `mutants` profile)");
            i = j;
        }
        Ok(rows)
    }

    // pdfplumber/table.py:414 Table.rows
    pub fn rows(&self) -> PyResult<Vec<CellGroup>> {
        self.get_rows_or_cols(CellGroupKind::Row)
    }

    // pdfplumber/table.py:418 Table.columns
    pub fn columns(&self) -> PyResult<Vec<CellGroup>> {
        self.get_rows_or_cols(CellGroupKind::Column)
    }

    // pdfplumber/table.py:421 Table.extract
    /// The text of each cell (`None` where a row has no cell).
    pub fn extract(&self, page: &dyn TablePage, kwargs: &Kwargs) -> PyResult<Vec<Vec<Option<PyStr>>>> {
        let chars = page.chars()?;
        let mut kwargs = kwargs.clone();
        let mut table_arr = Vec::new();

        // char_in_bbox (nested in Table.extract), in two steps: the
        // midpoints of each char, then the comparisons. The midpoints depend
        // on the char only, so they are computed once, in the order of the
        // first row's scan (char by char, `v_mid` before `h_mid`): the first
        // exception, if any, is the one that scan raises (comparing the
        // numbers of a cell's bbox raises nothing).
        fn midpoints(char: &Obj) -> PyResult<(PyValue, PyValue)> {
            let two = PyValue::from(2i64);
            let v_mid = char.hot(Hot::Top)?.add(char.hot(Hot::Bottom)?)?.truediv(&two)?;
            let h_mid = char.hot(Hot::X0)?.add(char.hot(Hot::X1)?)?.truediv(&two)?;
            Ok((v_mid, h_mid))
        }
        fn char_in_bbox((v_mid, h_mid): &(PyValue, PyValue), bbox: &Bbox) -> PyResult<bool> {
            let [x0, top, x1, bottom] = bbox;
            Ok(h_mid.ge(x0)? && h_mid.lt(x1)? && v_mid.ge(top)? && v_mid.lt(bottom)?)
        }

        let rows = self.rows()?;
        let mids =
            if rows.is_empty() { Vec::new() } else { chars.iter().map(midpoints).collect::<PyResult<Vec<_>>>()? };
        for row in rows {
            let mut arr = Vec::with_capacity(row.cells.len());
            let mut row_chars = Vec::new();
            for (k, mid) in mids.iter().enumerate() {
                if char_in_bbox(mid, &row.bbox)? {
                    row_chars.push(k);
                }
            }
            for cell in &row.cells {
                let cell_text = match cell {
                    None => None,
                    Some(cell) => {
                        let mut cell_chars = Vec::new();
                        for &k in &row_chars {
                            if char_in_bbox(&mids[k], cell)? {
                                cell_chars.push(chars[k].clone());
                            }
                        }
                        if !cell_chars.is_empty() {
                            if kwargs.contains("layout") {
                                kwargs.set("layout_width", cell[2].sub(&cell[0])?);
                                kwargs.set("layout_height", cell[3].sub(&cell[1])?);
                                kwargs.set("layout_bbox", bbox_value(cell));
                            }
                            Some(extract_text_kwargs(&cell_chars, &kwargs)?)
                        } else {
                            Some(PyStr::empty())
                        }
                    }
                };
                arr.push(cell_text);
            }
            table_arr.push(arr);
        }
        Ok(table_arr)
    }
}

// pdfplumber/table.py:460 TABLE_STRATEGIES
pub const TABLE_STRATEGIES: [&str; 4] = ["lines", "lines_strict", "text", "explicit"];

// pdfplumber/table.py:461 NON_NEGATIVE_SETTINGS
pub const NON_NEGATIVE_SETTINGS: [&str; 13] = [
    "snap_tolerance",
    "snap_x_tolerance",
    "snap_y_tolerance",
    "join_tolerance",
    "join_x_tolerance",
    "join_y_tolerance",
    "edge_min_length",
    "edge_min_length_prefilter",
    "min_words_vertical",
    "min_words_horizontal",
    "intersection_tolerance",
    "intersection_x_tolerance",
    "intersection_y_tolerance",
];

/// pdfplumber/table.py:486 TableSettings. A field that upstream defaults to
/// UNSET (pdfplumber/table.py:482 UNSET) is `None` here until `__post_init__`
/// resolves it.
#[derive(Clone, Debug)]
pub struct TableSettings {
    pub vertical_strategy: PyValue,
    pub horizontal_strategy: PyValue,
    pub explicit_vertical_lines: PyValue,
    pub explicit_horizontal_lines: PyValue,
    pub snap_tolerance: PyValue,
    pub snap_x_tolerance: Option<PyValue>,
    pub snap_y_tolerance: Option<PyValue>,
    pub join_tolerance: PyValue,
    pub join_x_tolerance: Option<PyValue>,
    pub join_y_tolerance: Option<PyValue>,
    pub edge_min_length: PyValue,
    pub edge_min_length_prefilter: PyValue,
    pub min_words_vertical: PyValue,
    pub min_words_horizontal: PyValue,
    pub intersection_tolerance: PyValue,
    pub intersection_x_tolerance: Option<PyValue>,
    pub intersection_y_tolerance: Option<PyValue>,
    /// `None`, or a `dict` (an [`Obj`]) once resolved.
    pub text_settings: PyValue,
}

impl Default for TableSettings {
    /// `TableSettings()`: the field defaults, resolved by `__post_init__`.
    fn default() -> TableSettings {
        let mut s = TableSettings::field_defaults();
        s.post_init().expect("the defaults are valid");
        s
    }
}

impl TableSettings {
    /// The field defaults, before `__post_init__`.
    fn field_defaults() -> TableSettings {
        TableSettings {
            vertical_strategy: PyValue::str("lines"),
            horizontal_strategy: PyValue::str("lines"),
            explicit_vertical_lines: PyValue::None,
            explicit_horizontal_lines: PyValue::None,
            snap_tolerance: PyValue::from(DEFAULT_SNAP_TOLERANCE),
            snap_x_tolerance: None,
            snap_y_tolerance: None,
            join_tolerance: PyValue::from(DEFAULT_JOIN_TOLERANCE),
            join_x_tolerance: None,
            join_y_tolerance: None,
            edge_min_length: PyValue::from(3i64),
            edge_min_length_prefilter: PyValue::from(1i64),
            min_words_vertical: PyValue::from(DEFAULT_MIN_WORDS_VERTICAL),
            min_words_horizontal: PyValue::from(DEFAULT_MIN_WORDS_HORIZONTAL),
            intersection_tolerance: PyValue::from(3i64),
            intersection_x_tolerance: None,
            intersection_y_tolerance: None,
            text_settings: PyValue::None,
        }
    }
}

/// `UNSET` (pdfplumber/table.py:482 UNSET): `UnsetFloat(0)`, a float.
const UNSET_VALUE: PyValue = PyValue::Float(0.0);

/// The value of an `UNSET`-defaulted field (`__post_init__` resolves them
/// all; a field left unresolved reads as `UNSET`, a `0.0`).
fn resolved(v: &Option<PyValue>) -> &PyValue {
    v.as_ref().unwrap_or(&UNSET_VALUE)
}

impl TableSettings {
    /// `TableSettings(**kwargs)`: the dataclass `__init__` (unknown keyword:
    /// `TypeError`), then `__post_init__`.
    pub fn new(kwargs: &Kwargs) -> PyResult<TableSettings> {
        let mut s = TableSettings::field_defaults();
        for (k, v) in kwargs.items() {
            let v = v.clone();
            match k.as_str().unwrap_or("") {
                "vertical_strategy" => s.vertical_strategy = v,
                "horizontal_strategy" => s.horizontal_strategy = v,
                "explicit_vertical_lines" => s.explicit_vertical_lines = v,
                "explicit_horizontal_lines" => s.explicit_horizontal_lines = v,
                "snap_tolerance" => s.snap_tolerance = v,
                "snap_x_tolerance" => s.snap_x_tolerance = Some(v),
                "snap_y_tolerance" => s.snap_y_tolerance = Some(v),
                "join_tolerance" => s.join_tolerance = v,
                "join_x_tolerance" => s.join_x_tolerance = Some(v),
                "join_y_tolerance" => s.join_y_tolerance = Some(v),
                "edge_min_length" => s.edge_min_length = v,
                "edge_min_length_prefilter" => s.edge_min_length_prefilter = v,
                "min_words_vertical" => s.min_words_vertical = v,
                "min_words_horizontal" => s.min_words_horizontal = v,
                "intersection_tolerance" => s.intersection_tolerance = v,
                "intersection_x_tolerance" => s.intersection_x_tolerance = Some(v),
                "intersection_y_tolerance" => s.intersection_y_tolerance = Some(v),
                "text_settings" => s.text_settings = v,
                _ => {
                    let mut m = PyStrBuilder::new();
                    m.push_rust_str("TableSettings.__init__() got an unexpected keyword argument '");
                    m.push_str(k);
                    m.push_rust_str("'");
                    return Err(PyErr::with_message(ExcKind::TypeError, &m.finish()));
                }
            }
        }
        s.post_init()?;
        Ok(s)
    }

    /// A setting by name (`getattr(self, setting)`; `UNSET` reads as `0.0`).
    fn setting(&self, name: &str) -> PyValue {
        let unset = |v: &Option<PyValue>| v.clone().unwrap_or(PyValue::Float(0.0));
        match name {
            "snap_tolerance" => self.snap_tolerance.clone(),
            "snap_x_tolerance" => unset(&self.snap_x_tolerance),
            "snap_y_tolerance" => unset(&self.snap_y_tolerance),
            "join_tolerance" => self.join_tolerance.clone(),
            "join_x_tolerance" => unset(&self.join_x_tolerance),
            "join_y_tolerance" => unset(&self.join_y_tolerance),
            "edge_min_length" => self.edge_min_length.clone(),
            "edge_min_length_prefilter" => self.edge_min_length_prefilter.clone(),
            "min_words_vertical" => self.min_words_vertical.clone(),
            "min_words_horizontal" => self.min_words_horizontal.clone(),
            "intersection_tolerance" => self.intersection_tolerance.clone(),
            "intersection_x_tolerance" => unset(&self.intersection_x_tolerance),
            "intersection_y_tolerance" => unset(&self.intersection_y_tolerance),
            other => unreachable!("not a setting: {other}"),
        }
    }

    // pdfplumber/table.py:506 TableSettings.__post_init__
    pub fn post_init(&mut self) -> PyResult<()> {
        let zero = PyValue::from(0i64);
        for setting in NON_NEGATIVE_SETTINGS {
            let v = self.setting(setting);
            let v = if v.truthy() { v } else { zero.clone() };
            if v.lt(&zero)? {
                return Err(exc::ValueError(format!("Table setting '{setting}' cannot be negative")));
            }
        }

        for (orientation, strategy) in
            [("horizontal", &self.horizontal_strategy), ("vertical", &self.vertical_strategy)]
        {
            if !TABLE_STRATEGIES.iter().any(|s| strategy.py_eq(&PyValue::str(s))) {
                return Err(exc::ValueError(format!(
                    "{orientation}_strategy must be one of{{{}}}",
                    TABLE_STRATEGIES.join(",")
                )));
            }
        }

        if self.text_settings.is_none() {
            self.text_settings = PyValue::Dict(Obj::new());
        }

        // This next section is for backwards compatibility. A value that is
        // not a dict (only `TableSettings(text_settings=...)` gives one:
        // `resolve` makes a dict of the `text_` settings) goes through the
        // same Python operations.
        let ts = &mut self.text_settings;
        for attr in ["x_tolerance", "y_tolerance"] {
            if !py_contains(ts, &PyValue::str(attr))? {
                match ts {
                    PyValue::Dict(d) => {
                        let v = d.get("tolerance").cloned().unwrap_or(PyValue::from(3i64));
                        d.set(attr, v);
                    }
                    other => {
                        return Err(exc::AttributeError(format!(
                            "'{}' object has no attribute 'get'",
                            other.type_name()
                        )))
                    }
                }
            }
        }
        if py_contains(ts, &PyValue::str("tolerance"))? {
            match ts {
                PyValue::Dict(d) => {
                    d.remove("tolerance")?;
                }
                PyValue::List(_) => return Err(exc::TypeError("list indices must be integers or slices, not str")),
                other => {
                    return Err(exc::TypeError(format!(
                        "'{}' object does not support item deletion",
                        other.type_name()
                    )))
                }
            }
        }
        // End of that section

        let fallback = |v: &mut Option<PyValue>, f: &PyValue| {
            if v.is_none() {
                *v = Some(f.clone());
            }
        };
        fallback(&mut self.snap_x_tolerance, &self.snap_tolerance);
        fallback(&mut self.snap_y_tolerance, &self.snap_tolerance);
        fallback(&mut self.join_x_tolerance, &self.join_tolerance);
        fallback(&mut self.join_y_tolerance, &self.join_tolerance);
        fallback(&mut self.intersection_x_tolerance, &self.intersection_tolerance);
        fallback(&mut self.intersection_y_tolerance, &self.intersection_tolerance);
        Ok(())
    }

    // pdfplumber/table.py:558 TableSettings.resolve
    /// `TableSettings.resolve(settings)` for `None` or a `dict` (a
    /// `TableSettings` resolves to itself).
    pub fn resolve(settings: Option<&Kwargs>) -> PyResult<TableSettings> {
        let Some(settings) = settings else {
            return TableSettings::new(&Obj::new());
        };
        let mut core_settings = Obj::new();
        let mut text_settings = Obj::new();
        for (k, v) in settings.items() {
            // k[:5] == "text_"
            match k.strip_prefix("text_") {
                Some(rest) => text_settings.set(&rest, v.clone()),
                None => core_settings.set(k, v.clone()),
            }
        }
        core_settings.set("text_settings", PyValue::Dict(text_settings));
        TableSettings::new(&core_settings)
    }

    /// The resolved `text_settings` dict (empty when it is not a dict).
    pub fn text_settings(&self) -> Obj {
        match &self.text_settings {
            PyValue::Dict(d) => d.clone(),
            _ => Obj::new(),
        }
    }

    /// `f(**(self.text_settings or {}))`: the keyword arguments of a call
    /// to `func` (its module and qualified name,
    /// `"pdfplumber.page.Page.extract_words"`), a `TypeError` when
    /// `text_settings` is a true value that is not a mapping.
    pub fn text_settings_kwargs(&self, func: &str) -> PyResult<Obj> {
        match &self.text_settings {
            PyValue::Dict(d) => Ok(d.clone()),
            v if !v.truthy() => Ok(Obj::new()),
            other => {
                Err(exc::TypeError(format!("{func}() argument after ** must be a mapping, not {}", other.type_name())))
            }
        }
    }
}

/// pdfplumber/table.py:577 TableFinder
#[derive(Clone, Debug)]
pub struct TableFinder {
    pub settings: TableSettings,
    pub edges: Vec<Obj>,
    pub intersections: Intersections,
    pub cells: Vec<Bbox>,
    pub tables: Vec<Table>,
}

/// `len(lines)`
fn py_len(v: &PyValue) -> PyResult<usize> {
    match v {
        PyValue::Tuple(t) | PyValue::List(t) => Ok(t.len()),
        PyValue::Str(s) => Ok(s.len()),
        PyValue::Bytes(b) => Ok(b.len()),
        PyValue::Dict(d) => Ok(d.len()),
        other => Err(exc::TypeError(format!("object of type '{}' has no len()", other.type_name()))),
    }
}

/// `for desc in (lines or [])`: any iterable (a `str` gives its
/// characters, `bytes` its ints, a `dict` its keys).
fn explicit_items(v: &PyValue) -> PyResult<Vec<PyValue>> {
    if !v.truthy() {
        return Ok(Vec::new());
    }
    match py_iter(v) {
        Some(items) => items,
        None => Err(exc::TypeError(format!("'{}' object is not iterable", v.type_name()))),
    }
}

impl TableFinder {
    // pdfplumber/table.py:588 TableFinder.__init__
    pub fn new(page: &dyn TablePage, settings: TableSettings) -> PyResult<TableFinder> {
        let edges = TableFinder::get_edges(page, &settings)?;
        let intersections = edges_to_intersections(
            &edges,
            resolved(&settings.intersection_x_tolerance),
            resolved(&settings.intersection_y_tolerance),
        )?;
        let cells = intersections_to_cells(&intersections)?;
        let tables = cells_to_tables(&cells)?.into_iter().map(Table::new).collect();
        Ok(TableFinder { settings, edges, intersections, cells, tables })
    }

    // pdfplumber/table.py:602 TableFinder.get_edges
    pub fn get_edges(page: &dyn TablePage, settings: &TableSettings) -> PyResult<Vec<Obj>> {
        for (orientation, strategy, lines) in [
            ("vertical", &settings.vertical_strategy, &settings.explicit_vertical_lines),
            ("horizontal", &settings.horizontal_strategy, &settings.explicit_horizontal_lines),
        ] {
            if strategy.as_str() == Some("explicit") && py_len(lines)? < 2 {
                return Err(exc::ValueError(format!(
                    "If {orientation}_strategy == 'explicit', explicit_{orientation}_lines must be specified \
                     as a list/tuple of two or more floats/ints."
                )));
            }
        }

        let v_strat = settings.vertical_strategy.as_str().unwrap_or("");
        let h_strat = settings.horizontal_strategy.as_str().unwrap_or("");

        let words = if v_strat == "text" || h_strat == "text" {
            page.extract_words(&settings.text_settings_kwargs("pdfplumber.page.Page.extract_words")?)?
        } else {
            Vec::new()
        };

        let bbox = || page.bbox();
        let mut v_explicit = Vec::new();
        for desc in explicit_items(&settings.explicit_vertical_lines)? {
            if let PyValue::Dict(d) = &desc {
                for e in obj_to_edges(d)? {
                    if orientation(&e)?.py_eq(&PyValue::str("v")) {
                        v_explicit.push(e);
                    }
                }
            } else {
                let b = bbox()?;
                v_explicit.push(Obj::from_pairs([
                    ("x0", desc.clone()),
                    ("x1", desc.clone()),
                    ("top", b[1].clone()),
                    ("bottom", b[3].clone()),
                    ("height", b[3].sub(&b[1])?),
                    ("orientation", PyValue::str("v")),
                ]));
            }
        }

        let prefilter = &settings.edge_min_length_prefilter;
        let line = PyValue::str("line");
        let mut v = match v_strat {
            "lines" => filter_edges(&page.edges()?, Some("v"), None, prefilter)?,
            "lines_strict" => filter_edges(&page.edges()?, Some("v"), Some(&line), prefilter)?,
            "text" => words_to_edges_v(&words, &settings.min_words_vertical)?,
            _ => Vec::new(), // "explicit"
        };
        v.extend(v_explicit);

        let mut h_explicit = Vec::new();
        for desc in explicit_items(&settings.explicit_horizontal_lines)? {
            if let PyValue::Dict(d) = &desc {
                for e in obj_to_edges(d)? {
                    if orientation(&e)?.py_eq(&PyValue::str("h")) {
                        h_explicit.push(e);
                    }
                }
            } else {
                let b = bbox()?;
                h_explicit.push(Obj::from_pairs([
                    ("x0", b[0].clone()),
                    ("x1", b[2].clone()),
                    ("width", b[2].sub(&b[0])?),
                    ("top", desc.clone()),
                    ("bottom", desc.clone()),
                    ("orientation", PyValue::str("h")),
                ]));
            }
        }

        let mut h = match h_strat {
            "lines" => filter_edges(&page.edges()?, Some("h"), None, prefilter)?,
            "lines_strict" => filter_edges(&page.edges()?, Some("h"), Some(&line), prefilter)?,
            "text" => words_to_edges_h(&words, &settings.min_words_horizontal)?,
            _ => Vec::new(), // "explicit"
        };
        h.extend(h_explicit);

        let mut edges = v;
        edges.extend(h);

        let edges = merge_edges(
            edges,
            resolved(&settings.snap_x_tolerance),
            resolved(&settings.snap_y_tolerance),
            resolved(&settings.join_x_tolerance),
            resolved(&settings.join_y_tolerance),
        )?;

        filter_edges(&edges, None, None, &settings.edge_min_length)
    }
}
