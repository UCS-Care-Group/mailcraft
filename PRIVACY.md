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
  says why. MailCraft notes the names (never the contents) of the entries it puts there in its
  settings, so that Remove MailCraft's data, and turning remembering off, can remove every one,
  also those of mailboxes you opened earlier, and list any that could not be removed.
- **Pictures you asked to download**, and senders' pictures when sender images are on, are kept
  on your computer for up to 30 days so they are not asked for again; File › Options shows how
  many and clears them.
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
| A JMAP server you add an account for (the session address you gave it, and the addresses at the same server that its session names) | Send/Receive, on the timer you set, and when you open a message that isn't downloaded yet; only in a build that connects to JMAP servers (the browser build, from the version that adds it) | What a mail program sends: your password or access token, and your mail. A password or token you type is kept in memory only, until the program closes, the server turns it down or the account's server changes; it is never remembered in the credential store, the mailbox file, the settings or the browser's storage. Where the page you use signed you in itself, the program asks it for the credential at each Send/Receive or download (once more if the server turns it down), and keeps none of it beyond that |
| Microsoft's sign-in pages and Microsoft Graph, for an account that signs in with Microsoft | When you sign in; for calendar, contacts, tasks (Microsoft To Do), sending mail, working hours, categories and Outlook's Favorites | Your sign-in happens on Microsoft's page in your browser; the program receives tokens, never your password |
| A public DNS-over-HTTPS resolver (Cloudflare, then Google) | Once, when you press Connect while adding an account at a domain the program does not know | The domain of your address (the part after the @), never the mailbox name |
| The web address of a picture in a message | Only when you ask for a message's pictures, or for a sender or domain you chose to always download from (and then only for mail that your own mail server, not the sender, says really came from that domain) | The address alone: no cookies, no referrer, no sign-in |
| Gravatar (`gravatar.com`), then favicon.im, then Google's favicon service (`www.google.com/s2/favicons`), for sender images | Off unless you turn on File › Options › Show sender images: then while a sender's mail is in the part of the message list on screen, only for the people you know (your contacts, and whoever you have written to) and the mail whose pictures you download, never for mail in Junk E-mail, and a sender's picture only for mail your mail server says really came from them (its `dmarc=pass`, or, when it gives no DMARC verdict, its `dkim=pass` for their domain's signature); each in turn until one has a picture, and nothing more once you turn it off (a request already on its way finishes, and what it brings is not kept) | Gravatar: a hash (SHA-256) of the sender's address. favicon.im and Google: the domain of the sender's website (`nutrient.io` for mail from `mail.nutrient.io`), never a mail provider's such as `gmail.com` or a hosting platform's such as `onmicrosoft.com`; Google's service passes the request on to its `gstatic.com`, which is told the same domain. Nothing about you: no cookies, no referrer, no sign-in. It does tell those services whose pictures you look at |
| The certificate authority that issued a server's certificate, on Windows and macOS | When a server MailCraft connects to leaves out the intermediate certificate its own was issued by: the system fetches it from the address the certificate names, as it does for other programs (and may ask whether the certificate was revoked) | Nothing of yours: the authority learns only which of its certificates a server on the way uses |
| GitHub's API (`api.github.com`), to see whether a new version is out | Off unless you turn it on (the program asks once, and File › Options changes it): then at most once a day; and whenever you press Check for Updates in File › About | One request for the newest release of MailCraft's repository (`GET /repos/UCS-Care-Group/mailcraft/releases/latest`), with nothing about you, your accounts, your mail or this computer in it, not even the version you run |
| GitHub's release downloads (`github.com`, and `objects.githubusercontent.com` or `release-assets.githubusercontent.com`, where GitHub sends those), to download a new version | Only for a copy installed from the Windows `.msi`, the macOS app or a Linux AppImage, while the daily check is on and File › Options › Install updates automatically is on (as it is unless you, or your organisation, turn it off); when a check finds a newer release | Requests for three files of that release: its list of checksums, the signature over it, and the package for your kind of computer. Nothing about you, your accounts, your mail or the version you run. The package is kept in the program's own folder, installed only when MailCraft's release signature says it is the project's own (when you press Restart to update, or when you quit), and deleted otherwise. Installing it connects to nothing |

Nothing in a message runs, and nothing in a message is fetched unless you ask for its pictures. A message's own style sheet is read for how the message looks (colours, sizes, spacing, what the sender hid), but nothing in it is ever fetched: a sheet's `@import`, its fonts and its pictures are ignored, and a link it styles is never shown as visited. Its `@media` rules are tested against two things on your own computer and nothing else: how wide the message is laid out in the reading pane, and whether your window is dark; the program sends neither to anyone. One consequence to know: once you allow pictures, a message can show a different picture in a dark window, or at each of several widths, and the picture for the look on screen is fetched, so its server can tell from which address was asked for that your window is dark and how wide the message is laid out, to within a few pixels (it is told nothing else, and nothing is fetched until you allow pictures). What a sender hid in a message (a line of preview text, the other copy of a desktop and mobile pair) is not shown to you and is not quoted in your replies, but it is kept with the message on your computer, so that your search and your rules can find it, and it is what an agent you have allowed to read your mail is given as the message's text. The one other thing fetched for your mail is a sender's picture, and only once you turn sender images on.

## Permissions asked of a Microsoft account

When you sign in with Microsoft, its page lists what MailCraft asks for. Each is used on your
computer, for you, and for nothing else:

- reading and sending your mail (IMAP and SMTP access, and sending through Microsoft Graph);
- reading and changing your calendar, and reading your contacts;
- reading your mailbox settings (working hours and categories);
- reading and changing your To Do lists and tasks (`Tasks.ReadWrite`), so that your tasks are kept in
  step with Microsoft To Do: your tasks' titles, notes, dates, importance, categories and whether they are
  done go to your own Microsoft account and come back from it, and nowhere else. It is never asked for
  on a sign-in page, so signing in for mail never waits on it: it is asked for by itself, for a new account
  and an old one alike, the first time its tasks would be kept in step, after the program has asked you,
  and File › Info shows whether it was given. Until it is, tasks stay on this computer and nothing of them is sent. Even then, tasks that exist only on this computer are not sent on their own: the program asks you once ("Send your tasks to your account's To Do?", with the account to choose when you have several, and Not now) and keeps them here until you say yes; File › Info offers it again. After a yes, the tasks you make from then on are kept in step too;
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
