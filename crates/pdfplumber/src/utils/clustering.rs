//! Port of `pdfplumber/utils/clustering.py`.
//!
//! `make_cluster_dict` builds `set(values)` and sorts it: the port keeps the
//! first occurrence of each value (by Python `==`) in input order before
//! sorting. Sorting distinct, totally ordered values gives the same list
//! whatever the set's internal order, so the result is Python's; with
//! values that are not totally ordered (NaN, mixed types), Python's own
//! result depends on hash values (`hash(nan)`, `hash(None)` are object
//! identities) and varies between runs.

use std::collections::HashMap;

use pycompat::listsort::sort_by_key;
use pycompat::{exc, PyResult};

use crate::obj::{HashKey, PyValue};

// pdfplumber/utils/clustering.py:9 cluster_list
pub fn cluster_list(xs: Vec<PyValue>, tolerance: &PyValue) -> PyResult<Vec<Vec<PyValue>>> {
    let zero = PyValue::Int(pycompat::PyInt::ZERO);
    if tolerance.py_eq(&zero) || xs.len() < 2 {
        let mut xs = xs;
        sorted_values(&mut xs)?;
        return Ok(xs.into_iter().map(|x| vec![x]).collect());
    }
    let mut xs = xs;
    sorted_values(&mut xs)?;
    let mut groups = Vec::new();
    let mut iter = xs.into_iter();
    let first = iter.next().expect("two values or more");
    let mut last = first.clone();
    let mut current_group = vec![first];
    for x in iter {
        if x.le(&last.add(tolerance)?)? {
            current_group.push(x.clone());
        } else {
            groups.push(std::mem::replace(&mut current_group, vec![x.clone()]));
        }
        last = x;
    }
    groups.push(current_group);
    Ok(groups)
}

/// `sorted(xs)` in place.
fn sorted_values(xs: &mut Vec<PyValue>) -> PyResult<()> {
    pycompat::listsort::sort_by_lt(xs, |a, b| a.lt(b))
}

/// The value -> cluster index map of [`make_cluster_dict`].
pub type ClusterDict = HashMap<HashKey, usize>;

// pdfplumber/utils/clustering.py:30 make_cluster_dict
pub fn make_cluster_dict(values: impl IntoIterator<Item = PyResult<PyValue>>, tolerance: &PyValue) -> PyResult<ClusterDict> {
    // list(set(values)), see the module documentation
    let mut seen: HashMap<HashKey, ()> = HashMap::new();
    let mut unique = Vec::new();
    for v in values {
        let key = HashKey::new(v?)?;
        if !seen.contains_key(&key) {
            unique.push(key.0.clone());
            seen.insert(key, ());
        }
    }
    let clusters = cluster_list(unique, tolerance)?;
    let mut dict = ClusterDict::with_capacity(seen.len());
    for (i, value_cluster) in clusters.into_iter().enumerate() {
        for val in value_cluster {
            // dict(chain(...)): a later equal key keeps the first key and
            // takes the later value (values are distinct here).
            dict.insert(HashKey(val), i);
        }
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

// pdfplumber/utils/clustering.py:45 cluster_objects
pub fn cluster_objects<T: Clone>(
    xs: &[T],
    mut key_fn: impl FnMut(&T) -> PyResult<PyValue>,
    tolerance: &PyValue,
    preserve_order: bool,
) -> PyResult<Vec<Vec<T>>> {
    let cluster_dict = make_cluster_dict(xs.iter().map(&mut key_fn), tolerance)?;
    let mut cluster_tuples: Vec<(usize, Option<usize>)> = Vec::with_capacity(xs.len());
    for (i, x) in xs.iter().enumerate() {
        let key = key_fn(x)?;
        // dict.get hashes its argument: an unhashable key raises
        let key = HashKey::new(key)?;
        cluster_tuples.push((i, cluster_dict.get(&key).copied()));
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
