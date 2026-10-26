"""Hand mutants of the Rust code the join of wave 5 wrote (docs/architecture.md,
"Jonction"): the struct matcher's mask argument, the C units of the page layer,
the typing staircases, the merged path_segment. Each replaces one exact text (the
n-th occurrence when given), runs its tests in a copy of the sources, and is
killed when they fail; the source is restored after each. The copy must be fresh
(no build products from another copy: cargo compares mtimes).

    scripts/heavy.sh python3 scripts/join-hand-mutants.py COPY_DIR [ids...]
"""
import subprocess
import sys

COPY = sys.argv[1]
ONLY = set(sys.argv[2:])

FRAMES = ["cargo", "test", "--release", "-p", "pdftrace", "--test", "plumber_frames"]
STRUCT = ["cargo", "test", "--release", "-p", "pdfplumber-wasm", "--test", "js_runtime_unit", "--test", "boundary"]
TYPING = ["cargo", "test", "--release", "-p", "pycompat", "--lib", "typing_cache"]
INTERP = ["cargo", "test", "--release", "-p", "pdftrace", "--test", "interp_unit"]
PLUMBER = ["cargo", "test", "--release", "-p", "pdftrace", "--test", "plumber_unit"]

PI = "crates/pdfplumber/src/utils/pdfinternals.rs"
PG = "crates/pdfplumber/src/page.rs"
TC = "crates/pycompat/src/typing_cache.rs"
ST = "crates/wasm/src/structs.rs"

# (id, file, from, to, occurrence (1-based, None: must be unique), tests)
MUTANTS = [
    ("s1", ST, "(None, PyValue::Str(tag)) => Matcher::Tag(tag),", "", None, [STRUCT]),
    ("s2", ST, "arg(a, 1)?, a.get(2));", "arg(a, 1)?, a.get(3));", None, [STRUCT]),
    ("s3", ST, "let truth = seq(mask)?;", "let truth = seq(mask)?; let truth = &truth[..truth.len().min(1)];", None, [STRUCT]),
    ("c1", PI, "fn resolve_all_unref(x: &PDFObject) -> PyResult<PDFObject> {\n    isinstance_tuple()?;", "fn resolve_all_unref(x: &PDFObject) -> PyResult<PDFObject> {", None, [FRAMES]),
    ("c2", PG, "    let _frame = py_frame()?;\n    isinstance_tuple()?;\n    match x {", "    let _frame = py_frame()?;\n    match x {", None, [FRAMES]),
    ("c3", PG, "            let _frame = py_frame()?;\n            isinstance_tuple()?;\n            let _entry = eval_entry()?;", "            let _frame = py_frame()?;\n            let _entry = eval_entry()?;", None, [FRAMES]),
    ("c4", PG, "        let _frame = py_frame()?;\n        isinstance_tuple()?;\n        return Ok(x);", "        let _frame = py_frame()?;\n        return Ok(x);", None, [FRAMES]),
    ("c5", PG, "        // checks\n        isinstance_tuple()?;\n        let text = match obj {", "        // checks\n        let text = match obj {", None, [FRAMES]),
    ("c6", PG, "            _ => {\n                isinstance_tuple()?;\n                if let LayoutObj::Item(LTItem::Curve(c)) = obj {", "            _ => {\n                if let LayoutObj::Item(LTItem::Curve(c)) = obj {", None, [FRAMES]),
    ("c7", PI, "    if !matches!(obj, PDFObject::List(_) | PDFObject::Literal(_)) {\n        isinstance_tuple()?;\n    }", "", None, [FRAMES]),
    ("c8", PI, "    let _join = c_call(WHILE_CALLING)?;\n    let _entry = eval_entry()?;\n    pycompat::recursion::leaf(2)?;\n    match s {", "    let _entry = eval_entry()?;\n    pycompat::recursion::leaf(2)?;\n    match s {", None, [FRAMES]),
    ("c9", PI, "                drop(c_call(WHILE_CALLING)?); // ord(c)", "", None, [FRAMES]),
    ("c10", PI, "            let _call = c_call(WHILE_CALLING)?;\n            pycompat::recursion::c_called_leaf()?;\n            let units", "            pycompat::recursion::c_called_leaf()?;\n            let units", 1, [FRAMES]),
    ("c11", PI, "            let _call = c_call(WHILE_CALLING)?;\n            pycompat::recursion::c_called_leaf()?;\n            let units", "            pycompat::recursion::c_called_leaf()?;\n            let units", 2, [FRAMES]),
    ("c12", PI, "        let _join = c_call(WHILE_CALLING)?;\n        let _entry = eval_entry()?;\n        pycompat::recursion::leaf(2)?;\n        // \"\".join materializes", "        let _entry = eval_entry()?;\n        pycompat::recursion::leaf(2)?;\n        // \"\".join materializes", None, [FRAMES]),
    ("c13", PI, "                PyValue::Str(ch) => {\n                    drop(c_call(WHILE_CALLING)?);", "                PyValue::Str(ch) => {", None, [FRAMES]),
    ("c14", PI, "        let _join = c_call(WHILE_CALLING)?;\n        let _entry = eval_entry()?;\n        pycompat::recursion::leaf(2)?;\n        if !text.is_empty() {", "        let _entry = eval_entry()?;\n        pycompat::recursion::leaf(2)?;\n        if !text.is_empty() {", None, [FRAMES]),
    ("c15", PI, "        if !text.is_empty() {\n            drop(c_call(WHILE_CALLING)?);\n        }", "", None, [FRAMES]),
    ("c16", PI, "            if !s.is_empty() {\n                drop(c_call(WHILE_CALLING)?); // ord(c)\n            }", "            drop(c_call(WHILE_CALLING)?);", None, [FRAMES]),
    ("t1", TC, "            check_c(self.c_hit)?;\n            return leaf(self.hit);", "            return leaf(self.hit);", None, [FRAMES, TYPING]),
    ("t2", TC, "        check_c(self.c_miss)?;\n        leaf(self.miss)?;", "        leaf(self.miss)?;", None, [FRAMES, TYPING]),
    ("t3", TC, "    match steps.get(spare) {", "    match steps.get(spare + 1) {", None, [FRAMES, TYPING]),
    ("t4", TC, "let constant = self.miss == self.hit && self.c_miss == self.c_hit;", "let constant = self.miss == self.hit;", None, [FRAMES, TYPING]),
    # cargo mutants' survivors with plumber_frames alone, against the other replays
    ("m1", PG, "                attr.push_new(k.stroking_color.clone(), color_tuple(&gs.scolor));\n", "", None, [FRAMES, PLUMBER]),
    ("m2", PI, "    if !matches!(obj, PDFObject::List(_) | PDFObject::Literal(_)) {", "    if matches!(obj, PDFObject::List(_) | PDFObject::Literal(_)) {", None, [FRAMES]),
    ("m3", TC, "let constant = self.miss == self.hit && self.c_miss == self.c_hit;", "let constant = self.miss == self.hit || self.c_miss == self.c_hit;", None, [FRAMES, TYPING]),
    ("m4", TC, "let constant = self.miss == self.hit && self.c_miss == self.c_hit;", "let constant = self.miss != self.hit && self.c_miss == self.c_hit;", None, [FRAMES, TYPING]),
    ("m5", TC, "let constant = self.miss == self.hit && self.c_miss == self.c_hit;", "let constant = self.miss == self.hit && self.c_miss != self.c_hit;", None, [FRAMES, TYPING]),
    ("r1", PG, "            let _check = c_call(IN_INSTANCECHECK)?;\n            drop(c_call(WHILE_CALLING)?);\n", "            drop(c_call(WHILE_CALLING)?);\n", None, [FRAMES]),
    ("r2", PG, "            let _check = c_call(IN_INSTANCECHECK)?;\n            drop(c_call(WHILE_CALLING)?);\n", "            let _check = c_call(IN_INSTANCECHECK)?;\n", None, [FRAMES]),
    # (.lower()'s unit; _cache2's look-up takes none: wave5/fix removed it)
    ("r3", PG, "        }\n        drop(c_call(WHILE_CALLING)?);\n        // def process_attr", "        }\n        // def process_attr", None, [FRAMES]),
    ("r4", PG, "        }\n        drop(c_call(WHILE_CALLING)?);\n        // def process_attr", "        }\n        drop(c_call(IN_COMPARISON)?);\n        // def process_attr", None, [FRAMES]),
    ("r5", PG, "            let _check = c_call(IN_INSTANCECHECK)?;\n            drop(c_call(WHILE_CALLING)?);\n", "            drop(c_call(IN_INSTANCECHECK)?);\n            drop(c_call(WHILE_CALLING)?);\n", None, [FRAMES]),
    ("i1", "crates/pdfminer/src/pdfinterp.rs", "                None => all = false,", "                None => {}", None, [INTERP]),
    ("i2", "crates/pdfminer/src/pdfinterp.rs", "        match all.then_some(coords) {", "        match Some(coords) {", None, [INTERP]),
    ("i3", "crates/pdfminer/src/pdfinterp.rs", "            Some(coords) => self.curpath.push((op, coords)),", "            Some(_) => {}", None, [INTERP]),
    ("i4", "crates/pdfminer/src/pdfinterp.rs", "    fn path_segment(&mut self, op: char, operands: &[&PDFStackT]) -> PyResult<()> {\n", "    fn path_segment(&mut self, op: char, operands: &[&PDFStackT]) -> PyResult<()> {\n        if true { return Ok(()); }\n", None, [INTERP]),
    ("t5", TC, "        Subscription { c_miss, c_hit, ..self }", "        Subscription { c_miss: c_hit, c_hit, ..self }", None, [FRAMES, TYPING]),
]


def run(cmd):
    r = subprocess.run(cmd, cwd=COPY, capture_output=True, text=True)
    return r.returncode == 0, r.stdout[-400:] + r.stderr[-800:]


survivors = 0
for mid, f, a, b, occ, tests in MUTANTS:
    if ONLY and mid not in ONLY:
        continue
    path = f"{COPY}/{f}"
    src = open(path).read()
    n = src.count(a)
    if occ is None and n != 1 or occ is not None and n < occ:
        print(f"{mid} SKIPPED: {n} occurrences", flush=True)
        continue
    k = -1
    for _ in range(occ or 1):
        k = src.index(a, k + 1)
    open(path, "w").write(src[:k] + b + src[k + len(a):])
    try:
        ok = True
        for t in tests:
            passed, out = run(t)
            if not passed:
                ok = False
                compiled = "error[" not in out
                break
    finally:
        open(path, "w").write(src)
    if ok:
        survivors += 1
    print(f"{mid} {'SURVIVED' if ok else 'killed' if compiled else 'UNVIABLE'}", flush=True)
print(f"{survivors} survivors", flush=True)
