// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A text with values in it, kept apart until it is said: an English template with `{name}`
//! placeholders and the values that fill them. The engine says what went wrong this way, so
//! that the window can say it in the interface language (the template is looked up in the
//! catalogue, then filled), while the CLI and MCP get the English ([`Display`]).

use std::fmt;

use crate::Lang;

/// An English template and its values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    pub template: &'static str,
    pub args: Vec<(&'static str, Arg)>,
}

/// A value in a [`Text`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    /// Said as it is: a name, a number, an address.
    Plain(String),
    /// Interface text of its own (a folder's default name, a fixed reason), said in the language too.
    Words(&'static str),
    /// A standard folder's default name ("Archive"), said as the language names that folder
    /// ([`Lang::folder`]), which may differ from the same English word elsewhere.
    Folder(&'static str),
}

impl Arg {
    /// Interface text, said in the language like the template.
    pub fn words(s: &'static str) -> Arg {
        Arg::Words(s)
    }

    /// A standard folder's default name, said as the language names the folder.
    pub fn folder(s: &'static str) -> Arg {
        Arg::Folder(s)
    }
}

/// What a [`Text`] can take as a value: anything that can be written ([`Arg::Plain`]), or an
/// [`Arg`] as it is.
pub trait ToArg {
    fn to_arg(&self) -> Arg;
}

impl<T: fmt::Display + ?Sized> ToArg for T {
    fn to_arg(&self) -> Arg {
        Arg::Plain(self.to_string())
    }
}

impl ToArg for Arg {
    fn to_arg(&self) -> Arg {
        self.clone()
    }
}

impl Text {
    pub fn new(template: &'static str, args: Vec<(&'static str, Arg)>) -> Text {
        Text { template, args }
    }

    /// The text in `lang`: its template from the catalogue (English when it has none), filled.
    pub fn in_lang(&self, lang: Lang) -> String {
        let values: Vec<(&str, &str)> = self
            .args
            .iter()
            .map(|(k, a)| match a {
                Arg::Plain(s) => (*k, s.as_str()),
                Arg::Words(w) => (*k, lang.t(w)),
                Arg::Folder(w) => (*k, lang.folder(w)),
            })
            .collect();
        crate::fmt(lang.t(self.template), &values)
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.in_lang(Lang::English))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_for_display_and_the_language_for_the_window() {
        let t = Text::new("no {folder} folder in {name}", vec![("folder", Arg::words("Sent Items")), ("name", "Work {folder}".to_arg())]);
        assert_eq!(t.to_string(), "no Sent Items folder in Work {folder}");
        assert_eq!(t.in_lang(Lang::Pseudo), "nó Sént Ítéms fóldér ín Work {folder}");
        assert_eq!(Text::new("{n} of {total}", vec![("n", 3u32.to_arg())]).to_string(), "3 of {total}");
    }
}
