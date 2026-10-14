//! Port of `pdfminer/settings.py`.

/// pdfminer/settings.py:1 `STRICT`: the oracle runs with the default,
/// `False` (lenient parsing: type errors return defaults, syntax errors are
/// skipped where upstream checks `settings.STRICT`).
pub const STRICT: bool = false;
