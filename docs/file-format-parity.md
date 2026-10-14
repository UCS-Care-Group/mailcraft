# File formats and protocols

> **Last reviewed:** 2026-10-11 · **Last updated:** 2026-10-14 · **Change:** minor (S/MIME and OpenPGP: signed and encrypted mail read and written, key files; the mailbox file's format 3; Outlook `.msg` files read) · **Target:** Microsoft Outlook (Microsoft 365, classic desktop)

What MailCraft reads, writes and speaks, and how far each goes. Everything is implemented from the
public specifications; every reader is fed arbitrary bytes by property tests and must never panic.

## Files

| Format | Read | Write | Notes |
|---|---|---|---|
| **`.mailcraft`** (the mailbox) | yes | yes | Three forms, all read. Format 1, one self-contained JSON file (attachments in base64), is what exports, File › Save Mailbox As and the browser write. Format 2, JSON without the content, with bodies and attachments over 1 KB as write-once files named by their SHA-256 in `<file>.blobs/`, is read and never written any more. Format 3, what the desktop keeps (`crates/engine/src/store_index.rs`), is a log of records beside the same content folder: a header (`MailCraft index\n`, the format number), then records of a length, a kind, a payload and a truncated SHA-256: the rest of the mailbox (JSON), one item (a message, contact, event or task as JSON, a message with the names of its content outside), the deletion of one, and a commit that closes a save. A save appends what changed and a commit, in one write flushed to the disk; when the appended records outgrow the mailbox the file is written whole again, to a temporary file renamed over. Only what a commit closes is read: a save cut short is no save, zeros after the last commit are ignored, and anything else there is reported and the file kept aside (`<file>.damaged-<time>`) before it is next written. A file of format 1 or 2 becomes format 3 the first time a changed mailbox is saved, and the old file is kept beside it (`<file>.format1` or `.format2`). `repair()` makes any file consistent. Event times are clock times, and the file says by which zone's clock (`zone`, an IANA name); an event may name zones of its own (`startZone`, `endZone`) and which of two passes of a repeated hour it is in (`startRepeated`, `endRepeated`). A file from before 2026-10-12 says none of that: it is read as written by this computer's clock, as it always was, and stamped |
| **`.eml`** (RFC 5322 + MIME) | yes | yes | Multipart trees, encoded words, RFC 2231 names, quoted-printable and base64, attachments and inline parts, importance, keywords, status flags; round-trips its own output. No uuencode, TNEF (`winmail.dat`), `format=flowed` |
| **`.mbox`** (RFC 4155, mboxrd-tolerant) | yes | yes | `Content-Length` variants not honoured |
| **`.ics`** (iCalendar, RFC 5545) | yes | yes | Events, to-dos, attendees, alarms, recurrence (the common rules), skipped days, changed instances read as separate events. Time zones: `TZID` by the file's own `VTIMEZONE` (which decides the moment), by IANA and Windows names, UTC and floating times; an item with a zone is written with its `VTIMEZONE`, a calendar export in the calendar's zone. A zone nobody knows is kept at the offset the file gives, else at its clock time, and reported. No `RDATE`, recurring to-dos |
| **`.vcf`** (vCard 2.1 / 3.0 / 4.0) | yes | 3.0 | Names, organisation, e-mails, phones, addresses, URLs, birthday, notes, categories. No photos, groups, name prefixes/suffixes |
| **`.msg`** (Outlook message: [MS-CFB], [MS-OXMSG]) | yes | no | One message saved from or dragged out of Outlook, through File › Import, drag and drop, opening the file and `mailcraft-cli import`/`convert`; known by its extension and by the compound file's signature. Subject, sender (SMTP address; for an Exchange-only address the transport headers' `From`, else the name), To/Cc/Bcc, sent and received times, Message-ID, In-Reply-To, References, importance, read, follow-up flag, replied/forwarded; the text body (Unicode, or 8-bit in the message's code page), the HTML body (its own property, or taken back out of compressed RTF that encapsulates it, [MS-OXRTFCP]/[MS-OXRTFEX]; an RTF-only message gives its text); attachments by value with name, type and content id (inline pictures), attached messages as `message/rfc822` (seven deep at most); the transport headers' Reply-To, Keywords and unmodelled fields. Version 3 and 4 files. Not read: named properties (categories set in Outlook, the follow-up due date), attachments by reference or OLE objects, RTF formatting beyond its text, signed or encrypted messages (`.p7m` content stays an attachment), TNEF |
| **S/MIME** (RFC 8551; CMS, RFC 5652; X.509, RFC 5280) | yes | yes | `multipart/signed` with `application/pkcs7-signature` verified (RSA PKCS#1 v1.5 or P-256 ECDSA, SHA-256/384/512; SHA-1 said to be unsafe rather than accepted); `application/pkcs7-mime` enveloped data decrypted with the owner's RSA key (key transport, PKCS#1 v1.5 or OAEP; AES-128/192/256-CBC, 3DES-CBC) and signed data unwrapped; BER (indefinite lengths, cut OCTET STRINGs, as OpenSSL and NSS write it) read. The signer's chain is checked up to roots the owner trusts (names, signatures, validity, CA flag, e-mail use); no revocation check. Written: detached SHA-256 signatures carrying the owner's certificate, enveloped data for every recipient and the owner (RSA key transport, AES-256-CBC). Not done: key agreement (ECDH) recipients, AuthEnvelopedData (AES-GCM), RSASSA-PSS, triple wrapping, the OS trust store |
| **OpenPGP** (RFC 9580, 4880; PGP/MIME, RFC 3156) | yes | yes | Through rPGP: `multipart/signed` with `application/pgp-signature`, `multipart/encrypted` (signatures inside checked too), inline `-----BEGIN PGP SIGNED MESSAGE-----` and `-----BEGIN PGP MESSAGE-----` in a text body. Written: PGP/MIME, signed then encrypted (SEIPD v1, AES-256) to every recipient and the owner. Keys are vouched for by the owner (no web of trust, no key servers). Not done: bzip2-packed messages, protected headers |
| Key files: `.cer`, `.crt`, `.pem`, `.p7b`; `.p12`, `.pfx`; `.asc`, `.gpg` | yes | public only | Certificates (DER or PEM), certificate bundles, digital IDs under a passphrase (PKCS#12, RFC 7292, read by our own bounded reader: HMAC-SHA1/SHA-2 integrity, PBES2 or the PKCS#12 3DES and RC2 schemes; work capped), PKCS#8 (plain or PBES2-encrypted), PKCS#1 and SEC 1 keys, OpenPGP public and secret keys (unlocked with their passphrase). Exported: certificates as PEM or DER, OpenPGP public keys armored; private keys never |
| `.pst`, `.ost`, `.olm`, `.oft` | no | no | Gap #12 |
| `.html`, `.pdf` (save a message as) | no | no | With printing (gap #15) |

Text encodings: UTF-8, US-ASCII, ISO-8859-1/15 and Windows-1252 are decoded properly; others
lossily (gap #13). Output is always UTF-8.

## Protocols

| Protocol | State | Notes |
|---|---|---|
| **SMTP submission** (RFC 5321, 4954, 3207) | send | EHLO/HELO, STARTTLS or TLS from the start, AUTH PLAIN and LOGIN, dot-stuffing, multi-line replies. No OAuth, SMTPUTF8, DSN, pipelining |
| **IMAP4rev1** (RFC 3501, special-use 6154) | download | LOGIN, LIST, EXAMINE, UID FETCH (flags, sizes, whole messages with literals), modified UTF-7 names, UIDVALIDITY handling; `uid_store` and `append` exist but sync doesn't use them yet (gap #2). No IDLE, CONDSTORE/QRESYNC, AUTHENTICATE, COMPRESS, partial fetch |
| **TLS** | rustls with the Mozilla root set | Certificate and host name verified; unencrypted connections refused except to loopback; a missing STARTTLS offer is an error |
| POP3 | no | Not planned before M6 |
| Exchange (EWS, MAPI), Microsoft Graph, JMAP, CalDAV, CardDAV | no | Gap #5; JMAP and Graph are how the browser build will get a server connection |

Interoperability with real servers has not been tested (gap #1).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-11 | major | First version |
| 2026-10-12 | minor | Time zones (M3.5): the mailbox file's `zone` and an event's own zones; `VTIMEZONE` read and written |
| 2026-10-14 | minor | Outlook `.msg` files read (gap 12, first part): the `msg` crate |
| 2026-10-14 | minor | The mailbox file's format 3, a log of records a save appends to (gap 8); formats 1 and 2 still read and migrated |
