# Calendar: matching Outlook

Status: working list, 2026-10-24 (fourteenth revision: a shared mailbox's calendar that the server also lists under Shared Calendars is offered, once, to be hidden; thirteenth revision: an appointment moves between your calendar and a shared mailbox's or one opened by address; twelfth revision: calendars opened by address, on the shared mailboxes' code; eleventh revision: a shared mailbox's calendars are written to, and its meeting requests answered; tenth revision: a shared mailbox's calendars are read; ninth revision: calendars know whose they are and what the account may do with them; eighth revision: the views on View › Arrangement too; seventh: the fourth wave is in; sixth: time zones; fifth: a change that doesn't reach the server can't go unnoticed). The owner's bar for the
calendar is "behaves exactly like Outlook"; this is the list that bar is checked against.
Behaviour is written down from using Outlook for Windows (classic) as a black box; nothing of it
is copied but how it behaves.

Legend: **Done** works and is tested · **Now** being built · **Next** planned, in the order of the
waves at the end · **Later** known, not scheduled. "Tested" means against stand-ins for the
server; what has met the real Microsoft 365 service is only what the owner has used.

## Views and getting around

| Outlook does | MailCraft |
|---|---|
| Day, Work Week, Week, Month views, switched from the ribbon and with Ctrl+Alt+1…4 | Done (Home › Arrange and View › Arrangement, where the calendar's View tab has them in place of the messages' sorting and preview (#239); the ribbon and the keys are the only switch, as in Outlook; the Simplified ribbon labels them) |
| Schedule view (people or calendars as rows) | Later |
| Today, Back, Forward; the title names the range; Next 7 Days | Done |
| Date navigator: click a day, a week, drag over days; shows more months when the pane is tall; bold days have events | Done (a drag is one run of up to 14 days; a click in the week margin shows that week) |
| Go to Date (Ctrl+G) | Done |
| Alt+1…0 shows that many days; Alt+= month, Alt+- week | Done |
| Half-hour time scale by default; 60, 15, 10, 6 and 5 minutes on request | Done (right-click the time bar, View › Time Scale, or File › Options); no minute labels at the fine scales yet |
| Working hours lighter than the rest, from the owner's settings (start, end, days) | Done: the mailbox's own (days, start, end; asked for once an hour, and it takes the permission to read mailbox settings) for the shading, Work Week's days and Home/End, the local options until they come and whenever File › Options › "Use my own working hours" is ticked. The hours are those of the zone the mailbox keeps them in, shown by the clock the calendar is shown by (a mailbox in Pacific time that works 8 to 5 is shaded 11 to 8 on a calendar in New York); a zone this program doesn't know leaves them as written. The first day of the week stays a local option: the mailbox's settings as Microsoft Graph gives them don't have it |
| A line at the present time | Done |
| A time zone for the calendar, with a label over the time bar; a second time zone beside it | Done: File › Options › Calendar › Time zones (`view.timeZone`). The calendar is shown in this computer's zone and follows it, or in one that is chosen; "Today", the line at the present time, reminders and working hours go by it. A second zone adds a column of its hours to the time bar, each column under its label (the zone's short name, `UTC-5`, unless one is typed). One second zone, where Outlook now takes two |
| The computer is carried to another time zone | Done: every appointment stays at its moment and is read by the new clock; an all-day item stays on its day (Outlook moves it, so that it straddles two days); a series goes on repeating at the clock time of the zone it was made in; reminders come up at the same moments; nothing is sent to the server for it. Noticed while the program runs (within a few minutes) and when it starts |
| Week numbers (option) | Done (ISO weeks) |
| Several calendars side by side or overlaid, each in its colour, under My Calendars / Other Calendars / shared | Done for the account's own: the list by group (the account's own first, its other groups, Other Calendars, On this computer), each group folding; every ticked calendar in a block of its own with its tab, in Day, Work Week, Week and Month; the arrow on a tab (or View › Overlay) lays calendars over one another, the one in front solid and the others dimmer, a click on a tab bringing it to the front; View › Side-by-Side stands them apart. Right-click a calendar: Show Only This Calendar, Overlay, Colour. A calendar that can only be read says so (a lock beside it) and takes no new items. A calendar the server lists that belongs to somebody else (an accepted sharing invitation) is under **Shared Calendars**, whatever group the server puts it in. Eight blocks at most (the calendars beyond share the last). Later: a shared mailbox's calendars, calendars opened by address, Schedule view (#70) |
| The calendar clicked in the list is the one new items go to | Done (the account's own calendar unless another is picked); and what is picked, typed or dropped in a calendar's block is that calendar's, which is then the one being worked in |
| Weather, To-Do bar, daily task list | Later |

## Working with items in the grid

| Outlook does | MailCraft |
|---|---|
| Click selects an item; click an empty slot selects the slot; drag selects a range of slots | Done (a range is within one day in the time grid; across days in Month and the all-day row) |
| Typing with a slot selected starts the item right there; Enter saves, Escape drops it | Done (a click elsewhere saves what was typed, as Outlook does) |
| Double-click an empty slot opens a new appointment for that time; double-click an item opens it | Done, each in a window of its own |
| Drag an item to another time or day; the grid scrolls at its edges | Done (snaps to the time scale, ghost), and the server is told at once |
| Drag the top or bottom edge to change start or end | Done (also the ends of all-day and multi-day bars) |
| Ctrl+drag copies; right-button drag asks Move or Copy | Done (a copy is a plain appointment: attendees and recurrence are not copied) |
| Drag in Month view, and all-day items between days; into and out of the all-day row | Done (a repeating item is not turned all-day or timed by dragging) |
| Arrow keys move the slot selection, Shift+arrows extend it, Tab goes from item to item, Enter opens, Delete deletes | Done; also Home/End, Page Up/Down, Alt+Up/Down to move the item, Ctrl+C/X/V (a cut item stays in place until it is pasted) |
| Hovering shows subject, time, location, organizer | Done (and that it is of high or low importance, or held online) |
| Right-click: Open, Private, Show As, Categorize, Meeting responses, Delete (and Move/Copy to another calendar) | Done: Open, Delete, Show As, Private, Importance (High / Normal / Low), Categorize, Accept / Tentative / Decline, Propose New Time, Forward (and Forward This Occurrence), Join (an online meeting that has a link), Save as iCalendar, Move to Calendar, and a drop in another calendar's block moves the item there (Ctrl: a copy). Between two calendars of the same Microsoft 365 account an appointment moves; the service has no way of moving an event, so it is made in the other calendar first and taken out of the first one only once the copy has been seen there in what the service sends back (one step to undo here; a refusal of either half, or a copy that turns out not to be there, puts it back and says so). Refused, each with its reason: a meeting (everyone invited would get a cancellation and a new invitation), a series and one occurrence of one, an item with files attached (only the server keeps them), and taking an item out of its account. What MailCraft doesn't keep of an item (formatting in its text) is not carried over |
| A recurring item asks "just this one" or "the entire series" when opened, moved, resized or deleted | Done, for series on this computer and on the server (an occurrence dragged to another day of a series that keeps its weekdays moves alone, without the question) |
| An item that came from someone else's invitation warns before it is changed here | Done (drag, move, resize, edit, delete, and the form's Save) |
| Deleting a meeting the owner organizes offers to send a cancellation; deleting an invitation offers to decline | Done: a cancellation with a note (Microsoft 365 has no deleting without telling the attendees); an invitation can be deleted quietly, declined with word to the organizer, or declined without |
| Overlapping items sit side by side; all-day and multi-day items in the band at the top; private items show a lock; tentative, free, out-of-office and working-elsewhere each have their own look | Done (tentative hatched, out of office purple, an item takes its first category's colour, and the edge down its left is always its calendar's) |
| An item of high importance shows a red mark, one of low importance a blue arrow down | Done, among the item's marks at its right (our own drawings); an online meeting has a camera there |

## The appointment and meeting windows

| Outlook does | MailCraft |
|---|---|
| Opens in its own window with its own ribbon: Save & Close, Delete, Show As, Reminder, Recurrence, Time Zones, Categorize, Private, importance | Done: new and existing items open in windows of their own, several at once, each with its own command bar (Save & Close or Send / Send Update, Delete or Cancel Meeting, Forward, Show As, Reminder, Recurrence, Categorize, Private, High Importance, Low Importance, Invite Attendees, and on a meeting Teams Meeting, Response Options, Check Names), Ctrl+S, Ctrl+Enter, and a question before closing with unsaved changes. One row, no tabs; Time Zones is in the row. What doesn't fit goes behind `…`: on a meeting, Low Importance and Response Options first |
| Subject, location, start and end with date and time pickers, All day | Done |
| Recurrence dialog: daily, weekly on chosen days, monthly by date or by "second Tuesday", yearly; no end, after n, by date | Done as a dialog over the window; the form says the rule in one line. A window opened for one occurrence shows that day and saves to it alone |
| Reminder choices from "none" and "0 minutes" to "2 weeks" | Done |
| Time zones for start and end | Done: **Time Zones** in the command bar shows a zone beside Start and beside End, each a list to type in (this computer's zone first, the ones used lately next, then one per zone Windows names, every other zone of the IANA database by typing; each with its offset now). Changing a zone keeps the clock time and moves the moment, as in Outlook; a time typed beside a zone is that zone's clock time. An item with zones of its own opens with them showing; an all-day item has none. In the grid everything is at its moment by the calendar's clock, and the tooltip and a meeting request say the item's own time too: `9:00 AM – 10:00 AM (Eastern Time)`; its window shows its zones. A series made in another zone repeats at its clock time there, across both zones' daylight-saving changes. Microsoft 365 keeps the zones (`start` and `end` said in them, read back from `originalStartTimeZone`, `originalEndTimeZone` and `recurrenceTimeZone`), and so does `.ics` (`TZID` with a `VTIMEZONE`) |
| Rich-text body, attachments, inline pictures | Later (plain text) |
| Meeting: required and optional attendees with names checked against contacts; Send | Done: on a Microsoft 365 calendar Send creates the meeting there and the server mails the invitations; "Don't Send" keeps it on this computer. Updates and cancellations are sent the same way, each after a question |
| Scheduling Assistant: everyone's free/busy on a grid; Room Finder | Done for one day: a row per person, busy / tentative / out of office / working elsewhere / no information, and the meeting's time as a band to move and stretch. Next: several days, AutoPick; Later: rooms |
| Teams meeting button | Done on a meeting in a Microsoft 365 calendar: Teams Meeting in the window's bar (and `calendar.teamsMeeting`); the service makes the meeting when it is sent, and the link to join by comes back with it. The window then shows the link as it is, with Join and Copy Link; the item's menu, the ribbon and the reminder window have Join. Only an `https` link is kept or opened, and only on the owner's click. Once the service has made a meeting online it stays so (the service's rule), which the button says. An account that can't make one (a personal one, a calendar that doesn't allow Teams) is told so before, where the service says which calendars can, and otherwise after: the meeting is made without, and that is said. Not offered for a calendar on this computer. Outlook's "Don't Host Online" setting and the dial-in details are not there |
| Tracking: who accepted, declined, tentatively accepted | Done, kept current with every download |
| Response options: request responses, allow new time proposals | Done: Response Options in a meeting's bar, both on unless switched off; kept in the mailbox, sent to Microsoft 365 and read back, and in iCalendar files (`RSVP`, `X-MICROSOFT-DISALLOW-COUNTER`). Somebody else's meeting has none: they are its organizer's to say. Whether other times may be proposed goes to the service with a new meeting; its reference doesn't list that among what can be changed afterwards, so a change to it on a meeting that is already there may be refused: the save is then sent once more without it, so everything else in it holds (and the people invited get one update, not two), that option alone is put back, and the owner is told in one line. The option stays usable: the service may take it |

## Invitations and replies

| Outlook does | MailCraft |
|---|---|
| A meeting request in the Inbox shows Accept, Tentative, Decline and Propose New Time, with a look at the day it falls on | Done: the bar under the header with Accept, Tentative, Decline (each with its three ways of answering), Propose New Time (when the organizer takes proposals) and Calendar…; when, where, who; what else is on at that time, and a look at that part of the day. Updates, cancellations, replies, meetings that are over and the owner's own each say what they are. Meeting mail has its mark in the message list |
| Accepting puts it on the calendar and answers the organizer (or not, as chosen) | Done from the message and from the calendar item, with Outlook's three choices: send with a note, send now, don't send. On Microsoft 365 the message is answered through the item the server put on the calendar (asked for if it isn't here yet). Moving an answered request out of the Inbox is an option, off by default |
| Updates and cancellations from the organizer change or remove the item | Done for what comes down from Microsoft 365; a cancellation in mail offers Remove from Calendar (it is not removed by itself) |
| Respond from the calendar item itself | Done |
| Propose New Time | Done, from the meeting request in the mail, the item's menu and the ribbon: a dialog with the proposed start and end, the owner's own calendar on that day with the proposal in it, a note, and "Tentative and Propose" / "Decline and Propose", which are what sends (no key does). It says who is written to. On Microsoft 365 the answer goes with the proposed time (the service refuses one the organizer doesn't allow, which is put back and said); for an account whose calendar is kept here it goes from the Outbox as an iCalendar counter-proposal. Outlook opens the Scheduling Assistant's grid for this; here it is the one day |
| The organizer sees a proposal: Accept Proposal, View All Proposals | Done: a message that proposes another time says what is proposed and offers Accept Proposal, which opens the meeting with that time in its fields, to be sent as an update through the usual question (nothing is changed or sent by opening it); View All Proposals when several are known, as a list (Microsoft 365 says who proposed what with the meeting). The meeting's own window says how many proposed. Outlook shows the proposals as rows of the scheduling grid |
| Forward a meeting or an appointment | Done, from the item's menu, the ribbon and the window's bar: a small dialog with who it goes to and a note, which says who Send writes to and sends from Send alone. Microsoft 365 forwards it (and tells the organizer when the owner is an invitee, as the service does); an item on this computer goes as mail from the Outbox with the item as an iCalendar file. One occurrence of a series can be forwarded by itself. Outlook opens a message window for this; attachments can't be added here |
| Reply to all attendees with mail | Next |

## Reminders

| Outlook does | MailCraft |
|---|---|
| A reminders window of its own, listing what is due, with Dismiss, Dismiss All and Snooze for a chosen time | Done |
| Double-click opens the item | Done: in the item's own window; of an item that repeats, the occurrence the reminder is for, without asking |
| Join Online on the reminder of an online meeting | Done: Join beside Dismiss, for a meeting that has a link (shown when the pointer rests on it) |
| Sound, and a notification while the program is in the background | Later (the taskbar asks for attention) |

## Staying in step with Microsoft 365

| Outlook does | MailCraft |
|---|---|
| Shows what is on the server, within moments of a change made elsewhere | Done: every half minute while the calendar is in front, at once when the window comes back, the calendar is switched to, other days come into view or something is changed here; every five minutes otherwise |
| Everything made or changed here is on the server at once | Done, meetings and series included: a refusal is put back and said, an unreachable server is tried again. Nothing that mails other people goes without the owner's yes. Undo doesn't reach a change the server already has |
| A change made here is in Outlook, or the owner knows it is not | Done: the server's "yes" is not taken for the change having arrived. Every change it says it took is looked for in what comes down next (asked for again within seconds); what the server doesn't show after three looks, or shows otherwise than it was sent, is put back to what the server has, and the owner is told which item and what of it (status bar and Send/Receive problems). A deletion the server answers "not found" is looked for in the same way: that is its answer for an item that is gone already and for one it was asked for at the wrong address, so the item is gone when what comes down no longer has it (nothing is said), and back with a line when it still does. An appointment that is taken back while it is being made on the server is deleted there through the calendar it was made in. A waiting change that can't be sent any more is never dropped without a word either. An answer that is older than the change it follows (a view asked for right after a write) changes nothing here |
| What is only on this computer doesn't pass for the Outlook calendar | Done: a calendar of the mailbox's own that holds items beside the account's says "Only on this computer: not in your Outlook calendar" on its block and in the list, a line under the header counts the items and offers "Move to My Account's Calendar" (appointments move, meetings ask about their invitations, a copy of what the account's calendar has already just goes, an emptied calendar goes away), and a change to such an item says in the status bar that it stays here |
| (none: Outlook is the server's own program) | File › Info › Check Calendar Connection: on the real account, after a question that says exactly what it does, one test appointment a year ahead is made, read back, changed twice (once as an instant, once said in this computer's time zone), read back each time and deleted; the report has the server's status and words for each step, and what was sent beside what the server has |
| An item of another calendar than the main one, or of one that somebody shares | Done: written through its calendar (`/me/calendars/{id}/events/{id}`), which is the only way the service knows an item of a shared calendar by |
| Recurring series as series (not only as their occurrences), with exceptions | Done both ways for the rules the mailbox can express; "first weekday of the month" and the like still come down as loose occurrences |
| Categories with the mailbox's own names and colours | Done on items both ways; the mailbox's own list and colours come once the owner allows reading mailbox settings (asked for at the next Connect) |
| Every calendar and calendar group of the account; shared calendars; other people's calendars | Done for the account's own calendars and groups (see the list row above): only the account's main calendar is ticked when the list first comes, the rest wait to be ticked, and what the owner ticks stays. Each calendar has its owner (address and name) and rights (free/busy only, read, edit) as the server says, read again at every listing; a calendar that isn't edit refuses new items, changes and deletes with the reason. Calendars the server lists for somebody else's are under Shared Calendars. A shared mailbox's calendars are listed under My Calendars with its address, unticked, and read once ticked (#70): the owner is asked for `Calendars.ReadWrite.Shared` by itself the first time, and File › Info shows it; a calendar shared for free and busy times only is drawn as blocks (Busy, Tentative, Out of Office, Working Elsewhere) with no subject, read only; none raises a reminder. Where the owner has edit rights on one of a shared mailbox's calendars it is changed as his own is: new items, changes and deletes go to `/users/{address}/…` with his token (never `/me`), a meeting is called off or forwarded from the mailbox, and a meeting request that arrives in the shared mailbox's Inbox is accepted, tentatively accepted, declined or answered with another time proposed there, the answer going out from the mailbox's address (what makes the server send mail from it asks for `Mail.Send.Shared` by itself, with the question the shared mailbox's mail asks, the first time; until it is given the answer waits here). A calendar the owner may only read, or sees only as free and busy times, refuses changes with the reason. A change the server refuses is put back (an item made is kept on this computer only) and the window says why; if the rights shrink at a listing, changes not yet sent are held, not dropped, and the owner is told, and they go if the rights come back. An item doesn't move between the owner's own calendar and a shared mailbox's (that would cross two servers: refused, as moving out of any account's server is); within one shared mailbox it moves as within the owner's own account. Open Shared Calendar (File › Open & Export, Home › Manage Calendars, #70) reads a colleague's calendar by address through `/users/{address}` with the owner's Microsoft work or school sign-in, with the same one permission (`Calendars.ReadWrite.Shared`, asked for by itself, once, for shared mailboxes' calendars and opened ones alike; for an account that only opens calendars, without the shared mailbox mail permissions): kept on this computer only, under Shared Calendars with its own tick, colour and rights; it is read and changed by the same code as a shared mailbox's calendar (free and busy blocks with no subject when nothing more is shared, edit rights taking changes at `/users/{address}/events` and never `/me`, a refused change put back, rights that shrink holding changes); at most 20, removed by right-click; a refusal adds nothing and says why. When the server also lists a shared mailbox's calendar among the account's own (an invitation accepted in Outlook puts it under Shared Calendars), the window offers once to hide that copy (`calendar.hideListedCopy`; the answer, Hide the Copy or Keep Both, is kept in the mailbox file): a hidden copy is neither read from the server nor shown, the shared mailbox's own calendar staying, and right-clicking the shared mailbox's calendar shows it again (the copy's own menu hides it). |
| Free/busy of other people | Done, in the Scheduling Assistant (asked for only while it is in view) |
| Contacts in People, both ways | Done one way (down); Later: both ways |

## Waves

1. **Done 2026-10-12**: slots and typing, the current calendar, changes written to Microsoft 365,
   the invitation warning, a calendar that keeps itself current, own windows.
2. **Done 2026-10-12**: recurring series both ways with the occurrence-or-series question; resize by
   the edges, copy by Ctrl+drag, keyboard in the grid, Month and all-day dragging, the navigator,
   time scales, week numbers; every calendar of the account with its colour; categories; meetings
   for real (invitations, updates and cancellations through Microsoft 365, each after a question)
   and answering from the calendar; several appointment windows.
3. **Done 2026-10-12**: the reading pane's Accept / Tentative / Decline on a meeting request; the
   calendar list by group, side by side and overlaid; the Scheduling Assistant; the appointment
   window's own command bar, its unsaved-changes question and the recurrence dialog; working hours
   from the mailbox.
4. **Done 2026-10-12** (M3.4): Teams meetings, importance and response options; Propose New
   Time and, for the organizer, the proposals with Accept Proposal; forwarding a meeting; moving
   appointments between the calendars of an account. **Next** of this wave: the Scheduling
   Assistant over several days.
5. **Later**: rich bodies and attachments in items, rooms, shared calendars,
   Schedule view, notifications in the background.
6. **Done 2026-10-12** (M3.5, ahead of its wave): time zones. A zone database for every build;
   every stored time a moment; invitations and items from other zones at the right time here;
   zones of an item's own in its window, kept by Microsoft 365 and in `.ics`; a zone for the
   calendar and a second one in the time bar; a computer that travels.

## Known differences from Outlook in what is done

- Event ids are the service's ordinary ones, which change when an item is moved to another
  calendar in Outlook: a change that was waiting for such an item is answered "not found", the
  item goes here and the owner is told; the next download has it in its new calendar. (Asking
  for immutable ids would change every id the mailbox has kept so far.)

- A copy made by dragging or pasting is a plain appointment; Outlook keeps the attendees as an
  unsent meeting.
- Cut leaves the item where it is until it is pasted (deleting it first would cancel a meeting on
  the server).
- A repeating item cannot be changed between timed and all-day by dragging.
- Send in a new meeting's window sends at once, as in Outlook; Send Update and the keys for
  sending ask first, where Outlook does not.
- The questions about mail to attendees come up in the main window, not in the item's own.
- After declining on a Microsoft 365 account the item is gone from the server, so the answer can't
  be changed from the message.
- Time zones are called by shorter names than Outlook's: `Eastern Time` for what Windows calls
  Eastern Standard Time and Outlook lists as "(UTC-05:00) Eastern Time (US & Canada)"; a zone
  Windows has no name for goes by its place (`Kathmandu`). They are made from the identifiers the zones have (Windows' and the IANA database's), not from Outlook's list.
- The rules of the zones come with the program (the IANA database, built in), on every system
  and in the browser the same; Outlook takes them from Windows. A zone whose rules a government
  changes is right here after an update of MailCraft, not of Windows.
- An all-day item is a day, wherever the computer is. In Outlook it is the midnights of the zone
  it was made in, and shows across two days from another zone.
- An item whose zone has the same clock as the calendar's at its time is kept without a zone of
  its own (a series always keeps the zone it was made in). Opened again, its window shows no
  zones until Time Zones is pressed.
- A zone that an invitation names and nobody knows (and that the invitation doesn't define) is
  not guessed: the item is put at the offset the invitation gives, or else at its clock time
  here, and the request says so in a line of its own.
- A zone that Microsoft 365 has no name for (the service documents the names Windows has and
  about a hundred more) costs an item its zone on the server, never its moment: it is sent in
  UTC. A series by such a zone's clock is not sent by another clock; it is refused and said.
- The time bar names the calendar's zone over the hours once a zone is chosen for the calendar,
  a label is typed or a second zone is shown; Outlook shows a label only when one is typed.
- Enter in a one-line field of the window saves or sends; in Outlook it does not.
- None of the Microsoft 365 behaviour has been checked against the real service beyond the
  owner's own use; where the service turns out to differ, the change is refused and put back.
- An item that repeats is not taken to another calendar by dragging or by cut and paste (one
  day of a series can't be in another calendar than the rest): that is said, and Ctrl copies the
  occurrence there instead.
- The mailbox's working hours are shifted from its zone by what the two clocks differ today;
  hours that cross midnight that way are shown to the end of the day. Working days that are not
  next to one another (Monday,
  Wednesday, Friday) make a Work Week from the first of them to the last.
- An appointment is moved to another calendar of the account by making it there and deleting
  it where it was (the service has no move): the original goes only once the copy has been seen
  on the server, and the move is taken back and said if either half is refused. A meeting, a
  guest's item, a series, one day of a series and an item with attachments are not moved; each
  says why.
- The same holds between your own calendar and a shared mailbox's, or one you opened by address
  (#70), and between two of those, where you may edit both: the copy is made at the target's
  own address, and the original is deleted at its own only after the copy has been seen. A
  meeting is not moved to another mailbox's calendar (it would send everyone invited a
  cancellation and break their replies): forward it or create it again there. A series and an
  item with files are refused as within one account. If the copy is refused, the item stays
  where it was; if the original then can't be deleted, the item is in both calendars and a line
  says so (the copy is not taken out of somebody else's mailbox behind their back). One Undo
  puts a move back while nothing has been sent; once the copy is made, the move is not an Undo
  step any more (as for every change the server has been told), and moving the item back does it.
- A Teams meeting the server has made can't be turned back into a plain one (the service doesn't
  allow it); an account that can't make Teams meetings says so, and the meeting is made without.
- Whether proposals of another time are allowed is told the server when a meeting is made; the
  documented service doesn't take a change of it afterwards. When the server refuses it, the
  rest of what was saved stands, that one option is as it was, and a line says so.
- Propose New Time shows the owner's own day beside the new time, not the other attendees' free
  and busy times.
- Forwarding a meeting takes the people and a note; there is no message to write beyond it.
- Turning Request Responses off doesn't change what Accept, Tentative and Decline offer the guest.
- The join link is named by the provider the server names, not by where the link leads; the
  link's real destination is shown beside it, and only an `https` link is opened.
- A calendar's tab has our own arrow and cross; the Overlay and Side-by-Side buttons borrow
  icons the ribbon had already. Outlook opens a calendar in a window of its own on request;
  that is not offered.

## Proven and not proven

Everything in "Staying in step" is tested against a stand-in that takes only what the documented
Microsoft Graph v1.0 service takes (`crates/net/src/graph/standin.rs`: addresses, headers, bodies,
ids with `=`, `/` and `+`, a view that lags behind a change). What only the real service can say
is what the owner's Check Calendar Connection shows: whether a change of time is taken as it is
sent, and whether a time said in this computer's zone means the same instant there.

Time zones: every conversion is tested around the changes of the clock in both hemispheres, in
zones half and three quarters of an hour off, in zones whose rules changed, far in the past and
the future, and with nonsense (names nobody knows, a megabyte of `TZID`, a `VTIMEZONE` that
contradicts its name, offsets of days); `.ics` and Graph bodies round-trip under property tests.
The stand-in takes the zone names the Graph reference lists and no others, keeps the zones a
start and an end were told in, and repeats a series by its zone's clock from rules written out
on its own (checked against the program's tables for every half hour of two years). The flows in
`apps/mailcraft/src/calendar_flow.rs` run an item made in another zone, series across both
zones' changes of the clock, and a mailbox written before zones were kept that is opened and
carried to Berlin and to Kiritimati and home: not one write reaches the server. Not proven:
which names beyond the documented ones the real service takes, and what it gives back as an
item's original zone when another program made the item.

## Revision history

| Date | Change |
|---|---|
| 2026-10-24 | #70: a shared mailbox's calendar listed twice (also under Shared Calendars) is offered once to be hidden, `calendar.hideListedCopy` |
| 2026-10-24 | #70 calendars 5: an appointment moves between your calendar and a shared mailbox's or one opened by address, copy first and the original deleted once the copy is seen; a meeting stays |
| 2026-10-24 | #70 calendars 4: Open Shared Calendar by address, `calendar.openShared` and `calendar.removeShared` |
| 2026-10-12 | First version, when the owner made the calendar the top priority |
| 2026-10-12 | After a change of time made here didn't show in the owner's Outlook: sent changes are checked against what comes down, nothing is dropped silently, items that are only on this computer say so and can be moved, events of other calendars go through their calendar, and the owner can check the connection on the real account |
| 2026-10-12 | The first wave is in: rows marked Done, and rows corrected where the code does something else than the list said (server series, deleting meetings, the calendar list) |
| 2026-10-12 | The third wave is in: meeting requests in the reading pane, the appointment window's command bar, unsaved-changes question and recurrence dialog, the Scheduling Assistant, calendars by group, side by side and overlaid, working hours from the mailbox |
| 2026-10-12 | The second wave is in: the grid (resize, copy, keys, navigator, time scales), series as series with the question, meetings and answers through Microsoft 365, every calendar, categories, several appointment windows; a list of known differences |
| 2026-10-12 | Of the third wave: the calendar list by group, calendars side by side and overlaid, working hours from the mailbox, a reminder opening its occurrence; calendars other than the account's own arrive unticked |
| 2026-10-12 | The fourth wave is in (M3.4): Teams meetings, importance, response options, Propose New Time, forwarding, moving appointments between calendars; the known differences that came with them |
| 2026-10-12 | Time zones (M3.5): the calendar's zone and a second one, zones of an item's own in its window, on Microsoft 365 and in `.ics`, working hours in the mailbox's zone, a computer that travels; the known differences that came with them |
| 2026-10-24 | #70 calendars 2: a shared mailbox's calendars are read at its own address, with free/busy-only ones as blocks; the permission is asked for by itself; no reminders from them |
| 2026-10-24 | #70 calendars 3: a shared mailbox's calendars are written to where the owner may edit, its meeting requests answered from its address, refused changes put back, shrunken rights hold what waits |
| 2026-10-24 | #70 calendars 1: a calendar has an owner and rights (free/busy only, read, edit) in place of read-only, an older file's read-only reads as read or edit; calendars the server lists for other people are under Shared Calendars |
| 2026-10-19 | The calendar's View tab has the four views in Arrangement, and no longer the messages' sorting and preview (#239) |
