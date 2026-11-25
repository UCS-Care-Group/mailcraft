// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Texts that used to be built or stored in English (#1821): every one is in the catalogue and
//! has a translation in every language, and the default category names read in the interface
//! language.

use mailcraft_i18n::po;

const POS: [(&str, &str); 11] = [
    ("cs", include_str!("../../i18n/po/cs.po")),
    ("de", include_str!("../../i18n/po/de.po")),
    ("es", include_str!("../../i18n/po/es.po")),
    ("fr", include_str!("../../i18n/po/fr.po")),
    ("hu", include_str!("../../i18n/po/hu.po")),
    ("it", include_str!("../../i18n/po/it.po")),
    ("ka", include_str!("../../i18n/po/ka.po")),
    ("nl", include_str!("../../i18n/po/nl.po")),
    ("pl", include_str!("../../i18n/po/pl.po")),
    ("ro", include_str!("../../i18n/po/ro.po")),
    ("zh_CN", include_str!("../../i18n/po/zh_CN.po")),
];

/// Menus (reading pane, message preview, time scale), About tab, signature field, symbol,
/// category names and rule sentences.
const TEXTS: [&str; 18] = [
    "Right",
    "Bottom",
    "1 Line",
    "3 Lines",
    "5 Minutes",
    "60 Minutes",
    "Testers",
    "Organisation",
    "NBSP",
    "Yellow category",
    "Purple category",
    "When {when}: {then}",
    "from contains {words}",
    "categorize it as {category}",
    "flag it for tomorrow",
    "received from {from} to {to}",
    "show a desktop alert",
    "size is {min} KB to {max} KB",
];

#[test]
fn fixed_texts_are_in_every_catalogue() {
    for (lang, text) in POS {
        let file = po::parse(text);
        for id in TEXTS {
            let entry = file.entries.iter().find(|e| e.id == id && !e.obsolete);
            assert!(entry.is_some_and(|e| e.translated()), "{lang}: `{id}` has no translation");
        }
    }
}

#[test]
fn default_category_names_are_looked_up_and_others_kept() {
    assert_eq!(crate::i18n::category_label("Work stuff"), "Work stuff");
    assert_eq!(crate::i18n::category_label("Yellow category"), "Yellow category");
}
