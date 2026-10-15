//! The exported functions of the WebAssembly module (`docs/js-runtime.md`,
//! "ABI"), without `wasm-bindgen`:
//!
//! - `pp_alloc(len) -> ptr`: a buffer of `len` bytes that JavaScript fills
//!   (the request, the PDF bytes, a CMap file). Rust owns it, keyed by its
//!   address, until a call takes it or `pp_free(ptr)` drops it.
//! - `pp_call(request_ptr, data_ptr) -> status`: takes the request buffer
//!   and the optional data buffer (`0`: none) and runs the request
//!   (`ops.rs`); the response stays available through `pp_result_ptr()` and
//!   `pp_result_len()` until the next call. The status is the response's.
//!
//! No `unsafe` code: JavaScript writes and reads the memory, Rust only
//! hands out addresses of buffers it owns. The names are prefixed so that
//! a native build of this crate does not export a `free` that would replace
//! the C library's.
#![allow(unsafe_code)] // `#[no_mangle]` only: the `unsafe_code` lint covers it

use std::cell::RefCell;

use rustc_hash::FxHashMap;

use crate::ops::Core;

thread_local! {
    static CORE: RefCell<Option<Core>> = const { RefCell::new(None) };
    static BUFFERS: RefCell<FxHashMap<usize, Vec<u8>>> = RefCell::new(FxHashMap::default());
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Allocates a zeroed buffer of `len` bytes and returns its address
/// (never 0).
#[no_mangle]
pub extern "C" fn pp_alloc(len: usize) -> usize {
    // capacity at least 1: a distinct, non-dangling address even for 0 bytes
    let mut buf = Vec::with_capacity(len.max(1));
    buf.resize(len, 0);
    let ptr = buf.as_ptr() as usize;
    BUFFERS.with(|b| b.borrow_mut().insert(ptr, buf));
    ptr
}

/// Drops a buffer of `pp_alloc` that no call took (unknown addresses are
/// ignored).
#[no_mangle]
pub extern "C" fn pp_free(ptr: usize) {
    BUFFERS.with(|b| b.borrow_mut().remove(&ptr));
}

/// Takes a buffer of `pp_alloc`.
pub fn take_buffer(ptr: usize) -> Option<Vec<u8>> {
    BUFFERS.with(|b| b.borrow_mut().remove(&ptr))
}

/// Runs a request (see the module documentation).
#[no_mangle]
pub extern "C" fn pp_call(request_ptr: usize, data_ptr: usize) -> u32 {
    let request = take_buffer(request_ptr).unwrap_or_default();
    let data = if data_ptr == 0 { None } else { take_buffer(data_ptr) };
    let response = CORE.with(|c| c.borrow_mut().get_or_insert_with(Core::new).call(&request, data));
    let status = u32::from(response[4]);
    RESULT.with(|r| *r.borrow_mut() = response);
    status
}

/// The address of the last response.
#[no_mangle]
pub extern "C" fn pp_result_ptr() -> usize {
    RESULT.with(|r| r.borrow().as_ptr() as usize)
}

/// The length of the last response.
#[no_mangle]
pub extern "C" fn pp_result_len() -> usize {
    RESULT.with(|r| r.borrow().len())
}

/// Drops the last response (JavaScript has decoded it).
#[no_mangle]
pub extern "C" fn pp_result_free() {
    RESULT.with(|r| *r.borrow_mut() = Vec::new());
}

/// The number of live buffers of `pp_alloc` (leak checks in the tests).
#[no_mangle]
pub extern "C" fn pp_buffers() -> usize {
    BUFFERS.with(|b| b.borrow().len())
}

/// A buffer of `pp_alloc` filled with `bytes` (what JavaScript does through
/// the memory), for native callers and tests.
pub fn put(bytes: &[u8]) -> usize {
    let ptr = pp_alloc(bytes.len());
    BUFFERS.with(|b| {
        if let Some(buf) = b.borrow_mut().get_mut(&ptr) {
            buf.copy_from_slice(bytes);
        }
    });
    ptr
}

/// A copy of the last response (what JavaScript reads through the memory).
pub fn last_response() -> Vec<u8> {
    RESULT.with(|r| r.borrow().clone())
}
