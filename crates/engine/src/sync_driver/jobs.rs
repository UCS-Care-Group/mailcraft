// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The kinds of background job a front end runs, and what may start while another is at work,
//! with no I/O: the order of the jobs is a rule of the program, not of a window. A front end says
//! what is at work now ([`Busy`], from the handles of its jobs) and asks whether the job it is
//! about to start has to wait ([`Busy::blocks`]).
//!
//! The rule behind every row: one job at a time that renews the same sign-in, and anything that
//! sends or asks the owner for something goes first. A new kind of job is added to [`JobKind`];
//! the front end's replacement test has a row for each.

macro_rules! job_kinds {
    ($($(#[$doc:meta])* $kind:ident),* $(,)?) => {
        /// Every kind of background job a front end starts.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum JobKind {
            $($(#[$doc])* $kind),*
        }

        impl JobKind {
            /// Every kind, each once.
            pub const ALL: &[JobKind] = &[$(JobKind::$kind),*];
        }
    };
}

job_kinds! {
    /// An IMAP (and SMTP) account's Send/Receive.
    Imap,
    /// A Send/Receive through Microsoft Graph: an account kept in step through Graph, or the
    /// sending that goes before an IMAP account's mail.
    Graph,
    /// An Exchange server account's Send/Receive.
    Ews,
    /// A JMAP account's Send/Receive.
    Jmap,
    /// An Inbox pass, a round's or the Inbox checked by itself, and the checks that gave way.
    Inbox,
    /// A shared mailbox's Send/Receive, read with its owner's sign-in.
    Shared,
    /// The calendar and contacts kept current between runs.
    Calendar,
    /// A check of the calendar connection.
    CalendarCheck,
    /// Other people's free and busy times.
    FreeBusy,
    /// The to-dos kept in step with Microsoft To Do.
    Tasks,
    /// The rules kept on the server.
    Rules,
    /// The automatic replies.
    AutoReplies,
    /// A sign-in on a provider's page.
    SignIn,
    /// What the Add Account dialog asks of the host (mail exchangers, the servers' check).
    AccountSetup,
    /// An administrator's approval on Microsoft's page.
    AdminConsent,
}

/// A job that is about to start, and has to know whether it may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// The calendar's job, by the clock.
    Calendar,
    /// A look at other days of the calendar, asked for by the owner or by a message.
    CalendarAround,
    /// A check of the calendar connection.
    CalendarCheck,
    /// The to-dos' job.
    Tasks,
    /// The rules' job.
    Rules,
    /// The automatic replies' job.
    AutoReplies,
    /// An Inbox pass by itself, between rounds.
    InboxCheck,
    /// A look at other people's free and busy times.
    FreeBusy,
}

/// What is at work now, as the front end's job handles say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Busy {
    /// The calendar's job is in flight.
    pub calendar: bool,
    /// The to-dos' job is in flight.
    pub tasks: bool,
    /// The rules' job is in flight.
    pub rules: bool,
    /// The automatic replies' job is in flight.
    pub auto_replies: bool,
    /// A free and busy look is in flight.
    pub free_busy: bool,
    /// The Microsoft Graph part of a Send/Receive is at work.
    pub graph: bool,
    /// An IMAP, Exchange or JMAP run is at work.
    pub mail_run: bool,
    /// A sign-in is open.
    pub signing_in: bool,
    /// A Send/Receive is waiting its turn.
    pub queued: bool,
    /// An Inbox pass is waiting its turn.
    pub inbox_queued: bool,
    /// An administrator's approval is being asked for.
    pub approving: bool,
    /// The Graph switch of an account is copying.
    pub switching: bool,
}

impl Busy {
    /// Whether `job` has to wait for something that is at work now.
    pub fn blocks(&self, job: Start) -> bool {
        let signing = self.signing_in;
        match job {
            // A Send/Receive that is about to ask Graph, or is asking, goes first: an answer
            // asked for before a change went up could otherwise be taken in after it.
            Start::Calendar => self.calendar || self.graph || self.mail_run || signing || self.queued,
            Start::CalendarAround => self.calendar || self.graph || signing || self.queued,
            Start::CalendarCheck => self.calendar || self.graph || signing,
            // The calendar's job, a Send/Receive and a sign-in go first.
            Start::Tasks => self.calendar || self.rules || self.graph || self.mail_run || signing || self.queued,
            // The calendar's job, the to-dos', the automatic replies', a Send/Receive and a
            // sign-in go first (the jobs renew the same sign-in).
            Start::Rules => self.calendar || self.tasks || self.auto_replies || self.graph || self.mail_run || signing || self.queued,
            // The calendar's, the to-dos' and the rules' jobs, a Send/Receive and a sign-in go
            // first; a Send/Receive that is only waiting its turn doesn't hold it back.
            Start::AutoReplies => self.calendar || self.tasks || self.rules || self.graph || self.mail_run || signing,
            Start::InboxCheck => self.mail_run || self.graph || self.queued || self.inbox_queued || signing || self.approving || self.switching,
            // One question at a time, and not while the account's sign-in or calendar is being seen to.
            Start::FreeBusy => self.free_busy || signing || self.graph || self.calendar,
        }
    }
}
