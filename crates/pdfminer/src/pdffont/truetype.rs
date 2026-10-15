//! `TrueTypeFont` of `pdfminer/pdffont.py`: the table directory of an
//! embedded TrueType program (`/FontFile2`) and the Unicode `cmap`
//! subtables it reads to build a CID font's unicode map.
//!
//! Only `TrueTypeFont.__init__` catches `struct.error` (a truncated table
//! directory keeps the tables read so far); a truncated `cmap` table raises
//! `struct.error` out of `create_unicode_map`, and a subtable of a format
//! upstream does not read raises `AssertionError`, both of which end the
//! creation of the font (`PDFCIDFont` only suppresses `CMapNotFound`).

use indexmap::IndexMap;
use pycompat::io::{BytesIO, Pos};
use pycompat::{exc, ExcKind, PyInt, PyResult};
use rustc_hash::FxBuildHasher;

use super::frames::{self, Level, Logger};
use super::unpack::{read_n, read_n_u64, unpack_be, unpack_be_i16};
use crate::cmapdb::{Attrs, CMapBase, UnicodeMap};
use crate::pdftypes::PDFObject;

/// `char2gid`: character code -> glyph id, in the order of first insertion.
pub type Char2Gid = IndexMap<i64, i64, FxBuildHasher>;

/// pdfminer/pdffont.py:726 `TrueTypeFont`.
pub struct TrueTypeFont {
    pub name: String,
    pub fp: BytesIO,
    /// `self.tables`: tag -> `(offset, length)`.
    pub tables: IndexMap<Vec<u8>, (i64, i64), FxBuildHasher>,
    pub fonttype: Vec<u8>,
}

/// `TrueTypeFont.CMapNotFound()`.
fn cmap_not_found() -> pycompat::PyErr {
    pycompat::PyErr::new(ExcKind::TrueTypeFont_CMapNotFound, "")
}

/// `log.debug("parse_cmap_format: ...")`
fn log_debug() -> PyResult<()> {
    frames::log_frames(Logger::PdfFont, Level::Debug)
}

/// `struct.unpack(">{n}H", fp.read(2 * n))` (or `h`).
fn unpack_u16s(fp: &mut BytesIO, n: u64, signed: bool) -> PyResult<Vec<i64>> {
    let data = read_n_u64(fp, 2 * n)?;
    Ok(data.chunks_exact(2).map(|c| if signed { unpack_be_i16(c) } else { unpack_be(c, 2) }).collect())
}

impl TrueTypeFont {
    // pdfminer/pdffont.py:730 TrueTypeFont.__init__
    /// Reads the table directory; a truncated one keeps what was read.
    pub fn new(name: &str, fp: BytesIO) -> PyResult<TrueTypeFont> {
        let (_entry, _frame) = frames::c_called()?;
        let mut fp = fp;
        let fonttype = fp.read_bytes(Some(4)).to_vec();
        let mut tables = IndexMap::default();
        let directory = (|| -> PyResult<()> {
            let ntables = unpack_be(read_n(&mut fp, 8)?, 2);
            for _ in 0..ntables {
                let entry = read_n(&mut fp, 16)?;
                let (name_bytes, offset, length) =
                    (entry[..4].to_vec(), unpack_be(&entry[8..], 4), unpack_be(&entry[12..], 4));
                tables.insert(name_bytes, (offset, length));
            }
            Ok(())
        })();
        match directory {
            // Do not fail if there are not enough bytes to read. Even for
            // corrupted PDFs we would like to get as much information as
            // possible, so continue.
            Err(e) if e.is_instance(ExcKind::StructError) => {}
            other => other?,
        }
        Ok(TrueTypeFont { name: name.to_owned(), fp, tables, fonttype })
    }

    // pdfminer/pdffont.py:748 TrueTypeFont.create_unicode_map
    /// The unicode map of the Unicode subtables of the `cmap` table (a
    /// `FileUnicodeMap`: glyph id -> character), or `CMapNotFound` when
    /// there is no `cmap` table or it maps nothing.
    pub fn create_unicode_map(&mut self) -> PyResult<UnicodeMap> {
        let _frame = frames::frame()?;
        let Some(&(base_offset, _length)) = self.tables.get(&b"cmap"[..]) else {
            return Err(cmap_not_found());
        };
        let fp = &mut self.fp;
        fp.seek(base_offset as Pos)?;
        let header = read_n(fp, 4)?;
        let nsubtables = unpack_be(&header[2..], 2);
        let mut subtables = Vec::new();
        for _ in 0..nsubtables {
            let entry = read_n(fp, 8)?;
            subtables.push((unpack_be(entry, 2), unpack_be(&entry[2..], 2), unpack_be(&entry[4..], 4)));
        }
        let mut char2gid = Char2Gid::default();
        // Supports subtable type 0, 2, 4, 6, 10 and 12.
        for (platform_id, encoding_id, st_offset) in subtables {
            // Skip non-Unicode cmaps.
            // https://docs.microsoft.com/en-us/typography/opentype/spec/cmap
            if !(platform_id == 0 || (platform_id == 3 && [1, 10].contains(&encoding_id))) {
                continue;
            }
            fp.seek((base_offset + st_offset) as Pos)?;
            let fmttype = unpack_be(read_n(fp, 2)?, 2);
            match fmttype {
                0 => parse_cmap_format_0(fp, &mut char2gid)?,
                2 => parse_cmap_format_2(fp, &mut char2gid)?,
                4 => parse_cmap_format_4(fp, &mut char2gid)?,
                6 => parse_cmap_format_6(fp, &mut char2gid)?,
                10 => parse_cmap_format_10(fp, &mut char2gid)?,
                12 => parse_cmap_format_12(fp, &mut char2gid)?,
                _ => return Err(exc::AssertionError(format!("('Unhandled', {fmttype})"))),
            }
        }
        if char2gid.is_empty() {
            return Err(cmap_not_found());
        }
        // create unicode map
        let mut unicode_map = {
            let (_entry, _frame) = frames::c_called()?;
            frames::leaf(1)?; // CMapBase.__init__
            UnicodeMap::new_file(Attrs::default())
        };
        for (char, gid) in char2gid {
            let _f = frames::frame()?; // FileUnicodeMap.add_cid2unichr
            unicode_map.add_cid2unichr(PyInt::Small(gid), &PDFObject::Int(PyInt::Small(char)))?;
        }
        Ok(unicode_map)
    }
}

// pdfminer/pdffont.py:789 TrueTypeFont.parse_cmap_format_0
/// Parse cmap subtable format 0
pub fn parse_cmap_format_0(fp: &mut BytesIO, char2gid: &mut Char2Gid) -> PyResult<()> {
    let _frame = frames::frame()?;
    read_n(fp, 4)?;
    log_debug()?;
    let gids = read_n(fp, 256)?;
    for (c, &gid) in gids.iter().enumerate() {
        char2gid.insert(c as i64, i64::from(gid));
    }
    Ok(())
}

// pdfminer/pdffont.py:795 TrueTypeFont.parse_cmap_format_2
/// Parse cmap subtable format 2
pub fn parse_cmap_format_2(fp: &mut BytesIO, char2gid: &mut Char2Gid) -> PyResult<()> {
    let _frame = frames::frame()?;
    read_n(fp, 4)?;
    log_debug()?;
    let subheaderkeys = unpack_u16s(fp, 256, false)?;
    let mut firstbytes = vec![0i64; 8192];
    for (i, &k) in subheaderkeys.iter().enumerate() {
        firstbytes[(k / 8) as usize] = i as i64;
    }
    let nhdrs = subheaderkeys.iter().max().expect("256 keys") / 8 + 1;
    let mut hdrs = Vec::new();
    for i in 0..nhdrs {
        let h = read_n(fp, 8)?;
        let (firstcode, entcount, delta, offset) =
            (unpack_be(h, 2), unpack_be(&h[2..], 2), unpack_be_i16(&h[4..]), unpack_be(&h[6..], 2));
        hdrs.push((i, firstcode, entcount, delta, fp.tell() - 2 + offset));
    }
    for (i, firstcode, entcount, delta, pos) in hdrs {
        if entcount == 0 {
            continue;
        }
        let first = firstcode + (firstbytes[i as usize] << 8);
        fp.seek(pos)?;
        for c in 0..entcount {
            let mut gid = unpack_be(read_n(fp, 2)?, 2);
            if gid != 0 {
                gid += delta;
            }
            char2gid.insert(first + c, gid);
        }
    }
    Ok(())
}

// pdfminer/pdffont.py:819 TrueTypeFont.parse_cmap_format_4
/// Parse cmap subtable format 4
pub fn parse_cmap_format_4(fp: &mut BytesIO, char2gid: &mut Char2Gid) -> PyResult<()> {
    let _frame = frames::frame()?;
    read_n(fp, 4)?;
    log_debug()?;
    let segcount = unpack_be(read_n(fp, 8)?, 2) / 2;
    let n = segcount as u64;
    let ecs = unpack_u16s(fp, n, false)?;
    fp.read(Some(2));
    let scs = unpack_u16s(fp, n, false)?;
    let idds = unpack_u16s(fp, n, true)?;
    let pos = fp.tell();
    let idrs = unpack_u16s(fp, n, false)?;
    for (((&ec, &sc), &idd), &idr) in ecs.iter().zip(&scs).zip(&idds).zip(&idrs) {
        if idr != 0 {
            fp.seek(pos + idr)?;
            for c in sc..=ec {
                let b = unpack_be(read_n(fp, 2)?, 2);
                char2gid.insert(c, (b + idd) & 0xFFFF);
            }
        } else {
            for c in sc..=ec {
                char2gid.insert(c, (c + idd) & 0xFFFF);
            }
        }
    }
    Ok(())
}

// pdfminer/pdffont.py:841 TrueTypeFont.parse_cmap_format_6
/// Parse cmap subtable format 6
pub fn parse_cmap_format_6(fp: &mut BytesIO, char2gid: &mut Char2Gid) -> PyResult<()> {
    let _frame = frames::frame()?;
    read_n(fp, 4)?;
    log_debug()?;
    let h = read_n(fp, 4)?;
    let (firstcode, entcount) = (unpack_be(h, 2), unpack_be(&h[2..], 2));
    let gids = unpack_u16s(fp, entcount as u64, false)?;
    for (i, gid) in gids.into_iter().enumerate() {
        char2gid.insert(firstcode + i as i64, gid);
    }
    Ok(())
}

// pdfminer/pdffont.py:850 TrueTypeFont.parse_cmap_format_10
/// Parse cmap subtable format 10
pub fn parse_cmap_format_10(fp: &mut BytesIO, char2gid: &mut Char2Gid) -> PyResult<()> {
    let _frame = frames::frame()?;
    read_n(fp, 10)?;
    log_debug()?;
    let h = read_n(fp, 8)?;
    let (startcode, numchars) = (unpack_be(h, 4), unpack_be(&h[4..], 4));
    let gids = unpack_u16s(fp, numchars as u64, false)?;
    for (i, gid) in gids.into_iter().enumerate() {
        char2gid.insert(startcode + i as i64, gid);
    }
    Ok(())
}

// pdfminer/pdffont.py:861 TrueTypeFont.parse_cmap_format_12
/// Parse cmap subtable format 12
pub fn parse_cmap_format_12(fp: &mut BytesIO, char2gid: &mut Char2Gid) -> PyResult<()> {
    let _frame = frames::frame()?;
    read_n(fp, 10)?;
    log_debug()?;
    let numgroups = unpack_be(read_n(fp, 4)?, 4);
    for _ in 0..numgroups {
        let g = read_n(fp, 12)?;
        let (sc, ec, mut sgid) = (unpack_be(g, 4), unpack_be(&g[4..], 4), unpack_be(&g[8..], 4));
        for code in sc..=ec {
            char2gid.insert(code, sgid);
            sgid += 1;
        }
    }
    Ok(())
}
