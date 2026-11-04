# Table-row fragments

A pull request that adds a row to a document's **Revision history**, or to the **Progress log**
in `ROADMAP.md`, adds the row here, as a file of its own, instead of editing the table (so two
pull requests never conflict over the top of a table):

- `<document without .md>/revisions/<name>.md` for a row of its revision history:
  `docs/gaps/revisions/795.md` goes into `docs/gaps.md`, `ROADMAP/revisions/795.md` into
  `ROADMAP.md`;
- `ROADMAP/progress/<name>.md` for a row of the progress log.

`<name>` is the issue it resolves, or the pull request's own number, or a short name when it has
neither. The file holds the row (or rows) exactly as it reads in the table, with the table's
columns and the date in its Date column:

```
| 2026-11-04 | minor | #795: what changed in this document |
```

The status line at the top of the document is still edited in the document itself.

`cargo xtask docs-log` shows each document with its fragments joined in at the top of the table,
newest date first, and changes nothing; `cargo xtask ci` checks that every fragment names a
document and table that are there, that each row has the table's columns and a date, and that
they join. At release time, `cargo xtask docs-log --write` moves them into the documents and
removes them ([`docs/releasing.md`](../releasing.md)).
