# Importing accounts from GNOME

On Linux, **File › Info › Import accounts from GNOME…** reads the mail accounts enabled in
GNOME Online Accounts (GOA). Nothing is linked automatically. Select an account and choose:

- **Link with GNOME**: GOA keeps configuration and authentication. MailCraft stores a reference
  to the selected account and its mail connection settings, and keeps its own copy of mail.
  Linking starts Send/Receive for that account. Send/Receive asks GOA to ensure its credentials and obtain a current mail token or password.
  No second MailCraft sign-in is opened and there is no fallback to one. Incoming and outgoing
  passwords and login names can differ. Passwords and tokens stay in the worker's memory;
  MailCraft never stores them or requests a refresh token or GNOME's client registration.
- **Set up separately**: prefill MailCraft's ordinary setup for Google, Microsoft 365
  (`ms_graph`) or older Microsoft Personal (`windows_live`), then sign in through MailCraft's
  own provider registration. The resulting account is independent of GOA.

The ordinary Add Account action stays unchanged. An address already in the current mailbox
cannot be imported again. Accounts without an enabled mail service are omitted. Other
accounts remain visible with unsupported actions disabled.

## Linked mail capabilities

Linking requires GOA's public Mail interface to expose both IMAP and authenticated SMTP, with
TLS or STARTTLS and valid server and login settings. Certificate verification is always
required; configurations accepting invalid certificates are refused. Numeric port overrides
and bracketed IPv6 addresses are supported; symbolic service ports are not guessed.

PasswordBased accounts use the public `imap-password` and `smtp-password` credential methods.
OAuth2Based accounts currently support Google's Gmail mail endpoints and older Microsoft
Personal (`windows_live`) mail endpoints, only when GOA also advertises SMTP XOAUTH2. A
Microsoft 365 `ms_graph` account is available for separate setup, but cannot be linked through
this IMAP/SMTP backend: its Graph token is not an IMAP credential. Unknown OAuth profiles and
accounts exposing only Exchange or another protocol cannot be linked.

GOA mail credentials are used only for the selected account's IMAP/SMTP endpoints. They never
reach MailCraft's Google API, Graph, calendar, contact, task or shared-mailbox backends. GNOME
maintains the provider registration and credential refresh; MailCraft neither changes its
scopes nor opens a MailCraft sign-in if GOA refuses access.

GOA's [third-party application guidance](https://gnome.pages.gitlab.gnome.org/gnome-online-accounts/goals.html)
asks application maintainers to contact the GOA maintainers and GNOME design/release teams
before using GNOME's provider profiles. Maintainers should complete that upstream
coordination and provider terms review before shipping OAuth-linked accounts. This implementation does not claim that approval has
been obtained. Separate setup continues to use MailCraft's identity.

## Changes, failures and removal

File › Info identifies linked accounts as managed by GNOME. Their connection settings cannot
be edited in MailCraft. Each Send/Receive rechecks the selected GOA account, then calls
`EnsureCredentials` and `GetAccessToken` or `GetPassword` through public D-Bus interfaces.
Removing the GOA account, disabling mail, losing GOA access or needing a new sign-in stops the
run and explains what to repair in GNOME Online Accounts. Try Send/Receive again after repair.
Other MailCraft accounts remain usable. No account is imported when a new one appears in GOA.

A change of address, incoming login or mail servers requires removing the account from
MailCraft and linking it again: existing IMAP UIDs must not be associated with a different
mailbox. Outgoing login names and credentials are read fresh for each run. Removing an account
from MailCraft removes its local mail and reference; it never removes or changes the GOA
account. Calendars and contacts are not published to Evolution Data Server or GNOME Shell.

No GNOME client library is required to build or start MailCraft. Discovery and credential
resolution run on background threads using the existing Rust D-Bus dependency, with method
timeouts and cancellation checks. Discovery alone never requests credentials. A linked
account is resolved at Send/Receive, including scheduled runs; ordinary accounts do not query GOA. Both
Flatpak manifests allow communication with `org.gnome.OnlineAccounts`. On other platforms or
in a host without GOA support, linked accounts cannot sync; no local password is substituted.

## Verification

Synthetic ObjectManager replies cover disabled mail, unknown providers, malformed properties,
invalid addresses, duplicate entries, bounded account lists and secure protocol capabilities.
UI tests cover explicit discovery and linking, cancellation, duplicate accounts, persistent
references, separate setup and dropping discovery when the mailbox changes. Transport tests
exercise distinct SMTP credentials and prevent unresolved GOA references from connecting.
No test needs a real account.

The optional D-Bus integration tests must run on a private session, with no real GOA daemon:

```sh
dbus-run-session -- env MAILCRAFT_TEST_PRIVATE_BUS=1 cargo test --locked -p mailcraft --bin mailcraft private_bus_ -- --ignored --test-threads=1
```

These register synthetic GOA interfaces with invented `example.org` identities. They check
that discovery requests no credentials, each run ensures credentials again, incoming and
outgoing passwords differ, OAuth mail tokens stay in memory, and changed or disabled accounts
are refused before credentials are requested. Live provider and packaged Flatpak acceptance
remain separate from these synthetic checks.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-12 | minor | #989: optional discovery and separate account setup from GNOME |
