# R3: Graph mail for Microsoft accounts: implementation plan

> **Status:** proposed, with [the spec](../specs/r3-graph-mail.md) · **Written:** 2026-10-15 · **Change:** minor (R3.3 split in two, estimated in agent time)

Refines step R3 of [the roadmap reset plan](roadmap-reset.md). Requirement numbers (S1–S36) are
the spec's. Each task is one pull request, reviewed by a person (`needs-review`), with
`cargo xtask ci` green on all three CI platforms; task ids go in commit messages
(`R3.2: messages come down by delta`). The Graph stand-in (`crates/net/src/graph/standin.rs`)
grows inside each task with exactly what that task needs. The engine stays free of I/O: the
request/apply split of `crates/model/src/sync.rs` gains Graph types of its own beside the IMAP
ones, rather than bending the uid-based ones. Hours are guesses until a task is done.

## R3.1 Folders, and the ground under everything else (#12) (14–22 h)

| Part | What |
|---|---|
| Account kind | `AccountKind::Microsoft` (S1); behind the testing switch (S35); Add Account makes one only with the switch on |
| Server ids | A server id on `Message` and `Folder`, a delta link per folder, and the last synced state of a message widened to read, flag, categories, importance and folder (ADR 0003); older files open with these empty |
| Graph client | `Prefer: IdType="ImmutableId"` on every mail request (S4); the retry policy by kind of request, `Retry-After`, a per-account pause and at most 4 requests at once, 2 for background work (S14, S21) |
| Folders | The full walk of mail folders, hidden and search folders left out, Outlook's order (S6); New, Rename, Move, Delete Folder to Deleted Items (S7); removal only after a complete listing (S8) |
| Stand-in | Folders: list, childFolders, create, PATCH, move, delete, names with `/` and `\`; 429 with `Retry-After` |

**Done when:** stand-in flow tests both ways (a folder made here is there; one renamed, moved or
deleted there is so here, keeping its server id and its messages); folders named `Sales/Marketing` and
`A\B` are listed under their parent, whole; a listing cut off removes
nothing; a 429 pauses and resumes; a mailbox file from 0.2.1 opens.

## R3.2 Mail down (#13) (16–26 h)

Three pull requests, in order. The sign-in part of the first version of this plan is gone: R3.1
already asks Microsoft accounts for `Mail.ReadWrite`.

### R3.2.1 Stand-in and model (4–6 h)

| Part | What |
|---|---|
| Stand-in | Message delta per folder: pages (`odata.maxpagesize`), `@odata.nextLink` and `@odata.deltaLink`, newest first, `@removed`, a page served twice, `410 Gone` and `syncStateNotFound`, moves between folders; `$value` (MIME); a generated mailbox of 200,000 messages (held in memory: a round of it takes about 2 s); the record of requests capped |
| Model | Fetched as a field of the message (S12); a message's "gone from this folder" mark (S9); the per-account "Download messages in full" setting (S12), read leniently by older files |
| Sync types | Graph message pages beside `GraphMailResult`: messages new and changed, `@removed`, the folder's next or delta link, counts for progress (S10, S15) |

**Done when:** the stand-in's delta, read straight, gives every message of the generated mailbox
once across pages, replays and links; a mailbox file from before opens.

### R3.2.2 Delta down, resumable (8–12 h)

| Part | What |
|---|---|
| Job | The Graph job sends a piece per page, as IMAP's does (`sync_with` + `apply_sync_part`); a shared Cancel stops after the page in hand and stops the account's Graph work (S15) |
| Delta | Newest page of every folder first, then each to its end; every folder on each run (S9); the next or delta link taken in with its page (S10); expired links listed afresh (S11); matched by server id (S5) |
| Moves and changes | `@removed` marks, moves kept with what was fetched, removal only after every folder's delta ended in the run (S9); a change here is never overwritten (S9, ADR 0003) |
| Progress | Status bar: folder with its count, folders done of all, Cancel (S15) |

**Done when:** 200,000 messages from the stand-in, interrupted and resumed three times at random
(Cancel, a dropped connection, a crash between page and save), arrive with nothing twice and
nothing missing; the list is usable before the run ends; a message moved on the server keeps its
fetched content; an expired link relists that folder and removes only what a complete listing
lacks; a flag changed here survives a page that brings the old one.

### R3.2.3 Fetching (4–8 h)

| Part | What |
|---|---|
| Background | MIME newest first across the account, at most 2 requests at once, under the account's "Download messages in full" limit (S12, S14); a larger limit fetches the rest |
| On demand | At once on opening, before Reply, Forward and Save, ahead of background work; offline wording (S12) |
| Search | Counts what isn't searched in full (S13) |
| Options | "Download messages in full" in the account's settings: Everything, up to 10, 25 or 50 MB (default 10) |
| Progress | Fetched of all, and how many larger messages wait for opening (S15) |

**Done when:** every message of the generated mailbox under the limit is fetched, newest first,
and Cancel stops it; opening a large one fetches it at once; offline says it isn't here yet;
search says how many weren't searched in full.

## R3.3 Changes up (#14) (about 3–5 agent hours)

Two pull requests, in order, from the R3.3 decision session. The estimates of this section are in
an agent's working time, measured against R3.1 and R3.2 (planned at 30–48 hours, landed in a day);
the other sections still give the first version's estimates in a person's hours.

### R3.3.1 Stand-in and model (about 1–2 agent hours)

| Part | What |
|---|---|
| Stand-in | PATCH of read, flag (with dates), categories and importance; move, copy (a new id, extended properties carried), delete; `$batch` of up to 20 with an answer per request; `$filter` by extended property; 404/403/400 on demand per message; a connection dropped after the request; throttling of a request or a whole batch |
| Model | The copy and delete-for-good queue; the categories merge (S17); "being moved" and "incomplete" on a message moved out of the account (S19); files from before open as they were |

**Done when:** the stand-in answers each of these as Graph does, tested on its own; nothing is any
different for the owner yet.

### R3.3.2 Changes up (about 2–3 agent hours)

| Part | What |
|---|---|
| Differences | From the last synced state: read, flag with its due date, categories, importance as one PATCH, a different folder as a move (S16); the conflict rule, categories merged (S17); in `$batch` (S22); the order of a run (S16) |
| Queue | Copy with its marker, delete for good (S16, S21) |
| Deleting and moving out | Delete to Deleted Items; delete for good with Graph's delete; never `permanentDelete`; a move into a local folder or another account removes the server's copy only once fetched whole and saved; moves into a server folder refused until R3.4 (S19) |
| Refused | Put back to the server's state, said once per run and kind, logged (S20); a move looked up by id and a copy by its marker before retrying (S21) |
| Categories | Outlook's colour wins for a category on its list; "The colour is set in Outlook"; "Not in Outlook's list: no colour there" in the Categorize dialog (S18) |

**Done when:** one test per kind of change; a refused change is put back and said; a change made
here and one made on the server to the same message end as S17 says (categories merged); marking
10,000 messages read takes no more than 500 batches and their retries; a batch answered partly
done, partly refused and partly throttled puts back the refused, retries the throttled and sends
nothing twice; a copy whose answer was lost is found by its marker and not made twice; a move into
a local folder, offline and not fetched, leaves the server's copy until the whole message is saved
here; a grep test finds no `permanentDelete` anywhere.

## R3.4 Drafts and sending (#15) (12–18 h)

| Part | What |
|---|---|
| Drafts up | Created with our extended property, found again after a lost answer (S23); create-reply and create-forward; changed in place field by field, attachments added and removed, large ones by upload session (S24); on save, close and every 5 minutes (S25); edited on both sides kept twice (S26) |
| Sending | Send-draft or `sendMail`, no SMTP fallback, no copy uploaded (S27); held messages (S28) |
| Stand-in | Create from MIME and JSON, createReply/createForward, PATCH of drafts, attachments and upload sessions, send, `sendMail`, `$filter` by extended property |

**Done when:** a draft written here is in the stand-in's Drafts and back, once, after a lost
answer; editing it changes it in place; a reply stays in its conversation; a send whose answer is
lost is held and not sent again.

## R3.5 Undo (#16) (4–8 h)

Undo for a Microsoft account no longer forgets its history when a change reaches the server; the
next run sends the reverse (S29). **Done when:** undo of a move, a flag, categories and a delete
reach the stand-in as their reverses; undo of a delete for good says it can't.

## R3.6 Imported mail (#17) (2–4 h)

Imports go to a local folder; moving imported mail into a server folder is refused with the reason
(S30). **Done when:** tested; the refusal's wording is in the UI test.

## R3.7 The switch (#18) (10–16 h) [owners read the wording]

The once-only page; the last IMAP push first, waiting when offline; the dated backup; the fresh
download; local folders, unsent drafts and favourites kept (S31–S33). The testing switch goes
(S35). **Done when:** a mailbox from 0.2.1 with an offline flag, an offline move, a local folder,
an unsent draft and a favourite switches against the fake IMAP server and the stand-in, and keeps
all five.

## R3.8 IMAP off for Microsoft accounts (#19) (4–6 h)

When S34's measure is met: the XOAUTH2 IMAP and SMTP path for Microsoft accounts, its permissions
and the switch's IMAP half go. **Done when:** no IMAP or SMTP connection is made for a Microsoft
account in any flow test; the admin consent list has no IMAP or SMTP permission.

## R3.9 The author's mailbox, 7 days (by hand)

Martin Ball's own Microsoft 365 mailbox on a build with R3.1–R3.7, daily, for 7 days, checked
against Outlook on the web at the end. Any data-loss incident is fixed with a regression test and
the 7 days restart.

## Order

```
R3.1 ─ R3.2 ─ R3.3 ─ R3.4 ─ R3.5 ─ R3.7 ─ R3.9 ─ R3.8
                     └─ R3.6 ┘
```

Total: about 76–122 hours before R3.9.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-15 | major | First version |
| 2026-10-15 | minor | Graph client row: immutable ids on mail requests only (spec S4) |
| 2026-10-15 | minor | R3.2 split into R3.2.1 (stand-in and model), R3.2.2 (delta down, resumable) and R3.2.3 (fetching), from the R3.2 decision session; sign-in row dropped (done in R3.1) |
| 2026-10-15 | minor | R3.2.1 done; the stand-in holds its generated mailbox in memory (a round of 200,000 messages takes about 2 s) |
| 2026-10-15 | minor | R3.2.2 done |
| 2026-10-15 | minor | R3.2.3 done; R3.2 done |
| 2026-10-15 | minor | R3.3 split into R3.3.1 (stand-in and model) and R3.3.2 (changes up), from the R3.3 decision session; `$batch`, the copy marker and category colours in scope; R3.3 estimated in agent time |
