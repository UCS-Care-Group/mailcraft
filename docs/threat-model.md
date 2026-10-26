# Threat model

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-26 · **Change:** minor · **Target:** Microsoft Outlook (Microsoft 365)

What MailCraft protects, from whom, and how, with the code that does it. Rules for working in the
code are in [`CLAUDE.md`](../CLAUDE.md) and [`AGENTS.md`](../AGENTS.md); how to report a
vulnerability is in [`SECURITY.md`](../SECURITY.md). A claim here that the code doesn't keep is a
bug: report it like one.

## Assets

| Asset | Where it lives |
|---|---|
| The owner's correspondence (messages, attachments, drafts) | the `.mailcraft` mailbox file and the folder of message content beside it; the servers |
| Sign-ins: passwords, OAuth refresh and access tokens | memory for the session (`MailApp::passwords`, `MailApp::tokens`); the operating system's credential store when the owner wants them remembered |
| Contacts, calendars, tasks | the mailbox file; the servers |
| The owner's privacy: when and whether they read a message, where they are, who they are | everything that leaves the computer |

## Adversaries and what MailCraft does about them

### A hostile sender

Writes any bytes into a message: crafted MIME, HTML, headers, attachments, pictures, links.

- **Nothing in a message runs.** HTML becomes display blocks in `crates/html`: no script engine, no
  CSS engine, no web view. The only presentation read from a message is the whitelist in
  `crates/html/src/style.rs`, from inline attributes; `<style>` blocks are ignored.
- **Nothing is fetched by itself.** Pictures attached to the message (`cid:`) are shown; pictures on
  the web only on the owner's say-so, and only `<img>` addresses (no backgrounds, style sheets,
  fonts or frames, nothing 2 px or smaller), so a tracking pixel doesn't fire.
- **"Always download pictures" is not a trust in a `From:` line.** A remembered sender or domain
  holds only for a message whose topmost `Authentication-Results` header says `dmarc=pass` (or
  `compauth=pass`) for the sender's domain **and was written by the account's own server**
  (`crates/ui-egui/src/reading/remote.rs`, `vouched`, `from_own_server`): for a Microsoft account any
  topmost header (Exchange Online stamps its own, with no authserv-id); for another account one whose
  authserv-id (RFC 8601) is a known id of its IMAP host's provider
  (`mailcraft_model::provider::authserv_ids`: `mx.google.com` for `imap.gmail.com`) or is the IMAP
  host's domain one label up, or a name under it (`imap.lumen.example` takes `mx.lumen.example`; a bare
  suffix such as `co.uk` never matches). Anything else, a message without that, or an account that
  is not IMAP or Microsoft is asked about one message at a time. Mail the server does not hold
  (brought in from a file: no UID on a server, no server record) is never vouched for, whatever
  headers the file carries. The header is read as RFC 8601 clauses: only a `dmarc` (or
  `compauth`) clause whose own result is `pass` counts, with the `header.from` of that same clause,
  which must be present and the sender's domain or one above it (Gmail names the organisation's
  domain: `shop.example` for mail from `news.shop.example`; a bare suffix such as `co.uk`, or a
  hosting platform's domain, never stands for the names under it); a `dmarc=pass` inside a
  comment, a quoted string or another method's clause is only text. A `compauth=pass` that names
  no `header.from` is not enough. Addresses are kept and
  compared as ASCII (the Kelvin sign is not a "k"), and mail anywhere under Junk E-mail, not only in
  it, never takes the lists or the switch for all mail. The limits: a provider whose id is neither
  in the table nor under its IMAP host's domain asks every time (nothing is guessed); the
  public-suffix check is a short list of common two-label suffixes, not the full list.
- **A meeting invitation does not rewrite a meeting it isn't about.** An invitation carrying the
  `UID` of a meeting on a local calendar changes or removes it only when it names the same
  organizer (`crates/engine/src/cmd/calendar/invitation.rs`, `same_organizer`); the bar shows the
  organizer's name and address, and says when the message did not come from that address.
- **A cancellation calls off only what its sender may.** "Remove from Calendar" is offered, and
  `calendar.removeCancelled` acts, only for a meeting that is somebody else's and, on a local
  calendar, when the cancellation names the meeting's organizer **and** comes from that address; a
  meeting an account's server keeps only for a cancellation received in that account
  (`may_call_off`). Any other cancellation is shown and the owner removes the meeting
  themselves. No DMARC word is needed: the organizer and the From line must agree, and the
  limit is that both are written by the sender.
- **Links** are shown with their real destination; only `http`, `https`, `mailto`, `tel` and `cid`
  are followed. A web address is read by MailCraft's own strict reader (`crates/link`, RFC 3986;
  no `url` crate): the host shown (after any `@`, an international name as `xn--`, a number as
  dotted decimal) is the host in the string that is launched, and what is ambiguous (a
  backslash, a control character, no `//`, a space or `%` in the host) is no link. The opener is
  a program given that one string as its argument, never a shell (`rundll32.exe url.dll,
  FileProtocolHandler` on Windows, `open` on macOS, `$BROWSER` and the desktops' openers
  elsewhere); `mailto:` and `tel:` are written without spaces or quotes first. There is no UTS
  \#46 mapping: a name is shown and launched as written, lower-cased.
- **Parsers never crash.** No `unwrap`, `expect`, `panic!` or `unsafe` in production code
  (`unsafe_code = "forbid"` in the workspace `Cargo.toml`, the clippy denials at every crate root);
  every format and protocol crate (`mime`, `html`, `ical`, `vcard`, `proto`, `net`, `model`,
  `engine`, `time`) has property tests that feed arbitrary input. `Session::run` turns an escaped
  panic into an error and leaves the mailbox as it was (`crates/engine/src/session.rs`).
- **Header injection.** A value that reaches a header loses CR, LF and NUL
  (`crates/mime/src/header.rs`, `clean`; `crates/mime/src/write.rs`), so a reply can't be made to
  carry headers the owner didn't write.

### A malicious picture host

Gets the request for a picture the owner asked for.

- `mailcraft_net::pictures::fetch` (`crates/net/src/pictures.rs`) is the only way a picture is
  fetched. It sends the address and nothing else: no cookies, no referrer, no credentials.
- **No pivot into the owner's network:** public hosts only. Loopback, private, link-local,
  unique-local and similar addresses are refused, checked on every redirect and against the
  addresses actually connected to (DNS rebinding).
- **Bounded:** at most five redirects (`MAX_REDIRECTS`), 8 MB (`MAX_BYTES`), a time limit, and what
  arrives must be a PNG, JPEG, GIF or WebP by its own bytes.
- The picture cache (`apps/mailcraft/src/picture_cache.rs`) names files by hashes and never writes
  the address; it is capped and read back through the same checks as a download.

### A hostile or compromised mail server

Answers IMAP, SMTP or Microsoft Graph with whatever it likes.

- Server responses are parsed as hostile input under the same never-crash rules and property tests
  (`crates/proto`, `crates/net`); sizes are clamped and allocations capped.
- **One command can't grow memory without bound.** An IMAP command's answer is read up to 32 MB of
  text outside literals and 250,000 responses (a FETCH of a folder, twice its EXISTS plus 1,000 if more), and a command with its answer gets its time plus at
  most 30 more minutes however much moves (`crates/proto/src/imap/mod.rs`,
  `crates/net/src/transport.rs`). A LIST keeps 4,000 mailboxes (the rest are read and dropped),
  none with a name over 1,024 bytes; a flag is cut at 256 bytes. A FETCH of a set keeps only UIDs
  in that set and below the mailbox's UIDNEXT, at most twice its EXISTS plus 1,000 distinct ones;
  more is the command's error and nothing of it is taken in.
- **Secrets aren't handed to the wrong party:** a sign-in goes only to the server the account names,
  over TLS (below). OAuth uses the authorization code with PKCE and a loopback redirect
  (`crates/net/src/oauth.rs`).
- A compromised server can still read and change the mail it holds. That is the server's job, not
  something a client can prevent; end-to-end encryption would be (see Known limits).

### A network attacker

Watches or changes traffic between MailCraft and its servers.

- **Encrypt or refuse.** Every connection that carries mail or a sign-in is TLS (rustls) or STARTTLS,
  with the server's certificate checked by the operating system's own verifier
  (`rustls-platform-verifier`; `crates/net/src/tls.rs`, the one check for mail, sign-ins and
  pictures alike) and its name matched. On Windows and macOS what the system trusts is trusted,
  and the system may fetch an intermediate certificate a server left out from the address its
  certificate names (it may also ask whether the certificate was revoked); on Linux and FreeBSD
  the system's certificate store is read and the chain checked against it, and a server that
  leaves out its intermediate is refused. There is no built-in root set to fall back on: where the
  system's verifier can't be set up, every secure connection is refused. An unencrypted connection
  is allowed only to loopback, for tests.
- **No silent downgrade:** a server that doesn't offer STARTTLS when it was asked for is an error
  ("the server doesn't offer STARTTLS", `crates/net/src/sync.rs`, `crates/net/src/send.rs`).
- **Provider discovery** asks a DNS-over-HTTPS resolver for the domain's mail exchangers
  (`mailcraft_net::lookup_mx`): only the domain goes out, never the mailbox name, and only when the
  owner presses Connect.
- The one thing fetched over plain `http` is a picture the owner asked for, when that is the address
  the message gives; nothing of the owner's travels with it.

### A hostile mailbox or import file

An `.eml`, `.mbox`, `.ics`, `.vcf` or `.mailcraft` file from somewhere else.

- Read by the same parsers, under the same never-crash rules and property tests; a file that can't
  be read is an error, never a crash, and a command that fails leaves the mailbox as it was.
- `crates/engine/src/tests.rs::hostile_params_never_panic` runs every command with junk parameters.
- An `.ics` file replaces an event already on a local calendar only when it names the same
  organizer (two that name nobody are the owner's own appointment, exported and imported again);
  another organizer's event with the same `UID` is added beside it. Its events are cut to size as an
  invitation's are (`tidy_imported`).
- Mail from an `.eml` or `.mbox` file is never put in an Outbox or a folder under one: with an
  Outbox in view it goes to the account's Drafts (as drafts, not queued), else its Inbox; naming an
  Outbox is refused (`crates/engine/src/io.rs`).
- Autocomplete (`compose.suggest`) takes nobody from mail in Junk E-mail, counts a date after now as
  now, and puts addresses the owner has written to (Sent Items) above those that only wrote. The
  people the search box offers after `from:`, `to:` and `cc:` (#308) come from it, so the same
  holds for them (and then the owner's own addresses, which it leaves out); nothing is asked of
  a server for them.
- A reply, forward or reopened draft quotes an HTML original as its HTML reads
  (`quote_text`), never the text/plain alternative the sender wrote beside it, which the owner
  never saw.
- A `.mailcraft` attachment is never opened from a message: it can only be saved
  (`crates/engine/src/opening.rs`), so a sender cannot put a mailbox with accounts of their
  choosing in front of the owner with one click.
- The passwords and sign-ins held in memory belong to the mailbox they were given for: any
  mailbox that takes its place (File › Open, File › New) drops them all, since a file chooses its
  accounts' ids and servers (`MailApp::drop_secrets_if_replaced`).
- When an agent asks to add or change an account, or to open a mailbox file, the owner's question
  names the servers the account would connect to, read from the file for an open
  (`crates/engine/src/agent.rs`).
- A file's rules are cut to the most a mailbox keeps (500) when it is repaired.
- A password or sign-in the system remembers is filed under the account's address and the
  servers it goes to (IMAP and SMTP host and port; a Microsoft account kept in step through Graph
  under the address alone, Graph and Microsoft's sign-in page being fixed): an account that names
  other servers, such as one in a mailbox from a file, finds nothing and asks. Changing an
  account's server drops what was kept for the old one, and so does removing the account.
  Secrets kept before this move once, for the mailbox the window starts with, to the servers of
  its accounts (`MailApp::secret_key`, `MailApp::migrate_secrets`).
- A mailbox opened from a file (File › Open, a double-click, a drop) is **unconfirmed** until the
  owner has seen where each of its accounts connects and confirmed it (`crates/ui-egui/src/unconfirmed.rs`).
  Nothing of it reaches a server: no automatic Send/Receive, no quiet push, a manual Send/Receive
  asks the owner to confirm, its Outbox is not sent, its changes wait, its rules do not run, and
  nothing is signed in for it. A bar says so and lists the servers; `ui.confirmServers` is the
  owner's yes (an agent needs the owner's say, class `accounts`). The yes is kept in `ui.json` for
  that file and a SHA-256 fingerprint of its accounts' addresses, kinds and servers: change either
  and the owner is asked again. The owner's own mailbox and one named with `--mailbox` are never
  unconfirmed, nor is a file with no account that connects to a server.
- No rule moves mail into an Outbox, or a folder under one, whoever made it: `rules.add` and
  `rules.fromMessage` refuse it, and applying rules skips such a target (a rule from a file or an
  older version), so incoming mail is never relayed by the owner's SMTP account
  (`Mailbox::is_send_queue`).
- An agent is asked before it deletes a contact, task, event or category (class `delete`; none
  has a Deleted Items to come back from), and before an Undo or Redo that would put back the
  accounts, rules or signatures of before. The question shows the call in full: one too long to
  show (`MAX_QUESTION`) is refused, never cut; a calendar question names the event and the people
  it writes to. `ui.copyDiagnostics` is not available to agents.
  Undo and redo swap the whole mailbox, so they are classed by what the step would bring back or
  remove, strongest first: accounts, a meeting change that writes to people (`send`), contacts,
  events, tasks, calendars or categories removed (`delete`), rules and signatures (`settings`).
  `ui.copySelection` (text the owner selected in a message) is never for agents, as
  `ui.copyLinkAddress` is; saving or sending an appointment in its window
  (`ui.eventWindow` `do`) runs later as the owner, so it asks `send` and the question names the
  people invited.
- **The bridge checks the app.** The MCP bridge (`mailcraft-cli mcp --connect`) sends the control
  key only to a program that has proved it holds it: it sends a random nonce and the app answers
  HMAC-SHA256 over it keyed with the control key (`auth.challenge`,
  [control protocol](control-protocol.md#keys)). A program that only took the loopback port, before
  the app or instead of it, gets neither the key nor a request.
- Counts are bounded as well as sizes. A message keeps at most 2,000 recipients (`To`, `Cc` and
  `Bcc` together; `Reply-To` as many again), 500 categories and 4 MB of header fields
  (`crates/mime/src/parse.rs`, `header.rs`); the reading pane lists the first 50 names and draws 8
  category chips, and a quote under a reply or forward the first 100 names, each with a count of the
  rest. An mbox import takes 200,000 messages and says it stopped (`crates/engine/src/io.rs`).
  A `.mailcraft` file is counted while it is read (`crates/model/src/load_limit.rs`): one that holds
  more items (messages, contacts, events, tasks, accounts, folders, categories) than four times the
  body budget allows at a kilobyte each is refused before it uses the memory. Loading keeps at most
  1,000 accounts, 50,000 folders and 1,000 categories, cleans their names, and repairs in time
  linear in the file (`Mailbox::repair`).

### Other programs and users on the same computer

- **Files are the owner's alone:** on Unix the mailbox file and its message content are created
  0600 and their folders 0700 (`crates/engine/src/store.rs`), and so are the settings folder, the
  logs and the pictures kept from the web (`apps/mailcraft/src/logging.rs`,
  `apps/mailcraft/src/picture_cache.rs`); a settings, log or picture folder that an earlier version
  left open to others is closed when MailCraft next uses it. On Windows they live in the owner's
  profile.
- **Secrets are in the operating system's credential store** (Windows Credential Manager, the macOS
  keychain, the Secret Service: GNOME Keyring, KWallet; `apps/mailcraft/src/secrets.rs`), never in
  the mailbox file, `ui.json`, a command line, a URL or a log line. They are `#[serde(skip)]` and redacted
  from `Debug` output (`SyncRequest`, `Tokens` in `crates/model/src/sync.rs`) and from error text. Access tokens are never stored.
  `ui.json` holds the key names of the entries MailCraft wrote (`rememberedKeys`, names only), so
  that the full wipe and turning Remember sign-in off delete all of them, whichever mailbox they
  were for, and report any that stay (AU2-18). Work started for a mailbox (a sign-in, the copy
  before a switch to Graph) records `Session::openings()` and is dropped when another mailbox has
  taken its place; a mailbox waiting for confirmation starts no switch (AU2-15, AU2-16). A
  `mailto:` link handed in from outside saves a message with changes to Drafts first (AU2-19).
- **The control channel** (only when started with `--control`) listens on `127.0.0.1` only, and every
  request must carry a key: 32 random bytes from the operating system, written to a key file created
  0600 on Unix and compared in constant time (`apps/mailcraft/src/control_server.rs`,
  `crates/control-key`). Another user can't drive the app; another program running as the owner
  can read the key, which is the next item's problem.
- **Agents on the same computer.** The agent gate ([MCP](mcp.md#agents-get-only-what-the-owner-allows))
  binds agents that reach MailCraft only through MCP. The control channel takes a request without
  the bridge's `agent` marking for the owner's own script, so an agent that can run a shell or read
  files as the owner (it could read the control key in the settings folder, or the mailbox file
  itself, or run `mailcraft-cli`) is outside the gate. Run such an agent as another user or in a
  sandbox, and don't start the app with `--control` while one is around. With
  `--control-agents-only` every control request is an agent's, allowed nothing in advance, so the
  running window can't be driven past the owner's question; the mailbox file and `mailcraft-cli`
  stay within such an agent's reach.

### A supply-chain compromise

A malicious or vulnerable dependency, GitHub Action or build step.

- **Dependencies:** `cargo deny check` ([`deny.toml`](../deny.toml); `cargo xtask deny`, a job of CI
  that `ci-ok` requires) fails on a RustSec vulnerability, allows only OSI-permissive licences (no
  GPL, LGPL or AGPL), and only crates.io as a source. `Cargo.lock` is committed, and CI and the
  release build with `--locked`, so the lock that is audited is the one that ships. The same
  check runs daily on a schedule (`audit.yml`, the workspace and `fuzz/`'s own lock), and Dependabot
  proposes crate updates weekly. `.github/CODEOWNERS` asks the owners to review the workflows,
  packaging, xtask, the lock files, the updater and the agent gate.
- **Workflows:** every third-party action is pinned to a full commit SHA, kept current by Dependabot
  with a week's cooldown; checkouts don't keep credentials; workflows get read-only permissions
  unless a job needs more; the release passes its signing secrets by name, not `inherit`.
- **Release signing:** the signing secrets are kept in the protected `release` environment (version
  tags only, a maintainer's approval), and only the jobs that sign a pushed version tag, and the
  one that makes the release, run in it; a dry run started by hand, on any ref, is unsigned and
  gets no secrets. A pushed tag is built and signed only when its commit is on `main` and `ci-ok`
  passed on it (checked by the first job, which the others need). Dependencies are compiled in a
  step with no signing secret in its environment (Windows and macOS builds, and `xtask`, which
  signs the checksums), and the secrets reach only the step that signs; release builds use no build
  cache. The workflow makes a draft release, which a maintainer publishes by hand.
  [zizmor](https://docs.zizmor.sh/) checks the workflows on every change to them, and
  [OpenSSF Scorecard](https://scorecard.dev/) scores the repository weekly.
- **Build tools:** the shared workflows are pinned to full commit SHAs too, and every tool the
  release downloads by URL (nfpm, appimagetool and the AppImage runtime, the Trusted Signing
  client, rustup-init, wasm-opt) and WiX (a NuGet package) is an exact version checked against a recorded sha256 before
  use; trunk runs offline. Every release file gets a build provenance attestation
  (`gh attestation verify <file> -R UCS-Care-Group/mailcraft`).
- Pure-Rust TLS (rustls), and no handwritten JavaScript.

## Assurance case

Why MailCraft's security requirements are met, and the evidence for it.

**Claim.** MailCraft keeps the owner's correspondence and sign-ins confidential and intact against
the adversaries above: nothing in a message runs or reaches out by itself, sign-ins never leave
memory or the operating system's credential store, and hostile input never crashes it or
corrupts the mailbox.

**Threat model.** The assets and adversaries above, each with the mechanism that answers it and
the file that holds it.

**Trust boundaries.** Everything from outside is hostile and checked where it crosses in:
messages and attachments (`crates/mime`, `crates/html`, `crates/ical`, `crates/vcard`,
`crates/charset`); mail servers and Microsoft Graph (`crates/proto`, `crates/net`); the web, for
pictures the owner asked for (`mailcraft_net::pictures::fetch`, the only way out); mailbox and
settings files (`crates/engine/src/store.rs`); commands from the control channel and MCP
(`crates/engine`, the agent gate in `docs/mcp.md`).

**Secure design principles applied.**

| Principle | How |
|---|---|
| Least privilege | Agents get only what the owner grants (Safe, Owner, Never); the Microsoft permissions are only those used, optional ones asked for by themselves; settings, logs and kept pictures are for the owner alone |
| Fail-safe defaults | Pictures from the web off; an unencrypted connection only to loopback; a missing STARTTLS is an error; an update that doesn't verify is not kept |
| Economy of mechanism | No web view, no script or CSS engine: HTML becomes display blocks from a short whitelist (`crates/html/src/style.rs`) |
| Complete mediation | Every action is a command through one registry, whichever surface asked for it; every picture through one fetcher, checked on every redirect |
| Separation | The engine does no I/O with servers; the host does it on a background thread and the engine only applies results |
| Input validation | Sizes, counts and depths clamped; `get()` instead of indexing; strings sliced at character boundaries; CR, LF and NUL stripped from headers |

**Common weaknesses countered.**

| Weakness | Countered by |
|---|---|
| Memory safety (CWE-119, -416, -787) | Rust, with `unsafe_code = "forbid"` in every crate |
| Crashes and panics (CWE-248, -400) | No `unwrap`, `expect` or `panic!` in production code (denied by lint); a command that fails leaves the mailbox as it was |
| Injection into headers and protocol lines (CWE-93, -113) | CR, LF and NUL stripped from every header value and protocol argument |
| Cross-site scripting and active content (CWE-79) | No script engine; HTML reduced to display blocks |
| Server-side request forgery from a message (CWE-918) | Pictures only on the owner's say-so, public hosts only, re-checked on every redirect and connection |
| Cleartext transmission (CWE-319) | TLS or STARTTLS for everything of the owner's; rustls, TLS 1.2 and 1.3 only |
| Insecure storage of credentials (CWE-256, -522) | Sign-ins in memory or the operating system's credential store only; redacted from logs and errors |
| Path traversal and deceptive file names (CWE-22, -451) | Every name from mail cleaned before saving: no directions reversed, invisible characters or reserved names |
| Download of code without integrity check (CWE-494) | Updates kept only when MailCraft's own ed25519 signature over `SHA256SUMS.txt` verifies and the checksum matches |

**Evidence.**

- Tests: 2,286, including a test that runs every command with junk parameters
  (`crates/engine/src/tests.rs::hostile_params_never_panic`), property tests that feed arbitrary
  bytes to every reader, and in-process fake servers (`crates/net/src/fake.rs`).
- Fuzzing: a cargo-fuzz target for each reader (`fuzz/`), run in CI weekly and when a reader changes.
- Static analysis on every pull request: clippy with warnings as errors, CodeQL, `cargo deny`
  (advisories, licences, sources), zizmor for the workflows; OpenSSF Scorecard.
- Reviews: changes to sync, the store, sign-in and secrets are read by a code owner
  (`.github/CODEOWNERS`); an external security review's findings were fixed in 0.5.1.
- Vulnerabilities reported privately ([`SECURITY.md`](../SECURITY.md)) are fixed and published as
  advisories crediting the reporter.

## Out of scope and known limits

- **Malware running as the owner.** A program with the owner's rights can read the mailbox, the
  control key and, usually, the credential store. MailCraft doesn't defend against its own user's
  account being compromised.
- **No S/MIME or OpenPGP yet.** Messages are protected in transit (TLS) but not end to end, and
  signatures aren't checked.
- **The mailbox isn't encrypted at rest.** It relies on the operating system's disk encryption
  (BitLocker, FileVault, LUKS, GELI).
- **Builds not signed by Windows or macOS.** Releases carry MailCraft's own signature over their
  checksums, which the program checks before it installs an update, but the Windows and macOS
  builds aren't code-signed yet, so the systems warn before running them; check a first download
  against the release's checksums.
- **Message-IDs are not identities.** A sender chooses a Message-ID, so after a server numbers an IMAP mailbox anew, a deletion or move that waits is made again only on the one arrival with that id that also has the same date, sender, subject and recipients; otherwise it is dropped and the owner told. A Graph forward's attachment is taken for the owner's only when its bytes match, and a saved draft is written whole before it is sent, so an edit made on the server meanwhile is not sent. A Microsoft server's Outbox is an ordinary folder: only mail queued on this computer is sent.
- **The web build** runs in the browser's sandbox and inherits the browser's security; it has no
  connection to mail servers of its own. Links open in a new tab with `noopener,noreferrer`
  (only `http`, `https`, `mailto` and `tel`), its headers send `Cross-Origin-Opener-Policy:
  same-origin`, and a `?sample` or `?empty` start never writes the mailbox kept in the browser.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-26 | minor | #501: server certificates are checked by the operating system's verifier instead of a built-in Mozilla root set: Windows and macOS trust what the system trusts and may fetch a missing intermediate; Linux and FreeBSD use the system's certificate store |
| 2026-10-26 | minor | #458: a `dmarc=pass` whose `header.from` is the organisation's domain (Gmail's way: `shop.example` for `news.shop.example`) vouches for the names under it; a bare suffix or a hosting platform's domain never does |
| 2026-10-25 | minor | #308: the people the search box offers after `from:`, `to:` and `cc:` come from autocomplete's source, on this computer only |
| 2026-10-25 | minor | #350: web addresses are read by MailCraft's own strict reader (`crates/link`), not the `url` crate, and opened by a program with the address as its one argument on every system (`rundll32 url.dll,FileProtocolHandler` on Windows, `open` on macOS, `$BROWSER` and the desktops' openers elsewhere), not the `webbrowser` crate; `mailto:` and `tel:` are written without spaces or quotes before they are launched; ambiguous addresses (backslash, no `//`, a percent sign or a full stop of another script in a host) are no links |
| 2026-10-25 | minor | Batch P4: a change waiting over an IMAP renumbering is made only on a message that is the same mail, not by Message-ID alone; a forward's attachments are matched by content; a saved Graph draft is sent as the owner has it; a Microsoft server's Outbox is never the send queue |
| 2026-10-25 | minor | Security run 2, batch P3: cancellations need the organizer's address or the keeping account; an `.ics` import replaces only the same organizer's meeting; no import into an Outbox; autocomplete skips Junk and clamps dates; replies quote what the HTML reads as; vouching reads the `dmarc` clause alone and never for mail from a file |
| 2026-10-25 | minor | AU2-P6: every link the window follows passes the opener's check on every system (the web address is parsed, needs a host, and what is launched is the parser's text); a comma or semicolon in an attachment's name becomes an underscore, so Explorer is never handed a second argument; .deskthemepack, .afploc, .ftploc, .vncloc, .wcx, .its, .cdr and .smi are never opened; a Microsoft sign-in (XOAUTH2) is made only to Microsoft's own mail servers or this computer, and a shared mailbox's name for a stored sign-in belongs to a Microsoft account alone; in the browser build links open in a new tab with `noopener,noreferrer`, pages are served with `Cross-Origin-Opener-Policy: same-origin`, and `?sample` or `?empty` never writes the mailbox kept in the browser |
| 2026-10-23 | minor | C1 PR-c: a mailbox opened from a file is unconfirmed until its servers are confirmed; nothing of it reaches a server meanwhile |
| 2026-10-23 | minor | C1 PR-b: remembered passwords and sign-ins are filed under the address and the servers they go to; changing a server asks again; older entries move once |
| 2026-10-25 | minor | The agent gate covers undo and redo of what was deleted or sent, `ui.copySelection` and the appointment window's save and send; the MCP bridge makes the app prove it holds the control key before sending it |
| 2026-10-23 | minor | Batch 5: attachments saved with Save As are marked from the Internet (Windows zone, macOS quarantine; Linux has no such mark); the launchable kinds that are never opened grow, legacy Office binaries ask first, and the same list bars the files an agent may have written; a web link the parser rejects is never handed to the opener |
| 2026-10-23 | minor | Batch 4: no rule moves mail into an Outbox; agents are asked before deleting contacts, tasks, events and categories, and before an undo that reverts rules, signatures or accounts; questions show complete parameters or are refused; calendar questions name the event and the people invited; Copy Diagnostics is not for agents |
| 2026-10-23 | minor | C1 PR-a: a `.mailcraft` attachment is saved, never opened; secrets in memory dropped when a mailbox is replaced; agent questions for account and open commands name servers; rules trimmed on repair |
| 2026-10-23 | minor | Supply chain: release builds only from `main` after CI; compile and sign in separate steps; WiX checked by hash; `--locked`; daily cargo-deny; Dependabot for crates; wider CODEOWNERS |
| 2026-10-23 | minor | iCalendar work is bounded as a whole: a zone a file defines is resolved once per offset, its parts and dates are capped at 64, one parse has a work budget (past it the times are kept as written and a note says so), an invitation's card reads its first event only, and a series with a count is not walked from its first day (PR-d) |
| 2026-10-23 | minor | Counts bounded: recipients, categories and header bytes per message, mbox import, items while a mailbox file is read, accounts, folders and categories on load; repair linear (PR-e) |
| 2026-10-23 | minor | An IMAP server's answer to one command is capped in text, responses, folders, messages and time; flags and mailbox names are cut short |
| 2026-10-21 | minor | The HTML whitelist adds padding and margins (0 to 64 px), lines (1 to 4 px, solid, dashed or dotted), `border-radius` (up to 24 px) and `line-height` (1 to 2.5 times), all clamped, functions refused (#71) |
| 2026-10-21 | minor | The HTML whitelist adds `width` (tables, cells), `max-width` and centring by automatic margins, all clamped (#150) |
| 2026-10-20 | minor | An assurance case: the claim, trust boundaries, secure design principles, common weaknesses countered, evidence; releases carry MailCraft's own signature |
| 2026-10-19 | minor | Agents on the same computer: the agent gate binds agents that reach MailCraft only through MCP; one with a shell or the owner's files is outside it |
| 2026-10-19 | minor | The settings folder, the logs and the pictures kept from the web are created for the owner alone on Unix (0700, 0600); open ones from earlier versions are closed |
| 2026-10-16 | minor | Release signing in the protected `release` environment; draft releases |
| 2026-10-16 | minor | Pinned build tools with checksums; build provenance for release files |
| 2026-10-15 | major | First version: assets, adversaries, mechanisms with their files, known limits |
| 2026-10-25 | minor | Late sign-in and Graph-switch copy dropped after a mailbox swap; credential-store key names indexed so the wipe and Remember-off remove every entry; a handed mailto: keeps the open draft (AU2-15, 16, 18, 19) |
| 2026-10-23 | minor | Always-download pictures need the server's `dmarc=pass`, ASCII addresses, and Junk subfolders count as Junk; an invitation changes a local meeting only for its organizer, whose address is shown; the update question names installing; the log keeps no subjects, addresses or folder names |
| 2026-10-22 | minor | `--control-agents-only`: every control request is an agent's, allowed nothing in advance; the mailbox file and `mailcraft-cli` stay within reach of an agent running as the owner |
