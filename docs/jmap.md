# JMAP accounts

> **Last reviewed:** 2026-10-28 · **Last updated:** 2026-10-28 · **Change:** major

A JMAP account is one whose server speaks JMAP (RFC 8620 and RFC 8621) over HTTPS. MailCraft reads
such an account in the browser build. The desktop app and the command line have no way to reach a
JMAP server yet: the desktop app leaves such an account out of Send/Receive, and `mailcraft-cli
sync` refuses it.

## What works

- **Send/Receive** lists the server's mailboxes and reads the newest messages of each, as many as
  Messages to download per folder says (500 at the most): headers, marks and a preview. It runs when
  you ask for it and every few minutes by itself (then the Inbox and the folder on screen only),
  takes its turn in a Send/Receive of all accounts, and shows in Send/Receive Progress; Cancel stops
  it at once, the request under way with it.
- **A message comes whole** when you open it, before Reply, Reply All, Forward, saving it or one of
  its attachments, and in the background for the Inbox's forty newest (one larger than 10 MB waits
  until it is opened).
- **What the server changes comes down**: new mail, read marks, flags, answered and forwarded marks,
  and mail gone from a mailbox small enough to be read whole in a run (no more messages than
  Messages to download per folder, nor than the server takes in one request).

Not yet: nothing you change goes up to the server and nothing is sent from such an account. Moving or
deleting a message the server has, renaming, moving or deleting one of its mailboxes, and emptying
a folder that holds the server's mail, are refused with the reason until changes go up, since the
next Send/Receive would bring them back.

## The password, or the access token

The first Send/Receive asks for the account's password, or for its access token when the account
says the server takes a bearer token. MailCraft keeps it in memory until you close the page, the
server turns it down, the account's server address changes, the account is removed or another
mailbox is opened; it is never written to the browser's storage, the mailbox or a file. A server
that turns it down is not asked again by itself: its messages say so when you open them, and the
next Send/Receive you start asks for it again.

## Adding an account

There is no JMAP page in Add Account yet. An account comes into the browser build in one of two ways:

- **The page names it.** A site that serves MailCraft can name the accounts of its users before
  MailCraft starts. Those it holds the credential for are never asked for one (see
  [For a site that serves MailCraft](#for-a-site-that-serves-mailcraft)).
- **A mailbox file brings it.** Make one with the command line, then open it with File › Open
  (it takes the place of the mailbox kept in the browser) and confirm its servers:

  ```text
  mailcraft-cli run --cmd 'account.add={"email": "you@example.org", "kind": "jmap", "jmap": {"session": "https://mail.example.org/jmap/session"}}' --save jmap.mailcraft
  ```

  `"bearer": true` beside `"session"` when the server takes an access token rather than the
  password.

The session address is the address of the server's session resource, such as
`https://<server>/jmap/session`. It must be `https://` (plain `http://` only to this computer, for
trying things), and it must be the address itself: the browser build follows no redirect, so
`https://<server>/.well-known/jmap` will do only on a server that answers with the session there
rather than redirecting to it. Where the server redirects, the account's Send/Receive says to give
the session's own address.

## What the server has to allow

The browser asks the server from the page's own address, so the server has to let that page in
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
this computer for a browser:

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

## How it is built

- `crates/jmap` (`mailcraft-jmap`): the JMAP client, with no sockets and no browser APIs; it builds
  each request and judges each answer, and holds every answer to sizes. The in-process server
  (`standin`) is behind the `stand-in` feature, for tests.
- The engine works out what a run reads (`sync.jmapRequest`) and takes in what it brought
  (`sync.applyJmap`), and the same for messages fetched whole, only for the account and from the
  server it was asked for; it never reaches a server ([control-protocol.md](control-protocol.md)).
- The window hands a run to its host (`Services::jmap`, `Services::jmap_fetch`) with the
  credential, and takes the pieces in as they come.
- The browser build is that host (`apps/mailcraft-web/src/jmap.rs`), over `fetch`
  (`apps/mailcraft-web/src/fetch.rs`).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-28 | major | First version: reading a JMAP account in the browser build (M6, #226) |
