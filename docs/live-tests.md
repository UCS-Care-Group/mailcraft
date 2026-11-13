# Live tests against a real mail server

> **Status:** opt-in; run by hand, never by `cargo test`, `cargo xtask ci` or a workflow · **Written:** 2026-11-12 · **Last updated:** 2026-11-12 · **Change:** major

`cargo xtask live` runs the desktop app's JMAP code against a real mail server in a throwaway
Docker container on this computer. It is for whoever changes that code and wants to see it work
against a server before proposing the change. Nobody has to run it, and nothing depends on it.

## Why it exists beside the scripted tests

The unit tests script the server: scripted exchanges, and the stand-in server in `crates/jmap`.
They prove that the client does what we think the protocol asks. They cannot show that a server
answers the way the client expects, because the script was written by the same people as the
client.

A run against a real server has already found one such case. A server answered `Email/set` with
`destroy` for an id it did not know with neither `destroyed` nor `notDestroyed`. A client that
keeps to RFC 8620 then has no answer for that id and asks again at every run, for ever. No
scripted test would have found it, since nobody would have written that script.

The scripted tests stay the tests of record: they run everywhere, need nothing, and cover the
failures a healthy server never shows. The live run is one more look, at the one thing they
cannot see.

## Running it

It needs Docker, and nothing else that building MailCraft does not need.

```text
cargo xtask live
```

The first run pulls two images. Every run builds the desktop app's tests, then prints one line
for each test and the usual summary. `cargo xtask live --keep` leaves the two containers running
afterwards, for a look at them with a browser; the next run removes them first.

## What it starts and what it removes

- A Docker network of its own, `mailcraft-live`.
- A mail catcher, `mailcraft-live-catcher` (`axllent/mailpit`, MIT licence). It is the server's
  smart host, so that mail the server sends goes to the catcher and nowhere else. Its web page is
  published on `127.0.0.1:58125`.
- The server, `mailcraft-live-jmap`, published on `127.0.0.1:58180`. On its first start it makes
  one mailbox, `ada@example.org`, with a password that is made up for the run and never printed.
- Then the tests: `cargo test --locked -p mailcraft --bin mailcraft live_ -- --ignored
  --test-threads=1`, with the server's address, the mailbox's address and its password in the
  environment of that one process.
- Afterwards both containers and the network are removed, whatever came of the tests. The mailbox
  goes with its container.

Both ports are published on loopback only: nothing outside this computer can reach either
container.

## What it never does

- It is not part of `cargo test` or `cargo xtask ci`: every test of it is `#[ignore]`d, and no
  workflow runs it. CI never pulls either image.
- It adds nothing to the build and nothing to `Cargo.lock`. The harness is `xtask/src/live.rs`,
  on the standard library alone; the tests use what the desktop app has already.
- No test touches a real mailbox. The tests refuse a server that is not on this computer
  (`localhost`, `127.0.0.1` or `::1`) before a byte is sent, since they write to the mailbox they
  are given. Never give them a real one.
- Without the harness's environment a test fails and says so, rather than pass having tested
  nothing.

## What is tested

The tests are in `apps/mailcraft/src/live_tests.rs`. Each puts something into the test mailbox
the way another client would (a JMAP `set` call), reads it back through the functions the
window's jobs run (`jmap_runner::run_*_with`), and looks at what they made of it.

| Test | What it shows |
|---|---|
| `live_mail_is_listed_and_a_message_comes_down_whole` | A Send/Receive lists the folders and finds the Inbox; a message put there is listed with its subject and sender; it is fetched whole, as the bytes of a message |
| `live_contacts_come_down_with_their_fields` | A contact card put into the account's address book comes down with its name, company, address, phone number and note, in a list that is complete |
| `live_calendars_and_their_events_come_down` | The account's calendars are listed, one of them the main one and each read only; an event put there comes down at its time, with its length and place |

A server has to offer mail. Where its session names no account for contacts
(`urn:ietf:params:jmap:contacts`) or for calendars (`urn:ietf:params:jmap:calendars`), that test
prints a line starting with `live: skipped` and ends there; the test run then counts it as
passed, so read the lines when you try another server.

A feature that reaches a server gets its test here in the pull request that brings the feature.

## Another server

The harness is not tied to one server. These variables change what it starts:

| Variable | What it is | When it is not set |
|---|---|---|
| `MAILCRAFT_LIVE_IMAGE` | The server image | `nextxch/server:1.3.3` |
| `MAILCRAFT_LIVE_ENV` | The image's start recipe: what the container's environment holds, as `NAME=value` entries with white space between them | The default image's recipe |
| `MAILCRAFT_LIVE_SERVER_PORT` | The port the server speaks HTTP on inside its container | `80` |
| `MAILCRAFT_LIVE_PORT` | The loopback port the server is published on | `58180` |
| `MAILCRAFT_LIVE_CATCHER_IMAGE` | The mail catcher's image | `axllent/mailpit:latest` |
| `MAILCRAFT_LIVE_CATCHER_PORT` | The loopback port the catcher's web page is published on | `58125` |

In `MAILCRAFT_LIVE_ENV`, `{email}` stands for the mailbox to make (`ada@example.org`),
`{password}` for the run's password and `{catcher}` for the catcher's host name on the network
of the two. Write `{password}` there and never a password of your own. An entry cannot hold
white space.

What an image has to offer:

- **One mailbox made on first start from environment variables**: an address and a password it
  is told, with nothing to click and no second command.
- **HTTP on one port**, without TLS (the port is published on loopback only), with the JMAP
  session at `/.well-known/jmap` or a redirect from there on the same origin, and the mailbox's
  address and password taken as HTTP Basic credentials.
- **An outbound smart host**: a setting that sends all outgoing mail to one host, in the clear,
  on port 1025, where the catcher listens.

A catcher other than the default has to take mail on port 1025 and serve its web page on 8025.

The harness passes the tests three variables, which you can also set yourself to run the tests
against a server you started on this computer by other means (`cargo test -p mailcraft --bin
mailcraft live_ -- --ignored --test-threads=1`): `MAILCRAFT_LIVE_SESSION` (the session's address,
on loopback), `MAILCRAFT_LIVE_EMAIL` and `MAILCRAFT_LIVE_SECRET`.

## The default image

The image a run starts when none is named is `nextxch/server` from Docker Hub. It is the mail
server that the author of this harness works on. It is **commercial software under its own
licence, free to pull and to run for a few mailboxes, and not open source**. It is the default
because it offers JMAP for mail, contacts and calendars on one origin, and because its author
can fix it when a test finds a fault on its side.

Nothing of it is in this repository or in any build of MailCraft; it is pulled by Docker, on the
computer of whoever runs `cargo xtask live`, and by nothing else. The tests ask the session what
the server offers and are about the protocol, not about that server: a test that passes only
against one server is a fault in the test.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-12 | major | #1113: first version: what the live tests are, how to run them, what they start and remove, another server, the default image |
