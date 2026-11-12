// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encapsulated properties (2.1.3.4): the counted lists of `attMsgProps` and `attAttachment`.
//!
//! Every value is read to find the next, but only what the reader asks for is kept: numbers,
//! strings and binary values of properties with a fixed id. A named property (an id of 0x8000
//! or more) is read past, and so is every multi-valued one.

use crate::Limits;
use crate::bytes::Cursor;

/// Property types ([MS-OXCDATA] 2.11.1, as 2.1.3.4 lists them).
mod ty {
    pub const INT16: u16 = 0x0002;
    pub const INT32: u16 = 0x0003;
    pub const FLT32: u16 = 0x0004;
    pub const FLT64: u16 = 0x0005;
    pub const CURRENCY: u16 = 0x0006;
    pub const APP_TIME: u16 = 0x0007;
    pub const ERROR: u16 = 0x000A;
    pub const BOOLEAN: u16 = 0x000B;
    pub const OBJECT: u16 = 0x000D;
    pub const INT64: u16 = 0x0014;
    pub const STRING8: u16 = 0x001E;
    pub const UNICODE: u16 = 0x001F;
    pub const SYSTIME: u16 = 0x0040;
    pub const CLSID: u16 = 0x0048;
    pub const BINARY: u16 = 0x0102;
    /// The flag of a multi-valued type.
    pub const MV: u16 = 0x1000;
}

/// The first id of a named property ([MS-OXPROPS] 1.3.3).
const FIRST_NAMED: u16 = 0x8000;

/// A property's value, as far as the reader needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Value<'a> {
    /// An integer or a boolean (0 or 1).
    Int(i64),
    /// An 8-bit string in the stream's code page, as written (its NUL included).
    Ansi(&'a [u8]),
    /// A UTF-16LE string, as written.
    Unicode(&'a [u8]),
    Binary(&'a [u8]),
    /// An embedded object: its interface identifier, then its data.
    Object,
    /// A value of a type nothing here reads.
    Other,
}

#[derive(Debug, Clone, Copy)]
struct Prop<'a> {
    id: u16,
    value: Value<'a>,
}

/// The properties of a message or an attachment, in the order read.
#[derive(Debug, Default)]
pub(crate) struct Props<'a> {
    list: Vec<Prop<'a>>,
    /// At most this many are kept, from however many lists ([`Limits::max_properties`]).
    room: Option<usize>,
}

impl<'a> Props<'a> {
    /// The first value of a property.
    pub(crate) fn get(&self, id: u16) -> Option<&Value<'a>> {
        self.list.iter().find(|p| p.id == id).map(|p| &p.value)
    }

    pub(crate) fn int(&self, id: u16) -> Option<i64> {
        match self.get(id)? {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// A string property as text: an 8-bit one in the character set `label` names, without
    /// its NUL and what follows.
    pub(crate) fn text(&self, id: u16, label: &str) -> Option<String> {
        let text = match self.get(id)? {
            Value::Ansi(b) => mailcraft_charset::decode(b, label),
            Value::Unicode(b) => mailcraft_charset::decode(b, "utf-16le"),
            _ => return None,
        };
        Some(match text.split_once('\0') {
            Some((before, _)) => before.to_string(),
            None => text,
        })
    }

    /// Add another list's properties after these, up to the room there is.
    pub(crate) fn merge(&mut self, other: Props<'a>) {
        let cap = self.room.or(other.room).unwrap_or(0);
        self.room = Some(cap);
        let room = cap.saturating_sub(self.list.len());
        self.list.extend(other.list.into_iter().take(room));
    }
}

/// A counted list of properties. The second part is false when the list couldn't be read to
/// its end (a type of no known size, a value past the end, more than [`Limits`] allow): what
/// was read before is kept.
pub(crate) fn read<'a>(data: &'a [u8], limits: &Limits) -> (Props<'a>, bool) {
    let mut props = Props { list: Vec::new(), room: Some(limits.max_properties) };
    let mut c = Cursor::new(data);
    let Some(count) = c.u32() else { return (props, false) };
    // Each property takes at least four bytes, so a count past what the data holds stops at its
    // end rather than going round for nothing.
    for _ in 0..count {
        let (Some(kind), Some(id)) = (c.u16(), c.u16()) else { return (props, false) };
        let named = id >= FIRST_NAMED;
        if named && name(&mut c).is_none() {
            return (props, false);
        }
        let Some(value) = value(&mut c, kind, limits) else { return (props, false) };
        if !named {
            if props.list.len() >= limits.max_properties {
                return (props, false);
            }
            props.list.push(Prop { id, value });
        }
    }
    (props, true)
}

/// A named property's name (`NamedPropSpec`): its namespace, then a number or a string.
fn name(c: &mut Cursor<'_>) -> Option<()> {
    c.take(16)?;
    match c.u32()? {
        0 => c.u32().map(drop),
        1 => {
            let length = usize::try_from(c.u32()?).ok()?;
            c.padded(length).map(drop)
        }
        _ => None,
    }
}

/// The size of a value of a fixed-size type.
fn fixed(kind: u16) -> Option<usize> {
    Some(match kind {
        ty::INT16 | ty::BOOLEAN => 2,
        ty::INT32 | ty::FLT32 | ty::ERROR => 4,
        ty::FLT64 | ty::CURRENCY | ty::APP_TIME | ty::INT64 | ty::SYSTIME => 8,
        ty::CLSID => 16,
        _ => return None,
    })
}

fn variable(kind: u16) -> bool {
    matches!(kind, ty::STRING8 | ty::UNICODE | ty::BINARY | ty::OBJECT)
}

/// One property's value, and the cursor past it. `None` for a type of no known size or a value
/// that runs past the end.
fn value<'a>(c: &mut Cursor<'a>, kind: u16, limits: &Limits) -> Option<Value<'a>> {
    if let Some(size) = fixed(kind) {
        let b = c.padded(size)?;
        return Some(match kind {
            ty::INT16 => Value::Int(i64::from(i16::from_le_bytes(b.get(..2)?.try_into().ok()?))),
            ty::BOOLEAN => Value::Int(i64::from(u16::from_le_bytes(b.get(..2)?.try_into().ok()?) != 0)),
            ty::INT32 => Value::Int(i64::from(i32::from_le_bytes(b.get(..4)?.try_into().ok()?))),
            ty::INT64 => Value::Int(i64::from_le_bytes(b.get(..8)?.try_into().ok()?)),
            _ => Value::Other,
        });
    }
    if variable(kind) {
        // One value is the rule; a writer that gives more has the first kept.
        let count = counted(c, limits)?;
        if count == 0 {
            return Some(Value::Other);
        }
        let first = sized(c)?;
        for _ in 1..count {
            sized(c)?;
        }
        return Some(match kind {
            ty::STRING8 => Value::Ansi(first),
            ty::UNICODE => Value::Unicode(first),
            ty::BINARY => Value::Binary(first),
            _ => Value::Object,
        });
    }
    if kind & ty::MV != 0 {
        let base = kind & !ty::MV;
        let count = counted(c, limits)?;
        if let Some(size) = fixed(base) {
            for _ in 0..count {
                c.padded(size)?;
            }
            return Some(Value::Other);
        }
        if variable(base) && base != ty::OBJECT {
            for _ in 0..count {
                sized(c)?;
            }
            return Some(Value::Other);
        }
    }
    None
}

/// A count of values, at most [`Limits::max_values`].
fn counted(c: &mut Cursor<'_>, limits: &Limits) -> Option<usize> {
    let count = usize::try_from(c.u32()?).ok()?;
    (count <= limits.max_values).then_some(count)
}

/// A value of a variable-length type: its size, the value, its padding.
fn sized<'a>(c: &mut Cursor<'a>) -> Option<&'a [u8]> {
    let size = usize::try_from(c.u32()?).ok()?;
    c.padded(size)
}
