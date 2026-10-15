# MailCraft: domain language

A glossary of the words the project uses with one meaning. Not a spec: how things are built lives in `docs/`.

## People

- **Owner**: the person whose mailbox MailCraft holds. The docs and the interface say "owner", never "user", so it is never confused with an account's user name.
- **Tester**: an owner other than the project's authors who uses a release daily and reports what goes wrong. Only testers count as evidence of readiness.
- **Administrator**: a person who can approve applications for an organisation's Microsoft directory. Not the owner, as a rule.

## Accounts

- **Microsoft account**: an account that signs in with Microsoft and whose mail is kept in step through Microsoft Graph, never IMAP or SMTP. Either a **personal Microsoft account** (Outlook.com, Hotmail, Live) or a **work or school account** (Microsoft 365, in an organisation's directory).
- **IMAP account**: any other account, reached by IMAP and SMTP with a password.
- **Shared mailbox**: a mailbox other than the owner's own that the owner may open (a team's, or another person's as their delegate). Not supported before beta.

## Mail

- **Server id**: the name the server gives a message or a folder, which stays the same when the folder is renamed or moved and when the message is moved within the mailbox. How MailCraft knows it is the same message or folder. The Message-ID a message carries is not a server id: it can be missing or shared.
- **Fetched**: a message whose whole content (body and attachments) is on this computer. Until then it shows its headers and a preview, and is fetched when the owner opens it or in the background, newest first.
- **Delete**: move to Deleted Items, here and on the server. Undo moves it back.
- **Delete for good**: remove from every folder (Shift+Del, or emptying Deleted Items). As in Outlook, the server keeps it recoverable for its retention period; MailCraft never removes anything past that.
- **Held message**: a message in the Outbox that MailCraft will not send by itself, because it may already have gone (the server didn't say) or the server refused it. The owner looks and decides.
- **Signature**: text, and at most a few pictures (a logo), that the owner chooses to end their messages with. Kept on this computer only: Microsoft offers no way to keep signatures in step, so another device has its own. Each account names one for new messages and one for replies and forwards, either of which may be none. A plain-text message carries its words only.
- **Last synced state**: what the server last said of a message (read, flag, categories, importance, the folder it is in). A property is **changed here** when the message differs from it; only what was changed here goes up, and where the server changed the same message too, what was changed here wins for those properties alone (categories merge: what was added or taken away here is applied to the server's list).
- **Category**: a name the owner puts on messages, kept in step by name. Its colour comes from Outlook's list of categories when the owner lets MailCraft read it; MailCraft never changes that list.
- **Local folder**: a folder that exists only on this computer, never on a server. The only place mail may be changed without the server knowing.
- **In step**: an account is in step when every change the owner made to its mail has reached the server or been refused with a reason, and what the server holds is what MailCraft shows. Undoing a change that reached the server is another change sent to it.

## Progress

- **Milestone**: a named stage with a pass/fail checklist. A milestone is reached when every item on its checklist passes, and not before.
- **Beta**: the milestone at which at least 5 testers, each on a Microsoft account and at least 2 devices, use MailCraft on 14 working days with no data-loss incident and no crash that loses work, reading, filing, searching, replying in rich text, forwarding and answering meetings, with everything in step with Outlook on the web, on Windows, macOS and Linux, for accounts of up to 200,000 messages.
- **Data-loss incident**: anything a tester had (a message, a flag, a folder, an appointment, a reply being written) that is gone or changed without their asking, here or on the server.
- **Crash that loses work**: the program ending without the owner's asking and taking with it something they had done or written.
- **Diagnostics**: what an owner copies to report a problem: the program's version and a record of what it did, with no message content, address, password or token in it. Nothing is ever sent by itself.
- **Measured number**: a figure a command or a count can reproduce (catalog coverage, tests, servers verified, testers, days of use). The only kind of number the roadmap reports.

## Flagged ambiguities

- "Real server" was used both for "one author's Microsoft 365 mailbox" and for "any server not a stand-in". Say **tester's account** or **stand-in** instead.
- "Released" was used both for "built locally" and "published on GitHub". Say **published** for the second.
