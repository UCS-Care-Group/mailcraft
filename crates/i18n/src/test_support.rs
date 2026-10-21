// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! For tests (the `test-support` feature): a catalogue read from a string stands in for a
//! language's compiled-in one on this thread, so a test can see text in a language whose
//! catalogue isn't written yet, and tests running alongside are not touched.

use std::cell::RefCell;

use crate::{Catalogue, Lang};

thread_local! {
    static STANDING_IN: RefCell<Vec<(Lang, &'static Catalogue)>> = const { RefCell::new(Vec::new()) };
}

/// Runs `f` with `po` (the text of a `.po` file) as the catalogue of `lang` on this thread.
pub fn with_catalogue<R>(lang: Lang, po: &str, f: impl FnOnce() -> R) -> R {
    // A test's catalogue is small, and lives as long as the test's process.
    let c: &'static Catalogue = Box::leak(Box::new(Catalogue::parse(po)));
    STANDING_IN.with(|s| s.borrow_mut().push((lang, c)));
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            STANDING_IN.with(|s| s.borrow_mut().pop());
        }
    }
    let _pop = Pop;
    f()
}

/// The catalogue standing in for `lang` on this thread, if any.
pub(crate) fn catalogue(lang: Lang) -> Option<&'static Catalogue> {
    STANDING_IN.with(|s| s.borrow().iter().rev().find(|(l, _)| *l == lang).map(|(_, c)| *c))
}

/// A `.po` file's text from pairs of English and translation (the French plural rule).
pub fn po(pairs: &[(&str, &str)]) -> String {
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    let mut out = String::from("msgid \"\"\nmsgstr \"Plural-Forms: nplurals=2; plural=(n > 1);\\n\"\n");
    for (id, s) in pairs {
        out.push_str(&format!("\nmsgid \"{}\"\nmsgstr \"{}\"\n", quote(id), quote(s)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stands_in_on_this_thread_only() {
        let text = po(&[("Reply", "Répondre"), ("Say \"hi\"", "Dites « salut »")]);
        with_catalogue(Lang::French, &text, || {
            assert_eq!(Lang::French.t("Reply"), "Répondre");
            assert_eq!(Lang::French.t("Say \"hi\""), "Dites « salut »");
            std::thread::spawn(|| {
                assert_eq!(Lang::French.t("Reply"), crate::catalogue(Lang::French).and_then(|c| c.get("Reply")).unwrap_or("Reply"))
            })
            .join()
            .unwrap();
        });
        assert!(catalogue(Lang::French).is_none());
    }
}
