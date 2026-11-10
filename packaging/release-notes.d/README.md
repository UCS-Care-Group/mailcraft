# Release-note fragments

A pull request with a user-visible change adds its line for the release notes here, as a file of
its own, instead of editing `packaging/release-notes.md` (so two pull requests never conflict over
the notes):

- `fixes/<issue>.md` for a line under **Fixes**;
- `new/<issue>.md` for a line under **What's new**.

`<issue>` is the issue it resolves, or the pull request's own number, or a short name when it has
neither. The file holds the line exactly as it reads under the heading, issue linked in full:

```
- Opening an attachment works where `xdg-open` isn't installed ([#445](https://github.com/GITHUB_REPOSITORY/issues/445)).
```

`cargo xtask release-notes` shows the notes with every fragment joined in, newest first;
`cargo xtask ci` checks that they join. Before tagging, `cargo xtask release-notes --write` moves
them into `packaging/release-notes.md` and removes them ([`docs/releasing.md`](../../docs/releasing.md)).
