# Contributors in the About window

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-20 · **Change:** minor · **Target:** Microsoft Outlook (Microsoft 365, classic desktop)

**About ▸ Contributors** credits everyone who contributed to MailCraft. This follows the shared
craftrules standard
[`standards/contributors.md`](https://github.com/storytold/craftrules/blob/main/standards/contributors.md),
as WordCraft does; this page is the local copy of the decision.

## Decision

- Two files, both **compiled into the binary** by `crates/ui-egui/build.rs` (nothing is read from
  disk or the network at run time; the web build has them too):
  - [`contributors/contributors.json`](../contributors/contributors.json): commit stats per GitHub
    username. **Generated**; never hand-edit.
  - [`contributors/people.toml`](../contributors/people.toml): the names people chose for
    themselves. **Each contributor adds only their own entry**, in their own PR. It starts empty.
- Each contributor gets one line: GitHub username (always), display name (opt-in, or their synced
  GitHub profile name), real name (only if they told us), merged PRs, commits, lines added, lines
  deleted, binary assets added, binary assets removed, first and last commit date.
- The list is shown as a **grab bag** (names flowing as text) or a **table**, in the same order. It
  sorts by name, PRs, commits, lines added, lines deleted, line delta, binary assets added/removed,
  first or last commit date. The name button cycles **Username → Display name → Real name**; a
  missing name falls back to `@username`. Alphabetical sorting is case-insensitive and ignores the `@`.
- Each name links to the person's GitHub profile, which opens only when it is clicked.

### Where MailCraft differs from the standard

- **No About ▸ Models.** The standard also credits the AI models named in `Co-Authored-By`
  trailers. MailCraft's commits and PRs credit AI help with a plain line instead,
  `Human-reviewed, AI-assisted, nobody's mailbox harmed.`, and carry no such trailer, so that tab
  would be empty (the older trailers in the history are not counted), and it is left out.
  `build.rs` reads no `models` list.
- **Both files are read as hostile.** They arrive in pull requests. A username GitHub would not
  give (not 1–39 letters, digits and hyphens) is left out, as is a second entry for the same
  username; names are trimmed, stripped of control characters and cut to 100 characters; a date
  that isn't one is no date; at most 10,000 contributors are built in. A malformed or missing
  `contributors.json` never fails a build: the build warns and the tab says no credits were built
  in; without `people.toml` everyone is credited by username. The tests read the repository's own
  files with the same code (`crates/ui-egui/src/credits_data.rs`), so a malformed file fails
  `cargo xtask ci`.
- **The first `contributors.json` was written by a stand-in.** craftrules (and its
  `scripts/contributors.py`) could not be reached when this landed, so the file was generated with
  a script that writes the same schema 1 from `git log --no-merges --numstat` on `main`, GitHub's
  commit list (for the username of each commit) and the merged PRs. It was regenerated the same way
  on 2026-10-20 (`--no-renames`; a binary counts as added when the commit creates it and deleted
  when it deletes it; a commit GitHub ties to no account counts as unattributed, a bot's to nobody).
  Regenerate it with the craftrules script when it is available.
- The files are data, not assets: `ATTRIBUTION.md` has no row for them.

## In MailCraft

- The About window is `Dialog::About` in `crates/ui-egui/src/dialogs.rs` (Help › About MailCraft;
  File › About › Show Contributors). Its Contributors tab is `crates/ui-egui/src/credits.rs`.
- From the control channel: `ui.dialog {"name": "contributors"}` (or `{"name": "about", "tab":
  "contributors"}`) opens the tab; `ui.inspect` then names the dialog `contributors`. The same
  with `testers` opens About ▸ Testers.

## Testers

**About ▸ Testers** thanks, by name, the testers who said yes to being credited. A tester is
someone who has used a MailCraft release on their own mail and reported what went wrong; signing
up alone doesn't count. Anyone who also contributes code is credited in both tabs.

- **Consent.** A tester says yes by ticking "Credit me in About › Testers" in the
  [sign-up form](../.github/ISSUE_TEMPLATE/tester-signup.yml), or by saying so in their sign-up
  issue. The number of that issue is recorded with their entry, for the maintainers' audit; it is
  never shown or built into the program.
- **A maintainer adds the entry by hand** to
  [`contributors/testers.toml`](../contributors/testers.toml), once the tester has tested a
  release. It is compiled in by `crates/ui-egui/build.rs` as the other two files are, and read as
  hostile in the same way: a bad or repeated username is left out, a date that isn't a day is no
  date, at most 10,000 testers are built in, and a malformed or missing file never fails a build
  (it warns, and the tab says nobody is credited yet).
- **Fields**, one table per tester:

  ```toml
  [testers.their-github-username]  # 1–39 letters, digits and hyphens
  display_name = "Nickname"        # optional; trimmed, no control characters, at most 100 characters
  since = 2026-10-19               # the day they qualified as a tester
  consent = 11                     # the issue where they said yes; an entry without one is left out
  ```

- **Shown** as the display name, else `@username`, each linking to the GitHub profile (opened
  only when clicked), earliest `since` first (no date last), then by username. No statistics and
  no sorting.
- **Removal.** Delete the entry. Later releases leave the tester out; copies already published
  keep the name.

## Credit yourself

Add your entry to `contributors/people.toml` in a PR you commit yourself (every field optional):

```toml
[people.your-github-username]
real_name = "Your Name"
display_name = "Nickname"
sync_github_name = true   # use the name on your public GitHub profile as display name
```

or, with craftrules checked out next to this repository:

```sh
python3 ../craftrules/scripts/contributors.py --add-me . --real-name "Your Name" --sync-github-name
```

Names show after the next refresh: the script checks that each entry was committed by its own
user (or by a maintainer with a `consent_source`) and marks it verified (`people_entry`); only a
verified entry's names are shown. Delete your entry to remove your names.

## Refreshing (maintainers)

```sh
python3 ../craftrules/scripts/contributors.py .           # regenerate + verify; needs git, Python 3.11+, gh
python3 ../craftrules/scripts/contributors.py --check .   # verify people.toml only
```

Commit the updated `contributors/contributors.json`. Git author names and emails are never recorded.

## Revision history

| Date | Change | What |
|---|---|---|
| 2026-10-15 | major | First version (D2): About ▸ Contributors, no Models tab |
| 2026-10-19 | minor | AI help credited by the line "Human-reviewed, AI-assisted, nobody's mailbox harmed." rather than a `Co-Authored-By` trailer |
| 2026-10-19 | minor | Testers: About ▸ Testers, `contributors/testers.toml`, consent recorded by issue number |
| 2026-10-20 | minor | `contributors.json` regenerated by the stand-in: strikerz5 and bvisible credited, the counts brought up to date |
