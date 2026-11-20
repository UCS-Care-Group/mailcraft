# Importing accounts from GNOME

On Linux, **File › Info › Import accounts from GNOME…** reads the mail accounts enabled in
GNOME Online Accounts (GOA). Nothing is linked automatically. The dialog shows an account card
with its provider and availability, highlights your selection, and offers two choices:

- **Link with GNOME**: keep configuration and authentication in GOA. MailCraft stores a reference
  to the selected account and keeps its own copy of mail. Linking starts that account's first
  Send/Receive using the IMAP and SMTP passwords supplied by GNOME. MailCraft keeps these
  passwords in worker memory for the duration of the run.
- **Set up separately**: prefill the ordinary setup for Google, Microsoft 365 (`ms_graph`) or
  older Microsoft Personal (`windows_live`), then sign in through MailCraft's own provider
  registration. That account remains independent of GOA.

The ordinary Add Account action stays unchanged. An address already in the current mailbox
cannot be imported again. Accounts without an enabled mail service are omitted. Unsupported
accounts remain visible with their status, and their unavailable actions are disabled. Supported
generic IMAP/SMTP profiles also offer separate setup. If you edit the prefilled address, setup
discards the imported provider hint and detects the edited address normally.

GOA is optional. Ordinary Microsoft Graph accounts keep MailCraft's own sign-in and token
renewal, whether GOA is absent, unused or managing a different account in the same mailbox.
Installing GNOME or adding a system account never converts an existing MailCraft account.

## Linked mail capabilities

Linked mail configurations require GOA's public Mail interface to expose IMAP and
SMTP with authentication, TLS or STARTTLS, and valid server and login settings. Certificate
verification is always required; configurations accepting invalid certificates are refused.
Numeric port overrides and bracketed IPv6 addresses are supported; symbolic ports are not guessed.

PasswordBased accounts use `imap-password` and `smtp-password`, with separate incoming/outgoing
passwords and login names when configured. If the outgoing credential alone is refused or
invalid, sending waits in the Outbox while receiving and IMAP changes continue; the incoming
password is never substituted for the outgoing one. For Gmail and Microsoft accounts using
OAuth in GOA, choose **Set up separately** to sign in through MailCraft.

## Changes, failures and removal

File › Info identifies linked accounts as managed by GNOME. Connection settings and sign-in
cannot be changed in MailCraft. Removing a GOA account, disabling mail or losing its credentials
stops new operations and directs you to GNOME Online Accounts. Other MailCraft accounts remain
usable. You can change the local account name, sender display name, colour and signature.
The import tile checks for a running or activatable
GOA service without listing accounts. MailCraft repeats that check every 30 seconds so
a service that starts later becomes available for import and scheduled sync. An authorised MCP
or control-channel caller can also select a link explicitly through `account.add`. New GOA
accounts are never added automatically.

A change of address, incoming login, protocol or mail servers requires removing the account
from MailCraft and linking it again: existing server ids must not become associated with a
different mailbox. Outgoing IMAP/SMTP login names and credentials are read fresh for each run.
Removing an account from MailCraft removes its local mail and reference; it never removes or
changes the GOA account. Calendars and contacts are not published to Evolution Data Server or
GNOME Shell by this feature.

No GNOME client library is required to build or start MailCraft. Discovery and credential
resolution run on background threads using the existing Rust D-Bus dependency, with method
timeouts and cancellation checks. Bounded D-Bus introspection enumerates object/interface names;
only Account and Mail properties are read. Discovery never requests OAuth registration properties
or credentials. Only selected
linked accounts query GOA during manual and scheduled operations. Both Flatpak manifests allow
communication with `org.gnome.OnlineAccounts`. Other platforms and hosts without the adapter
cannot sync GOA links and never substitute a local password or MailCraft login.

## Verification

Synthetic metadata covers disabled mail, unknown providers, malformed properties, invalid
addresses, duplicates, bounded account lists, secure mail capabilities and refusal of OAuth profiles.
UI tests cover explicit selection, independent setup, persisted references, initial sync,
scheduled-run eligibility and dropping discovery on mailbox replacement. The dialog is checked
in light and dark, including selection and disabled states, using synthetic accounts.

Run D-Bus integration tests only on a private session without a real GOA daemon:

```sh
dbus-run-session -- env MAILCRAFT_TEST_PRIVATE_BUS=1 cargo test --locked -p mailcraft --bin mailcraft private_bus_ -- --ignored --test-threads=1
```

The tests use invented identities and a synthetic GOA service. They cover discovery without
credential reads, independent incoming and outgoing passwords, and refusal when the link changes
or GOA disables mail. Live provider and packaged Flatpak acceptance remain separate checks.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-12 | minor | #989: optional discovery and separate account setup from GNOME |
