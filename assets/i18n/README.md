# Interface translations

One message catalog per language, built into the program (`crates/ui-egui/src/i18n.rs`):

| File | Language | State |
|---|---|---|
| `es.txt` | Español | full |
| `fr.txt` | Français | full |
| `de.txt` | Deutsch | full |
| `pt-BR.txt` | Português (Brasil) | full |
| `ar.txt` | العربية | partial: the short labels (right to left) |
| `he.txt` | עברית | partial: the short labels (right to left) |

**These translations were machine-written** from MailCraft's own English and want review by
native speakers. Corrections are very welcome. They are translated from MailCraft's English
only, never copied from another program's translations.

## Format

```text
# A comment.
@language de
@name Deutsch
@direction ltr

Reply All
= Allen antworten
```

A pair is a line of English, exactly as the program has it in `tl!("…")`, and under it `= ` and
the translation. `\n` is a line break, `\t` a tab, `\\` a backslash; a line of English that starts
with `=`, `#` or `@` starts with a `\` before it. An empty translation (`=` alone) is not
translated yet: the English is shown. `{name}` placeholders must be the same in the translation
as in the English (in any order); a translation that drops or invents one is not used.

## Checking

- `cargo test -p mailcraft-ui-egui --lib i18n::keys` fails on anything a catalog can't hold:
  a broken pair, a placeholder that doesn't match, English that isn't (or is no longer) in the
  program. It prints how much of the program each catalog covers, and doesn't fail below 100 %.
- `cargo xtask i18n [lang]` lists what a catalog still lacks, as pairs ready to fill in.

Dates, times and numbers are not in the catalogs: they are written as the language's region
writes them (`crates/ui-egui/src/i18n/locale.rs`).
