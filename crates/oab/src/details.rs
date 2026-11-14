// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The unpacked OAB version 4 Full Details file ([MS-OXOAB] 2.9): `OAB_HDR`, the schema
//! (`OAB_META_DATA`), the header record and one record for each entry of the address list.

use crate::{Cap, Error, Limits, crc32};

/// `ulVersion` of a version 4 Full Details file.
const VERSION: u32 = 0x20;

/// The property types a version 4 file has ([MS-OXOAB] 2.9.4).
mod ptype {
    pub const INTEGER32: u16 = 0x0003;
    pub const BOOLEAN: u16 = 0x000B;
    pub const OBJECT: u16 = 0x000D;
    pub const STRING8: u16 = 0x001E;
    pub const STRING: u16 = 0x001F;
    pub const BINARY: u16 = 0x0102;
    pub const MULTIPLE_INTEGER32: u16 = 0x1003;
    pub const MULTIPLE_STRING8: u16 = 0x101E;
    pub const MULTIPLE_STRING: u16 = 0x101F;
    pub const MULTIPLE_BINARY: u16 = 0x1102;
}

/// A property the schema says a record can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropDef {
    /// The property tag: its id in the high 16 bits, its type in the low.
    pub tag: u32,
    /// A primary key online: every entry has it.
    pub primary_key: bool,
    /// Part of ambiguous name resolution online: what a typed name is looked up in.
    pub anr: bool,
}

impl PropDef {
    /// The property's id, without its type.
    pub fn id(&self) -> u16 {
        (self.tag >> 16) as u16
    }
}

/// A property's value ([MS-OXOAB] 2.9.6). Strings of either kind are read as UTF-8, a byte
/// that isn't replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Integer(u32),
    Boolean(bool),
    Text(String),
    Binary(Vec<u8>),
    Integers(Vec<u32>),
    Texts(Vec<String>),
    Binaries(Vec<Vec<u8>>),
}

/// One record: the header's, or an entry's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    /// The values present, by property tag, in the schema's order.
    pub values: Vec<(u32, Value)>,
}

impl Record {
    /// The value of the property with this id, of whatever type.
    pub fn get(&self, id: u16) -> Option<&Value> {
        self.values.iter().find(|(tag, _)| (tag >> 16) as u16 == id).map(|(_, v)| v)
    }

    /// The text of a string property.
    pub fn text(&self, id: u16) -> Option<&str> {
        match self.get(id)? {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    /// The texts of a multi-valued string property (none when it is absent).
    pub fn texts(&self, id: u16) -> &[String] {
        match self.get(id) {
            Some(Value::Texts(t)) => t,
            _ => &[],
        }
    }

    /// The binary values of a binary property, one or many (none when it is absent).
    pub fn binaries(&self, id: u16) -> Vec<&[u8]> {
        match self.get(id) {
            Some(Value::Binary(b)) => vec![b.as_slice()],
            Some(Value::Binaries(b)) => b.iter().map(Vec::as_slice).collect(),
            _ => Vec::new(),
        }
    }

    /// An integer property.
    pub fn integer(&self, id: u16) -> Option<u32> {
        match self.get(id)? {
            Value::Integer(n) => Some(*n),
            _ => None,
        }
    }
}

/// A whole address list as a details file has it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Details {
    /// The schema of the header record.
    pub header_schema: Vec<PropDef>,
    /// The schema of the entries.
    pub schema: Vec<PropDef>,
    /// The header record: the list's name, its DN, its sequence number and GUID
    /// ([`crate::tag::OAB_NAME`] and the others).
    pub header: Record,
    pub records: Vec<Record>,
}

impl Details {
    /// The address list's name.
    pub fn name(&self) -> Option<&str> {
        self.header.text(crate::tag::OAB_NAME)
    }

    /// The address list's sequence number: one more for each generation.
    pub fn sequence(&self) -> Option<u32> {
        self.header.integer(crate::tag::OAB_SEQUENCE)
    }
}

/// A little-endian word at `at`.
fn u32_at(b: &[u8], at: usize, what: &'static str) -> Result<u32, Error> {
    let bytes = b.get(at..at.saturating_add(4)).ok_or(Error::Truncated(what))?;
    let mut four = [0u8; 4];
    four.copy_from_slice(bytes);
    Ok(u32::from_le_bytes(four))
}

/// Reads an unpacked version 4 Full Details file, checking its CRC.
pub fn read_details(file: &[u8], limits: &Limits) -> Result<Details, Error> {
    read_details_keeping(file, limits, |_| true)
}

/// [`read_details`], keeping of the entries' values only those whose property tag `keep` wants
/// (every value is still read and checked). The header record is kept whole.
pub fn read_details_keeping(file: &[u8], limits: &Limits, keep: impl Fn(u32) -> bool) -> Result<Details, Error> {
    if u32_at(file, 0, "OAB_HDR")? != VERSION {
        return Err(Error::NotOab("OAB_HDR version"));
    }
    let serial = u32_at(file, 4, "OAB_HDR")?;
    let total = u32_at(file, 8, "OAB_HDR")? as usize;
    if total > limits.max_records {
        return Err(Error::Cap(Cap::Records));
    }
    let rest = file.get(12..).ok_or(Error::Truncated("OAB_HDR"))?;
    if crc32(rest) != serial {
        return Err(Error::BadChecksum("OAB_HDR"));
    }
    let meta_size = u32_at(rest, 0, "OAB_META_DATA")? as usize;
    let meta = rest.get(..meta_size).filter(|_| meta_size >= 4).ok_or(Error::Truncated("OAB_META_DATA"))?;
    let (header_schema, used) = prop_table(meta, 4, limits)?;
    let (schema, _) = prop_table(meta, used, limits)?;
    let mut at = meta_size;
    let mut kept = 0usize;
    let header = record(rest, &mut at, &header_schema, limits, &|_| true, &mut kept)?;
    // Each record is at least its size and its presence bits.
    let smallest = 4usize.saturating_add(schema.len().div_ceil(8));
    let mut records = Vec::with_capacity(total.min(rest.len().checked_div(smallest).unwrap_or(0)));
    for _ in 0..total {
        records.push(record(rest, &mut at, &schema, limits, &keep, &mut kept)?);
    }
    Ok(Details { header_schema, schema, header, records })
}

/// An `OAB_PROP_TABLE` at `at`, and where it ends.
fn prop_table(meta: &[u8], at: usize, limits: &Limits) -> Result<(Vec<PropDef>, usize), Error> {
    let count = u32_at(meta, at, "OAB_PROP_TABLE")? as usize;
    if count > limits.max_properties {
        return Err(Error::Cap(Cap::Properties));
    }
    let from = at.saturating_add(4);
    let bytes = meta.get(from..from.saturating_add(count.saturating_mul(8))).ok_or(Error::Truncated("OAB_PROP_TABLE"))?;
    let mut props = Vec::with_capacity(count);
    for rec in bytes.as_chunks::<8>().0 {
        let tag = u32_at(rec, 0, "OAB_PROP_REC")?;
        let flags = u32_at(rec, 4, "OAB_PROP_REC")?;
        match tag as u16 {
            ptype::INTEGER32
            | ptype::BOOLEAN
            | ptype::OBJECT
            | ptype::STRING8
            | ptype::STRING
            | ptype::BINARY
            | ptype::MULTIPLE_INTEGER32
            | ptype::MULTIPLE_STRING8
            | ptype::MULTIPLE_STRING
            | ptype::MULTIPLE_BINARY => {}
            _ => return Err(Error::Corrupt("a property type an OAB file hasn't")),
        }
        props.push(PropDef { tag, primary_key: flags & 2 != 0, anr: flags & 1 != 0 });
    }
    Ok((props, from.saturating_add(bytes.len())))
}

/// An `OAB_V4_REC` at `*at`, which is moved past it, with the values `keep` wants, counted in
/// `kept`.
fn record(rest: &[u8], at: &mut usize, schema: &[PropDef], limits: &Limits, keep: &dyn Fn(u32) -> bool, kept: &mut usize) -> Result<Record, Error> {
    let size = u32_at(rest, *at, "OAB_V4_REC")? as usize;
    if size > limits.max_record {
        return Err(Error::Cap(Cap::RecordSize));
    }
    let bytes = rest.get(*at..at.saturating_add(size)).ok_or(Error::Truncated("OAB_V4_REC"))?;
    let presence_len = schema.len().div_ceil(8);
    let presence = bytes.get(4..4usize.saturating_add(presence_len)).ok_or(Error::Truncated("OAB_V4_REC presence"))?;
    let mut data = Cursor { b: bytes, at: 4usize.saturating_add(presence_len) };
    let mut values = Vec::new();
    for (i, prop) in schema.iter().enumerate() {
        let byte = presence.get(i / 8).copied().unwrap_or(0);
        if byte & (0x80 >> (i % 8)) == 0 {
            continue;
        }
        let value = data.value(prop.tag as u16)?;
        if keep(prop.tag) {
            *kept = kept.saturating_add(1);
            if *kept > limits.max_values {
                return Err(Error::Cap(Cap::Values));
            }
            values.push((prop.tag, value));
        }
    }
    *at = at.saturating_add(size);
    Ok(Record { values })
}

/// The data of one record.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

const VALUE: Error = Error::Truncated("an OAB value");

impl Cursor<'_> {
    fn byte(&mut self) -> Result<u8, Error> {
        let b = self.b.get(self.at).copied().ok_or(VALUE)?;
        self.at = self.at.saturating_add(1);
        Ok(b)
    }

    /// What is left of the record: no count can be larger.
    fn left(&self) -> usize {
        self.b.len().saturating_sub(self.at)
    }

    /// An integer (2.9.6.1): up to 0x7F itself, else 0x81 to 0x84 and so many bytes.
    fn integer(&mut self) -> Result<u32, Error> {
        let first = self.byte()?;
        if first <= 0x7F {
            return Ok(u32::from(first));
        }
        let n = match first {
            0x81..=0x84 => first & 0x0F,
            _ => return Err(Error::Corrupt("an OAB integer")),
        };
        let mut v = 0u32;
        for i in 0..n {
            v |= u32::from(self.byte()?) << (8 * u32::from(i));
        }
        Ok(v)
    }

    /// A string ended by a zero byte (2.9.6.3, 2.9.6.4).
    fn text(&mut self) -> Result<String, Error> {
        let rest = self.b.get(self.at..).ok_or(VALUE)?;
        let len = rest.iter().position(|&b| b == 0).ok_or(VALUE)?;
        let text = String::from_utf8_lossy(rest.get(..len).ok_or(VALUE)?).into_owned();
        self.at = self.at.saturating_add(len).saturating_add(1);
        Ok(text)
    }

    /// Bytes after their count (2.9.6.5).
    fn binary(&mut self) -> Result<Vec<u8>, Error> {
        let len = self.integer()? as usize;
        let bytes = self.b.get(self.at..self.at.saturating_add(len)).ok_or(VALUE)?.to_vec();
        self.at = self.at.saturating_add(len);
        Ok(bytes)
    }

    /// The values of a multi-valued property after their count.
    fn many<T>(&mut self, mut one: impl FnMut(&mut Self) -> Result<T, Error>) -> Result<Vec<T>, Error> {
        let count = self.integer()? as usize;
        // Every value is a byte at least.
        if count > self.left() {
            return Err(VALUE);
        }
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(one(self)?);
        }
        Ok(values)
    }

    fn value(&mut self, ptype: u16) -> Result<Value, Error> {
        Ok(match ptype {
            ptype::INTEGER32 => Value::Integer(self.integer()?),
            ptype::BOOLEAN => Value::Boolean(self.byte()? != 0),
            ptype::STRING8 | ptype::STRING => Value::Text(self.text()?),
            ptype::BINARY => Value::Binary(self.binary()?),
            ptype::MULTIPLE_INTEGER32 => Value::Integers(self.many(Self::integer)?),
            ptype::MULTIPLE_STRING8 | ptype::MULTIPLE_STRING => Value::Texts(self.many(Self::text)?),
            ptype::MULTIPLE_BINARY => Value::Binaries(self.many(Self::binary)?),
            // A PtypObject is never in the file (2.9.2.2.1).
            _ => return Err(Error::Corrupt("an OAB value of a type a file never has")),
        })
    }
}
