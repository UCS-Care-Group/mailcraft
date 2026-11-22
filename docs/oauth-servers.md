# Signing in with your own identity provider

> **Last reviewed:** 2026-11-22 · **Last updated:** 2026-11-22 · **Change:** major

An IMAP account on a mail server of your own (Dovecot for IMAP, Postfix for sending, say) can
sign in on your own identity provider's page instead of with a password, the way Microsoft and
Google accounts do: Keycloak, Authentik, Zitadel, Kanidm, Authelia or any other OpenID Connect or
OAuth 2.0 server. MailCraft opens the provider's sign-in page in your browser, the provider hands
back a token, and the mail servers take the token.

Every name and address on this page is made up (`example.org`). Nothing here was tried against a
real mailbox.

## What MailCraft needs

- **The provider's addresses.** Its own address (the *issuer*, such as
  `https://id.example.org/realms/mail`), from which MailCraft finds its sign-in and token
  addresses (OpenID Connect Discovery, or RFC 8414) when you press **Discover**; or the two
  addresses typed by hand when it describes itself nowhere. Only the issuer's address is sent when
  you press Discover: not your mail address, not the client id.
- **A client registered for MailCraft** at the provider, which you (or your administrator) set up
  once:
  - a *public* client (no secret), with PKCE required, method `S256`;
  - the redirect address `http://127.0.0.1` with any port (RFC 8252 §7.3). A provider that can't
    take "any port" gets a fixed one, such as `http://127.0.0.1:53682/`, and the same port in
    MailCraft's **Redirect port**;
  - the standard flow (authorization code) on; nothing else is needed.

  A provider that insists on a confidential client gives it a secret: enter it in **Client secret**.
  MailCraft keeps it in your system's credential store, never in the mailbox file, and sends it to
  the provider's token and revocation addresses alone. A desktop program can't really keep a secret
  from the person using it (RFC 8252 §8.5), so a public client is the better choice where the
  provider allows one.

  **Client secret sent** says how: *In the Authorization header* (`client_secret_basic`), the way
  every provider must take (RFC 6749 §2.3.1) and the one MailCraft uses unless told otherwise, or
  *In the form* (`client_secret_post`). **Discover** picks the form only when the provider's
  description lists it and not the header; one that lists nothing takes the header (RFC 8414).
- **Mail servers that take the token** with SASL OAUTHBEARER (RFC 7628) or XOAUTH2. MailCraft
  uses OAUTHBEARER when a server offers it and XOAUTH2 otherwise, or the one you choose in
  **Mechanism**. A server that offers neither is never shown the token, and MailCraft says so.

MailCraft asks the provider for `openid email offline_access`, and for any **Extra scopes** you
add (`mail`, say, if your mail servers look for one). A provider whose description lists the
scopes it has, without some of these three, isn't asked for those it doesn't list: a server may
refuse a scope it doesn't know (RFC 6749 §3.3). `offline_access` brings a refresh token, so
that you aren't asked to sign in again each time a token runs out; a provider that won't give one
is said to when you press Discover. When a mail server turns a token down in the middle of a
Send/Receive (it ran out early, or the provider withdrew it), MailCraft renews it with the refresh
token and goes on with the same Send/Receive, once: a second refusal waits for the next one.

When the provider's page sends you back to MailCraft it may say which provider it is (`iss`,
RFC 9207; Keycloak does). MailCraft then checks it is the issuer in the settings, and uses nothing
of an answer that names another. When the provider's description says it always names itself,
Discover notes that, and an answer that doesn't is refused too.

## Setting the account up

1. **File › Add Account**, type the address, **Connect**.
2. On the page with the servers, check the incoming and outgoing servers, then choose
   **Sign in with your identity provider (OAuth 2.0) instead**. (When your domain's autoconfig
   document says its mail signs in with OAuth 2.0 and names a provider, MailCraft offers that
   provider's address: **Use it**.)
3. Enter the provider's address and press **Discover**, or enter the sign-in and token
   addresses. An address outside the provider's own domain is named, and is used only once you
   tick **I trust these addresses**.
4. Enter the **Client ID** (and the client secret, if the provider gave one), then **Connect**:
   your browser opens the provider's page, you sign in there, and the account is added.

**File › Info › Account Settings** shows every value the sign-in uses and lets you change them.
For a Microsoft or Google account it shows their built-in values; only the application ID (and
Microsoft's tenant) can change there, and **Use your own identity provider instead…** turns the
account into one with its own provider, after a question, starting from the values it had.

The sign-in is remembered (when File › Options says to remember sign-ins) under a name that holds
the provider, its token and revocation addresses, the client id and both mail servers: changing any of them, from
here, a command or a hand-edited file, asks you to sign in again rather than sending what was
remembered anywhere new. The token goes only to the account's own incoming and outgoing servers,
over TLS, and never to Microsoft's or Google's. Removing the account gives the provider its refresh
token back (RFC 7009) when it names a revocation address, and forgets the sign-in and the client
secret here.

## Keycloak

In the realm your mail users live in:

1. **Clients › Create client**: client type *OpenID Connect*, client ID `mailcraft`.
2. **Capability config**: *Client authentication* off (a public client); *Standard flow* on; the
   other flows off.
3. **Login settings**: *Valid redirect URIs* `http://127.0.0.1/*`.
4. **Advanced › Proof Key for Code Exchange Code Challenge Method**: `S256`.

The issuer is `https://id.example.org/realms/<realm>`. The tokens carry the user's `email`; your
mail servers decide which claim names the mailbox (see Dovecot below).

## Authentik

1. **Applications › Providers › Create**: *OAuth2/OpenID Provider*, client type *Public*.
2. *Redirect URIs*: `http://127.0.0.1` matched as a regular expression,
   `^http://127\.0\.0\.1:[0-9]+/$`, or a fixed port.
3. Create an application that uses the provider, with the slug you like (`mail`).

The issuer is `https://auth.example.org/application/o/<slug>/` (with the slash at the end: enter
it exactly as the provider names it; MailCraft refuses a description that names another).

## Dovecot

Dovecot checks the token itself (its `oauth2` password database), either by validating the token's
signature with the provider's keys or by asking the provider (introspection). The mechanisms to
offer are named in `auth_mechanisms`, beside the ones you keep for passwords:

```
auth_mechanisms = plain oauthbearer xoauth2
```

and the `oauth2` database names your provider and the claim that holds the user name (such as
`email` or `preferred_username`). Dovecot's own documentation (OAuth2 authentication) has the
settings for your version; the user name MailCraft sends is the account's **User name** in Server
settings (its address when that's empty), which needn't be what the provider calls you.

## Postfix

Postfix hands SMTP sign-ins to Dovecot (`smtpd_sasl_type = dovecot`, with `smtpd_sasl_path` the
socket Dovecot listens on for it). With Dovecot offering OAUTHBEARER and XOAUTH2, Postfix offers
them on the submission port too: nothing else to set.

## When it doesn't work

- **"the identity provider's description names another issuer"**: the address entered isn't the
  issuer, character for character (a slash at the end, `http` for `https`, another realm).
- **"says it doesn't take the protected sign-in (PKCE with S256)"**: turn S256 on for the client.
- **"your identity provider doesn't know this client id, or its client secret"**: check the client
  ID, and the secret for a confidential client. A provider that won't take the secret in the
  header wants **Client secret sent** set to *In the form* (Discover sets it when the provider
  says so).
- **"the sign-in came back from another identity provider than the one asked"**: the page that
  sent you back named an issuer other than the one in the settings. Check the issuer address, and
  what opened the page.
- **The browser says the redirect address isn't allowed**: register `http://127.0.0.1` with any
  port, or set the same fixed port in both places.
- **"the server offers no way to sign in with an access token"**: the mail server offers neither
  OAUTHBEARER nor XOAUTH2 (Dovecot's `auth_mechanisms`).
- **"your identity provider no longer takes this sign-in: sign in again"**: the refresh token
  expired or was revoked at the provider. Send/Receive asks you to sign in again; your mail stays.

## Revision history

| Date | Change | Summary |
|---|---|---|
