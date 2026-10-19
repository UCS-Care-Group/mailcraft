# Threat model

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-19 · **Change:** minor (JMAP, read: a JMAP server is a hostile server like the others, its credentials go to the server its session is served from and to no other, and the web build reads a JMAP account over `fetch`; agents with a shell or the owner's files are outside the agent gate; the settings folder, logs and kept pictures are the owner's alone) · **Target:** Microsoft Outlook (Microsoft 365)

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
  every format and protocol crate (`mime`, `html`, `ical`, `vcard`, `proto`, `jmap`, `net`, `model`,
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

Answers IMAP, SMTP, Microsoft Graph or JMAP with whatever it likes.

- Server responses are parsed as hostile input under the same never-crash rules and property tests
  (`crates/proto`, `crates/net`, `crates/jmap`); sizes are clamped and allocations capped.
- **Secrets aren't handed to the wrong party:** a sign-in goes only to the server the account names,
  over TLS (below). OAuth uses the authorization code with PKCE and a loopback redirect
  (`crates/net/src/oauth.rs`). A JMAP account's password or token goes to the server its session
  is served from and to no other: an address the session names on another server, or on the same
  one without encryption (plain `http` is for this computer alone), is refused
  (`crates/jmap`, `Client::connect`).
- A compromised server can still read and change the mail it holds. That is the server's job, not
  something a client can prevent; end-to-end encryption would be (see Known limits).

### A network attacker

Watches or changes traffic between MailCraft and its servers.

- **Encrypt or refuse.** Every connection that carries mail or a sign-in is TLS (rustls) or STARTTLS,
  with certificates checked against the Mozilla root set built in (`webpki-roots`;
  `crates/net/src/transport.rs`). An unencrypted connection is allowed only to loopback, for tests.
  (In the browser build a JMAP account's requests go through the browser's `fetch`, so its TLS and
  its trust store apply.)
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

## Out of scope and known limits

- **Malware running as the owner.** A program with the owner's rights can read the mailbox, the
  control key and, usually, the credential store. MailCraft doesn't defend against its own user's
  account being compromised.
- **No S/MIME or OpenPGP yet.** Messages are protected in transit (TLS) but not end to end, and
  signatures aren't checked.
- **The mailbox isn't encrypted at rest.** It relies on the operating system's disk encryption
  (BitLocker, FileVault, LUKS, GELI).
- **Unsigned builds.** The Windows and macOS builds aren't code-signed until signing and publisher
  verification are done (B10, roadmap reset step R8.1); check a download against
  the release's checksums.
- **The web build** runs in the browser's sandbox and inherits the browser's security. It has no
  IMAP, SMTP or Microsoft Graph connection; a JMAP account is read through the browser's `fetch`, over `https`
  only (plain `http` to this computer alone), with no cookies and the credential in the
  `Authorization` header alone, which the browser drops on a redirect to another origin and never
  sends from `https` to `http`. The browser's own TLS and CORS rules apply, so the server has to
  allow the page's origin. The credential is held in memory for the session and is not remembered.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-19 | minor | JMAP, read: a JMAP server among the hostile servers, its credentials sent only to the server its session is served from, and the web build's JMAP account read over `fetch` |
| 2026-10-19 | minor | Agents on the same computer: the agent gate binds agents that reach MailCraft only through MCP; one with a shell or the owner's files is outside it |
| 2026-10-19 | minor | The settings folder, the logs and the pictures kept from the web are created for the owner alone on Unix (0700, 0600); open ones from earlier versions are closed |
| 2026-10-16 | minor | Release signing in the protected `release` environment; draft releases |
| 2026-10-16 | minor | Pinned build tools with checksums; build provenance for release files |
| 2026-10-15 | major | First version: assets, adversaries, mechanisms with their files, known limits |
