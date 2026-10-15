//! Replays the cases of `oracle/unit/utils.py` (pdfminer and pdfplumber
//! results) through `pdfminer::{utils, casting, data_structures, pdfcolor}`
//! and `pdfplumber::ctm`.

mod utils_layout_common;

use std::rc::Rc;

use pdfminer::casting::{safe_cmyk, safe_float, safe_int, safe_matrix, safe_rect, safe_rect_list, safe_rgb};
use pdfminer::data_structures::NumberTree;
use pdfminer::pdfcolor::{predefined_colorspace, predefined_colorspaces};
use pdfminer::pdftypes::PDFObject;
use pdfminer::utils::{
    apply_matrix_norm, apply_matrix_pt, apply_matrix_rect, bbox2str, decode_text, drange, format_int_alpha,
    format_int_roman, fsplit, get_bound, isnumber, matrix2str, mult_matrix, nunpack, parse_rect, pick,
    translate_matrix, uniq, Plane, PlaneItem, Point, Rect,
};
use pdfplumber::ctm::CTM;
use pdftrace::value::Value;
use pycompat::recursion;
use pycompat::{Num, NumKey, PyInt};
use utils_layout_common::*;

#[test]
fn matrix() {
    replay("utils", "matrix", |a| {
        let m: [Num; 6] = nums(&a[1]);
        match str_arg(&a[0]) {
            "mult_matrix" => result(mult_matrix(&m, &nums(&a[2])), |r| tuple(&r)),
            "translate_matrix" => result(translate_matrix(&m, &nums(&a[2])), |r| tuple(&r)),
            "apply_matrix_pt" => result(apply_matrix_pt(&m, &nums(&a[2])), |r| tuple(&r)),
            "apply_matrix_rect" => result(apply_matrix_rect(&m, &nums(&a[2])), |r| tuple(&r)),
            "apply_matrix_norm" => result(apply_matrix_norm(&m, &nums(&a[2])), |r| tuple(&r)),
            other => panic!("{other}"),
        }
    });
}

#[test]
fn geometry() {
    let (_doc, weak) = doc();
    replay("utils", "geometry", |a| match str_arg(&a[0]) {
        "get_bound" => {
            let pts: Vec<Point> = items(&a[1]).iter().map(nums).collect();
            tuple(&get_bound(&pts))
        }
        "drange" => result(drange(&num(&a[1]), &num(&a[2]), int_arg(&a[3])), |r| {
            Value::Tuple(vec![Value::Int(PyInt::Small(r.start)), Value::Int(PyInt::Small(r.end))])
        }),
        "bbox2str" => result(bbox2str(&nums(&a[1])), Value::str),
        "matrix2str" => result(matrix2str(&nums(&a[1])), Value::str),
        "parse_rect" => {
            let seq: Vec<_> = items(&a[1]).iter().map(|x| iter_item(x, &weak)).collect();
            result(parse_rect(seq.into_iter().map(Ok)), |r| float_tuple(&r))
        }
        other => panic!("{other}"),
    });
}

#[test]
fn iter() {
    replay("utils", "iter", |a| {
        let seq = items(&a[1]);
        match str_arg(&a[0]) {
            "uniq" => Value::List(uniq(seq.iter(), |x| NumKey::new(num(x))).into_iter().cloned().collect()),
            "fsplit" => {
                let (t, f) =
                    fsplit(|x: &&Value| Ok(num(x).py_gt(&Num::Int(PyInt::ZERO))), seq.iter()).expect("no error");
                Value::Tuple(vec![
                    Value::List(t.into_iter().cloned().collect()),
                    Value::List(f.into_iter().cloned().collect()),
                ])
            }
            "pick" => {
                // func: -x for an int (a bool is one), x * 2 for a float
                let score = |i: &usize| match num(&seq[*i]) {
                    Num::Int(x) => Num::Int(x.neg()),
                    Num::Float(x) => Num::Float(x * 2.0),
                };
                match pick(0..seq.len(), score, None) {
                    Some(i) => seq[i].clone(),
                    None => Value::str("none"),
                }
            }
            other => panic!("{other}"),
        }
    });
}

#[test]
fn numbers() {
    let (_doc, weak) = doc();
    replay("utils", "numbers", |a| match str_arg(&a[0]) {
        "isnumber" => Value::Bool(isnumber(&build(&a[1], &weak))),
        "nunpack" => {
            let Value::Bytes(s) = &a[1] else { panic!() };
            Value::Int(nunpack(s, int_arg(&a[2])))
        }
        "format_int_roman" => {
            let Value::Int(v) = &a[1] else { panic!() };
            result(format_int_roman(v), Value::str)
        }
        "format_int_alpha" => {
            let Value::Int(v) = &a[1] else { panic!() };
            result(format_int_alpha(v), Value::str)
        }
        other => panic!("{other}"),
    });
}

#[test]
fn decode_text_cases() {
    replay("utils", "decode_text", |a| {
        let Value::Bytes(s) = &a[0] else { panic!() };
        Value::str(decode_text(s))
    });
}

fn opt_floats(r: Option<impl AsRef<[f64]>>) -> Value {
    r.map_or(Value::None, |r| float_tuple(r.as_ref()))
}

#[test]
fn casting() {
    let (_doc, weak) = doc();
    replay("utils", "casting", |a| {
        let args: Vec<PDFObject> = a[1..].iter().map(|x| build(x, &weak)).collect();
        match str_arg(&a[0]) {
            "safe_int" => result(safe_int(&args[0]), |r| r.map_or(Value::None, Value::Int)),
            "safe_float" => safe_float(&args[0]).map_or(Value::None, Value::Float),
            "safe_matrix" => opt_floats(safe_matrix(&args[0], &args[1], &args[2], &args[3], &args[4], &args[5])),
            "safe_rgb" => opt_floats(safe_rgb(&args[0], &args[1], &args[2])),
            "safe_cmyk" => opt_floats(safe_cmyk(&args[0], &args[1], &args[2], &args[3])),
            "safe_rect" => opt_floats(safe_rect(&args[0], &args[1], &args[2], &args[3])),
            "safe_rect_list" => result(safe_rect_list(&args[0]), opt_floats),
            other => panic!("{other}"),
        }
    });
}

/// A box of the plane tests.
struct TBox {
    i: i64,
    bbox: Rect,
}

#[derive(Clone)]
struct BoxRef(Rc<TBox>);

impl PlaneItem for BoxRef {
    fn plane_bbox(&self) -> Rect {
        self.0.bbox.clone()
    }

    fn plane_id(&self) -> usize {
        Rc::as_ptr(&self.0) as usize
    }
}

#[test]
fn plane() {
    replay("utils", "plane", |a| {
        let mut plane: Plane<BoxRef> = Plane::new(&nums(&a[0]), int_arg(&a[1]));
        let mut objs: std::collections::HashMap<i64, BoxRef> = Default::default();
        let ids = |v: Vec<BoxRef>| Value::List(v.into_iter().map(|b| Value::Int(PyInt::Small(b.0.i))).collect());
        let mut out = Vec::new();
        for op in items(&a[2]) {
            let op = items(op);
            let r = match str_arg(&op[0]) {
                "add" => {
                    let b = BoxRef(Rc::new(TBox { i: int_arg(&op[1]), bbox: nums(&op[2]) }));
                    objs.insert(int_arg(&op[1]), b.clone());
                    result(plane.add(&b), |()| Value::None)
                }
                "remove" => result(plane.remove(&objs[&int_arg(&op[1])]), |()| Value::None),
                "find" => result(plane.find(&nums(&op[1])), ids),
                "iter" => ids(plane.iter().cloned().collect()),
                "len" => Value::Int(PyInt::from(plane.len())),
                "contains" => Value::Bool(plane.contains(&objs[&int_arg(&op[1])])),
                other => panic!("{other}"),
            };
            let stop = matches!(r, Value::Exception { .. });
            out.push(r);
            if stop {
                break;
            }
        }
        Value::List(out)
    });
}

#[test]
fn numbertree() {
    replay("utils", "numbertree", |a| {
        let (doc, weak) = doc();
        fill_table(&doc, &weak, &a[3]);
        let root = build(&a[2], &weak);
        let list = |l: Option<pdfminer::pdftypes::PDFList>| l.map_or(Value::None, |l| Value::from(&PDFObject::List(l)));
        recursion::with_depth(depth(&a[1]), || {
            // run(): the generator's closure, one frame
            let r = recursion::py_frame().and_then(|_frame| {
                let tree = NumberTree::new(&root)?;
                let values = tree.values()?;
                Ok((tree, values))
            });
            result(r, |(tree, values)| {
                Value::List(vec![
                    list(tree.nums),
                    list(tree.kids),
                    list(tree.limits),
                    Value::List(
                        values.iter().map(|(k, v)| Value::Tuple(vec![Value::from(k), Value::from(v)])).collect(),
                    ),
                ])
            })
        })
    });
}

#[test]
fn pdfcolor() {
    let spaces = predefined_colorspaces();
    let mut i = 0;
    replay("utils", "pdfcolor", |a| {
        let cs = &spaces[i];
        i += 1;
        assert_eq!(cs.name, str_arg(&a[0]));
        // PREDEFINED_COLORSPACE[name] is the shared instance
        let by_name = predefined_colorspace(&cs.name).expect("a predefined color space");
        assert!(Rc::ptr_eq(&by_name, cs));
        Value::List(vec![Value::str(cs.name.clone()), Value::from(&cs.ncomponents), result(cs.repr(), Value::str)])
    });
    assert_eq!(i, spaces.len());
}

fn ctm_values(a: &[Value]) -> Vec<Value> {
    let [a0, b, c, d, e, f] = nums(&a[0]);
    let m = CTM::new(a0, b, c, d, e, f);
    vec![
        result(m.scale_x(), Value::Float),
        result(m.scale_y(), Value::Float),
        result(m.skew_x(), Value::Float),
        result(m.skew_y(), Value::Float),
        num_value(&m.translation_x()),
        num_value(&m.translation_y()),
    ]
}

/// Every property. Not on WebAssembly, where the C library's `atan2` is
/// musl's, not the oracle's glibc (see `pdfplumber/src/ctm.rs`).
#[test]
#[cfg_attr(target_family = "wasm", ignore = "atan2 is musl's on WebAssembly")]
fn ctm() {
    replay("utils", "ctm", |a| Value::List(ctm_values(a)));
}

/// The properties that do not use `atan2`, on every target.
#[test]
fn ctm_scale_translation() {
    let cases = pdftrace::unit::UnitData::open("utils").cases("ctm");
    for (a, expected) in &cases {
        let actual = ctm_values(a);
        let Value::List(expected) = expected else { panic!() };
        for i in [0, 1, 4, 5] {
            assert!(matches(&actual[i], &expected[i]), "{a:?}: field {i}: {:?} != {:?}", actual[i], expected[i]);
        }
    }
}
