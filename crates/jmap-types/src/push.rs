// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What push brings (RFC 8620 §7.1).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Id;

/// A StateChange object (RFC 8620 §7.1): for each account, the new state of each type that changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateChange {
    /// `StateChange`.
    #[serde(rename = "@type")]
    pub kind: String,
    /// By account id, the type names (`Email`, `Mailbox`, …) to the state a /get would now return.
    pub changed: BTreeMap<Id, BTreeMap<String, String>>,
}
