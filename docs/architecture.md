# MailCraft architecture

> **Last reviewed:** 2026-10-11 · **Last updated:** 2026-10-28 · **Change:** minor · **Target:** Microsoft Outlook (Microsoft 365)

How MailCraft is built today. Rules for working in the code are in [`AGENTS.md`](../AGENTS.md). The
shape — layers, "everything is a command", the swappable egui front end, the control channel, MCP,
`xtask` gates — is WordCraft's, applied to mail.

## Layers

17 crates, 3 apps and `xtask` (`cargo xtask stats` prints lines and tests per crate). Layering is
enforced by `cargo xtask layers`: a crate may only depend on crates in lower layers (two exceptions:
inside L3, `net` drives `proto`; inside L0, `link` uses `names`), and nothing below L6 depends on
egui, eframe, winit or rfd.
`dispatch2`, which the desktop app uses to quit the macOS way, is for the apps alone.

| Layer | Crate | Job |
|---|---|---|
| L0 | `time` | Dates and times without a clock dependency: proleptic Gregorian maths, weekdays, ISO weeks, RFC 5322 and ISO 8601 text, list and group labels; time zones (`Zone`: the IANA database with its history, built in; Windows' names for zones; offsets; the system's zone), the only place the zone library is used; `now()` and the system's zone are the only calls that ask the system (or the browser) |
| L0 | `control-key` | Per-window control-port key files shared by the app and the MCP bridge |
| L0 | `charset` | Bytes to text in any character set of the WHATWG Encoding Standard (`encoding_rs`), one decoder for `mime` and `vcard`, which may not use each other |
| L0 | `link` | Links: a strict reader of `http` and `https` addresses (RFC 3986) that writes the host a browser would reach (lower-case ASCII, international labels as Punycode per RFC 3492, numbers as dotted decimal, IPv6 in brackets) and the address as it is shown and launched; percent-encoding. Its one dependency is `names`, for the list of format characters, which it refuses in a host but for the joiners IDNA2008 allows (#454); replaces the `url` crate (#350) |
| L0 | `names` | Names (people, folders, categories, calendars, accounts, signatures, rules): one list of Unicode's format characters (general category Cf, Unicode 17.0) and the zero-width joiner and non-joiner among them, and the two steps a name takes, coming in (`incoming`: one line, without control characters, format characters and Unicode's other default-ignorable code points (the Hangul fillers and the like, which show nothing), but with a joiner inside a word and one variation selector after a character that shows; trimmed, at most a given length, nothing when nothing is left) and being shown (`shown`: the same, cut with an ellipsis). No dependencies (#454) |
| L0 | `i18n` | Interface text in other languages: the languages (`Lang`), a reader of gettext `.po` catalogues (hostile input: sizes capped, never a panic) with plural rules, the catalogues compiled in from `crates/i18n/po/`, named `{placeholders}` (`fmt`), and the pseudo language that shows text not yet marked for translation. `cargo xtask i18n` writes `po/messages.pot` from every `tl!`/`tln!` and the engine's labels |
| L1 | `model` | The mailbox: accounts, folders, messages, contacts, calendars, events with recurrence, tasks, categories, rules, signatures; edit primitives (`move_message`, `delete_folder`…), conversations, the search query language, `repair()` for whatever a file held, and the sync data types |
| L1 | `html` | Hostile HTML mail → display blocks (`parse`), plain text (`to_text`) and the line a message list shows (`preview`, `text_preview`); plain text → blocks and HTML; entities; the safe-URL rule |
| L2 | `mime` | RFC 5322 / MIME reader and writer (`.eml`), encoded words, RFC 2231 parameters, charsets, mbox |
| L2 | `ical` | iCalendar reader and writer: events, to-dos, recurrence rules, alarms, attendees |
| L2 | `vcard` | vCard 2.1/3.0/4.0 reader, 3.0 writer |
| L3 | `proto` | IMAP4rev1 and SMTP clients over any `Read + Write` stream (no sockets, so they test against scripted transcripts and build for wasm) |
| L3 | `net` | TCP + TLS (rustls), sending through SMTP, two-way IMAP synchronisation, OAuth sign-in, MX lookup, Microsoft Graph (`graph`); empty on wasm |
| L3 | `jmap` | JMAP (RFC 8620, 8621) without sockets or browser APIs: builds each HTTP request and judges the answer, the program does the exchange (`HttpExchange`); an in-process server for tests (`stand-in` feature). The browser build runs it over `fetch` (M6, [jmap.md](jmap.md)) |
| L4 | `engine` | `Session` (mailbox, selection, view, the message being written, undo), the **command registry** (`cmd/*.rs`), the message list (`list.rs`), the Outlook feature catalog (`catalog.rs`), the sample mailbox, file dispatch (`io.rs`) |
| L5 | `mcp` | MCP server: tools over the command registry; headless or bridged to the running app |
| L6 | `ui-egui` | The front end: title bar with search, ribbon, module bar, folder pane, message list, reading pane, composer, calendar, people, tasks, Backstage, dialogs, themes, control channel, headless `ui_shot` example |
| apps | `mailcraft` | Desktop app (eframe + wgpu; logging, window geometry, native dialogs, Send/Receive thread, control server; leaving, `quit.rs`: unsaved work asks first on every platform, then on macOS the process ends through `-[NSApplication terminate:]` (`quit_macos.rs`, MailCraft's own) rather than by closing the window, which can crash AppKit, and the macOS menu bar (`menubar.rs`, `muda`; its table of command ids is `ui-egui`'s `menubar`) has Quit (⌘Q) asking like the close button) |
| apps | `mailcraft-cli` | `info`, `folders`, `list`, `show`, `import`, `export`, `convert`, `run`, `sync`, `commands`, `parity`, `mcp` |
| apps | `mailcraft-web` | Browser build (eframe web runner, WebGPU with WebGL2 fallback; browser storage, file picker, downloads) |

## Data flow

1. **Open:** the desktop app reads `<settings>/mailbox.mailcraft` (JSON) through `engine::io`;
   `Mailbox::repair` makes whatever the file held consistent. The browser reads the same JSON from
   its storage. Other files (`.eml`, `.mbox`, `.ics`, `.vcf`) come in through `file.import`.
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
   engine and the UI never touch a socket, which is why they build for the browser. A JMAP
   account goes the Graph way: `sync.jmapRequest` (pure) says what to read, the host (the browser
   build, over `fetch`) runs `mailcraft_jmap::sync_mail` and hands the run back in pieces, and
   `sync.applyJmap` (pure) takes each in by server id ([jmap.md](jmap.md)).
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
  over. Opening reads records to the last commit and parses items on several threads. A save a
  crash cut short is records no commit closes, and isn't read. A later save with a damaged record
  is where the reading stops (each save is a change to the one before, and a folder's mark of how
  far its mail was fetched is written only when it moves, so a save read past a damaged one could
  say mail is here that isn't); the saves from there on are counted. In the first save, which is
  the whole mailbox, a damaged item costs only itself, and every folder's fetch mark is forgotten
  so that missing mail comes again from its server. Either way `store::Opened::notice` says what
  was left out (`file.open` returns it as `lostSaves`, `lostRecords`, `unfinished` and `notice`;
  the window's status bar and the CLI show it), and the file is kept aside as
  `<file>.damaged-<time>` before it is next written; a file another program wrote since it was
  read here is kept as `<file>.changed-<time>` (one whose time changed but whose bytes are still what
  was written here, by their SHA-256, is still this program's); MailCraft never removes a file kept
  aside, as each may be the only copy of what it holds. The sweep keeps
  every content file a kept-aside file names (damaged, changed, or of an earlier form). Formats 1
  and 2 (JSON) are still read and become format 3 at the first changed save, the old file kept
  beside it as `<file>.format1` or `.format2`, which is what MailCraft 0.4.3 and earlier can open
  (they call a format-3 file not a mailbox); a file that begins as ours but in a form newer than
  this program reads is said to be from a newer MailCraft. A self-contained single file is still what the browser, exports
  and `file.saveAs` use. Everything is in memory while the app runs, which is the next limit
  (R6.1, see `docs/gaps.md`). An account of a kind this program doesn't know (one a newer
  MailCraft added: `AccountKind::Unknown`) doesn't stop the file from opening: only what says
  which account it is is read, the rest is kept as the file has it (`Account::as_written`) and
  written back so in every form, and nothing here sends, receives or changes its settings: going
  back to the version before, after an update that didn't start, loses nothing of it.
- **A Send/Receive arrives in pieces.** `mailcraft_net::sync_with` emits what was sent, what the
  server was told, then mail folder by folder (the Inbox first, newest first, fifty at a time),
  each piece safe to apply by itself (`apply_sync_part`), so mail shows while the rest is on its
  way and a run that is cut short loses nothing: the UID bookkeeping only advances past what was
  handed over.
- **Microsoft Graph beside the mail protocols.** For an account that signs in with Microsoft, the
  host first asks Graph to send the Outbox and, every quarter of an hour, for the calendar and
  contacts (`crates/net/src/graph.rs`); the engine merges them as server-kept, read-only items
  (`cmd/remote.rs`, `Origin`). Mail itself still goes over IMAP.
- **A Microsoft account kept in step through Graph alone (R3).**
  A Microsoft account is `AccountKind::Microsoft`: no mail servers, never IMAP or SMTP.
  One that was kept in step by IMAP switches when its owner says so (R3.7,
  `cmd/graph_switch.rs`, the page in `ui-egui/src/graph_switch.rs`): what waits for the IMAP
  server goes first, the mailbox is backed up beside its file, the server's mail is dropped and
  comes down afresh, and its folders wait (`Folder::awaiting_listing`) to be found by path in
  Graph's first complete listing, so that they keep their ids.
  Its folders and messages are known by server ids, beside what the server last said of them
  (`ServerFolder` on `Folder::server`, `ServerMessage` on `Message::server`;
  the server id stays the same when the item moves); a change made here is
  the difference between the two, and only what comparing can't show waits in
  `Mailbox::queued` (a folder deleted for good). The request and answer are Graph's own, beside
  the IMAP ones (`GraphMailRequest`, `FolderChange`, `GraphMailResult`, `FolderListing` in
  `crates/model/src/sync.rs`): `sync.graphRequest` / `graph_mail_request` and `sync.applyGraph` /
  `apply_graph_mail` (`crates/engine/src/cmd/graph_mail.rs`) are pure, and
  `mailcraft_net::graph::sync_folders` (`crates/net/src/graph/mail.rs`) does the talking on the
  host's Graph job: the changes first, then a walk of every folder (hidden and search folders
  left out, the tree from where each was found, names whole). What comes back is taken in by
  server id; the server wins but for what was changed here and hasn't reached it; a listing that
  didn't finish removes nothing. Every request of the mail code asks for immutable ids and goes by
  `graph::pace`: one gate per mailbox (4 requests at once, 2 for background work, all paused
  while the server says it is busy) and one retry policy by kind (reads and same-every-time
  changes again after `Retry-After`, a POST never by itself). The token is asked for with
  `Mail.ReadWrite` (`oauth::refresh_graph_mail`, `Resource::GraphMail`) for these accounts only.
  R3.1 covers the folders; mail comes in R3.2.
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
- **One way of reading them.** Graph is the only way, for an account of an organisation as for
  a personal one: there is no setting of the account for it and no choice in the window.
  Whether an account is an organisation's is still read from the directory its sign-in names
  (the identity token's `tid`), for the address an administrator is sent to when the
  organisation has to approve the program first ([`approval`](../crates/ui-egui/src/approval.rs)).
  A mailbox written by 0.2.0 or 0.2.1, when Exchange Web Services was a second way an owner could
  choose, may say `favoritesSource` for an account: it is read into
  `Account::retired_favorites_choice`, never written back, and the owner is told once that the
  Favorites now come through Graph (`Mailbox::take_retired_favorites_choices`, by `file.open`
  and by the desktop app as it starts).
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
  Windows, macOS, and the Secret Service on Linux and FreeBSD; a store that can't be used is
  `StoreTrouble`, said once a session). The mailbox file, `ui.json` and logs never hold one.
- **Signing in on the provider's page.** Microsoft accounts use OAuth 2.0 (authorization code with
  PKCE, a loopback redirect, the system's browser) and XOAUTH2. The engine only knows that an
  account signs in that way (`Account::oauth`); the host with sockets does it and hands tokens in.
- **Plain-text composing.** Rich text needs an editor component the project does not have yet.

## Start-up

The window comes up at once, at its remembered size and in its theme, and the mailbox is read on
a thread of its own (`apps/mailcraft/src/launch.rs`, #519); the window waits in
`crates/ui-egui/src/opening.rs`. If reading takes longer than 0.3 s it shows the opening screen:
the bytes read of the file's (`mailcraft_engine::io::OpenProgress`, counted by
`store::open_with`), then "Getting ready…". Until the mailbox is there the window can be moved,
resized and closed and nothing else: every command, and every control request but `app.status`,
is refused with "the mailbox is still opening"; files and links handed over (a second start,
Finder) wait and open after; nothing reaches a server and nothing is written, so closing it then
leaves the mailbox and the settings as they were. A panic on the reading thread is a mailbox that
couldn't be read. A file that can't be read, was damaged or was repaired is said on the same
screen with Continue, after which the window is made as it always was (an empty mailbox kept
nowhere, the damaged-mailbox notice). File › Open later reads the other file in the background
(`Services::read_ahead`, `store::open_ahead`) while the open mailbox stays in use, with its
progress and Cancel in the status bar; `file.open` then takes what was read. The browser build
uses the same screen, reading its storage after the first frame. Every open logs one line: the
time, the bytes and the counts of messages, folders and accounts.

The desktop window opens its mailbox with `store::open_for_window`, which leaves out what it
doesn't need before the first frame (#519): the SHA-256 of the whole file, which tells this
program's bytes from another writer's, is worked out on a thread of its own from the bytes read,
and every save waits for it first (it is only of bytes the reading itself took: a file cut short
has none, as before); only the newest 200 messages of the Inbox (the view the window opens on)
have their bodies read at once; and the content files left on disk are looked for afterwards.
Once the window shows, a thread the host starts looks for those files and reads the bodies the
memory budget holds that were left for later (`store::content_later`): the bodies found missing,
or whose file is there but damaged, are marked in their messages as an opening that looks at once
marks them (`Session::mark_unavailable`) and the "missing or damaged" count is said in the status
bar; then the window takes the bodies read in a few batches a frame (`Session::take_bodies`),
so that previews and everything else that reads bodies in memory end as after an opening that read
them at once. What comes for a mailbox another has taken the place of is dropped. The word index starts after the first
five frames. `store::open` and `file.open` open as before. `store_bench_tests.rs` times an open of
a made-up mailbox of 64,000 messages (ignored by `cargo test`).

## Testing and gates

- Unit tests in every crate; property tests feeding arbitrary bytes to the readers (`mime`,
  `ical`, `vcard`, `proto`) and round-tripping what the writers produce; in-process fake IMAP and
  SMTP servers on loopback for `net`; cargo-fuzz targets in `fuzz/` (its own workspace, nightly)
  for each reader (`mime`, `html`, `ical`, `vcard`, `charset`, IMAP and SMTP replies), run a minute
  each by `fuzz.yml` weekly and when a pull request changes a reader; `engine/src/tests.rs::hostile_params_never_panic` runs every
  command with junk params; an MCP acceptance test drives a realistic triage through the protocol.
- `cargo xtask ci`: fmt, clippy `-D warnings`, tests, asset attribution, layering, wasm build,
  `cargo deny check` ([`deny.toml`](../deny.toml): RustSec advisories, licences, sources; skipped
  with a hint when cargo-deny isn't installed, required in CI).
- Workflows pin every third-party action to a commit SHA (Dependabot keeps them current); zizmor
  checks them, OpenSSF Scorecard scores the repository. What all this defends against:
  [`threat-model.md`](threat-model.md).
- `cargo xtask parity` regenerates [`parity-checklist.md`](parity-checklist.md); a test keeps the
  catalog percentage above a floor that only rises.
- GitHub Actions (shared `storytold/craft-common-actions` workflows): packaging lint, FreeBSD and
  Windows ARM64 when build files change, release builds on pushes to `release`.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-11 | major | First architecture doc |
| 2026-10-26 | minor | #519: start-up: the window first, the mailbox read on a thread of its own behind the opening screen; File › Open reads in the background |
| 2026-10-23 | minor | `jmap`, L3: JMAP requests and answers without I/O (M6); 15 crates |
| 2026-10-15 | minor | Supply-chain gates: cargo-deny in `cargo xtask ci` and CI, pinned actions, zizmor, Scorecard; link to the threat model |
| 2026-10-12 | patch | `html` also makes the line a message list shows (`preview`, `text_preview`) |
| 2026-10-12 | minor | Calendar changes are checked against what comes down; the strict Graph stand-in |
| 2026-10-12 | minor | Time zones (M3.5): `time::Zone`, the mailbox's zone stamp and how stored times are read |
| 2026-10-13 | minor | The Favorites of Outlook (M4.2): `graph::favorites` in `net`, the server's list in the mailbox, `favorites_sync` in the front end |
| 2026-10-13 | minor | The owner chooses how they are read (M4.3): `ews` in `net` (Exchange Web Services, until 1 April 2027), `Account::favorites_source`, the choice and its job in the front end |
| 2026-10-15 | minor | Sign-ins remembered on Linux and FreeBSD (R5): the Secret Service behind `Services::secrets`, and `SecretStore::take_trouble` for a store that can't be used |
| 2026-10-15 | minor | Exchange Web Services removed (R2): Graph is the one way of reading the Favorites; a mailbox that chose the other opens and says so once |
| 2026-10-16 | minor | Quitting on macOS through `craft-quit` (craft-libs, a git dependency pinned by `rev`), for the apps alone (`cargo xtask layers`); Rust 1.95 |
| 2026-10-15 | minor | R3.1: `AccountKind::Microsoft` behind the switch for testing; server ids (`ServerFolder`, `ServerMessage`, `Mailbox::queued`); Graph's own sync types; `graph::mail` and `graph::pace` in `net`; `cmd/graph_mail.rs` in the engine |
| 2026-10-15 | minor | R6 (prerequisite of R6.1): the mailbox file in format 3, a log of records a save appends to (`store_index.rs`), crash-safe, formats 1 and 2 migrated with the old file kept; a damaged later save stops the reading there, a damaged item of the first save costs only itself and the mail is fetched again (`Opened::notice`, `file.open`'s `lostSaves`, `lostRecords`); a file another writer changed kept aside; kept-aside files protected from the sweep |
| 2026-10-17 | minor | R6.1 review: files kept aside are never removed by MailCraft; a file whose time changed but whose bytes didn't is still this program's (`Stamp::sum`); a damaged commit of the first save loses no item |
| 2026-10-23 | minor | Quitting on macOS is MailCraft's own code (`quit_macos.rs`); the `craft-quit` git dependency is gone, and `dispatch2` is the app-only crate in `cargo xtask layers` |
| 2026-10-16 | patch | The Graph-only account described without citing private decision records |
| 2026-10-16 | minor | R3.7: the switch from IMAP to Graph (`cmd/graph_switch.rs`, `Account::graph_switch`, `Folder::awaiting_listing`); the switch for testing is gone |
| 2026-10-18 | minor | `charset` (L0): the decoder of character sets moved out of `mime` so that `vcard` reads `CHARSET=` by the same rules, #180 |
| 2026-10-19 | minor | Fuzz targets (`fuzz/`, cargo-fuzz) for every reader, run by `fuzz.yml` |
| 2026-10-20 | minor | An account of a kind this program doesn't know (`AccountKind::Unknown`, one a newer MailCraft added) is kept as its file has it (`Account::as_written`), written back so, and never sent from, received for or changed here |
| 2026-10-20 | minor | `i18n` (L0): interface text, a `.po` reader with plural rules and the languages moved out of `ui-egui`, which keeps `tl!` and gains `tln!`, i18n-1 |
| 2026-10-21 | minor | #253: the macOS menu bar: a table of command ids in `crates/ui-egui/src/menubar.rs`, made native with `muda` in `apps/mailcraft/src/menubar.rs` |
| 2026-10-25 | minor | #350: `link` (L0): MailCraft's own reader of web addresses and Punycode replaces the `url` and `webbrowser` crates; pages are opened by `rundll32`, `open` or the desktops' openers as programs, never through a shell |
| 2026-10-27 | minor | #454: `names` (L0): one list of Unicode's format characters, and the two steps a name takes, coming in and being shown, which also remove the other characters that show nothing; the count of crates is 17 |
| 2026-10-27 | minor | #454 part 2: `link` and `html` use `names`' list of format characters instead of copies of their own; `link` depends on `names`, the second exception to the layers |
| 2026-10-27 | minor | #519 slice 2: opening for the window: the whole-file sum in the background, bodies of the opening rows only, content files looked for afterwards, the word index after the first frames |
| 2026-10-27 | patch | #519 slice 2 review: bodies found missing are marked, the bodies the budget holds come in the background, a cut-short file has no sum |
| 2026-10-27 | patch | #519 slice 2 review: a body whose file is damaged, read in the background, is marked and counted |
| 2026-10-28 | minor | M6: the browser build runs `jmap` over `fetch`; a JMAP account's Send/Receive in the data flow |
