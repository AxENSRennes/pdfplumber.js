//! Port of `pdfminer/data_structures.py`: `NumberTree`.
//!
//! The tree is walked recursively, as upstream, so a cyclic or very deep
//! `/Kids` chain raises `RecursionError` where Python does: each Python
//! function on the way takes its frame (`pycompat::recursion`), including
//! `dict_value`/`list_value`/`int_value` and the `resolve1` they call (the
//! accessors count theirs), the resumptions of the `choplist` generator and
//! the sort key called by `list.sort`.

use pycompat::recursion::{eval_entry, py_frame};
use pycompat::PyResult;

use crate::pdftypes::{dict_value, int_value_obj, list_value, PDFDict, PDFList, PDFObject};
use crate::settings;
use crate::utils::choplist;

/// pdfminer/data_structures.py:11 `NumberTree`: a PDF number tree (section
/// 3.8.6 of the PDF Reference).
#[derive(Clone)]
pub struct NumberTree {
    pub _obj: PDFDict,
    pub nums: Option<PDFList>,
    pub kids: Option<PDFList>,
    pub limits: Option<PDFList>,
}

impl NumberTree {
    // pdfminer/data_structures.py:17 NumberTree.__init__
    pub fn new(obj: &PDFObject) -> PyResult<NumberTree> {
        let _frame = py_frame()?;
        let _obj = dict_value(obj)?;
        let mut tree = NumberTree { _obj, nums: None, kids: None, limits: None };
        if let Some(nums) = tree._obj.get("Nums") {
            tree.nums = Some(list_value(&nums)?);
        }
        if let Some(kids) = tree._obj.get("Kids") {
            tree.kids = Some(list_value(&kids)?);
        }
        if let Some(limits) = tree._obj.get("Limits") {
            tree.limits = Some(list_value(&limits)?);
        }
        Ok(tree)
    }

    // pdfminer/data_structures.py:30 NumberTree._parse
    /// The `(int_value(key), value)` pairs of the leaves, depth first; the
    /// key is an int, or a `bool` as `int_value` lets one through.
    pub fn parse(&self) -> PyResult<Vec<(PDFObject, PDFObject)>> {
        let _frame = py_frame()?;
        let mut items = Vec::new();
        if let Some(nums) = self.nums.as_ref().filter(|l| !l.is_empty()) {
            // Leaf node: `for k, v in choplist(2, self.nums)`. The generator
            // is resumed once per pair and once more to finish, each time in
            // a frame of its own.
            let nums = nums.to_vec();
            let mut pairs = choplist(2, &nums);
            loop {
                drop(py_frame()?);
                let Some(pair) = pairs.next() else { break };
                items.push((int_value_obj(&pair[0])?, pair[1].clone()));
            }
        }
        if let Some(kids) = self.kids.as_ref().filter(|l| !l.is_empty()) {
            // Root or intermediate node
            for child_ref in kids.to_vec() {
                items.extend(NumberTree::new(&child_ref)?.parse()?);
            }
        }
        Ok(items)
    }

    // pdfminer/data_structures.py:45 NumberTree.values
    /// The pairs sorted by key (stable; `settings.STRICT` is false, so no
    /// order check). `list.sort` calls the key function `t[0]` on every
    /// pair, from C.
    pub fn values(&self) -> PyResult<Vec<(PDFObject, PDFObject)>> {
        let _frame = py_frame()?;
        let mut values = self.parse()?;
        const { assert!(!settings::STRICT) };
        let mut keys = Vec::with_capacity(values.len());
        for (k, _) in &values {
            let _entry = eval_entry()?;
            drop(py_frame()?);
            keys.push(k.as_num().expect("int_value returns an int or a bool"));
        }
        // ints: a total order, so any stable sort is list.sort
        let mut order: Vec<usize> = (0..values.len()).collect();
        order.sort_by(|&i, &j| keys[i].py_cmp(&keys[j]).expect("ints compare"));
        let mut slots: Vec<Option<(PDFObject, PDFObject)>> = values.drain(..).map(Some).collect();
        Ok(order.into_iter().map(|i| slots[i].take().expect("each index once")).collect())
    }
}
