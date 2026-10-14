# MailCraft architecture

> **Last reviewed:** 2026-10-11 · **Last updated:** 2026-10-14 · **Change:** minor (formatted composing: `model::rich`, `mime::rich`, the `format.*` commands, spelling; right-to-left text: `html::bidi`; the mailbox file's format 3, a log of records; the `crypto` crate at L2: S/MIME and OpenPGP; `engine::security`; earlier: the `msg` crate: Outlook `.msg` files come in through `file.import`; printing: the `print` crate at L2, `cmd/print.rs`, `Services::print`; uploads and IDLE) · **Target:** Microsoft Outlook (Microsoft 365)

How MailCraft is built today. Rules for working in the code are in [`AGENTS.md`](../AGENTS.md). The
shape — layers, "everything is a command", the swappable egui front end, the control channel, MCP,
`xtask` gates — is WordCraft's, applied to mail.

## Layers

15 crates, 3 apps and `xtask` (`cargo xtask stats` prints lines and tests per crate). Layering is
enforced by `cargo xtask layers`: a crate may only depend on crates in lower layers (one exception
inside L3: `net` drives `proto`; and inside L2: `msg` uses `mime`), and nothing below L6 depends on egui, eframe, winit or rfd.

| Layer | Crate | Job |
|---|---|---|
| L0 | `time` | Dates and times without a clock dependency: proleptic Gregorian maths, weekdays, ISO weeks, RFC 5322 and ISO 8601 text, list and group labels; time zones (`Zone`: the IANA database with its history, built in; Windows' names for zones; offsets; the system's zone), the only place the zone library is used; `now()` and the system's zone are the only calls that ask the system (or the browser) |
| L0 | `control-key` | Per-window control-port key files shared by the app and the MCP bridge |
| L1 | `model` | The mailbox: accounts, folders, messages, contacts, calendars, events with recurrence, tasks, categories, rules, signatures; edit primitives (`move_message`, `delete_folder`…), conversations, the search query language, `repair()` for whatever a file held, and the sync data types |
| L1 | `html` | Hostile HTML mail → display blocks (`parse`), plain text (`to_text`) and the line a message list shows (`preview`, `text_preview`); plain text → blocks and HTML; entities; the safe-URL rule |
| L2 | `mime` | RFC 5322 / MIME reader and writer (`.eml`), encoded words, RFC 2231 parameters, charsets, mbox |
| L2 | `ical` | iCalendar reader and writer: events, to-dos, recurrence rules, alarms, attendees |
| L2 | `vcard` | vCard 2.1/3.0/4.0 reader, 3.0 writer |
| L2 | `msg` | Outlook `.msg` reader: a bounded Compound File Binary reader ([MS-CFB]), the message's properties ([MS-OXMSG]), compressed RTF and the HTML it encapsulates ([MS-OXRTFCP], [MS-OXRTFEX]); attached messages become `.eml` attachments through `mime` |
| L2 | `crypto` | Signed and encrypted mail, on bytes: S/MIME (CMS `SignedData` and `EnvelopedData` over X.509, a BER-to-DER normaliser, chain checking up to roots the owner trusts) and OpenPGP (rPGP: detached and inline signatures, PGP/MIME encryption); key files (certificates, PKCS#12 read from RFC 7292 by our own bounded reader, PKCS#8, OpenPGP keys). No MIME here (`mime::secure` splits and wraps the parts); private keys in memory only, wiped on drop |
| L2 | `print` | Printing: a message in Memo Style (from `html`'s display blocks), the calendar (Daily, Work Week, Weekly, Monthly), contacts as cards and tasks as a table, laid out as pages of positioned text, lines, boxes and pictures (`Document`); the same pages as a PDF with the bundled fonts embedded as subsets (`pdf`). The window's Print Preview paints the `Document`; the engine's `file.print` writes the PDF and the desktop host hands it to the system |
| L3 | `proto` | IMAP4rev1 and SMTP clients over any `Read + Write` stream (no sockets, so they test against scripted transcripts and build for wasm) |
| L3 | `net` | TCP + TLS (rustls), sending through SMTP, two-way IMAP synchronisation, OAuth sign-in, MX lookup, Microsoft Graph (`graph`), and Outlook's Favorites through Exchange Web Services (`ews`, read-only); empty on wasm |
| L4 | `engine` | `Session` (mailbox, selection, view, the message being written, undo), the **command registry** (`cmd/*.rs`), the message list (`list.rs`), the Outlook feature catalog (`catalog.rs`), the sample mailbox, file dispatch (`io.rs`) |
| L5 | `mcp` | MCP server: tools over the command registry; headless or bridged to the running app |
| L6 | `ui-egui` | The front end: title bar with search, ribbon, module bar, folder pane, message list, reading pane, composer, calendar, people, tasks, Backstage, dialogs, themes, control channel, headless `ui_shot` example |
| apps | `mailcraft` | Desktop app (eframe + wgpu; logging, window geometry, native dialogs, Send/Receive thread, control server) |
| apps | `mailcraft-cli` | `info`, `folders`, `list`, `show`, `import`, `export`, `convert`, `run`, `sync`, `commands`, `parity`, `mcp` |
| apps | `mailcraft-web` | Browser build (eframe web runner, WebGPU with WebGL2 fallback; browser storage, file picker, downloads) |

## Data flow

1. **Open:** the desktop app reads `<settings>/mailbox.mailcraft` (JSON) through `engine::io`;
   `Mailbox::repair` makes whatever the file held consistent. The browser reads the same JSON from
   its storage. Other files (`.eml`, `.msg`, `.mbox`, `.ics`, `.vcf`) come in through `file.import`.
2. **Act:** every user-visible action is a command id run through `Session::run` (ribbon,
   shortcuts, command search, CLI, control channel and MCP all dispatch by id). `Session::run`
   snapshots the mailbox for undo — items are `Arc`-shared, so a snapshot copies pointers, not mail —
   restores it if the command fails, and catches an escaped panic the same way (never-crash
   standard).
3. **Show:** the front end reads session state each frame. The message list is computed by
   `engine::list` (filter, search, sort, conversations, group headings) and cached on the mailbox
   revision and the view; only the rows on screen are laid out. A message's body is turned into
   display blocks by `html` and painted as formatted text: nothing in a message runs or is fetched.
4. **Send/Receive:** `sync.request` (pure) says what to do; a host thread runs `net::sync`, which
   sends the Outbox through SMTP and downloads new mail and flag changes over IMAP with `proto`
   and `mime`; `sync.apply` (pure) takes the result in and runs the rules on new Inbox mail. The
   engine and the UI never touch a socket, which is why they build for the browser.
5. **Save:** the desktop app writes the mailbox file a few seconds after each change and on
   exit; the browser writes to its storage; File › Save As and the exports write other files (a
   download in the browser).

## Decisions worth knowing

- **Event times are wall-clock times, and the mailbox says whose clock.** `Event::start` and
  `end` are clock times in the zone the calendar is shown in, which `Mailbox::zone` names; a
  timed event is the moment that clock time is there (`Event::instants`; `start_repeated` says
  which, in an hour the clocks show twice), and a whole-day event is its days. When the zone
  the calendar is shown in changes (the computer travels, or File › Options names another),
  `Session::settle_zone` has `Mailbox::rebase` rewrite every timed event by the new clock: the
  moments stay, nothing is marked as changed for a server, and a series is pinned to the zone it
  was written in (`Event::start_zone`), so that it goes on repeating by that clock. A mailbox
  without the stamp (written before 2026-10-12) is read as it always was, by this computer's
  clock, and only stamped. Everything that converts goes through `mailcraft_model::zoned`.
- **Conversations are grouped by topic** (the subject without `RE:`/`FW:` prefixes) within an
  account, as the incumbent does, not by `References` chains.
- **A mailbox file, and its content beside it.** On the desktop `mailbox.mailcraft` holds
  everything but content as a log of records (format 3, `crates/engine/src/store_index.rs`): the
  rest of the mailbox, and one record per message, contact, event or task; bodies and attachments
  over a kilobyte are write-once files named by their SHA-256 in `mailbox.mailcraft.blobs/`
  (`crates/engine/src/store.rs`). A save appends the records of what changed and a commit in one
  flushed write, on a background thread: a read mark costs a few hundred bytes however large the
  mailbox. What was last written is remembered item by item (its fingerprint, and a weak pointer
  to the copy in memory that it is), so a save looks only at items whose copy changed; when the
  appended records outgrow the mailbox, the file is written whole to a temporary file and renamed
  over. Opening reads records to the last commit and parses items on several threads; formats 1
  and 2 (JSON) are still read and become format 3 at the first changed save, the old file kept
  beside it. A self-contained single file is still what the browser, exports and `file.saveAs`
  use. Every message's text and attachments are still in memory while the app runs, which is the
  next limit (see `docs/gaps.md`).
- **A Send/Receive arrives in pieces.** `mailcraft_net::sync_with` emits what was sent, what the
  server was told, then mail folder by folder (the Inbox first, newest first, fifty at a time),
  each piece safe to apply by itself (`apply_sync_part`), so mail shows while the rest is on its
  way and a run that is cut short loses nothing: the UID bookkeeping only advances past what was
  handed over.
- **Microsoft Graph beside the mail protocols.** For an account that signs in with Microsoft, the
  host first asks Graph to send the Outbox and, every quarter of an hour, for the calendar and
  contacts (`crates/net/src/graph.rs`); the engine merges them as server-kept, read-only items
  (`cmd/remote.rs`, `Origin`). Mail itself still goes over IMAP.
- **The Favorites are Outlook's, read and never written.** For the same accounts the host also
  reads which folders the owner keeps under Favorites in Outlook (`crates/net/src/graph/favorites.rs`:
  one configuration item of the mailbox, of which one entry is read with an XML reader that
  takes no DTD and expands no entity, then each folder looked up by its id, twenty to a batch).
  What crosses to the engine is where each folder is (its path of names, and which special
  folder it is), never an id: `sync.applyFavorites` finds the mailbox's own folders for them and
  keeps the list in the server's order (`Mailbox::favorites_from_server`), beside the favourites
  marked here, which are kept as they are and shown again whenever the server's list can't be
  had or the owner prefers them (`view.favorites`). The window asks with its other Graph jobs,
  at most every quarter of an hour (`crates/ui-egui/src/favorites_sync.rs`); the two permissions
  it takes are asked for by themselves, on the owner's say-so, never with a sign-in.
- **Two ways of reading them, and the owner of an organisation's account chooses.** The same
  download also comes from `crates/net/src/ews.rs`, which reads the navigation shortcuts that
  classic Outlook keeps in the mailbox's Common Views folder through Exchange Web Services
  (`FindFolder`, `FindItem` and `GetFolder`, nothing that writes; `ews/links.rs` says which
  shortcuts count and in what order, `ews/xml.rs` reads the answers as hostile, and
  `ews/standin.rs` is a stand-in that holds requests to the documented schema). Which way an
  account takes is a setting of the account (`Account::favorites_source`:
  `account.favoritesSource`; nothing is written of it until the owner chooses), and
  `ui.favoritesSource` does what follows from a choice: Microsoft's page for Graph's two
  permissions, or a read through Exchange Web Services, which is a host job of its own
  (`Services::ews`) with a token of its own, kept in memory. Whether an account is an
  organisation's is read from the directory its sign-in names (the identity token's `tid`);
  a personal Microsoft account has Graph only. Exchange Web Services ends on 1 April 2027
  (`FavoritesSource::EWS_ENDS`): from that day by the session's clock the engine refuses it and
  nothing is asked of it. Everything after the download is shared: `sync.applyFavorites`, the
  pane, the timing, and falling back to the favourites kept here.
- **Shared mailboxes are accounts that sign in as their owner.** A mailbox of Microsoft 365 that
  the owner has Full Access to (`support@`) is an account of its own in the mailbox
  (`Account::shared_of` names the owner's account): its own folders, mail and synchronisation
  state, its own heading in the folder pane right after its owner's (`Mailbox::accounts_in_pane_order`)
  and its own From address. It keeps no sign-in: `cmd/shared.rs` (`account.addShared`,
  `account.removeShared`) adds and closes one, and `sync_request` hands the host the owner's
  servers and Microsoft sign-in under the shared mailbox's address (`shared::signed_in_as`),
  with `SyncRequest::send_login` naming the owner for SMTP. The host brings the owner's access
  token (`MailApp::tokens` of the owner), so IMAP is XOAUTH2 with the shared address as the user,
  and mail from it goes out through SMTP as the owner with the shared address as the sender
  (the Send As right). Nothing goes through Graph for it and no permission beyond a sign-in's is
  asked for; a server's refusal (no Full Access, no Send As) is said in those words
  (`crates/net/src/shared.rs`).
- **What is made here goes up, and new mail is heard of at once.** `sync_request` lists
  `SyncRequest::uploads`: drafts, copies and imported mail in a folder the server keeps that it
  has no UID for. The host sends each with UID COPY when the server already has the message,
  else APPEND, after a `UID SEARCH HEADER Message-ID` so a run cut short never puts it there
  twice; the UID comes back in APPENDUID/COPYUID. Categories travel as IMAP keywords
  (`sync::category_keyword`), merged three ways like flags. Beside the runs, the desktop host
  keeps one more connection IDLE on each account's Inbox (`crates/net/src/idle.rs`,
  `Services::idle`, `crates/ui-egui/src/inbox_watch.rs`); news only queues a run, so the engine
  still does no I/O.
- **A server's "yes" to a calendar change is checked.** A change made here is noted
  (`Mailbox::event_changes`), sent by the host, and then noted once more as something to be seen
  (`Mailbox::event_checks`): the downloads that follow have to show the event as it was sent.
  Until one does, a download leaves the event as it is here (an answer may be older than the
  change); after three that don't, the event is put back to what the server has and the owner is
  told. A waiting change that can't be sent any more is given up with a word, never silently
  (`cmd/remote.rs`: `check_sent`, `tidy_changes`). The tests hold all of it against
  `mailcraft_net::graph::standin`, which takes only what the documented service takes.
- **Two-way sync without a journal of everything.** Flags are compared with what the server last
  said (`Message::synced`): a flag changed here since then goes up, one changed there comes down,
  and here wins when both did. Moves and permanent deletes can't be seen by comparing, so the
  mailbox keeps them as `Mailbox::pending` until a run has told the server (`RemoteOp` in
  `SyncRequest`, answered in `SyncResult`). A server copy of a message that is already here (sent
  mail the server files itself, a move the server gave no new UID for) is matched to it rather
  than added. Nothing is expunged without UIDPLUS, and undo history ends at a move, delete or
  send that reached the server. Not pushed yet: folder renames and deletes, drafts, categories.
- **Secrets stay out of our files.** A password or an OAuth refresh token lives in memory, and in
  the operating system's credential store when the owner wants it remembered (`Services::secrets`;
  Windows and macOS so far). The mailbox file, `ui.json` and logs never hold one.
- **Signing in on the provider's page.** Microsoft accounts use OAuth 2.0 (authorization code with
  PKCE, a loopback redirect, the system's browser) and XOAUTH2. The engine only knows that an
  account signs in that way (`Account::oauth`); the host with sockets does it and hands tokens in.
- **Formatted composing is plain text plus styles.** The body of a message being written stays
  one string (`Compose::body`: a paragraph per line, a picture as one U+FFFC); its look is
  `Compose::styles` (`model::rich::Styles`: runs of characters with a `CharStyle`, one
  `ParaStyle` per line), which every edit follows (`Styles::edit` from the old text to the new,
  `Styles::splice` for exact insertions) and which is fitted to the text again before use. The
  `format.*` commands act on character ranges (the editor reports its selection with
  `compose.select`). `mime::rich` writes it as simple inline-styled HTML plus its plain-text twin,
  and reads display blocks back into it, which is how a draft reopens (its own HTML round-trips
  exactly) and how an HTML message is quoted in a reply (only what the blocks carry survives).
  The egui editor is a `TextEdit` with its own layouter (`ui-egui/src/compose_editor.rs`): each
  paragraph laid out in its own width, moved in and aligned, then joined with `Galley::concat`;
  list markers, lines and pictures are painted over it. Spelling: `engine::spell`, the SCOWL
  word list in `assets/dictionaries/`.
- **Right-to-left text without a bidi engine in egui.** `html::bidi` finds a paragraph's
  direction from its first strong letter and puts a line in drawn order (a reduced UBA). The
  reading pane wraps such a paragraph itself, orders each line, and hands egui's shaper runs of
  Hebrew or Arabic letters in written order (HarfBuzz turns each such run round by itself);
  `TextItem::order` takes a copy back to the written order. Print sets those lines from the right.

## Testing and gates

- Unit tests in every crate; property tests feeding arbitrary bytes to the readers (`mime`,
  `msg`, `ical`, `vcard`, `proto`) and round-tripping what the writers produce; in-process fake IMAP and
  SMTP servers on loopback for `net`; `engine/src/tests.rs::hostile_params_never_panic` runs every
  command with junk params; an MCP acceptance test drives a realistic triage through the protocol.
- `cargo xtask ci`: fmt, clippy `-D warnings`, tests, asset attribution, layering, wasm build.
- `cargo xtask parity` regenerates [`parity-checklist.md`](parity-checklist.md); a test keeps the
  catalog percentage above a floor that only rises.
- GitHub Actions (shared `storytold/craft-common-actions` workflows): packaging lint, FreeBSD and
  Windows ARM64 when build files change, release builds on pushes to `release`.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-11 | major | First architecture doc |
| 2026-10-12 | patch | `html` also makes the line a message list shows (`preview`, `text_preview`) |
| 2026-10-12 | minor | Calendar changes are checked against what comes down; the strict Graph stand-in |
| 2026-10-12 | minor | Time zones (M3.5): `time::Zone`, the mailbox's zone stamp and how stored times are read |
| 2026-10-13 | minor | The Favorites of Outlook (M4.2): `graph::favorites` in `net`, the server's list in the mailbox, `favorites_sync` in the front end |
| 2026-10-13 | minor | The owner chooses how they are read (M4.3): `ews` in `net` (Exchange Web Services, until 1 April 2027), `Account::favorites_source`, the choice and its job in the front end |
| 2026-10-13 | minor | Shared mailboxes (gap 5): `Account::shared_of`, `account.addShared` / `account.removeShared`, the owner's sign-in for IMAP and SMTP, `SyncRequest::send_login` |
| 2026-10-14 | minor | Signing and encryption (gap 16): the L2 `crypto` crate (S/MIME, OpenPGP, key files, chains), `mime::secure` (detecting, splitting and wrapping `multipart/signed`, `application/pkcs7-mime`, `multipart/encrypted`), `engine::security` (the reading pane's security view, protecting what goes out, private keys in memory), `cmd/security.rs`, `Mailbox::keys` (public keys only) |
| 2026-10-14 | minor | Outlook `.msg` files (gap 12): the L2 `msg` crate (Compound File Binary, the message's properties, compressed and encapsulating RTF), `io::Format::Msg` |
| 2026-10-14 | minor | Printing (gap 15): the `print` crate (L2) lays out and writes PDFs; `file.print`; File › Print with its preview; `Services::print` |
| 2026-10-14 | minor | Two-way sync: uploads (APPEND/UID COPY), categories as keywords, the IDLE watch beside the runs |
| 2026-10-14 | minor | The mailbox file as a log of records a save appends to (gap 8, `store_index.rs`) |
| 2026-10-14 | minor | Formatted composing (gap 7): `model::rich`, `mime::rich`, `cmd/format.rs`, `engine::spell`, the compose editor; right-to-left text (gap 6): `html::bidi` |
