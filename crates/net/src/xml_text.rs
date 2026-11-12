// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The text of one field of a hostile XML document, as the bounded readers of this crate gather
//! it (the Favorites' configuration item, a domain's autoconfig document): piece by piece, for
//! the element that is innermost, under a cap. A field that grows past the cap, or that holds a
//! reference to anything but a character or one of XML's own five entities, is spoiled: nothing
//! is looked up anywhere for it, and the reader doesn't take it.

use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::BytesRef;

/// Add `text` to the field being read, when its element is the one `depth` deep. Past `max`
/// bytes the field is spoiled instead.
pub(crate) fn push(field: &mut Option<(String, usize)>, spoiled: &mut bool, depth: usize, text: &str, max: usize) {
    if let Some((field, _)) = field.as_mut().filter(|(_, at)| *at == depth) {
        if field.len().saturating_add(text.len()) > max {
            *spoiled = true;
        } else {
            field.push_str(text);
        }
    }
}

/// Add what `reference` stands for: a character by its number, or one of XML's own five
/// entities. Any other spoils the field.
pub(crate) fn push_reference(field: &mut Option<(String, usize)>, spoiled: &mut bool, depth: usize, reference: &BytesRef<'_>, max: usize) {
    if field.is_none() {
        return;
    }
    let mut one = [0u8; 4];
    match reference.resolve_char_ref() {
        Ok(Some(c)) => push(field, spoiled, depth, c.encode_utf8(&mut one), max),
        Ok(None) => match resolve_xml_entity(reference) {
            Some(text) => push(field, spoiled, depth, text, max),
            None => *spoiled = true,
        },
        Err(_) => *spoiled = true,
    }
}
