# Governance

> **Last reviewed:** 2026-10-20 · **Last updated:** 2026-10-23 · **Change:** minor (a bug fix comes with its report, no claim)

How MailCraft is run: who decides what, who does what, and how the project carries on if someone
can't.

## Roles

| Role | Who | Responsibilities |
|---|---|---|
| **Owner** | UCS Care Group, through Martin Ball ([@benandez](https://github.com/benandez)) | Sets the direction ([`ROADMAP.md`](ROADMAP.md)); makes the decisions listed below; publishes releases; holds the release signing keys, the Microsoft registration and the organisation's accounts; answers security reports ([`SECURITY.md`](SECURITY.md)) and conduct reports ([`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)). |
| **Maintainers** | [@benandez](https://github.com/benandez), [@billsecond](https://github.com/billsecond) (administrators of the repository) | Review and merge pull requests; triage issues; keep CI, dependencies and security alerts in order. Changes to sync, the store, sign-in and secrets are reviewed by a code owner ([`.github/CODEOWNERS`](.github/CODEOWNERS)). |
| **Contributors** | Anyone who opens an issue or a pull request | Follow [`CONTRIBUTING.md`](CONTRIBUTING.md): claim an issue before building (a bug fix comes with its report instead), one issue per pull request, `cargo xtask ci` before every commit, tests with every change; sign the [CLA](CLA.md) once. |
| **Testers** | People signed up in [#11](https://github.com/UCS-Care-Group/mailcraft/issues/11) | Use a release on their own mail and report what goes wrong. |

## How decisions are made

- **Day to day,** a maintainer decides by reviewing a pull request against the project's rules
  ([`AGENTS.md`](AGENTS.md), [`CONTRIBUTING.md`](CONTRIBUTING.md)). A pull request merges only when
  the required checks (`ci-ok`) pass.
- **Some decisions are the owner's alone** and are asked about in an issue first: the list is in
  [`CONTRIBUTING.md`](CONTRIBUTING.md#decisions-that-are-the-owners-not-a-contributors) (a new
  permission asked of a provider, the HTML presentation whitelist, `AGENTS.md`, `PRIVACY.md` and
  `TERMS.md`, a new crate or a dependency outside the usual licences, a new asset, anything newly
  sent off the computer). The owner answers in the issue, with reasons.
- **The order of the work** is `ROADMAP.md`; proposals to change it are issues.
- **Disagreements** are settled in the issue or pull request; the owner has the last word.

## How changes are reviewed

Every change reaches `main` through a pull request. A reviewer checks, before approving or merging:

- **Purpose.** It does one thing, says which issue it closes, and doesn't take one of the owner's
  decisions without the owner's yes in the issue.
- **The rules.** No panics or `unsafe` in production code, everything from outside treated as
  hostile, secrets never in our files or logs, nothing newly sent off the computer, no copied
  code or vendor assets ([`AGENTS.md`](AGENTS.md)).
- **Tests.** New behaviour has tests; a crash or data-loss fix has a regression test; nothing
  touches a real mailbox.
- **Docs.** What a user or contributor would need to know is updated in the same pull request,
  with the touched docs' status lines and revision rows.
- **The checks.** `ci-ok` is green (fmt, clippy, tests, assets, SPDX lines, layers, wasm,
  cargo-deny), and CodeQL raises nothing new.
- **The code owners' paths.** Changes to sync, the store, sign-in and secrets
  ([`.github/CODEOWNERS`](.github/CODEOWNERS)) are read line by line by a code owner.

The author answers every comment, by a change or a reason, before it merges.

## Continuity

The repository has two administrators, so the project can carry on, merge and publish if one of
them can't. The release signing key has a spare built into the program
([`docs/releasing.md`](docs/releasing.md)), so a release can still be signed if the key in use is
lost.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-23 | minor | A bug fix comes with its report, no claim |
| 2026-10-20 | minor | How changes are reviewed |
| 2026-10-20 | major | First version: roles, how decisions are made, continuity |
