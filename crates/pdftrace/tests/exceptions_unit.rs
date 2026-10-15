//! `pycompat::ExcKind` against the exception classes of pdfminer and
//! pdfplumber listed by `oracle/unit/exceptions.py`: every class is a kind
//! with the same `__name__` and `__bases__`, every pdfminer/pdfplumber kind
//! is a class, and `is_instance` is Python's `issubclass`.

use pdftrace::unit::UnitData;
use pdftrace::value::Value;
use pycompat::ExcKind;

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a str: {v:?}"))
}

fn qualified(kind: ExcKind) -> String {
    format!("{}.{}", kind.module(), kind.qualname())
}

#[test]
fn exception_classes() {
    let cases = UnitData::open("exceptions").cases("classes");
    assert!(!cases.is_empty());
    let mut python = Vec::new();
    for (args, out) in &cases {
        let key = format!("{}.{}", s(&args[0]), s(&args[1]));
        let kind = ExcKind::ALL
            .iter()
            .copied()
            .find(|k| qualified(*k) == key)
            .unwrap_or_else(|| panic!("{key} has no ExcKind"));
        let Value::List(out) = out else { panic!() };
        assert_eq!(kind.name(), s(&out[0]), "{key}: __name__");
        let Value::List(bases) = &out[1] else { panic!() };
        let bases: Vec<String> = bases
            .iter()
            .map(|b| match b {
                Value::List(p) => format!("{}.{}", s(&p[0]), s(&p[1])),
                other => panic!("{other:?}"),
            })
            .collect();
        let rust: Vec<String> = kind.bases().iter().map(|b| qualified(*b)).collect();
        assert_eq!(rust, bases, "{key}: __bases__");
        python.push(key);
    }
    for kind in ExcKind::ALL {
        if kind.module().starts_with("pdfminer") || kind.module().starts_with("pdfplumber") {
            assert!(python.contains(&qualified(*kind)), "{} is not a Python class", qualified(*kind));
        }
    }
}

/// The kind of `[module, qualname]`, if the port has one.
fn kind_of(v: &Value) -> Option<ExcKind> {
    let Value::List(p) = v else { panic!("{v:?}") };
    let key = format!("{}.{}", s(&p[0]), s(&p[1]));
    ExcKind::ALL.iter().copied().find(|k| qualified(*k) == key)
}

#[test]
fn subclass_relation() {
    let cases = UnitData::open("exceptions").cases("subclass");
    let mut checked = 0;
    for (args, out) in &cases {
        let (Some(a), Some(b)) = (kind_of(&args[0]), kind_of(&args[1])) else { continue };
        let Value::Bool(expected) = out else { panic!("{out:?}") };
        assert_eq!(a.is_instance(b), *expected, "issubclass({}, {})", qualified(a), qualified(b));
        let err = pycompat::PyErr::new(a, "");
        assert_eq!(err.is_instance(b), *expected, "isinstance({}(), {})", qualified(a), qualified(b));
        checked += 1;
    }
    // every kind except NotPorted, which is not a Python class
    let kinds = ExcKind::ALL.len() - 1;
    assert_eq!(checked, kinds * kinds, "pairs of kinds checked");
}
