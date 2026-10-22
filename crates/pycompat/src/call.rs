//! How CPython 3.12 binds the keyword arguments of a call to the parameters
//! of a Python function (`Python/ceval.c`, `initialize_locals`), for the
//! ported functions that take `**kwargs` or keyword parameters from a
//! caller's dict (`obj.method(*args, **kwargs)`, `cls(**kwargs)`).
//!
//! Each keyword is bound in the order given: a name of a parameter that a
//! positional argument (`self`, `cls`, ...) already bound raises
//! `TypeError("<qualname>() got multiple values for argument '<name>'")`;
//! a name that is no parameter goes to `**kwargs` when the function has
//! one, and raises `TypeError("<qualname>() got an unexpected keyword
//! argument '<name>'")` otherwise. The name is written with `%S` (`str`,
//! between quotes), not `repr`. The error happens when the call binds its
//! arguments, before the function's body runs.

use crate::err::{ExcKind, PyErr, PyResult};
use crate::PyStr;

/// The parameters of a Python function, as its call binds keywords.
#[derive(Clone, Copy, Debug)]
pub struct Signature<'a> {
    /// `func.__qualname__` (`"Page.extract_text"`).
    pub qualname: &'a str,
    /// The parameters bound by the positional arguments of the call
    /// (`self` for a method, and those the caller passes positionally).
    pub bound: &'a [&'a str],
    /// The other parameters a keyword may name; `None`: the function
    /// takes `**kwargs` (any other name goes there).
    pub keywords: Option<&'a [&'a str]>,
}

impl Signature<'_> {
    /// Binds the keyword names, in order (see the module documentation).
    pub fn bind<'k>(&self, names: impl IntoIterator<Item = &'k PyStr>) -> PyResult<()> {
        for name in names {
            let is = |p: &&str| *name == **p;
            if self.bound.iter().any(is) {
                return Err(self.error("got multiple values for argument '", name));
            }
            if let Some(keywords) = self.keywords {
                if !keywords.iter().any(is) {
                    return Err(self.error("got an unexpected keyword argument '", name));
                }
            }
        }
        Ok(())
    }

    fn error(&self, what: &str, name: &PyStr) -> PyErr {
        let message = PyStr::new(&format!("{}() {what}", self.qualname)).concat(name).concat(&PyStr::new("'"));
        PyErr::with_message(ExcKind::TypeError, &message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> Vec<PyStr> {
        n.iter().map(|s| PyStr::new(s)).collect()
    }

    #[test]
    fn keywords_bind_in_order() {
        let method =
            Signature { qualname: "Page.crop", bound: &["self", "bbox"], keywords: Some(&["relative", "strict"]) };
        assert!(method.bind(&names(&["strict", "relative"])).is_ok());
        let e = method.bind(&names(&["relative", "bbox", "nope"])).unwrap_err();
        assert_eq!(e.message, "Page.crop() got multiple values for argument 'bbox'");
        let e = method.bind(&names(&["no'pe", "self"])).unwrap_err();
        assert_eq!(e.message, "Page.crop() got an unexpected keyword argument 'no'pe'");
        let kwargs = Signature { qualname: "Page.extract_text", bound: &["self"], keywords: None };
        assert!(kwargs.bind(&names(&["layout", "x"])).is_ok());
        let e = kwargs.bind(&names(&["layout", "self"])).unwrap_err();
        assert_eq!(e.message, "Page.extract_text() got multiple values for argument 'self'");
    }
}
