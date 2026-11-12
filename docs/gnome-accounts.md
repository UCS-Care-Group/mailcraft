# Importing accounts from GNOME

On Linux, **File › Info › Import accounts from GNOME…** reads the mail accounts enabled in
GNOME Online Accounts (GOA). Choose an account and press Continue, then complete MailCraft's
account setup. The ordinary Add Account action is unchanged.

Google, Microsoft 365 (`ms_graph`) and older Microsoft Personal (`windows_live`) accounts are
supported. Other providers are shown but cannot be imported yet. An address already in the
current MailCraft mailbox cannot be imported again. Accounts without an enabled mail service
are left out.

This imports an identity and suggests its provider. MailCraft signs in through its own provider
registration; it does not read or copy GOA passwords, access tokens or refresh tokens. A Google
build without the project's verified registration retains the setup wizard's existing sign-in
limitations and explicit app-password option.

The account belongs to MailCraft after setup. Adding, changing or removing an account in GNOME
does not change accounts in MailCraft. Importing again is an explicit action. Calendars and
contacts are not published to Evolution Data Server or GNOME Shell by this feature.

No GNOME client library is required to build or start MailCraft. The discovery request uses
the existing Rust D-Bus dependency, on a background thread, only when the import dialog opens.
Without GOA, or if access is refused, the dialog explains that Add Account remains available.
The action is absent on other platforms. Both Flatpak manifests allow communication with
`org.gnome.OnlineAccounts` for this optional action.

## Verification

Synthetic ObjectManager replies cover disabled mail, unknown providers, malformed properties,
invalid addresses, duplicate entries and bounded account lists. UI tests cover explicit
discovery, cancellation, duplicate accounts, selection without sign-in, and dropping a pending
discovery when the mailbox changes. No test needs real accounts.

The optional D-Bus integration test must run on a private session, with no real GOA daemon:

```sh
dbus-run-session -- env MAILCRAFT_TEST_PRIVATE_BUS=1 cargo test --locked -p mailcraft --bin mailcraft private_bus_discovery -- --ignored
```

It registers a synthetic metadata-only ObjectManager under GOA's name and exercises the actual
client. Its only account is `owner@example.org`.
