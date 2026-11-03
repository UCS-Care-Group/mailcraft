// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn unknown_permissions_never_imply_write_access() {
    let c: Collection = serde_json::from_str(r#"{"account":7,"href":"/calendar/"}"#).unwrap();
    assert!(c.read_only);
    assert_eq!(c.account, 7);
}

#[test]
fn a_snapshot_round_trips_the_opaque_document_and_etag() {
    let r =
        Resource { account: 7, href: "/calendar/item.ics".into(), etag: "\"opaque\"".into(), raw: "X-PRIVATE:kept\r\n".into(), ..Default::default() };
    let encoded = serde_json::to_string(&r).unwrap();
    let got: Resource = serde_json::from_str(&encoded).unwrap();
    assert_eq!(got.raw, r.raw);
    assert_eq!(got.etag, r.etag);
    assert_eq!(got.account, r.account);
    assert!(!encoded.contains("password"));
}
