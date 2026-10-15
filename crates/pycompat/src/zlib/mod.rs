//! CPython 3.12's `zlib` module (decompression), on a port of zlib 1.3's
//! inflate.
//!
//! pdfminer decodes `/FlateDecode` with `zlib.decompress(data)` and, when
//! that raises `zlib.error`, with `decompress_corrupted` (pdftypes.py),
//! which feeds a `zlib.decompressobj()` one byte at a time until the first
//! error. What pdfminer returns therefore depends on exactly how much
//! output zlib produces before an error, and on the error message:
//! `zlib.error("Error -3 while decompressing data: invalid distance too far
//! back")`, `"Error -5 while decompressing data: incomplete or truncated
//! stream"`... A general-purpose inflater does not reproduce these, so this
//! module ports the inflate code of zlib 1.3 (the library the oracle's
//! CPython links, `zlib.ZLIB_RUNTIME_VERSION`): [`inflate`] (`inflate.c`,
//! the state machine), [`inffast`] (`inffast.c`, its fast path),
//! [`inftrees`] (`inftrees.c`, the Huffman tables) and [`adler32`]; and,
//! here, CPython 3.12.3's `Modules/zlibmodule.c` around it:
//! `zlib.decompress` ([`decompress`]) and `zlib.Decompress.decompress`
//! ([`Decompress`]), with their output buffers
//! (`Include/internal/pycore_blocks_output_buffer.h`), whose block sizes
//! decide how often `inflate()` returns, and so when the sliding window is
//! consulted.
//!
//! Not ported: compression, `flush()`, `copy()`, a `zdict`, the gzip
//! container (`wbits` 16 to 47: [`PyErr::not_ported`]); pdfminer uses none
//! of them. `MemoryError` (allocation failures) is not reproduced.

pub mod adler32;
mod inffast;
pub mod inflate;
pub mod inftrees;

use crate::err::{exc, PyErr, PyResult};
use inflate::{inflate, inflate_init2, Flush, ZStream, Z_BUF_ERROR, Z_DATA_ERROR, Z_NEED_DICT, Z_OK, Z_STREAM_END, Z_STREAM_ERROR};

/// `zlib.MAX_WBITS`
pub const MAX_WBITS: i32 = 15;
/// `zlib.DEF_BUF_SIZE`: the initial output buffer of `zlib.decompress`.
pub const DEF_BUF_SIZE: usize = 16 * 1024;

const KB: usize = 1024;
const MB: usize = 1024 * 1024;
/// The block size sequence of `_BlocksOutputBuffer`.
const BUFFER_BLOCK_SIZE: [usize; 17] =
    [32 * KB, 64 * KB, 256 * KB, MB, 4 * MB, 8 * MB, 16 * MB, 16 * MB, 32 * MB, 32 * MB, 32 * MB, 32 * MB, 64 * MB, 64 * MB, 128 * MB, 128 * MB, 256 * MB];

// cpython-3.12.3/Modules/zlibmodule.c:200 zlib_error
/// The `zlib.error` raised for the return code `err` of a zlib call.
fn zlib_error(zst: &ZStream, err: i32, msg: &str) -> PyErr {
    let zmsg = zst.msg.or(match err {
        Z_BUF_ERROR => Some("incomplete or truncated stream"),
        Z_STREAM_ERROR => Some("inconsistent stream state"),
        Z_DATA_ERROR => Some("invalid input data"),
        _ => None,
    });
    match zmsg {
        None => exc::ZlibError(format!("Error {err} {msg}")),
        // "%.200s": every zlib message is shorter
        Some(zmsg) => exc::ZlibError(format!("Error {err} {msg}: {zmsg}")),
    }
}

/// The gzip container is not ported (see the module documentation).
fn gzip_not_ported() -> PyErr {
    PyErr::not_ported("zlib with wbits 16 to 47 (gzip container)")
}

/// `_BlocksOutputBuffer`: the output of one call, in blocks. Only its
/// sizes are modeled (the bytes go to one `Vec`): `allocated` bytes in
/// `blocks` blocks; `zst.avail_out` is the space left in the last one.
struct BlocksOutputBuffer {
    blocks: usize,
    allocated: usize,
    /// negative: unlimited
    max_length: isize,
}

impl BlocksOutputBuffer {
    // cpython-3.12.3/Include/internal/pycore_blocks_output_buffer.h:104 _BlocksOutputBuffer_InitAndGrow
    /// The first block: `BUFFER_BLOCK_SIZE[0]`, or less under `max_length`.
    fn init_and_grow(max_length: isize) -> (BlocksOutputBuffer, usize) {
        let block_size = if 0 <= max_length && (max_length as usize) < BUFFER_BLOCK_SIZE[0] {
            max_length as usize
        } else {
            BUFFER_BLOCK_SIZE[0]
        };
        (BlocksOutputBuffer { blocks: 1, allocated: block_size, max_length }, block_size)
    }

    // cpython-3.12.3/Include/internal/pycore_blocks_output_buffer.h:151 _BlocksOutputBuffer_InitWithSize
    fn init_with_size(init_size: usize) -> (BlocksOutputBuffer, usize) {
        (BlocksOutputBuffer { blocks: 1, allocated: init_size, max_length: -1 }, init_size)
    }

    // cpython-3.12.3/Include/internal/pycore_blocks_output_buffer.h:186 _BlocksOutputBuffer_Grow
    /// The next block (the previous one is full).
    fn grow(&mut self) -> usize {
        let mut block_size = BUFFER_BLOCK_SIZE[self.blocks.min(BUFFER_BLOCK_SIZE.len() - 1)];
        if self.max_length >= 0 {
            // block_size of the last block
            let rest = self.max_length as usize - self.allocated;
            block_size = block_size.min(rest);
        }
        self.blocks += 1;
        self.allocated += block_size;
        block_size
    }
}

/// The largest `avail_in`/`avail_out` (`UINT_MAX`).
const UINT_MAX: usize = u32::MAX as usize;

// cpython-3.12.3/Modules/zlibmodule.c:272 arrange_input_buffer
fn arrange_input_buffer(zst: &mut ZStream, remains: &mut usize) {
    zst.avail_in = (*remains).min(UINT_MAX) as u32;
    *remains -= zst.avail_in as usize;
}

/// `zlib.decompress(data)`: the whole stream, or `zlib.error`.
pub fn decompress(data: &[u8]) -> PyResult<Vec<u8>> {
    decompress_with(data, MAX_WBITS, DEF_BUF_SIZE as isize)
}

// cpython-3.12.3/Modules/zlibmodule.c:422 zlib_decompress_impl
/// `zlib.decompress(data, wbits, bufsize)`. Trailing data after the end of
/// the stream is ignored.
pub fn decompress_with(data: &[u8], wbits: i32, bufsize: isize) -> PyResult<Vec<u8>> {
    if bufsize < 0 {
        return Err(exc::ValueError("bufsize must be non-negative"));
    }
    let bufsize = (bufsize as usize).max(1);
    // (the UINT32_MAX sliding window over a larger first block is not
    // modeled: such a buffer does not fit in a Wasm memory)
    assert!(bufsize <= UINT_MAX, "bufsize above UINT32_MAX");
    let (mut buffer, first) = BlocksOutputBuffer::init_with_size(bufsize);
    let mut zst = match inflate_init2(wbits) {
        Ok(Some(zst)) => zst,
        Ok(None) => return Err(gzip_not_ported()),
        // inflateInit2 sets msg to NULL before failing
        Err(err) => return Err(exc::ZlibError(format!("Error {err} while preparing to decompress data: inconsistent stream state"))),
    };
    zst.avail_out = first as u32;
    let mut output = Vec::new();
    let mut ibuflen = data.len();
    let mut err;
    loop {
        arrange_input_buffer(&mut zst, &mut ibuflen);
        let flush = if ibuflen == 0 { Flush::Finish } else { Flush::NoFlush };
        loop {
            if zst.avail_out == 0 {
                zst.avail_out = buffer.grow() as u32;
            }
            err = inflate(&mut zst, data, &mut output, flush);
            match err {
                Z_OK | Z_BUF_ERROR | Z_STREAM_END => {}
                _ => return Err(zlib_error(&zst, err, "while decompressing data")),
            }
            if zst.avail_out != 0 {
                break;
            }
        }
        if err == Z_STREAM_END || ibuflen == 0 {
            break;
        }
    }
    if err != Z_STREAM_END {
        return Err(zlib_error(&zst, err, "while decompressing data"));
    }
    Ok(output)
}

// cpython-3.12.3/Modules/zlibmodule.c:241 newcompobject
/// A `zlib.decompressobj()`: decompresses a stream given in pieces.
pub struct Decompress {
    zst: ZStream,
    /// `self.unused_data`: the bytes after the end of the stream.
    pub unused_data: Vec<u8>,
    /// `self.unconsumed_tail`: input left over when `max_length` stopped
    /// the output.
    pub unconsumed_tail: Vec<u8>,
    /// `self.eof`: true once the end of the stream was reached.
    pub eof: bool,
}

impl Decompress {
    // cpython-3.12.3/Modules/zlibmodule.c:654 zlib_decompressobj_impl
    /// `zlib.decompressobj(wbits)` (without `zdict`).
    pub fn new(wbits: i32) -> PyResult<Decompress> {
        match inflate_init2(wbits) {
            Ok(Some(zst)) => Ok(Decompress { zst, unused_data: Vec::new(), unconsumed_tail: Vec::new(), eof: false }),
            Ok(None) => Err(gzip_not_ported()),
            Err(_) => Err(exc::ValueError("Invalid initialization option")),
        }
    }

    // cpython-3.12.3/Modules/zlibmodule.c:805 save_unconsumed_input
    /// Saves any unconsumed input data in `unused_data` or
    /// `unconsumed_tail`, as appropriate.
    fn save_unconsumed_input(&mut self, data: &[u8], err: i32) {
        if err == Z_STREAM_END {
            // The end of the compressed data has been reached. Store the
            // leftover input data in self->unused_data.
            if self.zst.avail_in > 0 {
                self.unused_data.extend_from_slice(&data[self.zst.next_in..]);
                self.zst.avail_in = 0;
            }
        }
        if self.zst.avail_in > 0 || !self.unconsumed_tail.is_empty() {
            // 1. Output limit was reached. Save leftover input in
            // unconsumed_tail. 2. All input data was consumed. Clear
            // unconsumed_tail.
            self.unconsumed_tail = data[self.zst.next_in..].to_vec();
        }
    }

    /// `self.decompress(data)`
    pub fn decompress(&mut self, data: &[u8]) -> PyResult<Vec<u8>> {
        self.decompress_max(data, 0)
    }

    // cpython-3.12.3/Modules/zlibmodule.c:867 zlib_Decompress_decompress_impl
    /// `self.decompress(data, max_length)`: the output of this part of the
    /// stream (input it does not use goes to `unconsumed_tail`, or after
    /// the end of the stream to `unused_data`).
    pub fn decompress_max(&mut self, data: &[u8], max_length: isize) -> PyResult<Vec<u8>> {
        if max_length < 0 {
            return Err(exc::ValueError("max_length must be non-negative"));
        }
        let max_length = if max_length == 0 { -1 } else { max_length };
        self.zst.next_in = 0;
        let mut ibuflen = data.len();
        let (mut buffer, first) = BlocksOutputBuffer::init_and_grow(max_length);
        self.zst.avail_out = first as u32;
        let mut output = Vec::new();
        let mut err = Z_OK;
        'save: loop {
            arrange_input_buffer(&mut self.zst, &mut ibuflen);
            loop {
                if self.zst.avail_out == 0 {
                    if max_length >= 0 && output.len() == max_length as usize {
                        break 'save;
                    }
                    self.zst.avail_out = buffer.grow() as u32;
                }
                err = inflate(&mut self.zst, data, &mut output, Flush::SyncFlush);
                match err {
                    Z_OK | Z_BUF_ERROR | Z_STREAM_END => {}
                    // (a Z_NEED_DICT with a zdict would set it here)
                    _ => break 'save,
                }
                if self.zst.avail_out != 0 && err != Z_NEED_DICT {
                    break;
                }
            }
            if err == Z_STREAM_END || ibuflen == 0 {
                break;
            }
        }
        // save:
        self.save_unconsumed_input(data, err);
        if err == Z_STREAM_END {
            self.eof = true;
        } else if err != Z_OK && err != Z_BUF_ERROR {
            return Err(zlib_error(&self.zst, err, "while decompressing data"));
        }
        Ok(output)
    }
}
