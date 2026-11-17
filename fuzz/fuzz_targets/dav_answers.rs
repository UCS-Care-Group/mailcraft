// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Hostile DAV answers, addresses and statuses are bounded and never panic.
#![no_main]
use mailcraft_dav::answer::{Collections, Multistatus};
use mailcraft_dav::href::Origin;
use mailcraft_dav::http::HttpResponse;
use mailcraft_dav::request::{Endpoint, Kind};
use mailcraft_dav::status;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(origin) = Origin::parse("https://dav.example.org") else { return };
    let base = "https://dav.example.org/cal/owner/work/";
    if let Ok(m) = Multistatus::parse(data, &origin, base) {
        for e in &m.entries {
            let _ = (e.holds_events(), e.supports_sync(), e.is_gone());
        }
        let _ = Collections::from_listing(m);
    }
    let text = String::from_utf8_lossy(data);
    if let Ok(url) = origin.resolve(base, &text) {
        let _ = origin.is_within(base, &url);
        let endpoint = Endpoint::anonymous(origin.clone());
        let _ = endpoint.multiget(Kind::Calendar, base, &[&url, &text]);
        let _ = endpoint.sync_collection(base, Some(&text), Some(5));
        let _ = endpoint.put_create(Kind::AddressBook, base, &text, &text);
        let _ = endpoint.delete(base, &url, &text);
    }
    let _ = Origin::parse(&text);
    let (head, tail) = text.split_at(text.char_indices().nth(text.len() / 2).map_or(0, |(i, _)| i));
    let code = u16::try_from(data.first().copied().unwrap_or(0)).unwrap_or(0) * 4;
    let response = HttpResponse::new(code, tail.as_bytes().to_vec()).with_field("Retry-After", head).with_field("DAV", head).with_field("WWW-Authenticate", head);
    let _ = (status::classify(&response, 0), status::challenges(&response), status::is_dav_evidence(&response, &origin, base));
    let _ = (status::http_date(&text), status::retry_after(Some(&text), 0));
});
