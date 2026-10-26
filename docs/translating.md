# Translating MailCraft

> **Last reviewed:** 2026-10-20 · **Last updated:** 2026-10-20 · **Change:** major

MailCraft's interface comes in English and, as preview languages, French, German, Spanish and
Italian. A preview language has every text translated, but no native speaker has reviewed it yet:
File › Options names it so in its own words ("Français (aperçu)", "Deutsch (Vorschau)"). Reviewing
one, or adding another, needs no Rust.

## Where the texts are

- `crates/i18n/po/messages.pot`: the template, every text the window and the engine show. It is
  generated; never edit it by hand.
- `crates/i18n/po/<code>.po`: one catalogue per language (`fr.po`, `de.po`, `es.po`, `it.po`), in
  the gettext format any `.po` editor opens. The catalogues are compiled into the program.

`cargo xtask i18n` gathers the texts from the code, rewrites the template, merges it into every
catalogue (a new text arrives with an empty translation, a text gone from the code is dropped) and
prints how much of each language is translated. CI fails when the template is out of date, or when a
translation loses or adds a `{placeholder}`.

## Reviewing a preview language

Every entry of a preview catalogue is marked `#, fuzzy`: translated, not yet reviewed by a native
speaker. To review one, read the text in the window and in the catalogue, correct the `msgstr`
where needed, and remove the `#, fuzzy` line of each entry you have checked. When a native speaker
has reviewed a whole language, its entry in `crates/i18n/src/lib.rs` stops being a preview and the
"(preview)" goes from its name.

## Adding a language

1. Add the language to `Lang` in `crates/i18n/src/lib.rs`: its code, its name in its own words, and
   whether it is a preview; and its date conventions (24-hour or AM/PM, first day of the week, the
   order of a numeric date) in `crates/i18n/src/date.rs`.
2. Copy `messages.pot` to `<code>.po`, set `Language:` and `Plural-Forms:` in its header, and
   translate.
3. Run `cargo xtask i18n` until it reports every text translated, then `cargo xtask ci`.

## Rules for a translation

- Keep every `{placeholder}` as it is: the program fills it. Its place in the sentence may move.
- Keep `&` before a letter (a keyboard accelerator), " › " between the parts of a menu path, and
  shortcut text such as `Ctrl+Shift+R`.
- Don't translate the names of products and protocols: MailCraft, Microsoft 365, Outlook.com,
  Gmail, Graph, IMAP, SMTP.
- Use the language's own punctuation and the usual words of mail programs in it. Dates have their
  own entries (month and day names, patterns such as `{weekday} {day} {month}`), reordered as the
  language writes them.
- What goes to recipients (the lines above a quoted original, a meeting answer) should be what
  people writing in that language expect to read.
- Translate from the English yourself; don't copy another program's translation files.
- A plural has one `msgstr[n]` per form of the language's rule.
- A ribbon label with a line break (`"New\nEmail"`) is a large button's two lines: put the `\n`
  where the translation reads best, or leave it out for one short word.
- Some English words mean two things. Such an entry has a `msgctxt` that says which: `msgctxt
  "folder"` with `msgid "Archive"` is the folder, the same `msgid` without a context is the
  command. Translate each as its meaning needs.

Translations are contributed like code, by pull request, under the [Contributor Licence
Agreement](../CLA.md).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-20 | major | First version: French, German, Spanish and Italian as preview languages; reviewing and adding a language; ribbon labels' line breaks and `msgctxt` |
