# Threat model

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-23 · **Change:** minor (`--control-agents-only`: no control request passes for the owner's script; batch 4: no rule moves mail into an Outbox; agents are asked before deleting contacts, tasks, events and categories, and before an undo that reverts rules, signatures or accounts; questions show complete parameters or are refused; calendar questions name the event and the people invited; Copy Diagnostics is not for agents; releases build and sign only from `main` after a passing CI, compile and sign in separate steps, check WiX by hash and use `--locked`; a daily cargo-deny run; Dependabot for crates; CODEOWNERS covers the workflows, packaging, xtask, the lock files, the updater and the agent gate; a calendar file's time zone definitions, dates and series cost bounded work: a hostile invitation or calendar file can't freeze the window, PR-d; PR-e: a message keeps at most 2,000 recipients and 500 categories and 4 MB of header, a long list is cut in the reading pane and in a quote, an mbox import stops at 200,000 messages, a mailbox file is checked for its item count while it is read, repairing a mailbox is linear; a mail server's answer to one command is capped in text, responses, folders, messages and time, so it can't make MailCraft keep memory without bound; Batch 5b: agents are not written anything that asks before opening; Batch 5: saved attachments marked from the Internet, more launchable kinds blocked, one list for agents and the opener, unparseable web links refused; C1 PR-a: a mailbox in mail is saved, never opened; secrets in memory dropped when a mailbox is replaced; agent questions name servers; the whitelist adds padding, margins, lines, rounded corners and a line height, all clamped, #71; the whitelist adds width, max-width and centring by automatic margins, #150; an assurance case: claim, trust boundaries, design principles, common weaknesses and the evidence; releases carry MailCraft's own signature; agents with a shell or the owner's files are outside the agent gate; the settings folder, logs and kept pictures are the owner's alone) · **Target:** Microsoft Outlook (Microsoft 365)

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
- **Links** are shown with their real destination; only `http`, `https`, `mailto`, `tel` and `cid`
  are followed.
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
  with certificates checked against the Mozilla root set built in (`webpki-roots`;
  `crates/net/src/transport.rs`). An unencrypted connection is allowed only to loopback, for tests.
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
- No rule moves mail into an Outbox, or a folder under one, whoever made it: `rules.add` and
  `rules.fromMessage` refuse it, and applying rules skips such a target (a rule from a file or an
  older version), so incoming mail is never relayed by the owner's SMTP account
  (`Mailbox::is_send_queue`).
- An agent is asked before it deletes a contact, task, event or category (class `delete`; none
  has a Deleted Items to come back from), and before an Undo or Redo that would put back the
  accounts, rules or signatures of before. The question shows the call in full: one too long to
  show (`MAX_QUESTION`) is refused, never cut; a calendar question names the event and the people
  it writes to. `ui.copyDiagnostics` is not available to agents.
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
- **The web build** runs in the browser's sandbox and inherits the browser's security; it has no
  connection to mail servers of its own.

## Revision history

| Date | Change | Summary |
|---|---|---|
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
| 2026-10-22 | minor | `--control-agents-only`: every control request is an agent's, allowed nothing in advance; the mailbox file and `mailcraft-cli` stay within reach of an agent running as the owner |
