// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzzes every reader of an Exchange Web Services answer: the hostile XML reader, the SOAP
//! envelope and fault, the response messages, `GetFolder`'s answer, and the mail operations'
//! (`SyncFolderHierarchy`, `SyncFolderItems`, `GetItem` and its base64).
//!
//! Invariant: nothing panics, and an accepted answer can be inspected.

#![no_main]

use mailcraft_ews::{DistinguishedFolder, GetFolder, GetItemMime, HttpResponse, Reply, Shape, SyncFolderHierarchy, SyncFolderItems};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = mailcraft_ews::xml::parse(data);
    for status in [200, 500] {
        let response = HttpResponse::new(status, data.to_vec());
        if let Ok(reply) = Reply::judge(&response, 1 << 20) {
            if let Ok(messages) = reply.messages("GetFolder") {
                for message in &messages {
                    let _ = (message.class(), message.code().to_string(), message.ok().is_ok());
                }
            }
            let _ = GetFolder::new(&[DistinguishedFolder::Inbox]).answer(&reply);
            let _ = SyncFolderHierarchy::new("").answer(&reply);
            let _ = SyncFolderItems::new("AAMk", "", 200, Shape::Full).answer(&reply);
            let _ = GetItemMime::new("AAMk").answer(&reply);
            let _ = reply.busy();
        }
    }
});
