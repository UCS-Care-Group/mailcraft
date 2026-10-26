// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft names its own features in plain, generic words (#521). Another company's product
//! name, or a name it coined for one of its features, appears in an interface label only to say
//! that MailCraft connects to that product, and each such label is listed in [`ALLOWED`] with
//! its reason. Account setup and help text, which name Microsoft's products to say which account
//! is meant, are not labels and are not walked here.

use mailcraft_engine::Session;
use serde_json::{Value, json};

use crate::{MailApp, Services, ribbon};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// Names that are not ours to give a feature of our own. Matched case-insensitively.
const FORBIDDEN: &[&str] = &[
    "Outlook",
    "Office",
    "SmartArt",
    "WordArt",
    "Quick Part",
    "Quick Step",
    "Immersive Reader",
    "Scheduling Assistant",
    "Copilot",
    "OneNote",
    "OneDrive",
    "SharePoint",
];

/// Labels that may contain one of those names, each with why: they say what MailCraft connects
/// to, or the word is not a product's name there. The label must match exactly.
const ALLOWED: &[(&str, &str)] = &[
    ("Favorites from Outlook", "reads the Favorites the owner keeps in Outlook on the web, through Microsoft Graph"),
    ("Outlook's Favorites", "the same Favorites, read from Outlook on the web: the folder pane's switch for them"),
    ("Show Outlook's Favorites", "turns on reading the Favorites kept in Outlook on the web"),
];

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app
}

/// What brings up every tab of every module, and the tab a message being written has.
fn every_tab() -> Vec<(String, Vec<(&'static str, Value)>)> {
    let mut out = Vec::new();
    for module in ["view.mail", "view.calendar", "view.people", "view.tasks"] {
        for tab in ["Home", "Send / Receive", "Folder", "View", "Help"] {
            out.push((format!("{module} › {tab}"), vec![(module, json!({})), ("ui.tab", json!({"tab": tab}))]));
        }
    }
    out.push(("Message".to_string(), vec![("view.mail", json!({})), ("mail.new", json!({}))]));
    out
}

/// Every label the guard looks at, with where it was found.
fn labels() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for f in mailcraft_engine::catalog::features() {
        out.push((format!("catalog tab of {}", f.id), f.tab.to_string()));
        out.push((format!("catalog group of {}", f.id), f.group.to_string()));
        out.push((format!("catalog feature {}", f.id), f.name.to_string()));
    }
    let mut a = app();
    for spec in a.session.registry.all() {
        out.push((format!("command {}", spec.id), spec.label.to_string()));
        out.push((format!("location of command {}", spec.id), spec.location.to_string()));
    }
    for (id, label, location, _) in crate::UI_COMMANDS {
        out.push((format!("command {id}"), (*label).to_string()));
        out.push((format!("location of command {id}"), (*location).to_string()));
    }
    for (name, commands) in every_tab() {
        for (command, params) in commands {
            let _ = a.run(command, params);
        }
        for tab in ribbon::tabs(&a) {
            out.push((format!("ribbon tab ({name})"), tab.to_string()));
        }
        let tab = ribbon::describe(&a);
        out.push((format!("ribbon note ({name})"), tab.note.to_string()));
        for group in &tab.groups {
            out.push((format!("ribbon group ({name})"), group.title.to_string()));
            for item in &group.items {
                out.push((format!("ribbon {} ({name})", item.key), item.label.replace('\n', " ")));
                out.push((format!("ribbon tip of {} ({name})", item.key), item.tip.to_string()));
            }
        }
    }
    out
}

/// The labels in `labels` that use a forbidden name and are not allowed, as `where: "label"`.
fn offenders(labels: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (place, label) in labels {
        let lower = label.to_lowercase();
        let hit = FORBIDDEN.iter().find(|name| lower.contains(&name.to_lowercase()));
        if let Some(name) = hit
            && !ALLOWED.iter().any(|(allowed, _)| allowed == label)
        {
            out.push(format!("{place}: \"{label}\" ({name})"));
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn no_label_uses_another_products_name() {
    let found = offenders(&labels());
    assert!(found.is_empty(), "labels that use another company's name (rename them, or add them to ALLOWED with a reason): {found:#?}");
}

/// The guard does catch one: a planted label, in any case, fails it, and an exception covers
/// its own label and nothing more.
#[test]
fn a_planted_label_is_caught() {
    let mut all = labels();
    all.push(("planted".to_string(), "Insert SMARTART".to_string()));
    all.push(("planted".to_string(), "quick steps".to_string()));
    all.push(("planted".to_string(), "Favorites from Outlook!".to_string()));
    let found = offenders(&all);
    assert_eq!(
        found,
        vec![
            "planted: \"Favorites from Outlook!\" (Outlook)".to_string(),
            "planted: \"Insert SMARTART\" (SmartArt)".to_string(),
            "planted: \"quick steps\" (Quick Step)".to_string()
        ]
    );
}

/// Every allowed label is still there: an exception that nothing needs is taken out.
#[test]
fn every_exception_is_used() {
    let all = labels();
    for (allowed, reason) in ALLOWED {
        assert!(!reason.is_empty());
        assert!(all.iter().any(|(_, label)| label == allowed), "\"{allowed}\" is no longer a label: take it out of ALLOWED");
    }
}
