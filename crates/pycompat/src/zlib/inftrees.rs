//! zlib 1.3 `inftrees.c` / `inftrees.h`: building the Huffman decoding
//! tables of `inflate`.
//!
//! A table is a run of [`Code`] entries in a code array (`state->codes`,
//! or the fixed tables); offsets replace C's pointers. The function
//! assumes, as zlib does, that every length is in `0..=MAXBITS` (the
//! callers only store 3- and 4-bit values).

const MAXBITS: usize = 15;

/// Maximum size of the dynamic table: 852 for literal/length codes and 592
/// for distance codes (`enough 286 9 15`, `enough 30 6 15`).
pub const ENOUGH_LENS: u32 = 852;
pub const ENOUGH_DISTS: u32 = 592;
pub const ENOUGH: usize = (ENOUGH_LENS + ENOUGH_DISTS) as usize;

// zlib-1.3/inftrees.h:24 code
/// A decoding table entry. `op`: 0 literal; `0000tttt` table link (`tttt`
/// index bits of the sub-table); `0001eeee` length or distance with `eeee`
/// extra bits; `01100000` end of block; `01000000` invalid code. `bits`:
/// bits of this part of the code. `val`: literal, base length or distance,
/// or offset of the sub-table from the current table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Code {
    pub op: u8,
    pub bits: u8,
    pub val: u16,
}

// zlib-1.3/inftrees.h:58 codetype
/// The type of code to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeType {
    CODES,
    LENS,
    DISTS,
}

/// Length codes 257..285 base.
const LBASE: [u16; 31] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258, 0, 0];
/// Length codes 257..285 extra.
const LEXT: [u16; 31] = [
    16, 16, 16, 16, 16, 16, 16, 16, 17, 17, 17, 17, 18, 18, 18, 18, 19, 19, 19, 19, 20, 20, 20, 20, 21, 21, 21, 21, 16, 198,
    203,
];
/// Distance codes 0..29 base.
const DBASE: [u16; 32] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193,
    12289, 16385, 24577, 0, 0,
];
/// Distance codes 0..29 extra.
const DEXT: [u16; 32] = [
    16, 16, 16, 16, 17, 17, 18, 18, 19, 19, 20, 20, 21, 21, 22, 22, 23, 23, 24, 24, 25, 25, 26, 26, 27, 27, 28, 28, 29, 29,
    64, 64,
];

// zlib-1.3/inftrees.c:32 inflate_table
/// Builds the tables decoding the canonical Huffman code of lengths
/// `lens[..codes]`, starting at `table[*start]`, with `*bits` root index
/// bits. Returns 0 on success (`*start` then points after the tables and
/// `*bits` is the actual root size), -1 for an over-subscribed or
/// incomplete set of lengths, 1 if `ENOUGH` is not enough.
pub fn inflate_table(
    ty: CodeType,
    lens: &[u16],
    codes: usize,
    table: &mut [Code],
    start: &mut usize,
    bits: &mut u32,
    work: &mut [u16],
) -> i32 {
    let mut count = [0u16; MAXBITS + 1]; // number of codes of each length
    let mut offs = [0u16; MAXBITS + 1]; // offsets in table for each length

    // accumulate lengths for codes (assumes lens[] all in 0..MAXBITS)
    for &l in &lens[..codes] {
        count[usize::from(l)] += 1;
    }

    // bound code lengths, force root to be within code lengths
    let mut root = *bits;
    let mut max = MAXBITS as u32;
    while max >= 1 {
        if count[max as usize] != 0 {
            break;
        }
        max -= 1;
    }
    if root > max {
        root = max;
    }
    if max == 0 {
        // no symbols to code at all: a table forcing an error
        let here = Code { op: 64, bits: 1, val: 0 }; // invalid code marker
        table[*start] = here;
        table[*start + 1] = here;
        *start += 2;
        *bits = 1;
        return 0; // no symbols, but wait for decoding to report error
    }
    let mut min = 1u32;
    while min < max {
        if count[min as usize] != 0 {
            break;
        }
        min += 1;
    }
    if root < min {
        root = min;
    }

    // check for an over-subscribed or incomplete set of lengths
    let mut left: i32 = 1;
    for &c in &count[1..=MAXBITS] {
        left <<= 1;
        left -= i32::from(c);
        if left < 0 {
            return -1; // over-subscribed
        }
    }
    if left > 0 && (ty == CodeType::CODES || max != 1) {
        return -1; // incomplete set
    }

    // generate offsets into symbol table for each length for sorting
    offs[1] = 0;
    for len in 1..MAXBITS {
        offs[len + 1] = offs[len] + count[len];
    }

    // sort symbols by length, by symbol order within each length
    for (sym, &l) in lens[..codes].iter().enumerate() {
        if l != 0 {
            work[usize::from(offs[usize::from(l)])] = sym as u16;
            offs[usize::from(l)] += 1;
        }
    }

    // set up for code type
    let (base, extra, matched): (&[u16], &[u16], u32) = match ty {
        CodeType::CODES => (&[], &[], 20), // base, extra: not used
        CodeType::LENS => (&LBASE, &LEXT, 257),
        CodeType::DISTS => (&DBASE, &DEXT, 0),
    };

    // initialize state for loop
    let mut huff: u32 = 0; // starting code
    let mut sym: usize = 0; // starting code symbol
    let mut len: u32 = min; // starting code length
    let root_table = *start;
    let mut next = *start; // current table to fill in
    let mut curr = root; // current table index bits
    let mut drop: u32 = 0; // current bits to drop from code for index
    let mut low: u32 = u32::MAX; // trigger new sub-table when len > root
    let mut used: u32 = 1 << root; // use root table entries
    let mask: u32 = used - 1; // mask for comparing low

    // check available table space
    if (ty == CodeType::LENS && used > ENOUGH_LENS) || (ty == CodeType::DISTS && used > ENOUGH_DISTS) {
        return 1;
    }

    // process all codes and make table entries
    loop {
        // create table entry
        let w = u32::from(work[sym]);
        let here = if w + 1 < matched {
            Code { op: 0, bits: (len - drop) as u8, val: w as u16 }
        } else if w >= matched {
            let k = (w - matched) as usize;
            Code { op: extra[k] as u8, bits: (len - drop) as u8, val: base[k] }
        } else {
            Code { op: 32 + 64, bits: (len - drop) as u8, val: 0 } // end of block
        };

        // replicate for those indices with low len bits equal to huff
        let incr = 1u32 << (len - drop);
        let mut fill = 1u32 << curr;
        min = fill; // save offset to next table
        loop {
            fill -= incr;
            table[next + ((huff >> drop) + fill) as usize] = here;
            if fill == 0 {
                break;
            }
        }

        // backwards increment the len-bit code huff
        let mut incr = 1u32 << (len - 1);
        while huff & incr != 0 {
            incr >>= 1;
        }
        if incr != 0 {
            huff &= incr - 1;
            huff += incr;
        } else {
            huff = 0;
        }

        // go to next symbol, update count, len
        sym += 1;
        count[len as usize] -= 1;
        if count[len as usize] == 0 {
            if len == max {
                break;
            }
            len = u32::from(lens[usize::from(work[sym])]);
        }

        // create new sub-table if needed
        if len > root && (huff & mask) != low {
            // if first time, transition to sub-tables
            if drop == 0 {
                drop = root;
            }

            // increment past last table
            next += min as usize; // here min is 1 << curr

            // determine length of next table
            curr = len - drop;
            let mut left: i32 = 1 << curr;
            while curr + drop < max {
                left -= i32::from(count[(curr + drop) as usize]);
                if left <= 0 {
                    break;
                }
                curr += 1;
                left <<= 1;
            }

            // check for enough space
            used += 1 << curr;
            if (ty == CodeType::LENS && used > ENOUGH_LENS) || (ty == CodeType::DISTS && used > ENOUGH_DISTS) {
                return 1;
            }

            // point entry in root table to sub-table
            low = huff & mask;
            table[root_table + low as usize] =
                Code { op: curr as u8, bits: root as u8, val: (next - root_table) as u16 };
        }
    }

    // fill in remaining table entry if code is incomplete (guaranteed to
    // have at most one remaining entry, since if the code is incomplete,
    // the maximum code length that was allowed to get this far is one bit)
    if huff != 0 {
        table[next + huff as usize] = Code { op: 64, bits: (len - drop) as u8, val: 0 }; // invalid code marker
    }

    // set return parameters
    *start = root_table + used as usize;
    *bits = root;
    0
}
