# DAV model and documents

> **Last reviewed:** 2026-10-24 · **Last updated:** 2026-11-02 · **Change:** patch

**Status:** foundation only.

`mailcraft_model::dav` describes a discovered calendar or address book and a resource snapshot.
Snapshots retain the original document, the parsed contacts or events and the opaque ETag.
Unknown collection permissions default to read-only. These types are not yet added to the
mailbox file, account settings, or a sync command, and contain no passwords or tokens.

`mailcraft_dav` currently contains pure document helpers only. Nothing calls a server,
opens a socket, reads a file or schedules a job. There is no upload implementation, and the
search for a server (below) is not yet called by Add Account or anything else.

## Editing a document

`document::patch` compares the known properties before and after an edit and applies only
those differences to the original VEVENT or VCARD. Unknown properties, photos, folded lines,
timezone definitions and unchanged nested components retain their original bytes. A no-op
returns the original bytes, after validating them.

The helper refuses malformed documents, multiple events or cards in one resource, changed
UIDs, changed document versions (including a source/writer version mismatch), meetings with
direct VEVENT ORGANIZER or ATTENDEE properties, changes to nested components, edited properties
whose source differs from the writer, and ambiguous grouped contact properties/labels. A change
to a repeated property preserves equal occurrences, including their parameters and physical folds.
Nested email reminders remain untouched and do not make an event a meeting. Replaced or added
event properties are inserted before the first nested alarm, preserving RFC 5545 property order.
Inserted physical lines use the first line-ending style of the source; unchanged bytes, including
folds and their line endings, remain untouched.

Time edits keep the source date, floating, UTC or TZID form. A named zone must have its
VTIMEZONE definition in the source. An added DTEND must match the effective DTSTART form. A writer that converts the source DURATION to DTEND
cannot safely edit its times; conflicting DTEND and DURATION are refused.
Timing and recurrence properties form one unit: changing DTSTART, DTEND, DURATION, RRULE,
RDATE, EXDATE or EXRULE requires all of their source lines to match the writer’s baseline.
This refuses edits that would leave an unmodelled recurrence date behind or mix a floating
start with a UTC rule produced by normalisation. Timing edits also require nested alarms to match
the writer’s baseline, so an absolute reminder cannot silently stay at its old time.
Alternate keys for contact favourites, event importance, online-meeting links and free/busy
status are checked together before changing their shared field. Parameterised BEGIN/END
delimiters are refused rather than preserved as ordinary properties.
A changed recurrence rule must have a recognisable effective DTSTART. Its UNTIL uses a matching
DATE, floating DATE-TIME or UTC DATE-TIME form (UTC for a named-zone start), as required by
RFC 5545 section 3.3.10; ambiguous starts, duplicate UNTIL values and unknown forms are refused.
These cases return an error rather than silently discard information. They can be handled by
later, separately reviewed operations. A text is limited to 4 MiB, 65,536 physical lines,
256 KiB per unfolded line and a nesting depth of 16 components.

## Reading DAV XML

`xml::parse` reads one UTF-8 DAV multistatus with namespace-aware element names. It preserves
property text, CDATA, predefined entities and character references. `xml::properties` yields
only properties from a successful HTTP 200 propstat. Failed or absent properties are not
evidence of deletion; determining a complete inventory is outside this layer.

Malformed/truncated XML, DTDs, custom entities, undefined prefixes and extra document roots
fail the whole response. Budgets are 16 MiB, 100,000 elements, 32 nested elements and 256 bytes
per namespace, element or component name (so inherited namespace names cannot amplify memory). Errors
contain no input text. The XML dependency is already used by the workspace; no third-party dependency is added.

## Finding a server

`mailcraft_net::lookup_dav` asks for the records of RFC 6764: a domain's `_caldavs._tcp` or
`_carddavs._tcp` SRV record and the `path` of the TXT record beside it, over the same
DNS-over-HTTPS name servers as the mail servers' records. Only the domain goes out. The
services without TLS (`_caldav._tcp`, `_carddav._tcp`) are never asked for. What comes back is
the domain, every target with a port, best first (lower priority, then the higher weight: the
fixed order the mail servers' records are read in, not RFC 2782's weighted draw), and the path:
the first `path` of the TXT record, taken only when it is at most 250 bytes of printable ASCII
starting with one slash.

`mailcraft_dav::discovery::probe` then says whether a CalDAV or CardDAV server answers at a
`Start`: the place the records name, the domain's own `/.well-known/caldav` or
`/.well-known/carddav` when it has no record, or an address the owner typed. It performs
nothing itself: the host's `HttpExchange` does. Its rules:

- No credential is sent: every request is an anonymous `PROPFIND` for the principal.
- A redirect is followed on the starting origin only, one step at a time, at most five. A
  redirect to another server is not followed; its address is handed back (`Found::Elsewhere`),
  so that the owner can be asked before anything goes there. `https` only.
- A server counts as one by `status::is_dav_evidence` alone: a DAV multistatus, or a 401 whose
  `DAV` header names `calendar-access` or `addressbook`. A bare 401 proves nothing, and a
  search for calendars or for address books takes a 401 only when it names that kind's class.
- When a TXT record's path leads to no DAV server, the well-known address on the same origin is
  asked once more.
- A `Start` keeps the domain it was looked up for, and says whether the host is outside it.
  The certificate is checked for the host connected to, by whatever performs the exchange; a
  host outside the domain is vouched for by the DNS alone, so the owner is asked, with the
  host named, before a credential goes there.

## Validation and sources

Tests include parse/edit/write round trips through the real iCalendar and vCard writers,
repeated-value preservation, email reminders, and recurrence time-form changes around a
daylight-saving boundary. They use synthetic documents and arbitrary generated input, never a real mailbox or DAV
server. They exercise preservation, conservative refusals, namespace separation, successful
and failed propstats, hostile XML and the size/depth/count budgets.

The pure `mailcraft-dav` crate is at layer 3 and builds for WebAssembly. Cargo-fuzz targets
`dav_xml` and `dav_document` exercise both readers, and `dav_answers` the typed multistatus answers, addresses and statuses; CI runs them when `crates/dav` changes.
Structured property tests also mutate valid cards and events (folds, stray CRs, quotes and
missing END lines).

Protocol sources: [WebDAV, RFC 4918](https://www.rfc-editor.org/rfc/rfc4918),
[CalDAV, RFC 4791](https://www.rfc-editor.org/rfc/rfc4791),
[CardDAV, RFC 6352](https://www.rfc-editor.org/rfc/rfc6352),
[locating CalDAV and CardDAV services, RFC 6764](https://www.rfc-editor.org/rfc/rfc6764),
[iCalendar, RFC 5545](https://www.rfc-editor.org/rfc/rfc5545) and
[vCard, RFC 6350](https://www.rfc-editor.org/rfc/rfc6350).

## Revision history

| Date | Change |
|---|---|
| 2026-11-02 | Validate changed recurrence UNTIL against the effective surviving DTSTART |
| 2026-11-02 | Refuse lossy alarm and alias edits; strengthen unsafe-edit assertions |
| 2026-11-01 | Timing and recurrence changes refuse a lossy writer baseline |
| 2026-10-24 | Inserted properties and folds use the source line endings |
| 2026-10-24 | Separate portable DAV crate, property ordering, end-form guards and fuzz coverage |
| 2026-10-24 | Review fixes: time/end forms, repeated properties, source versions, email alarms and real-writer tests |
| 2026-10-24 | Pure model/document foundation; no network or sync behaviour |
