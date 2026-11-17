# Importing accounts from GNOME

On Linux, **File › Info › Import accounts from GNOME…** reads the mail accounts enabled in
GNOME Online Accounts (GOA). Nothing is linked automatically. The dialog shows an account card
with its provider and availability, highlights your selection, and offers two choices:

- **Link with GNOME**: keep configuration and authentication in GOA. MailCraft stores a reference
  to the selected account and keeps its own copy of mail. Linking starts that account's first
  Send/Receive. GNOME supplies and refreshes its credentials; MailCraft never opens an alternative
  sign-in or stores GOA passwords, refresh tokens or client registration details. Access tokens
  remain in the host worker, including for Microsoft Graph: they never enter the interface's
  token caches or MailCraft's credential store.
- **Set up separately**: prefill the ordinary setup for Google, Microsoft 365 (`ms_graph`) or
  older Microsoft Personal (`windows_live`), then sign in through MailCraft's own provider
  registration. That account remains independent of GOA.

The ordinary Add Account action stays unchanged. An address already in the current mailbox
cannot be imported again. Accounts without an enabled mail service are omitted. Unsupported
accounts remain visible with their status, and their unavailable actions are disabled.

GOA is optional. Ordinary Microsoft Graph accounts keep MailCraft's own sign-in and token
renewal, whether GOA is absent, unused or managing a different account in the same mailbox.
Installing GNOME or adding a system account never converts an existing MailCraft account.

## Linked mail capabilities

### Microsoft 365 through Microsoft Graph

A GOA `ms_graph` account exposing the Mail and OAuth2Based interfaces can be linked directly.
Its lack of IMAP/SMTP settings is expected: it uses MailCraft's existing Microsoft Graph mail
backend over HTTPS. This covers reading folders and messages, downloading full messages and
attachments, sending, drafts, mail changes and searching on the server. Each mail run, full
message download and server search validates fresh GOA metadata, calls `EnsureCredentials`
and obtains an access token through `GetAccessToken` for that selected account. The existing
bounded 401 retry during a mail run also asks GOA again; MailCraft's OAuth refresh endpoint is
never used for the linked account.

The Graph worker accepts only the selected account's own mail. Its token is not reused for
calendar, contacts, tasks, mailbox settings, Favorites or shared-mailbox operations. It requests
no additional scopes: the permissions GNOME obtained and the provider actually grants determine
which mail operations succeed. If authorization or permissions are refused, repair the account
in GNOME Online Accounts and try Send/Receive again; it never falls back to MailCraft sign-in.
A host without the GOA adapter refuses the account before trying MailCraft's own OAuth flow.

### IMAP and SMTP

Other linked mail configurations require GOA's public Mail interface to expose IMAP and
SMTP with authentication, TLS or STARTTLS, and valid server and login settings. Certificate
verification is always required; configurations accepting invalid certificates are refused.
Numeric port overrides and bracketed IPv6 addresses are supported; symbolic ports are not guessed.

PasswordBased accounts use `imap-password` and `smtp-password`, with separate incoming/outgoing
passwords and login names when configured. OAuth2Based accounts support Google's Gmail mail
endpoints and older Microsoft Personal (`windows_live`) mail endpoints when GOA advertises SMTP
XOAUTH2. An account shown as unsupported lacks a capability this backend needs; its settings
are not guessed. Google mail tokens never reach Microsoft's Graph or Google's separate APIs.

GOA's [third-party application guidance](https://gnome.pages.gitlab.gnome.org/gnome-online-accounts/goals.html)
asks maintainers to contact the GOA maintainers and GNOME design/release teams before using
GNOME's provider profiles. Maintainers should complete that coordination and provider terms
review before shipping OAuth-linked accounts. This implementation does not claim that approval
has been obtained. Separate setup continues to use MailCraft's identity.

## Changes, failures and removal

File › Info identifies linked accounts as managed by GNOME. Connection settings and sign-in
cannot be changed in MailCraft. Removing a GOA account, disabling mail or losing its credentials
stops new operations and directs you to GNOME Online Accounts. Other MailCraft accounts remain
usable. New GOA accounts are never added automatically.

A change of address, incoming login, protocol or mail servers requires removing the account
from MailCraft and linking it again: existing server ids must not become associated with a
different mailbox. Outgoing IMAP/SMTP login names and credentials are read fresh for each run.
Removing an account from MailCraft removes its local mail and reference; it never removes or
changes the GOA account. Calendars and contacts are not published to Evolution Data Server or
GNOME Shell by this feature.

No GNOME client library is required to build or start MailCraft. Discovery and credential
resolution run on background threads using the existing Rust D-Bus dependency, with method
timeouts and cancellation checks. Discovery alone never requests credentials. Only selected
linked accounts query GOA during manual and scheduled operations. Both Flatpak manifests allow
communication with `org.gnome.OnlineAccounts`. Other platforms and hosts without the adapter
cannot sync GOA links and never substitute a local password or MailCraft login.

## Verification

Synthetic metadata covers disabled mail, unknown providers, malformed properties, invalid
addresses, duplicates, bounded account lists, secure mail capabilities and Graph-only profiles.
UI tests cover explicit selection, independent setup, persisted references, initial sync,
scheduled-run eligibility and dropping discovery on mailbox replacement. The dialog is checked
in light and dark, including selection and disabled states, using synthetic accounts.

Run D-Bus integration tests only on a private session without a real GOA daemon:

```sh
dbus-run-session -- env MAILCRAFT_TEST_PRIVATE_BUS=1 cargo test --locked -p mailcraft --bin mailcraft private_bus_ -- --ignored --test-threads=1
```

The three tests use invented identities and a synthetic GOA service. The Graph flow also uses
the in-process Microsoft Graph stand-in: reading, downloading, searching, sending, changes,
401 renewal and refusal/disabled/cancelled states never leave this computer. Scope checks
prevent calendar or other extra operations from requesting credentials. No test requires a
real account. Live provider and packaged Flatpak acceptance remain separate checks.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-12 | minor | #989: optional discovery and separate account setup from GNOME |
