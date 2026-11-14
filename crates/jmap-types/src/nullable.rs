// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An argument for which null is a value of its own, distinct from leaving it out (a filter on
//! `parentId: null`, for instance, which matches the mailboxes at the top level): `None` is left out,
//! `Some(None)` is written as null. Used as `#[serde(default, skip_serializing_if = "Option::is_none",
//! with = "crate::nullable")]`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Writes the inner `Option`: null for `Some(None)`. (`None` is skipped by the field's attribute.)
pub fn serialize<S: Serializer, T: Serialize>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(inner) => inner.serialize(serializer),
        None => serializer.serialize_none(),
    }
}

/// Reads a present member, null included, as `Some`. (A member left out is `None` by `default`.)
pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}
