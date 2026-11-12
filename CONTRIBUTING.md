# Contributing to MailCraft

> **Status:** current · **Written:** 2026-10-15 · **Last updated:** 2026-11-11 · **Change:** patch

Thank you for wanting to help. MailCraft holds people's mail, so the way work comes in matters as
much as the work.

## Before you build anything

1. **Read [AGENTS.md](AGENTS.md).** It is for people as much as for agents: never crash, mail is
   private and hostile, no vendor assets, clean room. None of it is negotiable.
2. **Read the plan.** Work to beta follows the roadmap reset, tracked in
   [#2](https://github.com/UCS-Care-Group/mailcraft/issues/2). Work that goes against an accepted
   decision won't be merged, however good it is; propose a change to the decision first.
3. **Claim an issue by commenting on it.** Comment "I'm taking this" on an open issue that nobody
   has claimed in its comments, and start: you don't wait for anyone to assign it to you (a maintainer may add the `claimed`
   label later, so claims are easy to see; it is a marker, not a permission). Read the
   comments first, as the first claim stands; if you want to share the work, say so to the person
   who claimed it in the issue. A claim lapses when its author has posted no comment, pull request
   or draft for 14 days, and anyone may then claim it with a new comment. A new feature or change
   that isn't in the issues: open one and wait for a yes before building it.
4. **An open pull request holds its feature.** While a pull request is open, its feature is its
   author's. A pull request still waiting for a review keeps it however long that takes. Once a
   review asks for changes, the author has 7 days from that review to answer, with a commit or a
   comment. After 7 days without an answer the feature is free: the issue's claim lapses with it,
   anyone may claim the issue with a new comment and build the feature afresh, from the specs and
   never from that pull request's code. The pull request can still be updated and merged if nobody
   has built the feature meanwhile.
5. **Found a bug and have the fix? Send both at once.** Open the
   [problem report](https://github.com/UCS-Care-Group/mailcraft/issues/new?template=problem.yml)
   and the pull request that fixes it together, with `Closes #<issue>` in the pull request: no
   claim and no wait. A report without a fix is just as welcome. If the fix needs one of the
   [owners' decisions](#decisions-that-are-the-owners-not-a-contributors) below, ask in the issue
   first.

Issues labelled `post-beta` are parked: they aren't open to claims until beta is reached.

## The pull request

- **Sign the [Contributor Licence Agreement](CLA.md) once**, by commenting on your first pull
  request: `I have read the MailCraft CLA and I agree to it.` You keep the copyright in your work;
  it lets UCS Care Group Limited also release MailCraft under other terms, while MailCraft stays MIT OR
  Apache-2.0. The `cla` check on the pull request turns green when every author has signed.
- **One issue, one pull request**, based on current `main`, small enough to review in one sitting.
  Say `Closes #<issue>` in the description.
- **A feature in several parts:** each part is a pull request of its own, built on the part
  before it, and its description starts with `Builds on #<n>`. Open the later parts as drafts;
  when a part is merged, bring the next one up to date with `main` and mark it ready. If the
  parts only make sense together, say so in the first one: the maintainers then review the whole
  stack once, merge its top pull request and close the others as included in it. (Pull requests
  from a fork can't be based on each other's branches here, so the order lives in the
  descriptions.)
- **Allow edits by maintainers** (the box under a new pull request, on by default for a fork):
  it lets a maintainer bring your branch up to date with `main` or fix a small thing without
  waiting for you.
- **`cargo xtask ci` passes** on your machine before every commit (fmt, clippy, tests, assets,
  spdx, test-files, layers, wasm, and cargo-deny when it is installed). Commits carry the task id (`R3.2: messages come down by delta`).
- **Rows of the documents' tables and lines of the release notes are files of their own.** A row
  of a document's revision history goes in `docs/log.d/<document without .md>/revisions/<issue>.md`,
  a row of `ROADMAP.md`'s progress log in `docs/log.d/ROADMAP/progress/<issue>.md`
  ([`docs/log.d/README.md`](docs/log.d/README.md)), and a release-notes line in
  `packaging/release-notes.d/` ([its README](packaging/release-notes.d/README.md)), never in the
  table or the notes themselves: they are joined in at release, so pull requests don't conflict there.
- **Every crash fix and every data-loss fix comes with a regression test.**
- **Security fixes: a fix for a security finding comes with a test that covers every site of its
  root cause**, not only the one that was reported: a table over every command, reader or writer
  of that kind, built so that one added later without the fix fails it.
- **Tests go in files of their own**: `tests.rs`, `<name>_tests.rs` or a `tests/` directory,
  brought in with `#[cfg(test)] mod <name>_tests;`, not an inline `mod tests { … }` in a source
  file. Code scanning ([`.github/codeql/codeql-config.yml`](.github/codeql/codeql-config.yml))
  skips those files; in a source file, a test's made-up passwords and assertion messages are
  flagged as if they were production code. `cargo xtask test-files` (part of `cargo xtask ci`)
  fails on a new inline test module. The older ones are listed in `xtask/inline-tests.txt`; when
  you move one out, take its line off the list (the check says so), and never add one.
- **New Rust files start with the two SPDX lines** (copyright and licence); `cargo xtask spdx --fix`
  adds them.
- **Tests never touch a real mailbox.** Use the fake servers in `crates/net/src/fake.rs` and the
  Graph stand-ins, with `example.org` addresses.
- **Changes to sync, the store, sign-in and secrets** (the paths in
  [`CODEOWNERS`](.github/CODEOWNERS), which asks a maintainer for review) are read by a maintainer
  before they land. Expect questions.

## A first contribution

Issues labelled [`good first issue`](https://github.com/UCS-Care-Group/mailcraft/labels/good%20first%20issue)
are small and self-contained, with the files to look at named in the issue. Comment on one to
claim it.

## How pull requests are reviewed

The standards a reviewer holds a pull request to are in
[`GOVERNANCE.md`](GOVERNANCE.md#how-changes-are-reviewed).

## Decisions that are the owners', not a contributor's

Ask in an issue first, and don't put any of these into a pull request until the answer is yes:

- a new permission asked of Microsoft or any other provider;
- anything added to the HTML presentation whitelist (`crates/html/src/style.rs`);
- a change to `AGENTS.md` (which `CLAUDE.md` points to), `PRIVACY.md`, `TERMS.md` or an accepted design decision;
- a new crate, a new dependency with a licence other than MIT, Apache-2.0, BSD or ISC, or a new
  asset;
- anything that sends something off the computer that isn't sent today.

## Testing MailCraft

Want to use it rather than build it? Sign up with the
[tester form](https://github.com/UCS-Care-Group/mailcraft/issues/new?template=tester-signup.yml).
Found something wrong? Use the
[problem report](https://github.com/UCS-Care-Group/mailcraft/issues/new?template=problem.yml).

A build from this repository sends Help › Report a Problem › Send report to the project's real
report relay (`MAILCRAFT_REPORT_URL` in `.cargo/config.toml`), where every report becomes an
issue the maintainers read. Don't press it to try the window out: build with
`MAILCRAFT_REPORT_URL=` (empty) to have Open on GitHub alone, and use the tests' stand-in relay
(`crates/net/src/report_tests.rs`) for the sending.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-11 | patch | #956 part 3: Send report goes to the real report relay: build with `MAILCRAFT_REPORT_URL=` to try the window out |
| 2026-11-07 | minor | Security fixes: a fix for a security finding comes with a test that covers every site of its root cause |
| 2026-11-04 | minor | A row of a document's revision history or of the progress log is a fragment, `docs/log.d/…`, joined in at release like a release-notes line |
| 2026-11-03 | minor | Claim an issue by commenting on it: no waiting for an assignee; `claimed` stays as a maintainer's marker; the first claim stands and lapses after 14 quiet days |
| 2026-10-27 | minor | A feature in several parts: one pull request a part, `Builds on #<n>`, later parts as drafts or the stack merged at its top; allow edits by maintainers; a pull request template with both |
| 2026-10-24 | minor | `cargo xtask test-files` fails on a new inline test module; older ones listed in `xtask/inline-tests.txt` until moved |
| 2026-10-23 | minor | A bug fix needs no claim: the problem report and its pull request come together |
| 2026-10-21 | minor | Tests go in files of their own (`tests.rs`, `*_tests.rs`, `tests/`), which code scanning skips |
| 2026-10-20 | minor | A first contribution (`good first issue`); how pull requests are reviewed; SPDX lines in new Rust files |
| 2026-10-19 | minor | The Contributor Licence Agreement ([CLA.md](CLA.md)), signed once by a comment on a pull request |
| 2026-10-16 | minor | Changes to sync, the store, sign-in and secrets are read by a maintainer before they land (no promise of enforced code-owner review); `CLAUDE.md` is a link to `AGENTS.md` |
| 2026-10-15 | major | First version: claim before you build, one issue per pull request, decisions kept for the owners, tester sign-up |
