# MailCraft roadmap: milestones and what's next

> **Last reviewed:** 2026-10-14 · **Last updated:** 2026-10-19 · **Change:** patch (M6: JMAP read in progress, a JMAP account's mailboxes, newest messages and whole messages in the browser build, changes up and sending not yet; M5: the window never waits for a save, R6.3; superseded for the way to beta by the roadmap reset) · **Target:** Microsoft Outlook (Microsoft 365, classic desktop)

Forward-looking plan. The summary is [`ROADMAP.md`](../ROADMAP.md); the ranked work list is
[`gaps.md`](gaps.md); the numbers are in [`target-app-parity.md`](target-app-parity.md).

## Current focus

**Superseded on 2026-10-14** by the roadmap reset
(issue #2). This page is folded into `ROADMAP.md` in
step R8.3. What follows is the focus as it was written on 2026-10-11, kept for the record: two-way
sync, Microsoft sign-in and the first release have landed since.


1. **Real servers** (gap #1): a local Dovecot + Postfix in a container as a test target that needs
   no real credentials, then interop notes for the big hosted services. Fix what breaks.
2. **Two-way sync** (gap #2): a pending-changes queue in the mailbox (flags, moves, deletes,
   appends to Sent), pushed before each download; conflict rule "the server wins, except what is
   still pending". `proto` already has `uid_store` and `append`.
3. **Sign-in** (gaps #3, #4): OAuth 2.0 device/loopback flow with XOAUTH2 and OAUTHBEARER; tokens
   and passwords in the system keychain behind a `Services` hook (memory-only stays the fallback
   and the browser's only option).
4. **First release** (gap #18): run the packaging on every platform, fix what the adaptation from
   WordCraft missed, publish signed alpha builds and the web build.

## Milestones

| | Milestone | What it means | Main gaps |
|---|---|---|---|
| M0 | **Foundations** (done, 2026-10-11) | The layered workspace, engine, formats, protocols, front end and tooling exist and are tested | — |
| M1 | **Connects** | A mainstream hosted account works for a week on two devices | #1–#4, #18 |
| M2 | **Reads like mail** | Inline pictures, basic layout and colours in HTML mail, still without fetching anything; more charsets | #6, #13 |
| M3 | **Writes like mail** | Rich-text composing and replies, inline pictures, spell check | #7 |
| M4 | **Schedules** | Invitations by mail, accept/decline from the reading pane, time zones (done 2026-10-12, M3.5), reminders that fire | #9–#11 |
| M5 | **Scales** | A per-message store with an index; 500 000 messages stay quick (under way as R6 of the reset: the mailbox file is a log of records, and the window never waits for a save, R6.3) | #8 |
| M6 | **Beyond IMAP** | JMAP and Microsoft Graph over HTTPS — which also gives the browser build a server connection. Graph carries Microsoft accounts' mail (R3). JMAP is under way and reads so far: an account's mailboxes, the newest messages of each and a message fetched whole in the browser build (`crates/jmap`); changes made here going up, sending, drafts and an Add Account page for it are not built yet | #5 |
| M7 | **Switchers** | `.pst`/`.msg` import, languages, printing, Quick Steps and the long tail of the catalog | #12, #14, #15, #17 |

M6 is what makes the web build a real mail client rather than a local one; it may move earlier
if that turns out to be what people want most.

## Design notes for what's next

- **Pending changes** belong in `model` (plain data, like `sync`), so the engine stays pure and
  the browser build can queue changes for a later HTTP-based transport.
- **HTML rendering** should stay a display-list of our own, not a web view: grow `html::Block`
  (tables, colours, images by `cid:` from the message's own attachments) and paint it in
  `ui-egui`. Remote content stays opt-in per sender, off by default.
- **Rich composing** needs an editor model. WordCraft's document and layout crates are the obvious
  thing to share rather than rewrite; talk to that project first.
- **The store**: one file per message (`.eml`) plus an index, so other tools can read the mailbox
  and a crash can lose at most the message being written.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-11 | major | First version |
| 2026-10-12 | patch | Time zones, listed under M4, are done (M3.5) |
| 2026-10-14 | minor | Superseded for the way to beta by the roadmap reset |
| 2026-10-15 | patch | M5: under way as R6; the window never waits for a save (R6.3) |
| 2026-10-19 | patch | M6: JMAP read in progress (a JMAP account's mailboxes, newest messages and whole messages in the browser build); changes up and sending not yet |
