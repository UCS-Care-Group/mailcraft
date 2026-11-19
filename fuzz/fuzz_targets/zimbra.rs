// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzzes every reader of a Zimbra SOAP-JSON or REST answer: the envelope and fault, the sign-in
//! (`AuthRequest`), the folder tree (`GetFolderRequest`), the delta (`SyncRequest`), a page of
//! messages (`SearchRequest`), the tags (`GetTagRequest`), and a message's MIME.
//!
//! Invariant: nothing panics, and an accepted answer can be inspected.

#![no_main]

use mailcraft_zimbra::HttpResponse;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    for status in [200, 500] {
        let response = HttpResponse::new(status, data.to_vec());
        let _ = mailcraft_zimbra::soap::answer("AuthRequest", &response);
        let _ = mailcraft_zimbra::auth::answer(&response);
        let _ = mailcraft_zimbra::folders::answer(&response);
        for first in [false, true] {
            let _ = mailcraft_zimbra::delta::answer(&response, first);
        }
        let _ = mailcraft_zimbra::items::answer(&response);
        let _ = mailcraft_zimbra::tags::answer(&response);
        let _ = mailcraft_zimbra::fetch::answer(&response);
    }
});
