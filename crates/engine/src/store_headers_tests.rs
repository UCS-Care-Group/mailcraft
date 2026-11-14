// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's header lines in two parts (HS): what is read by name stays on the message, the
//! rest is its header block, and the mailbox file and View Source have every line, in order.

use super::*;

/// An Exchange-like header, invented: trace and reports among what is read by name.
pub(super) fn exchange_headers(seed: usize) -> Vec<(String, String)> {
    let filler = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo".repeat(40);
    vec![
        ("Received".into(), format!("from a{seed}.prod.example.org by b{seed}.prod.example.org with HTTPS; Mon, 1 Sep 2025 09:00:00 +0000")),
        ("Authentication-Results".into(), "mx.microsoft.com 1; spf=pass smtp.mailfrom=example.org; dmarc=pass header.from=example.org".into()),
        ("Received".into(), "from mail.example.org (192.0.2.1) by DB1PEPF00000001.mail.protection.outlook.com (10.167.242.1)".into()),
        ("X-MS-Exchange-Organization-AuthAs".into(), "Anonymous".into()),
        ("X-Microsoft-Antispam-Message-Info".into(), filler.clone()),
        ("DKIM-Signature".into(), format!("v=1; a=rsa-sha256; d=example.org; s=s1; b={filler}")),
        ("List-Id".into(), "<news.example.org>".into()),
        ("X-MS-Exchange-AntiSpam-MessageData-0".into(), filler),
        ("Return-Path".into(), "<bounce@example.org>".into()),
    ]
}

/// A mailbox whose messages carry Exchange-like headers, split as a message read is.
pub(super) fn with_exchange_headers(messages: usize) -> Mailbox {
    let mut mb = mailbox(messages);
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    for (n, id) in ids.into_iter().enumerate() {
        let mut m = Message::clone(mb.message(id).unwrap());
        m.headers = exchange_headers(n);
        m.split_headers();
        mb.messages.insert(id, Arc::new(m));
    }
    mb
}

#[test]
fn every_header_line_is_saved_and_comes_back_in_its_place() {
    let mb = with_exchange_headers(6);
    let id = *mb.messages.keys().next().unwrap();
    let m = mb.message(id).unwrap();
    assert!(m.headers.len() < exchange_headers(0).len() && !m.header_block.none_in_memory());
    let tmp = Temp::new("headers-round-trip");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    let back = whole(&path);
    for (id, m) in &mb.messages {
        let read = back.message(*id).unwrap();
        assert_eq!(read.all_headers(), m.all_headers(), "message {id}");
        assert_eq!(read.headers, m.headers);
    }
}
