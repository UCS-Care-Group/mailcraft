# MailCraft privacy statement

MailCraft is a mail, calendar, contacts and tasks program that runs on your own computer. The
project runs no service of its own: there is no MailCraft account, no MailCraft server, and
nothing in the program reports to the project or to anybody who works on it.

## What the project collects

Nothing. MailCraft has no telemetry, no analytics, no crash reporting and no advertising. It
checks for new versions only if you turn that on (see below), and that check sends nothing about
you.

## Where your data is

- **Your mailbox** (messages, calendar, contacts, tasks, settings) is kept in files on your
  computer, in MailCraft's settings folder, and on the servers of the mail provider you connect
  to, as with any mail program.
- **Passwords and sign-in tokens** are kept in memory while the program runs. When you let the
  program remember a sign-in, the password or the refresh token goes to your operating system's
  credential store (Windows Credential Manager, the macOS keychain, or on Linux and FreeBSD the
  desktop's Secret Service, such as GNOME Keyring or KWallet) and nowhere else: not into the
  mailbox file, the settings, a log or a command line. Where no such store is running, or it
  stays locked, nothing is remembered: you sign in each time the program starts, and the program
  says why.
- **Pictures you asked to download** are kept on your computer for up to 30 days so they are not
  asked for again; File › Options shows how many and clears them.
- **A downloaded new version** (only for a copy that updates itself) is kept in the `updates`
  folder of MailCraft's settings folder until it is installed (when you press Restart to update,
  or when you quit), and is deleted when it doesn't pass MailCraft's release signature or when a
  newer one replaces it. Installing it runs only the operating system's own tools on it (Windows
  Installer, `hdiutil` and `codesign` on macOS) and sends nothing anywhere. The version it
  replaced is kept in `previous-version` (on Windows its installer, from the second update on, in
  `this-version` and `previous-version`), so that MailCraft can go back to it when the new one
  doesn't start; a short record of the last update (`update-record.json`, `update-starts.json`:
  version numbers and a count of starts) is kept beside them. Before a new version first writes
  your mailbox file in a newer format, it keeps the file as it was beside it
  (`mailbox.mailcraft.before-<version>`, one kept). An organisation can turn this
  off for every user of the computer: the registry value `DisableAutomaticUpdates` (a DWORD other
  than 0) under `HKEY_LOCAL_MACHINE\Software\Policies\MailCraft` on Windows, or a file named
  `no-automatic-updates` in `/Library/Application Support/MailCraft/` on macOS and in
  `/etc/mailcraft/` on Linux and FreeBSD.

## What the program connects to, and when

MailCraft connects only to these, over encrypted connections:

| What | When | What is sent |
|---|---|---|
| Your mail provider's servers (IMAP, SMTP) | Send/Receive, and on the timer you set | What a mail program sends: your sign-in and your mail |
| Microsoft's sign-in pages and Microsoft Graph, for an account that signs in with Microsoft | When you sign in; for calendar, contacts, sending mail, working hours, categories and Outlook's Favorites | Your sign-in happens on Microsoft's page in your browser; the program receives tokens, never your password |
| A public DNS-over-HTTPS resolver (Cloudflare, then Google) | Once, when you press Connect while adding an account at a domain the program does not know | The domain of your address (the part after the @), never the mailbox name |
| The web address of a picture in a message | Only when you ask for a message's pictures, or for a sender or domain you chose to always download from (and then only for mail that your own mail server, not the sender, says really came from that domain) | The address alone: no cookies, no referrer, no sign-in |
| GitHub's API (`api.github.com`), to see whether a new version is out | Off unless you turn it on (the program asks once, and File › Options changes it): then at most once a day; and whenever you press Check for Updates in File › About | One request for the newest release of MailCraft's repository (`GET /repos/UCS-Care-Group/mailcraft/releases/latest`), with nothing about you, your accounts, your mail or this computer in it, not even the version you run |
| GitHub's release downloads (`github.com`, and `objects.githubusercontent.com` or `release-assets.githubusercontent.com`, where GitHub sends those), to download a new version | Only for a copy installed from the Windows `.msi`, the macOS app or a Linux AppImage, while the daily check is on and File › Options › Install updates automatically is on (as it is unless you, or your organisation, turn it off); when a check finds a newer release | Requests for three files of that release: its list of checksums, the signature over it, and the package for your kind of computer. Nothing about you, your accounts, your mail or the version you run. The package is kept in the program's own folder, installed only when MailCraft's release signature says it is the project's own (when you press Restart to update, or when you quit), and deleted otherwise. Installing it connects to nothing |

Nothing in a message runs, and nothing in a message is fetched unless you ask for its pictures.

## Permissions asked of a Microsoft account

When you sign in with Microsoft, its page lists what MailCraft asks for. Each is used on your
computer, for you, and for nothing else:

- reading and sending your mail (IMAP and SMTP access, and sending through Microsoft Graph);
- reading and changing your calendar, and reading your contacts;
- reading your mailbox settings (working hours and categories);
- reading the list of folders you keep under Favorites in Outlook, if you ask for it;
- reading and changing the mail of shared mailboxes you have been given access to, if you open one;
- sending from a shared mailbox, asked for by itself the first time you send from one (an
  answer to a meeting or an invitation from a shared mailbox's calendar is sending from it);
- reading and changing the calendars of shared mailboxes you have been given access to, asked for by itself the
  first time you tick one in the calendar's folder list (nothing of a shared mailbox's calendar is
  read before that, and it comes from your own Microsoft servers with your own sign-in; what you
  may see of it is up to its owner and your organisation);
- the same permission is asked for by itself the first time you open somebody's calendar by their address
  (Open Shared Calendar), without the permissions that reading a shared mailbox's mail takes; the
  address you type goes only to your own Microsoft servers, the calendar is read with your own sign-in
  and kept on this computer only, and what you may see of it is up to its owner and your organisation;
- keeping you signed in (a refresh token, stored as described above).

An administrator who approves MailCraft for an organisation is asked, in advance, for all of
these, so that one approval covers what is asked later.

You can take these back at any time in your Microsoft account's or your organisation's list of
approved applications; MailCraft then asks again the next time it needs them.

## Scripts and agents

MailCraft can be driven by scripts and AI agents through a control channel and an MCP server.
Both are off unless you start the program with them, listen on your own computer only, and take
requests only with a key the program keeps in a file in your settings folder.

## Changes and contact

This statement lives in the project's repository, and its history shows every change:
<https://github.com/UCS-Care-Group/mailcraft/blob/main/PRIVACY.md>. Questions and reports:
<https://github.com/UCS-Care-Group/mailcraft/issues>.
