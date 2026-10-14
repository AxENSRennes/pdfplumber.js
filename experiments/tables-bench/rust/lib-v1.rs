//! Port of pdfplumber table.py (TableFinder, "lines" strategies) and the utils it
//! relies on, output-identical to Python. Same algorithm as ../ts/table.ts.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;

pub const INT_X0: u8 = 1;
pub const INT_TOP: u8 = 2;
pub const V: u8 = 1;
pub const H: u8 = 2;

#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub x0: f64,
    pub top: f64,
    pub x1: f64,
    pub bottom: f64,
    pub width: f64,
    pub height: f64,
    /// 0 = None (diagonal curve edge), 1 = "v", 2 = "h"
    pub orientation: u8,
    /// 0 = line, 1 = rect_edge, 2 = curve_edge
    pub object_type: u8,
    /// Which coordinates were Python ints (only x0/top feed sum()).
    pub ints: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub snap_x: f64,
    pub snap_y: f64,
    pub join_x: f64,
    pub join_y: f64,
    pub isect_x: f64,
    pub isect_y: f64,
    pub edge_min_length: f64,
    pub edge_min_length_prefilter: f64,
    pub v_strict: bool,
    pub h_strict: bool,
}

pub type BBox = [f64; 4];

pub struct Point {
    pub x: f64,
    pub y: f64,
    pub v: Vec<usize>,
    pub h: Vec<usize>,
}

pub struct Table {
    pub cells: Vec<BBox>,
    pub bbox: BBox,
    /// Row-major indices into `cells`, None where the row has no cell.
    pub rows: Vec<Vec<Option<usize>>>,
}

pub struct TableFinder {
    pub edges: Vec<Edge>,
    pub points: Vec<Point>,
    pub cells: Vec<BBox>,
    pub tables: Vec<Table>,
}

// ---------------------------------------------------------------------------
// Python semantics helpers

/// Python 3.12 sum(values): exact int accumulation, then Neumaier-compensated
/// float accumulation where later ints are added plainly.
pub fn py_sum(values: &[f64], is_int: &[bool]) -> f64 {
    let n = values.len();
    let mut i = 0;
    let mut i_result = 0.0;
    while i < n && is_int[i] {
        i_result += values[i];
        i += 1;
    }
    if i == n {
        return i_result;
    }
    let mut f = i_result + values[i];
    let mut c = 0.0;
    i += 1;
    while i < n {
        let x = values[i];
        if is_int[i] {
            f += x;
        } else {
            let t = f + x;
            if f.abs() >= x.abs() {
                c += (f - t) + x;
            } else {
                c += (x - t) + f;
            }
            f = t;
        }
        i += 1;
    }
    if c != 0.0 && c.is_finite() {
        f += c;
    }
    f
}

#[inline]
fn py_min(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}
#[inline]
fn py_max(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}
#[inline]
fn cmp(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap()
}
/// Hash key with Python equality semantics (-0.0 == 0.0).
#[inline]
fn key(v: f64) -> u64 {
    (v + 0.0).to_bits()
}

// ---------------------------------------------------------------------------
// utils.clustering / utils.geometry

/// cluster_objects(xs, key, tolerance): groups in cluster order, input order inside.
fn cluster_objects(values: &[f64], tolerance: f64) -> Vec<Vec<usize>> {
    let mut uniq: Vec<f64> = values.to_vec();
    uniq.sort_by(|a, b| cmp(*a, *b));
    uniq.dedup_by(|a, b| a == b);
    let mut cluster_of = vec![0usize; uniq.len()];
    let mut n_clusters = uniq.len();
    if tolerance != 0.0 && uniq.len() >= 2 {
        let mut cluster = 0;
        let mut last = uniq[0];
        for k in 1..uniq.len() {
            let x = uniq[k];
            if !(x <= last + tolerance) {
                cluster += 1;
            }
            cluster_of[k] = cluster;
            last = x;
        }
        n_clusters = cluster + 1;
    } else {
        for (k, c) in cluster_of.iter_mut().enumerate() {
            *c = k;
        }
    }
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); n_clusters];
    for (i, &v) in values.iter().enumerate() {
        let k = uniq.binary_search_by(|u| cmp(*u, v)).unwrap();
        groups[cluster_of[k]].push(i);
    }
    groups.retain(|g| !g.is_empty());
    groups
}

/// snap_objects(edges, "x0" | "top", tolerance)
fn snap_objects(objs: &[Edge], by_x0: bool, tolerance: f64) -> Vec<Edge> {
    let values: Vec<f64> = objs.iter().map(|e| if by_x0 { e.x0 } else { e.top }).collect();
    let bit = if by_x0 { INT_X0 } else { INT_TOP };
    let mut out = Vec::with_capacity(objs.len());
    let mut vals = Vec::new();
    let mut ints = Vec::new();
    for group in cluster_objects(&values, tolerance) {
        vals.clear();
        ints.clear();
        for &i in &group {
            vals.push(values[i]);
            ints.push(objs[i].ints & bit != 0);
        }
        let avg = py_sum(&vals, &ints) / group.len() as f64;
        for &i in &group {
            let mut e = objs[i];
            let value = avg - values[i];
            // move_object: the moved coordinates become floats
            if by_x0 {
                e.x0 += value;
                e.x1 += value;
                e.ints &= !INT_X0;
            } else {
                e.top += value;
                e.bottom += value;
                e.ints &= !INT_TOP;
            }
            out.push(e);
        }
    }
    out
}

fn filter_edges(edges: &[Edge], orientation: Option<u8>, line_only: bool, min_length: f64) -> Vec<Edge> {
    edges
        .iter()
        .filter(|e| {
            (!line_only || e.object_type == 0)
                && orientation.map_or(true, |o| e.orientation == o)
                && (if e.orientation == V { e.height } else { e.width }) >= min_length
        })
        .copied()
        .collect()
}

// ---------------------------------------------------------------------------
// table.py

fn join_edge_group(mut group: Vec<Edge>, horizontal: bool, tolerance: f64) -> Vec<Edge> {
    if horizontal {
        group.sort_by(|a, b| cmp(a.x0, b.x0));
    } else {
        group.sort_by(|a, b| cmp(a.top, b.top));
    }
    let mut joined: Vec<Edge> = vec![group[0]];
    for e in &group[1..] {
        let last = joined.last_mut().unwrap();
        if horizontal {
            if e.x0 <= last.x1 + tolerance {
                if e.x1 > last.x1 {
                    // resize_object(last, "x1", e.x1)
                    last.width = e.x1 - last.x0;
                    last.x1 = e.x1;
                }
            } else {
                joined.push(*e);
            }
        } else if e.top <= last.bottom + tolerance {
            if e.bottom > last.bottom {
                // resize_object(last, "bottom", e.bottom)
                let diff = e.bottom - last.bottom;
                last.height += diff;
                last.bottom = e.bottom;
            }
        } else {
            joined.push(*e);
        }
    }
    joined
}

fn merge_edges(edges: Vec<Edge>, s: &Settings) -> Vec<Edge> {
    let mut edges = edges;
    if s.snap_x > 0.0 || s.snap_y > 0.0 {
        let v: Vec<Edge> = edges.iter().filter(|e| e.orientation == V).copied().collect();
        let h: Vec<Edge> = edges.iter().filter(|e| e.orientation != V).copied().collect();
        edges = snap_objects(&v, true, s.snap_x);
        edges.extend(snap_objects(&h, false, s.snap_y));
    }
    let gkey = |e: &Edge| if e.orientation == H { e.top } else { e.x0 };
    // sorted by ("h", top) / ("v", x0): "h" groups first
    edges.sort_by(|a, b| (a.orientation != H).cmp(&(b.orientation != H)).then(cmp(gkey(a), gkey(b))));
    let mut out = Vec::with_capacity(edges.len());
    let mut start = 0;
    for i in 1..=edges.len() {
        if i < edges.len() && edges[i].orientation == edges[start].orientation && gkey(&edges[i]) == gkey(&edges[start]) {
            continue;
        }
        let horizontal = edges[start].orientation == H;
        let tol = if horizontal { s.join_x } else { s.join_y };
        out.extend(join_edge_group(edges[start..i].to_vec(), horizontal, tol));
        start = i;
    }
    out
}

fn get_edges(page_edges: &[Edge], s: &Settings) -> Vec<Edge> {
    let mut edges = filter_edges(page_edges, Some(V), s.v_strict, s.edge_min_length_prefilter);
    edges.extend(filter_edges(page_edges, Some(H), s.h_strict, s.edge_min_length_prefilter));
    let merged = merge_edges(edges, s);
    filter_edges(&merged, None, false, s.edge_min_length)
}

fn edges_to_intersections(edges: &[Edge], xtol: f64, ytol: f64) -> Vec<Point> {
    let mut v: Vec<usize> = (0..edges.len()).filter(|&i| edges[i].orientation == V).collect();
    let mut h: Vec<usize> = (0..edges.len()).filter(|&i| edges[i].orientation == H).collect();
    v.sort_by(|&a, &b| cmp(edges[a].x0, edges[b].x0).then(cmp(edges[a].top, edges[b].top)));
    h.sort_by(|&a, &b| cmp(edges[a].top, edges[b].top).then(cmp(edges[a].x0, edges[b].x0)));
    let mut index: HashMap<(u64, u64), usize> = HashMap::new();
    let mut points: Vec<Point> = Vec::new();
    for &vi in &v {
        let ve = &edges[vi];
        for &hi in &h {
            let he = &edges[hi];
            if ve.top <= he.top + ytol && ve.bottom >= he.top - ytol && ve.x0 >= he.x0 - xtol && ve.x0 <= he.x1 + xtol {
                let k = *index.entry((key(ve.x0), key(he.top))).or_insert_with(|| {
                    points.push(Point { x: ve.x0, y: he.top, v: Vec::new(), h: Vec::new() });
                    points.len() - 1
                });
                points[k].v.push(vi);
                points[k].h.push(hi);
            }
        }
    }
    points.sort_by(|a, b| cmp(a.x, b.x).then(cmp(a.y, b.y)));
    points
}

struct Cell {
    bbox: BBox,
    corners: [usize; 4],
}

fn intersections_to_cells(edges: &[Edge], points: &[Point]) -> Vec<Cell> {
    let n = points.len();
    // edges_to_set(...) compares edges by bbox value: intern bboxes to ids
    let mut bbox_ids: HashMap<[u64; 4], u32> = HashMap::new();
    let mut id_of = |e: &Edge| {
        let next = bbox_ids.len() as u32;
        *bbox_ids.entry([key(e.x0), key(e.top), key(e.x1), key(e.bottom)]).or_insert(next)
    };
    let v_ids: Vec<Vec<u32>> = points.iter().map(|p| p.v.iter().map(|&i| id_of(&edges[i])).collect()).collect();
    let h_ids: Vec<Vec<u32>> = points.iter().map(|p| p.h.iter().map(|&i| id_of(&edges[i])).collect()).collect();
    let share = |a: &[u32], b: &[u32]| a.iter().any(|x| b.contains(x));
    let connects = |i: usize, j: usize| (points[i].x == points[j].x && share(&v_ids[i], &v_ids[j])) || (points[i].y == points[j].y && share(&h_ids[i], &h_ids[j]));

    let mut index: HashMap<(u64, u64), usize> = HashMap::with_capacity(n);
    let mut by_y: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut pos_in_row = vec![0usize; n];
    for (i, p) in points.iter().enumerate() {
        index.insert((key(p.x), key(p.y)), i);
        let row = by_y.entry(key(p.y)).or_default();
        pos_in_row[i] = row.len();
        row.push(i);
    }

    let mut cells = Vec::new();
    for i in 0..n.saturating_sub(1) {
        let pt = &points[i];
        let row = &by_y[&key(pt.y)];
        let mut b = i + 1;
        'search: while b < n && points[b].x == pt.x {
            if connects(i, b) {
                for &ri in &row[pos_in_row[i] + 1..] {
                    if !connects(i, ri) {
                        continue;
                    }
                    if let Some(&br) = index.get(&(key(points[ri].x), key(points[b].y))) {
                        if connects(br, ri) && connects(br, b) {
                            cells.push(Cell { bbox: [pt.x, pt.y, points[ri].x, points[b].y], corners: [i, b, ri, br] });
                            break 'search;
                        }
                    }
                }
            }
            b += 1;
        }
    }
    cells
}

fn cells_to_tables(cells: Vec<Cell>, n_points: usize) -> Vec<Vec<Cell>> {
    let mut mark = vec![0u32; n_points];
    let mut gen = 1u32;
    let mut remaining = cells;
    let mut current: Vec<Cell> = Vec::new();
    let mut tables: Vec<Vec<Cell>> = Vec::new();
    while !remaining.is_empty() {
        let initial = current.len();
        let mut next = Vec::with_capacity(remaining.len());
        for cell in remaining {
            let c = cell.corners;
            if current.is_empty() || c.iter().any(|&k| mark[k] == gen) {
                for &k in &c {
                    mark[k] = gen;
                }
                current.push(cell);
            } else {
                next.push(cell);
            }
        }
        remaining = next;
        if current.len() == initial {
            tables.push(std::mem::take(&mut current));
            gen += 1;
        }
    }
    if !current.is_empty() {
        tables.push(current);
    }
    let min_key = |t: &Vec<Cell>| {
        let mut best = (t[0].bbox[1], t[0].bbox[0]);
        for c in t {
            if c.bbox[1] < best.0 || (c.bbox[1] == best.0 && c.bbox[0] < best.1) {
                best = (c.bbox[1], c.bbox[0]);
            }
        }
        best
    };
    let mut keyed: Vec<((f64, f64), Vec<Cell>)> = tables.into_iter().map(|t| (min_key(&t), t)).collect();
    keyed.sort_by(|a, b| cmp(a.0 .0, b.0 .0).then(cmp(a.0 .1, b.0 .1)));
    keyed.into_iter().filter(|(_, t)| t.len() > 1).map(|(_, t)| t).collect()
}

fn table_rows(cells: &[BBox]) -> Vec<Vec<Option<usize>>> {
    let mut order: Vec<usize> = (0..cells.len()).collect();
    order.sort_by(|&a, &b| cmp(cells[a][1], cells[b][1]).then(cmp(cells[a][0], cells[b][0])));
    let mut xs: Vec<f64> = cells.iter().map(|c| c[0]).collect();
    xs.sort_by(|a, b| cmp(*a, *b));
    xs.dedup_by(|a, b| a == b);
    let mut rows = Vec::new();
    let mut start = 0;
    for i in 1..=order.len() {
        if i < order.len() && cells[order[i]][1] == cells[order[start]][1] {
            continue;
        }
        let group = &order[start..i];
        rows.push(xs.iter().map(|&x| group.iter().rev().find(|&&k| cells[k][0] == x).copied()).collect());
        start = i;
    }
    rows
}

/// TableFinder(page, settings) followed by `rows` on every table.
pub fn find_tables(page_edges: &[Edge], s: &Settings) -> TableFinder {
    let edges = get_edges(page_edges, s);
    let points = edges_to_intersections(&edges, s.isect_x, s.isect_y);
    let cells = intersections_to_cells(&edges, &points);
    let cell_boxes: Vec<BBox> = cells.iter().map(|c| c.bbox).collect();
    let tables = cells_to_tables(cells, points.len())
        .into_iter()
        .map(|group| {
            let tc: Vec<BBox> = group.iter().map(|c| c.bbox).collect();
            let mut b = tc[0];
            for c in &tc {
                b = [py_min(b[0], c[0]), py_min(b[1], c[1]), py_max(b[2], c[2]), py_max(b[3], c[3])];
            }
            let rows = table_rows(&tc);
            Table { cells: tc, bbox: b, rows }
        })
        .collect();
    TableFinder { edges, points, cells: cell_boxes, tables }
}

// ---------------------------------------------------------------------------
// Corpus job layout and trace serialization (shared with dump_tables.py)

pub const EDGE_BYTES: usize = 51;

pub fn read_job(b: &[u8]) -> (Settings, Vec<Edge>, usize) {
    let f = |o: usize| f64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let s = Settings {
        snap_x: f(0),
        snap_y: f(8),
        join_x: f(16),
        join_y: f(24),
        isect_x: f(32),
        isect_y: f(40),
        edge_min_length: f(48),
        edge_min_length_prefilter: f(56),
        v_strict: false,
        h_strict: false,
    };
    let n = u32::from_le_bytes(b[64..68].try_into().unwrap()) as usize;
    let mut edges = Vec::with_capacity(n);
    let mut o = 68;
    for _ in 0..n {
        let flags = b[o + 48];
        edges.push(Edge {
            x0: f(o),
            top: f(o + 8),
            x1: f(o + 16),
            bottom: f(o + 24),
            width: f(o + 32),
            height: f(o + 40),
            orientation: b[o + 49],
            object_type: b[o + 50],
            ints: (if flags & 1 != 0 { INT_X0 } else { 0 }) | (if flags & 2 != 0 { INT_TOP } else { 0 }),
        });
        o += EDGE_BYTES;
    }
    (s, edges, o)
}

fn num(out: &mut String, v: f64) {
    if v.fract() == 0.0 && v.abs() <= 9007199254740991.0 {
        let _ = write!(out, "d{}", v as i64);
    } else {
        let _ = write!(out, "f{:016x}", v.to_bits());
    }
}

fn bbox(out: &mut String, vals: &[f64]) {
    for (k, v) in vals.iter().enumerate() {
        if k > 0 {
            out.push(',');
        }
        num(out, *v);
    }
}

pub fn serialize(tf: &TableFinder) -> String {
    let mut out = String::new();
    for e in &tf.edges {
        out.push_str("E ");
        bbox(&mut out, &[e.x0, e.top, e.x1, e.bottom, e.width, e.height]);
        out.push_str(match e.orientation {
            V => " v\n",
            H => " h\n",
            _ => " None\n",
        });
    }
    for p in &tf.points {
        out.push_str("I ");
        num(&mut out, p.x);
        out.push(',');
        num(&mut out, p.y);
        let _ = writeln!(out, " {} {}", p.v.len(), p.h.len());
    }
    for c in &tf.cells {
        out.push_str("C ");
        bbox(&mut out, c);
        out.push('\n');
    }
    for t in &tf.tables {
        out.push_str("T ");
        bbox(&mut out, &t.bbox);
        let _ = writeln!(out, " {}", t.cells.len());
        for c in &t.cells {
            out.push_str("  c ");
            bbox(&mut out, c);
            out.push('\n');
        }
        for r in &t.rows {
            out.push_str("  r");
            for c in r {
                out.push(' ');
                match c {
                    Some(k) => bbox(&mut out, &t.cells[*k]),
                    None => out.push('-'),
                }
            }
            out.push('\n');
        }
    }
    if out.is_empty() {
        out.push('\n');
    }
    out.push_str("!ok\n");
    out
}

// ---------------------------------------------------------------------------
// wasm exports

struct Global {
    input: Vec<u8>,
    out: Vec<u8>,
}

thread_local! {
    static G: RefCell<Global> = RefCell::new(Global { input: Vec::new(), out: Vec::new() });
}

#[no_mangle]
pub extern "C" fn input_buffer(len: usize) -> *mut u8 {
    G.with(|g| {
        let g = &mut *g.borrow_mut();
        g.input.resize(len, 0);
        g.input.as_mut_ptr()
    })
}

#[no_mangle]
pub extern "C" fn out_ptr() -> *const u8 {
    G.with(|g| g.borrow().out.as_ptr())
}

#[no_mangle]
pub extern "C" fn out_len() -> usize {
    G.with(|g| g.borrow().out.len())
}

/// Full trace text (for verification against the Python oracle).
#[no_mangle]
pub extern "C" fn run_trace() {
    G.with(|g| {
        let g = &mut *g.borrow_mut();
        let (s, edges, _) = read_job(&g.input);
        g.out = serialize(&find_tables(&edges, &s)).into_bytes();
    })
}

/// Tables only, as a compact tape: u32 ntables; per table u32 ncells, cells
/// (4 x f64 each), bbox (4 x f64), u32 nrows, u32 ncols, i32 cell index per slot.
#[no_mangle]
pub extern "C" fn run_tables() -> u32 {
    G.with(|g| {
        let g = &mut *g.borrow_mut();
        let (s, edges, _) = read_job(&g.input);
        let tf = find_tables(&edges, &s);
        let out = &mut g.out;
        out.clear();
        let u32le = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        u32le(out, tf.tables.len() as u32);
        for t in &tf.tables {
            u32le(out, t.cells.len() as u32);
            for c in t.cells.iter().chain(std::iter::once(&t.bbox)) {
                for v in c {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
            u32le(out, t.rows.len() as u32);
            u32le(out, t.rows.first().map_or(0, |r| r.len()) as u32);
            for r in &t.rows {
                for c in r {
                    out.extend_from_slice(&c.map_or(-1i32, |k| k as i32).to_le_bytes());
                }
            }
        }
        tf.tables.len() as u32
    })
}
