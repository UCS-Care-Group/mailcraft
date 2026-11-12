# MailCraft privacy statement

MailCraft is a mail, calendar, contacts and tasks program that runs on your own computer. There
is no MailCraft account and no MailCraft mail server. The project runs one service, the report
relay, which takes a problem report you send yourself (below). Nothing is sent to the project
unless you press Send in Report a Problem, and then only what that window showed you.

## What the project collects

Nothing by itself. MailCraft has no telemetry, no analytics, no crash reporting and no
advertising. It checks for new versions only if you turn that on (see below), and that check
sends nothing about you. The one thing the project receives is a problem report you chose to
send ([Problem reports](#problem-reports)).

## Where your data is

- **Your mailbox** (messages, calendar, contacts, tasks, settings) is kept in files on your
  computer, in MailCraft's settings folder, and on the servers of the mail provider you connect
  to, as with any mail program. Those files are protected as your other files are, by your
  operating system's user account (and its disk encryption, where you have that on). Mail travels
  between your computer and your provider only over encrypted connections.
- **Passwords and sign-in tokens** are kept in memory while the program runs. When you let the
  program remember a sign-in, the password or the refresh token goes to your operating system's
  credential store (Windows Credential Manager, the macOS keychain, or on Linux and FreeBSD the
  desktop's Secret Service, such as GNOME Keyring or KWallet) and nowhere else: not into the
  mailbox file, the settings, a log or a command line. Where no such store is running, or it
  stays locked, nothing is remembered: you sign in each time the program starts, and the program
  says why. MailCraft notes the names (never the contents) of the entries it puts there in its
  settings, so that Remove MailCraft's data, and turning remembering off, can remove every one,
  also those of mailboxes you opened earlier, and list any that could not be removed.
- **Digitally signed and encrypted messages** (S/MIME and OpenPGP) are also kept as they
  arrived, beside the mailbox file with the rest of its content (up to 64 MB a message and 1 GB in
  all), so that their signature can still be checked and a copy moved to another account or saved
  as a file carries the signed body as it came; that copy's header is written as any other
  message's is, so what the owner's own servers stamped on it stays behind. For an encrypted
  message these bytes are the encrypted message itself, as the server holds it. They go when the
  message goes. When they are missing, or were never kept, the message's own server is asked for
  it again at the next Send/Receive, as for any message downloaded; nothing else is sent.
- **Pictures you asked to download**, and senders' pictures when sender images are on, are kept
  on your computer for up to 30 days so they are not asked for again; File › Options shows how
  many and clears them.
- **Attachment previews** are off until you enable them in File › Options. Previewing passes a
  temporary copy of the selected attachment and rendering arguments to a local executable, which
  runs with your operating system permissions. For PDFs or images respectively, MailCraft uses
  the path in `MAILCRAFT_PDFCRAFT_CLI` or `MAILCRAFT_PHOTOCRAFT_CLI` if set. Otherwise it looks
  for the expected CLI name in the usual Windows or macOS installation location and then on
  `PATH`. It selects the file by path and name without verifying the program's publisher or
  identity. In Junk E-mail and its subfolders, choose Preview Attachment from the attachment menu.
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
| Microsoft's sign-in pages and Microsoft Graph, for an account that signs in with Microsoft | When you sign in; for calendar, contacts, tasks (Microsoft To Do), sending mail, working hours, categories, automatic replies, the rules kept on the server and Outlook's Favorites | Your sign-in happens on Microsoft's page in your browser; the program receives tokens, never your password |
| Google's sign-in page (`accounts.google.com/o/oauth2/v2/auth`) and token endpoint (`oauth2.googleapis.com/token`), for an account that signs in with Google | When you sign in, switch an existing account to Google sign-in or reconnect; the token endpoint is also used when a saved sign-in is renewed | The browser's sign-in request carries the application's client ID, the permissions requested, your account address as a login hint, a random state value, a PKCE challenge and a return address on your own computer. The token endpoint receives the authorization code and PKCE verifier, or the refresh token, together with the application's client ID and Desktop client-secret registration value. Google returns tokens and identifies the signed-in address in the identity token; MailCraft makes no separate user-info request. You enter your Google password on Google's page: MailCraft never sees it. |
| Gmail's API (`gmail.googleapis.com`), for an account that signs in with Google, to move mail between Gmail's inbox categories | Not in this version: moving mail between Gmail's categories is switched off until Google has reviewed MailCraft's use of the API. Once it is switched on, only at the Send/Receive after you move a message of the Inbox from one Gmail category to another; never for an account that signs in with an app password | Gmail's own id of each message you moved, the category it goes to (the labels to add and take off), and the account's sign-in token, which Gmail's API takes as it is; nothing else of the message or of you |
| A public DNS-over-HTTPS resolver (Cloudflare, then Google) | Once, when you press Connect while adding an account at a domain the program does not know | The domain of your address (the part after the @), never the mailbox name |
| The web address of a picture in a message | Only when you ask for a message's pictures, or for a sender or domain you chose to always download from (and then only for mail that your own mail server, not the sender, says really came from that domain) | The address alone: no cookies, no referrer, no sign-in |
| Gravatar (`gravatar.com`), then favicon.im, then Google's favicon service (`www.google.com/s2/favicons`), for sender images | Off unless you turn on File › Options › Show sender images: then while a sender's mail is in the part of the message list on screen, only for the people you know (your contacts, and whoever you have written to) and the mail whose pictures you download, never for mail in Junk E-mail, and a sender's picture only for mail your mail server says really came from them (its `dmarc=pass`, or, when it gives no DMARC verdict, its `dkim=pass` for their domain's signature; on Microsoft 365, only the verdict Exchange Online writes as it takes the mail in from the internet, and also its word that the mail came from inside your own organisation and your own domain; never for mail MailCraft itself put on a server, moving or copying it there from this computer or another account, which leaves what any server said of it behind); each in turn until one has a picture, and nothing more once you turn it off (a request already on its way finishes, and what it brings is not kept) | Gravatar: a hash (SHA-256) of the sender's address. favicon.im and Google: the domain of the sender's website (`nutrient.io` for mail from `mail.nutrient.io`), never a mail provider's such as `gmail.com` or a hosting platform's such as `onmicrosoft.com`; Google's service passes the request on to its `gstatic.com`, which is told the same domain. Nothing about you: no cookies, no referrer, no sign-in. It does tell those services whose pictures you look at |
| The certificate authority that issued a server's certificate, on Windows and macOS | When a server MailCraft connects to leaves out the intermediate certificate its own was issued by: the system fetches it from the address the certificate names, as it does for other programs (and may ask whether the certificate was revoked) | Nothing of yours: the authority learns only which of its certificates a server on the way uses |
| GitHub's API (`api.github.com`), to see whether a new version is out | Off unless you turn it on (the program asks once, and the switch in File › About changes it): then at most once a day; and whenever you press Check for Updates in File › About | One request for the newest release of MailCraft's repository (`GET /repos/UCS-Care-Group/mailcraft/releases/latest`), with nothing about you, your accounts, your mail or this computer in it, not even the version you run |
| GitHub's release downloads (`github.com`, and `objects.githubusercontent.com` or `release-assets.githubusercontent.com`, where GitHub sends those), to download a new version | Only for a copy installed from the Windows `.msi`, the macOS app or a Linux AppImage, while the daily check is on and File › About › Install updates automatically is on (as it is unless you, or your organisation, turn it off); when a check finds a newer release | Requests for three files of that release: its list of checksums, the signature over it, and the package for your kind of computer. Nothing about you, your accounts, your mail or the version you run. The package is kept in the program's own folder, installed only when MailCraft's release signature says it is the project's own (when you press Restart to update, or when you quit), and deleted otherwise. Installing it connects to nothing |
| The project's report relay (`mailcraft.work/api/report`), in the desktop app | Only when you press Send report (or Try again) in Help › Report a Problem; never by itself, never again by itself, and never for a script or an agent | One request, over HTTPS, with what the window showed you: the title, what you wrote, MailCraft's version and your platform, the GitHub username if you gave one, and the diagnostics if you left them ticked. No account, address, mailbox name, computer name or identifier, no cookie, no sign-in. The relay sees your IP address, as any web server does ([Problem reports](#problem-reports)) |

Nothing in a message runs, and nothing in a message is fetched unless you ask for its pictures. A message's own style sheet is read for how the message looks (colours, sizes, spacing, what the sender hid), but nothing in it is ever fetched: a sheet's `@import`, its fonts and its pictures are ignored, and a link it styles is never shown as visited. Its `@media` rules are tested against two things on your own computer and nothing else: how wide the message is laid out in the reading pane, and whether your window is dark; the program sends neither to anyone. One consequence to know: once you allow pictures, a message can show a different picture in a dark window, or at each of several widths, and the picture for the look on screen is fetched, so its server can tell from which address was asked for that your window is dark and how wide the message is laid out, to within a few pixels (it is told nothing else, and nothing is fetched until you allow pictures). What a sender hid in a message (a line of preview text, the other copy of a desktop and mobile pair) is not shown to you and is not quoted in your replies, but it is kept with the message on your computer, so that your search and your rules can find it, and it is what an agent you have allowed to read your mail is given as the message's text. The one other thing fetched for your mail is a sender's picture, and only once you turn sender images on.

Read receipts go one way only. A message you write with Request a Read Receipt asks its recipients' programs to tell your own address when they read it. A request for a receipt in mail you receive is ignored: MailCraft never answers one, and nobody learns from MailCraft when you read their message.

## Permissions asked of a Microsoft account

When you sign in with Microsoft, its page lists what MailCraft asks for. Each is used on your
computer, for you, and for nothing else:

- reading and sending your mail (IMAP and SMTP access, and sending through Microsoft Graph);
- reading and changing your calendar, and reading your contacts;
- reading your mailbox settings (working hours, categories, automatic replies, and the rules your mailbox keeps on Microsoft's servers);
- changing your automatic replies (`MailboxSettings.ReadWrite`), so that File › Info › Automatic Replies
  can turn them on or off and change their text: never asked for on a sign-in page; it is asked for by
  itself the first time you change your automatic replies or a rule is to go to the server (below), after
  the program has asked you, and a yes to either is a yes to both. File › Info shows whether it was given.
  Only your automatic replies and your rules are changed with it, never your categories, working hours or
  other settings;
- reading and changing your To Do lists and tasks (`Tasks.ReadWrite`), so that your tasks are kept in
  step with Microsoft To Do: your tasks' titles, notes, dates, importance, categories and whether they are
  done go to your own Microsoft account and come back from it, and nowhere else. It is never asked for
  on a sign-in page, so signing in for mail never waits on it: it is asked for by itself, for a new account
  and an old one alike, the first time its tasks would be kept in step, after the program has asked you,
  and File › Info shows whether it was given. Until it is, tasks stay on this computer and nothing of them is sent. Even then, tasks that exist only on this computer are not sent on their own: the program asks you once ("Send your tasks to your account's To Do?", with the account to choose when you have several, and Not now) and keeps them here until you say yes; File › Info offers it again. A new task goes to the account you choose when you make it, and one made from a message (Add to Tasks) to that message's account when it has To Do;
- changing the rules your mailbox keeps on Microsoft's servers (the same permission), so that a
  rule you keep there runs whether MailCraft is open or not, and shows in Outlook on the web and on your
  phone: the rule's name, the words it looks for and what it does go to your own Microsoft account and
  nowhere else. It is never asked for on a sign-in page, so signing in for mail never waits on it: it is
  asked for by itself the first time a rule is to go to the server, after the program has asked you, and
  File › Info shows whether it was given. Until it is, your rules stay on this computer and nothing of them
  is sent; the rules already on the server are read with your mailbox settings and shown;
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

## Permissions asked of a Google account

When you sign in with Google (Gmail or Google Workspace), Google's page lists what MailCraft asks
for. MailCraft asks for three things, and uses each on your computer, for you, and for nothing else:

- `https://mail.google.com/`: reading, sending, moving and deleting your mail over IMAP and SMTP,
  as you do in the program. This is the only permission Google offers for a mail program that uses
  IMAP and SMTP, and it is broad: it is not read-only. Once moving mail between Gmail's inbox
  categories is switched on (it isn't in this version), the same permission also lets the program
  move a message you moved there through Gmail's API, as the table above says;
- `openid` and `email`: your Google address, so that the program can check that the account you
  signed in with is the one you added. It is read from what Google sends back at sign-in, and
  MailCraft asks Google for nothing else about you;
- keeping you signed in (a refresh token, stored as described above).

MailCraft does not ask for your Google Calendar, Contacts, Drive or anything else of your Google
account. The mail goes from Google's servers to your computer and stays there: nothing of your
Google data is sent to the project, to anybody who works on it, or to anybody else. (The project's
one service, the report relay, receives only a problem report you write and send yourself, and
its diagnostics hold no message, address or sign-in.) Nobody at the project can read it, it is not used for advertising, it is
not sold, and it is not used to develop, improve or train any AI or machine-learning model.
Scripts and AI agents (below) can read it only on your own computer, when you start the program
with them and they act for you. MailCraft itself sends none of it to any AI service. An agent you
connect is a program you chose, under its own terms: what it does with what it reads is up to it
and to you, and MailCraft never asks it to send your mail anywhere.

Your mail is kept on your computer until you delete it, remove the account, or use
File › Info › Remove MailCraft's data from this computer.

MailCraft's use and transfer to any other app of information received from Google APIs will
adhere to the [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy),
including the Limited Use requirements.

You can take the permissions back at any time at <https://myaccount.google.com/permissions>;
MailCraft then asks you to sign in again the next time it needs them. File › Info › Remove
MailCraft's data from this computer removes your mail and every stored sign-in from your computer.

## Problem reports

Help › Report a Problem shows you everything a report holds before anything goes: the title and
the text you type, the version and platform, an optional GitHub username, and the diagnostics
(the version, the platform and the end of the program's log with addresses, names, paths, hosts,
links, ids and quoted text taken out), which you can untick. Nothing is sent until you press Send
report, and a script or an AI agent can fill the window but never send it.

- **What the relay sees.** The report, and your IP address, which it needs to limit how many
  reports come from one place (10 a day). The relay never logs or keeps the address: it counts
  it only as a keyed hash whose key changes every day, and the day's counts are deleted when the
  next day's first report comes in. The relay runs on Microsoft Azure, whose monitoring
  (Application Insights) works out a rough location (country and region) from the address of
  each request and then keeps the address only as `0.0.0.0`. The relay's log holds one line for
  each report it took, with the report's number, and a line when one couldn't be filed, saying
  why; nothing of what was in the report.
- **Where reports go.** The relay files each report as an issue in a private repository of the
  project on GitHub, which only the project's maintainers can read. GitHub keeps it under its own
  terms, as it keeps the project's public issues.
- **Made public only as a copy.** When a report is a real problem, a maintainer may copy it into
  the project's public issues on GitHub, with what you wrote and the diagnostics, after taking out
  anything personal they find, and mention your GitHub username there if you gave one. That is
  why the window asks you to leave out anything private. A report that isn't copied stays private
  and notifies nobody.
- **Open on GitHub** sends nothing through MailCraft: it opens GitHub's own page in your browser,
  filled in, for you to read and submit there, publicly, under your GitHub account.

## Scripts and agents

MailCraft can be driven by scripts and AI agents through a control channel and an MCP server.
Both are off unless you start the program with them, listen on your own computer only, and take
requests only with a key the program keeps in a file in your settings folder.

## Changes and contact

This statement lives in the project's repository, and its history shows every change:
<https://github.com/UCS-Care-Group/mailcraft/blob/main/PRIVACY.md>. Questions and reports:
<https://github.com/UCS-Care-Group/mailcraft/issues>.
