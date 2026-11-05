# Exchange servers other than Microsoft's

> **Last reviewed:** 2026-11-04 · **Last updated:** 2026-11-04 · **Change:** minor

An Exchange server account (#252) reaches an organisation's own mail server through Exchange Web
Services: its mail, and now its calendars, address books and to-do lists, including a person's
calendar or address book opened by address. This page says what has been seen working on servers
other than Microsoft's, one at a time, and what has not. Everything here was tried with made-up
data on a server running on the same computer; no real mailbox was used.

The directory is read after the mail of a Send/Receive: the account's calendars, books and lists
(the folders the server gives that kind), the events of the days on screen and a week either side,
and every contact and to-do. What is made, changed or deleted here goes up before it is read.
Another mailbox's calendar or address book is read with the owner's sign-in by putting its address
in every folder request; it is listed under Shared Calendars or with the account's books. Grommunio
lets the owner into the shared folder it is given but not list that mailbox's other folders, so a
shared calendar or book is read as its default folder there.

## Grommunio

**Seen:** Grommunio's own container deployment, Gromox 3.13.33 (October 2026), one domain, three
made-up mailboxes, TLS with a self-signed certificate, sign-in with Basic. The window itself was
not used for the answers below: the account was added and driven through the control channel (the
screen was locked), so the certificate question and the Add Account pages were not looked at
against this server.

| What | Result | Notes |
|---|---|---|
| Autodiscover | Reader works; the search was not run end to end | The server answers `/autodiscover/autodiscover.xml` with a Basic challenge, then with its Exchange Web Services address (`EXHTTP`, `EXCH`; the `EXPR` protocol has no address, and the `EXCH` server is a mailbox identifier, not a host). MailCraft reads that shape (a test keeps it). The test domain had no name that led to the server, so the three places Autodiscover looks were not tried from an address; the server's address was typed instead |
| Certificate by fingerprint | Works | A certificate the system does not trust ended the first run with the owner's question, and nothing was sent to the server meanwhile. With that one certificate's fingerprint trusted, every request went through, and the log has no line about the system's check any more (it used to note a failure on every request) |
| Sign-in: Basic | Works | |
| Sign-in: NTLM | Not offered | The server's challenge lists Basic only, so NTLM was not tried against a real server |
| First download | Works | Folders, then headers with a preview, then whole messages; an accented subject and body, an attachment and a message in a folder made on the server all arrived. The server's folders for calendar, contacts, tasks, notes, journal and settings are left out: Grommunio's Default listing gives no folder class, so MailCraft now asks for it by name and also leaves out the calendar, contacts, tasks and search elements whatever class they say (before this, they showed as mail folders) |
| Delta after a new message | Works | A message delivered after the first run came with the next one, and nothing came twice |
| Read, flag, categories | Work | Seen on the server afterwards. Importance of a message already received cannot be changed from here, so it was not tried |
| Move, copy | Work | Between folders made on the server |
| Delete | Works | To Deleted Items, or for good (`SoftDelete`) from Delete Permanently |
| Folders | Work | A folder made under Inbox, renamed, and deleted (it goes to Deleted Items) |
| Move to Archive | Works (fixed) | Grommunio has no Archive folder. When mail is put in the Archive of an Exchange account, MailCraft makes a folder named Archive at the top of the mail, once, and the move follows in the same run. A server that has such a folder already gives it to the Archive by its name. Tried again after the fix: one Archive on the server holding the message, and none made on later runs |
| Draft saved and edited | Works | One draft on the server, changed in place; High importance kept |
| Send to other local users | Delivered once | To and Bcc recipients each got it once |
| Bcc | Does not leak | The To recipient's copy has no Bcc, and the Bcc recipient got the message |
| Copy in Sent Items | Works (fixed) | Sending the saved draft with `SendItem` delivered it but filed no copy in Sent Items and left the draft in Drafts. Every send, a saved draft too, now goes as one `CreateItem` with `SendAndSaveCopy` and the message's own content, and the draft is then taken away with a soft delete, only after a clear success. Tried again after the fix: delivered once to the To and Bcc recipients, one copy in Sent Items, no draft left. **Not yet checked against Exchange Server itself**, which filed the copy for `SendItem` and is expected to do the same here |
| Undo of a move | Works | The message went back to where it was, on the server too |
| Calendar read | Works | The account's main calendar came down with its subject, place, body, times, importance and people invited; a shared calendar's events came too |
| Contacts read | Works | Names, company, job title, e-mail addresses, phone numbers and notes came down. A physical address did not: Grommunio answers `FindItem` without `PhysicalAddresses` even when it is asked for (see Not fixed) |
| To-dos read | Works | Subject, notes, start, due, importance and status came down |
| Calendar made, changed, deleted | Works | An appointment made in the window arrived on the server; changing its title, place and time and then deleting it was seen on the server after the next Send/Receive |
| Contact made, changed, deleted | Works (fixed) | A contact made here arrived, a change to its company and title arrived, and a deletion took it from the server. A deletion first sent no request at all (it was taken for a card that couldn't be read); with that fixed it goes as one `DeleteItem` |
| To-do made, changed, deleted | Works | A to-do made here arrived in the server's Tasks folder; a change to its due date and importance arrived, and a deletion took it away |
| Shared calendar by address | Works | The calendar of another mailbox that gave the owner reading rights came down after Open Shared Calendar, and its events showed. Grommunio refuses `FindFolder` on the shared mailbox's top, so only its default calendar is found there (its other calendars wait for a server that lists them) |
| Shared address book by address | Works | The address book of another mailbox that gave the owner reading rights came down as a book of its own, named after its owner; the same `FindFolder` limit applies |

This table's calendar, contact and to-do rows were tried on a Grommunio container (gromox-core,
openSUSE Leap 16, 2026-11-04) through the desktop window's control channel, with two made-up users
and the window's own commands; the server was checked directly with `FindItem` afterwards. None of
it has been tried against Exchange Server itself.

### How sending keeps to "never twice"

Within a run the send is one request that is never repeated. An answer that is lost or unclear holds the message as "may have been sent", and the draft is kept. The draft is taken away only after a clear success; a draft whose make was never answered is found by its marker first, and if it cannot be found or taken away the next run deletes it, and sends nothing.

This does not close every window. If MailCraft is killed, or the computer loses power, between the server's answer and the moment the answer is taken in, the message is still in the Outbox and the next run sends it again. It is the same for Microsoft accounts. Look in Sent Items first if a run ended that way.

### Not fixed

- A Calendar (or Contacts, Tasks) folder taken in as a mail folder by the earlier trial build stays in the folder list as an empty stub on a delta run, until the next fresh read of the folders. It only affects accounts added with that build, which needed a testing switch to add one.
- Grommunio's `FindItem` does not return a contact's physical addresses, even when `contacts:PhysicalAddresses` is asked for; the other fields of a contact do come. A server that returns them is read as any other field.
- A shared mailbox's other calendars and address books are not found on Grommunio, which refuses `FindFolder` on that mailbox's top: only the default calendar and address book of a person whose folder is shared are read.
- A recurring series is read as the occurrences the server gives and the master when its rule can be said here; whether Exchange Server and Grommunio agree on the series' time zone over a daylight-saving change has not been tried.
- Changing a shared calendar's or address book's items needs the same editing rights Grommunio's `grant` names; only reading was tried there.

## Exchange Server 2019

**Seen:** Exchange Server 2019 (a current cumulative update), on premises, reached through Exchange Web
Services over HTTPS with a certificate the system trusts (so no fingerprint was needed), release 0.10.0,
Linux x86_64 build. The mailbox was a real one kept for testing, about 24,500 messages, nearly all in
Sent Items. It was reported by a tester, who ran it without a screen in a container whose only network
access was the Exchange server on port 443, driven through the control channel: `account.add` with
`"kind": "exchange"` and the server's address typed in (no Autodiscover), then `ui.sync` with the
password, then reads. Nothing was changed on the server: one Send/Receive, no flags, moves, drafts or
sends.

| What | Result | Notes |
|---|---|---|
| Sign-in: Basic over TLS | Works | NTLM was not tried |
| Certificate | Works | Trusted by the system; no fingerprint question |
| First download | Works | 24,574 messages as headers with a preview in 82.7 s; the whole messages then came down in the background ("Downloading 3052 of 24574" a few seconds later) |
| Folder counts | Exact | The 7 mail folders the server lists (Inbox, Drafts, Sent Items, Deleted Items, Junk Email, Outbox, Conversation History) each match the server's own `FindFolder` `TotalCount` |
| Archive | Extra, empty | MailCraft also shows an empty Archive, which the server does not list; being looked at |
| Non-mail folders | Left out | "20 folders that are not mail folders are left out" |
| A message in full (`mail.get`) | Works | Body, HTML and attachments present |
| Calendar, contacts | Not there | As this page says (#720) |
| Crashes | None | 8 log lines |

Two small things were seen and are tracked apart from this page: the folder pane showed Outbox
twice for the account, and `sync.progress` gave empty `timings` for it.

**Not tried:** NTLM, Autodiscover from an address, any change sent up (read, flag, move, draft, send),
the Windows installer, and the window used by hand.

## Not yet tried

Exchange Server Subscription Edition, SmarterMail, changes sent up and sending against Exchange Server itself, NTLM against a real server, Kerberos
and modern sign-in (not built), a shared mailbox, and Autodiscover from an address.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-04 | patch | #746: Exchange Server 2019 seen working for mail (Basic over TLS, 24,574 messages, exact folder counts), and taken off the not yet tried list |
| 2026-11-04 | minor | #252: calendars, contacts and to-dos kept in step both ways, and a person's calendar or address book opened by address; tried on a Grommunio container, including the shared folders and the limits there |
| 2026-11-01 | minor | #252: Exchange server accounts are on for everyone, for mail; the testing switch is gone |
| 2026-10-30 | major | First version: what was seen on Grommunio (#252, E8) |
| 2026-10-30 | minor | Sending a saved draft, the Archive folder, the folder class and the log line, fixed and tried again on Grommunio |
| 2026-10-30 | patch | Review: a draft whose make was lost is found by its marker; the log line is left out only for the pinned certificate; the "never twice" wording; one stale stub noted |
