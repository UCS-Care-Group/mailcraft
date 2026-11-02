# DAV model and documents

> **Last reviewed:** 2026-10-24 · **Last updated:** 2026-11-02 · **Change:** patch

**Status:** foundation only.

`mailcraft_model::dav` describes a discovered calendar or address book and a resource snapshot.
Snapshots retain the original document, the parsed contacts or events and the opaque ETag.
Unknown collection permissions default to read-only. These types are not yet added to the
mailbox file, account settings, or a sync command, and contain no passwords or tokens.

`mailcraft_dav` currently contains pure document helpers only. Nothing calls a server,
opens a socket, reads a file or schedules a job. There is no discovery or upload implementation.

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

## Validation and sources

Tests include parse/edit/write round trips through the real iCalendar and vCard writers,
repeated-value preservation, email reminders, and recurrence time-form changes around a
daylight-saving boundary. They use synthetic documents and arbitrary generated input, never a real mailbox or DAV
server. They exercise preservation, conservative refusals, namespace separation, successful
and failed propstats, hostile XML and the size/depth/count budgets.

The pure `mailcraft-dav` crate is at layer 3 and builds for WebAssembly. Cargo-fuzz targets
`dav_xml` and `dav_document` exercise both readers; CI runs them when `crates/dav` changes.
Structured property tests also mutate valid cards and events (folds, stray CRs, quotes and
missing END lines).

Protocol sources: [WebDAV, RFC 4918](https://www.rfc-editor.org/rfc/rfc4918),
[CalDAV, RFC 4791](https://www.rfc-editor.org/rfc/rfc4791),
[CardDAV, RFC 6352](https://www.rfc-editor.org/rfc/rfc6352),
[iCalendar, RFC 5545](https://www.rfc-editor.org/rfc/rfc5545) and
[vCard, RFC 6350](https://www.rfc-editor.org/rfc/rfc6350).

## Revision history

| Date | Change |
|---|---|
| 2026-11-02 | Refuse lossy alarm and alias edits; strengthen unsafe-edit assertions |
| 2026-11-01 | Timing and recurrence changes refuse a lossy writer baseline |
| 2026-10-24 | Inserted properties and folds use the source line endings |
| 2026-10-24 | Separate portable DAV crate, property ordering, end-form guards and fuzz coverage |
| 2026-10-24 | Review fixes: time/end forms, repeated properties, source versions, email alarms and real-writer tests |
| 2026-10-24 | Pure model/document foundation; no network or sync behaviour |
