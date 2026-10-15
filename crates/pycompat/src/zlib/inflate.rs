//! zlib 1.3 `inflate.c` / `inflate.h`: the inflate state machine.
//!
//! A faithful port of the slow path of `inflate()` (the fast path is
//! [`super::inffast`]), with the same states, bit accumulator, sliding
//! window and error messages, so that the output written before an error,
//! the error itself and the input left unused are those of the C library.
//!
//! Differences with C, none observable through CPython's `zlib` module:
//!
//! - pointers become offsets: the input is the slice given to [`inflate`]
//!   (`next_in` an index into it); the output is appended to a `Vec`
//!   (`next_out` is its end) and `avail_out` counts the space left in the
//!   buffer CPython gave to this call;
//! - not ported: the gzip container (`wbits` 16 to 47; CPython's callers
//!   in pdfminer pass 15, see [`super`]), `inflateSetDictionary` (CPython
//!   calls it only for a `zdict`), `Z_BLOCK`/`Z_TREES` flushes (CPython
//!   never passes them), `inflateMark` bookkeeping (`back`, `was`),
//!   `inflateUndermine` (`sane` is always 1), `INFLATE_STRICT` (not
//!   defined in the library CPython links), the totals and `data_type`
//!   (unused by CPython), memory errors.

use std::sync::LazyLock;

use super::adler32::adler32;
use super::inftrees::{inflate_table, Code, CodeType, ENOUGH};

// zlib.h return codes
pub const Z_OK: i32 = 0;
pub const Z_STREAM_END: i32 = 1;
pub const Z_NEED_DICT: i32 = 2;
pub const Z_STREAM_ERROR: i32 = -2;
pub const Z_DATA_ERROR: i32 = -3;
pub const Z_BUF_ERROR: i32 = -5;

/// `Z_DEFLATED`: the only compression method of the zlib header.
const Z_DEFLATED: u32 = 8;

/// The `flush` argument of [`inflate`], among those CPython passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flush {
    NoFlush,
    SyncFlush,
    Finish,
}

// zlib-1.3/inflate.h:20 inflate_mode
/// Possible inflate modes between inflate() calls (the gzip modes are not
/// ported; the order of the others is kept, the code compares modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    /// i: waiting for magic header
    HEAD,
    /// i: waiting for dictionary check value
    DICTID,
    /// waiting for inflateSetDictionary() call
    DICT,
    /// i: waiting for type bits, including last-flag bit
    TYPE,
    /// i: same, but skip check to exit inflate on new block
    TYPEDO,
    /// i: waiting for stored size (length and complement)
    STORED,
    /// i/o: same as COPY below, but only first time in
    COPY_,
    /// i/o: waiting for input or output to copy stored block
    COPY,
    /// i: waiting for dynamic block table lengths
    TABLE,
    /// i: waiting for code length code lengths
    LENLENS,
    /// i: waiting for length/lit and distance code lengths
    CODELENS,
    /// i: same as LEN below, but only first time in
    LEN_,
    /// i: waiting for length/lit/eob code
    LEN,
    /// i: waiting for length extra bits
    LENEXT,
    /// i: waiting for distance code
    DIST,
    /// i: waiting for distance extra bits
    DISTEXT,
    /// o: waiting for output space to copy string
    MATCH,
    /// o: waiting for output space to write literal
    LIT,
    /// i: waiting for 32-bit check value
    CHECK,
    /// finished check, done -- remain here until reset
    DONE,
    /// got a data error -- remain here until reset
    BAD,
}

/// Where a decoding table is: in `state.codes` at an offset, or one of the
/// fixed tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Table {
    Codes(usize),
    LenFix,
    DistFix,
}

/// The fixed tables, as `inffixed.h` holds them (see [`fixed_tables`]).
pub(super) struct Fixed {
    pub lenfix: [Code; 512],
    pub distfix: [Code; 32],
}

// zlib-1.3/inflate.c:221 fixedtables (BUILDFIXED) and :282 makefixed
/// The fixed tables built as `fixedtables()` builds them with
/// `BUILDFIXED`, then written as `makefixed()` writes `inffixed.h`, the
/// tables the library is compiled with: the two invalid length symbols 286
/// and 287 (`op` 198 and 203 from `lext`) become the plain invalid-code
/// marker 64, which every decoder treats the same way.
pub(super) static FIXED: LazyLock<Fixed> = LazyLock::new(fixed_tables);

fn fixed_tables() -> Fixed {
    let mut lens = [0u16; 320];
    let mut work = [0u16; 288];
    let mut fixed = [Code::default(); 544];
    // literal/length table
    for (sym, l) in lens.iter_mut().enumerate().take(288) {
        *l = match sym {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let mut next = 0;
    let mut bits = 9;
    inflate_table(CodeType::LENS, &lens, 288, &mut fixed, &mut next, &mut bits, &mut work);
    // distance table
    lens[..32].fill(5);
    let dist_start = next;
    bits = 5;
    inflate_table(CodeType::DISTS, &lens, 32, &mut fixed, &mut next, &mut bits, &mut work);
    let mut lenfix = [Code::default(); 512];
    lenfix.copy_from_slice(&fixed[..512]);
    for (low, c) in lenfix.iter_mut().enumerate() {
        if low & 127 == 99 {
            c.op = 64;
        }
    }
    let mut distfix = [Code::default(); 32];
    distfix.copy_from_slice(&fixed[dist_start..dist_start + 32]);
    Fixed { lenfix, distfix }
}

/// A safety net for mutation testing, active with debug assertions only
/// (and excluded from the mutants, `-E OutputGuard`): one inflate() call
/// writes at most the `avail_out` bytes it was given, so a broken copy that
/// would write gigabytes panics at once instead of exhausting the memory.
#[derive(Clone, Copy)]
pub(super) struct OutputGuard {
    limit: usize,
}

impl OutputGuard {
    pub(super) fn new(len: usize, avail_out: u32) -> OutputGuard {
        OutputGuard { limit: len + avail_out as usize }
    }

    /// `n` more bytes are about to be written after `len`.
    #[inline]
    pub(super) fn check(self, len: usize, n: usize) {
        debug_assert!(len + n <= self.limit, "inflate writes past avail_out");
    }
}

// zlib-1.3/inflate.h:82 inflate_state
/// State maintained between inflate() calls.
pub struct InflateState {
    /// current inflate mode
    pub(super) mode: Mode,
    /// true if processing last block
    pub(super) last: bool,
    /// bit 0 true for zlib, bit 2 true to validate check value (0 for raw)
    pub(super) wrap: u32,
    /// protected copy of check value
    pub(super) check: u64,
    // sliding window
    /// log base 2 of requested window size
    pub(super) wbits: u32,
    /// window size or zero if not using window
    pub(super) wsize: u32,
    /// valid bytes in the window
    pub(super) whave: u32,
    /// window write index
    pub(super) wnext: u32,
    /// allocated sliding window, if needed
    pub(super) window: Vec<u8>,
    // bit accumulator
    /// input bit accumulator
    pub(super) hold: u64,
    /// number of bits in "in"
    pub(super) bits: u32,
    // for string and stored block copying
    /// literal or length of data to copy
    pub(super) length: u32,
    /// distance back to copy string from
    pub(super) offset: u32,
    // for table and code decoding
    /// extra bits needed
    pub(super) extra: u32,
    // fixed and dynamic code tables
    /// starting table for length/literal codes
    pub(super) lencode: Table,
    /// starting table for distance codes
    pub(super) distcode: Table,
    /// index bits for lencode
    pub(super) lenbits: u32,
    /// index bits for distcode
    pub(super) distbits: u32,
    // dynamic table building
    /// number of code length code lengths
    pub(super) ncode: u32,
    /// number of length code lengths
    pub(super) nlen: u32,
    /// number of distance code lengths
    pub(super) ndist: u32,
    /// number of code lengths in lens[]
    pub(super) have: u32,
    /// next available space in codes[]
    pub(super) next: usize,
    /// temporary storage for code lengths
    pub(super) lens: [u16; 320],
    /// work area for code table building
    pub(super) work: [u16; 288],
    /// space for code tables
    pub(super) codes: [Code; ENOUGH],
}

impl InflateState {
    /// `state->lencode[i]` / `state->distcode[i]`
    #[inline]
    pub(super) fn code(&self, table: Table, i: usize) -> Code {
        match table {
            Table::Codes(start) => self.codes[start + i],
            Table::LenFix => FIXED.lenfix[i],
            Table::DistFix => FIXED.distfix[i],
        }
    }
}

/// The part of `z_stream` inflate uses (see the module documentation for
/// `next_in`/`next_out`).
pub struct ZStream {
    /// index of the next input byte in the slice given to `inflate`
    pub next_in: usize,
    /// number of bytes available at next_in
    pub avail_in: u32,
    /// remaining free space in the output buffer
    pub avail_out: u32,
    /// last error message, `None` if no error
    pub msg: Option<&'static str>,
    pub(super) state: Box<InflateState>,
}

// zlib-1.3/inflate.c:146 inflateReset2 (and :105 inflateResetKeep, :132 inflateReset)
/// The state after `inflateReset2(strm, windowBits)`: `Err(Z_STREAM_ERROR)`
/// for an invalid `windowBits`, `Ok(None)` for a gzip request (not ported).
fn reset2(window_bits: i32) -> Result<Option<(u32, u32)>, i32> {
    // extract wrap request from windowBits parameter
    let (wrap, mut window_bits) = if window_bits < 0 {
        if window_bits < -15 {
            return Err(Z_STREAM_ERROR);
        }
        (0, -window_bits)
    } else {
        ((window_bits >> 4) + 5, window_bits)
    };
    if window_bits < 48 {
        window_bits &= 15;
    }
    // set number of window bits
    if window_bits != 0 && !(8..=15).contains(&window_bits) {
        return Err(Z_STREAM_ERROR);
    }
    if wrap & 2 != 0 {
        return Ok(None); // gzip decoding (wbits 16 to 47): not ported
    }
    Ok(Some((wrap as u32, window_bits as u32)))
}

// zlib-1.3/inflate.c:182 inflateInit2_
/// `inflateInit2(strm, windowBits)`: `Err(Z_STREAM_ERROR)` for an invalid
/// `windowBits`; `Ok(None)` for a gzip request (not ported).
pub fn inflate_init2(window_bits: i32) -> Result<Option<ZStream>, i32> {
    let Some((wrap, wbits)) = reset2(window_bits)? else { return Ok(None) };
    let state = Box::new(InflateState {
        mode: Mode::HEAD,
        last: false,
        wrap,
        check: 0,
        wbits,
        wsize: 0,
        whave: 0,
        wnext: 0,
        window: Vec::new(),
        hold: 0,
        bits: 0,
        length: 0,
        offset: 0,
        extra: 0,
        lencode: Table::Codes(0),
        distcode: Table::Codes(0),
        lenbits: 0,
        distbits: 0,
        ncode: 0,
        nlen: 0,
        ndist: 0,
        have: 0,
        next: 0,
        lens: [0; 320],
        work: [0; 288],
        codes: [Code::default(); ENOUGH],
    });
    Ok(Some(ZStream { next_in: 0, avail_in: 0, avail_out: 0, msg: None, state }))
}

// zlib-1.3/inflate.c:321 updatewindow
/// Update the window with the last wsize (normally 32K) bytes written
/// before returning (`end` is the end of the output, `copy` the bytes
/// written by this call). If window does not exist yet, create it.
fn updatewindow(state: &mut InflateState, output: &[u8], copy: u32) {
    let end = output.len();
    // if it hasn't been done already, allocate space for the window
    if state.window.is_empty() {
        state.window = vec![0; 1 << state.wbits];
    }
    // if window not in use yet, initialize
    if state.wsize == 0 {
        state.wsize = 1 << state.wbits;
        state.wnext = 0;
        state.whave = 0;
    }
    // copy state->wsize or less output bytes into the circular window
    if copy >= state.wsize {
        let wsize = state.wsize as usize;
        state.window[..wsize].copy_from_slice(&output[end - wsize..]);
        state.wnext = 0;
        state.whave = state.wsize;
    } else {
        let mut copy = copy as usize;
        let wnext = state.wnext as usize;
        let dist = ((state.wsize - state.wnext) as usize).min(copy);
        state.window[wnext..wnext + dist].copy_from_slice(&output[end - copy..end - copy + dist]);
        copy -= dist;
        if copy != 0 {
            state.window[..copy].copy_from_slice(&output[end - copy..]);
            state.wnext = copy as u32;
            state.whave = state.wsize;
        } else {
            state.wnext += dist as u32;
            if state.wnext == state.wsize {
                state.wnext = 0;
            }
            if state.whave < state.wsize {
                state.whave += dist as u32;
            }
        }
    }
}

/// permutation of code lengths
const ORDER: [u16; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

// zlib-1.3/inflate.c:590 inflate
/// `inflate(strm, flush)`: decompresses as much of `input[strm.next_in..]`
/// (`strm.avail_in` bytes) as possible into `output` (at most
/// `strm.avail_out` bytes), and returns `Z_OK`, `Z_STREAM_END`,
/// `Z_NEED_DICT`, `Z_DATA_ERROR` (with `strm.msg`) or `Z_BUF_ERROR` (no
/// progress possible, or `Z_FINISH` before the end of the stream).
pub fn inflate(strm: &mut ZStream, input: &[u8], output: &mut Vec<u8>, flush: Flush) -> i32 {
    if strm.state.mode == Mode::TYPE {
        strm.state.mode = Mode::TYPEDO; // skip check
    }
    // LOAD()
    let mut next = strm.next_in; // next input
    let mut have = strm.avail_in; // available input
    let mut left = strm.avail_out; // available output
    let mut hold = strm.state.hold; // bit buffer
    let mut bits = strm.state.bits; // bits in bit buffer
    let in0 = have; // save starting available input
    let mut out = left; // save starting available output
    let guard = OutputGuard::new(output.len(), left);
    let mut ret = Z_OK;

    // Get a byte of input into the bit accumulator, or return from inflate()
    // if there is no input available.
    macro_rules! pullbyte {
        ($leave:lifetime) => {{
            if have == 0 {
                break $leave;
            }
            have -= 1;
            hold += u64::from(input[next]) << bits;
            next += 1;
            bits += 8;
        }};
    }
    // Assure that there are at least n bits in the bit accumulator.
    macro_rules! needbits {
        ($leave:lifetime, $n:expr) => {{
            while bits < ($n) as u32 {
                pullbyte!($leave);
            }
        }};
    }
    // Return the low n bits of the bit accumulator (n < 16)
    macro_rules! bits_ {
        ($n:expr) => {
            (hold as u32) & ((1u32 << ($n)) - 1)
        };
    }
    // Remove n bits from the bit accumulator
    macro_rules! dropbits {
        ($n:expr) => {{
            let n = ($n) as u32;
            hold >>= n;
            bits -= n;
        }};
    }
    // Clear the input bit accumulator
    macro_rules! initbits {
        () => {{
            hold = 0;
            bits = 0;
        }};
    }
    // Remove zero to seven bits as needed to go to a byte boundary
    macro_rules! bytebits {
        () => {{
            hold >>= bits & 7;
            bits -= bits & 7;
        }};
    }
    // RESTORE(): restore state from registers
    macro_rules! restore {
        () => {{
            strm.next_in = next;
            strm.avail_out = left;
            strm.avail_in = have;
            strm.state.hold = hold;
            strm.state.bits = bits;
        }};
    }
    // strm->msg = msg; state->mode = BAD;
    macro_rules! bad {
        ($state:ident, $msg:expr) => {{
            strm.msg = Some($msg);
            $state.mode = Mode::BAD;
        }};
    }

    'leave: loop {
        let state = &mut *strm.state;
        match state.mode {
            Mode::HEAD => {
                if state.wrap == 0 {
                    state.mode = Mode::TYPEDO;
                    continue;
                }
                needbits!('leave, 16);
                if state.wrap & 1 == 0 || !((bits_!(8) << 8) + (hold >> 8) as u32).is_multiple_of(31) {
                    bad!(state, "incorrect header check");
                    continue;
                }
                if bits_!(4) != Z_DEFLATED {
                    bad!(state, "unknown compression method");
                    continue;
                }
                dropbits!(4);
                let len = bits_!(4) + 8;
                if state.wbits == 0 {
                    state.wbits = len;
                }
                if len > 15 || len > state.wbits {
                    bad!(state, "invalid window size");
                    continue;
                }
                state.check = 1; // adler32(0L, Z_NULL, 0)
                state.mode = if hold & 0x200 != 0 { Mode::DICTID } else { Mode::TYPE };
                initbits!();
            }
            Mode::DICTID => {
                needbits!('leave, 32);
                state.check = u64::from((hold as u32).swap_bytes()); // ZSWAP32
                initbits!();
                state.mode = Mode::DICT;
            }
            Mode::DICT => {
                // no inflateSetDictionary(): CPython only sets a zdict
                restore!();
                return Z_NEED_DICT;
            }
            Mode::TYPE | Mode::TYPEDO => {
                // (TYPE: a Z_BLOCK or Z_TREES flush would return here)
                if state.last {
                    bytebits!();
                    state.mode = Mode::CHECK;
                    continue;
                }
                needbits!('leave, 3);
                state.last = bits_!(1) != 0;
                dropbits!(1);
                match bits_!(2) {
                    0 => state.mode = Mode::STORED, // stored block
                    1 => {
                        // fixed block
                        state.lencode = Table::LenFix;
                        state.lenbits = 9;
                        state.distcode = Table::DistFix;
                        state.distbits = 5;
                        state.mode = Mode::LEN_; // decode codes
                    }
                    2 => state.mode = Mode::TABLE, // dynamic block
                    _ => bad!(state, "invalid block type"),
                }
                dropbits!(2);
            }
            Mode::STORED => {
                bytebits!(); // go to byte boundary
                needbits!('leave, 32);
                if (hold & 0xffff) != ((hold >> 16) ^ 0xffff) {
                    bad!(state, "invalid stored block lengths");
                    continue;
                }
                state.length = (hold as u32) & 0xffff;
                initbits!();
                state.mode = Mode::COPY_;
            }
            Mode::COPY_ => state.mode = Mode::COPY,
            Mode::COPY => {
                let mut copy = state.length;
                if copy != 0 {
                    copy = copy.min(have).min(left);
                    if copy == 0 {
                        break 'leave;
                    }
                    guard.check(output.len(), copy as usize);
                    output.extend_from_slice(&input[next..next + copy as usize]);
                    have -= copy;
                    next += copy as usize;
                    left -= copy;
                    state.length -= copy;
                    continue;
                }
                state.mode = Mode::TYPE;
            }
            Mode::TABLE => {
                needbits!('leave, 14);
                state.nlen = bits_!(5) + 257;
                dropbits!(5);
                state.ndist = bits_!(5) + 1;
                dropbits!(5);
                state.ncode = bits_!(4) + 4;
                dropbits!(4);
                if state.nlen > 286 || state.ndist > 30 {
                    bad!(state, "too many length or distance symbols");
                    continue;
                }
                state.have = 0;
                state.mode = Mode::LENLENS;
            }
            Mode::LENLENS => {
                while state.have < state.ncode {
                    needbits!('leave, 3);
                    state.lens[usize::from(ORDER[state.have as usize])] = bits_!(3) as u16;
                    state.have += 1;
                    dropbits!(3);
                }
                while state.have < 19 {
                    state.lens[usize::from(ORDER[state.have as usize])] = 0;
                    state.have += 1;
                }
                state.next = 0;
                state.lencode = Table::Codes(0);
                state.lenbits = 7;
                if inflate_table(
                    CodeType::CODES,
                    &state.lens,
                    19,
                    &mut state.codes,
                    &mut state.next,
                    &mut state.lenbits,
                    &mut state.work,
                ) != 0
                {
                    bad!(state, "invalid code lengths set");
                    continue;
                }
                state.have = 0;
                state.mode = Mode::CODELENS;
            }
            Mode::CODELENS => {
                while state.have < state.nlen + state.ndist {
                    let mut here;
                    loop {
                        here = state.code(state.lencode, bits_!(state.lenbits) as usize);
                        if u32::from(here.bits) <= bits {
                            break;
                        }
                        pullbyte!('leave);
                    }
                    if here.val < 16 {
                        dropbits!(here.bits);
                        state.lens[state.have as usize] = here.val;
                        state.have += 1;
                    } else {
                        let len;
                        let mut copy;
                        if here.val == 16 {
                            needbits!('leave, u32::from(here.bits) + 2);
                            dropbits!(here.bits);
                            if state.have == 0 {
                                bad!(state, "invalid bit length repeat");
                                break;
                            }
                            len = state.lens[state.have as usize - 1];
                            copy = 3 + bits_!(2);
                            dropbits!(2);
                        } else if here.val == 17 {
                            needbits!('leave, u32::from(here.bits) + 3);
                            dropbits!(here.bits);
                            len = 0;
                            copy = 3 + bits_!(3);
                            dropbits!(3);
                        } else {
                            needbits!('leave, u32::from(here.bits) + 7);
                            dropbits!(here.bits);
                            len = 0;
                            copy = 11 + bits_!(7);
                            dropbits!(7);
                        }
                        if state.have + copy > state.nlen + state.ndist {
                            bad!(state, "invalid bit length repeat");
                            break;
                        }
                        while copy != 0 {
                            copy -= 1;
                            state.lens[state.have as usize] = len;
                            state.have += 1;
                        }
                    }
                }
                // handle error breaks in while
                if state.mode == Mode::BAD {
                    continue;
                }
                // check for end-of-block code (better have one)
                if state.lens[256] == 0 {
                    bad!(state, "invalid code -- missing end-of-block");
                    continue;
                }
                // build code tables
                state.next = 0;
                state.lencode = Table::Codes(0);
                state.lenbits = 9;
                let nlen = state.nlen as usize;
                if inflate_table(
                    CodeType::LENS,
                    &state.lens,
                    nlen,
                    &mut state.codes,
                    &mut state.next,
                    &mut state.lenbits,
                    &mut state.work,
                ) != 0
                {
                    bad!(state, "invalid literal/lengths set");
                    continue;
                }
                state.distcode = Table::Codes(state.next);
                state.distbits = 6;
                let ndist = state.ndist as usize;
                if inflate_table(
                    CodeType::DISTS,
                    &state.lens[nlen..],
                    ndist,
                    &mut state.codes,
                    &mut state.next,
                    &mut state.distbits,
                    &mut state.work,
                ) != 0
                {
                    bad!(state, "invalid distances set");
                    continue;
                }
                state.mode = Mode::LEN_;
            }
            Mode::LEN_ => state.mode = Mode::LEN,
            Mode::LEN => {
                if have >= 6 && left >= 258 {
                    restore!();
                    super::inffast::inflate_fast(strm, input, output, out);
                    // LOAD()
                    next = strm.next_in;
                    have = strm.avail_in;
                    left = strm.avail_out;
                    hold = strm.state.hold;
                    bits = strm.state.bits;
                    continue;
                }
                let mut here;
                loop {
                    here = state.code(state.lencode, bits_!(state.lenbits) as usize);
                    if u32::from(here.bits) <= bits {
                        break;
                    }
                    pullbyte!('leave);
                }
                if here.op != 0 && (here.op & 0xf0) == 0 {
                    let last = here;
                    loop {
                        let i = usize::from(last.val) + (bits_!(last.bits + last.op) >> last.bits) as usize;
                        here = state.code(state.lencode, i);
                        if u32::from(last.bits) + u32::from(here.bits) <= bits {
                            break;
                        }
                        pullbyte!('leave);
                    }
                    dropbits!(last.bits);
                }
                dropbits!(here.bits);
                state.length = u32::from(here.val);
                if here.op == 0 {
                    state.mode = Mode::LIT;
                    continue;
                }
                if here.op & 32 != 0 {
                    // end of block
                    state.mode = Mode::TYPE;
                    continue;
                }
                if here.op & 64 != 0 {
                    bad!(state, "invalid literal/length code");
                    continue;
                }
                state.extra = u32::from(here.op) & 15;
                state.mode = Mode::LENEXT;
            }
            Mode::LENEXT => {
                if state.extra != 0 {
                    needbits!('leave, state.extra);
                    state.length += bits_!(state.extra);
                    dropbits!(state.extra);
                }
                state.mode = Mode::DIST;
            }
            Mode::DIST => {
                let mut here;
                loop {
                    here = state.code(state.distcode, bits_!(state.distbits) as usize);
                    if u32::from(here.bits) <= bits {
                        break;
                    }
                    pullbyte!('leave);
                }
                if (here.op & 0xf0) == 0 {
                    let last = here;
                    loop {
                        let i = usize::from(last.val) + (bits_!(last.bits + last.op) >> last.bits) as usize;
                        here = state.code(state.distcode, i);
                        if u32::from(last.bits) + u32::from(here.bits) <= bits {
                            break;
                        }
                        pullbyte!('leave);
                    }
                    dropbits!(last.bits);
                }
                dropbits!(here.bits);
                if here.op & 64 != 0 {
                    bad!(state, "invalid distance code");
                    continue;
                }
                state.offset = u32::from(here.val);
                state.extra = u32::from(here.op) & 15;
                state.mode = Mode::DISTEXT;
            }
            Mode::DISTEXT => {
                if state.extra != 0 {
                    needbits!('leave, state.extra);
                    state.offset += bits_!(state.extra);
                    dropbits!(state.extra);
                }
                state.mode = Mode::MATCH;
            }
            Mode::MATCH => {
                if left == 0 {
                    break 'leave;
                }
                let mut copy = out - left;
                if state.offset > copy {
                    // copy from window
                    copy = state.offset - copy;
                    if copy > state.whave {
                        bad!(state, "invalid distance too far back");
                        continue;
                    }
                    let from = if copy > state.wnext {
                        copy -= state.wnext;
                        state.wsize - copy
                    } else {
                        state.wnext - copy
                    } as usize;
                    copy = copy.min(state.length).min(left);
                    guard.check(output.len(), copy as usize);
                    output.extend_from_slice(&state.window[from..from + copy as usize]);
                } else {
                    // copy from output
                    copy = state.length.min(left);
                    let from = output.len() - state.offset as usize;
                    guard.check(output.len(), copy as usize);
                    for k in 0..copy as usize {
                        let b = output[from + k];
                        output.push(b);
                    }
                }
                left -= copy;
                state.length -= copy;
                if state.length == 0 {
                    state.mode = Mode::LEN;
                }
            }
            Mode::LIT => {
                if left == 0 {
                    break 'leave;
                }
                guard.check(output.len(), 1);
                output.push(state.length as u8);
                left -= 1;
                state.mode = Mode::LEN;
            }
            Mode::CHECK => {
                if state.wrap != 0 {
                    needbits!('leave, 32);
                    out -= left;
                    if state.wrap & 4 != 0 && out != 0 {
                        state.check = adler32(state.check, &output[output.len() - out as usize..]);
                    }
                    out = left;
                    if state.wrap & 4 != 0 && u64::from((hold as u32).swap_bytes()) != state.check {
                        bad!(state, "incorrect data check");
                        continue;
                    }
                    initbits!();
                }
                // (LENGTH: gzip only)
                state.mode = Mode::DONE;
            }
            Mode::DONE => {
                ret = Z_STREAM_END;
                break 'leave;
            }
            Mode::BAD => {
                ret = Z_DATA_ERROR;
                break 'leave;
            }
        }
    }

    // inf_leave: Return from inflate(), updating the check value. If there
    // was no progress during the inflate() call, return a buffer error.
    // Call updatewindow() to create and/or update the window state.
    restore!();
    let state = &mut *strm.state;
    if state.wsize != 0
        || (out != strm.avail_out && state.mode < Mode::BAD && (state.mode < Mode::CHECK || flush != Flush::Finish))
    {
        updatewindow(state, output, out - strm.avail_out);
    }
    let in_used = in0 - strm.avail_in;
    let out_used = out - strm.avail_out;
    if state.wrap & 4 != 0 && out_used != 0 {
        state.check = adler32(state.check, &output[output.len() - out_used as usize..]);
    }
    if ((in_used == 0 && out_used == 0) || flush == Flush::Finish) && ret == Z_OK {
        ret = Z_BUF_ERROR;
    }
    ret
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::FIXED;

    /// `inffixed.h` as `makefixed()` writes it from [`FIXED`]: its SHA-256
    /// is that of the file in the zlib 1.3 sources, so the tables are
    /// those the library is compiled with.
    #[test]
    fn fixed_tables_are_inffixed_h() {
        let mut out = String::from(
            "    /* inffixed.h -- table for decoding fixed codes\n     * Generated automatically by makefixed().\n     \
             */\n\n    /* WARNING: this file should *not* be used by applications.\n       It is part of the \
             implementation of this library and is\n       subject to change. Applications should only use zlib.h.\n     \
             */\n\n",
        );
        for (name, table, per_line) in [("lenfix", &FIXED.lenfix[..], 7), ("distfix", &FIXED.distfix[..], 6)] {
            if name == "distfix" {
                out.push('\n');
            }
            out.push_str(&format!("    static const code {name}[{}] = {{", table.len()));
            for (low, c) in table.iter().enumerate() {
                if low % per_line == 0 {
                    out.push_str("\n        ");
                }
                out.push_str(&format!("{{{},{},{}}}", c.op, c.bits, c.val));
                if low + 1 < table.len() {
                    out.push(',');
                }
            }
            out.push_str("\n    };\n");
        }
        let sha: String = Sha256::digest(out.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        // sha256sum zlib-1.3/inffixed.h
        assert_eq!(sha, "237ba710f090e432b62ebf963bee8b302867e9691406b2d3f8ee89ee7bfef9b0", "{out}");
    }
}
