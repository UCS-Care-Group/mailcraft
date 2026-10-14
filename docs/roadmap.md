# MailCraft roadmap: milestones and what's next

> **Last reviewed:** 2026-10-11 · **Last updated:** 2026-10-14 · **Change:** patch (waves 1 and 2 of the gap build-out landed: M7 has `.msg` import, printing and most of the smaller features; the current focus says what wave 1 took of gap 2) · **Target:** Microsoft Outlook (Microsoft 365, classic desktop)

Forward-looking plan. The summary is [`ROADMAP.md`](../ROADMAP.md); the ranked work list is
[`gaps.md`](gaps.md); the numbers are in [`target-app-parity.md`](target-app-parity.md).

## Current focus (toward an alpha that works with a real account)

1. **Real servers** (gap #1): a local Dovecot + Postfix in a container as a test target that needs
   no real credentials, then interop notes for the big hosted services. Fix what breaks.
2. **Two-way sync** (gap #2; folder changes, folders the server dropped, a batched download, changes sent during a run, drafts, copies, imports, categories as keywords, IDLE and undo after a pushed move landed 2026-10-14; importance and Graph mail categories remain, and none of it has met a real server beyond the stand-ins): a pending-changes queue in the mailbox (flags, moves, deletes,
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
| M3 | **Writes like mail** | Rich-text composing and replies, inline pictures, spell check (done 2026-10-14) | #7 |
| M4 | **Schedules** | Invitations by mail, accept/decline from the reading pane, time zones (done 2026-10-12, M3.5), reminders that fire | #9–#11 |
| M5 | **Scales** | A per-message store with an index; 500 000 messages stay quick (incremental saves done 2026-10-14; content on demand left) | #8 |
| M6 | **Beyond IMAP** | JMAP and Microsoft Graph over HTTPS — which also gives the browser build a server connection | #5 |
| M7 | **Switchers** | `.pst`/`.msg` import (`.msg` done 2026-10-14), languages, printing (done 2026-10-14), Quick Steps, search folders and the rest of the smaller features (done 2026-10-14) and the long tail of the catalog | #12, #14, #15, #17 |

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
| 2026-10-14 | patch | M7: `.msg` import, printing and most of gap 17 landed |
| 2026-10-14 | patch | M3 done, M5 partly (wave 3) |
