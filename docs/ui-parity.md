# Interface parity

> **Last reviewed:** 2026-10-13 · **Last updated:** 2026-10-17 · **Change:** minor (#70: mail written in a shared mailbox is from it, and the first send from one asks first; #70: a shared mailbox's changes go up, delete to its own Deleted Items, moves between mailboxes refused with the reason; #70: a shared mailbox in the folder pane, Favorites and search; #120: Skip and Full messages in Send/Receive Progress; #120: Send/Receive Progress panel); patch (#122: Signatures once in File; About ▸ Contributors, D2; Copy Diagnostics in the title bar, D1) · **Target:** Microsoft Outlook (Microsoft 365, classic desktop)

How the window compares with the target's. The look is the Craft apps' own (WordCraft's design
tokens, type rungs and ribbon widgets, with MailCraft's teal accent); the *arrangement* is the one
Outlook users know. No pixel of the target's artwork is used: every icon is drawn in code.

## The window

| Part | State | Notes |
|---|---|---|
| Title bar | yes | Quick Access (Send/Receive, Undo, Redo), search box with scope, a Copy Diagnostics pill (D1), account avatar. macOS integrated title bar. No customisation |
| Ribbon | yes | File, Home (changes with the module), Send / Receive, Folder, View, Help, and a Message tab while writing. Collapsible by double-click. No keytips (Alt), no customisation, no simplified ribbon |
| Module bar | yes | Mail, Calendar, People, Tasks, with the Inbox's unread count. No Notes, no reordering |
| Folder pane | yes | Favorites, account trees, collapse/expand, unread counts, context menu, resizable. For an account that signs in with Microsoft the Favorites are the folders Outlook shows there, in Outlook's order, read from the server (never written: they are changed in Outlook); one quiet line under the heading offers to connect them, or says why they can't be had, and File › Options switches back to the favourites kept here. They are read through Microsoft Graph, for a work or school account as for any other; when an administrator has to approve it first, the pane says so and offers what can be done (a message to copy for the administrator, the page that says what to do next). No drag and drop, no search folders, no groups; a favourite that is a person, a category or a group is not shown. A shared mailbox (being tested, behind a switch) has its own tree under its owner's, its unread counted on its own folders; its folders can be put among the Favorites, kept in MailCraft whatever the owner's come from, and are named there with the mailbox ("Inbox - support@…"). Outlook's own Favorites that point into a shared mailbox are not shown yet |
| Message list | yes | Laid out from the target's at 100 %, to the pixel ([the measurements](#the-message-list-measured)): a header with All and Unread as tabs and the arrangement (By Date) with the arrow that turns the list over; rows of the sender (11 pt), the subject with the time at the end of its line, and 0–3 lines of preview (8 pt); a line of dots round the row the keys act on; the bar an unread message wears; a scroll bar with arrows in its own lane. Date/sender/importance groups, conversations as one row with a count, marks, category marks, flag and delete on the row under the pointer, the read switch down a row's left edge, dragging to a folder, context menu, multi-select with Ctrl/Shift, keyboard movement, virtualised. The preview is what the message says to its reader: the words of its HTML (the opening line a sender hid for message lists included), without markup, invisible padding or web addresses. No column view, no collapsing of groups, no opening a conversation in place; the tabs do not turn into a menu in a narrow list (the one in use stays, the others are in the arrangement's menu) |
| Reading pane | yes | Right, bottom or off; header with avatar, recipients, date; notes (flag, importance, replied, pictures withheld); attachment chips; conversation jump list; formatted body with clickable links. Simplified HTML (gap #6); no zoom, no inline reply |
| Composer | yes | In place of the reading pane: From (several accounts), To, Cc, Bcc, Subject, attachment chips, body in rich text (bold, italic, underline, a size, links, bulleted and numbered lists eight levels deep, a font and a text colour, a signature's pictures; copy and paste as HTML on the desktop; the Message tab's Basic Text group and Ctrl+B/I/U/K/Z/Y while the body has the keyboard), the original of a reply shown as the reading pane shows it; Send, Attach, Save draft, Discard. Gap #7; no pop-out window |
| Calendar | yes | Side pane with a month navigator and calendar list; day, work-week, week and month grids; drag to move. A second time zone in the time bar. No schedule view, overlay, drag to resize or create |
| People | yes | List with letter headings and favourites; contact card with recent mail. No business-card or table views, no groups |
| Tasks | yes | Filters, quick add, grouped list with check boxes, details pane. No To-Do bar, no table view |
| Backstage (File) | yes | Info (accounts, mailbox, rules, categories), Open & Export, Options (general, mail, calendar), Signatures (the list, the composer's editor with Insert/Paste Picture and a picture's width, description and layout, Create from details, a preview as recipients see it and in plain text, each account's two defaults), About. No print, no automatic replies, no add-ins |
| Status bar | yes | Counts, messages from the last action, Send/Receive status (a click shows Send/Receive Progress), reading-pane switch. No zoom slider, no connection indicator |
| Send/Receive Progress | yes | A panel over the foot of the main window, not a window of its own: opened from the status bar's Send/Receive text, its red "Send/Receive problems" link (at the first problem) or Send / Receive › Show Progress, never by itself; Escape or a click elsewhere closes it. One row per account in the run's order: waiting, the phase and counts of the one running with a bar when the total is known (else a spinner), and what each one that is done came to ("Done: 34 new, 2 sent", "Stopped", "2 problems"); the problems under each account; Skip on each account still waiting (its row then says "Skipped"); under a Microsoft account, how far its messages have been fetched whole ("Full messages: 40 of 900"), paused by Stop until the next Send/Receive and said so; Stop while it runs. No overall bar, no Details/history, no stopping one account mid-run (Stop is that, as accounts go one at a time) |
| Dialogs | partial | Search Commands, New/Rename Folder, Go to Date, Sign In, unsaved-message prompt, About (with a Contributors tab), and forms for appointments, contacts, tasks, accounts, categories and rules. The target has many more (Advanced Find, Rules Wizard, Options panes, permissions…) |
| Add Account | yes | From the address alone: whose servers it uses, then a password that is tried before the account is added, or Microsoft's sign-in page in the browser. While the browser has the sign-in, the page says so ("Microsoft's sign-in page is open in your browser"), in what order things come for a work or school account (the sign-in, then how Outlook's Favorites are read), and what to do if Microsoft's page asks for an administrator: nothing has to come back for that. "Your organisation has to approve MailCraft first" is a page of its own, shown when the sign-in comes back saying so or from its link: why, and what to do next (an administrator approves on Microsoft's page, opened from there and waited for; anybody else copies a message for one; try again; not now, which leaves nothing behind). A sign-in that ends otherwise says how, in a sentence (the page came back with a no, nothing came back, no browser could be started), with Try again and the way to that page. An account that exists gets the same page as a dialog when a Send/Receive its owner asked for is refused for want of an approval. MailCraft has no web view: the provider's page is in the system's browser, never in this window |

## The message list, measured

Distances in pixels at 100 %, from the target's list in its dark theme beside ours (2026-10-12).
A row is measured from the hairline above it.

| What | Target | Ours |
|---|---|---|
| Hairline to the top of the sender's capitals | 15 clear | 15 |
| Under the sender's capitals to the top of the subject's | 7 clear | 7 |
| Under the subject's capitals to the top of the preview's | 7 clear | 7 |
| Under the preview's capitals to the hairline below | 15 clear | 15 |
| Row to row, one line of preview | 72 | 72 (57 with none, 13 more for each further line: the system font's own line at that size) |
| Sender; subject, time and preview | 11 pt (capitals 11 tall); 8 pt (capitals 8 tall) | the same |
| Text from the list's left edge | 30 | 30 |
| Where text stops, and where the time ends, from the row's right end | 98; 24 | 98; 24 |
| Hairline | 1, `#666666`, from 2 inside the list's left edge to the rows' right end | the same (`border_strong`; `border` on a light list) |
| Right of the rows | 7 clear, a lane of 17 (`#141414`) with a handle of 9 and an arrow at each end, 5 to the list's edge | the same |
| Header | 48 high; tabs at 11 pt from 29 in, a line of 1 under the one in use on the header's last row; the arrangement at 9 pt, its arrow 11 from the rows' end | the same |
| Selected row | filled `#424242`, a line of dots (`#BDBDBD`, one on, one off) round it | `#444444` (`selection`), dots in `text_dim` |
| Unread | a bar of 4 (`#58B8FE`) down the row from 2 inside the list's edge | the same (`accent`), with the sender, subject and time in the heavier weight and the subject and time in the accent |

What differs: letters are drawn by our own rasteriser, not the system's, so strokes are a little
heavier than the target's at these sizes; the lines of a two- or three-line preview are 13 apart
by the font's own measure, which the reference (one line of preview) does not show; where marks
go when a row has several (files, importance, categories) is ours, the reference having none.

## Themes, sizes, input

- **Light, dark and follow-the-system** themes; every colour comes from `theme::Tokens`.
- **Interface size** 80–200 % on top of the system scale.
- **Keyboard:** the registry's shortcuts follow the target's where it has them (Ctrl+N, Ctrl+R,
  Ctrl+Shift+R, Ctrl+F, Ctrl+Q/U, Ctrl+1–4, Ctrl+E, Ctrl+Shift+A/Q/C/K, F9, Delete, arrows).
  Missing: keytips, Customize Keyboard, full keyboard access to every control, screen-reader
  labels beyond what egui/AccessKit give by default.
- **Languages:** English; every string goes through `tl!` and a pseudo-language proves it, but
  there are no catalogs and no right-to-left layout (gap #14).
- **Touch and pen:** whatever egui provides; nothing designed for them.

## Hardware and platforms

Rendering is egui on wgpu (DirectX 12 with an OpenGL fallback on Windows, Metal, Vulkan; WebGPU
with a WebGL2 fallback in the browser). Verified here: Windows 11 x64 and a Chromium browser.
High-DPI is handled by egui; multi-window, system tray, notifications and badges are absent
(gap #11).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-11 | major | First version |
| 2026-10-12 | minor | The message list measured against the target's: rows, header, scroll bar, previews |
| 2026-10-12 | patch | The calendar's time bar can show a second time zone |
| 2026-10-13 | minor | The Favorites of the folder pane are Outlook's own, read from the server, for an account that signs in with Microsoft (M4.2) |
| 2026-10-13 | minor | A work or school account chooses how Outlook's Favorites are read: Microsoft Graph, Exchange Web Services, or not now (M4.3) |
| 2026-10-13 | minor | Add Account: what the page says while the browser has the sign-in, the page about an organisation's approval, and how a sign-in that ended says so (M4.4) |
| 2026-10-15 | minor | Exchange Web Services removed (R2): no choice of how Outlook's Favorites are read, in Add Account, File › Options or the folder pane |
| 2026-10-15 | minor | The composer edits rich text as it will be sent (R4.2), and shows the original of a reply through the reading pane's renderer |
| 2026-10-15 | minor | Rich signatures with a logo (R4.5): File › Signatures, font and colour in the composer, pictures drawn in the editor |
| 2026-10-15 | minor | Copy Diagnostics in the title bar (D1): a pill beside the account picture runs Help › Copy Diagnostics |
| 2026-10-15 | minor | About ▸ Contributors (D2): a Contributors tab in the About window; File › About › Show Contributors |
| 2026-10-17 | minor | Send/Receive Progress (#120): a panel in the main window, from the status bar and Send / Receive › Show Progress; the Send/Receive problems dialog folds into it |
| 2026-10-17 | minor | Send/Receive Progress (#120): Skip for a waiting account; the Full messages line under a Microsoft account, paused by Stop until the next Send/Receive |
| 2026-10-17 | patch | Signatures appear once in File (#122): their own page; File › Options no longer has a Signatures section |
| 2026-10-17 | minor | #70: a shared mailbox's folders among the Favorites, named with their mailbox; the bar over a search in one names it too; search of All Mailboxes takes shared mailboxes in |
| 2026-10-17 | minor | #70: what is done in a shared mailbox (read, flag, categorize, move, copy, delete to its own Deleted Items, delete for good, its folders) goes up to it, and Undo with it; moving or copying mail between it and another mailbox says why it can't; a change it refuses waits, and Send/Receive Progress says so under its row |
| 2026-10-17 | minor | #70: a reply, forward or new message begun in a shared mailbox is From its address (the From picker lists it with the owner's); its drafts are in its own Drafts; the first send from one asks "Send from a Shared Mailbox" before Microsoft's page opens, and a refusal is said on the held message in its Outbox; the Add Shared Mailbox dialog no longer says mail goes from the owner's address; its signatures are chosen beside the accounts' |
