//! Low-level reader and writer for the binary form.
//!
//! Layout rules: integers are little-endian and fixed width, except lengths and counts, which
//! are LEB128 varints of at most 5 bytes (u32). A byte string or text is `varint length` then
//! the bytes. A bool or an option flag is one byte, 0 or 1. Every length is checked against
//! the field limit and against the bytes that remain before anything is allocated.

use crate::error::DecodeError;

pub(crate) struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new(tag: u8) -> Self {
        Writer { buf: vec![tag] }
    }
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn raw(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }
    pub fn varint(&mut self, mut v: u32) {
        while v >= 0x80 {
            self.buf.push((v & 0x7f) as u8 | 0x80);
            v >>= 7;
        }
        self.buf.push(v as u8);
    }
    /// Lengths of validated fields always fit in u32 (the frame limit is 16 MiB).
    pub fn bytes(&mut self, v: &[u8]) {
        self.varint(u32::try_from(v.len()).unwrap_or(u32::MAX));
        self.buf.extend_from_slice(v);
    }
    pub fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }

    /// An optional field: a flag byte (0 or 1), then the value when present.
    pub fn option<T>(&mut self, v: Option<T>, put: impl FnOnce(&mut Self, T)) {
        match v {
            Some(v) => {
                self.u8(1);
                put(self, v);
            }
            None => self.u8(0),
        }
    }
    pub fn count(&mut self, n: usize) {
        self.varint(u32::try_from(n).unwrap_or(u32::MAX));
    }
}

pub(crate) struct Reader<'a> {
    /// The bytes not read yet.
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len()
    }

    pub fn finish(&self) -> Result<(), DecodeError> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(DecodeError::TrailingBytes(n)),
        }
    }

    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], DecodeError> {
        let (head, tail) = self.buf.split_at_checked(n).ok_or(DecodeError::Truncated(what))?;
        self.buf = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], DecodeError> {
        <[u8; N]>::try_from(self.take(N, what)?).map_err(|_| DecodeError::Truncated(what))
    }

    pub fn u8(&mut self, what: &'static str) -> Result<u8, DecodeError> {
        let [byte] = self.array(what)?;
        Ok(byte)
    }
    pub fn u16(&mut self, what: &'static str) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.array(what)?))
    }
    pub fn u32(&mut self, what: &'static str) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }
    pub fn u64(&mut self, what: &'static str) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.array(what)?))
    }
    pub fn f64(&mut self, what: &'static str) -> Result<f64, DecodeError> {
        Ok(f64::from_le_bytes(self.array(what)?))
    }
    pub fn id16(&mut self, what: &'static str) -> Result<[u8; 16], DecodeError> {
        self.array(what)
    }

    pub fn bool(&mut self, what: &'static str) -> Result<bool, DecodeError> {
        match self.u8(what)? {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(DecodeError::UnknownValue { field: what, value: u32::from(v) }),
        }
    }

    /// An optional field: reads the flag byte called `flag`, then the value when it is 1.
    pub fn option<T>(
        &mut self,
        flag: &'static str,
        get: impl FnOnce(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<Option<T>, DecodeError> {
        if self.bool(flag)? { get(self).map(Some) } else { Ok(None) }
    }

    pub fn varint(&mut self, what: &'static str) -> Result<u32, DecodeError> {
        let mut value: u32 = 0;
        for i in 0..5u32 {
            let byte = self.u8(what)?;
            let bits = u32::from(byte & 0x7f);
            if i == 4 && bits > 0x0f {
                return Err(DecodeError::BadVarint(what));
            }
            value |= bits << (7 * i);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(DecodeError::BadVarint(what))
    }

    /// Reads a length and checks it against `max` and the remaining input.
    pub fn len(&mut self, what: &'static str, max: usize) -> Result<usize, DecodeError> {
        let len = usize::try_from(self.varint(what)?).unwrap_or(usize::MAX);
        if len > max {
            return Err(DecodeError::FieldTooLong { field: what, len, max });
        }
        Ok(len)
    }

    pub fn bytes(&mut self, what: &'static str, max: usize) -> Result<Vec<u8>, DecodeError> {
        let len = self.len(what, max)?;
        Ok(self.take(len, what)?.to_vec())
    }

    pub fn string(&mut self, what: &'static str, max: usize) -> Result<String, DecodeError> {
        let len = self.len(what, max)?;
        let raw = self.take(len, what)?;
        std::str::from_utf8(raw).map(str::to_owned).map_err(|_| DecodeError::InvalidUtf8(what))
    }

    /// Reads an element count. Every element takes at least `min_elem` bytes, so a count that
    /// the remaining input cannot hold is refused before a vector is allocated.
    pub fn count(&mut self, what: &'static str, max: usize, min_elem: usize) -> Result<usize, DecodeError> {
        let n = self.len(what, max)?;
        if n.saturating_mul(min_elem) > self.remaining() {
            return Err(DecodeError::Truncated(what));
        }
        Ok(n)
    }
}
