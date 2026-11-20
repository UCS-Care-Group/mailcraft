// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Full import-then-repair contract for the review's bounded HTML classes. Synthetic data only.
use mailcraft_engine::{Session, sample};
use mailcraft_html::compat::{Severity, check};
use serde_json::json;

#[test]
fn imported_review_shapes_open_and_can_be_repaired_without_an_unseen_save_refusal() {
    let now = 1_791_993_600;
    let (mut imported, mut rejected, mut saved, mut repaired) = (0, 0, 0, 0);
    let mut shapes = vec![
        "<p>Ordinary signature</p>".to_string(),
        "<p>BEFORE</p><template><body class='inert'><p>INERT</p></template><p>AFTER</p>".to_string(),
        "<body class='active'><p>BEFORE</p><template><template><body class='inert'><p>INERT</p></template></template><p>AFTER</p></body>".to_string(),
        "<style>@font-face{font-family:Example}</style><p>Owner repairs this source</p>".to_string(),
        "<p style='--brand:red;color:var(--brand,/* ) */blue)'>Owner repairs this source</p>".to_string(),
        "<template><img src='missing.png'></template><p>Visible words</p>".to_string(),
        "<svg><template></svg><img src='missing.png'><p>Visible words</p>".to_string(),
    ];
    for spaces in [1, 483, 484, 485, 507, 1024] {
        let query = format!("screen{}and (min-width: 600px)", " ".repeat(spaces));
        shapes.push(format!("<style media=\"{query}\">p{{color:red}}</style><p>Visible words</p>"));
        shapes.push(format!("<style>@media {query}{{p{{color:red}}}}</style><p>Visible words</p>"));
    }
    for n in [0, 31, 32, 40] {
        let attrs: String = (0..n).map(|i| format!(" data-x{i}='' ")).collect();
        for kind in ["text/css", "text/plain"] {
            shapes.push(format!("<style media='screen'{attrs} type='{kind}' type='text/css'>p{{color:red}}</style><p>Visible words</p>"));
        }
    }
    for (n, html) in shapes.iter().enumerate() {
        let mut s = Session::at(sample::mailbox(now, 0), now, 0);
        let before = s.mailbox.clone();
        let name = format!("Review shape {n}");
        match s.run("signature.import", &json!({"name": name, "html": html, "assets": []})) {
            Err(e) => {
                rejected += 1;
                assert!(s.mailbox == before, "refusal changed the mailbox: {html}");
                assert!(html.contains("missing.png"), "non-picture import refused: {html}: {e}");
                continue;
            }
            Ok(_) => imported += 1,
        }
        s.run("signature.open", &json!({"name": name})).unwrap();
        let editor = s.signature_edit.as_ref().unwrap();
        let source = editor.signature.html().unwrap();
        assert_eq!(source, html, "source editor lost imported text");
        let errors = check(source).unwrap().iter().any(|f| f.severity == Severity::Error);
        match s.run("signature.save", &json!({})) {
            Ok(_) => {
                saved += 1;
                assert!(!errors, "Save accepted compatibility errors");
            }
            Err(e) => {
                repaired += 1;
                assert!(errors, "Save refused without a visible compatibility error: {html}: {e}");
                assert_eq!(e.to_string(), "this signature has email compatibility errors to fix before it can be saved");
            }
        }
        s.run("signature.html", &json!({"html": "<p>Repaired signature</p>"})).unwrap();
        s.run("signature.save", &json!({})).unwrap();
        s.run("signature.close", &json!({})).unwrap();
        assert_eq!(s.mailbox.signature(&name).unwrap().html(), Some("<p>Repaired signature</p>"));
    }
    println!("IMPORT_REPAIR cases={} imported={imported} rejected={rejected} saved={saved} repaired={repaired}", shapes.len());
    assert!(imported >= 20 && rejected == 2 && saved >= 5 && repaired >= 5);
}
