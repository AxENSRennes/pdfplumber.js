//! zlib 1.3 `inffast.c`: the fast path of `inflate()` for literals and
//! length/distance pairs, used while at least 6 input bytes and 258 output
//! bytes are available. Same decoding and errors as the slow path, but
//! fewer checks; the unused whole bytes of the bit accumulator are given
//! back to the input on return.

use super::inflate::{Mode, ZStream};

/// Where a match is copied from: the sliding window or the output.
#[derive(Clone, Copy)]
enum From {
    Window(usize),
    Output(usize),
}

// zlib-1.3/inffast.c:50 inflate_fast
/// Decode literal, length, and distance codes and write out the resulting
/// literal and match bytes until either not enough input or output is
/// available, an end-of-block is encountered, or a data error is
/// encountered. `start` is inflate()'s starting value for
/// `strm.avail_out`. Entry assumptions: `state.mode == LEN`,
/// `strm.avail_in >= 6`, `strm.avail_out >= 258`, `state.bits < 8`.
pub(super) fn inflate_fast(strm: &mut ZStream, input: &[u8], output: &mut Vec<u8>, start: u32) {
    let state = &mut *strm.state;
    // copy state to local variables
    let mut inp = strm.next_in; // local strm->next_in
    let last = inp + (strm.avail_in as usize - 5); // have enough input while in < last
    let beg = output.len() - (start - strm.avail_out) as usize; // inflate()'s initial strm->next_out
    let end = output.len() + (strm.avail_out as usize - 257); // while out < end, enough space available
    let wsize = state.wsize as usize; // window size or zero if not using window
    let whave = state.whave as usize; // valid bytes in the window
    let wnext = state.wnext as usize; // window write index
    let mut hold = state.hold; // local strm->hold
    let mut bits = state.bits; // local strm->bits
    let lcode = state.lencode; // local strm->lencode
    let dcode = state.distcode; // local strm->distcode
    let lmask: u64 = (1u64 << state.lenbits) - 1; // mask for first level of length codes
    let dmask: u64 = (1u64 << state.distbits) - 1; // mask for first level of distance codes

    macro_rules! pull {
        () => {{
            hold += u64::from(input[inp]) << bits;
            inp += 1;
            bits += 8;
        }};
    }

    // decode literals and length/distances until end-of-block or not
    // enough input data or output space
    'outer: loop {
        if bits < 15 {
            pull!();
            pull!();
        }
        let mut here = state.code(lcode, (hold & lmask) as usize);
        loop {
            // dolen:
            let op = u32::from(here.bits);
            hold >>= op;
            bits -= op;
            let mut op = u32::from(here.op);
            if op == 0 {
                // literal
                output.push(here.val as u8);
            } else if op & 16 != 0 {
                // length base
                let mut len = u32::from(here.val);
                op &= 15; // number of extra bits
                if op != 0 {
                    if bits < op {
                        pull!();
                    }
                    len += (hold as u32) & ((1u32 << op) - 1);
                    hold >>= op;
                    bits -= op;
                }
                if bits < 15 {
                    pull!();
                    pull!();
                }
                here = state.code(dcode, (hold & dmask) as usize);
                loop {
                    // dodist:
                    let op = u32::from(here.bits);
                    hold >>= op;
                    bits -= op;
                    let mut op = u32::from(here.op);
                    if op & 16 != 0 {
                        // distance base
                        let mut dist = u32::from(here.val);
                        op &= 15; // number of extra bits
                        if bits < op {
                            pull!();
                            if bits < op {
                                pull!();
                            }
                        }
                        dist += (hold as u32) & ((1u32 << op) - 1);
                        hold >>= op;
                        bits -= op;
                        let mut op = (output.len() - beg) as u32; // max distance in output
                        let mut from;
                        if dist > op {
                            // see if copy from window
                            op = dist - op; // distance back in window
                            if op as usize > whave {
                                strm.msg = Some("invalid distance too far back");
                                state.mode = Mode::BAD;
                                break 'outer;
                            }
                            let op_ = op as usize;
                            if wnext == 0 {
                                // very common case
                                from = From::Window(wsize - op_);
                                if op < len {
                                    // some from window
                                    len -= op;
                                    copy(output, &state.window, &mut from, op);
                                    from = From::Output(output.len() - dist as usize);
                                    // rest from output
                                }
                            } else if wnext < op_ {
                                // wrap around window
                                from = From::Window(wsize + wnext - op_);
                                op -= wnext as u32;
                                if op < len {
                                    // some from end of window
                                    len -= op;
                                    copy(output, &state.window, &mut from, op);
                                    from = From::Window(0);
                                    if (wnext as u32) < len {
                                        // some from start of window
                                        op = wnext as u32;
                                        len -= op;
                                        copy(output, &state.window, &mut from, op);
                                        from = From::Output(output.len() - dist as usize);
                                        // rest from output
                                    }
                                }
                            } else {
                                // contiguous in window
                                from = From::Window(wnext - op_);
                                if op < len {
                                    // some from window
                                    len -= op;
                                    copy(output, &state.window, &mut from, op);
                                    from = From::Output(output.len() - dist as usize);
                                    // rest from output
                                }
                            }
                        } else {
                            // copy direct from output
                            from = From::Output(output.len() - dist as usize);
                        }
                        copy(output, &state.window, &mut from, len);
                        break;
                    } else if op & 64 == 0 {
                        // 2nd level distance code
                        here = state.code(dcode, usize::from(here.val) + ((hold as u32) & ((1u32 << op) - 1)) as usize);
                        continue;
                    } else {
                        strm.msg = Some("invalid distance code");
                        state.mode = Mode::BAD;
                        break 'outer;
                    }
                }
            } else if op & 64 == 0 {
                // 2nd level length code
                here = state.code(lcode, usize::from(here.val) + ((hold as u32) & ((1u32 << op) - 1)) as usize);
                continue;
            } else if op & 32 != 0 {
                // end-of-block
                state.mode = Mode::TYPE;
                break 'outer;
            } else {
                strm.msg = Some("invalid literal/length code");
                state.mode = Mode::BAD;
                break 'outer;
            }
            break;
        }
        if !(inp < last && output.len() < end) {
            break;
        }
    }

    // return unused bytes (on entry, bits < 8, so in won't go too far back)
    let len = bits >> 3;
    inp -= len as usize;
    bits -= len << 3;
    hold &= (1u64 << bits) - 1;

    // update state and return
    strm.avail_in = (last + 5 - inp) as u32;
    strm.next_in = inp;
    strm.avail_out = (end + 257 - output.len()) as u32;
    state.hold = hold;
    state.bits = bits;
}

/// `do { *out++ = *from++; } while (--n);` (and the unrolled copies, which
/// copy the same bytes): appends `n` bytes read at `from`, a match that
/// overlaps the output being written repeats it.
#[inline]
fn copy(output: &mut Vec<u8>, window: &[u8], from: &mut From, n: u32) {
    let n = n as usize;
    match from {
        From::Window(i) => {
            output.extend_from_slice(&window[*i..*i + n]);
            *i += n;
        }
        From::Output(i) => {
            let dist = output.len() - *i;
            if dist >= n {
                output.extend_from_within(*i..*i + n);
            } else {
                for k in 0..n {
                    let b = output[*i + k];
                    output.push(b);
                }
            }
            *i += n;
        }
    }
}
