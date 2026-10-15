# MailCraft privacy statement

MailCraft is a mail, calendar, contacts and tasks program that runs on your own computer. The
project runs no service of its own: there is no MailCraft account, no MailCraft server, and
nothing in the program reports to the project or to anybody who works on it.

## What the project collects

Nothing. MailCraft has no telemetry, no analytics, no crash reporting and no advertising, and it
does not check for updates by itself.

## Where your data is

- **Your mailbox** (messages, calendar, contacts, tasks, settings) is kept in files on your
  computer, in MailCraft's settings folder, and on the servers of the mail provider you connect
  to, as with any mail program.
- **Passwords and sign-in tokens** are kept in memory while the program runs. When you let the
  program remember a sign-in, the password or the refresh token goes to your operating system's
  credential store (Windows Credential Manager, the macOS keychain, and on Linux and BSD the
  Secret Service: GNOME Keyring, KWallet or KeePassXC) and nowhere else: not into the mailbox
  file, the settings, a log or a command line. Where no credential store is running, nothing is
  remembered: you sign in each time the program starts, and File › Options says so.
- **Private keys for signed and encrypted mail** (an S/MIME digital ID, an OpenPGP key) that you
  import are kept the same way: in the operating system's credential store when sign-ins are
  remembered, otherwise only while the program runs. A passphrase is used once, to open the
  file, and kept nowhere. Certificates and public keys (yours, and those of people who sent you
  signed mail, unless you turn that off) are kept in the mailbox. An encrypted message stays
  encrypted on disk; what it says is worked out while it is on screen. Signatures are checked on
  your computer: no certificate authority, revocation list or key server is asked about them.
- **Notifications** for new mail and reminders are shown by your operating system while
  MailCraft's window is not in front. One for new mail says how many messages came, and names
  the newest one's sender and subject only if you turn that on in File › Options; a reminder
  says its subject and time. None holds a message's text. You can turn them off there.
- **Pictures you asked to download** are kept on your computer for up to 30 days so they are not
  asked for again; File › Options shows how many and clears them.

## What the program connects to, and when

MailCraft connects only to these, over encrypted connections:

| What | When | What is sent |
|---|---|---|
| Your mail provider's servers (IMAP, SMTP) | Send/Receive, and on the timer you set | What a mail program sends: your sign-in and your mail |
| Microsoft's sign-in pages and Microsoft Graph, for an account that signs in with Microsoft | When you sign in; for calendar, contacts, sending mail, working hours, categories and Outlook's Favorites; and for all of your mail, when you choose to have it read through Microsoft Graph | Your sign-in happens on Microsoft's page in your browser; the program receives tokens, never your password |
| Exchange Web Services, for a work or school account | Only if you choose it for Outlook's Favorites | Read requests for the list of favourite folders |
| A public DNS-over-HTTPS resolver (Cloudflare, then Google) | Once, when you press Connect while adding an account at a domain the program does not know | The domain of your address (the part after the @), never the mailbox name |
| The web address of a picture in a message | Only when you ask for a message's pictures, or for a sender or domain you chose to always download from | The address alone: no cookies, no referrer, no sign-in |

Nothing in a message runs, and nothing in a message is fetched unless you ask for its pictures.

## Permissions asked of a Microsoft account

When you sign in with Microsoft, its page lists what MailCraft asks for. Each is used on your
computer, for you, and for nothing else:

- reading and sending your mail (IMAP and SMTP access, and sending through Microsoft Graph);
- reading and changing your calendar, and reading your contacts;
- reading your mailbox settings (working hours and categories);
- reading the list of folders you keep under Favorites in Outlook, if you ask for it;
- reading and changing the calendars of shared mailboxes you open, when you add one or allow it
  under the shared mailbox in File › Info;
- reading, changing and filing your mail through Microsoft Graph instead of IMAP, if you switch
  that on in the account's Account Settings; and then the same, and sending, for the shared
  mailboxes you open under that account, which are read the way the account is;
- keeping you signed in (a refresh token, stored as described above).

You can take these back at any time in your Microsoft account's or your organisation's list of
approved applications; MailCraft then asks again the next time it needs them.

## Scripts and agents

MailCraft can be driven by scripts and AI agents through a control channel and an MCP server.
Both are off unless you start the program with them, listen on your own computer only, and take
requests only with a key the program keeps in a file in your settings folder.

## Changes and contact

This statement lives in the project's repository, and its history shows every change:
<https://github.com/billsecond/mailcraft/blob/main/PRIVACY.md>. Questions and reports:
<https://github.com/billsecond/mailcraft/issues>.
