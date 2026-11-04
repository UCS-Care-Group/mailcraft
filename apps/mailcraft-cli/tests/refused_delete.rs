// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #793 through a host's own start-up rather than `file.open`: a mailbox in which an earlier
//! version put a message back empty after a refused delete for good is opened by the CLI, as the
//! desktop window and the browser open theirs. The message is marked to be fetched again before
//! any command runs, so a move to another account is refused instead of making an empty copy
//! there while the server deletes the original. Every message is invented; nothing leaves this
//! machine.
#![cfg(not(target_arch = "wasm32"))]

use std::process::Command;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::apply_graph_mail;
use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, ServerMessage};
use mailcraft_model::{Attachment, FolderKind, Id, Message};
use serde_json::json;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mailcraft-cli"))
}

/// A Microsoft account whose server lists an Inbox ("in").
fn microsoft(s: &mut Session, email: &str) -> Id {
    let account = s.run("account.add", &json!({"email": email, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let folders = vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let listing = GraphMailResult { account, email: email.into(), listing: Some(FolderListing { folders, complete: true }), ..Default::default() };
    apply_graph_mail(s, listing).unwrap();
    account
}

#[test]
fn a_message_put_back_empty_is_fetched_again_when_the_cli_opens_the_mailbox_and_moves_nowhere() {
    let dir = std::env::temp_dir().join(format!("mailcraft-793-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mailbox.mailcraft");
    let mut s = Session::empty();
    let ms = microsoft(&mut s, "owner@example.org");
    let other = microsoft(&mut s, "owner@outlook.example");
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    // As an earlier version put it back: no text, and a file of no bytes.
    let server = ServerMessage { id: "AAkA1".into(), folder: "in".into(), size: 12_000, ..Default::default() };
    let mut m = Message { folder: inbox, subject: "Invoice 42".into(), server: Some(server), ..Default::default() };
    m.attachments.push(Attachment::new("plan.txt", "text/plain", Vec::new()));
    let id = s.mailbox.add_message(m).unwrap();
    s.run("file.save", &json!({"path": path})).unwrap();
    drop(s);
    mailcraft_engine::store::clear_cache();

    // The move is refused: the message is to be fetched again first.
    let path_text = path.to_str().unwrap();
    let moved = cli().args(["run", "--file", path_text, "--cmd", &format!(r#"mail.move={{"ids":[{id}],"folder":{theirs}}}"#)]).output().unwrap();
    let stderr = String::from_utf8_lossy(&moved.stderr);
    assert!(!moved.status.success(), "moved: {stderr}");
    assert!(stderr.contains("\"Invoice 42\" isn't downloaded yet"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    // And it is saved so.
    let out = dir.join("out.mailcraft");
    let saved = cli().args(["run", "--file", path_text, "--save", out.to_str().unwrap()]).output().unwrap();
    assert!(saved.status.success(), "{}", String::from_utf8_lossy(&saved.stderr));
    mailcraft_engine::store::clear_cache();
    let opened = mailcraft_engine::store::open(&out).unwrap();
    let m = opened.mailbox.message(id).unwrap();
    assert!(m.unfetched && m.has_attachments() && m.account == ms, "{m:?}");
    let _ = std::fs::remove_dir_all(dir);
}
