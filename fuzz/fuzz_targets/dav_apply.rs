// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a calendar server sends is taken in without a panic and within the caps: arbitrary bytes as
//! a resource's document, and as the whole download (`sync.applyDav`, #209).
#![no_main]
use mailcraft_engine::Session;
use mailcraft_model::{DavChange, DavDownload, DavListing, DavRead, DavReadState};
use serde_json::json;

const COLLECTION: &str = "https://dav.example.org/cal/ada/work/";

fn session() -> Option<(Session, u64, String)> {
    let mut s = Session::empty();
    s.dav_enabled = true;
    // (A mailbox in a file, so that the documents kept beside it are worked too.)
    s.path = Some(std::env::temp_dir().join(format!("mc-fuzz-dav-apply-{}", std::process::id())).join("mailbox.mailcraft"));
    let id = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).ok()?["id"].as_u64()?;
    s.run("account.davSet", &json!({"id": id, "calendarHomes": ["https://dav.example.org/cal/ada/"]})).ok()?;
    // (The settings a read is made under: a download without them is refused before anything.)
    let server = s.run("sync.davRequest", &json!({})).ok()?["ask"]["server"].as_str()?.to_string();
    Some((s, id, server))
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Some((mut s, id, server)) = session() else { return };
    let text = String::from_utf8_lossy(data).into_owned();
    // The bytes as a document, under an address the bytes also name.
    let name: String = text.chars().filter(|c| c.is_ascii_alphanumeric()).take(20).collect();
    let download = DavDownload {
        account: id,
        email: "ada@example.org".into(),
        server: server.clone(),
        listing: Some(DavListing {
            calendars: vec![mailcraft_model::DavCalendar { href: COLLECTION.into(), name: text.chars().take(100).collect(), ..Default::default() }],
            complete: true,
            capped: false,
        }),
        collections: vec![DavRead {
            href: COLLECTION.into(),
            state: DavReadState::Read,
            members: data.first().is_some_and(|b| b & 1 == 1).then(|| vec![format!("{COLLECTION}{name}.ics")]),
            complete: true,
            sync_token: text.chars().take(2_000).collect(),
            changes: vec![
                DavChange { href: format!("{COLLECTION}{name}.ics"), etag: text.chars().take(50).collect(), raw: Some(text.clone()), gone: false },
                DavChange { href: text.clone(), etag: "\"x\"".into(), raw: Some(text.clone()), gone: false },
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let _ = s.run("sync.applyDav", &json!({"result": download}));
    // And the bytes as the whole parameter.
    if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(data) {
        let _ = s.run("sync.applyDav", &json!({"result": value.clone()}));
        // And again under the account's settings, so that it gets past that check.
        if let Some(object) = value.as_object_mut() {
            object.insert("server".into(), json!(server));
            object.insert("account".into(), json!(id));
            let _ = s.run("sync.applyDav", &json!({"result": value}));
        }
    }
});
