# File formats and protocols

> **Last reviewed:** 2026-10-11 · **Last updated:** 2026-10-19 · **Change:** minor (JMAP mail (RFC 8620, 8621) read over HTTPS, M6; vCard 2.1's raw 8-bit values in their own `CHARSET`, #185; the word index beside the mailbox file, R6.2; vCard files in every character set of the WHATWG Encoding Standard, #180; mail in every character set of the WHATWG Encoding Standard, #178; the mailbox file in format 3, a log of records, R6; files kept aside never removed; earlier: time zones in the mailbox file and in `.ics`) · **Target:** Microsoft Outlook (Microsoft 365, classic desktop)

What MailCraft reads, writes and speaks, and how far each goes. Everything is implemented from the
public specifications; every reader is fed arbitrary bytes by property tests and must never panic.

## Files

| Format | Read | Write | Notes |
|---|---|---|---|
| **`.mailcraft`** (the mailbox) | yes | yes | One self-contained JSON file (format 1: exports, the browser, Save As), or on the desktop a log of records (format 3: the rest of the mailbox and one record per message, contact, event or task, each with a checksum, closed by commits; a save appends what changed) with bodies and attachments over a kilobyte in `<file>.blobs/` named by their SHA-256. Beside it, `<file>.words` is the word index a search answers from (R6.2): a log of checksummed records (words, each message's words and the parts they are in, messages taken out), written again whole without what was taken out; it is derived data, so a damaged file or one of another version is thrown away and built again, and the file is for its owner only. Formats 1 and 2 (JSON with the content folder) open and become format 3 at the first changed save, the old file kept beside it (MailCraft 0.4.3 and earlier don't read format 3 and call it not a mailbox: they open that kept file; this build says a file of a newer form is from a newer MailCraft). A damaged record ends the reading at the save it is in, or, in the first save, costs only its item (the folders' fetch marks are then forgotten, so the mail comes again); what was left out is said and the file is kept aside (MailCraft never removes a file it kept aside). `repair()` makes any file consistent. Event times are clock times, and the file says by which zone's clock (`zone`, an IANA name); an event may name zones of its own (`startZone`, `endZone`) and which of two passes of a repeated hour it is in (`startRepeated`, `endRepeated`). A file from before 2026-10-12 says none of that: it is read as written by this computer's clock, as it always was, and stamped |
| **`.eml`** (RFC 5322 + MIME) | yes | yes | Multipart trees, encoded words, RFC 2231 names, quoted-printable and base64, attachments and inline parts, importance, keywords, status flags; round-trips its own output. No uuencode, TNEF (`winmail.dat`), `format=flowed` |
| **`.mbox`** (RFC 4155, mboxrd-tolerant) | yes | yes | `Content-Length` variants not honoured |
| **`.ics`** (iCalendar, RFC 5545) | yes | yes | Events, to-dos, attendees, alarms, recurrence (the common rules), skipped days, changed instances read as separate events. Time zones: `TZID` by the file's own `VTIMEZONE` (which decides the moment), by IANA and Windows names, UTC and floating times; an item with a zone is written with its `VTIMEZONE`, a calendar export in the calendar's zone. A zone nobody knows is kept at the offset the file gives, else at its clock time, and reported. No `RDATE`, recurring to-dos |
| **`.vcf`** (vCard 2.1 / 3.0 / 4.0) | yes | 3.0 | Names, organisation, e-mails, phones, addresses, URLs, birthday, notes, categories. No photos, groups, name prefixes/suffixes |
| `.msg`, `.pst`, `.ost`, `.olm`, `.oft` | no | no | Gap #12 |
| `.html`, `.pdf` (save a message as) | no | no | With printing (gap #15) |

Text encodings: a message's bodies, encoded-word headers and RFC 2231 parameters are read in every
character set of the WHATWG Encoding Standard, as a browser reads them (ISO-8859-1 as Windows-1252;
an unlabelled part as UTF-8 when it is, else Windows-1252); a set it doesn't name is read as UTF-8,
lossily (#178). vCard 2.1's `CHARSET=` is read by the same decoder, by the same rules (#180), whether the value is
quoted-printable or raw 8-bit bytes: a contact file is read from its bytes, line by line (#185).
Output is always UTF-8.

## Protocols

| Protocol | State | Notes |
|---|---|---|
| **SMTP submission** (RFC 5321, 4954, 3207) | send | EHLO/HELO, STARTTLS or TLS from the start, AUTH PLAIN and LOGIN, dot-stuffing, multi-line replies. No OAuth, SMTPUTF8, DSN, pipelining |
| **IMAP4rev1** (RFC 3501, special-use 6154) | download | LOGIN, LIST, EXAMINE, UID FETCH (flags, sizes, whole messages with literals), modified UTF-7 names, UIDVALIDITY handling; `uid_store` and `append` exist but sync doesn't use them yet (gap #2). No IDLE, CONDSTORE/QRESYNC, AUTHENTICATE, COMPRESS, partial fetch |
| **TLS** | rustls with the Mozilla root set | Certificate and host name verified; unencrypted connections refused except to loopback; a missing STARTTLS offer is an error |
| POP3 | no | Not planned before M6 |
| **JMAP mail** (RFC 8620 core, 8621 mail) | download | Over HTTPS, in the browser build (`fetch`) and in the tests: the session, `Mailbox/get`, `Email/query` and `Email/get` for each mailbox's newest messages, a message fetched whole from its blob, keywords as read, flag, answered, forwarded and draft. Bearer token or password; `https` only, plain `http` to this computer alone. No `*/changes`, no `Email/set`, `EmailSubmission` or `Mailbox/set` (nothing made here goes up, nothing is sent), no search, push or contacts and calendars. Tested against an in-process server only (gap #1) |
| Exchange (EWS, MAPI), Microsoft Graph, CalDAV, CardDAV | no | Gap #5. Graph runs over HTTPS too, but the browser build has no Graph runner yet |

Interoperability with real servers has not been tested (gap #1).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-11 | major | First version |
| 2026-10-12 | minor | Time zones (M3.5): the mailbox file's `zone` and an event's own zones; `VTIMEZONE` read and written |
| 2026-10-15 | minor | The mailbox file in format 3, a log of records a save appends to (R6); formats 1 and 2 migrated with the old file kept; damage stops the reading at its save, or costs one item of the first; older builds and newer forms |
| 2026-10-17 | patch | Files kept aside are never removed; 0.4.3 and earlier read the kept file |
| 2026-10-18 | minor | The word index beside the mailbox file (`<file>.words`), R6.2 |
| 2026-10-18 | minor | Mail in every character set of the WHATWG Encoding Standard (Windows-1250, ISO-8859-2, KOI8-R, Shift_JIS…), #178 |
| 2026-10-18 | minor | vCard files in every character set of the WHATWG Encoding Standard, through mail's decoder, #180 |
| 2026-10-18 | minor | vCard 2.1's raw 8-bit values read in their own `CHARSET`, #185 |
| 2026-10-19 | minor | JMAP mail (RFC 8620, 8621) read over HTTPS: mailboxes, the newest messages of each, a message fetched whole (M6); nothing made here goes up yet |
