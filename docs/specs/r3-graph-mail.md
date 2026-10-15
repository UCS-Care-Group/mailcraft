# R3: Graph mail for Microsoft accounts

> **Status:** proposed · **Written:** 2026-10-15 · **Author:** Martin Ball, with Claude · **Change:** minor (S16–S22: R3.3 decided in detail) · **Target:** Microsoft Outlook (Microsoft 365, Outlook.com)

The words in this document have the meanings in [`CONTEXT.md`](../../CONTEXT.md). It details step
R3 of [the roadmap reset](roadmap-reset.md); the decisions behind it are
[ADR 0002](../adr/0002-graph-mail-for-microsoft-accounts.md) (Graph only, no IMAP fallback) and
[ADR 0003](../adr/0003-server-ids-and-changes-from-the-last-synced-state.md) (server ids, changes
from the last synced state, deleting as Outlook does). The order of the work is in
[`docs/plans/r3-graph-mail.md`](../plans/r3-graph-mail.md). Tracked in issues #12–#19.

## Why

Beta needs every Microsoft account in step with Outlook on the web (B7), and organisations that
switch IMAP off able to connect at all. Today, on `main`:

- Graph sends mail and handles the calendar, contacts, Favorites and working hours; **mail itself
  comes down and goes up only by IMAP**.
- An account is `Local` or `Imap`; the sync request refuses anything else.
- A message is known by its IMAP uid and Message-ID, a folder by its path. There is no field for a
  server id.
- The sign-in doesn't ask for `Mail.ReadWrite`.
- The HTTP client opens a connection per request and retries nothing; only the Favorites reader
  honours `Retry-After`. `sendMail` has no protection against being sent twice.
- Categories, importance and copy have no way up; drafts never go up; undo forgets its history once
  a change reached the server; imported mail lands in the current folder and never goes up.
- The Graph stand-in fakes no mail folders beyond one read, no messages, no delta, no sending.

## Goal

A Microsoft account's folders and mail are kept in step through Microsoft Graph alone: everything
the owner does in MailCraft happens on the server as it would in Outlook, every change reaches the
server or is refused with a reason, nothing comes down twice and nothing is missed, and accounts of
up to 200,000 messages are usable within minutes of being added.

## Requirements

### Accounts and sign-in

1. A Microsoft account is a kind of account of its own (not an IMAP account with a marker). It has
   no incoming or outgoing server settings; it is kept in step through Graph and never by IMAP or
   SMTP.
2. A sign-in asks for `Mail.ReadWrite`, `Mail.Send`, `Calendars.ReadWrite`, `Contacts.Read`,
   `MailboxSettings.Read`, `offline_access`, `openid`, `profile` and `email`. `Mail.ReadBasic` is
   no longer asked for (`Mail.ReadWrite` covers it). An account signed in before is asked once
   more, through the existing flow. The administrator consent list matches. The IMAP and SMTP
   permissions are dropped at R3.8. The sign-in rule in `CLAUDE.md` and `AGENTS.md` and
   `PRIVACY.md` change with it; this is an owners' decision, agreed by Martin Ball on 2026-10-15
   and to be agreed by billsecond in the pull request.
3. Shared mailboxes and delegate access are not part of R3 (an issue after beta). The server id of
   a folder is stored so that a mailbox's address can be added beside it later.

### Identity

4. Every Graph request for mail asks for immutable ids (`Prefer: IdType="ImmutableId"`). A message is known
   by its immutable id, which stays the same when it moves between folders of the mailbox; a folder
   by its Graph id, which stays the same when it is renamed or moved. A folder's path is for
   showing only. The Message-ID is kept and never trusted to say two messages are the same.
   Requests for the calendar, the contacts and the Favorites keep Graph's ordinary ids: the mailbox
   files of 0.2.x and 0.3.x key events and contacts by them, and immutable ids there would make
   every event and contact look gone and new. Moving those to immutable ids waits for a step that
   re-keys them.
5. Everything taken in from the server is matched by server id, so a delta page that arrives twice
   (Microsoft warns that it can) changes nothing.

### Folders (R3.1)

6. The folder list is read in full each run by walking every mail folder's subfolders (folder delta
   isn't documented to cover every depth; folders are few). Hidden folders and search folders are
   not shown. Order: the well-known folders first, as Outlook orders them, then the rest
   alphabetically, nested. A folder's name may contain `/` or `\` (Exchange's IMAP leaves such
   folders out altogether; seen on a tester's mailbox, 2026-10-15): the tree is built from each
   folder's parent id, a name is never split into a path, and a path shown is the names joined.
7. New Folder, Rename, Move Folder reach the server. Delete Folder moves the folder under Deleted
   Items, as Outlook does.
8. A folder the complete listing no longer has is removed here with its mail, which the server no
   longer has either. A listing that didn't finish removes nothing.

### Mail down (R3.2)

9. Each folder is read by Graph's message delta, newest first (`$orderby=receivedDateTime desc`),
   in pages of 100 (`Prefer: odata.maxpagesize=100`; pages, not `$batch`), asking for the
   headers, the preview, read, flag, categories, importance, the folder and the Message-ID: enough
   for the message list. Deletions arrive as `@removed`. A first run reads the newest page of
   every folder first (the Inbox, then the well-known folders in Outlook's order, then the rest),
   then each folder to its end in the same order, so every folder shows mail within minutes. Each
   Send/Receive, by hand or on the timer, runs every folder's delta. All of an account's mail is
   kept; there is no "mail to keep offline" window.
   - A message the server moved arrives as `@removed` in one folder and as new in another, in
     either order and possibly in different runs. `@removed` marks the message gone from that
     folder and keeps it (marked so in the mailbox file); seen in another folder, it has moved
     there with what was fetched of it. Only a message still gone from every folder once every
     folder's delta has reached its end in a run is removed here.
   - Where the owner changed a property here that the server has not been told of yet (R3.3
     sends it), the server's value is taken only if the message's value here is still what the
     server last said (ADR 0003); the last synced state is updated either way. Nothing changed
     here is overwritten by coming down.
10. Each page is taken in as it arrives (the message list fills while the run goes on), and the
    folder's next-page link (or, at the end, its delta link) is stored with the folder in the same
    step as the page's messages, so the mailbox file never holds one without the other. The
    ordinary saving of the mailbox (at most every few seconds) writes them; a crash loses at most
    the pages since the last save, which come down again and change nothing (S5). Cancel,
    quitting, a crash or going offline carry on from the stored link next time: nothing twice,
    nothing missed. A round begun in an earlier run sees the mailbox as it was when it began, so
    at its end one more round (begun in this run) brings what changed since; only then is the
    folder current.
11. A delta link the server no longer accepts (`410 Gone` or `syncStateNotFound`) means that folder
    is listed afresh; what the complete new listing doesn't have is then removed here.
12. A message's whole content (its MIME, with attachments) is **fetched** in the background, newest
    first across the account, and at once when the owner opens it. Reply, Forward and saving a
    message fetch it first. Opening a message that isn't fetched while offline says it isn't on
    this computer yet and will download when online. Whether a message is fetched is part of the
    message, not a marker in its headers.
    - Each Microsoft account has "Download messages in full": Everything, or up to 10, 25 or 50
      MB (default up to 10 MB). A larger message is fetched only when opened, replied to,
      forwarded or saved, and the progress line says how many wait for that. Choosing a larger
      limit fetches the rest in the background; choosing a smaller one removes nothing already
      fetched. A message's size is Graph's extended property `Integer 0x0E08` (`PidTagMessageSize`),
      asked for with the delta; one whose size isn't said is fetched.
13. Search covers headers and previews of every message, and the whole text of fetched ones; while
    the background fetch isn't finished, results say how many messages weren't searched in full.
14. Microsoft allows 4 requests at once per mailbox. Background fetching uses at most 2; what the
    owner is waiting for (opening a message, sending, Send/Receive) goes first. A "busy" answer
    (429, 503 with `Retry-After`) pauses all Graph work for that account for as long as it asks.
15. Progress is shown in the status bar, as for IMAP: the folder being read with its count of
    messages from the folder listing, how many folders of how many, and separately how many
    messages are fetched of how many. Cancel stops after the page in hand, and stops all of the
    account's Graph work, the background fetch included, until the next Send/Receive.

### Changes up (R3.3)

16. Each message keeps what the server last said of it (its last synced state): read, flag,
    categories, importance and the server folder it is in. A run sends the difference between that
    and the message as it is now, per property: read, flag, categories and importance as one change
    of the message, a different folder as a move. A flag goes up with its due date (its start the
    day it was flagged, its completed date when marked complete). A copy and a delete for good wait
    in a queue (they can't be worked out by comparing). A run of a Microsoft account goes in this
    order: folder changes up (a move may name a folder made here), message changes up, the folder
    listing, mail down by delta, then fetching.
17. Where a message changed here and on the server since the last run, what was changed here wins
    for the properties it touched; the server's version for the rest. Categories merge: what was
    added and taken away here is applied to the server's list, so a category the server added
    meanwhile stays. A message moved here and also moved on the server goes where it was moved
    here. A message the server no longer has leaves here too, whatever was changed here, and that
    is said once (S20); the server's retention still has it.
18. Categories are applied by name. Their colours are read from Outlook's list of categories
    (`MailboxSettings.Read`, which a sign-in already asks for where it can be had); until it is
    readable a category keeps the colour chosen here. For a category on Outlook's list, Outlook's
    colour wins, and changing it here says "The colour is set in Outlook" and changes nothing on
    the server. A category made here that isn't on Outlook's list says so in the Categorize dialog
    ("Not in Outlook's list: no colour there"). The list is not written in R3.
19. **Deleting is Outlook's** (ADR 0003): Delete moves to Deleted Items; Delete for good (Shift+Del,
    emptying Deleted Items) removes from the folders with Graph's delete, recoverable for the
    server's retention period; Graph's `permanentDelete` is never used. Moving a server message into
    a local folder, or into another account, shows it there at once, marked as being moved; it is
    removed from the server only once it is fetched whole **and the mailbox file holding it is
    saved**. When the server says it no longer exists before it could be fetched, the copy here is
    kept as it is (its headers and preview), marked incomplete. Moving anything from another
    account or a local folder into a Microsoft account's server folder is refused with the reason
    until R3.4 can put a whole message there.
20. A change the server refuses (404 gone, 403 not allowed, 400 invalid) is put back to how the
    server has it and written in full to the Send/Receive log. The Send/Receive status says it once
    per run and kind of change, in plain words, naming the message when there is one ("Couldn't
    move 'Invoice 42' to Archive: the server says it no longer exists.") and counting when there
    are more ("Couldn't mark 3 messages as read: the server says they no longer exist."). A busy
    server or a lost connection is not a refusal: the change waits for the next run.

### Retrying and never twice (all steps)

21. One retry policy for all of Graph: reads (GET) and changes that are the same however often they
    are made (setting read, flag, categories, importance; delete) are retried with back-off,
    honouring `Retry-After`. A move or copy whose answer was lost is retried only after looking
    where the message is now: a move by the message's immutable id, which a move keeps; a copy,
    which gets an id of its own, by a marker. Before a copy, the source message is given an
    invisible extended property of MailCraft's with a value used once (the mechanism drafts use,
    S23); a copy carries it, so a lost answer is a search of the destination folder for that value,
    and only when nothing has it is the copy made again. `sendMail` and sending a draft are never
    retried by themselves.
22. Connections stay one per request. Changes of messages (PATCH, move, copy, delete) go up in
    `$batch` requests of up to 20, so that marking 10,000 messages read or emptying Deleted Items
    takes minutes, not half an hour. Each request in a batch is answered on its own, and is
    retried, put back or held by itself as S20 and S21 say; a batch that is throttled as a whole
    waits as one request does.

### Drafts and sending (R3.4)

23. A draft saved here is created in the server's Drafts carrying an extended property of ours, so
    a create whose answer was lost is found again instead of made twice. A reply or forward draft is
    created with Graph's create-reply/create-forward, so it stays in its conversation.
24. Saving again changes the server's draft in place: subject, body (HTML), To, Cc, Bcc and
    importance changed field by field; an attachment added here is uploaded (directly under 3 MB,
    else through an upload session, up to the server's message size limit), one removed here is
    removed. Headers such as `In-Reply-To` and `References` go in `internetMessageHeaders` where
    Graph allows.
25. Drafts go up when the owner saves or closes, and every 5 minutes while one is open.
26. A draft edited both here and elsewhere since the last run is kept twice: the server's version
    comes down as a second draft, nothing lost.
27. A draft on the server is sent with Graph's send-draft action; a message never saved as a draft
    with `sendMail`. The server files the copy in Sent Items; it comes down by delta, and MailCraft
    uploads no copy of its own. No SMTP, no fallback.
28. When it isn't known whether a message went (the answer was lost), or the server refused it, it
    becomes a **held message**: kept in the Outbox, not sent again by itself, with what happened
    and what to check ("may have been sent: look in Sent Items").

### Undo (R3.5)

29. Undo restores the mailbox as today; the next run sends the difference, which is the reverse
    change (a move back, a flag cleared, out of Deleted Items). The undo history is no longer
    forgotten when a change reaches a Microsoft account's server. Undo of a delete for good says it
    can't be undone here.

### Imported mail (R3.6)

30. Graph cannot put received mail into a mailbox folder as an ordinary message (what it creates is
    always a draft; its import API is for administrators, not for personal accounts, and not for
    MIME). Imported mail (`.eml`, `.mbox`) therefore goes to a local folder, and moving it into a
    server folder is refused with the reason: "Microsoft doesn't let a mail program put received
    mail into your mailbox; it stays in a local folder."

### Switching an account synced by IMAP (R3.7)

31. Once the switch has landed, each Microsoft account still on IMAP shows a page once: what will
    happen, with Switch and Later.
32. Switch: a last IMAP run pushes what is waiting first, and the switch waits if it can't
    (offline); the mailbox file is backed up beside itself with the date in its name; server folders
    and their messages are dropped and downloaded afresh through Graph; local folders are kept;
    drafts not on the server are uploaded as server drafts; MailCraft's own favourites are matched to
    the new folders by path.
33. Later keeps that account on IMAP until R3.8.

### Turning IMAP off for Microsoft accounts (R3.8)

34. IMAP and SMTP for Microsoft accounts are removed (code, permissions, the switch's IMAP half) when
    the author's mailbox has run on Graph for 7 days without a data-loss incident (R3.9) and at
    least 2 alpha testers have switched and used it for 7 days without one.

### Until R3 is whole

35. A Microsoft account becomes a Graph account only behind a switch for testing (an environment
    variable, or a hidden option) until R3.7 is merged; released builds stay on IMAP until then.
    Each task is one pull request, reviewed by a person (`needs-review`).

### Tests

36. Every done-when is proved against the Graph stand-in, which grows with each task: the folder
    walk and folder changes; message delta in pages with `@removed`, replayed pages and both kinds
    of expired link; MIME; message changes, move, copy, delete; create from MIME, create-reply,
    draft PATCH and attachments, send-draft, `sendMail`; filtering by extended property; `$batch`
    with answers mixed per request (done, refused, throttled); 429 with `Retry-After`; a connection that drops after the request went (for held messages); and a
    generated mailbox of 200,000 messages. No test touches a real mailbox; R3.9 is the author's own,
    by hand.

## Out of scope

Shared mailboxes and delegates; writing the category master list; a Sync Issues folder; push
notifications from Graph (the Inbox is looked at on the Send/Receive timer); IMAP accounts (the
milestone after beta); importing into server folders.

## Risks

| Risk | What we do |
|---|---|
| The background fetch of 200,000 messages takes hours under Graph's limits | The list is usable from headers in minutes; fetch newest first; measured against the stand-in and the author's mailbox |
| Graph's undocumented corners (where a plain delete goes, `$batch` with MIME, folder delta depth) | Requirements avoid relying on them; R3.9 observes the real service |
| Delta links expire unpredictably | A complete fresh listing per folder, removing only what a whole listing lacks |
| A send whose answer is lost | Held, never resent by itself |
| The permission change needs administrators to approve again | Said by the existing approval flow; the same consent list covers it |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-15 | major | First version, from a decision session with Martin Ball |
| 2026-10-15 | minor | S4: immutable ids are asked for on mail requests only; calendar, contacts and Favorites keep ordinary ids (found in R3.1, #39) |
| 2026-10-15 | minor | S9–S15 from the R3.2 decision session with Martin Ball: pages of 100, newest page of every folder first, every folder on each run, moves kept by server id, changes here never overwritten, pages saved with their links by the ordinary save, fetched as part of the message, "Download messages in full" per account (default up to 10 MB), progress in the status bar, Cancel stops all of the account's Graph work |
| 2026-10-15 | minor | S10: a round begun in an earlier run is followed by one more before the folder counts as current (found in R3.2.2) |
| 2026-10-15 | minor | S12: where a message's size comes from (found in R3.2.3) |
| 2026-10-15 | minor | S16–S22 and S36 from the R3.3 decision session with Martin Ball: flags with their due dates; categories merge when both sides changed them; a move here wins over one on the server, a message gone from the server goes here too; Outlook's category colours win, read with the `MailboxSettings.Read` a sign-in already asks for; a move out of the account removes the server's copy only once the whole message is saved here; moves into a server folder refused until R3.4; refusals said once per run and kind; the order of a run; a lost copy found by an extended property; message changes in `$batch` |
