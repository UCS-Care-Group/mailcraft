// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People's Address Book (#1262): the organisation's address list of an Exchange server account,
//! beside the contacts, chosen where People chooses whose contacts it shows (`people.addressBook`).
//!
//! An address list can be searched and not listed whole (Exchange Web Services answers a search,
//! at most 100 people for it), so what is typed in People's search box is looked up, after a short
//! pause and from two characters on, by the host ([`crate::Services::ews_resolve`], as Check Names
//! is) with the account's own server, sign-in and certificate. Only the text typed is sent. Who is
//! found is shown here and kept nowhere: not in the mailbox, not in a file, not in a log; a
//! contact is made of somebody only when the owner presses Add to Contacts.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Align2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::Module;
use mailcraft_engine::cmd::ews_mail::ews_resolve_request_for;
use mailcraft_engine::cmd::people;
use mailcraft_model::sync::{DirectoryKind, DirectoryPerson, EwsResolveResult, MAX_RESOLVE_PEOPLE, MAX_RESOLVE_TEXT, one_line};
use mailcraft_model::{AccountChoice, Id};
use serde_json::json;

use crate::check_names::EwsResolveJob;
use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons, widgets};

/// The fewest characters looked up.
pub const MIN_CHARS: usize = 2;
/// How long typing pauses before what is typed is looked up, in milliseconds.
pub const PAUSE_MS: f64 = 400.0;
const ROW_H: f32 = 52.0;
const CARD_W: f32 = 720.0;

/// What People's Address Book has: what was looked up and who it found, while it is shown.
pub(crate) struct AddressBook {
    account: Id,
    /// The mailbox it is of (`Session::openings`): another mailbox starts it again.
    opening: u64,
    /// What was looked up last, and its answer.
    query: String,
    pub(crate) people: Vec<DirectoryPerson>,
    pub(crate) more: bool,
    pub(crate) error: Option<String>,
    pub(crate) selected: Option<usize>,
    pending: Option<Pending>,
    /// What the search box held when it was last seen to change, and when.
    typed: (String, f64),
    /// What the owner typed in the search box while this list was shown (or held there when they
    /// chose it): only that is looked up, and only while the box still holds it.
    owner: String,
    /// What kept it from asking, said in `error`: once that has gone, it asks again.
    blocked: Option<Blocked>,
}

/// What can keep the Address Book from asking, and go by itself.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Blocked {
    /// Work Offline is on.
    Offline,
    /// There is no password at hand, or the server turned it down.
    Password,
}

/// A look-up under way, for `query`.
struct Pending {
    query: String,
    rx: Receiver<EwsResolveResult>,
    cancel: Arc<AtomicBool>,
    /// The password it went with: only that one is forgotten when the server turns it down.
    used: String,
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
            owner: String::new(),
            blocked: None,
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
        self.blocked = None;
    }
}

/// The text the search box holds, as it would be looked up, when the owner typed it for the list
/// shown; else nothing (a search a script or an agent put in the box is not looked up).
fn search_text(app: &MailApp) -> String {
    let typed = one_line(&app.session.view.people_search, MAX_RESOLVE_TEXT);
    let owner = app.address_book.as_ref().map(|book| one_line(&book.owner, MAX_RESOLVE_TEXT));
    if owner.as_deref() == Some(typed.as_str()) { typed } else { String::new() }
}

impl MailApp {
    /// The account whose address list People shows now, when it is on screen.
    pub(crate) fn address_book_shown(&self) -> Option<Id> {
        (self.session.view.module == Module::People).then(|| people::shown_book(&self.session)).flatten()
    }

    /// `text` typed in People's search box: the contacts are searched for it, and an address list
    /// that is shown is asked for it when the owner typed it (not the control channel's input).
    pub fn type_people_search(&mut self, text: &str) {
        if !self.driven
            && let Some(account) = self.address_book_shown()
        {
            let opening = self.session.openings();
            let book = self.address_book.get_or_insert_with(|| AddressBook::new(account, opening));
            if book.account != account || book.opening != opening {
                *book = AddressBook::new(account, opening);
            }
            // Something else typed: who was found for what was there before goes at once, before
            // the list and the card are drawn again, and so does a look-up still out for it.
            if book.owner != text {
                book.clear();
            }
            book.owner = text.to_string();
        }
        let _ = self.execute("people.search", json!({"query": text}));
    }

    /// Who People's Address Book shows, while it is shown.
    pub fn address_book_found(&self) -> &[DirectoryPerson] {
        self.address_book.as_ref().map_or(&[], |b| b.people.as_slice())
    }

    /// Each frame: what the search box holds is looked up once typing has paused, and an answer
    /// that has come is taken in. Away from it, it is let go of.
    pub fn poll_address_book(&mut self, ctx: &egui::Context) {
        let Some(account) = self.address_book_shown() else {
            self.address_book = None;
            return;
        };
        let opening = self.session.openings();
        let now = crate::now_ms();
        if self.address_book.as_ref().is_none_or(|book| book.account != account || book.opening != opening) {
            self.address_book = Some(AddressBook::new(account, opening));
        }
        // The box holds something else than the owner typed (a script changed it): what they typed
        // no longer counts.
        if let Some(book) = &mut self.address_book
            && one_line(&book.owner, MAX_RESOLVE_TEXT) != one_line(&self.session.view.people_search, MAX_RESOLVE_TEXT)
        {
            book.owner.clear();
        }
        let typed = search_text(self);
        let ready = match self.address_book.as_ref().and_then(|book| book.blocked) {
            Some(Blocked::Offline) => !self.offline(),
            Some(Blocked::Password) => self.ews_secret(account).is_some(),
            None => false,
        };
        let mut refused = None;
        let Some(book) = &mut self.address_book else { return };
        // Something else in the box (a script changed it, or the owner typed): what was found for,
        // or is being asked for, what was there before goes, and an answer for it that has come
        // is not taken in, so that nobody found for that is written to or kept for this.
        if typed != book.typed.0 {
            book.typed = (typed.clone(), now);
            book.clear();
        }
        if let Some(pending) = &book.pending {
            match pending.rx.try_recv() {
                Ok(result) => {
                    let query = pending.query.clone();
                    refused = result.auth_refused.then(|| pending.used.clone());
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
                    book.blocked = result.auth_refused.then_some(Blocked::Password);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    book.error = Some(tl!("Searching the address list stopped unexpectedly").to_string());
                    book.query = pending.query.clone();
                    book.pending = None;
                }
            }
        }
        // What kept it from asking has gone (Work Offline is off, the password was typed): what is
        // typed is asked for again.
        if ready && book.pending.is_none() {
            (book.blocked, book.error) = (None, None);
            book.query.clear();
        }
        if let Some(used) = refused {
            self.ews_password_refused_with(account, &used);
        }
        let Some(book) = &mut self.address_book else { return };
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
        let started = self.start_address_book(account, &typed);
        if let Some(book) = &mut self.address_book {
            match started {
                Ok(pending) => book.pending = Some(pending),
                Err((error, blocked)) => {
                    // Said, and not asked again until something else is typed, or what kept it
                    // from asking has gone.
                    (book.query, book.people, book.more, book.selected, book.pending) = (typed, Vec::new(), false, None, None);
                    (book.error, book.blocked) = (Some(error), blocked);
                }
            }
        }
    }

    /// The look-up of `typed`, or what keeps it from being asked, and what that is when it can go
    /// by itself (working offline, no password).
    fn start_address_book(&mut self, account: Id, typed: &str) -> Result<Pending, (String, Option<Blocked>)> {
        if self.offline() {
            return Err((crate::send_receive::said(), Some(Blocked::Offline)));
        }
        let request = ews_resolve_request_for(&self.session, account, &[typed.to_string()]).map_err(|e| (tl!(&e.to_string()).to_string(), None))?;
        let Some(resolve) = &self.services.ews_resolve else {
            return Err((tl!("This version of MailCraft can't search an address list").to_string(), None));
        };
        let Some(secret) = self.ews_secret(account) else {
            return Err((tl!("The account's password is needed first: press Send/Receive and type it").to_string(), Some(Blocked::Password)));
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let used = secret.clone();
        let rx = resolve(EwsResolveJob { request, secret, cancel: cancel.clone() });
        Ok(Pending { query: typed.to_string(), rx, cancel, used })
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
        /// One of an account's contacts folders alone (#1917).
        Folder(Id),
        List(Id),
    }
    let label = |book: Book| match book {
        Book::Folder(folder) => crate::people_view::folder_name(mailbox, folder),
        Book::Contacts(_) if accounts.is_empty() => tl!("Contacts").to_string(),
        Book::Contacts(AccountChoice::All) => tl!("All accounts").to_string(),
        Book::Contacts(AccountChoice::Here) => widgets::place_name(mailbox, 0),
        Book::Contacts(AccountChoice::Account(account)) => widgets::place_name(mailbox, account),
        Book::List(account) => crate::i18n::fmt(tl!("Address list · {account}"), &[("account", &widgets::place_name(mailbox, account))]),
    };
    let current = match (people::shown_book(&app.session), people::shown_folder(&app.session)) {
        (Some(account), _) => Book::List(account),
        (None, Some(folder)) => Book::Folder(folder),
        (None, None) => Book::Contacts(people::shown_account(&app.session)),
    };
    let contacts: Vec<Book> = if accounts.is_empty() {
        vec![Book::Contacts(AccountChoice::All)]
    } else {
        // Each account, with its contacts folders under it, as a list of folders shows them.
        let mut books = vec![Book::Contacts(AccountChoice::All)];
        for account in accounts.iter().copied() {
            books.push(Book::Contacts(AccountChoice::Account(account)));
            books.extend(contact_folders(mailbox, account).into_iter().map(Book::Folder));
        }
        books.push(Book::Contacts(AccountChoice::Here));
        books
    };
    let read_only = |book: Book| matches!(book, Book::Folder(f) if mailbox.contact_folder(f).is_some_and(|f| !f.writable));
    let mut picked = None;
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        let width = (ui.available_width() - 10.0).max(60.0);
        egui::ComboBox::from_id_salt("people_book").width(width).height(480.0).selected_text(label(current)).truncate().show_ui(ui, |ui| {
            for book in contacts.iter().copied().chain(lists.iter().copied().map(Book::List)) {
                let folder = matches!(book, Book::Folder(_));
                let clicked = ui
                    .horizontal(|ui| {
                        // A folder sits under its account; one that can only be read has a lock.
                        if folder {
                            ui.add_space(16.0);
                        }
                        let r = ui.selectable_label(book == current, label(book));
                        if read_only(book) {
                            let t = Tokens::get(ui.ctx());
                            let (lock, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                            icons::paint(ui.painter(), lock, "lock", t.text_dim, t.accent);
                            r.clone().on_hover_text(tl!("Can only be read"));
                        }
                        r.clicked()
                    })
                    .inner;
                if clicked {
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
        Some(Book::Folder(folder)) => {
            let _ = app.run("view.peopleFolder", json!({"value": folder}));
        }
        Some(Book::List(account)) => choose_address_list(app, account),
        None => {}
    }
}

/// The contacts folders of `account`, as a list of folders shows them: its own main folder
/// first, its other own folders, then shared mailboxes' and people's, each by name.
pub(crate) fn contact_folders(mailbox: &mailcraft_model::Mailbox, account: Id) -> Vec<Id> {
    use mailcraft_model::contact_sync::FolderOwner;
    let mut folders: Vec<_> = mailbox.contact_folders.iter().filter(|f| f.account == account).collect();
    let rank = |owner: &FolderOwner| match owner {
        FolderOwner::Own => 0,
        FolderOwner::Mailbox(_) => 1,
        FolderOwner::Person(_) => 2,
    };
    folders.sort_by_cached_key(|f| (rank(&f.owner), f.owner.address().map(str::to_lowercase), !f.is_main(), f.name.to_lowercase(), f.id));
    folders.into_iter().map(|f| f.id).collect()
}

/// The owner chose `account`'s address list in People: what the search box holds as they choose
/// is looked up in it.
pub(crate) fn choose_address_list(app: &mut MailApp, account: Id) {
    if app.run("people.addressBook", json!({"account": account})).is_ok() {
        let mut book = AddressBook::new(account, app.session.openings());
        book.owner = app.session.view.people_search.clone();
        app.address_book = Some(book);
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
    // What there is to say in place of a list.
    let said = if typed.chars().count() < MIN_CHARS {
        Some((tl!("Type a name in the search box to search the organisation's address list").to_string(), "search"))
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
    let (people, more, selected) = (book.people.clone(), book.more, book.selected);
    let mut clicked = None;
    let mut write = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("address_list").show(ui, |ui| {
        for (index, person) in people.iter().enumerate() {
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
                write = Some(index);
            }
        }
        if more {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new(crate::i18n::fmt(
                        tl!("Only the first {n} are shown: type more of the name to find somebody else."),
                        &[("n", &people.len().to_string())],
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
    if let Some(person) = write.and_then(|i| people.get(i)) {
        new_email(app, person);
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

/// A contact on this computer made of `person`, on the owner's say-so.
fn add_to_contacts(app: &mut MailApp, person: &DirectoryPerson) {
    let mut phones = Vec::new();
    for (label, number) in [("work", &person.phone), ("mobile", &person.mobile)] {
        if !number.is_empty() {
            phones.push(json!({"label": label, "value": number}));
        }
    }
    let params = json!({
        "displayName": person.name, "email": person.email, "company": person.company,
        "department": person.department, "title": person.title, "phones": phones,
    });
    match app.run("people.new", params) {
        // (`people.new` shows the new contact: People goes back to the contacts.)
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
                if widgets::icon_text_button(ui, "newContact", tl!("Add to Contacts"), true).clicked() {
                    add_to_contacts(app, &person);
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
            ui.add_space(14.0);
            let from = crate::i18n::fmt(
                tl!("From the organisation's address list of {account}. Shown while you look, and not kept: Add to Contacts keeps a copy on this computer."),
                &[("account", &widgets::place_name(&app.session.mailbox, account))],
            );
            ui.add(egui::Label::new(RichText::new(from).font(regular(11.5)).color(t.text_dim)).wrap());
        });
    });
}

#[cfg(test)]
#[path = "address_book_tests.rs"]
mod address_book_tests;
