// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The named property map ([MS-PST] 2.4.7): which name (a property set and a number or a string)
//! each property id from 0x8000 stands for in this file. Read from its entry, GUID and string
//! streams; the hash table is only for writers and is not read.

use std::collections::BTreeMap;
use std::io::{Read, Seek};

use crate::bytes::{array16, slice, u16_at, u32_at, utf16};
use crate::error::{Cap, Error, Result};
use crate::ltp::{Budget, NodeData, pc_records};
use crate::ndb::Ndb;

/// PS_MAPI ([MS-OXPROPS] 1.3.2), as stored.
pub const PS_MAPI: [u8; 16] = [0x28, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46];
/// PS_PUBLIC_STRINGS ([MS-OXPROPS] 1.3.2), as stored.
pub const PS_PUBLIC_STRINGS: [u8; 16] = [0x29, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46];

/// The ids 0x8000 to 0xFFFF.
const MAX_ENTRIES: usize = 0x8000;
const MAX_STRINGS: u64 = 16 * 1024 * 1024;

/// A named property's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropName {
    Id(u32),
    Name(String),
}

/// What one property id from 0x8000 stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedProperty {
    pub id: u16,
    /// The property set, as stored (the first three fields little-endian).
    pub guid: [u8; 16],
    pub name: PropName,
}

/// The file's named properties, by id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamedMap {
    map: BTreeMap<u16, NamedProperty>,
}

impl NamedMap {
    pub fn get(&self, id: u16) -> Option<&NamedProperty> {
        self.map.get(&id)
    }

    /// The id that stands for a name in a property set, if the file has one.
    pub fn find(&self, guid: &[u8; 16], name: &PropName) -> Option<u16> {
        self.map.values().find(|p| &p.guid == guid && &p.name == name).map(|p| p.id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &NamedProperty> {
        self.map.values()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

pub(crate) fn read<R: Read + Seek>(ndb: &mut Ndb<R>, node: &NodeData) -> Result<NamedMap> {
    let mut guids = Vec::new();
    let mut entries = Vec::new();
    let mut strings = Vec::new();
    let mut budget = Budget { left: ndb.limits.max_item_bytes };
    for (id, ty, raw) in pc_records(node)? {
        if ty != crate::prop::types::BINARY {
            continue;
        }
        let max = match id {
            0x0002 => (MAX_ENTRIES as u64).saturating_mul(16),
            0x0003 => (MAX_ENTRIES as u64).saturating_mul(8),
            0x0004 => MAX_STRINGS,
            _ => continue,
        };
        let b = node.hnid_bytes(ndb, raw, max, Cap::NamedProperties)?;
        budget.take(b.len() as u64)?;
        match id {
            0x0002 => guids = b,
            0x0003 => entries = b,
            _ => strings = b,
        }
    }
    let mut map = BTreeMap::new();
    for e in entries.as_chunks::<8>().0 {
        let value = u32_at(e, 0)?;
        let flags = u16_at(e, 4)?;
        let index = u16_at(e, 6)?;
        let is_string = flags & 1 != 0;
        let guid = match flags >> 1 {
            0 => [0u8; 16],
            1 => PS_MAPI,
            2 => PS_PUBLIC_STRINGS,
            n => array16(&guids, usize::from(n.saturating_sub(3)).saturating_mul(16))?,
        };
        let name = if is_string {
            let at = value as usize;
            let len = u32_at(&strings, at)? as usize;
            PropName::Name(utf16(slice(&strings, at.saturating_add(4), len)?))
        } else {
            PropName::Id(value)
        };
        let Some(id) = 0x8000u16.checked_add(index) else {
            return Err(Error::Cap(Cap::NamedProperties));
        };
        map.entry(id).or_insert(NamedProperty { id, guid, name });
    }
    Ok(NamedMap { map })
}
