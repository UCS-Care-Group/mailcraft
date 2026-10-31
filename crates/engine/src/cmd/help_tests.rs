// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Help and the account page: the project's own pages, with nothing of the owner in them.

use serde_json::json;

use super::*;

fn url(s: &mut Session, id: &str) -> String {
    s.run(id, &json!({})).unwrap()["url"].as_str().unwrap_or_default().to_string()
}

#[test]
fn help_pages_are_the_projects_own_and_name_nobody() {
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
    let repo = REPOSITORY.trim_end_matches('/');
    assert!(repo.starts_with("https://github.com/"), "{repo}");
    assert_eq!(url(&mut s, "help.help"), format!("{repo}/#readme"));
    assert_eq!(url(&mut s, "help.support"), format!("{repo}/issues"));
    assert_eq!(url(&mut s, "help.whatsNew"), format!("{repo}/releases/tag/v{VERSION}"));
    let feedback = s.run("help.feedback", &json!({})).unwrap();
    assert_eq!(feedback["dialog"], "reportProblem");
    for id in ["help.help", "help.support", "help.feedback", "help.whatsNew"] {
        let page = url(&mut s, id);
        assert!(page.starts_with(repo) && !page.contains("ada") && !page.contains('?'), "{id}: {page}");
    }
}

#[test]
fn the_account_page_says_the_version_and_the_accounts() {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64();
    let told = s.run("file.account", &json!({})).unwrap();
    assert_eq!(told["version"], VERSION);
    assert_eq!(told["accounts"][0]["id"].as_u64(), a);
    assert_eq!(told["accounts"][0]["email"], "ada@mail.example");
    assert_eq!(told["offline"], false);
}

#[test]
fn the_catalog_rows_this_fills_have_commands() {
    let s = Session::empty();
    for id in [
        "help.help",
        "help.support",
        "help.feedback",
        "help.whatsNew",
        "file.account",
        "sync.cancel",
        "sync.offline",
        "view.next7Days",
        "calendar.delete",
    ] {
        assert!(crate::catalog::features().any(|f| f.id == id), "{id} is in the catalog");
        assert!(s.registry.get(id).is_some(), "{id} is a command");
    }
}
