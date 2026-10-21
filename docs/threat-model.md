# Threat model

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-21 · **Change:** minor (the whitelist adds width, max-width and centring by automatic margins, #150; an assurance case: claim, trust boundaries, design principles, common weaknesses and the evidence; releases carry MailCraft's own signature; agents with a shell or the owner's files are outside the agent gate; the settings folder, logs and kept pictures are the owner's alone) · **Target:** Microsoft Outlook (Microsoft 365)

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
  sandbox, and don't start the app with `--control` while one is around.

### A supply-chain compromise

A malicious or vulnerable dependency, GitHub Action or build step.

- **Dependencies:** `cargo deny check` ([`deny.toml`](../deny.toml); `cargo xtask deny`, a job of CI
  that `ci-ok` requires) fails on a RustSec vulnerability, allows only OSI-permissive licences (no
  GPL, LGPL or AGPL), and only crates.io as a source. `Cargo.lock` is committed.
- **Workflows:** every third-party action is pinned to a full commit SHA, kept current by Dependabot
  with a week's cooldown; checkouts don't keep credentials; workflows get read-only permissions
  unless a job needs more; the release passes its signing secrets by name, not `inherit`.
- **Release signing:** the signing secrets are kept in the protected `release` environment (version
  tags only, a maintainer's approval), and only the jobs that sign a pushed version tag, and the
  one that makes the release, run in it; a dry run started by hand, on any ref, is unsigned and
  gets no secrets. The workflow makes a draft release, which a maintainer publishes by hand.
  [zizmor](https://docs.zizmor.sh/) checks the workflows on every change to them, and
  [OpenSSF Scorecard](https://scorecard.dev/) scores the repository weekly.
- **Build tools:** the shared workflows are pinned to full commit SHAs too, and every tool the
  release downloads by URL (nfpm, appimagetool and the AppImage runtime, the Trusted Signing
  client, rustup-init, wasm-opt) is an exact version checked against a recorded sha256 before
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
| 2026-10-21 | minor | The HTML whitelist adds `width` (tables, cells), `max-width` and centring by automatic margins, all clamped (#150) |
| 2026-10-20 | minor | An assurance case: the claim, trust boundaries, secure design principles, common weaknesses countered, evidence; releases carry MailCraft's own signature |
| 2026-10-19 | minor | Agents on the same computer: the agent gate binds agents that reach MailCraft only through MCP; one with a shell or the owner's files is outside it |
| 2026-10-19 | minor | The settings folder, the logs and the pictures kept from the web are created for the owner alone on Unix (0700, 0600); open ones from earlier versions are closed |
| 2026-10-16 | minor | Release signing in the protected `release` environment; draft releases |
| 2026-10-16 | minor | Pinned build tools with checksums; build provenance for release files |
| 2026-10-15 | major | First version: assets, adversaries, mechanisms with their files, known limits |
