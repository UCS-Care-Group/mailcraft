# Gmail and Google Workspace on the desktop

> **Status:** desktop implementation; official MailCraft registration and public Google verification remain release requirements · **Written:** 2026-11-03 · **Last updated:** 2026-11-04 · **Change:** major

MailCraft offers **Sign in with Google** first for new Google accounts when its Desktop OAuth
registration is configured. **Use an app password instead** is a separate alternative. An
existing account keeps its current sign-in method until you explicitly change it.

This guide describes the desktop implementation in this contribution, not a promise that an
older downloaded release includes it. It covers Gmail and Google Workspace mail over IMAP and
SMTP. It does not add Google Calendar or Contacts, nor Gmail access in the browser build;
[browser Gmail #233](https://github.com/UCS-Care-Group/mailcraft/issues/233) is separate.

## Add a Google account

1. Open **Send / Receive → Add Account**, enter your email address and continue. For a custom
   Workspace domain that is not recognised, select **Google — Gmail and Google Workspace** as
   the provider. The wizard may still be headed **Gmail account** for a Workspace address.
2. Choose **Sign in with Google**. MailCraft opens your system browser. Choose the same primary Google
   address you entered and review Google's consent screen there. Your ordinary Google password
   belongs only on Google's page, never in MailCraft.
3. Return to MailCraft after Google finishes. Finish the wizard, then use Send/Receive to
   download mail. **File → Info** shows how the account signs in; **Account Settings** manages it.

You do not need to create an app password for this route. A normally configured distribution
also does not ask each user to create a Google Cloud project. A developer build without an
application registration explains that Google sign-in is unavailable and offers app-password
setup instead; see [Development registration](#development-registration).

MailCraft requests `https://mail.google.com/ openid email`: mail access for IMAP/SMTP and the
identity needed to check which account signed in. The mail scope is broad, **not read-only**;
the application can send and apply your mail changes. A read-only acceptance test does not
reduce the granted permission. Google's [XOAUTH2 specification][xoauth] describes the mail scope.

### Primary addresses and Workspace aliases

Use the primary email address of the Google account for MailCraft's account identity. Google
returns that address when you sign in. MailCraft compares it with the account address you
entered, ignoring letter case and surrounding spaces, and refuses a different address.

A Workspace alias is not supported as the OAuth account identity in this version, even if mail
sent to it reaches the same inbox. This applies to new accounts, conversion and reconnect.
MailCraft does not discover aliases, substitute Google's returned address, or silently attach
another account's sign-in to existing mail. If an existing app-password account is configured
with an alias, leave it on that method until you have deliberately configured the account with
its primary address; a refused conversion keeps the existing credential and mail intact.
This sign-in flow does not configure sending from aliases.

### App-password alternative

On the Google account page, select **Use an app password instead**, then enter an app password
created for this mail client in your Google Account. Do not enter your normal Google password,
a Google Cloud client ID, or a Google Cloud client secret in the account password field. A
missing-registration build presents this fallback directly, with an explanation.

Google recommends browser sign-in and requires 2-Step Verification for app passwords. Your
account or organisation may not offer them. Check Google's [app-password help][app-passwords]
rather than weakening account protection or trying to bypass an administrator's policy.
**Use Google sign-in instead** returns to the preferred route when it is configured; switching
routes clears an entered password.

## Keep an existing account, or convert it explicitly

Opening or upgrading MailCraft does not convert an app-password account. Its servers,
credentials and downloaded mail remain as they are.

To convert an existing Gmail or Workspace account, open its **Account Settings**, save or
discard any edits, then select **Switch to Google sign-in**. The account must already use
Google's standard IMAP/SMTP endpoints and an email-matching user name; the window explains
incompatible settings. Complete browser consent for that same address.

Only successful matching-account authorization changes the sign-in method. Canceling, denying
consent, selecting a different address, or failing the conversion leaves the previous account
and its credential intact. A successful conversion retains the account, messages and folders;
it does not require deleting and re-adding the account.

For an account already using OAuth, **Reconnect with Google** renews authorization without
replacing downloaded mail. Save or discard unsaved account-settings edits before reconnecting.
When Google rejects a refresh grant as expired or revoked, MailCraft asks for a new sign-in;
it does not silently switch to an app password or clear another account's credential.

## Remembering sign-ins

**File → Options → Accounts → Remember passwords and sign-ins on this computer** controls
whether account passwords and OAuth refresh credentials survive a restart. Remembered account
credentials use the operating system's credential store; access tokens stay in memory. Without
remembering, sign in again after restarting. A locked or unavailable system store is reported,
not replaced by a plaintext mailbox credential file.

The application registration discussed below is different from your account credential. Its
Desktop registration value comes from the build or process environment; MailCraft does not
provide a client-secret preference or write that value to `ui.json`.

## Google restrictions and useful error distinctions

A development registration and an organisation's access policy are separate checks. An HTTP
403 or “Access blocked” page alone does not identify which check failed; read the reason Google
shows. Some blocked pages never return to MailCraft, so cancel the waiting dialog and retry
only after the relevant restriction is addressed.

| What Google or MailCraft says | What to check |
|---|---|
| The app is in testing; your account is not an approved test user | Ask the registration owner to add the exact address to the project's Test users list. |
| Your organisation blocks this app, or `admin_policy_enforced` | Ask your Google Workspace administrator to review the client and requested data access. Microsoft's administrator-consent page is not applicable. |
| `org_internal` | The registration is restricted to its own organisation; an external account is outside that audience. |
| `invalid_client` or `redirect_uri_mismatch` | The developer should check the matching Desktop client ID/secret and client type, not your mail-account password. |
| A different account was selected, including the primary address of an account entered as an alias | Use the primary Google address as the MailCraft account identity and select that same address in the browser. Workspace aliases are not accepted as OAuth identities. |
| Browser did not open, was closed, or no callback arrived | Check that the system browser can open and return to the same computer; retry in MailCraft. Do not paste a browser authorization code into a password field. |
| A sign-in expires after several days in a test build | Check the registration's testing status before assuming token storage is broken. |

For an External project in **Testing**, Google limits access to its listed test users (up to
100) and gives these Gmail-scope refresh grants a seven-day lifetime. The identity-only scope
exception does not apply here. See [Google's audience settings][audience] and [refresh-token
expiration][expiry]. These are development-registration limits, not a general seven-day
expiry promise for production MailCraft sign-ins.

Workspace administrators can separately restrict third-party apps and high-risk Gmail access,
including for listed test users. See [Workspace app access control][workspace]. Verification of
a public app does not override an organisation's access rules. An administrator can also
turn IMAP off or restrict it to specific OAuth clients, independently of browser consent;
see [Workspace IMAP controls][workspace-imap].

### Google sign-in works, but mail does not arrive

Browser authorization and mail synchronization use different connections:

| Purpose | Host and port | Protection |
|---|---|---|
| Google authorization and token exchange | `accounts.google.com:443`, `oauth2.googleapis.com:443` | HTTPS |
| Incoming mail | `imap.gmail.com:993` | TLS |
| Outgoing mail, default | `smtp.gmail.com:465` | TLS |
| Outgoing mail, alternative | `smtp.gmail.com:587` | STARTTLS |

Google documents these mail endpoints in [IMAP and SMTP access][mail-ports]. A browser page or
ping succeeding does not test the mail ports. A TCP timeout before TLS is not fixed by changing
an OAuth client secret. Check the account's server settings and the relevant endpoint,
virtual-machine, gateway and upstream network policy with the administrator; do not disable
firewalls or TLS verification. MailCraft does not normally need administrator privileges.

“Up to date” applies to the folders and download limits configured in MailCraft, not a promise
that every message in the Google account has been downloaded. OAuth is a sign-in change, not a
change to those synchronization limits.

## Development registration

This section is for maintainers and contributors building MailCraft, not the normal account
setup route. Use a registration owned by the MailCraft project for a distribution, or a
separate registration you own specifically for MailCraft development. Never borrow another
mail program's client identity.

Create a **Desktop app** OAuth client in Google Auth Platform for the chosen project. Configure
its audience and test users for the intended test accounts and declare the scopes MailCraft
actually requests. Follow Google's [desktop authorization guide][desktop] and [client
management help][clients]. Save the newly issued client secret securely when it is shown;
Google does not promise that a closed creation dialog can show it again. Do not rotate or
replace a shared client merely to recover a lost local copy without its owner's agreement.

The implementation uses authorization code with PKCE, state validation and an ephemeral IPv4
loopback callback at `http://127.0.0.1:<port>`. It is not a Web application client, a service
account, or Workspace domain-wide delegation. No fixed public callback server is required.

### Which value goes where

| Value | Purpose and destination |
|---|---|
| Google Desktop client ID | Identifies the application. Supply it at build time or in `MAILCRAFT_GOOGLE_CLIENT_ID`. **File → Options → Accounts → Google Desktop application ID** can override the public ID, but must match the registration paired with the host's configured secret. |
| Matching Google Desktop client secret | Supply the same Desktop registration's value at build time or in `MAILCRAFT_GOOGLE_CLIENT_SECRET`. There is no client-secret field in File → Options. It is not an account password. |
| Google account password | Enter only on Google's browser sign-in page. |
| Google app password | Enter only after choosing the explicit app-password account route. It is not an OAuth registration value. |

Desktop clients cannot keep a distributed application secret confidential; PKCE protects the
code exchange. This implementation still requires Google's matching Desktop registration
value when exchanging and refreshing tokens. It is supplied by the host, and is not saved as
an application preference. Keep development registration values and credential downloads out
of commits, screenshots, diagnostics and support reports.

An earlier development build offered a client-secret field in File → Options. A value stored
there as `googleClientSecret` in `ui.json` is now ignored and omitted the next time preferences
are saved normally; startup alone does not rewrite the file. If your test setup relied on that
field, configure the matching ID and secret through the build or process environment before
signing in or renewing a grant. This change does not replace any account's identity or stored
refresh credential.

### Overrides and precedence

Both registration values can be supplied when building or launching the desktop application:

| Setting | Source |
|---|---|
| `MAILCRAFT_GOOGLE_CLIENT_ID` | Build-time default or process-environment override for the Desktop client ID. |
| `MAILCRAFT_GOOGLE_CLIENT_SECRET` | Build-time default or process-environment override for its matching registration value. |
| Google Desktop application ID in File → Options | A nonempty local override of the public client ID; it cannot supply or change the host's registration secret. |

The host takes a present process-environment value before its compiled default. Unset an
unwanted environment variable instead of leaving it empty. A nonempty File → Options client-ID
override wins over the host's default ID. An existing account with its own explicit application
ID keeps that ID; changing the global default does not rewrite it. The host's registration
secret can be used only with its matching client ID, including when an account has an explicit
ID or File → Options overrides the default. Changing the ID alone does not pair the secret with
a different registration. Clearing the client-ID option returns to the host default; it does
not erase an account's explicit ID.

Supply a matching pair using your local development environment or a private launcher; this
guide deliberately includes no real values or secret-bearing command lines. A build without
a usable pair keeps the explained app-password fallback. `--sample` and
`MAILCRAFT_NO_PREFS=1` are temporary testing modes, not a way to prove saved preferences across
restarts. To reopen a chosen saved mailbox directly, use the supported
`--mailbox <path>` argument rather than importing it as an unrelated positional file.

## What has been verified, and what has not

Manual Windows desktop acceptance with developer-owned registrations covered Gmail OAuth and
Workspace app-password sign-in, an unchanged password-account restart, canceled and successful
explicit conversion, preservation of downloaded mail, OAuth reconnect/restart, actual refresh
exchanges, and incoming synchronization. SMTP OAuth authentication was checked on both ports
and ended with QUIT, **without sending a message**. Native clipboard paste into account fields
was also verified. This is not evidence of an outbound message-delivery test or complete
interoperability on every operating system and organisation policy.

Automated regression tests use synthetic accounts and local stand-ins, not live credentials.
Denial, wrong-account selection, revoked grants and administrator-policy errors have synthetic
coverage; this does not imply that every negative policy was forced on a real user account.
The desktop flow and its documentation do not implement browser Gmail, Google Calendar or
Google Contacts.

Code review can proceed with a development registration. Public rollout still needs the
maintainer-owned registration and the applicable [Google restricted-scope verification][verification]
work. Development/testing has different verification requirements; a successful test-user
sign-in is not public-app approval. See [release readiness](releasing.md#google-desktop-oauth-release-readiness).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-04 | minor | #232: document primary-address identity checks and unsupported Workspace aliases; keep the public client-ID option while taking the matching registration secret only from build or environment configuration, including migration from the former preference |
| 2026-11-03 | major | Desktop Google sign-in, app-password alternative, explicit conversion, registration overrides, restrictions and verified acceptance scope (#232) |

[desktop]: https://developers.google.com/identity/protocols/oauth2/native-app
[clients]: https://support.google.com/cloud/answer/15549257
[audience]: https://support.google.com/cloud/answer/15549945
[expiry]: https://developers.google.com/identity/protocols/oauth2#expiration
[app-passwords]: https://support.google.com/mail/answer/185833
[workspace]: https://knowledge.workspace.google.com/admin/apps/control-which-apps-access-google-workspace-data
[workspace-imap]: https://knowledge.workspace.google.com/admin/sync/turn-pop-and-imap-on-or-off-for-users
[mail-ports]: https://developers.google.com/workspace/gmail/imap/imap-smtp
[xoauth]: https://developers.google.com/workspace/gmail/imap/xoauth2-protocol
[verification]: https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification
