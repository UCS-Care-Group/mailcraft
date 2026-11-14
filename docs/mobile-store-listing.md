# Android store listing and data safety

> **Last reviewed:** 2026-11-13 · **Last updated:** 2026-11-13 · **Change:** major

The words for MailCraft's Google Play listing: the data-safety answers, each with its reason, and
the short and full descriptions. Every sentence says only what the app does as built on the open mobile pull requests, which are
not on `main` yet (the
privacy side is [`PRIVACY.md`](../PRIVACY.md), [The phone apps](../PRIVACY.md#the-phone-apps)).
Screenshots come from the screenshot tests, with invented people and `example.org` addresses
only. When the app changes what it collects or sends, this page and `PRIVACY.md` change in the
same pull request.

## Data safety answers

| Question | Answer | Reason |
|---|---|---|
| Does the app collect user data? | Only the optional problem report | The app has no telemetry, analytics, advertising or crash reporting. The one thing the project receives is a problem report the owner chooses to send, after the window has shown what it holds. |
| Data collected, and is it optional? | Diagnostics, what the owner types in a problem report, and the optional GitHub username; optional, only when sent | Collected means sent off the device to the project. The report is shown first, is never sent by itself, and holds no message, address or sign-in. The relay's monitoring works out a rough location (country and region) from the IP address of the request. To decide: whether Play counts the optional username as a user ID and that location as approximate location. |
| Is data shared with third parties? | No | Nothing goes to anybody on the project's behalf. Mail, calendar and tasks travel only between the phone and the owner's own provider, and pictures a message names are fetched from the address it gives, only when the owner asks. Sender images are off by default; when the owner turns them on, the switch says who is told what (a hash of the sender's address to Gravatar, the sender's site domain to favicon.im and Google's icon service), so it is an action the owner started with a plain disclosure, and nothing is sent on the project's behalf. |
| Is data encrypted in transit? | Yes | All of the owner's data (mail, sign-in, calendar, tasks) goes over TLS. The one plain `http` request is a picture the owner allowed whose address the message gives as `http`; it carries nothing of the owner's but that address. |
| Can the owner ask for data to be deleted? | Yes for data on the device; no for a sent report | Mail, settings and sign-ins are kept only on the phone, and uninstalling the app removes them. A report already sent is kept as an issue, and the privacy statement names no way to ask for it to be deleted yet: the owner decides whether to add one (an issue or contact link) before the form is filled in. |
| Is the data processed ephemerally? | No | Play asks this of collected data, which is the problem report: it is kept as an issue, not discarded. The mailbox stays on the device and is not collected. |
| Does the app follow the families policy or target children? | No | It is a mail program for adults. |
| Account creation | None | There is no MailCraft account. Sign-in is on the provider's own page. |
| Privacy policy link | `PRIVACY.md` in the project's repository | The address to give is decided by the owner. |

Not claimed: that nothing is collected at all. The problem report is collected, so the form says
so.

## Short description (80 characters at most)

Mail, calendar, people and tasks for Microsoft accounts. Private, no tracking.

## Full description

MailCraft is mail, calendar, people and tasks for your Microsoft accounts, on your phone and
tablet, free and open source.

Sign in on Microsoft's own page. MailCraft never sees your password. Add several accounts and
read the mail of shared mailboxes you have been given access to.

**Mail.** Folders, a fast message list with filters and swipe actions you choose, conversations,
reply, reply all and forward, search, flags, categories, junk, archive and move, attachments you
can open, save or share, and a composer with bold, italic, underline, lists, links and pictures.

**Calendar.** An agenda, day and month views; make and change meetings; answer invitations.

**People and tasks.** Find a person and tap to write or call. Your Microsoft To Do lists and tasks
are shown and can be changed on the phone.

**Private by design.**
- Nothing in a message runs, and a message's pictures are fetched only when you ask.
- Your sign-in is kept in your phone's secure key store, never in a file in the clear.
- Your mail stays on your phone and is left out of backups.
- No account with us, no tracking, no analytics and no advertising.
- The only thing we ever receive is a problem report you choose to send, after you have seen what
  it holds.

**Made to read.** Messages are drawn from their text and layout at your phone's width, in light
and dark, with large text and a screen reader in mind.

MailCraft is open source under the MIT or Apache 2.0 licence, written in Rust from public
specifications. The source and the privacy statement are at github.com/UCS-Care-Group/mailcraft.

## To check before the first release

- The notification, background check and problem report parts of the privacy statement are marked
  "when the app is released". Remove the mark, and check this page again, once the app does them.
- Add a line to the full description for each of them only when it is built. Reminders
  (notifications are not built) and the two-way tasks sync (the phone has no tasks sync run yet)
  are left out of the full description until they are.
- Check the merged manifest's permissions against the privacy statement's list: the release is
  planned to add notifications, exact alarms and what WorkManager brings.
- The privacy policy address is the owner's choice (the repository's `PRIVACY.md`, or a page on
  `mailcraft.work`).

## Revision history

| Date | Change | Summary |
|---|---|---|
