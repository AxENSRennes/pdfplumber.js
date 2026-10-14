//! WebAssembly entry points (stub).
//!
//! The coarse-grained JS boundary of `docs/architecture.md` ("Frontière
//! JavaScript"): a minimal ABI (`alloc`, `free`, calls that fill a result
//! buffer), no `wasm-bindgen`. Nothing is exported yet; this crate only
//! fixes the build target (`cdylib` for `wasm32-unknown-unknown`).
