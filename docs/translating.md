# Translating MailCraft

> **Last reviewed:** 2026-10-29 · **Last updated:** 2026-11-10 · **Change:** minor

MailCraft's interface comes in English and, as preview languages, French, German, Spanish,
Italian, Dutch, Romanian and Simplified Chinese. Preview translations may be marked for further review; the initial Simplified Chinese catalogue was reviewed by its contributor.
Simplified Chinese is offered on desktop only until the browser build has a CJK font.
File › Options names each preview in its own words ("Français (aperçu)", "Deutsch (Vorschau)"). Reviewing
one, or adding another, needs no Rust.

## Where the texts are

- `crates/i18n/po/messages.pot`: the template, every text the window and the engine show. It is
  generated; never edit it by hand.
- `crates/i18n/po/<code>.po`: one catalogue per language (`fr.po`, `de.po`, `es.po`, `it.po`, `nl.po`, `ro.po`, `zh_CN.po`), in
  the gettext format any `.po` editor opens. The catalogues are compiled into the program.

`cargo xtask i18n` gathers the texts from the code, rewrites the template, merges it into every
catalogue (a new text arrives with an empty translation, a text gone from the code is dropped) and
prints how much of each language is translated. CI fails when the template is out of date, or when a
translation loses or adds a `{placeholder}`.

## Reviewing a preview language

An entry marked `#, fuzzy` is a draft translation awaiting further review. To review one, read the text in the window and in the catalogue, correct the `msgstr`
where needed, and remove the `#, fuzzy` line of each entry you have checked. When a native speaker
has reviewed a whole language, its entry in `crates/i18n/src/lib.rs` stops being a preview and the
"(preview)" goes from its name.

## Adding a language

1. Add the language to `Lang` in `crates/i18n/src/lib.rs`: its code, its name in its own words, and
   whether it is a preview; and its date conventions (24-hour or AM/PM, first day of the week, the
   order of a numeric date) in `crates/i18n/src/date.rs`. English's catalogue is the US's; outside
   the US, Canada and the Philippines (by the system's regional format) English writes the day
   first, and its clock and first day of the week are the region's (`Lang::conventions_in`).
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
- A message that says what didn't happen and why is two entries and a pattern: the heading
  ("Your automatic replies weren't changed"), each reason by itself (`msgctxt "why automatic
  replies weren't changed"`, `msgid "you are working offline."`), and `msgid "{heading}:
  {reason}"` (`msgctxt "what didn't happen, then why"`), which joins them. Put the language's own
  punctuation and spacing in the pattern (French `{heading} : {reason}`, with a narrow no-break
  space), and write each reason so it reads after it, as it would after the heading.

Translations are contributed like code, by pull request, under the [Contributor Licence
Agreement](../CLA.md).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-10 | patch | #978: English writes dates the day first outside the US, Canada and the Philippines, with the region's clock and first day of the week |
| 2026-11-08 | minor | #912: a heading and its reason are separate entries, joined by the `{heading}: {reason}` pattern, which each language punctuates its own way |
| 2026-10-29 | minor | Dutch as a preview language (`nl.po`); File › Options names it Nederlands (voorbeeld) |
| 2026-10-20 | major | First version: French, German, Spanish and Italian as preview languages; reviewing and adding a language; ribbon labels' line breaks and `msgctxt` |
