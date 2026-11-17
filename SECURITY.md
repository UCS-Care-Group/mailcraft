# Security policy

People trust MailCraft with years of correspondence and with the sign-ins to their mail, so a
security problem is treated before feature work.

## Reporting a vulnerability

**Please don't open a public issue.** Report it privately through GitHub:
[Report a vulnerability](https://github.com/UCS-Care-Group/mailcraft/security/advisories/new)
(the repository's Security tab, "Report a vulnerability").

Say what you found, the version (File › About, or Copy Diagnostics), the platform, and how to
reproduce it. Use synthetic mail and `example.org` addresses; never send us real mail, passwords
or tokens.

We aim to acknowledge a report within 3 working days, to agree a fix and a date for disclosure
with you, and to credit you in the advisory unless you'd rather not be named.

## Supported versions

Fixes go into the latest release only. Please check that a problem still happens there.

## What counts

Anything that breaks what MailCraft promises (see `PRIVACY.md` and the rules in `AGENTS.md`), for
example:

- something in a message that runs, fetches from the network by itself, or reaches outside its
  display (HTML, CSS, a link, a picture, an attachment, a header);
- a crash, hang or runaway memory use caused by a message, a server's answer or a mailbox file;
- a password, refresh token or access token written to a file, a log or an error, or sent anywhere
  but the provider it belongs to;
- a connection that carries mail or a sign-in without TLS, or a certificate that isn't checked;
- header injection in mail that MailCraft writes;
- a picture download that reaches a private, loopback or link-local address, or sends cookies,
  a referrer or credentials;
- the control port or the MCP server accepting a command without its key.

## Known limits (not vulnerabilities)

- S/MIME and OpenPGP signatures are checked where a message is read, but a certificate's revocation is not; signing and encrypting the mail you send, and opening encrypted mail, are not built yet ([#30](https://github.com/UCS-Care-Group/mailcraft/issues/30)).
- The mailbox file on disk isn't encrypted by MailCraft; it relies on the operating system's disk
  encryption (BitLocker, FileVault, LUKS).
- Anyone who can run programs as you on your computer can read your mailbox; MailCraft doesn't
  protect against that.
