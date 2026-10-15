//! Replays the cases of `oracle/unit/ccitt.py` (pdfminer results) through
//! `pdfminer::ccitt`: the class trees, `BitParser.add`, the parser driven
//! through its methods, `ccittfaxdecode` on generated and real streams, and
//! at depths up to Python's frame limit.

use std::borrow::Cow;
use std::rc::Rc;

use pdfminer::ccitt::{
    ccittfaxdecode, ccittfaxdecode_obj, BitParserState, CCITTFaxDecoder, CCITTG4Parser, Leaf, PrintLines, Slot, BLACK,
    MODE, UNCOMPRESSED, WHITE,
};
use pdfminer::pdftypes::{PDFDict, PDFList, PDFObjRef, PDFObject, PDFStream};
use pdfminer::psparser::{LIT_bytes, KWD, LIT};
use pdftrace::canon::sha256_hex;
use pdftrace::unit::UnitData;
use pdftrace::value::{NameValue, Value};
use pycompat::recursion;
use pycompat::{PyInt, PyResult};

/// A PDF object of a case (references have no document: ccitt never
/// resolves them).
fn build(v: &Value) -> PDFObject {
    match v {
        Value::None => PDFObject::Null,
        Value::Bool(b) => PDFObject::Bool(*b),
        Value::Int(i) => PDFObject::Int(i.clone()),
        Value::Float(f) => PDFObject::Float(*f),
        Value::Bytes(b) => PDFObject::Bytes(Rc::from(&b[..])),
        Value::Literal(NameValue::Str(s)) => PDFObject::Literal(LIT(s.as_str().expect("a literal name"))),
        Value::Literal(NameValue::Bytes(b)) => PDFObject::Literal(LIT_bytes(b)),
        Value::Keyword(k) => PDFObject::Keyword(KWD(k)),
        Value::List(items) => PDFObject::List(PDFList::new(items.iter().map(build).collect())),
        Value::Dict(items) => PDFObject::Dict(dict(items)),
        Value::Ref(objid) => PDFObject::Ref(PDFObjRef::new(None, objid.clone())),
        Value::Stream(sv) => {
            let Value::Bytes(raw) = &sv.raw else { panic!("a stream with its raw data: {sv:?}") };
            let s = PDFStream::new(dict(&sv.attrs), Rc::from(&raw[..]), None);
            if let (Value::Int(objid), Value::Int(genno)) = (&sv.objid, &sv.genno) {
                s.set_objid(objid.clone(), genno.clone());
            }
            PDFObject::Stream(Rc::new(s))
        }
        other => panic!("not a PDF object: {other:?}"),
    }
}

fn dict(items: &[(Value, Value)]) -> PDFDict {
    PDFDict::from_pairs(items.iter().map(|(k, x)| (Rc::from(k.as_str().expect("a str key")), build(x))))
}

fn bytes(v: &Value) -> &[u8] {
    match v {
        Value::Bytes(b) => b,
        other => panic!("not bytes: {other:?}"),
    }
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(i) => i.as_i64().expect("a small int"),
        other => panic!("not an int: {other:?}"),
    }
}

fn int_value(i: i64) -> Value {
    Value::Int(PyInt::from(i))
}

/// `output` of the generator: the bytes, hashed beyond 512, or the
/// exception.
fn output(r: PyResult<Vec<u8>>) -> Value {
    match r {
        Ok(b) if b.len() <= 512 => Value::Bytes(b),
        Ok(b) => Value::Tuple(vec![
            Value::str("sha256"),
            int_value(i64::try_from(b.len()).expect("a length")),
            Value::str(sha256_hex(&b)),
        ]),
        Err(e) => Value::from(&e),
    }
}

fn replay(name: &str, f: impl FnMut(&[Value]) -> Value) {
    replay_in("ccitt", name, f);
}

/// The cases `name` of the generator `module`.
fn replay_in(module: &str, name: &str, mut f: impl FnMut(&[Value]) -> Value) {
    let cases = UnitData::open(module).cases(name);
    assert!(!cases.is_empty(), "{name}: no cases");
    let mut failures = Vec::new();
    for (k, (args, expected)) in cases.iter().enumerate() {
        let actual = f(args);
        if !actual.same(expected) {
            let args = format!("{args:?}");
            failures.push(format!(
                "case {k}: {}\n    python: {expected:?}\n    rust:   {actual:?}",
                args.chars().take(2000).collect::<String>()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{name}: {}/{} cases differ; first ones:\n{}",
        failures.len(),
        cases.len(),
        failures.iter().take(5).map(|f| f.chars().take(4000).collect::<String>()).collect::<Vec<_>>().join("\n")
    );
}

/// A tree as Python's nested lists, built bottom-up without recursion.
fn tree_value(tree: &BitParserState) -> Value {
    let mut done: Vec<Option<Value>> = vec![None; tree.nodes.len()];
    // children have larger indexes than their parent (`add` appends them)
    for n in (0..tree.nodes.len()).rev() {
        let mut slot = |s: &Slot| match s {
            Slot::None => Value::None,
            Slot::Node(m) => done[*m as usize].take().expect("a child after its parent"),
            Slot::Leaf(Leaf::Int(i)) => int_value(*i),
            Slot::Leaf(Leaf::Str(s)) => Value::str(s.to_string()),
        };
        let pair = [slot(&tree.nodes[n][0]), slot(&tree.nodes[n][1])];
        done[n] = Some(Value::List(pair.into()));
    }
    done[0].take().expect("the root")
}

fn leaf(v: &Value) -> Leaf {
    match v {
        Value::Int(i) => Leaf::Int(i.as_i64().expect("small")),
        Value::Str(s) => Leaf::Str(Cow::Owned(s.as_str().expect("no surrogate").to_owned())),
        other => panic!("not a leaf: {other:?}"),
    }
}

fn opt_leaf(v: &Value) -> Option<Leaf> {
    match v {
        Value::None => None,
        v => Some(leaf(v)),
    }
}

/// The class trees as named by the generator.
fn class_trees() -> [(&'static str, &'static BitParserState); 4] {
    [("MODE", &MODE), ("WHITE", &WHITE), ("BLACK", &BLACK), ("UNCOMPRESSED", &UNCOMPRESSED)]
}

/// `"TREE:bits"`: the class tree holding the node, and its path.
fn node_path(tree: &'static BitParserState, node: u32) -> String {
    let (name, _) = class_trees().into_iter().find(|(_, t)| std::ptr::eq(*t, tree)).expect("a class tree");
    let mut stack = vec![(0u32, String::new())];
    while let Some((n, prefix)) = stack.pop() {
        if n == node {
            return format!("{name}:{prefix}");
        }
        for b in 0..2 {
            if let Slot::Node(m) = tree.nodes[n as usize][b] {
                stack.push((m, format!("{prefix}{b}")));
            }
        }
    }
    panic!("node {node} not in {name}")
}

fn digits(s: &str) -> Vec<i8> {
    s.chars().map(|c| i8::try_from(c.to_digit(10).expect("a digit")).expect("a digit")).collect()
}

fn text(a: &[i8]) -> String {
    a.iter().map(|b| b.to_string()).collect()
}

fn opt_int(v: Option<i64>) -> Value {
    v.map_or(Value::None, int_value)
}

#[test]
fn trees() {
    replay("trees", |a| {
        let name = a[0].as_str().expect("a tree name");
        let (_, tree) = class_trees().into_iter().find(|(n, _)| *n == name).expect("a class tree");
        tree_value(tree)
    });
}

#[test]
fn add() {
    replay("add", |a| {
        let Value::List(codes) = &a[0] else { panic!() };
        let mut root = BitParserState::default();
        let results = codes
            .iter()
            .map(|code| {
                let Value::List(code) = code else { panic!() };
                match root.add(leaf(&code[0]), code[1].as_str().expect("bits")) {
                    Ok(()) => Value::None,
                    Err(e) => Value::from(&e),
                }
            })
            .collect();
        Value::List(vec![Value::List(results), tree_value(&root)])
    });
}

/// `run_ops` of the generator.
fn run_ops(width: &Value, bytealign: bool, ops: &[Value]) -> Value {
    let mut p = match CCITTG4Parser::<PrintLines>::new(&build(width), bytealign) {
        Ok(p) => p,
        Err(e) => return Value::List(vec![Value::from(&e)]),
    };
    let mut out = Vec::new();
    for op in ops {
        let Value::List(op) = op else { panic!("{op:?}") };
        let kind = op[0].as_str().expect("an op name");
        let tree_name = |r: PyResult<&'static BitParserState>| r.map(|t| Value::str(node_path(t, 0)));
        let unit = |r: PyResult<()>| r.map(|()| Value::None);
        let r: PyResult<Value> = match kind {
            "state" => {
                let printed = std::mem::take(&mut p.ext.text);
                Ok(Value::List(vec![
                    int_value(p._curpos),
                    int_value(p._color),
                    int_value(p._y),
                    int_value(p._pos),
                    Value::str(p._accept.name()),
                    Value::str(node_path(p._state.tree, p._state.node)),
                    Value::str(text(&p._refline)),
                    Value::str(text(&p._curline)),
                    opt_int(p._n1),
                    opt_int(p._n2),
                    Value::str(printed),
                ]))
            }
            "setcurline" => {
                p._curline = digits(op[1].as_str().expect("bits"));
                Ok(Value::None)
            }
            "setrefline" => {
                p._refline = digits(op[1].as_str().expect("bits"));
                Ok(Value::None)
            }
            "set" => {
                let v = int(&op[2]);
                match op[1].as_str().expect("a field") {
                    "_color" => p._color = v,
                    "_curpos" => p._curpos = v,
                    "_y" => p._y = v,
                    "_n1" => p._n1 = Some(v),
                    "_n2" => p._n2 = Some(v),
                    other => panic!("unknown field {other}"),
                }
                Ok(Value::None)
            }
            "vertical" => unit(p._do_vertical(int(&op[1]))),
            "pass" => unit(p._do_pass()),
            "horizontal" => unit(p._do_horizontal(int(&op[1]), int(&op[2]))),
            "uncompressed" => unit(p._do_uncompressed(op[1].as_str().expect("bits"))),
            "flush" => unit(p._flush_line()),
            "reset_line" => unit(p._reset_line()),
            "reset" => unit(p.reset()),
            "mode" => tree_name(p._parse_mode(opt_leaf(&op[1]).as_ref())),
            "horiz1" => tree_name(p._parse_horiz1(opt_leaf(&op[1]).as_ref())),
            "horiz2" => tree_name(p._parse_horiz2(opt_leaf(&op[1]).as_ref())),
            "uncomp" => tree_name(p._parse_uncompressed(opt_leaf(&op[1]).as_ref())),
            "bit" => unit(p._parse_bit(int(&op[1]) != 0)),
            "feed" => unit(p.feedbytes(bytes(&op[1]))),
            "bitfeed" => unit(p.bitparser_feedbytes(bytes(&op[1]))),
            "bits" => p._get_bits().map(Value::str),
            "refline_at" => p._get_refline(int(&op[1])).map(Value::str),
            other => panic!("unknown op {other}"),
        };
        out.push(r.unwrap_or_else(|e| Value::from(&e)));
    }
    Value::List(out)
}

#[test]
fn ops() {
    replay("ops", |a| {
        let Value::Bool(bytealign) = a[1] else { panic!() };
        let Value::List(ops) = &a[2] else { panic!() };
        run_ops(&a[0], bytealign, ops)
    });
}

#[test]
fn decode() {
    replay("decode", |a| {
        let PDFObject::Dict(params) = build(&a[1]) else { panic!("params are a dict") };
        output(ccittfaxdecode(bytes(&a[0]), &params))
    });
}

#[test]
fn obj() {
    replay("obj", |a| output(ccittfaxdecode_obj(bytes(&a[0]), &build(&a[1]))));
}

/// `None` for an instance made, or the exception.
fn made<T>(r: PyResult<T>) -> Value {
    match r {
        Ok(_) => Value::None,
        Err(e) => Value::from(&e),
    }
}

/// From the depth the generator called `ccittfaxdecode` (or the class) at,
/// near the frame limit or with few C units left: the same
/// `RecursionError`, at the same depth.
#[test]
fn deep() {
    replay("deep", |a| {
        let Value::Dict(depth) = &a[0] else { panic!() };
        let field = |name: &str| match depth.iter().find(|(k, _)| k.as_str() == Some(name)) {
            Some((_, v)) => u32::try_from(int(v)).expect("a depth"),
            None => panic!("no {name} in {depth:?}"),
        };
        let base = recursion::Depth { py: field("py"), c: recursion::C_RECURSION_LIMIT - field("c_left") };
        let params = build(&a[3]);
        let data = bytes(&a[2]);
        recursion::with_depth(base, || match (a[1].as_str(), &params) {
            (Some("dict"), PDFObject::Dict(d)) => output(ccittfaxdecode(data, d)),
            (Some("obj"), params) => output(ccittfaxdecode_obj(data, params)),
            // the class called: None once made
            (Some("g4init"), width) => made(CCITTG4Parser::<PrintLines>::new(width, false)),
            (Some("faxinit"), width) => made(CCITTFaxDecoder::new(width, false, false)),
            other => panic!("{other:?}"),
        })
    });
}

/// Every `ccittfaxdecode` call pdfminer makes on the corpus set `all`
/// (`oracle/unit/ccitt_corpus.py`).
#[test]
fn corpus() {
    replay_in("ccitt_corpus", "corpus", |a| output(ccittfaxdecode_obj(bytes(&a[1]), &build(&a[2]))));
}
