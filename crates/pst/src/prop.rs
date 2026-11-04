// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Property values: the types of [MS-OXCDATA] 2.11.1 as a .pst file stores them, single and
//! multi-valued ([MS-PST] 2.3.3.4).

use crate::bytes::{slice, u16_at, u32_at, u64_at, utf16};
use crate::error::{Error, Result};

/// Property types ([MS-OXCDATA] 2.11.1).
pub mod types {
    pub const NULL: u16 = 0x0001;
    pub const INT16: u16 = 0x0002;
    pub const INT32: u16 = 0x0003;
    pub const FLOAT32: u16 = 0x0004;
    pub const FLOAT64: u16 = 0x0005;
    pub const CURRENCY: u16 = 0x0006;
    pub const APP_TIME: u16 = 0x0007;
    pub const ERROR: u16 = 0x000A;
    pub const BOOLEAN: u16 = 0x000B;
    pub const OBJECT: u16 = 0x000D;
    pub const INT64: u16 = 0x0014;
    pub const STRING8: u16 = 0x001E;
    pub const STRING: u16 = 0x001F;
    pub const TIME: u16 = 0x0040;
    pub const GUID: u16 = 0x0048;
    pub const SERVER_ID: u16 = 0x00FB;
    pub const RESTRICTION: u16 = 0x00FD;
    pub const RULE_ACTION: u16 = 0x00FE;
    pub const BINARY: u16 = 0x0102;
    /// Set on a type for its multi-valued form.
    pub const MULTIPLE: u16 = 0x1000;
}

use types::*;

/// A property's value.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PropValue {
    Null,
    Int16(i16),
    Int32(i32),
    Float32(f32),
    Float64(f64),
    /// Currency, in ten-thousandths.
    Currency(i64),
    /// An OLE automation date.
    AppTime(f64),
    Error(u32),
    Bool(bool),
    Int64(i64),
    /// A FILETIME: 100-nanosecond intervals since 1601-01-01 UTC.
    Time(u64),
    Guid([u8; 16]),
    String(String),
    /// Text in the item's code page, as stored (ANSI files, and some properties of Unicode ones).
    String8(Vec<u8>),
    Binary(Vec<u8>),
    /// An object in a subnode (an attached message's data): its subnode id and size.
    Object {
        nid: u32,
        size: u32,
    },
    MultiInt16(Vec<i16>),
    MultiInt32(Vec<i32>),
    MultiFloat32(Vec<f32>),
    MultiFloat64(Vec<f64>),
    MultiCurrency(Vec<i64>),
    MultiAppTime(Vec<f64>),
    MultiInt64(Vec<i64>),
    MultiTime(Vec<u64>),
    MultiGuid(Vec<[u8; 16]>),
    MultiString(Vec<String>),
    MultiString8(Vec<Vec<u8>>),
    MultiBinary(Vec<Vec<u8>>),
    /// Larger than [`crate::Limits::max_inline_value`]: not read with the rest; read it with
    /// [`crate::Pst::read_property`].
    Large {
        size: u64,
    },
    /// A type this reader doesn't know, as stored.
    Unknown {
        ty: u16,
        data: Vec<u8>,
    },
    /// A value that could not be read, and why: kept in its place so that it is accounted for,
    /// and named in the `left_out` of whatever holds it.
    Damaged {
        ty: u16,
        reason: crate::Error,
    },
}

/// One property of an object.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub id: u16,
    pub ty: u16,
    pub value: PropValue,
}

impl Property {
    /// The 32-bit property tag (id in the high half, type in the low).
    pub fn tag(&self) -> u32 {
        (u32::from(self.id) << 16) | u32::from(self.ty)
    }
}

/// An object's properties, in the order of their ids.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Properties(pub Vec<Property>);

impl Properties {
    pub fn get(&self, id: u16) -> Option<&PropValue> {
        self.0.iter().find(|p| p.id == id).map(|p| &p.value)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Property> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A text property: Unicode, or 8-bit text read as Latin-1. Latin-1 is right only for
    /// part of the code pages an ANSI file may use; the import decodes [`PropValue::String8`]
    /// with the item's own code page (`PidTagMessageCodepage`, `PidTagInternetCodepage`)
    /// instead of calling this.
    pub fn text(&self, id: u16) -> Option<String> {
        match self.get(id)? {
            PropValue::String(s) => Some(s.clone()),
            PropValue::String8(b) => Some(b.iter().map(|&c| char::from(c)).collect()),
            _ => None,
        }
    }

    /// The properties that could not be read ([`PropValue::Damaged`]): id and reason.
    pub fn damaged(&self) -> impl Iterator<Item = (u16, &crate::Error)> {
        self.0.iter().filter_map(|p| match &p.value {
            PropValue::Damaged { reason, .. } => Some((p.id, reason)),
            _ => None,
        })
    }

    pub fn int(&self, id: u16) -> Option<i64> {
        match self.get(id)? {
            PropValue::Int16(v) => Some(i64::from(*v)),
            PropValue::Int32(v) => Some(i64::from(*v)),
            PropValue::Int64(v) => Some(*v),
            PropValue::Error(v) => Some(i64::from(*v)),
            _ => None,
        }
    }

    pub fn bool(&self, id: u16) -> Option<bool> {
        match self.get(id)? {
            PropValue::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn binary(&self, id: u16) -> Option<&[u8]> {
        match self.get(id)? {
            PropValue::Binary(v) => Some(v),
            _ => None,
        }
    }

    pub fn time(&self, id: u16) -> Option<u64> {
        match self.get(id)? {
            PropValue::Time(v) => Some(*v),
            _ => None,
        }
    }
}

/// The size of a fixed-size type, `None` for a variable-size one.
pub(crate) fn fixed_size(ty: u16) -> Option<usize> {
    match ty {
        NULL => Some(0),
        BOOLEAN => Some(1),
        INT16 => Some(2),
        INT32 | FLOAT32 | ERROR => Some(4),
        FLOAT64 | CURRENCY | APP_TIME | INT64 | TIME => Some(8),
        GUID => Some(16),
        _ => None,
    }
}

/// A fixed-size value from exactly its bytes (or more: the first are read).
pub(crate) fn fixed(ty: u16, b: &[u8]) -> Result<PropValue> {
    Ok(match ty {
        NULL => PropValue::Null,
        BOOLEAN => PropValue::Bool(b.first().copied().ok_or(Error::Corrupt("a short value"))? != 0),
        INT16 => PropValue::Int16(u16_at(b, 0)? as i16),
        INT32 => PropValue::Int32(u32_at(b, 0)? as i32),
        FLOAT32 => PropValue::Float32(f32::from_bits(u32_at(b, 0)?)),
        ERROR => PropValue::Error(u32_at(b, 0)?),
        FLOAT64 => PropValue::Float64(f64::from_bits(u64_at(b, 0)?)),
        CURRENCY => PropValue::Currency(u64_at(b, 0)? as i64),
        APP_TIME => PropValue::AppTime(f64::from_bits(u64_at(b, 0)?)),
        INT64 => PropValue::Int64(u64_at(b, 0)? as i64),
        TIME => PropValue::Time(u64_at(b, 0)?),
        GUID => PropValue::Guid(crate::bytes::array16(b, 0)?),
        other => PropValue::Unknown { ty: other, data: b.to_vec() },
    })
}

/// A value stored apart (in the heap or a subnode), from all its bytes.
pub(crate) fn stored(ty: u16, b: &[u8]) -> Result<PropValue> {
    if ty & MULTIPLE != 0 {
        return multi(ty & !MULTIPLE, b);
    }
    Ok(match ty {
        STRING => PropValue::String(utf16(b)),
        STRING8 => PropValue::String8(trim_nul(b).to_vec()),
        BINARY | SERVER_ID | RESTRICTION | RULE_ACTION => PropValue::Binary(b.to_vec()),
        OBJECT => PropValue::Object { nid: u32_at(b, 0)?, size: u32_at(b, 4)? },
        t if fixed_size(t).is_some() => fixed(t, b)?,
        other => PropValue::Unknown { ty: other, data: b.to_vec() },
    })
}

fn trim_nul(b: &[u8]) -> &[u8] {
    let mut end = b.len();
    while end > 0 && b.get(end.saturating_sub(1)) == Some(&0) {
        end = end.saturating_sub(1);
    }
    b.get(..end).unwrap_or(b)
}

fn multi(base: u16, b: &[u8]) -> Result<PropValue> {
    if let Some(size) = fixed_size(base).filter(|&s| s > 1) {
        if !b.len().is_multiple_of(size) {
            return Err(Error::Corrupt("a multi-valued property's size"));
        }
        let values: Vec<PropValue> = b.chunks_exact(size).map(|c| fixed(base, c)).collect::<Result<_>>()?;
        return Ok(match base {
            INT16 => PropValue::MultiInt16(values.iter().filter_map(|v| if let PropValue::Int16(x) = v { Some(*x) } else { None }).collect()),
            INT32 => PropValue::MultiInt32(values.iter().filter_map(|v| if let PropValue::Int32(x) = v { Some(*x) } else { None }).collect()),
            FLOAT32 => PropValue::MultiFloat32(values.iter().filter_map(|v| if let PropValue::Float32(x) = v { Some(*x) } else { None }).collect()),
            FLOAT64 => PropValue::MultiFloat64(values.iter().filter_map(|v| if let PropValue::Float64(x) = v { Some(*x) } else { None }).collect()),
            CURRENCY => {
                PropValue::MultiCurrency(values.iter().filter_map(|v| if let PropValue::Currency(x) = v { Some(*x) } else { None }).collect())
            }
            APP_TIME => PropValue::MultiAppTime(values.iter().filter_map(|v| if let PropValue::AppTime(x) = v { Some(*x) } else { None }).collect()),
            INT64 => PropValue::MultiInt64(values.iter().filter_map(|v| if let PropValue::Int64(x) = v { Some(*x) } else { None }).collect()),
            TIME => PropValue::MultiTime(values.iter().filter_map(|v| if let PropValue::Time(x) = v { Some(*x) } else { None }).collect()),
            GUID => PropValue::MultiGuid(values.iter().filter_map(|v| if let PropValue::Guid(x) = v { Some(*x) } else { None }).collect()),
            _ => PropValue::Unknown { ty: base | MULTIPLE, data: b.to_vec() },
        });
    }
    let items = variable_items(b)?;
    Ok(match base {
        STRING => PropValue::MultiString(items.into_iter().map(utf16).collect()),
        STRING8 => PropValue::MultiString8(items.into_iter().map(|i| trim_nul(i).to_vec()).collect()),
        BINARY => PropValue::MultiBinary(items.into_iter().map(<[u8]>::to_vec).collect()),
        _ => PropValue::Unknown { ty: base | MULTIPLE, data: b.to_vec() },
    })
}

/// The items of a multi-valued property of a variable-size type (2.3.3.4.2).
fn variable_items(b: &[u8]) -> Result<Vec<&[u8]>> {
    if b.is_empty() {
        return Ok(Vec::new());
    }
    let count = u32_at(b, 0)? as usize;
    // Each item has its 4-byte offset, so the count can't pass a quarter of the size.
    if count > b.len() / 4 {
        return Err(Error::Corrupt("a multi-valued property's count"));
    }
    let table_end = count.saturating_mul(4).saturating_add(4);
    let mut offsets = Vec::with_capacity(count);
    for i in 0..count {
        offsets.push(u32_at(b, i.saturating_mul(4).saturating_add(4))? as usize);
    }
    let mut items = Vec::with_capacity(count);
    for (i, &start) in offsets.iter().enumerate() {
        let end = offsets.get(i.saturating_add(1)).copied().unwrap_or(b.len());
        if start < table_end || end < start || end > b.len() {
            return Err(Error::Corrupt("a multi-valued property's offsets"));
        }
        items.push(slice(b, start, end.saturating_sub(start))?);
    }
    Ok(items)
}
