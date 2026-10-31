// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The sample mailbox (`--sample`, `?sample`, File › New › Sample): our own invented people,
//! mail, calendar and tasks, dated around "now" so every date group has something in it.

use mailcraft_model::{
    Action, Address, Attachment, Attendee, Body, Condition, Contact, Event, FlagStatus, FolderKind, Freq, Id, Importance, Labeled, Mailbox, Message,
    PostalAddress, Recurrence, Response, Rule, ShowAs, Signature, Task,
};
use mailcraft_time::{Date, DateTime, Timestamp, Weekday};

const ME: (&str, &str) = ("Avery Quinn", "avery@lumen.example");
const MAYA: (&str, &str) = ("Maya Okafor", "maya@lumen.example");
const TOMAS: (&str, &str) = ("Tomás Rivera", "tomas@lumen.example");
const PRIYA: (&str, &str) = ("Priya Nair", "priya@lumen.example");
const JONAS: (&str, &str) = ("Jonas Weber", "jonas@lumen.example");
const SOFIA: (&str, &str) = ("Sofia Marin", "sofia@lumen.example");
const DEV: (&str, &str) = ("Dev Patel", "dev.patel@example.org");
const LIN: (&str, &str) = ("Lin Zhao", "lin.zhao@harborworks.example");
const HANNAH: (&str, &str) = ("Hannah Brooks", "travel@lumen.example");
const BOOKS: (&str, &str) = ("Harbor Books", "news@harborbooks.example");
const BILLING: (&str, &str) = ("Cloudline Billing", "billing@cloudline.example");

fn addr(p: (&str, &str)) -> Address {
    Address::new(p.0, p.1)
}

struct Builder {
    mb: Mailbox,
    account: Id,
    today: Date,
    offset: i32,
    n: u64,
}

impl Builder {
    /// `days` ago (negative: ahead) at `hour:minute` local time.
    fn at(&self, days: i64, hour: u8, minute: u8) -> Timestamp {
        DateTime::new(self.today.add_days(-days), hour, minute, 0).unwrap_or_default().to_timestamp(self.offset)
    }

    fn local(&self, days_ahead: i64, hour: u8, minute: u8) -> DateTime {
        DateTime::new(self.today.add_days(days_ahead), hour, minute, 0).unwrap_or_default()
    }

    fn folder(&self, kind: FolderKind) -> Id {
        self.mb.folder_of_kind(self.account, kind).unwrap_or(0)
    }

    fn mail(&mut self, folder: Id, from: (&str, &str), to: &[(&str, &str)], subject: &str, when: Timestamp, text: &str) -> Id {
        self.n += 1;
        let m = Message {
            folder,
            message_id: format!("sample.{}@lumen.example", self.n),
            from: addr(from),
            to: to.iter().map(|p| addr(*p)).collect(),
            subject: subject.into(),
            date: when,
            received: when,
            body: Body { text: text.into(), html: None },
            read: true,
            ..Default::default()
        };
        self.mb.add_message(m).unwrap_or(0)
    }

    fn edit(&mut self, id: Id, f: impl FnOnce(&mut Message)) {
        if let Ok(m) = self.mb.message_mut(id) {
            f(m);
        }
    }
}

/// The sample mailbox as of `now`, in a zone `offset` minutes east of UTC.
pub fn mailbox(now: Timestamp, offset: i32) -> Mailbox {
    let mb = Mailbox::with_local_account(ME.0, ME.1);
    let account = mb.accounts.first().map_or(0, |a| a.id);
    let mut b = Builder { mb, account, today: DateTime::from_timestamp(now, offset).date, offset, n: 0 };
    let inbox = b.folder(FolderKind::Inbox);
    let sent = b.folder(FolderKind::Sent);
    let projects = b.mb.add_folder(account, Some(inbox), "Projects").unwrap_or(inbox);
    let atlas = b.mb.add_folder(account, Some(projects), "Atlas").unwrap_or(inbox);
    let receipts = b.mb.add_folder(account, None, "Receipts").unwrap_or(inbox);
    if let Ok(f) = b.mb.folder_mut(atlas) {
        f.favorite = true;
    }

    // ---- Inbox ----
    // Today's mail is dated in the small hours, so it is in the past whenever the sample opens.
    let id = b.mail(
        inbox,
        MAYA,
        &[ME, TOMAS],
        "Homepage mockups, round three",
        b.at(0, 0, 42),
        "Hi both,\n\nRound three is attached. The big change is the hero: one message, one button, and the customer quotes move below the fold.\n\nTwo things I'd like your eyes on:\n\n1. The pricing teaser. Is three tiers too much for the first screen?\n2. The dark variant. I think it reads better, Tomás thinks it hides the illustration.\n\nNotes for each screen are in the text file. If we can agree by Thursday I can hand off to build on Friday.\n\nMaya",
    );
    b.edit(id, |m| {
        m.read = false;
        m.categories = vec!["Blue category".into()];
        m.attachments = vec![Attachment::new(
            "homepage-round-3-notes.txt",
            "text/plain",
            b"Homepage, round three\n\nHero: single headline, single call to action.\nQuotes: moved under the feature grid.\nPricing teaser: three tiers, monthly prices only.\nDark variant: same layout, illustration brightened by ten percent.\n".to_vec(),
        )];
    });
    let id = b.mail(
        inbox,
        PRIYA,
        &[ME],
        "Q4 roadmap review moved to Thursday",
        b.at(0, 0, 25),
        "Avery,\n\nThe roadmap review is moving from Wednesday to Thursday at 2 pm so finance can join. Same room, same agenda.\n\nPlease bring the Atlas estimate, even if it is rough. A range is fine.\n\nThanks,\nPriya",
    );
    b.edit(id, |m| {
        m.read = false;
        m.importance = Importance::High;
    });
    let id = b.mail(
        inbox,
        BILLING,
        &[ME],
        "Your October invoice is ready",
        b.at(0, 0, 10),
        "Your invoice for October is ready.\n\nAmount due: 84.00\nDue date: the 28th\n\nThe invoice is attached as a spreadsheet. Nothing to do if you pay by card: we charge it on the due date.\n\nCloudline Billing",
    );
    b.edit(id, |m| {
        m.read = false;
        m.body.html = Some(
            "<h2>Your October invoice is ready</h2><p>Amount due: <b>84.00</b><br>Due date: the 28th</p><p>The invoice is attached as a spreadsheet. Nothing to do if you pay by card: we charge it on the due date.</p><hr><p><a href=\"https://cloudline.example/billing\">Open the billing page</a></p><p><i>Cloudline Billing</i></p>"
                .into(),
        );
        m.attachments = vec![Attachment::new("invoice-october.csv", "text/csv", b"item,quantity,unit price,amount\nTeam plan,7,12.00,84.00\n".to_vec())];
    });
    let checklist = "Draft checklist for the 4.2 release:\n\n- Freeze the main branch Tuesday noon\n- Run the full test pass on all three platforms\n- Update the screenshots in the help pages\n- Write the release notes (Avery)\n- Tag and publish Thursday morning\n\nAnything missing?\n\nTomás";
    b.mail(inbox, TOMAS, &[ME, MAYA], "Release checklist", b.at(4, 10, 15), checklist);
    let id = b.mail(
        inbox,
        TOMAS,
        &[ME, MAYA],
        "RE: Release checklist",
        b.at(1, 16, 20),
        "Good catch on the installer signing step. I added it after the test pass, and moved the screenshots to Wednesday so they show the final build.\n\nAvery, can you confirm the release notes will be ready by Wednesday evening?\n\nTomás",
    );
    b.edit(id, |m| {
        m.flag = FlagStatus::Flagged;
        m.in_reply_to = "sample.reply-1@lumen.example".into();
    });
    b.mail(
        inbox,
        JONAS,
        &[ME],
        "Expense reports are due Friday",
        b.at(1, 11, 5),
        "A reminder that September expense reports close on Friday.\n\nReceipts over 25.00 need a photo or a scan. Mileage goes on the separate form.\n\nJonas",
    );
    let id = b.mail(
        inbox,
        LIN,
        &[ME],
        "Feedback on the onboarding flow",
        b.at(2, 14, 48),
        "Hello Avery,\n\nWe rolled the new onboarding out to our support team last week, twelve people in all. Overall it went well. A few notes, in the order people hit them:\n\nThe invitation e-mail landed in junk for three people. Its subject line might be the reason.\n\nStep two asks for a team name before explaining what a team is. Two people stopped there and asked me.\n\nThe sample project is very helpful. Several people said it was the moment the product made sense.\n\nNobody found the keyboard shortcuts page on their own.\n\nHappy to get on a call if useful.\n\nBest,\nLin",
    );
    b.edit(id, |m| m.read = false);
    let id = b.mail(
        inbox,
        BOOKS,
        &[ME],
        "Five new arrivals for October",
        b.at(3, 8, 0),
        "New on our shelves this month\n\n• A field guide to coastal birds\n• Bread, slowly: a baking notebook\n• Maps of places that never were\n• The night ferry (a novel)\n• Letters from a lighthouse keeper\n\nMembers get ten percent off through the end of the month.\n\nHarbor Books, 12 Quay Street",
    );
    b.edit(id, |m| {
        m.body.html = Some(
            "<h1>New on our shelves this month</h1><ul><li>A field guide to coastal birds</li><li>Bread, slowly: a baking notebook</li><li>Maps of places that never were</li><li><i>The night ferry</i> (a novel)</li><li>Letters from a lighthouse keeper</li></ul><p>Members get <b>ten percent off</b> through the end of the month.</p><p><a href=\"https://harborbooks.example/october\">See them all</a></p><p>Harbor Books, 12 Quay Street</p>"
                .into(),
        );
    });
    let id = b.mail(
        inbox,
        SOFIA,
        &[ME],
        "Benefits enrollment opens Monday",
        b.at(8, 9, 30),
        "Hi everyone,\n\nOpen enrollment starts Monday and runs for two weeks. If you change nothing, your current choices carry over, except the flexible spending amount, which resets to zero.\n\nI'm holding two drop-in sessions next week for questions.\n\nSofia",
    );
    b.edit(id, |m| m.categories = vec!["Green category".into()]);
    let id = b.mail(
        inbox,
        HANNAH,
        &[ME],
        "Itinerary: Lisbon, November 3 to 6",
        b.at(9, 15, 12),
        "Avery,\n\nYour trip is booked.\n\nOut: Monday November 3, departs 07:40, arrives 11:05\nBack: Thursday November 6, departs 18:20, arrives 21:50\nHotel: three nights near the conference hall, breakfast included\n\nThe confirmation is attached. Tell me by the 20th if anything needs to change; after that the fare can't be altered.\n\nHannah",
    );
    b.edit(id, |m| {
        m.flag = FlagStatus::Flagged;
        m.attachments = vec![Attachment::new(
            "lisbon-confirmation.txt",
            "text/plain",
            b"Booking reference: LMN-4471\nTraveller: Avery Quinn\nOut: Nov 3, 07:40 to 11:05\nBack: Nov 6, 18:20 to 21:50\nHotel: 3 nights, breakfast included\n".to_vec(),
        )];
    });
    let id = b.mail(
        inbox,
        DEV,
        &[ME],
        "Lunch next week?",
        b.at(15, 12, 2),
        "Avery! It has been too long. I'm near your office Tuesday and Wednesday next week. Lunch?\n\nDev",
    );
    b.edit(id, |m| m.answered = true);
    let id = b.mail(
        inbox,
        PRIYA,
        &[ME, MAYA, TOMAS],
        "Welcome to the Atlas project",
        b.at(24, 10, 0),
        "Team,\n\nAtlas is go. The short version: one place to see every customer project, its status and who owns it.\n\nPlan of record: design through October, build in November, pilot with two customers in December.\n\nOur notes and decisions live in the Atlas folder. If it isn't written there, it isn't decided.\n\nPriya",
    );
    b.edit(id, |m| m.pinned = true);
    b.mail(
        inbox,
        SOFIA,
        &[ME],
        "Photos from the summer offsite",
        b.at(40, 17, 45),
        "The photographer sent the full set from the offsite. There are about two hundred; I put my favourite thirty in the shared album.\n\nIf there is one of you that you would rather not have shared, tell me and it goes.\n\nSofia",
    );

    // ---- Projects/Atlas, Receipts ----
    b.mail(
        atlas,
        MAYA,
        &[ME, PRIYA],
        "Atlas: first sketches",
        b.at(20, 13, 30),
        "Three directions for the project overview, from densest to calmest: a table, cards, and a timeline.\n\nMy pick is cards with a compact toggle. Tables win for people with forty projects, and we have two of those customers.\n\nMaya",
    );
    b.mail(
        atlas,
        TOMAS,
        &[ME, PRIYA],
        "Atlas: where the data comes from",
        b.at(18, 9, 10),
        "I traced where project status lives today: in three places, and they disagree about a fifth of the time.\n\nProposal: Atlas reads from the planning tool only, and we fix the other two to follow it. Estimate attached to the planning doc.\n\nTomás",
    );
    b.mail(
        receipts,
        ("Quay Street Coffee", "receipts@quaycoffee.example"),
        &[ME],
        "Your receipt",
        b.at(6, 8, 14),
        "Thanks for stopping by.\n\nFlat white  3.80\nOat bar     2.50\nTotal       6.30\n\nPaid by card ending 4410.",
    );
    b.mail(
        receipts,
        ("Paperworks", "orders@paperworks.example"),
        &[ME],
        "Order 20417 has shipped",
        b.at(12, 16, 40),
        "Your order is on its way.\n\n2 x dotted notebook, A5\n1 x fine liner set\n\nIt should arrive within three working days.",
    );

    // ---- Sent, Drafts, Archive, Junk, Deleted ----
    let id = b.mail(sent, ME, &[TOMAS, MAYA], "RE: Release checklist", b.at(2, 9, 5), &format!("One thing missing: signing the installers. Last time it cost us half a day.\n\nAvery\n\n-----Original Message-----\nFrom: Tomás Rivera <tomas@lumen.example>\nSubject: Release checklist\n\n{checklist}\n"));
    b.edit(id, |m| m.message_id = "sample.reply-1@lumen.example".into());
    b.mail(sent, ME, &[DEV], "RE: Lunch next week?", b.at(14, 9, 40), "Tuesday works. Noon at the place by the canal?\n\nAvery");
    b.mail(sent, ME, &[HANNAH], "Lisbon conference travel", b.at(11, 10, 20), "Hi Hannah,\n\nCould you book travel for the Lisbon conference, November 3 to 6? Morning flight out if there is one, and a hotel within walking distance of the hall.\n\nThanks,\nAvery");
    let drafts = b.folder(FolderKind::Drafts);
    let id = b.mail(
        drafts,
        ME,
        &[LIN],
        "RE: Feedback on the onboarding flow",
        b.at(1, 17, 30),
        "Hi Lin,\n\nThank you, this is exactly the kind of detail we need. On the invitation e-mail: ",
    );
    b.edit(id, |m| m.draft = true);
    let archive = b.folder(FolderKind::Archive);
    b.mail(archive, JONAS, &[ME], "Budget for next year: first pass", b.at(52, 11, 0), "First pass at next year's numbers is in the shared sheet. Travel is flat, tooling is up by a tenth.\n\nComments by the end of the month, please.\n\nJonas");
    b.mail(archive, PRIYA, &[ME], "Notes from the customer council", b.at(66, 15, 20), "Three themes came up again and again: onboarding, reporting, and a way to see everything in one place.\n\nThat last one is what became Atlas.\n\nPriya");
    let junk = b.folder(FolderKind::Junk);
    let id = b.mail(
        junk,
        ("Prize Desk", "winner@prizes.invalid"),
        &[ME],
        "You are today's lucky visitor",
        b.at(1, 3, 33),
        "Congratulations! You have been selected. To claim your prize, reply with your details within 24 hours.",
    );
    b.edit(id, |m| m.read = false);
    let deleted = b.folder(FolderKind::Deleted);
    b.mail(
        deleted,
        ("Lumen Facilities", "facilities@lumen.example"),
        &[ME],
        "Lift maintenance on Tuesday",
        b.at(5, 7, 50),
        "The east lift is out of service Tuesday morning from 8 to 10. The west lift and the stairs are open as usual.",
    );

    people(&mut b);
    calendar(&mut b);
    tasks(&mut b);

    b.mb.signatures.push(Signature::plain_text("Standard", "Avery Quinn\nLumen Studio"));
    if let Ok(a) = b.mb.account_mut(account) {
        a.signature = "Standard".into();
    }
    let rule = b.mb.alloc_id();
    b.mb.rules.push(Rule {
        id: rule,
        name: "Bookshop newsletters".into(),
        enabled: true,
        conditions: vec![Condition::FromContains("harborbooks".into())],
        actions: vec![Action::Categorize("Purple category".into())],
        stop: false,
        account,
        ..Default::default()
    });
    b.mb
}

fn people(b: &mut Builder) {
    let person = |p: (&str, &str), title: &str, company: &str, phone: &str| {
        let (first, last) = p.0.rsplit_once(' ').unwrap_or((p.0, ""));
        Contact {
            first: first.into(),
            last: last.into(),
            title: title.into(),
            company: company.into(),
            emails: vec![Labeled::new("work", p.1)],
            phones: if phone.is_empty() { Vec::new() } else { vec![Labeled::new("mobile", phone)] },
            ..Default::default()
        }
    };
    let mut maya = person(MAYA, "Design Lead", "Lumen Studio", "+1 555 0101");
    maya.favorite = true;
    maya.notes = "Prefers feedback as comments on the file, not in mail.".into();
    b.mb.add_contact(maya);
    let mut tomas = person(TOMAS, "Engineering Manager", "Lumen Studio", "+1 555 0102");
    tomas.favorite = true;
    b.mb.add_contact(tomas);
    let mut priya = person(PRIYA, "Head of Product", "Lumen Studio", "+1 555 0103");
    priya.favorite = true;
    priya.categories = vec!["Blue category".into()];
    b.mb.add_contact(priya);
    b.mb.add_contact(person(JONAS, "Finance", "Lumen Studio", "+1 555 0104"));
    b.mb.add_contact(person(SOFIA, "People Operations", "Lumen Studio", ""));
    let mut dev = person(DEV, "Architect", "Patel & Rowe", "+1 555 0142");
    dev.birthday = Date::new(1988, 3, 14);
    dev.emails.push(Labeled::new("home", "dev@patel.example"));
    b.mb.add_contact(dev);
    let mut lin = person(LIN, "Support Director", "Harborworks", "+1 555 0177");
    lin.addresses = vec![PostalAddress {
        label: "work".into(),
        street: "40 Dock Road".into(),
        city: "Port Haven".into(),
        region: "".into(),
        postal_code: "20417".into(),
        country: "".into(),
    }];
    lin.websites = vec!["https://harborworks.example".into()];
    lin.categories = vec!["Orange category".into()];
    b.mb.add_contact(lin);
    b.mb.add_contact(person(HANNAH, "Travel Desk", "Lumen Studio", "+1 555 0110"));
    b.mb.add_contact(Contact {
        company: "Harbor Books".into(),
        emails: vec![Labeled::new("work", BOOKS.1)],
        websites: vec!["https://harborbooks.example".into()],
        ..Default::default()
    });
}

fn calendar(b: &mut Builder) {
    let attendee = |p: (&str, &str), response: Response| Attendee { address: addr(p), optional: false, response };
    let me = Some(addr(ME));
    let weekdays = vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri];
    // A month back, so the series is already running.
    let _ = b.mb.add_event(Event {
        title: "Team stand-up".into(),
        location: "Studio corner".into(),
        start: b.local(-28, 9, 30),
        end: b.local(-28, 9, 45),
        recurrence: Some(Recurrence { by_day: weekdays, ..Recurrence::every(Freq::Daily) }),
        reminder_minutes: Some(5),
        categories: vec!["Blue category".into()],
        ..Default::default()
    });
    let tuesday = b.today.start_of_week(Weekday::Mon).add_days(1).days_until(b.today);
    let _ = b.mb.add_event(Event {
        title: "1:1 with Priya".into(),
        location: "Priya's office".into(),
        start: b.local(-tuesday - 21, 14, 0),
        end: b.local(-tuesday - 21, 14, 30),
        organizer: Some(addr(PRIYA)),
        attendees: vec![attendee(ME, Response::Accepted)],
        recurrence: Some(Recurrence::every(Freq::Weekly)),
        reminder_minutes: Some(15),
        ..Default::default()
    });
    let _ = b.mb.add_event(Event {
        title: "Design review: homepage".into(),
        location: "Room Birch".into(),
        description: "Round three of the homepage. Decide on the pricing teaser and the dark variant.".into(),
        start: b.local(0, 11, 0),
        end: b.local(0, 12, 0),
        organizer: me,
        attendees: vec![attendee(MAYA, Response::Accepted), attendee(TOMAS, Response::Tentative), attendee(PRIYA, Response::None)],
        reminder_minutes: Some(15),
        categories: vec!["Blue category".into()],
        ..Default::default()
    });
    let _ = b.mb.add_event(Event {
        title: "Lunch with Dev".into(),
        location: "Canal Kitchen".into(),
        start: b.local(1, 12, 30),
        end: b.local(1, 13, 30),
        show_as: ShowAs::OutOfOffice,
        reminder_minutes: Some(30),
        ..Default::default()
    });
    let _ = b.mb.add_event(Event {
        title: "Q4 roadmap review".into(),
        location: "Room Oak".into(),
        start: b.local(2, 14, 0),
        end: b.local(2, 15, 30),
        organizer: Some(addr(PRIYA)),
        attendees: vec![attendee(ME, Response::Accepted), attendee(JONAS, Response::Accepted), attendee(TOMAS, Response::Accepted)],
        reminder_minutes: Some(15),
        categories: vec!["Red category".into()],
        ..Default::default()
    });
    let _ = b.mb.add_event(Event {
        title: "Dentist".into(),
        start: b.local(3, 8, 30),
        end: b.local(3, 9, 15),
        show_as: ShowAs::OutOfOffice,
        private: true,
        reminder_minutes: Some(60),
        ..Default::default()
    });
    let _ = b.mb.add_event(Event {
        title: "Write release notes".into(),
        start: b.local(-1, 15, 0),
        end: b.local(-1, 17, 0),
        show_as: ShowAs::Tentative,
        ..Default::default()
    });
    // Two whole days next week.
    let next_monday = b.today.start_of_week(Weekday::Mon).add_days(7);
    let _ = b.mb.add_event(Event {
        title: "Planning days".into(),
        location: "Boathouse".into(),
        all_day: true,
        start: DateTime::at_midnight(next_monday.add_days(2)),
        end: DateTime::at_midnight(next_monday.add_days(4)),
        show_as: ShowAs::Busy,
        categories: vec!["Green category".into()],
        ..Default::default()
    });
    let _ = b.mb.add_event(Event {
        title: "Budget sync".into(),
        location: "Room Oak".into(),
        start: DateTime { hour: 10, ..DateTime::at_midnight(b.today.add_months(-2).start_of_month().add_days(13)) },
        end: DateTime { hour: 11, ..DateTime::at_midnight(b.today.add_months(-2).start_of_month().add_days(13)) },
        organizer: Some(addr(JONAS)),
        attendees: vec![attendee(ME, Response::Accepted)],
        recurrence: Some(Recurrence { by_day: vec![Weekday::Tue], nth: Some(2), ..Recurrence::every(Freq::Monthly) }),
        ..Default::default()
    });
    let birthday = Date::new(b.today.year - 1, 3, 14).unwrap_or(b.today);
    let _ = b.mb.add_event(Event {
        title: "Dev's birthday".into(),
        all_day: true,
        start: DateTime::at_midnight(birthday),
        end: DateTime::at_midnight(birthday.add_days(1)),
        show_as: ShowAs::Free,
        recurrence: Some(Recurrence::every(Freq::Yearly)),
        ..Default::default()
    });
}

fn tasks(b: &mut Builder) {
    let today = b.today;
    b.mb.add_task(Task {
        title: "Review the Q4 roadmap draft".into(),
        due: Some(today),
        importance: Importance::High,
        notes: "Bring the Atlas estimate as a range.".into(),
        ..Default::default()
    });
    b.mb.add_task(Task { title: "Submit the September expense report".into(), due: Some(today.add_days(-1)), ..Default::default() });
    b.mb.add_task(Task {
        title: "Write the 4.2 release notes".into(),
        due: Some(today.add_days(2)),
        categories: vec!["Blue category".into()],
        ..Default::default()
    });
    b.mb.add_task(Task { title: "Reply to Lin about onboarding".into(), due: Some(today.add_days(1)), ..Default::default() });
    b.mb.add_task(Task { title: "Read up on Lisbon conference sessions".into(), ..Default::default() });
    b.mb.add_task(Task {
        title: "Renew the studio domain".into(),
        due: Some(today.add_days(-6)),
        completed: Some(b.at(7, 10, 0)),
        importance: Importance::Low,
        ..Default::default()
    });
}
