//! Port of the security handlers of `pdfminer/pdfdocument.py`
//! (`PDFStandardSecurityHandler`, `PDFStandardSecurityHandlerV4`,
//! `PDFStandardSecurityHandlerV5`, `PDFDocument.security_handler_registry`):
//! the standard security handler of PDF encryption (RC4, AES-128, AES-256).
//!
//! # Classes
//!
//! The three Python classes form a hierarchy whose methods dispatch on the
//! class of `self` (`init` calls the `init_params` of the subclass, which
//! calls the subclass's `get_cfm`...). The port is one struct,
//! [`PDFStandardSecurityHandler`], whose [`SecurityHandlerClass`] selects the
//! method each call resolves to, as Python's method resolution does. The
//! constructor takes exactly what upstream's `__init__` reads: the document
//! id (`docid`, the trailer's `/ID` list, or `(b"", b"")`), the encryption
//! dictionary (`param`) and the password; `PDFDocument._initialize_password`
//! picks the class with [`security_handler_registry`] and wraps
//! [`PDFStandardSecurityHandler::decrypt`] as the document's
//! `DecipherCallable` ([`PDFStandardSecurityHandler::decipher`]).
//!
//! # Python semantics
//!
//! The entries of `param` are read with pdfminer's `*_value` accessors
//! (`STRICT = False`: a wrong type becomes 0, `b""`, `{}`), so `/V`, `/R`,
//! `/Length` and `/P` keep Python's `int` (or `bool`) values and their
//! arithmetic: `self.length // 8` of any integer, `result[:n]` slicing with
//! a negative or huge `n`, `struct.pack("<L", ...)` raising `struct.error`
//! out of range, an empty RC4 key raising `ZeroDivisionError`. The
//! exceptions are upstream's, with their messages (`repr(param)` included).
//!
//! # Cryptographic primitives
//!
//! MD5 (`md-5`), SHA-256/384/512 (`sha2`) and the AES block cipher (`aes`)
//! are RustCrypto implementations of the standard algorithms `hashlib` and
//! OpenSSL compute. Upstream calls AES through `cryptography` 48 (the
//! oracle's): `Cipher(algorithms.AES(key), modes.CBC(iv)).decryptor()
//! .update(data)` is ported in [`CipherAesCbc`], with that library's checks
//! and messages (key sizes of 128/192/256/512 bits for `AES`, of at most 256
//! for CBC, a 16-byte IV) and its streaming behavior: `update` returns the
//! complete blocks only, so a trailing partial block is dropped (upstream
//! never calls `finalize` when decrypting); padding is removed by
//! `utils.unpad_aes`, never by the cipher.
//!
//! # Recursion depth
//!
//! `decrypt` runs inside `decipher_all`, whose recursion can reach the
//! Python frame limit: each ported method takes its frame, and the
//! `cryptography` calls take the frames its Python layer takes
//! ([`CipherAesCbc`]; measured against the oracle by the `deep` cases of
//! `oracle/unit/crypto.py`, with `cryptography`'s `isinstance` caches
//! warm, as they are once a process has decrypted anything). C recursion
//! units are not counted: these functions take none that could matter
//! (`decipher_all` recurses through Python frames only, far from the C
//! limit).

use std::rc::Rc;

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::{Aes128, Aes192, Aes256};
use indexmap::IndexMap;
use md5::{Digest, Md5};
use pycompat::recursion;
use pycompat::repr::str_repr;
use pycompat::{exc, ExcKind, PyErr, PyInt, PyResult};
use sha2::{Sha256, Sha384, Sha512};

use crate::arcfour::Arcfour;
use crate::pdftypes::{dict_value, int_value_obj, str_value, uint_value, DecipherCallable, PDFDict, PDFObject, PyBytes};
use crate::psparser::literal_name;
use crate::saslprep::saslprep;
use crate::utils::unpad_aes;

// pdfminer/pdfdocument.py:336 PDFStandardSecurityHandler.PASSWORD_PADDING
/// `PDFStandardSecurityHandler.PASSWORD_PADDING`
pub const PASSWORD_PADDING: &[u8; 32] =
    b"(\xbfN^Nu\x8aAd\x00NV\xff\xfa\x01\x08..\x00\xb6\xd0h>\x80/\x0c\xa9\xfedSiz";

/// Which of the three Python classes a handler is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecurityHandlerClass {
    /// `PDFStandardSecurityHandler` (RC4, revisions 2 and 3)
    PDFStandardSecurityHandler,
    /// `PDFStandardSecurityHandlerV4` (crypt filters, revision 4)
    PDFStandardSecurityHandlerV4,
    /// `PDFStandardSecurityHandlerV5` (AES-256, revisions 5 and 6)
    PDFStandardSecurityHandlerV5,
}

impl SecurityHandlerClass {
    /// The class `supported_revisions`.
    pub fn supported_revisions(self) -> &'static [i64] {
        match self {
            // pdfminer/pdfdocument.py:339 PDFStandardSecurityHandler.supported_revisions
            SecurityHandlerClass::PDFStandardSecurityHandler => &[2, 3],
            // pdfminer/pdfdocument.py:476 PDFStandardSecurityHandlerV4.supported_revisions
            SecurityHandlerClass::PDFStandardSecurityHandlerV4 => &[4],
            // pdfminer/pdfdocument.py:549 PDFStandardSecurityHandlerV5.supported_revisions
            SecurityHandlerClass::PDFStandardSecurityHandlerV5 => &[5, 6],
        }
    }

    /// `type(handler).__name__`
    pub fn name(self) -> &'static str {
        match self {
            SecurityHandlerClass::PDFStandardSecurityHandler => "PDFStandardSecurityHandler",
            SecurityHandlerClass::PDFStandardSecurityHandlerV4 => "PDFStandardSecurityHandlerV4",
            SecurityHandlerClass::PDFStandardSecurityHandlerV5 => "PDFStandardSecurityHandlerV5",
        }
    }
}

// pdfminer/pdfdocument.py:686 PDFDocument.security_handler_registry
/// `PDFDocument.security_handler_registry.get(v)`: the handler class of an
/// `/V` value (`int_value(param.get("V", 0))`; a `True` is 1).
pub fn security_handler_registry(v: &PyInt) -> Option<SecurityHandlerClass> {
    match v.as_i64()? {
        1 | 2 => Some(SecurityHandlerClass::PDFStandardSecurityHandler),
        4 => Some(SecurityHandlerClass::PDFStandardSecurityHandlerV4),
        5 => Some(SecurityHandlerClass::PDFStandardSecurityHandlerV5),
        _ => None,
    }
}

/// A crypt filter method of `self.cfm` (a bound method of the handler).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptFilterMethod {
    /// `self.decrypt_rc4`
    DecryptRc4,
    /// `self.decrypt_aes128`
    DecryptAes128,
    /// `self.decrypt_aes256`
    DecryptAes256,
    /// `self.decrypt_identity`
    DecryptIdentity,
}

impl CryptFilterMethod {
    /// The method's name.
    pub fn name(self) -> &'static str {
        match self {
            CryptFilterMethod::DecryptRc4 => "decrypt_rc4",
            CryptFilterMethod::DecryptAes128 => "decrypt_aes128",
            CryptFilterMethod::DecryptAes256 => "decrypt_aes256",
            CryptFilterMethod::DecryptIdentity => "decrypt_identity",
        }
    }
}

/// pdfminer/pdfdocument.py:335 `PDFStandardSecurityHandler` and its
/// subclasses `PDFStandardSecurityHandlerV4` (:475) and
/// `PDFStandardSecurityHandlerV5` (:548). The public fields are the
/// instance attributes of the same name; those of a subclass are `None`
/// (or empty) on the others.
#[derive(Clone)]
pub struct PDFStandardSecurityHandler {
    /// `type(self)`
    pub class: SecurityHandlerClass,
    /// `self.docid`
    pub docid: Vec<PDFObject>,
    /// `self.param`
    pub param: PDFDict,
    /// `self.password`
    pub password: String,
    /// `self.v`, `self.r`, `self.length`: `int_value` results (an `int`
    /// or a `bool`).
    pub v: PDFObject,
    pub r: PDFObject,
    pub length: PDFObject,
    /// `self.p`: `uint_value(param["P"], 32)` (an `int`, or `True`).
    pub p: PDFObject,
    /// `self.o`, `self.u`
    pub o: PyBytes,
    pub u: PyBytes,
    /// `self.key` (`None` until `init_key`).
    pub key: Option<PyBytes>,
    // PDFStandardSecurityHandlerV4
    /// `self.cf`
    pub cf: Option<PDFDict>,
    /// `self.stmf`, `self.strf`
    pub stmf: Option<String>,
    pub strf: Option<String>,
    /// `self.encrypt_metadata`
    pub encrypt_metadata: Option<bool>,
    /// `self.cfm`, in insertion order.
    pub cfm: IndexMap<String, CryptFilterMethod>,
    // PDFStandardSecurityHandlerV5
    /// `self.oe`, `self.ue`, and the slices of `o` and `u`.
    pub oe: Option<PyBytes>,
    pub ue: Option<PyBytes>,
    pub o_hash: Option<PyBytes>,
    pub o_validation_salt: Option<PyBytes>,
    pub o_key_salt: Option<PyBytes>,
    pub u_hash: Option<PyBytes>,
    pub u_validation_salt: Option<PyBytes>,
    pub u_key_salt: Option<PyBytes>,
}

// ---------------------------------------------------------------------------
// Python operations on the values the handlers read

/// The `int` value of an `int_value`/`uint_value` result (`bool` is 0/1).
fn int_of(x: &PDFObject) -> PyInt {
    match x {
        PDFObject::Int(i) => i.clone(),
        PDFObject::Bool(b) => PyInt::Small(i64::from(*b)),
        _ => unreachable!("int_value/uint_value return an int or a bool"),
    }
}

/// `struct.pack("<L", x)`
fn pack_l(x: &PyInt) -> PyResult<[u8; 4]> {
    match x.as_i64().and_then(|i| u32::try_from(i).ok()) {
        Some(v) => Ok(v.to_le_bytes()),
        None => Err(exc::StructError("'L' format requires 0 <= number <= 4294967295")),
    }
}

/// `b[:n]` for a Python `int` `n`.
fn slice_to<'a>(b: &'a [u8], n: &PyInt) -> &'a [u8] {
    let len = b.len() as i64;
    let end = match n.as_i64() {
        Some(n) if n >= 0 => n.min(len),
        Some(n) => (len + n).max(0),
        None if n.is_negative() => 0,
        None => len,
    };
    &b[..end as usize]
}

/// `b"".join(bytes((c ^ i,)) for c in iter(key))` (the generator
/// expression takes a frame).
fn xor_key(key: &[u8], i: u8) -> PyResult<Vec<u8>> {
    let _genexpr = recursion::py_frame()?;
    Ok(key.iter().map(|&c| c ^ i).collect())
}

/// `md5(data).digest()`
fn md5(parts: &[&[u8]]) -> [u8; 16] {
    let mut h = Md5::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// `hash.update(x)` of a `hashlib` object with an arbitrary object: the
/// bytes, or the `TypeError` of an object without the buffer protocol.
fn buffer(x: &PDFObject) -> PyResult<PyBytes> {
    match x {
        PDFObject::Bytes(b) => Ok(b.clone()),
        _ => Err(exc::TypeError("object supporting the buffer API required")),
    }
}

/// `x[key]` for a `str` key: `dict.__getitem__`, `PDFStream.__getitem__`,
/// or the `TypeError` of the other types.
fn getitem_str(x: &PDFObject, key: &str) -> PyResult<PDFObject> {
    match x {
        PDFObject::Dict(d) => d.get(key).ok_or_else(|| exc::KeyError(str_repr(key))),
        PDFObject::Stream(s) => s.getitem(key),
        PDFObject::List(_) => Err(exc::TypeError("list indices must be integers or slices, not str")),
        PDFObject::Bytes(_) => Err(exc::TypeError("byte indices must be integers or slices, not str")),
        other => Err(exc::TypeError(format!("'{}' object is not subscriptable", other.type_name()))),
    }
}

/// `param[key]`: `KeyError` when missing.
fn param_item(param: &PDFDict, key: &str) -> PyResult<PDFObject> {
    param.get(key).ok_or_else(|| exc::KeyError(str_repr(key)))
}

/// `s.encode("latin1")`, with the `UnicodeEncodeError` of the first run of
/// characters above U+00FF.
fn encode_latin1(s: &str) -> PyResult<Vec<u8>> {
    let chars: Vec<char> = s.chars().collect();
    let Some(start) = chars.iter().position(|&c| c as u32 > 0xff) else {
        return Ok(chars.iter().map(|&c| c as u8).collect());
    };
    let end = start + chars[start..].iter().take_while(|&&c| c as u32 > 0xff).count();
    let message = if end - start == 1 {
        let c = chars[start] as u32;
        let shown = if c <= 0xffff { format!("\\u{c:04x}") } else { format!("\\U{c:08x}") };
        format!("'latin-1' codec can't encode character '{shown}' in position {start}: ordinal not in range(256)")
    } else {
        format!(
            "'latin-1' codec can't encode characters in position {start}-{}: ordinal not in range(256)",
            end - 1
        )
    };
    Err(PyErr::new(ExcKind::UnicodeEncodeError, message))
}

/// `f"...: param={self.param!r}"`
fn param_message(what: &str, param: &PDFDict) -> PyResult<String> {
    Ok(format!("{what}: param={}", PDFObject::Dict(param.clone()).repr()?))
}

// ---------------------------------------------------------------------------
// cryptography: Cipher(algorithms.AES(key), modes.CBC(iv))

/// The AES block cipher of a validated key.
enum AesKey {
    A128(Box<Aes128>),
    A192(Box<Aes192>),
    A256(Box<Aes256>),
}

impl AesKey {
    fn decrypt_block(&self, block: &mut [u8; 16]) {
        let b = GenericArray::from_mut_slice(block);
        match self {
            AesKey::A128(k) => k.decrypt_block(b),
            AesKey::A192(k) => k.decrypt_block(b),
            AesKey::A256(k) => k.decrypt_block(b),
        }
    }

    fn encrypt_block(&self, block: &mut [u8; 16]) {
        let b = GenericArray::from_mut_slice(block);
        match self {
            AesKey::A128(k) => k.encrypt_block(b),
            AesKey::A192(k) => k.encrypt_block(b),
            AesKey::A256(k) => k.encrypt_block(b),
        }
    }
}

/// `Cipher(algorithms.AES(key), modes.CBC(iv)[, backend=default_backend()])`
/// of `cryptography` 48, and the contexts upstream uses.
///
/// Frames: `AES.__init__` -> `_verify_key_size` -> `utils._check_byteslike`
/// (3, then the key size check); `CBC.__init__` -> `_check_byteslike` (2);
/// `default_backend()` (1); `Cipher.__init__` -> `_check_iv_and_key_length`
/// -> `_check_aes_key_length` -> the `key_size` property, and
/// `_check_iv_length` -> the `initialization_vector` property (4, then
/// both checks); `decryptor()` (1, its Rust part needs no more);
/// `encryptor()` -> `ABCMeta.__instancecheck__` (2).
pub struct CipherAesCbc {
    key: AesKey,
    iv: [u8; 16],
}

impl CipherAesCbc {
    /// Builds the cipher, with `cryptography`'s checks in its order.
    pub fn new(key: &[u8], iv: &[u8], backend: bool) -> PyResult<CipherAesCbc> {
        // algorithms.AES(key)
        {
            let _frames = recursion::py_frames(3)?;
            let bits = key.len() * 8;
            if ![128, 192, 256, 512].contains(&bits) {
                return Err(exc::ValueError(format!("Invalid key size ({bits}) for AES.")));
            }
        }
        // modes.CBC(iv)
        drop(recursion::py_frames(2)?);
        if backend {
            // default_backend()
            drop(recursion::py_frame()?);
        }
        // Cipher.__init__
        let _frames = recursion::py_frames(4)?;
        let key = match key.len() {
            16 => AesKey::A128(Box::new(Aes128::new(GenericArray::from_slice(key)))),
            24 => AesKey::A192(Box::new(Aes192::new(GenericArray::from_slice(key)))),
            32 => AesKey::A256(Box::new(Aes256::new(GenericArray::from_slice(key)))),
            _ => return Err(exc::ValueError("Only 128, 192, and 256 bit keys are allowed for this AES mode")),
        };
        let Ok(iv) = <[u8; 16]>::try_from(iv) else {
            return Err(exc::ValueError(format!("Invalid IV size ({}) for CBC.", iv.len())));
        };
        Ok(CipherAesCbc { key, iv })
    }

    /// `cipher.decryptor().update(data)`: the complete blocks of `data`,
    /// decrypted (a trailing partial block stays buffered, never returned).
    pub fn decryptor_update(&self, data: &[u8]) -> PyResult<Vec<u8>> {
        drop(recursion::py_frame()?);
        let mut prev = self.iv;
        let mut out = Vec::with_capacity(data.len());
        for chunk in data.as_chunks::<16>().0 {
            let mut block = *chunk;
            self.key.decrypt_block(&mut block);
            for (b, p) in block.iter_mut().zip(prev) {
                *b ^= p;
            }
            out.extend_from_slice(&block);
            prev = *chunk;
        }
        Ok(out)
    }

    /// `e = cipher.encryptor(); e.update(data) + e.finalize()`: `finalize`
    /// raises when a partial block is left.
    pub fn encryptor_update_finalize(&self, data: &[u8]) -> PyResult<Vec<u8>> {
        drop(recursion::py_frames(2)?);
        let mut prev = self.iv;
        let mut out = Vec::with_capacity(data.len());
        for chunk in data.as_chunks::<16>().0 {
            let mut block = *chunk;
            for (b, p) in block.iter_mut().zip(prev) {
                *b ^= p;
            }
            self.key.encrypt_block(&mut block);
            out.extend_from_slice(&block);
            prev = block;
        }
        if !data.len().is_multiple_of(16) {
            return Err(exc::ValueError("The length of the provided data is not a multiple of the block length."));
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// The handlers

impl PDFStandardSecurityHandler {
    // pdfminer/pdfdocument.py:341 PDFStandardSecurityHandler.__init__
    /// `cls(docid, param, password)` for the class `class`: reads `param`,
    /// checks the revision and authenticates the password
    /// (`PDFPasswordIncorrect` when it is neither the user nor the owner
    /// password).
    pub fn new(
        class: SecurityHandlerClass,
        docid: Vec<PDFObject>,
        param: PDFDict,
        password: &str,
    ) -> PyResult<PDFStandardSecurityHandler> {
        let _frame = recursion::py_frame()?;
        let zero = PDFObject::Int(PyInt::ZERO);
        let mut handler = PDFStandardSecurityHandler {
            class,
            docid,
            param,
            password: password.to_owned(),
            v: zero.clone(),
            r: zero.clone(),
            length: zero.clone(),
            p: zero,
            o: Rc::from(&[][..]),
            u: Rc::from(&[][..]),
            key: None,
            cf: None,
            stmf: None,
            strf: None,
            encrypt_metadata: None,
            cfm: IndexMap::new(),
            oe: None,
            ue: None,
            o_hash: None,
            o_validation_salt: None,
            o_key_salt: None,
            u_hash: None,
            u_validation_salt: None,
            u_key_salt: None,
        };
        handler.init()?;
        Ok(handler)
    }

    /// The document's decryption callback: `self.decrypt`.
    pub fn decipher(self: &Rc<Self>) -> DecipherCallable {
        let handler = Rc::clone(self);
        Rc::new(move |objid: &PyInt, genno: &PyInt, data: &[u8], attrs: Option<&PDFDict>| {
            handler.decrypt(objid, genno, data, attrs)
        })
    }

    // pdfminer/pdfdocument.py:352 PDFStandardSecurityHandler.init
    fn init(&mut self) -> PyResult<()> {
        let _frame = recursion::py_frame()?;
        self.init_params()?;
        let r = int_of(&self.r);
        if !self.class.supported_revisions().iter().any(|&s| r == PyInt::Small(s)) {
            return Err(PyErr::new(
                ExcKind::PDFEncryptionError,
                param_message("Unsupported revision", &self.param)?,
            ));
        }
        self.init_key()
    }

    /// `self.init_params()`, resolved on the class.
    fn init_params(&mut self) -> PyResult<()> {
        match self.class {
            SecurityHandlerClass::PDFStandardSecurityHandler => self.init_params_base(),
            SecurityHandlerClass::PDFStandardSecurityHandlerV4 => self.init_params_v4(),
            SecurityHandlerClass::PDFStandardSecurityHandlerV5 => self.init_params_v5(),
        }
    }

    // pdfminer/pdfdocument.py:359 PDFStandardSecurityHandler.init_params
    fn init_params_base(&mut self) -> PyResult<()> {
        let _frame = recursion::py_frame()?;
        let v = self.param.get("V").unwrap_or(PDFObject::Int(PyInt::ZERO));
        self.v = int_value_obj(&v)?;
        self.r = int_value_obj(&param_item(&self.param, "R")?)?;
        self.p = uint_value(&param_item(&self.param, "P")?, 32)?;
        self.o = str_value(&param_item(&self.param, "O")?)?;
        self.u = str_value(&param_item(&self.param, "U")?)?;
        let length = self.param.get("Length").unwrap_or(PDFObject::Int(PyInt::Small(40)));
        self.length = int_value_obj(&length)?;
        Ok(())
    }

    // pdfminer/pdfdocument.py:478 PDFStandardSecurityHandlerV4.init_params
    fn init_params_v4(&mut self) -> PyResult<()> {
        let _frame = recursion::py_frame()?;
        self.init_params_base()?;
        self.length = PDFObject::Int(PyInt::Small(128));
        let cf = dict_value(&self.param.get("CF").unwrap_or(PDFObject::Null))?;
        self.cf = Some(cf.clone());
        self.stmf = Some(literal_name(&param_item(&self.param, "StmF")?)?);
        let strf = literal_name(&param_item(&self.param, "StrF")?)?;
        self.strf = Some(strf.clone());
        self.encrypt_metadata = Some(self.param.get("EncryptMetadata").is_none_or(|x| x.is_truthy()));
        if self.stmf.as_deref() != Some(strf.as_str()) {
            return Err(PyErr::new(
                ExcKind::PDFEncryptionError,
                param_message("Unsupported crypt filter", &self.param)?,
            ));
        }
        self.cfm = IndexMap::new();
        let mut i = 0;
        while let Some((k, v)) = cf.get_index(i) {
            let f = self.get_cfm(&literal_name(&getitem_str(&v, "CFM")?)?)?;
            let Some(f) = f else {
                return Err(PyErr::new(
                    ExcKind::PDFEncryptionError,
                    param_message("Unknown crypt filter method", &self.param)?,
                ));
            };
            self.cfm.insert(k.to_string(), f);
            i += 1;
        }
        self.cfm.insert("Identity".to_owned(), CryptFilterMethod::DecryptIdentity);
        if !self.cfm.contains_key(&strf) {
            return Err(PyErr::new(
                ExcKind::PDFEncryptionError,
                param_message("Undefined crypt filter", &self.param)?,
            ));
        }
        Ok(())
    }

    // pdfminer/pdfdocument.py:551 PDFStandardSecurityHandlerV5.init_params
    fn init_params_v5(&mut self) -> PyResult<()> {
        let _frame = recursion::py_frame()?;
        self.init_params_v4()?;
        self.length = PDFObject::Int(PyInt::Small(256));
        self.oe = Some(str_value(&param_item(&self.param, "OE")?)?);
        self.ue = Some(str_value(&param_item(&self.param, "UE")?)?);
        let part = |b: &PyBytes, lo: usize, hi: usize| -> PyBytes {
            let hi = hi.min(b.len());
            Rc::from(&b[lo.min(hi)..hi])
        };
        self.o_hash = Some(part(&self.o, 0, 32));
        self.o_validation_salt = Some(part(&self.o, 32, 40));
        self.o_key_salt = Some(part(&self.o, 40, usize::MAX));
        self.u_hash = Some(part(&self.u, 0, 32));
        self.u_validation_salt = Some(part(&self.u, 32, 40));
        self.u_key_salt = Some(part(&self.u, 40, usize::MAX));
        Ok(())
    }

    /// `self.get_cfm(name)`, resolved on the class (only the subclasses
    /// define it, and only their `init_params` calls it).
    fn get_cfm(&self, name: &str) -> PyResult<Option<CryptFilterMethod>> {
        match self.class {
            SecurityHandlerClass::PDFStandardSecurityHandler => {
                Err(exc::AttributeError("'PDFStandardSecurityHandler' object has no attribute 'get_cfm'"))
            }
            SecurityHandlerClass::PDFStandardSecurityHandlerV4 => self.get_cfm_v4(name),
            SecurityHandlerClass::PDFStandardSecurityHandlerV5 => self.get_cfm_v5(name),
        }
    }

    // pdfminer/pdfdocument.py:500 PDFStandardSecurityHandlerV4.get_cfm
    fn get_cfm_v4(&self, name: &str) -> PyResult<Option<CryptFilterMethod>> {
        let _frame = recursion::py_frame()?;
        Ok(match name {
            "V2" => Some(CryptFilterMethod::DecryptRc4),
            "AESV2" => Some(CryptFilterMethod::DecryptAes128),
            _ => None,
        })
    }

    // pdfminer/pdfdocument.py:563 PDFStandardSecurityHandlerV5.get_cfm
    fn get_cfm_v5(&self, name: &str) -> PyResult<Option<CryptFilterMethod>> {
        let _frame = recursion::py_frame()?;
        Ok(if name == "AESV3" { Some(CryptFilterMethod::DecryptAes256) } else { None })
    }

    // pdfminer/pdfdocument.py:367 PDFStandardSecurityHandler.init_key
    fn init_key(&mut self) -> PyResult<()> {
        let _frame = recursion::py_frame()?;
        let password = self.password.clone();
        self.key = self.authenticate(&password)?;
        if self.key.is_none() {
            return Err(PyErr::new(ExcKind::PDFPasswordIncorrect, ""));
        }
        Ok(())
    }

    /// `self.p & bit != 0`
    fn p_bit(&self, bit: u32) -> bool {
        // (p // 2**bit) % 2: the bit of Python's infinite two's complement
        let p = int_of(&self.p);
        let shifted = p.floordiv(&PyInt::Small(1 << bit)).expect("non-zero divisor");
        !shifted.modulo(&PyInt::Small(2)).expect("non-zero divisor").is_zero()
    }

    // pdfminer/pdfdocument.py:372 PDFStandardSecurityHandler.is_printable
    pub fn is_printable(&self) -> PyResult<bool> {
        let _frame = recursion::py_frame()?;
        Ok(self.p_bit(2))
    }

    // pdfminer/pdfdocument.py:375 PDFStandardSecurityHandler.is_modifiable
    pub fn is_modifiable(&self) -> PyResult<bool> {
        let _frame = recursion::py_frame()?;
        Ok(self.p_bit(3))
    }

    // pdfminer/pdfdocument.py:378 PDFStandardSecurityHandler.is_extractable
    pub fn is_extractable(&self) -> PyResult<bool> {
        let _frame = recursion::py_frame()?;
        Ok(self.p_bit(4))
    }

    /// `self.docid[0]`
    fn docid0(&self) -> PyResult<PDFObject> {
        self.docid.first().cloned().ok_or_else(|| exc::IndexError("list index out of range"))
    }

    // pdfminer/pdfdocument.py:381 PDFStandardSecurityHandler.compute_u
    pub fn compute_u(&self, key: &[u8]) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        if int_of(&self.r) == PyInt::Small(2) {
            // Algorithm 3.4
            return Arcfour::new(key)?.encrypt(PASSWORD_PADDING); // 2
        }
        // Algorithm 3.5
        let docid0 = buffer(&self.docid0()?)?;
        let hash = md5(&[PASSWORD_PADDING, &docid0]); // 2, 3
        let mut result = Arcfour::new(key)?.encrypt(&hash)?; // 4
        for i in 1..20u8 {
            // 5
            let k = xor_key(key, i)?;
            result = Arcfour::new(&k)?.encrypt(&result)?;
        }
        result.extend_from_within(..); // 6
        Ok(result)
    }

    /// `n = self.length // 8`
    fn length_bytes(&self) -> PyInt {
        int_of(&self.length).floordiv(&PyInt::Small(8)).expect("non-zero divisor")
    }

    // pdfminer/pdfdocument.py:396 PDFStandardSecurityHandler.compute_encryption_key
    pub fn compute_encryption_key(&self, password: &[u8]) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        // Algorithm 3.2
        let mut padded = password.to_vec();
        padded.extend_from_slice(PASSWORD_PADDING);
        padded.truncate(32); // 1
        let mut hash = Md5::new(); // 2
        hash.update(&padded);
        hash.update(&self.o); // 3
        // See https://github.com/pdfminer/pdfminer.six/issues/186
        hash.update(pack_l(&int_of(&self.p))?); // 4
        hash.update(&*buffer(&self.docid0()?)?); // 5
        let r = int_of(&self.r);
        // The base class has no `encrypt_metadata`, but its revisions (2, 3)
        // never reach this test.
        if r >= PyInt::Small(4) && !self.encrypt_metadata.unwrap_or(true) {
            hash.update(b"\xff\xff\xff\xff");
        }
        let mut result: Vec<u8> = hash.finalize().to_vec();
        let mut n = PyInt::Small(5);
        if r >= PyInt::Small(3) {
            n = self.length_bytes();
            for _ in 0..50 {
                result = md5(&[slice_to(&result, &n)]).to_vec();
            }
        }
        Ok(slice_to(&result, &n).to_vec())
    }

    /// `self.authenticate(password)`, resolved on the class: the key, or
    /// `None` for a wrong password.
    pub fn authenticate(&self, password: &str) -> PyResult<Option<PyBytes>> {
        match self.class {
            SecurityHandlerClass::PDFStandardSecurityHandlerV5 => self.authenticate_v5(password),
            _ => self.authenticate_base(password),
        }
    }

    // pdfminer/pdfdocument.py:417 PDFStandardSecurityHandler.authenticate
    fn authenticate_base(&self, password: &str) -> PyResult<Option<PyBytes>> {
        let _frame = recursion::py_frame()?;
        let password_bytes = encode_latin1(password)?;
        let key = self.authenticate_user_password(&password_bytes)?;
        if key.is_none() {
            return self.authenticate_owner_password(&password_bytes);
        }
        Ok(key)
    }

    // pdfminer/pdfdocument.py:424 PDFStandardSecurityHandler.authenticate_user_password
    pub fn authenticate_user_password(&self, password: &[u8]) -> PyResult<Option<PyBytes>> {
        let _frame = recursion::py_frame()?;
        let key = self.compute_encryption_key(password)?;
        if self.verify_encryption_key(&key)? {
            Ok(Some(Rc::from(key)))
        } else {
            Ok(None)
        }
    }

    // pdfminer/pdfdocument.py:431 PDFStandardSecurityHandler.verify_encryption_key
    pub fn verify_encryption_key(&self, key: &[u8]) -> PyResult<bool> {
        let _frame = recursion::py_frame()?;
        // Algorithm 3.6
        let u = self.compute_u(key)?;
        if int_of(&self.r) == PyInt::Small(2) {
            return Ok(u[..] == self.u[..]);
        }
        Ok(u[..u.len().min(16)] == self.u[..self.u.len().min(16)])
    }

    // pdfminer/pdfdocument.py:438 PDFStandardSecurityHandler.authenticate_owner_password
    pub fn authenticate_owner_password(&self, password: &[u8]) -> PyResult<Option<PyBytes>> {
        let _frame = recursion::py_frame()?;
        // Algorithm 3.7
        let mut padded = password.to_vec();
        padded.extend_from_slice(PASSWORD_PADDING);
        padded.truncate(32);
        let mut hash = md5(&[&padded]);
        let r = int_of(&self.r);
        if r >= PyInt::Small(3) {
            for _ in 0..50 {
                hash = md5(&[&hash]);
            }
        }
        let mut n = PyInt::Small(5);
        if r >= PyInt::Small(3) {
            n = self.length_bytes();
        }
        let key = slice_to(&hash, &n);
        let user_password = if r == PyInt::Small(2) {
            Arcfour::new(key)?.decrypt(&self.o)?
        } else {
            let mut user_password = self.o.to_vec();
            for i in (0..=19u8).rev() {
                let k = xor_key(key, i)?;
                user_password = Arcfour::new(&k)?.decrypt(&user_password)?;
            }
            user_password
        };
        self.authenticate_user_password(&user_password)
    }

    /// `self.decrypt(objid, genno, data, attrs)`, resolved on the class:
    /// decrypts a string or the data of a stream (`attrs`, its
    /// dictionary) of the object `objid`/`genno`.
    pub fn decrypt(&self, objid: &PyInt, genno: &PyInt, data: &[u8], attrs: Option<&PDFDict>) -> PyResult<Vec<u8>> {
        match self.class {
            SecurityHandlerClass::PDFStandardSecurityHandler => self.decrypt_base(objid, genno, data, attrs),
            _ => self.decrypt_v4(objid, genno, data, attrs, None),
        }
    }

    // pdfminer/pdfdocument.py:458 PDFStandardSecurityHandler.decrypt
    fn decrypt_base(&self, objid: &PyInt, genno: &PyInt, data: &[u8], _attrs: Option<&PDFDict>) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        self.decrypt_rc4(objid, genno, data)
    }

    // pdfminer/pdfdocument.py:508 PDFStandardSecurityHandlerV4.decrypt
    /// `PDFStandardSecurityHandlerV4.decrypt(objid, genno, data, attrs,
    /// name)`: `name` (default `self.strf`) picks the crypt filter.
    pub fn decrypt_v4(
        &self,
        objid: &PyInt,
        genno: &PyInt,
        data: &[u8],
        attrs: Option<&PDFDict>,
        name: Option<&str>,
    ) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        if !self.encrypt_metadata.unwrap_or(true) {
            if let Some(attrs) = attrs {
                if let Some(t) = attrs.get("Type").filter(|t| !matches!(t, PDFObject::Null)) {
                    // literal_name's own frame (psparser's literal_name
                    // takes none yet)
                    let _literal_name = recursion::py_frame()?;
                    if literal_name(&t)? == "Metadata" {
                        return Ok(data.to_vec());
                    }
                }
            }
        }
        let name = name.or(self.strf.as_deref()).unwrap_or_default();
        let Some(&method) = self.cfm.get(name) else {
            return Err(exc::KeyError(str_repr(name)));
        };
        self.call_cfm(method, objid, genno, data)
    }

    /// `self.cfm[name](objid, genno, data)`
    pub fn call_cfm(&self, method: CryptFilterMethod, objid: &PyInt, genno: &PyInt, data: &[u8]) -> PyResult<Vec<u8>> {
        match method {
            CryptFilterMethod::DecryptRc4 => self.decrypt_rc4(objid, genno, data),
            CryptFilterMethod::DecryptAes128 => self.decrypt_aes128(objid, genno, data),
            CryptFilterMethod::DecryptAes256 => self.decrypt_aes256(objid, genno, data),
            CryptFilterMethod::DecryptIdentity => self.decrypt_identity(objid, genno, data),
        }
    }

    /// `self.key` (`assert self.key is not None`).
    fn key(&self) -> PyResult<&PyBytes> {
        self.key.as_ref().ok_or_else(|| exc::AssertionError(""))
    }

    // pdfminer/pdfdocument.py:467 PDFStandardSecurityHandler.decrypt_rc4
    pub fn decrypt_rc4(&self, objid: &PyInt, genno: &PyInt, data: &[u8]) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        let mut key = self.key()?.to_vec();
        key.extend_from_slice(&pack_l(objid)?[..3]);
        key.extend_from_slice(&pack_l(genno)?[..2]);
        let hash = md5(&[&key]);
        let key = &hash[..key.len().min(16)];
        Arcfour::new(key)?.decrypt(data)
    }

    // pdfminer/pdfdocument.py:524 PDFStandardSecurityHandlerV4.decrypt_identity
    pub fn decrypt_identity(&self, _objid: &PyInt, _genno: &PyInt, data: &[u8]) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        Ok(data.to_vec())
    }

    // pdfminer/pdfdocument.py:527 PDFStandardSecurityHandlerV4.decrypt_aes128
    pub fn decrypt_aes128(&self, objid: &PyInt, genno: &PyInt, data: &[u8]) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        let mut key = self.key()?.to_vec();
        key.extend_from_slice(&pack_l(objid)?[..3]);
        key.extend_from_slice(&pack_l(genno)?[..2]);
        key.extend_from_slice(b"sAlT");
        let hash = md5(&[&key]);
        let key = &hash[..key.len().min(16)];
        let (initialization_vector, ciphertext) = data.split_at(data.len().min(16));
        let cipher = CipherAesCbc::new(key, initialization_vector, true)?;
        let plaintext = cipher.decryptor_update(ciphertext)?;
        Ok(unpad_aes(&plaintext)?.to_vec())
    }

    // pdfminer/pdfdocument.py:569 PDFStandardSecurityHandlerV5.authenticate
    fn authenticate_v5(&self, password: &str) -> PyResult<Option<PyBytes>> {
        let _frame = recursion::py_frame()?;
        let v5 = |x: &Option<PyBytes>| x.clone().expect("set by PDFStandardSecurityHandlerV5.init_params");
        let password_b = self._normalize_password(password)?;
        let hash = self._password_hash(&password_b, &v5(&self.o_validation_salt), Some(&self.u))?;
        if hash[..] == v5(&self.o_hash)[..] {
            let hash = self._password_hash(&password_b, &v5(&self.o_key_salt), Some(&self.u))?;
            let cipher = CipherAesCbc::new(&hash, &[0; 16], true)?;
            return Ok(Some(Rc::from(cipher.decryptor_update(&v5(&self.oe))?)));
        }
        let hash = self._password_hash(&password_b, &v5(&self.u_validation_salt), None)?;
        if hash[..] == v5(&self.u_hash)[..] {
            let hash = self._password_hash(&password_b, &v5(&self.u_key_salt), None)?;
            let cipher = CipherAesCbc::new(&hash, &[0; 16], true)?;
            return Ok(Some(Rc::from(cipher.decryptor_update(&v5(&self.ue))?)));
        }
        Ok(None)
    }

    // pdfminer/pdfdocument.py:591 PDFStandardSecurityHandlerV5._normalize_password
    #[allow(non_snake_case)]
    pub fn _normalize_password(&self, password: &str) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        let mut password = password.to_owned();
        if int_of(&self.r) == PyInt::Small(6) {
            // saslprep expects non-empty strings, apparently
            if password.is_empty() {
                return Ok(Vec::new());
            }
            password = saslprep(&password, true)?;
        }
        let mut bytes = password.into_bytes();
        bytes.truncate(127);
        Ok(bytes)
    }

    // pdfminer/pdfdocument.py:601 PDFStandardSecurityHandlerV5._password_hash
    /// Compute password hash depending on revision number
    #[allow(non_snake_case)]
    pub fn _password_hash(&self, password: &[u8], salt: &[u8], vector: Option<&[u8]>) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        if int_of(&self.r) == PyInt::Small(5) {
            return self._r5_password(password, salt, vector);
        }
        self._r6_password(password, &salt[..salt.len().min(8)], vector)
    }

    // pdfminer/pdfdocument.py:612 PDFStandardSecurityHandlerV5._r5_password
    /// Compute the password for revision 5
    #[allow(non_snake_case)]
    pub fn _r5_password(&self, password: &[u8], salt: &[u8], vector: Option<&[u8]>) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        let mut hash = Sha256::new();
        hash.update(password);
        hash.update(salt);
        if let Some(vector) = vector {
            hash.update(vector);
        }
        Ok(hash.finalize().to_vec())
    }

    // pdfminer/pdfdocument.py:625 PDFStandardSecurityHandlerV5._r6_password
    /// Compute the password for revision 6
    #[allow(non_snake_case)]
    pub fn _r6_password(&self, password: &[u8], salt: &[u8], vector: Option<&[u8]>) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        let mut initial_hash = Sha256::new();
        initial_hash.update(password);
        initial_hash.update(salt);
        if let Some(vector) = vector {
            initial_hash.update(vector);
        }
        let mut k: Vec<u8> = initial_hash.finalize().to_vec();
        let vector = vector.unwrap_or_default();
        let mut round_no: i64 = 0;
        let mut last_byte_val: i64 = 0;
        while round_no < 64 || last_byte_val > round_no - 32 {
            let mut unit = password.to_vec();
            unit.extend_from_slice(&k);
            unit.extend_from_slice(vector);
            let k1 = unit.repeat(64);
            let e = self._aes_cbc_encrypt(&k[..16], &k[16..32], &k1)?;
            // compute the first 16 bytes of e,
            // interpreted as an unsigned integer mod 3
            k = match Self::_bytes_mod_3(&e[..16])? {
                0 => Sha256::digest(&e).to_vec(),
                1 => Sha384::digest(&e).to_vec(),
                _ => Sha512::digest(&e).to_vec(),
            };
            last_byte_val = i64::from(e[e.len() - 1]);
            round_no += 1;
        }
        k.truncate(32);
        Ok(k)
    }

    // pdfminer/pdfdocument.py:651 PDFStandardSecurityHandlerV5._bytes_mod_3
    #[allow(non_snake_case)]
    pub fn _bytes_mod_3(input_bytes: &[u8]) -> PyResult<u32> {
        let _frame = recursion::py_frame()?;
        // 256 is 1 mod 3, so we can just sum 'em
        let _genexpr = recursion::py_frame()?;
        Ok(input_bytes.iter().map(|&b| u32::from(b) % 3).sum::<u32>() % 3)
    }

    // pdfminer/pdfdocument.py:655 PDFStandardSecurityHandlerV5._aes_cbc_encrypt
    #[allow(non_snake_case)]
    pub fn _aes_cbc_encrypt(&self, key: &[u8], iv: &[u8], data: &[u8]) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        let cipher = CipherAesCbc::new(key, iv, false)?;
        cipher.encryptor_update_finalize(data)
    }

    // pdfminer/pdfdocument.py:660 PDFStandardSecurityHandlerV5.decrypt_aes256
    pub fn decrypt_aes256(&self, _objid: &PyInt, _genno: &PyInt, data: &[u8]) -> PyResult<Vec<u8>> {
        let _frame = recursion::py_frame()?;
        let (initialization_vector, ciphertext) = data.split_at(data.len().min(16));
        let key = self.key()?;
        let cipher = CipherAesCbc::new(key, initialization_vector, true)?;
        let plaintext = cipher.decryptor_update(ciphertext)?;
        Ok(unpad_aes(&plaintext)?.to_vec())
    }
}
