# JMAP accounts

> **Last reviewed:** 2026-10-28 · **Last updated:** 2026-10-28 · **Change:** major

A JMAP account is one whose server speaks JMAP (RFC 8620 and RFC 8621) over HTTPS. MailCraft reads
such an account in the desktop app and in the browser build, and takes what you change in its mail
up to its server; the desktop app reads its contacts (JMAP for Contacts, RFC 9610) and its calendars
(JMAP for Calendars) too, where the server offers them. The command line has no way to reach a JMAP
server yet: `mailcraft-cli sync` refuses it.

## What works

- **Send/Receive** lists the server's mailboxes and reads the newest messages of each, as many as
  Messages to download per folder says (500 at the most): headers, marks and a preview. It runs when
  you ask for it and every few minutes by itself (then the Inbox and the folder on screen only),
  takes its turn in a Send/Receive of all accounts, and shows in Send/Receive Progress; Cancel stops
  it at once, the request under way with it (in the desktop app, nothing more is asked once you
  press it, and a request already on its way is let finish within its time limit).
- **A message comes whole** when you open it, before Reply, Reply All, Forward, saving it or one of
  its attachments, and in the background for the Inbox's forty newest (one larger than 10 MB waits
  until it is opened).
- **What the server changes comes down**: new mail, read marks, flags, answered and forwarded marks,
  and mail gone from a mailbox small enough to be read whole in a run (no more messages than
  Messages to download per folder, nor than the server takes in one request).
- **The account's contacts come down**, in the desktop app, from a server that offers JMAP for
  Contacts (RFC 9610): every contact of every address book of the account, 20,000 at the most, with
  names, company and title, e-mail addresses, phone numbers, postal addresses, web addresses,
  birthday and notes. People shows them beside the contacts kept on this computer. They are read by
  themselves after the account's first Send/Receive and after each one you ask for, and otherwise
  every quarter of an hour while Send/Receive runs by itself, with the password or token the
  Send/Receive went with (nothing asks you for one for the contacts alone). A contact the server no
  longer has goes from here once the whole list could be read; a list read in part only adds and
  updates. A server that offers no contacts is not asked again until the program starts anew or
  you ask for the account's Send/Receive. They are read only: a contact the server keeps is not
  changed or deleted here, as a Microsoft account's is not. A contact's picture is not read, and no
  address in a contact is fetched. Groups of contacts are passed over.
- **The account's calendars are shown**, in the desktop app, from a server that offers JMAP for
  Calendars: the calendars the account subscribes to and may read (100 at the most, the events of 64), each with
  its
  name and colour, and their events from a month back to a year ahead (20,000 of a calendar at
  the most): subject, place, notes, start and end with the time zone, whole-day events, show-as,
  private, categories, the reminder, and the people invited with what they answered. They are read
  after the contacts, at the same times, and also when you bring the calendar to the front and the
  last reading is more than two minutes old. An event the server no longer has in that stretch goes
  from here once the calendar's whole list could be read. They are read only: every such calendar
  is one you can't change here, nothing made here goes into one, and nothing of an event is written
  to the server. A repeating event comes as its occurrences in the stretch,
  each an event of its own (the server works them out); the series as one repeating event is
  [#1090](https://github.com/UCS-Care-Group/mailcraft/issues/1090).

- **What you change goes up** (#1074): a read mark, a flag, the answered and forwarded marks, a
  move between two of the server's mailboxes (Delete is a move into its Deleted Items) and a
  deletion for good. They go with the next Send/Receive, before it reads, and one starts by itself
  a few seconds after the change once the password or token is at hand. Only the marks you changed
  are touched; the server's other keywords stay. A change the server refuses is put back to how
  the server has it and said once, in the status bar and in Send/Receive Progress, and so is a
  change to a message the server no longer has.
- **A flag is the server's `$flagged` and nothing else.** A JMAP server keeps no due date and no
  "complete": a follow-up's due date stays on this computer, and one marked complete is not flagged
  there. So whether a flag was changed here is told by that one bit. A flag you didn't change since
  the last Send/Receive follows the server: cleared on another device, it is cleared here with its
  due date; flagged again there after you marked it complete, it is flagged here. One you changed
  here goes up, whatever the server said meanwhile. Changed on both sides between two runs, the two
  can only have gone the same way, and the due date or the "complete" kept here stays.
- **A move takes the message out of the mailbox it was in and puts it into the other**, and says
  nothing of any other mailbox (`mailboxIds/<from>` and `mailboxIds/<to>`, never the whole of
  `mailboxIds`). That way it doesn't undo what another program did to the message meanwhile, and
  never takes it out of a mailbox this computer didn't know it was in (a server may keep a message
  in several). The other side of it: a message another program moved on in the meantime, or one
  that lay in a second mailbox already, ends up in both. The next Send/Receive that reads those
  mailboxes lists it where the server has it, and it is shown there (in one of them, when the
  server keeps it in two).

Not yet: nothing is sent from such an account, and what you change in its folders doesn't go up.
Renaming, moving or deleting one of the server's mailboxes, deleting a folder that holds the
server's mail, and moving the server's mail into a folder the server hasn't got (one made here,
another account's) are refused with the reason, since the next Send/Receive would bring them back.
What you write in a draft the server has stays on this computer.

## The password, or the access token

The first Send/Receive asks for the account's password, or for its access token when the account
says the server takes a bearer token. The desktop app keeps it in memory until it closes: it is
never put in the operating system's credential store, whatever File › Options says of remembering
sign-ins, nor in the mailbox or any other file, so it is asked for again at the next start. The
browser build keeps it in memory until you close the page, the
server turns it down, the account's server address changes, the account is removed or another
mailbox is opened; it is never written to the browser's storage, the mailbox or a file. A server
that turns it down is not asked again by itself: its messages say so when you open them, and the
next Send/Receive you start asks for it again.

## Adding an account

There is no JMAP page in Add Account yet. An account comes into the desktop app from a mailbox file
(below), and into the browser build in one of two ways:

- **The page names it.** A site that serves MailCraft can name the accounts of its users before
  MailCraft starts. Those it holds the credential for are never asked for one (see
  [For a site that serves MailCraft](#for-a-site-that-serves-mailcraft)).
- **A mailbox file brings it.** Make one with the command line, then open it with File › Open
  (in the browser build it takes the place of the mailbox kept there) and confirm its servers:

  ```text
  mailcraft-cli run --cmd 'account.add={"email": "you@example.org", "kind": "jmap", "jmap": {"session": "https://mail.example.org/jmap/session"}}' --save jmap.mailcraft
  ```

  `"bearer": true` beside `"session"` when the server takes an access token rather than the
  password. Without it, the password goes with the account's address as the user name (Basic).

The session address is the address of the server's session resource, such as
`https://<server>/jmap/session`. It must be `https://` (plain `http://` only to this computer, for
trying things). The desktop app follows a redirect of the session within the same server (scheme,
name and port), at most four, and sends the credential nowhere else. In the browser build it must
be the address itself: the browser build follows no redirect, so
`https://<server>/.well-known/jmap` will do only on a server that answers with the session there
rather than redirecting to it. Where the server redirects, the account's Send/Receive says to give
the session's own address.

## What the server has to allow

The desktop app needs nothing of the server beyond JMAP over HTTPS, with a certificate the
operating system trusts: one it doesn't is refused, and there is no way yet to trust a server's own
certificate for a JMAP account, as there is for an Exchange server account.

In the browser build, the browser asks the server from the page's own address, so the server has to let that page in
(CORS):

- answer the preflight (`OPTIONS`) of `GET` and `POST` with the fields `Authorization`,
  `Content-Type` and `Accept` allowed, for the page's origin;
- send `Access-Control-Allow-Origin` with every answer, the session's, the API's and a download's;
- send `Access-Control-Expose-Headers: Retry-After`, so that what a busy server asks is read: the
  status line then says how long it asked to wait, and the background download of whole messages
  waits at least that long.

MailCraft sends no cookies (`credentials: "omit"`), keeps nothing in the browser's HTTP cache and
sends no `Referer`. Every address the session names (the API, downloads, uploads) must be on the
same origin as the session address (scheme, name and port): MailCraft sends the credential nowhere
else.

## For a site that serves MailCraft

A page that serves the browser build can name JMAP accounts, and hold their credential itself, with
an object it sets before MailCraft starts:

```js
window.mailcraftJmap = {
  accounts: [
    { email: "ada@example.org", session: "https://mail.example.org/jmap/session", bearer: true, name: "Work" },
  ],
  // Optional: the credential of one of those accounts, a string or a promise of one, within
  // thirty seconds.
  secret(email, session) { return currentToken(email); },
};
```

- The accounts are added to the mailbox when it has none at that address. With accounts named, a
  first visit starts from an empty mailbox rather than the sample.
- With `secret`, MailCraft never asks the owner for the credential of one of those accounts, at the
  server the page named (an account the owner points at another server is the owner's to sign in
  to); each Send/Receive and each download asks the page for it, and a run the server turns it
  down for asks once more before it ends, so that the page can hand over a renewed one. MailCraft
  keeps none of it beyond the job. When the server still turns it down, no Send/Receive starts by
  itself for that account until the owner asks for one.
- Without `secret`, the owner types the credential as for any JMAP account.

## Trying it

The command line's tests have the JMAP server they test against, in the process, served over HTTP on
this computer, for the desktop app or a browser:

```text
cargo test -p mailcraft-cli --test jmap_loopback -- --ignored --nocapture
```

It serves `http://127.0.0.1:8780/jmap/session` with a few mailboxes and messages, lets in the bearer
token `standin-token`, and answers a page from any origin. Name the account for the browser build
in a script at the top of the `<head>` of `apps/mailcraft-web/index.html` (for the try only):

```html
<script>
  window.mailcraftJmap = { accounts: [{ email: "ada@example.org", session: "http://127.0.0.1:8780/jmap/session", bearer: true }] };
</script>
```

then serve it (`trunk serve` in `apps/mailcraft-web`) and open it: Send/Receive asks for the token.

For the desktop app, make a mailbox file with the account as [Adding an account](#adding-an-account)
says, with `"session": "http://127.0.0.1:8780/jmap/session"` and `"bearer": true`, open it with
File › Open, and give the token when Send/Receive asks.

To run the desktop app's JMAP code against a real server instead, in a throwaway container on
this computer, see [live-tests.md](live-tests.md) (`cargo xtask live`, opt-in).

## How it is built

- `crates/jmap` (`mailcraft-jmap`): the JMAP client, with no sockets and no browser APIs; it builds
  each request and judges each answer, and holds every answer to sizes. The in-process server
  (`standin`) is behind the `stand-in` feature, for tests.
- The engine works out what a run reads (`sync.jmapRequest`) and takes in what it brought
  (`sync.applyJmap`), and the same for messages fetched whole, only for the account and from the
  server it was asked for; it never reaches a server ([control-protocol.md](control-protocol.md)).
- The window hands a run to its host (`Services::jmap`, `Services::jmap_fetch`) with the
  credential, and takes the pieces in as they come.
- Contacts are a job of their own (`Services::jmap_contacts`, `crates/ui-egui/src/jmap_contacts.rs`):
  `mailcraft_jmap::sync_contacts` lists the cards (`ContactCard/query`) and reads them
  (`ContactCard/get`), each JSContact card (RFC 9553) made into a contact within bounds
  (`crates/jmap/src/contacts/card.rs`), and the engine merges them as it does a Microsoft account's
  (`cmd::remote::apply_contacts`). Only the desktop app is a host for it so far. What is the same
  for every kind of data read this way has one home in each layer: the list page by page and the
  `get` that is halved until the item that breaks it is found (`crates/jmap/src/reader.rs`,
  `Calls::read_all`), when such a job is due in the window and how long it waits after one that
  came to nothing (`crates/ui-egui/src/jmap_side_job.rs`), and the thread and the sign-in of the
  desktop's job (`on_thread` and `signed_in` in `apps/mailcraft/src/jmap_runner.rs`).
- Calendars are a job of their own too (`Services::jmap_calendar`,
  `crates/ui-egui/src/jmap_calendar.rs`): `mailcraft_jmap::sync_calendar` lists the calendars
  (`Calendar/get`), then for each lists the events of the stretch with what repeats expanded by
  the server (`CalendarEvent/query`) and reads them (`CalendarEvent/get`), each JSCalendar event
  (RFC 8984) made into an event within bounds (`crates/jmap/src/calendars/event.rs`); the engine
  takes the list and the events in as it does a Microsoft account's (`cmd::remote::apply_calendars`,
  `apply_events`), every calendar as one the account only reads. The events of a calendar are
  listed and read by the code the contacts are (`reader.rs`), and the job in the window and on the
  desktop goes through the same `jmap_side_job.rs`, `on_thread` and `signed_in`.
- The desktop app is that host (`apps/mailcraft/src/jmap_runner.rs`), over `mailcraft_net::https`,
  each job on a thread of its own; Stop is heard before each request.
- The browser build is that host too (`apps/mailcraft-web/src/jmap.rs`), over `fetch`
  (`apps/mailcraft-web/src/fetch.rs`).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-28 | major | First version: reading a JMAP account in the browser build (M6, #226) |
