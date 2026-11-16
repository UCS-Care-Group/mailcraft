// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Graph run itself (native only).

use mailcraft_model::sync::Tokens;

use super::{GraphRequest, GraphResult};

/// How far back and ahead of today the calendar is brought down (seconds).
const CALENDAR_BEHIND: i64 = 31 * 86_400;
const CALENDAR_AHEAD: i64 = 366 * 86_400;

/// This machine's time zone by a name Microsoft Graph knows, for an upload that doesn't name
/// the zone the calendar is shown in (the engine names it whenever it has a name: this is what
/// is left for a zone it can't name); empty when the system doesn't say either. The name is the
/// one Windows has for the zone (`Pacific Standard Time`), which is the one Graph prefers, else
/// the one of the IANA database (`Europe/Berlin`).
pub fn local_zone() -> String {
    let zone = mailcraft_time::Zone::system();
    zone.windows_name().map_or_else(|| zone.id(), str::to_string)
}

/// What the server made of one calendar change, for the log: the kind of change, a tag made of
/// the event's id (the same in every line about that event, and nothing of the id can be read
/// from it), which of the event's properties were sent (their names, never what they say), the
/// zone its times were said in, the calendar, and the server's answer. Nothing of the event.
fn log_outcome(outcome: &mailcraft_model::remote::EventOutcome) {
    use mailcraft_model::remote::{EventResult, id_tag};
    let op = &outcome.op;
    // What the server calls it: for a new event, what it is called from now on.
    let id = [op.remote.as_str(), outcome.remote.as_str()].into_iter().find(|id| !id.is_empty());
    let tag = id.map_or("an event without a name yet".to_string(), |id| format!("#{}", id_tag(id)));
    let one = if op.of_occurrence() { format!(" (one occurrence of the series #{})", id_tag(&op.series)) } else { String::new() };
    let names = op.fields.names();
    let sent = if names.is_empty() { String::new() } else { format!(", sent [{}]", names.join(", ")) };
    let zone = if outcome.zone.is_empty() { String::new() } else { format!(" with times said in {}", outcome.zone) };
    let calendar = if op.calendar.is_empty() { "the main calendar".to_string() } else { format!("calendar #{}", id_tag(&op.calendar)) };
    let status = if outcome.status == 0 { "no answer".to_string() } else { format!("status {}", outcome.status) };
    match outcome.result {
        EventResult::Done => log::info!("calendar: {:?} {tag}{one}{sent}{zone}, in {calendar}: taken by the server ({status})", op.kind),
        result => {
            log::warn!(
                "calendar: {:?} {tag}{one}{sent}{zone}, in {calendar}: not taken by the server ({result:?}, {status}): {}",
                op.kind,
                outcome.error
            )
        }
    }
}

/// What Microsoft Graph does for an account before its mail is synchronised: send what waits in
/// the Outbox, and bring the calendar and the contacts down. Nothing here stops the mail: every
/// trouble is a line in the answer.
pub fn graph(request: GraphRequest) -> GraphResult {
    graph_in(request, &local_zone())
}

/// [`graph`], with the owner's time zone as `zone` (a name Microsoft Graph knows).
pub fn graph_in(request: GraphRequest, zone: &str) -> GraphResult {
    graph_in_with(request, zone, &mailcraft_net::oauth::refresh_graph_opened_calendars)
}

/// [`graph_in`], with how a token that covers other people's calendars is had as `renew_calendars`
/// (the tests' own, which asks nobody).
pub fn graph_in_with(
    request: GraphRequest,
    zone: &str,
    renew_calendars: &dyn Fn(&mailcraft_model::OAuth, &str) -> Result<mailcraft_net::oauth::GraphTokens, mailcraft_net::NetError>,
) -> GraphResult {
    use mailcraft_net::oauth;
    // A token for everything Graph is asked for, and for reading the Favorites of Outlook when
    // the window wants that too (it stops wanting it for the day once it was told no).
    // (An account kept in step through Graph has its token asked for with its mail, R3.)
    let mail = request.mail.is_some();
    // (A shared mailbox's, with what reading shared mailboxes takes: #70.)
    let shared = request.mail.as_ref().is_some_and(|mail| mail.shared);
    // (A shared mailbox's calendars, with what reading those takes: #70.)
    let shared_calendars = !mail && request.account.is_shared();
    let renew: Renew<'_> = &|settings, refresh, favorites| {
        if shared_calendars {
            oauth::refresh_graph_shared_calendars(settings, refresh)
        } else if shared {
            oauth::refresh_graph_shared(settings, refresh)
        } else if mail {
            oauth::refresh_graph_mail(settings, refresh)
        } else if favorites {
            oauth::refresh_graph_with_favorites(settings, refresh)
        } else {
            oauth::refresh_graph(settings, refresh)
        }
    };
    // (And for sending from a shared mailbox, when it has mail to send: #70.)
    let (request, opened) = opened_token(request, shared, renew_calendars);
    // (And a shared mailbox's calendar change that makes the server send mail from it.)
    let mut out = if shared_calendars {
        graph_sending_with(request, zone, renew, &oauth::refresh_graph_shared_calendars_send)
    } else {
        graph_sending_with(request, zone, renew, &oauth::refresh_graph_shared_send)
    };
    out.shared_calendars_consent_needed = opened.consent_needed;
    if out.tokens.is_none() {
        out.tokens = opened.tokens;
    }
    out
}

/// What [`opened_token`] found out.
#[derive(Default)]
struct OpenedToken {
    /// The token that was obtained, when one was: it covers other people's calendars.
    tokens: Option<Tokens>,
    /// The sign-in doesn't cover reading other people's calendars: the window asks the owner.
    consent_needed: bool,
}

/// A calendar job of an account with calendars opened by address (#70) needs a token that covers
/// reading other people's calendars (`Calendars.ReadWrite.Shared`); one that was handed in may
/// not, and a refusal of it would read as "no rights". So such a job gets its own, from the
/// refresh token, and when the sign-in doesn't cover it the job goes on without those calendars
/// (the account's own are read as ever) and says that the owner is to be asked. A job that is not
/// the calendar's (mail's, a shared mailbox's) never reads them, and is left as it is.
fn opened_token(
    mut request: GraphRequest,
    shared: bool,
    renew: &dyn Fn(&mailcraft_model::OAuth, &str) -> Result<mailcraft_net::oauth::GraphTokens, mailcraft_net::NetError>,
) -> (GraphRequest, OpenedToken) {
    let mut found = OpenedToken::default();
    if request.account.opened_calendars.is_empty() || request.mail.is_some() || shared {
        request.account.opened_calendars.clear();
        return (request, found);
    }
    if request.refresh.is_empty() {
        // (Nothing to renew with: what the token covers isn't known, so they are left for later.)
        request.account.opened_calendars.clear();
        return (request, found);
    }
    match renew(&request.oauth, &request.refresh) {
        Ok(got) => {
            request.access = got.tokens.access.clone();
            found.tokens = Some(got.tokens);
        }
        Err(e) => {
            found.consent_needed = e.is_consent_needed();
            log::info!("calendar: other people's calendars aren't read in this run (consent needed: {})", found.consent_needed);
            request.account.opened_calendars.clear();
        }
    }
    (request, found)
}

/// How a job gets a token for Microsoft Graph when the window handed it none: from the sign-in
/// settings and the refresh token, and whether one that is good for the Favorites is wanted.
pub type Renew<'a> = &'a dyn Fn(&mailcraft_model::OAuth, &str, bool) -> Result<mailcraft_net::oauth::GraphTokens, mailcraft_net::NetError>;

/// Why reading the Favorites of Outlook failed, in a few plain words for the folder pane.
fn favorites_words(error: &mailcraft_net::NetError) -> &'static str {
    match error {
        mailcraft_net::NetError::Busy { .. } => "Outlook is busy",
        error if error.is_unreachable() => "Couldn't reach Outlook",
        _ => "Couldn't read the list",
    }
}

/// [`graph_in`], with the token coming from `renew` when the request brings none (and, for a
/// shared mailbox's mail to send, from `renew` too).
#[cfg(any(test, feature = "test-support"))]
pub fn graph_with(request: GraphRequest, zone: &str, renew: Renew<'_>) -> GraphResult {
    graph_sending_with(request, zone, renew, &|settings, refresh| renew(settings, refresh, false))
}

/// How a shared mailbox's run that has mail to send gets a token that covers sending from it
/// (`Mail.Send.Shared`, #70) when the window handed it none.
pub type RenewSend<'a> = &'a dyn Fn(&mailcraft_model::OAuth, &str) -> Result<mailcraft_net::oauth::GraphTokens, mailcraft_net::NetError>;

/// [`graph_in`], with the token coming from `renew` when the request brings none, and from
/// `renew_send` for a shared mailbox's run that has mail to send.
pub fn graph_sending_with(mut request: GraphRequest, zone: &str, renew: Renew<'_>, renew_send: RenewSend<'_>) -> GraphResult {
    use mailcraft_model::remote::{FavoritesDownload, FavoritesOutcome};
    use mailcraft_net::{NetError, graph};
    let mut out = GraphResult { account: request.account.id, ..Default::default() };
    let mut access = request.access.clone();
    // A shared mailbox with mail to send (#70): the token covers sending from it, or nothing of
    // it is sent this run (it waits, and the window asks the owner when he asked for the run).
    // Its changes and drafts go all the same, with what reading it takes.
    let sending_shared = request.mail.as_ref().is_some_and(|mail| mail.shared && !mail.sends.is_empty());
    if access.is_empty() && sending_shared {
        match renew_send(&request.oauth, &request.refresh) {
            Ok(got) => {
                access = got.tokens.access.clone();
                out.tokens = Some(got.tokens);
            }
            Err(e) => {
                if e.is_consent_needed() {
                    log::info!("graph sending: sending from a shared mailbox isn't allowed yet; its mail waits for the owner's permission");
                    out.send_consent_needed = true;
                } else {
                    log::info!("graph sending: no token for sending from a shared mailbox ({e}); its mail waits");
                }
                if let Some(mail) = request.mail.as_mut() {
                    mail.sends.clear();
                }
            }
        }
    }
    // A shared mailbox's calendar changes that make the server send mail to other people (an
    // answer, a meeting called off or forwarded, a meeting with people invited): they go with a
    // token that covers sending from it (#70), or they wait for the owner's permission, and
    // everything else goes all the same. (Same as its mail: the window asks the owner.)
    let sending_calendar = request.account.is_shared()
        && request.mail.is_none()
        && request.changes.as_ref().is_some_and(|changes| changes.ops.iter().any(mailcraft_model::remote::EventOp::mails_others));
    if sending_calendar {
        let kept = if request.refresh.is_empty() {
            Err(NetError::Config("no sign-in to renew".into()))
        } else {
            renew_send(&request.oauth, &request.refresh)
        };
        match kept {
            Ok(got) => {
                access = got.tokens.access.clone();
                out.tokens = Some(got.tokens);
            }
            Err(e) => {
                if e.is_consent_needed() {
                    log::info!(
                        "calendar: sending from a shared mailbox isn't allowed yet; what writes to other people waits for the owner's permission"
                    );
                    out.send_consent_needed = true;
                } else {
                    log::info!("calendar: no token for sending from a shared mailbox ({e}); what writes to other people waits");
                }
                if let Some(changes) = request.changes.as_mut() {
                    changes.ops.retain(|op| !op.mails_others());
                }
            }
        }
    }
    if access.is_empty() {
        // For everything Graph is asked for; a sign-in from before the newest of it was asked
        // for gets its tokens for what it covers, and says what is missing.
        match renew(&request.oauth, &request.refresh, request.favorites_scopes) {
            Ok(got) => {
                access = got.tokens.access.clone();
                out.extras_missing = got.extras_missing;
                // (Asked for without the Favorites' permissions, it says nothing of them.)
                out.favorites_covered = request.favorites_scopes.then_some(got.covers_favorites);
                out.tokens = Some(got.tokens);
            }
            Err(e) if e.is_consent_needed() => {
                log::info!("graph: the sign-in doesn't cover what was asked for; the owner's permission is needed");
                out.consent_needed = true;
                return out;
            }
            // A sign-in that has run out: the window asks the owner (the mail's leg does for an
            // account on IMAP; for one kept in step through Graph, the Graph run does).
            Err(e) if e.is_refused_sign_in() => {
                log::info!("graph: the sign-in was refused when a token was asked for");
                out.sign_in_refused = true;
                return out;
            }
            Err(e) => {
                log::warn!("graph: the sign-in couldn't be renewed: {e}");
                out.errors.push(format!("Couldn't reach Microsoft for sending, the calendar and the contacts: {e}"));
                return out;
            }
        }
    }
    let shared_address = request.account.shared_mailbox();
    let mut refused = false;
    for message in &request.outbox {
        if refused {
            // The token stopped working part-way: the rest waits for the next run and a new one.
            out.held.push(message.id);
            continue;
        }
        match graph::send(&access, message, &graph::SendOptions::default()) {
            Ok(()) => out.sent.push(message.id),
            // Not allowed to send this way: the mail server gets its turn in this run.
            Err(NetError::Forbidden(_)) => {}
            Err(e) if e.is_refused_sign_in() => {
                refused = true;
                out.held.push(message.id);
            }
            // Whether it went is not known (then it is held for the owner), or Microsoft said
            // later (then it goes next run): not sent a second way now.
            Err(e) => {
                out.held.push(message.id);
                if graph::send_unsure(&e) {
                    out.unsure.push(message.id);
                }
                out.errors.push(format!("Couldn't send \"{}\" through Microsoft: {e}", message.subject.trim().replace(['"', '\\'], "'")));
            }
        }
    }
    // An account kept in step through Graph (R3): the folder changes made here go up, and every
    // folder is listed. (Nothing of a folder's name or id goes into the log.)
    if refused && request.mail.is_some() {
        log::warn!("graph mail: the token stopped working while sending; the mail waits for the next run");
    }
    if let Some(mail) = request.mail.as_ref().filter(|_| !refused) {
        // The run comes a piece at a time (the folders, then a page of mail at a time): to the
        // window as each comes, or put together for a caller that takes it in at once.
        let mut result: Option<mailcraft_model::sync::GraphMailResult> = None;
        let (cancel, pieces) = (&request.cancel, &request.mail_pieces);
        // When Graph stops taking the token part-way (it ran out during a long run, or while the
        // computer slept), one new one from the refresh token, the newest there is (#306).
        let renewed: std::cell::RefCell<Option<mailcraft_model::sync::Tokens>> = std::cell::RefCell::new(None);
        let refresh = out.tokens.as_ref().map(|t| t.refresh.clone()).filter(|r| !r.is_empty()).unwrap_or_else(|| request.refresh.clone());
        let again = || {
            let got = if mail.shared && !mail.sends.is_empty() {
                renew_send(&request.oauth, &refresh)?
            } else {
                renew(&request.oauth, &refresh, request.favorites_scopes)?
            };
            let access = got.tokens.access.clone();
            *renewed.borrow_mut() = Some(got.tokens);
            Ok(access)
        };
        graph::sync_mail_renewing(&access, &again, mail, cancel, &mut |piece| match pieces {
            Some(to) => {
                let summary = (piece.listing.as_ref().map(|l| l.folders.len()), piece.errors.len());
                // Nobody is listening any more (the window closed): stop.
                if to.send(piece).is_err() {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if let (Some(folders), problems) = summary {
                    log::info!("graph folders: {folders} folders listed, {problems} problems");
                }
            }
            None => match &mut result {
                Some(all) => all.absorb(piece),
                None => result = Some(piece),
            },
        });
        if let Some(result) = result {
            log::info!(
                "graph folders: {} of {} changes answered, {} folders listed, complete: {}, {} problems",
                result.changes.len(),
                mail.changes.len(),
                result.listing.as_ref().map_or(0, |listing| listing.folders.len()),
                result.listing.as_ref().is_some_and(|listing| listing.complete),
                result.errors.len()
            );
            out.mail = Some(result);
        }
        // A token renewed part-way is the one kept, and the one the rest of the run shows.
        if let Some(tokens) = renewed.into_inner() {
            access = tokens.access.clone();
            out.tokens = Some(tokens);
        }
    }
    // What was made, moved, changed or deleted in the calendar here goes up before the calendar
    // comes down: what comes down then is what the server has after it.
    if let Some(changes) = request.changes.as_ref().filter(|changes| !changes.ops.is_empty() && !refused) {
        log::info!("calendar: {} changes go to the server", changes.ops.len());
        let uploaded =
            graph::push_events(&access, changes, &graph::EventOptions { zone: zone.to_string(), shared: shared_address.clone().unwrap_or_default() });
        // What the server made of each change, for the log: the kind of change, what was sent
        // of it by name, and the answer, never what the event says.
        for outcome in &uploaded.outcomes {
            log_outcome(outcome);
        }
        if uploaded.outcomes.len() < changes.ops.len() {
            log::warn!("calendar: {} of {} changes were answered; the rest wait", uploaded.outcomes.len(), changes.ops.len());
        }
        refused = uploaded.token_refused;
        out.uploaded = Some(uploaded);
    }
    // The wide stretch with the contacts, every quarter of an hour; otherwise the days on screen.
    // (The window counts the wide stretch from its own clock and says it in `calendar`.)
    let now = mailcraft_time::now();
    let stretch = if request.directory { request.calendar.or(Some((now - CALENDAR_BEHIND, now + CALENDAR_AHEAD))) } else { request.calendar };
    // A shared mailbox's calendars (#70): asked for and written at its own address, and only
    // those that are ticked are asked for (the window says which, in the asks); no contacts,
    // categories or settings of it.
    // The series the mailbox expects in each calendar's stretch: asked about by name when the
    // stretch brings nothing of them.
    let expected = |calendar: &str| request.asks.iter().find(|ask| ask.calendar == calendar).map(|ask| ask.series.clone()).unwrap_or_default();
    // Which calendars the account has, and in which groups: with the wide stretch.
    let mut others: Vec<String> = request.asks.iter().map(|ask| ask.calendar.clone()).filter(|calendar| !calendar.is_empty()).collect();
    if request.directory && !refused {
        let listed = match &shared_address {
            Some(address) => graph::shared_calendars(&access, address),
            None => graph::calendars(&access),
        };
        match listed {
            Ok(mut list) => {
                // The calendars the owner opened by address (#70), each read at its own address:
                // those that can be read are in the list as calendars of the account, and those
                // that can't are said, with why. (Not for a shared mailbox's job.)
                if shared_address.is_none() {
                    // The copy of a shared mailbox's calendar that the owner hid (the server lists
                    // it among the account's own too, #70) isn't read: the shared mailbox's job
                    // reads its calendar at its own address.
                    list.calendars.retain(|calendar| calendar.main || !request.account.hides_listed(calendar.owner.as_ref()));
                    let (opened, no_good) = open_by_address(&access, &request.account.opened_calendars);
                    list.calendars.extend(opened);
                    list.refused = no_good.0;
                    refused |= no_good.1;
                }
                // Every calendar there is comes with the wide stretch, known here already or not.
                // (Not from a list that doesn't say which one is the main calendar: that one
                // comes anyway, and would then come twice.) A shared mailbox's only when ticked.
                let told_apart = list.calendars.iter().any(|calendar| calendar.main);
                for calendar in list.calendars.iter().filter(|calendar| told_apart && !calendar.main && shared_address.is_none()) {
                    if !others.contains(&calendar.id) {
                        others.push(calendar.id.clone());
                    }
                }
                out.calendars = Some(list.for_account(&request.account));
            }
            Err(e) if e.is_refused_sign_in() => refused = true,
            // A shared mailbox that lists no calendars to this account (its calendar is shared
            // for free and busy times only, or not at all): its main one is asked for all the
            // same, and says which.
            Err(NetError::Forbidden(_)) if shared_address.is_some() => log::info!("calendar: a shared mailbox lists no calendars to this account"),
            Err(e) => out.errors.push(format!("Couldn't bring the list of calendars down: {e}")),
        }
    }
    if let Some((from, to)) = stretch.filter(|_| !refused) {
        let shared = shared_address.clone().unwrap_or_default();
        // (A shared mailbox's main calendar when it is ticked.)
        let wants_main = shared_address.is_none() || request.asks.iter().any(|ask| ask.calendar.is_empty());
        let main = graph::EventsQuery { calendar: String::new(), from, to, series: expected(""), shared: shared.clone() };
        let days = |instant: i64| mailcraft_time::DateTime::from_timestamp(instant, 0).date.iso();
        match wants_main.then(|| graph::events_in(&access, &main)) {
            None => {}
            Some(Ok(events)) => {
                log::info!(
                    "calendar: the main calendar came down: {} events from {} to {} (all of them: {})",
                    events.events.len(),
                    days(from),
                    days(to),
                    events.complete
                );
                out.events = Some(events.for_account(&request.account));
            }
            Some(Err(e)) if e.is_refused_sign_in() => refused = true,
            Some(Err(e)) if shared_address.is_some() => out.errors.push(format!("Couldn't bring the shared mailbox's calendar down: {e}")),
            Some(Err(e)) => out.errors.push(format!("Couldn't bring the calendar down: {e}")),
        }
        // Those opened by address come first (the owner asked for them by name): they count
        // within the same cap as the account's other calendars.
        others.sort_by_key(|calendar| !calendar.starts_with(mailcraft_model::remote::OPENED_PREFIX));
        for calendar in others.iter().take(MAX_OTHER_CALENDARS) {
            if refused {
                break;
            }
            let query = graph::EventsQuery { calendar: calendar.clone(), from, to, series: expected(calendar), shared: shared.clone() };
            match graph::events_in(&access, &query) {
                Ok(events) => {
                    log::info!(
                        "calendar: calendar #{} came down: {} events (all of them: {})",
                        mailcraft_model::remote::id_tag(calendar),
                        events.events.len(),
                        events.complete
                    );
                    out.more_events.push(events.for_account(&request.account));
                }
                Err(e) if e.is_refused_sign_in() => refused = true,
                // A calendar that is gone, or may not be read: the list says so the next time.
                Err(e) => log::debug!("one of the account's other calendars didn't come down: {e}"),
            }
        }
    }
    if request.directory && !refused && shared_address.is_none() {
        match graph::contacts(&access) {
            Ok(contacts) => out.contacts = Some(contacts.for_account(&request.account)),
            Err(e) if e.is_refused_sign_in() => refused = true,
            Err(e) => out.errors.push(format!("Couldn't bring the contacts down: {e}")),
        }
    }
    // The mailbox's own categories and their colours: an extra that an older sign-in doesn't
    // cover, and nothing to trouble the owner with when it can't be had.
    if request.directory && !refused && shared_address.is_none() {
        match graph::categories(&access) {
            Ok(categories) => out.categories = Some(categories.for_account(&request.account)),
            Err(NetError::Forbidden(_)) => out.extras_missing = true,
            Err(e) if e.is_refused_sign_in() => refused = true,
            Err(e) => log::debug!("the list of categories didn't come down: {e}"),
        }
    }
    // The working hours the mailbox has set: an extra of the same kind, and only ever read.
    if request.settings && !refused {
        match graph::mailbox_settings(&access) {
            Ok(settings) => out.mailbox_hours = settings.working_hours.map(|hours| mailbox_hours(&request.account.email, hours)),
            Err(NetError::Forbidden(_)) => out.extras_missing = true,
            Err(e) if e.is_refused_sign_in() => refused = true,
            Err(e) => log::debug!("the mailbox's working hours didn't come down: {e}"),
        }
    }
    // The Favorites as Outlook has them: read, never written, and nothing to trouble the owner
    // with when they can't be had (the folder pane says what there is to say, in one line).
    if request.favorites && !refused {
        let outcome = |outcome: FavoritesOutcome| Some(FavoritesDownload { outcome, ..Default::default() }.for_account(&request.account));
        if out.favorites_covered == Some(false) {
            // The sign-in doesn't cover it: the server isn't asked what it would only refuse.
            log::info!("favorites: the sign-in doesn't cover reading them");
            out.favorites = outcome(FavoritesOutcome::NotPermitted);
        } else {
            match graph::favorites(&access) {
                Ok(download) => out.favorites = Some(download.for_account(&request.account)),
                Err(NetError::Forbidden(_)) => {
                    log::info!("favorites: the server says the sign-in doesn't cover reading them (status 403)");
                    out.favorites = outcome(FavoritesOutcome::NotPermitted);
                }
                Err(e) if e.is_refused_sign_in() => refused = true,
                // (The error says what was being done and the status: no name, no id.)
                Err(e) => {
                    log::warn!("favorites: they couldn't be read: {e}");
                    out.favorites = outcome(FavoritesOutcome::Failed);
                    out.favorites_error = favorites_words(&e).to_string();
                }
            }
        }
    }
    out.token_refused = refused;
    out
}

/// A mailbox's working hours as the engine keeps them, for the account with the address `email`.
/// The zone they are in goes along by the name the mailbox has for it (`Pacific Standard Time`):
/// the engine knows the zones, and shifts the hours to the clock the calendar is shown by
/// (`calendar_settings::work_hours`). No offset is given beside it: a zone the engine doesn't
/// know (`Customized Time Zone`) is one this program can't look up either, and its hours are
/// then taken as they are written.
fn mailbox_hours(email: &str, hours: mailcraft_net::graph::WorkingHours) -> mailcraft_engine::cmd::calendar_settings::MailboxHours {
    mailcraft_engine::cmd::calendar_settings::MailboxHours {
        email: email.trim().to_string(),
        days: hours.days,
        start: hours.start,
        end: hours.end,
        zone: hours.zone,
        offset: None,
    }
}

/// Most calendars of an account, beside its main one, that are brought down in one go: those
/// the server lists and those the owner opened by address, together.
const MAX_OTHER_CALENDARS: usize = 20;

/// Read the calendars opened by address with `token`: the calendars that can be read (named by
/// their owners, with their rights), and those that can't, with why; and whether the sign-in
/// itself was refused. At most [`mailcraft_model::MAX_OPENED_CALENDARS`], one request each.
pub fn open_by_address(
    token: &str,
    opened: &[mailcraft_model::OpenedCalendar],
) -> (Vec<mailcraft_model::remote::RemoteCalendar>, (Vec<mailcraft_model::remote::OpenedRefusal>, bool)) {
    use mailcraft_model::remote::OpenedRefusal;
    use mailcraft_net::graph::{self, Opened};
    let (mut read, mut refusals, mut signed_out) = (Vec::new(), Vec::new(), false);
    for one in opened.iter().take(mailcraft_model::MAX_OPENED_CALENDARS) {
        if signed_out {
            break;
        }
        match graph::open_calendar(token, &one.address) {
            Opened::Read(calendar) => read.push(*calendar),
            Opened::Refused(why) => {
                // (Said by what happened, never by whom: the address is not in the log.)
                log::info!("calendar: a calendar opened by address couldn't be read ({why:?})");
                refusals.push(OpenedRefusal { address: one.address.clone(), why });
            }
            Opened::SignInRefused => signed_out = true,
        }
    }
    (read, (refusals, signed_out))
}
