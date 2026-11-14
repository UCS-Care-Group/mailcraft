// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People's Address Book (#1262, #1393): the organisation's address list of an Exchange server
//! account, beside the contacts, chosen where People chooses whose contacts it shows
//! (`people.addressBook`).
//!
//! **The whole list.** When it is first shown in a run, the account's Offline Address Book is
//! downloaded from its own server by the host ([`crate::Services::ews_resolve`] with
//! `address_list`, the account's sign-in and certificate) and held in memory while the program
//! runs ([`HeldList`]), never written anywhere; it is downloaded again when it is shown a day
//! later. Everybody in it is listed, by name, and what is typed in People's search box narrows the
//! list on this computer, without asking the server anything.
//!
//! **A search.** A server that publishes no address book (or one that can't be had) is searched
//! instead, as before: what is typed is looked up after a short pause and from two characters on,
//! at most 100 people for it. Only the text typed is sent.
//!
//! Who is listed or found is shown here and kept nowhere else: not in the mailbox, not in a file,
//! not in a log; a contact is made of somebody only when the owner presses Add to Contacts.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Align2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::Module;
use mailcraft_engine::cmd::ews_mail::{ews_address_list_request, ews_resolve_request_for};
use mailcraft_engine::cmd::people;
use mailcraft_model::sync::{
    DirectoryKind, DirectoryPerson, EwsAddressList, EwsResolveRequest, EwsResolveResult, MAX_RESOLVE_PEOPLE, MAX_RESOLVE_TEXT, one_line,
};
use mailcraft_model::{AccountChoice, ContactKey, Id, KeyKind};
use serde_json::json;

use crate::check_names::EwsResolveJob;
use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons, widgets};

/// The fewest characters looked up.
pub const MIN_CHARS: usize = 2;
/// How long typing pauses before what is typed is looked up, in milliseconds.
pub const PAUSE_MS: f64 = 400.0;
/// How old a downloaded address list may get before it is downloaded again, in milliseconds.
pub const REFRESH_MS: f64 = 24.0 * 3600.0 * 1000.0;
/// How long after a download that failed one is tried again, in milliseconds.
pub const RETRY_MS: f64 = 60.0 * 1000.0;
const ROW_H: f32 = 52.0;
const CARD_W: f32 = 720.0;

/// The whole address list of one account, downloaded, or being downloaded (#1393).
pub(crate) struct HeldList {
    account: Id,
    /// The mailbox it is of (`Session::openings`).
    opening: u64,
    pub(crate) list: Option<EwsAddressList>,
    /// What each person is found by, in lower case, beside [`EwsAddressList::people`].
    keys: Vec<String>,
    /// Why the last download failed.
    pub(crate) error: Option<String>,
    pending: Option<Pending>,
    /// When the list came, or the last download was tried.
    at_ms: f64,
    /// One more each time a list comes.
    version: u64,
}

impl HeldList {
    /// What `person` is found by: the name, the address, department, title, company and office.
    fn key(person: &DirectoryPerson) -> String {
        [&person.name, &person.email, &person.department, &person.title, &person.company, &person.office]
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_lowercase())
            .collect::<Vec<_>>()
            .join("\u{1}")
    }

    /// The people every word of `typed` is found in, in the list's order.
    fn matching(&self, typed: &str) -> Vec<DirectoryPerson> {
        let Some(list) = &self.list else { return Vec::new() };
        let words: Vec<String> = typed.split_whitespace().map(str::to_lowercase).collect();
        list.people.iter().zip(&self.keys).filter(|(_, key)| words.iter().all(|w| key.contains(w.as_str()))).map(|(p, _)| p.clone()).collect()
    }
}

/// What People's Address Book has while it is shown: who it lists, and what was looked up.
pub(crate) struct AddressBook {
    account: Id,
    /// The mailbox it is of (`Session::openings`): another mailbox starts it again.
    opening: u64,
    /// What was looked up last (or the list narrowed by), and its answer.
    query: String,
    pub(crate) people: Vec<DirectoryPerson>,
    pub(crate) more: bool,
    pub(crate) error: Option<String>,
    pub(crate) selected: Option<usize>,
    pending: Option<Pending>,
    /// What the search box held when it was last seen to change, and when.
    typed: (String, f64),
    /// The version of the whole list `people` were taken from; none while it is searched.
    local: Option<u64>,
}

/// A look-up or a download under way, for `query`.
struct Pending {
    query: String,
    rx: Receiver<EwsResolveResult>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl AddressBook {
    fn new(account: Id, opening: u64) -> Self {
        Self {
            account,
            opening,
            query: String::new(),
            people: Vec::new(),
            more: false,
            error: None,
            selected: None,
            pending: None,
            typed: (String::new(), 0.0),
            local: None,
        }
    }

    /// Whether a look-up is under way.
    pub(crate) fn searching(&self) -> bool {
        self.pending.is_some()
    }

    /// The person whose card is shown.
    pub(crate) fn chosen(&self) -> Option<&DirectoryPerson> {
        self.selected.and_then(|i| self.people.get(i))
    }

    fn clear(&mut self) {
        (self.query, self.people, self.more, self.error, self.selected, self.pending) = (String::new(), Vec::new(), false, None, None, None);
    }
}

/// The text the search box holds, as it would be looked up, when the owner typed it; else nothing
/// (a search a script or an agent put in the box is not looked up).
fn search_text(app: &MailApp) -> String {
    let typed = one_line(&app.session.view.people_search, MAX_RESOLVE_TEXT);
    if typed == one_line(&app.people_typed, MAX_RESOLVE_TEXT) { typed } else { String::new() }
}

impl MailApp {
    /// The account whose address list People shows now, when it is on screen.
    pub(crate) fn address_book_shown(&self) -> Option<Id> {
        (self.session.view.module == Module::People).then(|| people::shown_book(&self.session)).flatten()
    }

    /// `text` typed by the owner in People's search box: the contacts are searched for it, and an
    /// address list that is shown is narrowed by it or asked for it.
    pub fn type_people_search(&mut self, text: &str) {
        self.people_typed = text.to_string();
        let _ = self.execute("people.search", json!({"query": text}));
    }

    /// Who People's Address Book shows, while it is shown.
    pub fn address_book_found(&self) -> &[DirectoryPerson] {
        self.address_book.as_ref().map_or(&[], |b| b.people.as_slice())
    }

    /// The whole address list of `account` held for this mailbox, when it has come.
    pub fn address_list(&self, account: Id) -> Option<&EwsAddressList> {
        let opening = self.session.openings();
        self.address_lists.iter().find(|h| h.account == account && h.opening == opening).and_then(|h| h.list.as_ref())
    }

    /// Whether the whole address list of an account is being downloaded.
    pub fn downloading_address_list(&self) -> bool {
        self.address_lists.iter().any(|h| h.pending.is_some())
    }

    /// Downloads that have ended are taken in, those of another mailbox let go of; and the list of
    /// `account`, when it is shown, is downloaded when there is none, it is a day old, or the last
    /// try failed a while ago.
    fn poll_address_lists(&mut self, shown: Option<Id>) {
        let opening = self.session.openings();
        let now = crate::now_ms();
        self.address_lists.retain(|h| h.opening == opening);
        for held in &mut self.address_lists {
            let Some(pending) = &held.pending else { continue };
            let result = match pending.rx.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => continue,
                Err(TryRecvError::Disconnected) => {
                    EwsResolveResult { error: Some(tl!("Downloading the address book stopped unexpectedly").to_string()), ..Default::default() }
                }
            };
            held.pending = None;
            held.at_ms = now;
            match result.address_list {
                Some(mut list) => {
                    list.people = std::mem::take(&mut list.people).into_iter().filter_map(DirectoryPerson::checked).collect();
                    held.keys = list.people.iter().map(HeldList::key).collect();
                    held.list = Some(list);
                    held.error = None;
                    held.version = held.version.wrapping_add(1);
                }
                None => held.error = Some(result.error.unwrap_or_else(|| tl!("The server sent no address book").to_string())),
            }
        }
        let Some(account) = shown else { return };
        let due = match self.address_lists.iter().find(|h| h.account == account) {
            None => true,
            Some(h) if h.pending.is_some() => false,
            Some(h) if h.list.is_some() => now - h.at_ms >= REFRESH_MS,
            Some(h) => now - h.at_ms >= RETRY_MS,
        };
        if !due {
            return;
        }
        let started =
            ews_address_list_request(&self.session, account).map_err(|e| tl!(&e.to_string()).to_string()).and_then(|r| self.start_job(r, ""));
        let held = match self.address_lists.iter().position(|h| h.account == account) {
            Some(at) => &mut self.address_lists[at],
            None => {
                self.address_lists.push(HeldList {
                    account,
                    opening,
                    list: None,
                    keys: Vec::new(),
                    error: None,
                    pending: None,
                    at_ms: now,
                    version: 0,
                });
                let last = self.address_lists.len() - 1;
                &mut self.address_lists[last]
            }
        };
        held.at_ms = now;
        match started {
            Ok(pending) => held.pending = Some(pending),
            Err(error) => held.error = Some(error),
        }
    }

    /// Each frame: the whole list is downloaded and narrowed by what the search box holds; or,
    /// where there is none, what the search box holds is looked up once typing has paused, and an
    /// answer that has come is taken in. Away from it, it is let go of.
    pub fn poll_address_book(&mut self, ctx: &egui::Context) {
        let shown = self.address_book_shown();
        self.poll_address_lists(shown);
        let Some(account) = shown else {
            self.address_book = None;
            return;
        };
        let opening = self.session.openings();
        let now = crate::now_ms();
        let typed = search_text(self);
        let book = self.address_book.get_or_insert_with(|| AddressBook::new(account, opening));
        if book.account != account || book.opening != opening {
            *book = AddressBook::new(account, opening);
        }
        if typed != book.typed.0 {
            book.typed = (typed.clone(), now);
        }
        let held = self.address_lists.iter().find(|h| h.account == account && h.opening == opening);
        // The whole list: narrowed here, and nothing is asked.
        if let Some(held) = held.filter(|h| h.list.is_some()) {
            if book.local != Some(held.version) || book.query != typed {
                let chosen = book.chosen().map(|p| p.email.clone());
                book.people = held.matching(&typed);
                book.selected = chosen.and_then(|email| book.people.iter().position(|p| p.email == email)).or((book.people.len() == 1).then_some(0));
                (book.query, book.more, book.error, book.pending, book.local) = (typed, false, None, None, Some(held.version));
            }
            return;
        }
        if book.local.is_some() {
            book.clear();
            book.local = None;
        }
        // Being downloaded: nothing is searched meanwhile.
        if held.is_some_and(|h| h.pending.is_some()) {
            return;
        }
        if let Some(pending) = &book.pending {
            match pending.rx.try_recv() {
                Ok(result) => {
                    let query = pending.query.clone();
                    let answer = result.found.into_iter().find(|f| f.text.to_lowercase() == query.to_lowercase());
                    book.people = answer
                        .as_ref()
                        .map(|a| a.people.iter().take(MAX_RESOLVE_PEOPLE).cloned().filter_map(DirectoryPerson::checked).collect())
                        .unwrap_or_default();
                    book.more = answer.is_some_and(|a| a.more);
                    book.error = result.error;
                    book.selected = (book.people.len() == 1).then_some(0);
                    book.query = query;
                    book.pending = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    book.error = Some(tl!("Searching the address list stopped unexpectedly").to_string());
                    book.query = pending.query.clone();
                    book.pending = None;
                }
            }
        }
        if typed.chars().count() < MIN_CHARS {
            book.clear();
            return;
        }
        let asked = book.pending.as_ref().map(|p| p.query.as_str()).unwrap_or(book.query.as_str());
        if typed == asked {
            return;
        }
        let waited = now - book.typed.1;
        if waited < PAUSE_MS {
            ctx.request_repaint_after(std::time::Duration::from_millis((PAUSE_MS - waited).max(1.0) as u64));
            return;
        }
        let started = ews_resolve_request_for(&self.session, account, std::slice::from_ref(&typed))
            .map_err(|e| tl!(&e.to_string()).to_string())
            .and_then(|request| self.start_job(request, &typed));
        if let Some(book) = &mut self.address_book {
            match started {
                Ok(pending) => book.pending = Some(pending),
                Err(error) => {
                    // Said, and not asked again until something else is typed.
                    (book.query, book.people, book.more, book.selected, book.pending) = (typed, Vec::new(), false, None, None);
                    book.error = Some(error);
                }
            }
        }
    }

    /// Hands `request` to the host, for `query`.
    fn start_job(&self, request: EwsResolveRequest, query: &str) -> Result<Pending, String> {
        let account = request.account;
        let Some(resolve) = &self.services.ews_resolve else {
            return Err(tl!("This version of MailCraft can't search an address list").to_string());
        };
        let Some(secret) = self.ews_secret(account) else {
            return Err(tl!("The account's password is needed first: press Send/Receive and type it").to_string());
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = resolve(EwsResolveJob { request, secret, cancel: cancel.clone() });
        Ok(Pending { query: query.to_string(), rx, cancel })
    }
}

/// People with an address list in place of the contacts: the list of who was found, and the card
/// of the one chosen.
pub(crate) fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let frame = egui::Frame::NONE.fill(t.surface).stroke(Stroke::new(1.0, t.border));
    let max = (ui.available_width() - 320.0).max(260.0);
    egui::Panel::left("people_list").default_size(320.0f32.min(max)).size_range(240.0..=max).resizable(true).frame(frame).show(ui, |ui| {
        crate::nav::module_foot(app, ui);
        list(app, ui);
    });
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.surface)).show(ui, |ui| card(app, ui));
}

/// The choice of what People lists, at the top of its pane: the contacts (of all accounts, of
/// one, of this computer, where accounts keep contacts) or the address list of an Exchange server
/// account. Shown when there is anything to choose.
pub(crate) fn book_choice(app: &mut MailApp, ui: &mut Ui) {
    let mailbox = &app.session.mailbox;
    let accounts = people::contact_accounts(mailbox);
    let lists = people::address_lists(mailbox);
    if accounts.is_empty() && lists.is_empty() {
        return;
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Book {
        Contacts(AccountChoice),
        List(Id),
    }
    let label = |book: Book| match book {
        Book::Contacts(_) if accounts.is_empty() => tl!("Contacts").to_string(),
        Book::Contacts(AccountChoice::All) => tl!("All accounts").to_string(),
        Book::Contacts(AccountChoice::Here) => widgets::place_name(mailbox, 0),
        Book::Contacts(AccountChoice::Account(account)) => widgets::place_name(mailbox, account),
        Book::List(account) => crate::i18n::fmt(tl!("Address list · {account}"), &[("account", &widgets::place_name(mailbox, account))]),
    };
    let current = match people::shown_book(&app.session) {
        Some(account) => Book::List(account),
        None => Book::Contacts(people::shown_account(&app.session)),
    };
    let contacts: Vec<Book> = if accounts.is_empty() {
        vec![Book::Contacts(AccountChoice::All)]
    } else {
        std::iter::once(AccountChoice::All)
            .chain(accounts.iter().copied().map(AccountChoice::Account))
            .chain([AccountChoice::Here])
            .map(Book::Contacts)
            .collect()
    };
    let mut picked = None;
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        let width = (ui.available_width() - 10.0).max(60.0);
        egui::ComboBox::from_id_salt("people_book").width(width).selected_text(label(current)).truncate().show_ui(ui, |ui| {
            for book in contacts.iter().copied().chain(lists.iter().copied().map(Book::List)) {
                if ui.selectable_label(book == current, label(book)).clicked() {
                    picked = Some(book);
                }
            }
        });
    });
    ui.add_space(8.0);
    match picked.filter(|book| *book != current) {
        Some(Book::Contacts(choice)) => {
            let _ = app.run("view.peopleAccount", json!({"value": choice.code()}));
        }
        Some(Book::List(account)) => {
            let _ = app.run("people.addressBook", json!({"account": account}));
        }
        None => {}
    }
}

fn list(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let found = app.address_book.as_ref().map_or(0, |b| b.people.len());
    let header = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 34.0));
    crate::chrome::pane_title(ui, tl!("Address list"), &if found > 0 { found.to_string() } else { String::new() });
    book_choice(app, ui);
    let line = ui.cursor().min.y;
    ui.painter().line_segment([pos2(header.min.x, line), pos2(header.max.x, line)], Stroke::new(1.0, t.border));
    let typed = search_text(app);
    let Some(book) = &app.address_book else { return };
    let opening = app.session.openings();
    let held = app.address_lists.iter().find(|h| h.account == book.account && h.opening == opening);
    // What there is to say in place of a list.
    let said = if book.local.is_some() {
        match (book.people.is_empty(), typed.is_empty()) {
            (true, true) => Some((tl!("The organisation's address book is empty").to_string(), "people")),
            (true, false) => Some((tl!("Nobody in the address book matches").to_string(), "search")),
            _ => None,
        }
    } else if held.is_some_and(|h| h.pending.is_some()) {
        Some((tl!("Downloading the organisation's address book…").to_string(), "people"))
    } else if typed.chars().count() < MIN_CHARS {
        let hint = tl!("Type a name in the search box to search the organisation's address list");
        let text = match held.and_then(|h| h.error.as_ref()) {
            Some(error) => format!("{}\n\n{hint}", tl!(error)),
            None => hint.to_string(),
        };
        Some((text, "search"))
    } else if book.searching() && book.people.is_empty() {
        Some((tl!("Searching…").to_string(), "search"))
    } else if let Some(error) = &book.error {
        Some((error.clone(), "people"))
    } else if book.people.is_empty() && book.query == typed {
        Some((tl!("Nobody in the address list matches").to_string(), "search"))
    } else if book.people.is_empty() {
        Some((tl!("Searching…").to_string(), "search"))
    } else {
        None
    };
    if let Some((text, icon)) = said {
        ui.vertical_centered(|ui| {
            ui.add_space(56.0);
            let (r, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
            icons::paint(ui.painter(), r, icon, t.border_strong, t.border_strong);
            ui.add_space(10.0);
            ui.add(egui::Label::new(RichText::new(text).font(regular(12.5)).color(t.text_dim)).wrap());
        });
        return;
    }
    let (count, more, selected) = (book.people.len(), book.more, book.selected);
    let mut clicked = None;
    let mut write = None;
    // Only the rows on screen are drawn: a whole address list may hold many thousands.
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("address_list").show_rows(ui, ROW_H, count, |ui, rows| {
        let shown: Vec<(usize, DirectoryPerson)> =
            app.address_book.as_ref().map(|b| rows.clone().filter_map(|i| b.people.get(i).cloned().map(|p| (i, p))).collect()).unwrap_or_default();
        for (index, person) in &shown {
            let index = *index;
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            app.hit.add("addressListRow", index as u64, r);
            if selected == Some(index) {
                ui.painter().rect_filled(r, 0.0, t.selection);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 0.0, t.hover);
            }
            widgets::avatar(ui, pos2(r.min.x + 28.0, r.center().y), 16.0, &person.address());
            let x = r.min.x + 54.0;
            let w = (r.max.x - x - 10.0).max(0.0);
            let name = if person.name.is_empty() { person.email.as_str() } else { person.name.as_str() };
            paint_clipped(ui.painter(), pos2(x, r.min.y + 17.0), w, name, semibold(13.0), t.text);
            paint_clipped(ui.painter(), pos2(x, r.min.y + 35.0), w, &second_line(person), regular(11.5), t.text_dim);
            if resp.clicked() {
                clicked = Some(index);
            }
            if resp.double_clicked() {
                write = Some(person.clone());
            }
        }
        if more && rows.end >= count {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new(crate::i18n::fmt(
                        tl!("Only the first {n} are shown: type more of the name to find somebody else."),
                        &[("n", &count.to_string())],
                    ))
                    .font(regular(11.5))
                    .color(t.text_dim),
                );
            });
        }
    });
    if let (Some(index), Some(book)) = (clicked, &mut app.address_book) {
        book.selected = Some(index);
    }
    if let Some(person) = write {
        new_email(app, &person);
    }
}

/// What tells them apart, under the name: the job title and department, else the address, and
/// what kind of recipient it is when it isn't a person.
fn second_line(person: &DirectoryPerson) -> String {
    let mut parts: Vec<String> = [&person.title, &person.department].iter().filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    if parts.is_empty() {
        parts.push(person.email.clone());
    }
    if let Some(kind) = kind_text(person.kind) {
        parts.push(kind.to_string());
    }
    parts.join(" · ")
}

fn kind_text(kind: DirectoryKind) -> Option<&'static str> {
    match kind {
        DirectoryKind::List => Some(tl!("Distribution list")),
        DirectoryKind::Group => Some(tl!("Group")),
        DirectoryKind::PublicFolder => Some(tl!("Public folder")),
        DirectoryKind::Contact => Some(tl!("Outside the organisation")),
        DirectoryKind::Mailbox | DirectoryKind::Other => None,
    }
}

/// A new message to `person`, from the account whose address list it is.
fn new_email(app: &mut MailApp, person: &DirectoryPerson) {
    let account = app.address_book.as_ref().map(|b| b.account);
    let _ = app.run("mail.new", json!({"to": person.address().full(), "account": account}));
}

/// The contact that already has `person`'s address, if any: Add to Contacts makes no second one.
fn known_contact(app: &MailApp, person: &DirectoryPerson) -> Option<Id> {
    (!person.email.trim().is_empty()).then(|| app.session.mailbox.contact_by_email(&person.email).map(|c| c.id)).flatten()
}

/// The contact `id` shown among the contacts, leaving the address list.
fn open_contact(app: &mut MailApp, id: Id) {
    if let Err(e) = app.run("people.select", json!({"id": id})) {
        app.status(tl!(&e));
    }
}

/// A contact on this computer made of `person`, on the owner's say-so; one that has the address
/// already is not made twice. The address list stays in view, and the card says the contact is
/// there, with Open Contact.
fn add_to_contacts(app: &mut MailApp, person: &DirectoryPerson) {
    if known_contact(app, person).is_some() {
        app.status(tl!("Already in your contacts"));
        return;
    }
    let mut phones = Vec::new();
    for (label, number) in [("work", &person.phone), ("mobile", &person.mobile)] {
        if !number.is_empty() {
            phones.push(json!({"label": label, "value": number}));
        }
    }
    // The certificates go onto the contact's card, those a card takes (for mail, not an
    // authority's), as a certificate added from a file would.
    let keys: Vec<serde_json::Value> = person
        .certificates
        .iter()
        .filter(|der| mailcraft_engine::cmd::keys::card_key_from(der).is_ok())
        .map(|der| json!({"data": mailcraft_engine::cmd::base64_encode(der)}))
        .collect();
    let params = json!({
        "displayName": person.name, "email": person.email, "company": person.company,
        "department": person.department, "title": person.title, "phones": phones, "keys": keys,
    });
    match app.run("people.new", params) {
        Ok(_) => app.status(tl!("Added to Contacts")),
        Err(e) => app.status(tl!(&e)),
    }
}

fn card(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(person) = app.address_book.as_ref().and_then(AddressBook::chosen).cloned() else {
        if app.address_book.as_ref().is_some_and(|b| !b.people.is_empty()) {
            let r = ui.available_rect_before_wrap();
            let y = r.min.y + r.height() * 0.38;
            icons::paint(ui.painter(), Rect::from_center_size(pos2(r.center().x, y), vec2(56.0, 56.0)), "people", t.border_strong, t.border_strong);
            ui.painter().text(
                pos2(r.center().x, y + 50.0),
                Align2::CENTER_CENTER,
                tl!("Select somebody to see the details"),
                regular(13.0),
                t.text_dim,
            );
        }
        return;
    };
    let account = app.address_book.as_ref().map_or(0, |b| b.account);
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("address_card").show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 28, right: 28, top: 24, bottom: 28 }).show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(CARD_W));
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(80.0, 80.0), Sense::hover());
                widgets::avatar(ui, r.center(), 40.0, &person.address());
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
                    let role: Vec<&str> = [person.title.as_str(), person.department.as_str(), person.company.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
                    ui.add_space(if role.is_empty() { 26.0 } else { 16.0 });
                    let name = if person.name.is_empty() { person.email.as_str() } else { person.name.as_str() };
                    ui.add(egui::Label::new(RichText::new(name).font(semibold(20.0)).color(t.text)).wrap());
                    let mut line = role.join(" · ");
                    if let Some(kind) = kind_text(person.kind) {
                        line = if line.is_empty() { kind.to_string() } else { format!("{line} · {kind}") };
                    }
                    if !line.is_empty() {
                        ui.add(egui::Label::new(RichText::new(line).font(regular(12.5)).color(t.text_dim)).wrap());
                    }
                });
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
                if widgets::primary_button(ui, tl!("Email"), true).clicked() {
                    new_email(app, &person);
                }
                match known_contact(app, &person) {
                    Some(id) => {
                        if widgets::icon_text_button(ui, "people", tl!("Open Contact"), true).clicked() {
                            open_contact(app, id);
                        }
                        ui.add_space(4.0);
                        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                        icons::paint(ui.painter(), r, "check", t.text_dim, t.text_dim);
                        ui.label(RichText::new(tl!("In your contacts")).font(regular(12.5)).color(t.text_dim));
                    }
                    None => {
                        if widgets::icon_text_button(ui, "newContact", tl!("Add to Contacts"), true).clicked() {
                            add_to_contacts(app, &person);
                        }
                    }
                }
            });
            crate::people_view::section(ui, tl!("Contact"));
            let tip = crate::i18n::fmt(tl!("Write to {address}"), &[("address", &person.email)]);
            if crate::people_view::field(ui, "mail", &person.email, "", true).on_hover_text(tip).clicked() {
                new_email(app, &person);
            }
            if !person.phone.is_empty() {
                crate::people_view::field(ui, "phone", &person.phone, tl!("Work"), false);
            }
            if !person.mobile.is_empty() {
                crate::people_view::field(ui, "phone", &person.mobile, tl!("Mobile"), false);
            }
            if !person.office.is_empty() {
                crate::people_view::section(ui, tl!("Workplace"));
                crate::people_view::field(ui, "address", &person.office, "", false);
            }
            if !person.certificates.is_empty() {
                crate::people_view::section(ui, tl!("Certificates"));
                for der in &person.certificates {
                    let key = ContactKey { kind: KeyKind::X509, data: der.to_vec(), ..ContactKey::default() };
                    crate::security::card_key_line(app, ui, &key, &t);
                    if mailcraft_engine::cmd::keys::card_key_from(der).is_err() {
                        ui.label(
                            RichText::new(tl!("Not for mail, or an authority's: Add to Contacts leaves it out."))
                                .font(regular(11.5))
                                .color(t.text_dim),
                        );
                    }
                }
            }
            ui.add_space(14.0);
            let place = widgets::place_name(&app.session.mailbox, account);
            let from = if app.address_book.as_ref().is_some_and(|b| b.local.is_some()) {
                crate::i18n::fmt(
                    tl!("From the organisation's address book of {account}, held in memory while MailCraft runs and never saved: Add to Contacts keeps a copy on this computer."),
                    &[("account", &place)],
                )
            } else {
                crate::i18n::fmt(
                    tl!("From the organisation's address list of {account}. Shown while you look, and not kept: Add to Contacts keeps a copy on this computer."),
                    &[("account", &place)],
                )
            };
            ui.add(egui::Label::new(RichText::new(from).font(regular(11.5)).color(t.text_dim)).wrap());
        });
    });
}

#[cfg(test)]
#[path = "address_book_tests.rs"]
mod address_book_tests;
