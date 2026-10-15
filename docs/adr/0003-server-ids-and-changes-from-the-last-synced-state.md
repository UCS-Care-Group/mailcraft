# 0003. Messages and folders are known by their server id; changes are worked out from the last synced state

> **Status:** proposed · **Date:** 2026-10-15 · **Deciders:** Martin Ball (co-owner), billsecond (co-owner)

## Context

Graph mail ([ADR 0002](0002-graph-mail-for-microsoft-accounts.md)) replaces IMAP for Microsoft
accounts. Today a message is known by its IMAP number (uid, with the folder's UIDVALIDITY) and its
Message-ID, and a folder by its path. A folder renamed or moved in another program then looks like
one folder gone and another new, which loses the changes waiting in it and fetches its mail again
(found in the review of #9). Message-IDs are sometimes missing and sometimes shared.

Changes made here go up two ways today: read and flag changes are found by comparing a message with
what the server last said of it, while moves and deletes wait in a queue. Undo restores a snapshot
of the whole mailbox, and once a change has reached the server the undo history is forgotten,
because nothing would send the reverse change.

## Decision

1. **Server ids.** For a Microsoft account, a message is known by Graph's immutable id (asked for
   with `Prefer: IdType="ImmutableId"` on every mail request; calendar and contacts keep Graph's
   ordinary ids until a later step re-keys them), which stays the same when the message
   moves within the mailbox, and a folder by its Graph id, which stays the same when it is renamed
   or moved. A folder's path is kept for showing only. The Message-ID stays a field of the message
   and is never trusted to say two messages are the same.
2. **Changes from the last synced state.** Each message keeps what the server last said of it: read,
   flag, categories, importance and the server folder it is in. A synchronisation sends the
   difference between that and the message as it is now, per property. Only what can't be worked
   out by comparing (a copy, a delete for good) waits in a queue. Where a message changed both here and on the server
   since the last run, what was changed here wins for the properties it touched, and the server's
   version for the rest; categories merge, what was added and taken away here being applied to the
   server's list, so that a category added on the server meanwhile isn't lost.
3. **Undo is another change.** Undo restores the snapshot as today; the restored state differs from
   the server's, so the next run sends the reverse change. The undo history is no longer forgotten
   when a change reaches the server.
4. **Taking in is by server id**, so a delta page that arrives twice changes nothing, and a folder
   listed afresh after its delta link expired removes only what a complete listing no longer has.
5. **Deleting is Outlook's.** Delete moves a message (or a folder) to Deleted Items. Delete for good
   (Shift+Del, or emptying Deleted Items) removes it from the folders the way Outlook does, so it
   stays recoverable for the server's retention period; MailCraft never uses Graph's
   `permanentDelete`, which goes past that. Moving a server message into a local folder copies it
   there whole and then removes it from the server, as moving to a PST file does. A draft is
   changed in place on the server, not replaced.

## Alternatives

- **Paths for folders, Graph ids for messages**: smaller change; keeps the rename bug.
- **Message-ID for everything**: no new field; wrong whenever a Message-ID is missing or shared.
- **Never remove anything from the server's folders** (moving to a local folder copies; no delete
  for good): safer, but not what an Outlook owner expects, and harder to judge against Outlook on
  the web.
- **Queue every change** (as IMAP moves and deletes are today): explicit, but a crash between doing
  a change here and queuing it loses it, and undo needs a reverse operation for every kind.

## Consequences

- The mailbox file gains a server id on messages and folders, a delta link per folder and a
  fuller last-synced state on messages; files written before open with these empty.
- Accounts synced by IMAP are downloaded afresh at the switch (ADR 0002), so no IMAP uid is ever
  translated into a server id.
- Shared mailboxes (after beta) need the mailbox's address beside each folder's id; the id is
  stored with room for it.
- IMAP accounts keep their uid-based identity until the milestone after beta.

## Amendments

- 2026-10-15 (R3.3 decision session, Martin Ball): categories merge as sets when both sides
  changed them (decision 2). Taking the list here whole would drop a category added in Outlook
  meanwhile, which is a data-loss incident.
