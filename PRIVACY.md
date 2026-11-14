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
| A JMAP server you add an account for (the session address you gave it, and the addresses at the same server that its session names) | Send/Receive, on the timer you set, a few seconds after you mark, move or delete a message of the account or send one from it, and when you open a message that isn't downloaded yet; only in a build that connects to JMAP servers (the browser build, from the version that adds it) | What a mail program sends: your password or access token, and what you change in your mail: the marks you set on a message (read, flagged, answered, forwarded), the messages you move between the server's folders and those you delete for good are told to that server, and to nobody else. Mail you send from the account goes to that server to be sent, and to nobody else: the message whole, and the addresses it goes to, those in Bcc among them (the message itself names no Bcc); the server keeps its copy, in the account's Sent folder where it has one. A password or token you type is kept in memory only, until the program closes, the server turns it down or the account's server changes; it is never remembered in the credential store, the mailbox file, the settings or the browser's storage. Where the page you use signed you in itself, the program asks it for the credential at each Send/Receive or download (once more if the server turns it down), and keeps none of it beyond that |
| Microsoft's sign-in pages and Microsoft Graph, for an account that signs in with Microsoft | When you sign in; for calendar, contacts, tasks (Microsoft To Do), sending mail, working hours, categories, automatic replies, the rules kept on the server and Outlook's Favorites | Your sign-in happens on Microsoft's page in your browser; the program receives tokens, never your password |
| Google's sign-in page (`accounts.google.com/o/oauth2/v2/auth`) and token endpoint (`oauth2.googleapis.com/token`), for an account that signs in with Google | When you sign in, switch an existing account to Google sign-in or reconnect; the token endpoint is also used when a saved sign-in is renewed | The browser's sign-in request carries the application's client ID, the permissions requested, your account address as a login hint, a random state value, a PKCE challenge and a return address on your own computer. The token endpoint receives the authorization code and PKCE verifier, or the refresh token, together with the application's client ID and Desktop client-secret registration value. Google returns tokens and identifies the signed-in address in the identity token; MailCraft makes no separate user-info request. You enter your Google password on Google's page: MailCraft never sees it. |
| Gmail's API (`gmail.googleapis.com`), for an account that signs in with Google, to move mail between Gmail's inbox categories | Not in this version: moving mail between Gmail's categories is switched off until Google has reviewed MailCraft's use of the API. Once it is switched on, only at the Send/Receive after you move a message of the Inbox from one Gmail category to another; never for an account that signs in with an app password | Gmail's own id of each message you moved, the category it goes to (the labels to add and take off), and the account's sign-in token, which Gmail's API takes as it is; nothing else of the message or of you |
| A public DNS-over-HTTPS resolver (Cloudflare, then Google) | When you press Connect while adding an account at a domain the program does not know: once for the domain's mail exchangers and twice for where its Exchange automatic setup is (the `_autodiscover._tcp` record and `autodiscover.<domain>`), and, when the exchangers name no provider the program knows, up to four times more for the DNS records that say where the domain's mail servers are (its `_imaps`, `_imap`, `_submissions` and `_submission` SRV records); and while an Exchange server account is set up | The domain of your address (the part after the @), never the mailbox name |
| Your address's own domain, over HTTPS (`autoconfig.<domain>`, then `<domain>` at `/.well-known/autoconfig/…`) | Only when you press Connect while adding an account at a domain the program does not know and neither its mail exchangers nor its DNS records say where both its mail servers are; nothing is sent to the mail servers found until you have seen them and pressed Connect again | A request for the settings the domain publishes for mail programs, with the domain in the address and nothing else: not the mailbox name, no cookie, no sign-in |
| Mozilla's ISP database (`autoconfig.thunderbird.net`) | Only after that, when neither the domain's DNS records nor what it publishes say where both its mail servers are | The domain of your address, and then, when its mail exchangers are in another domain (a hosting company's), that domain; never the mailbox name |
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

## The phone apps

MailCraft for Android (and, after it, for iPhone and iPad) is the same program on a smaller
screen, and everything above holds for it unless this section says otherwise. There is no iPhone
or iPad app yet: what is said of iPhone below is for when that app is released. There is still no
MailCraft account and no MailCraft server that sees your mail: nothing goes through a server of
the project, and notifications are raised on the phone itself. Parts of the phone apps that are
not in a release yet are marked "when the app is released".

- **Signing in.** You sign in on Microsoft's page, in a browser tab the system opens (Custom Tabs
  on Android; on iPhone, when that app is released, the system's own sign-in session); the app never shows you a page
  of its own for your password and never sees it. The page sends you back through a link on a
  scheme only MailCraft registers (`work.mailcraft.android` on Android), and the app checks that
  link, to the letter, before it believes it. What is asked of your account, and why, is
  [Permissions asked of a Microsoft account](#permissions-asked-of-a-microsoft-account). The phone
  apps do not use your phone's own calendar or contacts and ask for no permission to them.
- **Secrets.** A sign-in token is sealed with a key held in the Android Keystore (on iPhone,
  when that app is released, the Keychain), which the system keeps outside the app's own files, and the
  sealed file sits in the app's own folder that is left out of backups. Nothing secret goes into
  the mailbox file, the settings, a log, a link or a command. Access tokens are never stored.
- **Your mailbox** is kept in the app's own folder on the phone, which Android does not copy to a
  cloud backup or to another phone (backup and device transfer are switched off for the app); the
  mailbox is fetched again from your provider when you set up a new phone. Uninstalling the app
  removes everything it kept.
- **Connections.** Every connection that carries your mail, a sign-in or anything of yours is
  encrypted (TLS). The one exception is as on the desktop: a picture you allowed whose address
  in the message is plain `http` is fetched over plain `http`, and nothing of yours travels with
  that request but the address. The Android app's network security config and manifest allow no
  unencrypted traffic for the phone's own web stack (links open in the browser, which is the
  system's), and the app trusts only the system's own certificate authorities. As built today,
  the only permissions the Android app asks of the phone are to use the network and to see
  whether it is connected, how it is metered and whether Data Saver is on. The release adds
  more, and this section lists them then: notifications, exact alarms for reminders, and what
  the background checks bring (to run after a restart and to keep the phone awake for a moment).
  The phone's proxy settings are ignored.
- **Background checks, when the app is released.** Android runs a check about every 15 minutes
  (WorkManager) when there is a network, and iPhone, when that app is released, whenever the
  system grants one (BGAppRefreshTask); the app also checks when you open it and when you pull the list down. A check
  asks your own provider (Microsoft) for the headers of each account's Inbox and for changes to
  its calendar and To Do tasks. The bodies of new mail within how long you keep mail offline come
  only on a connection that is not metered. Pictures, sender images and other folders come only
  when you open the app. The system decides when a check runs and may skip it: the app says
  "Updated N min ago" and "Offline" instead of promising. The app asks for no battery exemption
  and runs no foreground service, and exact alarms are used for reminders only, never to check
  mail.
- **Notifications, when the app is released.** A notification for new mail shows the sender and
  the subject and never the body or a picture, only for mail that would alert (Primary or Focused,
  as the account's setting says). A switch in Settings shows "New mail" and nothing else, and on a lock
  screen the system hides private content from, a public version that says only "New mail"
  stands in for the notification. Reminders for meetings
  and tasks are notifications raised on the phone from what the last check brought. Nothing about
  a notification goes to a server of the project, or to Google's or Apple's push services: no
  push token is made and none is sent.
- **Pictures on the web.** The rules are the desktop's, as in the table above: a message's
  pictures are fetched only when you ask for them for that message, for a sender or a domain you
  chose, or with the switch in Settings (off by default; for mail in Junk E-mail only for that
  message), only while the message is on screen, with the address alone and no cookies, no referrer
  and no sign-in. On Android, while Data Saver is on and the connection is metered, an allowed
  picture waits for a tap instead of coming by itself. Downloaded pictures are kept in the app's
  cache folder, named by hashes with the address never written, at most 4,000 pictures, 256 MB and
  30 days, and Settings has Clear. Android may also empty that cache on its own.
- **Sender images** are off until you turn them on in Settings, and follow the desktop's rules:
  only for the senders in the part of the list on screen, only for people you know or whose mail's
  pictures you allow, never for mail in Junk E-mail, from the same three services and telling each
  the same thing (a hash of the address to Gravatar, the sender's site domain to the other two).
  While Data Saver is on and the connection is metered, only pictures already in the cache are
  shown and nothing is asked.
- **Reading width and dark mode.** A message is laid out at your phone's real width and in its
  light or dark state, so a message can show another picture for a dark window or a narrow width
  and, once you allow its pictures, its server can tell from the address asked for that the window
  is dark and how wide the message is laid out, to within a few pixels. This is the same
  disclosure as the desktop's, above, and nothing else about the phone is told: not its model, its
  system version, its screen or its language. Nothing is fetched until you allow pictures.
- **Problem reports, when the app is released.** The one thing the project receives from the phone
  apps is a problem report you choose to send, through the same relay and with the same window
  contents and limits as the desktop's ([Problem reports](#problem-reports)): it is shown to you
  first, never sent by itself, and holds no message, address or sign-in. Nothing else is sent to
  the project from the phone: no telemetry, no analytics, no advertising identifier, no crash
  reporting, and none of Google Play services or Firebase in the Android app.
- **Opening and sharing.** An attachment you open goes to the app you choose through a one-off,
  read-only grant on a temporary copy in the app's cache folder, which is cleared when the app next
  starts; Save uses the system's file picker, so the app needs no storage permission. Web links
  open in your browser, `mailto` links in the composer, and `tel` links in the dialer, which
  shows the number and does not dial; only `http`, `https`, `mailto` and `tel` links are followed.

## Scripts and agents

MailCraft can be driven by scripts and AI agents through a control channel and an MCP server.
Both are off unless you start the program with them, listen on your own computer only, and take
requests only with a key the program keeps in a file in your settings folder.

## Changes and contact

This statement lives in the project's repository, and its history shows every change:
<https://github.com/UCS-Care-Group/mailcraft/blob/main/PRIVACY.md>. Questions and reports:
<https://github.com/UCS-Care-Group/mailcraft/issues>.
