//! Port of `pdfplumber/utils/clustering.py`.
//!
//! `make_cluster_dict` clusters `list(set(values))` and maps each value to
//! its cluster's index; `cluster_objects` then looks each object's key up in
//! that dict. Two CPython details decide the results:
//!
//! - **The order of `list(set(values))`**, which `sorted` starts from. The
//!   port lists the values equal to themselves first, each at its first
//!   occurrence, then the NaNs (each object once, see below) in the order
//!   of their first occurrence. Sorting distinct, totally ordered values
//!   gives the same list whatever their order. NaNs listed last stay last,
//!   in their order: a NaN is never less than anything and nothing is less
//!   than a NaN, so binary insertion leaves it where it is and a merge
//!   takes it after the other run's values. Each NaN then becomes a
//!   one-value cluster, after all the others. In CPython, a NaN's place in
//!   the set follows `hash(nan)`, its object's address, so Python's result
//!   varies with memory layout; the port's is the one CPython gives when
//!   every NaN hashes after the other values, in the order of their first
//!   occurrence. The generated cases (`oracle/unit/_nan_objects.py`) build
//!   NaN objects whose hashes give CPython exactly this order, and check it
//!   on every set built. Values that are not totally ordered for another
//!   reason (`None`, a `str` among numbers) raise or not depending on that
//!   order; the port's order is its first occurrences'.
//! - **Identity.** A set or dict lookup tries `is` before `==`, and a
//!   [`PyValue`] NaN carries its object's identity (`obj.rs`, "NaN
//!   identity"). A key function that returns an object stored in the item
//!   (`itemgetter`, `lambda x: x["top"]`) gives the lookup the very object
//!   that went into the set, so a NaN key finds its own cluster, and items
//!   sharing one NaN object share its set entry and its cluster. One that
//!   computes its value (`lambda x: -x["bottom"]`, `get_center`) gives a new
//!   object: a NaN is not found, its cluster index is `None`, and sorting
//!   the clusters raises `TypeError`.
//!
//! `HashKey` holds `Rc`-shared values, which clippy takes for interior
//! mutability; a key's hash only reads immutable parts (numbers, strings,
//! tuples), so `mutable_key_type` is allowed.
#![allow(clippy::mutable_key_type)]

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use pycompat::listsort::{sort_by_key, sort_permutation};
use pycompat::{exc, PyResult};

use crate::obj::{HashKey, PyValue};

// pdfplumber/utils/clustering.py:9 cluster_list
pub fn cluster_list(xs: Vec<PyValue>, tolerance: &PyValue) -> PyResult<Vec<Vec<PyValue>>> {
    let groups = cluster_indices(&xs, tolerance)?;
    let mut slots: Vec<Option<PyValue>> = xs.into_iter().map(Some).collect();
    Ok(groups.into_iter().map(|g| g.into_iter().map(|i| slots[i].take().expect("each value once")).collect()).collect())
}

/// `cluster_list(xs, tolerance)` as positions in `xs`: the same comparisons
/// in the same order.
fn cluster_indices(xs: &[PyValue], tolerance: &PyValue) -> PyResult<Vec<Vec<usize>>> {
    let zero = PyValue::Int(pycompat::PyInt::ZERO);
    // sorted(xs)
    let sorted = sort_permutation(xs.len(), |a, b| xs[a].lt(&xs[b]))?;
    if tolerance.py_eq(&zero) || xs.len() < 2 {
        return Ok(sorted.into_iter().map(|x| vec![x]).collect());
    }
    let mut groups = Vec::new();
    let mut iter = sorted.into_iter();
    let first = iter.next().expect("two values or more");
    let mut last = first;
    let mut current_group = vec![first];
    for x in iter {
        if xs[x].le(&xs[last].add(tolerance)?)? {
            current_group.push(x);
        } else {
            groups.push(std::mem::replace(&mut current_group, vec![x]));
        }
        last = x;
    }
    groups.push(current_group);
    Ok(groups)
}

/// `cluster_list(list(set(values)), tolerance)` (see the module
/// documentation): the distinct values (`==` with the identity shortcut),
/// those equal to themselves first, then the NaNs, each in the order of its
/// first occurrence, and the clusters as indices into `unique`.
struct SetClusters {
    unique: Vec<PyValue>,
    groups: Vec<Vec<usize>>,
}

fn set_clusters(values: impl IntoIterator<Item = PyResult<PyValue>>, tolerance: &PyValue) -> PyResult<SetClusters> {
    let mut seen: HashSet<HashKey> = HashSet::default();
    let mut unique: Vec<PyValue> = Vec::new();
    let mut nans: Vec<PyValue> = Vec::new();
    for v in values {
        let key = HashKey::new(v?)?;
        if seen.contains(&key) {
            continue;
        }
        if key.0.py_eq(&key.0) {
            unique.push(key.0.clone());
        } else {
            nans.push(key.0.clone());
        }
        seen.insert(key);
    }
    unique.append(&mut nans);
    let groups = cluster_indices(&unique, tolerance)?;
    Ok(SetClusters { unique, groups })
}

// pdfplumber/utils/clustering.py:29 make_cluster_dict
/// The items of the dict, in its order: cluster by cluster, each in sorted
/// order. A NaN key is in the dict too, where only its own object finds it.
pub fn make_cluster_dict(
    values: impl IntoIterator<Item = PyResult<PyValue>>,
    tolerance: &PyValue,
) -> PyResult<Vec<(PyValue, usize)>> {
    let s = set_clusters(values, tolerance)?;
    let mut slots: Vec<Option<PyValue>> = s.unique.into_iter().map(Some).collect();
    let mut items = Vec::with_capacity(slots.len());
    for (i, value_cluster) in s.groups.into_iter().enumerate() {
        for u in value_cluster {
            items.push((slots[u].take().expect("each value once"), i));
        }
    }
    Ok(items)
}

/// `make_cluster_dict(values, tolerance)` as a dict (`dict(chain(...))`:
/// the values are distinct).
fn clusters(
    values: impl IntoIterator<Item = PyResult<PyValue>>,
    tolerance: &PyValue,
) -> PyResult<HashMap<HashKey, usize>> {
    let items = make_cluster_dict(values, tolerance)?;
    let mut dict = HashMap::with_capacity_and_hasher(items.len(), Default::default());
    for (v, i) in items {
        dict.insert(HashKey(v), i);
    }
    Ok(dict)
}

/// `a < b` for the cluster indices `cluster_dict.get(...)` returns (an
/// `int`, or `None` for a value not found).
fn cluster_id_lt(a: Option<usize>, b: Option<usize>) -> PyResult<bool> {
    match (a, b) {
        (Some(a), Some(b)) => Ok(a < b),
        _ => {
            let name = |x: Option<usize>| if x.is_some() { "int" } else { "NoneType" };
            Err(exc::TypeError(format!("'<' not supported between instances of '{}' and '{}'", name(a), name(b))))
        }
    }
}

// pdfplumber/utils/clustering.py:42 cluster_objects
/// `key_fn` must give the stored object itself when Python's key function
/// reads it from the item, and a new value (a new NaN identity) when Python
/// computes one (see the module documentation).
pub fn cluster_objects<T: Clone>(
    xs: &[T],
    mut key_fn: impl FnMut(&T) -> PyResult<PyValue>,
    tolerance: &PyValue,
    preserve_order: bool,
) -> PyResult<Vec<Vec<T>>> {
    let clusters = clusters(xs.iter().map(&mut key_fn), tolerance)?;
    let mut cluster_tuples: Vec<(usize, Option<usize>)> = Vec::with_capacity(xs.len());
    for (i, x) in xs.iter().enumerate() {
        // cluster_dict.get(key_fn(x)): every key was hashed when the set
        // was built, so this one is hashable
        let key = key_fn(x)?;
        cluster_tuples.push((i, clusters.get(&HashKey(key)).copied()));
    }
    if !preserve_order {
        sort_by_key(&mut cluster_tuples, |t| Ok(t.1), |a, b| cluster_id_lt(*a, *b))?;
    }
    // itertools.groupby(cluster_tuples, key=get_1)
    let mut out: Vec<Vec<T>> = Vec::new();
    let mut current: Option<Option<usize>> = None;
    for (i, id) in cluster_tuples {
        match current {
            Some(c) if c == id => out.last_mut().expect("a group").push(xs[i].clone()),
            _ => {
                out.push(vec![xs[i].clone()]);
                current = Some(id);
            }
        }
    }
    Ok(out)
}
