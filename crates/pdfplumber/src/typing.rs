//! Port of `pdfplumber/_typing.py`: the type aliases of pdfplumber, as Rust
//! types of the object model ([`crate::obj`]).
#![allow(non_camel_case_types)]

// pdfplumber/_typing.py:3 T_seq (any sequence: a Rust slice)

/// pdfplumber/_typing.py:4 T_num (`int` or `float`; values of objects are
/// dynamic, see [`crate::obj::PyValue`])
pub type T_num = crate::obj::PyValue;

/// pdfplumber/_typing.py:5 T_point
pub type T_point = (crate::obj::PyValue, crate::obj::PyValue);

/// pdfplumber/_typing.py:6 T_bbox
pub type T_bbox = crate::obj::Bbox;

/// pdfplumber/_typing.py:7 T_obj
pub type T_obj = crate::obj::Obj;

/// pdfplumber/_typing.py:8 T_obj_list
pub type T_obj_list = Vec<crate::obj::Obj>;

/// pdfplumber/_typing.py:10 T_dir: `"ltr"`, `"rtl"`, `"ttb"` or `"btt"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum T_dir {
    Ttb,
    Btt,
    Ltr,
    Rtl,
}

impl T_dir {
    /// The direction named `s`, if it is one.
    pub fn parse(s: &str) -> Option<T_dir> {
        Some(match s {
            "ttb" => T_dir::Ttb,
            "btt" => T_dir::Btt,
            "ltr" => T_dir::Ltr,
            "rtl" => T_dir::Rtl,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            T_dir::Ttb => "ttb",
            T_dir::Btt => "btt",
            T_dir::Ltr => "ltr",
            T_dir::Rtl => "rtl",
        }
    }
}
