// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Add Account: the wizard that sets an account up from its address, and the page that changes
//! the settings of one that exists.
//!
//! The owner types an address and presses Connect. The address's domain says whose servers it
//! uses or, for a company's own domain, its mail exchangers do (the host looks them up). A
//! Microsoft account then signs in on Microsoft's page in the browser; any other gives its
//! password, which the host tries on the servers before the account is added.
//!
//! The wizard is a small state machine: a [`Page`], what was collected so far, and what the host
//! still owes an answer for. It only collects input: what it does, it does by running commands
//! (`account.preset`, `account.add`, `account.edit`, `ui.microsoftClientId`, `ui.sync`), and the
//! functions that say which command a page runs need no window.
//!
//! A sign-in on Microsoft's page may end on a page of Microsoft's that says the organisation
//! has to approve MailCraft first, and nothing may come back from it. The owner is never left
//! with a page that only waits: what to do next is said while the browser has the sign-in, on
//! a page of its own, and whenever a sign-in ends without having signed anybody in (`help`).

mod help;

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use egui::{Align, Align2, Key, Layout, Margin, Rect, Response, RichText, Role, Sense, Stroke, StrokeKind, TextEdit, Ui, WidgetInfo, pos2, vec2};
use mailcraft_model::{Account, AccountKind, Directory, Id, KeepOffline, OAuth, OAuthProvider, Security, Server, provider};
use serde_json::{Map, Value, json};

use crate::approval::SignInEnd;
use crate::forms::kit::{Pass, body, changes, error, field_id, object, shown};
use crate::i18n::fmt;
use crate::secret_field::{self, Watch};
use crate::theme::{Tokens, medium, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

pub(crate) use help::Help;

/// The dialog's width in points.
pub const WIDTH: f32 = 520.0;
/// The most characters an address, a name or a user name takes.
const LINE: usize = 254;
/// The most characters a host name takes.
const HOST: usize = 253;
/// The most characters a password takes.
const SECRET: usize = 256;
/// Room left and right of a page's content.
const PAD: i8 = 12;
/// Room around the text inside a field.
const FIELD_MARGIN: Margin = Margin::symmetric(8, 6);

/// The pages of the wizard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    /// Type the address.
    Address,
    /// The host is asking the DNS who takes the domain's mail.
    Detecting,
    /// Pick the kind of account by hand.
    Choose,
    /// Microsoft's sign-in needs an application ID this installation doesn't have yet.
    ApplicationId,
    /// The owner is on the provider's page in the browser, or that came to nothing.
    SignIn,
    /// The organisation has to approve MailCraft before its people can sign in: why, and what
    /// to do next. Also the whole dialog for an account that exists and was refused for it.
    Approval,
    /// The owner signed in on the provider's page as somebody other than the address typed:
    /// sign in again, or set up the account that signed in (#103).
    SomeoneElse,
    /// The password, and the servers when they want a look.
    Password,
    /// The host is trying the password on the servers.
    Checking,
    /// The account exists.
    Done,
    /// The settings of an account that exists already.
    Settings,
}

/// The kinds of account the owner can pick by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Microsoft365,
    OutlookCom,
    Google,
    Yahoo,
    ICloud,
    /// Any other mail server.
    Other,
    /// A mailbox on this device, with no server.
    Local,
}

impl Kind {
    /// The kinds that have servers, in the order of their tiles.
    const HOSTED: [Kind; 6] = [Kind::Microsoft365, Kind::OutlookCom, Kind::Google, Kind::Yahoo, Kind::ICloud, Kind::Other];

    /// What `account.preset` calls the service; `None` for the two that aren't one.
    fn service(self) -> Option<&'static str> {
        match self {
            Kind::Microsoft365 => Some("microsoft365"),
            Kind::OutlookCom => Some("outlook.com"),
            Kind::Google => Some("google"),
            Kind::Yahoo => Some("yahoo"),
            Kind::ICloud => Some("icloud"),
            Kind::Other | Kind::Local => None,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Kind::Microsoft365 => "Microsoft 365",
            Kind::OutlookCom => "Outlook.com",
            Kind::Google => "Google",
            Kind::Yahoo => "Yahoo",
            Kind::ICloud => "iCloud",
            Kind::Other => "Other (IMAP)",
            Kind::Local => "This device only",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Kind::Microsoft365 => "Work or school mail",
            Kind::OutlookCom => "Also Hotmail, Live and MSN",
            Kind::Google => "Gmail and Google Workspace",
            Kind::Yahoo => "Yahoo Mail",
            Kind::ICloud => "icloud.com, me.com and mac.com",
            Kind::Other => "Any other mail server",
            Kind::Local => "A mailbox kept here, with no mail server",
        }
    }

    /// Our own glyphs, by what the account is for: never a provider's mark.
    fn icon(self) -> &'static str {
        match self {
            Kind::Microsoft365 => "work",
            Kind::OutlookCom | Kind::Google | Kind::Yahoo | Kind::ICloud => "mail",
            Kind::Other => "server",
            Kind::Local => "device",
        }
    }
}

/// What the owner asked of the page in view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The page's main button: Connect, Continue, Try again, Done, Save.
    Go,
    /// Back, or Cancel while something is awaited.
    Back,
    /// A tile of the Choose page.
    Pick(Kind),
    /// Set the account up by hand after all.
    Manual,
    /// Sign in again, asking for mail alone (without the calendar and contacts).
    MailOnly,
    /// Start over for one more account.
    Another,
    /// Show what to do when the organisation has to approve MailCraft first.
    Approval,
    /// "I'm an administrator": open Microsoft's approval page in the browser.
    Administrator,
    /// Copy a message for an administrator.
    CopyMessage,
    /// "Not now": close, leaving nothing behind.
    NotNow,
    /// Set up the account that signed in, rather than the address that was typed.
    UseSignedIn,
}

/// How setting the account up ended, for the last page to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// The first Send/Receive is running.
    Downloading,
    /// It has servers, but its first Send/Receive is still to be started.
    Later,
    /// It has servers, and this build can't reach any.
    Offline,
    /// A mailbox on this device only.
    Local,
}

/// A password while it is being typed: never printed, and blanked when it goes.
struct Secret(String);

impl Default for Secret {
    fn default() -> Self {
        // Room for the longest password, so typing never moves the text and leaves a copy behind.
        Secret(String::with_capacity(SECRET * 4))
    }
}

impl Secret {
    /// Blank the text where it lies.
    fn wipe(&mut self) {
        let len = self.0.len();
        self.0.clear();
        self.0.extend(std::iter::repeat_n('\0', len));
        self.0.clear();
    }

    /// Hand the text over, keeping nothing.
    fn take(&mut self) -> String {
        std::mem::take(&mut self.0)
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.wipe();
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() { "" } else { "…" })
    }
}

/// Text from outside as a field takes it: one line of at most `max` characters.
fn line(text: &str, max: usize) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(max).collect::<String>().trim().to_string()
}

/// The address a field holds, with its domain; `None` when the text isn't an address.
pub fn address(text: &str) -> Option<(&str, &str)> {
    let text = text.trim();
    if text.len() > LINE {
        return None;
    }
    let (local, domain) = text.split_once('@')?;
    let ok = !local.is_empty() && !local.chars().any(|c| c.is_whitespace() || c.is_control()) && provider::is_domain(domain);
    ok.then_some((text, domain))
}

/// Whether `id` can be an application (client) ID: letters, digits, dots and dashes.
fn is_identifier(id: &str, max: usize) -> bool {
    id.len() <= max && id.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c))
}

/// One mail server as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerFields {
    host: String,
    /// Digits; empty for the usual port.
    port: String,
    security: Security,
}

impl ServerFields {
    /// The fields for an account's server.
    fn of(server: Option<&Server>) -> ServerFields {
        match server {
            Some(s) => {
                ServerFields { host: line(&s.host, HOST), port: if s.port == 0 { String::new() } else { s.port.to_string() }, security: s.security }
            }
            None => ServerFields::default(),
        }
    }

    /// The fields for a server as `account.preset` answers it.
    fn read(v: Option<&Value>) -> ServerFields {
        let Some(v) = v else { return ServerFields::default() };
        ServerFields {
            host: line(v.get("host").and_then(Value::as_str).unwrap_or_default(), HOST),
            port: v.get("port").and_then(Value::as_u64).filter(|p| (1..=65_535).contains(p)).map(|p| p.to_string()).unwrap_or_default(),
            security: v.get("security").and_then(Value::as_str).and_then(Security::parse).unwrap_or_default(),
        }
    }

    /// Whether the host is a name a server can have: what `account.add` takes for one.
    fn host_ok(&self) -> bool {
        let host = self.host.trim();
        !host.is_empty() && host.len() <= HOST && host.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c))
    }

    /// The port typed: 0 for an empty field (the usual port), `None` for anything that isn't one.
    fn port(&self) -> Option<u16> {
        match self.port.trim() {
            "" => Some(0),
            digits => digits.parse::<u16>().ok().filter(|p| *p != 0),
        }
    }

    /// The server as `account.add` takes it; `None` when no host was given.
    fn value(&self) -> Option<Value> {
        let host = self.host.trim();
        if host.is_empty() {
            return None;
        }
        let mut m = object(json!({"host": host, "security": self.security.code()}));
        if let Some(port) = self.port().filter(|p| *p != 0) {
            m.insert("port".into(), json!(port));
        }
        Some(Value::Object(m))
    }

    /// The server as an account keeps it; `None` when no host was given.
    fn server(&self) -> Option<Server> {
        let host = self.host.trim();
        (!host.is_empty()).then(|| Server { host: host.to_string(), port: self.port().unwrap_or(0), security: self.security })
    }
}

/// What `account.preset` answered, as far as the wizard goes by it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preset {
    /// The servers are a provider's own, not a guess made from the domain.
    known: bool,
    /// The provider's name (`Microsoft 365`); empty for a guess.
    service: String,
    /// Set when the owner signs in on the provider's page instead of with a password.
    oauth: Option<OAuthProvider>,
    username: String,
    incoming: ServerFields,
    outgoing: ServerFields,
    /// Something the owner should know before connecting; may be empty.
    note: String,
}

impl Preset {
    /// Whatever the answer holds is read; what it lacks stays empty.
    fn read(v: &Value) -> Preset {
        let text = |k: &str, max: usize| line(v.get(k).and_then(Value::as_str).unwrap_or_default(), max);
        let oauth = (v.get("auth").and_then(Value::as_str) == Some("oauth2"))
            .then(|| v.get("provider").and_then(Value::as_str).and_then(OAuthProvider::parse).unwrap_or_default());
        Preset {
            known: v.get("known").and_then(Value::as_bool).unwrap_or(false),
            service: text("service", 60),
            oauth,
            username: text("username", LINE),
            incoming: ServerFields::read(v.get("incoming")),
            outgoing: ServerFields::read(v.get("outgoing")),
            note: text("note", 300),
        }
    }
}

/// An account whose settings are being changed.
struct Editing {
    id: Id,
    /// The fields as the account was opened; Save sends what differs from them.
    baseline: Map<String, Value>,
}

/// The Add Account wizard, and Account Settings for an account that exists.
pub struct AccountSetup {
    page: Page,
    /// Google refused the password: the password page links to Google's App passwords page.
    app_passwords: bool,
    email: String,
    /// The owner's name, as recipients see it.
    name: String,
    /// Advanced options are shown.
    advanced: bool,
    /// Skip detection and ask for the kind of account.
    manual: bool,
    /// The kind was picked from the tiles, which is where Back returns to.
    picked: bool,
    /// The provider's name (`Gmail`); empty when nobody knows whose the servers are.
    service: String,
    /// What the provider's owners should know before connecting; may be empty.
    note: String,
    /// Set for an account that signs in on its provider's page instead of with a password.
    oauth: Option<OAuth>,
    username: String,
    incoming: ServerFields,
    outgoing: ServerFields,
    /// The servers are shown on the password page.
    settings_open: bool,
    /// The servers are a guess made from the domain: the page says so.
    guessed: bool,
    password: Secret,
    /// The application (client) ID Microsoft's sign-in goes by.
    client_id: String,
    /// The Microsoft directory an existing account signs in to.
    tenant: String,
    /// What went wrong, shown on the page in view.
    error: String,
    /// The page's first field takes the keyboard.
    focus: bool,
    /// The domain's mail exchangers, once the host has them.
    lookup: Option<Receiver<Result<Vec<String>, String>>>,
    /// Whether the servers took the password, once the host knows.
    check: Option<Receiver<Result<(), String>>>,
    /// The owner is in the browser, signing in for this wizard.
    waiting: bool,
    /// The owner asked to sign in for mail alone (the provider's page refused the fuller request).
    mail_only: bool,
    /// How the last sign-in ended, when it ended without signing anybody in.
    ended: Option<SignInEnd>,
    /// The page that says what to do when the organisation has to approve MailCraft first.
    approval: help::Approval,
    /// An administrator approved MailCraft from this wizard: the sign-in that follows says so.
    approved: bool,
    /// What a sign-in as somebody other than the address typed brought back, while the owner
    /// says what to do about it. Kept in memory only, and gone with the dialog.
    someone_else: Option<mailcraft_model::sync::Tokens>,
    /// The account the approval page is about, when it is one that exists (0: the one that is
    /// being set up), and its organisation's directory, when that is known.
    existing: Id,
    organisation: String,
    /// The dialog is finished: what a page did between two frames says so.
    done: bool,
    outcome: Outcome,
    /// Which kind of Microsoft account the one being added is, as its sign-in said.
    directory: Directory,
    edit: Option<Editing>,
    /// For the settings of a Microsoft account of its own: its mail to keep offline as it was,
    /// and as the slider has it now (#143). Saved through `ui.keepOffline`, which asks first
    /// when it is shorter.
    keep_offline: Option<(KeepOffline, KeepOffline)>,
    /// The settings are of an account kept in step through Microsoft Graph (R3): it has no user
    /// name or mail servers to show, and the page says how its mail comes instead (#312).
    graph: bool,
    /// The settings are of an account a newer MailCraft added: nothing of it can be changed
    /// here, and the page says why.
    newer: bool,
    /// The window's context, once a frame has run: what the password field left in its memory
    /// is cleared through it.
    ctx: Option<egui::Context>,
    /// The owner clicked the password's eye: it is shown until the next click, the next page, or
    /// the dialog's end (#438).
    reveal: bool,
}

impl Drop for AccountSetup {
    /// However the dialog goes (its buttons, Escape, the window's close button, `ui.closeDialog`),
    /// the window's memory keeps nothing of the password, and a sign-in nobody waits for any more
    /// is noted for the next wizard to give up.
    fn drop(&mut self) {
        let Some(ctx) = self.ctx.take() else { return };
        forget_field(&ctx);
        if self.waiting && self.existing == 0 {
            ctx.data_mut(|d| d.insert_temp(left_waiting(), true));
        }
    }
}

fn password_id() -> egui::Id {
    field_id(("account", "password"))
}

/// Where a wizard that went while its owner was in the browser says so.
fn left_waiting() -> egui::Id {
    egui::Id::unique("account_setup_left_waiting")
}

/// Drop what the password field keeps between frames: its undo history holds what was typed.
fn forget_field(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<egui::text_edit::TextEditState>(password_id()));
}

/// Whether the window's own close button was clicked in this pass. The dialog goes right after,
/// and nothing else tells a form so; the button is the one thing to click in the window above
/// the form.
fn closed_by_window(ui: &Ui) -> bool {
    let ctx = ui.ctx();
    let Some(clicked) = ctx.interaction_snapshot(|s| s.clicked) else { return false };
    ctx.read_response(clicked).is_some_and(|r| r.layer_id == ui.layer_id() && r.rect.bottom() <= ui.max_rect().top())
}

impl AccountSetup {
    fn blank(page: Page) -> AccountSetup {
        AccountSetup {
            page,
            email: String::new(),
            name: String::new(),
            advanced: false,
            manual: false,
            picked: false,
            app_passwords: false,
            service: String::new(),
            note: String::new(),
            oauth: None,
            username: String::new(),
            incoming: ServerFields::default(),
            outgoing: ServerFields::default(),
            settings_open: false,
            guessed: false,
            password: Secret::default(),
            client_id: String::new(),
            tenant: String::new(),
            error: String::new(),
            focus: true,
            lookup: None,
            check: None,
            waiting: false,
            mail_only: false,
            ended: None,
            approval: help::Approval::default(),
            approved: false,
            someone_else: None,
            existing: 0,
            organisation: String::new(),
            done: false,
            outcome: Outcome::Downloading,
            directory: Directory::Unknown,
            edit: None,
            keep_offline: None,
            graph: false,
            newer: false,
            ctx: None,
            reveal: false,
        }
    }

    /// The wizard, with `p.email` and `p.displayName` filled in when given; or, with `p.id`, the
    /// settings of that account. `None` for an account that isn't there, for the nameless
    /// mailbox of a new installation, which has nothing to change until an address is added, and
    /// for a shared mailbox, which has no settings of its own: its owner's sign-in reads it (#70).
    pub fn open(app: &MailApp, p: &Value) -> Option<AccountSetup> {
        match p.get("id").and_then(Value::as_u64).filter(|id| *id != 0) {
            // What to do when its organisation has to approve MailCraft first, for an account
            // that exists and signs in with Microsoft (`ui.approvalHelp`).
            Some(id) if p.get("page").and_then(Value::as_str) == Some("approval") => AccountSetup::approval_for(app, id),
            Some(id) => app.session.mailbox.account(id).filter(|a| !a.email.is_empty() && !a.is_shared()).map(AccountSetup::edit),
            None => Some(AccountSetup::new(app, p)),
        }
    }

    fn new(app: &MailApp, p: &Value) -> AccountSetup {
        let text = |k: &str| line(p.get(k).and_then(Value::as_str).unwrap_or_default(), LINE);
        let mut w = AccountSetup::blank(Page::Address);
        w.email = text("email");
        w.name = text("displayName");
        if w.name.is_empty() {
            // Whoever writes from here already most likely adds another address of their own.
            w.name = app.session.mailbox.default_account().filter(|a| !a.email.is_empty()).map(|a| line(&a.display_name, LINE)).unwrap_or_default();
        }
        w
    }

    fn edit(a: &Account) -> AccountSetup {
        let mut w = AccountSetup::blank(Page::Settings);
        w.email = line(&a.email, LINE);
        w.name = line(&a.display_name, LINE);
        w.username = line(&a.username, LINE);
        w.incoming = ServerFields::of(a.incoming.as_ref());
        w.outgoing = ServerFields::of(a.outgoing.as_ref());
        if let Some(oauth) = &a.oauth {
            w.client_id = line(&oauth.client_id, 200);
            w.tenant = line(&oauth.tenant, 64);
        }
        w.oauth = a.oauth.clone();
        w.edit = Some(Editing { id: a.id, baseline: w.fields() });
        if a.kind == AccountKind::Microsoft && !a.is_shared() {
            w.keep_offline = Some((a.keep_offline, a.keep_offline));
        }
        w.graph = a.kind == AccountKind::Microsoft;
        w.newer = a.kind == AccountKind::Unknown;
        w
    }

    /// The dialog's title, in the interface language.
    pub fn title(&self) -> String {
        if self.edit.is_some() {
            tl!("Account Settings")
        } else if self.existing != 0 {
            tl!("Approval Needed")
        } else {
            tl!("Add Account")
        }
        .to_string()
    }

    // ---- what each page runs ----

    /// `account.preset` for the address: with the domain's mail exchangers once the host has
    /// them, or with the service the owner picked. `None` while the text isn't an address.
    pub fn preset_command(&self, mx: Option<&[String]>, service: Option<&str>) -> Option<(&'static str, Value)> {
        let (email, _) = address(&self.email)?;
        let mut m = object(json!({"email": email}));
        if let Some(mx) = mx {
            m.insert("mx".into(), json!(mx.iter().take(20).map(|host| line(host, 255)).collect::<Vec<_>>()));
        }
        if let Some(service) = service {
            m.insert("service".into(), json!(service));
        }
        Some(("account.preset", Value::Object(m)))
    }

    /// `account.add` for an account on the servers the fields name. `fallback` is the owner's
    /// name as the provider gave it, used when none was typed. `None` while the address or a
    /// server can't be used.
    pub fn add_command(&self, fallback: &str) -> Option<(&'static str, Value)> {
        let (email, _) = address(&self.email)?;
        if self.settings_problem(true).is_some() {
            return None;
        }
        let name = if self.name.trim().is_empty() { line(fallback, LINE) } else { self.name.trim().to_string() };
        let mut m = object(json!({"email": email, "displayName": name, "username": self.login(email)}));
        for (key, server) in [("incoming", &self.incoming), ("outgoing", &self.outgoing)] {
            if let Some(v) = server.value() {
                m.insert(key.into(), v);
            }
        }
        match &self.oauth {
            Some(oauth) => {
                m.insert("auth".into(), json!("oauth2"));
                m.insert("provider".into(), json!(oauth.provider.code()));
                // Which kind of Microsoft account it is, once a sign-in has said.
                if let Some(kind) = self.directory.code() {
                    m.insert("directory".into(), json!(kind));
                }
            }
            None => {
                m.insert("auth".into(), json!("password"));
            }
        }
        Some(("account.add", Value::Object(m)))
    }

    /// `account.add` for a mailbox on this device only: no user name, no servers.
    pub fn local_command(&self) -> Option<(&'static str, Value)> {
        let (email, _) = address(&self.email)?;
        Some(("account.add", json!({"email": email, "displayName": self.name.trim()})))
    }

    /// `ui.microsoftClientId` with the application ID that was pasted; `None` while it isn't one.
    pub fn client_id_command(&self) -> Option<(&'static str, Value)> {
        let id = self.client_id.trim();
        (!id.is_empty() && is_identifier(id, 200)).then(|| ("ui.microsoftClientId", json!({"value": id})))
    }

    /// `account.edit` with the fields that changed; `None` when nothing did, or while a field
    /// can't be used.
    pub fn edit_command(&self) -> Option<(&'static str, Value)> {
        let edit = self.edit.as_ref()?;
        if self.settings_problem(false).is_some() {
            return None;
        }
        let mut m = changes(&edit.baseline, &self.fields(), &[]);
        if m.is_empty() {
            return None;
        }
        m.insert("id".into(), json!(edit.id));
        Some(("account.edit", Value::Object(m)))
    }

    /// The account the fields describe, for the host to try before it is added.
    pub fn account(&self) -> Option<Account> {
        let (email, _) = address(&self.email)?;
        if self.settings_problem(true).is_some() {
            return None;
        }
        Some(Account {
            name: email.to_string(),
            email: email.to_string(),
            display_name: self.name.trim().to_string(),
            kind: AccountKind::Imap,
            incoming: self.incoming.server(),
            outgoing: self.outgoing.server(),
            username: self.login(email),
            oauth: self.oauth.clone(),
            ..Default::default()
        })
    }

    /// The name to sign in with: the one typed, else the address.
    fn login(&self, email: &str) -> String {
        match self.username.trim() {
            "" => email.to_string(),
            name => name.to_string(),
        }
    }

    /// The fields Account Settings can change, as `account.edit` takes them.
    fn fields(&self) -> Map<String, Value> {
        let mut m = object(json!({
            "displayName": self.name.trim(),
            "username": self.username.trim(),
            "incoming": self.incoming.value(),
            "outgoing": self.outgoing.value(),
        }));
        if self.oauth.is_some() {
            m.insert("clientId".into(), json!(self.client_id.trim()));
            m.insert("tenant".into(), json!(self.tenant.trim()));
        }
        m
    }

    /// What keeps the servers from being used, in the interface language; nothing when they can be. A new
    /// account needs its incoming server (`connected`); an existing one may have none.
    fn settings_problem(&self, connected: bool) -> Option<String> {
        let (incoming, outgoing) = (&self.incoming, &self.outgoing);
        if incoming.host.trim().is_empty() {
            if connected {
                return Some(tl!("Type the name of the incoming mail server").to_string());
            }
        } else if !incoming.host_ok() {
            return Some(tl!("The incoming mail server isn't a host name (letters, digits, dots and dashes)").to_string());
        }
        if !outgoing.host.trim().is_empty() && !outgoing.host_ok() {
            return Some(tl!("The outgoing mail server isn't a host name (letters, digits, dots and dashes)").to_string());
        }
        if incoming.port().is_none() || outgoing.port().is_none() {
            return Some(tl!("A port is a number from 1 to 65535").to_string());
        }
        if !incoming.host.trim().is_empty() && mailcraft_model::pop::looks_like_pop(&incoming.host, incoming.port().unwrap_or(0)) {
            return Some(
                tl!("MailCraft doesn't support POP yet. Use your provider's IMAP server instead (usually imap.<your domain>, port 993).").to_string(),
            );
        }
        if self.edit.is_some() && self.oauth.is_some() && !(is_identifier(self.client_id.trim(), 200) && is_identifier(self.tenant.trim(), 64)) {
            return Some(tl!("An application ID and a tenant are letters, digits, dots and dashes").to_string());
        }
        None
    }

    // ---- moving between pages ----

    fn go(&mut self, page: Page) {
        self.page = page;
        // A password shown on one page is dots on the next.
        self.reveal = false;
        self.error.clear();
        self.app_passwords = false;
        self.focus = true;
    }

    /// The page Back returns to from the password or the application ID.
    fn origin(&self) -> Page {
        if self.picked { Page::Choose } else { Page::Address }
    }

    /// Take over what `account.preset` answered.
    fn take(&mut self, preset: Preset) {
        self.service = preset.service;
        self.note = preset.note;
        self.oauth = preset.oauth.map(|provider| OAuth { provider, ..Default::default() });
        self.username = preset.username;
        self.incoming = preset.incoming;
        self.outgoing = preset.outgoing;
        self.settings_open = false;
        self.guessed = false;
    }

    /// What is said of text that isn't an address, however long it is.
    fn not_an_address(&self) -> String {
        fmt(tl!("\"{address}\" isn't an e-mail address"), &[("address", &shown(self.email.trim(), 80))])
    }

    /// Do what the owner asked of the page in view. Returns `true` when the dialog should close.
    pub fn act(&mut self, app: &mut MailApp, action: Action) -> bool {
        match (self.page, action) {
            (Page::Address, Action::Go) => self.connect(app),
            (Page::Detecting, Action::Back) => {
                self.lookup = None;
                self.go(Page::Address);
            }
            (Page::Choose, Action::Pick(kind)) => self.pick(app, kind),
            (Page::Choose, Action::Back) => self.go(Page::Address),
            (Page::ApplicationId, Action::Go) => self.use_client_id(app),
            (Page::ApplicationId, Action::Back) => self.go(self.origin()),
            // "Try again", and "Open the page again" while one is still waiting.
            (Page::SignIn, Action::Go) => {
                self.stop_sign_in(app);
                self.start_sign_in(app);
            }
            (Page::SignIn, Action::Back) => {
                self.stop_sign_in(app);
                self.go(Page::Address);
            }
            // The sign-in that is waiting goes on waiting: the owner may yet come back from it.
            (Page::SignIn, Action::Approval) => self.open_approval(false),
            (Page::SignIn | Page::Approval, Action::Administrator) => {
                if let Err(e) = self.ask_administrator(app) {
                    self.error = e;
                }
            }
            (Page::SignIn | Page::Approval, Action::CopyMessage) => match self.copy_message(app) {
                Ok(_) => self.error.clear(),
                Err(e) => self.error = e,
            },
            (Page::Approval, Action::Go) => {
                if let Err(e) = self.again(app) {
                    self.error = e;
                }
                return self.done;
            }
            // Cancel, while Microsoft's approval page is open: back to what there is to do.
            (Page::Approval, Action::Back) => {
                app.cancel_admin_consent();
                self.approval.approving = false;
            }
            (Page::Approval, Action::NotNow) => {
                self.give_up(app);
                return true;
            }
            (Page::SignIn, Action::MailOnly) => {
                self.stop_sign_in(app);
                self.mail_only = true;
                self.start_sign_in(app);
            }
            (Page::SignIn, Action::Manual) => {
                self.stop_sign_in(app);
                self.go(Page::Choose);
            }
            // "Sign in again": for the address typed; Microsoft's page asks who, every time.
            (Page::SomeoneElse, Action::Go) => {
                self.someone_else = None;
                self.start_sign_in(app);
            }
            (Page::SomeoneElse, Action::UseSignedIn) => self.use_signed_in(app),
            (Page::Password, Action::Go) => self.try_password(app),
            (Page::Password, Action::Back) => {
                self.forget_password();
                self.go(self.origin());
            }
            (Page::Checking, Action::Back) => {
                self.check = None;
                self.go(Page::Password);
            }
            (Page::Done, Action::Go) | (Page::Settings, Action::Back) => return true,
            (Page::Done, Action::Another) => *self = AccountSetup::new(app, &Value::Null),
            (Page::Settings, Action::Go) => return self.save(app),
            _ => {}
        }
        false
    }

    /// Connect on the first page: find out whose the address is, or ask, and move on.
    fn connect(&mut self, app: &mut MailApp) {
        self.picked = false;
        let Some((command, params)) = self.preset_command(None, None) else {
            self.error = self.not_an_address();
            return;
        };
        if self.manual {
            self.go(Page::Choose);
            return;
        }
        let preset = match app.execute(command, params) {
            Ok(v) => Preset::read(&v),
            Err(e) => {
                self.error = e;
                return;
            }
        };
        let known = preset.known;
        self.take(preset);
        if known {
            self.proceed(app);
            return;
        }
        // A domain nobody here has heard of: the servers are a guess for the owner to check,
        // unless the domain's mail exchangers say whose it is.
        (self.guessed, self.settings_open) = (true, true);
        let domain = address(&self.email).map(|(_, domain)| domain.to_ascii_lowercase());
        match (&app.services.lookup_mx, domain) {
            (Some(lookup), Some(domain)) => {
                self.lookup = Some(lookup(domain));
                self.go(Page::Detecting);
            }
            _ => self.go(Page::Password),
        }
    }

    /// The servers are settled: on to signing in, the way the account does it.
    fn proceed(&mut self, app: &mut MailApp) {
        let Some(oauth) = self.oauth.clone() else {
            self.go(Page::Password);
            return;
        };
        // Without an application ID Microsoft shows no sign-in page. A build that can't sign in
        // at all says that instead of asking for one.
        if app.services.sign_in.is_some() && app.effective_oauth(oauth).client_id.is_empty() {
            self.client_id.clear();
            self.go(Page::ApplicationId);
        } else {
            self.start_sign_in(app);
        }
    }

    /// A tile of the Choose page.
    fn pick(&mut self, app: &mut MailApp, kind: Kind) {
        self.picked = true;
        let command = if kind == Kind::Local { self.local_command() } else { self.preset_command(None, kind.service()) };
        let answer = match command {
            Some((command, params)) => app.execute(command, params),
            None => Err(self.not_an_address()),
        };
        let v = match answer {
            Ok(v) => v,
            Err(e) => {
                self.error = e;
                return;
            }
        };
        if kind == Kind::Local {
            self.outcome = Outcome::Local;
            self.go(Page::Done);
            return;
        }
        let mut preset = Preset::read(&v);
        if kind == Kind::Other {
            // Whatever the domain suggests, another server takes a password, and its name wants
            // a look.
            (preset.oauth, preset.service, preset.note) = (None, String::new(), String::new());
        }
        self.take(preset);
        self.settings_open = kind == Kind::Other;
        self.proceed(app);
    }

    /// Continue on the application ID page: keep the ID, then sign in with it.
    fn use_client_id(&mut self, app: &mut MailApp) {
        let Some((command, params)) = self.client_id_command() else {
            self.error = tl!("An application ID is letters, digits, dots and dashes").to_string();
            return;
        };
        match app.execute(command, params) {
            Ok(_) => self.start_sign_in(app),
            Err(e) => self.error = e,
        }
    }

    /// Send the owner to the provider's page.
    fn start_sign_in(&mut self, app: &mut MailApp) {
        let Some(oauth) = self.oauth.clone() else { return };
        self.go(Page::SignIn);
        self.ended = None;
        match app.begin_setup_sign_in(oauth, self.email.trim(), self.mail_only) {
            Ok(()) => self.waiting = true,
            Err(e) => self.error = e,
        }
    }

    /// Give up the sign-in this wizard is waiting for, if it is.
    fn stop_sign_in(&mut self, app: &mut MailApp) {
        if !std::mem::take(&mut self.waiting) {
            return;
        }
        // An answer that came in this very pass is dropped with the rest.
        if app.take_setup_sign_in().is_none() && app.signing_in() {
            let _ = app.execute("ui.cancelSignIn", json!({}));
        }
    }

    /// Connect on the password page: have the host try the account, or add it untried where
    /// nothing can.
    fn try_password(&mut self, app: &mut MailApp) {
        let Some(account) = self.account() else {
            self.error = self.settings_problem(true).unwrap_or_else(|| tl!("Check the account's settings").to_string());
            self.settings_open = true;
            return;
        };
        // The field goes out of view; what it kept between frames goes with it.
        if let Some(ctx) = &self.ctx {
            forget_field(ctx);
        }
        match &app.services.test_account {
            Some(test) => {
                self.check = Some(test(account, self.password.0.clone()));
                self.go(Page::Checking);
            }
            None => self.add_with_password(app),
        }
    }

    /// Add the account the password page describes, and keep its password for this session.
    fn add_with_password(&mut self, app: &mut MailApp) {
        let added = match self.add_command("") {
            Some((command, params)) => app.execute(command, params),
            None => Err(tl!("Check the account's settings").to_string()),
        };
        match added.map(|v| v.get("id").and_then(Value::as_u64)) {
            Ok(Some(id)) => {
                let password = self.password.take();
                if !password.is_empty() {
                    app.adopt_password(id, password);
                }
                self.finish(app, id);
            }
            Ok(None) => self.refuse(tl!("The account couldn't be added").to_string()),
            Err(e) => self.refuse(e),
        }
    }

    /// Back on the password page, with what went wrong and the servers in view.
    fn refuse(&mut self, why: String) {
        self.go(Page::Password);
        self.error = why;
        self.settings_open = true;
    }

    /// Add the account the owner just signed in to, with the tokens that brought back. One that
    /// signed in as somebody other than the address typed is not added: the owner is asked
    /// whether to sign in again or to set up the account that signed in (#103).
    fn add_signed_in(&mut self, app: &mut MailApp, tokens: mailcraft_model::sync::Tokens) {
        if tokens.someone_else(self.email.trim(), "") {
            self.someone_else = Some(tokens);
            self.go(Page::SomeoneElse);
            return;
        }
        // Which kind of Microsoft account it is: the directory the sign-in named says; without
        // that, whose servers the address was found to use (work or school mail, or Outlook.com).
        self.directory = match Directory::of_tenant(&tokens.tenant) {
            Directory::Unknown if self.service == "Microsoft 365" => Directory::Organisation,
            Directory::Unknown if self.service == "Outlook.com" => Directory::Personal,
            said => said,
        };
        let added = match self.add_command(&tokens.name) {
            Some((command, mut params)) => {
                // A Microsoft account is kept in step through Microsoft Graph, without mail servers
                // (R3), unless the owner asked for mail only (that is IMAP's, until R3.8).
                if !self.mail_only && self.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft) {
                    params["kind"] = json!("microsoft");
                }
                app.execute(command, params)
            }
            None => Err(tl!("Check the account's settings").to_string()),
        };
        match added.map(|v| v.get("id").and_then(Value::as_u64)) {
            Ok(Some(id)) => {
                app.adopt_tokens(id, tokens);
                self.finish(app, id);
            }
            Ok(None) => self.error = tl!("The account couldn't be added").to_string(),
            Err(e) => self.error = e,
        }
    }

    /// "Use …" on the page about a sign-in as somebody else: the account that is set up is the
    /// one that signed in, by its own address and the name the provider gave. Should that not be
    /// added (it is set up already, say), the page stays, saying why, with the address typed.
    fn use_signed_in(&mut self, app: &mut MailApp) {
        let Some(tokens) = self.someone_else.take() else { return };
        let who = line(&tokens.email, LINE);
        if address(&who).is_none() {
            self.error = fmt(tl!("\"{address}\" isn't an e-mail address"), &[("address", &shown(&who, 80))]);
            return;
        }
        let typed = (self.email.clone(), self.name.clone(), self.username.clone());
        (self.email, self.name) = (who.clone(), line(&tokens.name, LINE));
        // The servers know the owner by the address that signed in.
        if !self.username.trim().is_empty() {
            self.username = who;
        }
        let kept = tokens.clone();
        self.add_signed_in(app, tokens);
        if self.page != Page::Done {
            let error = std::mem::take(&mut self.error);
            (self.email, self.name, self.username) = typed;
            self.someone_else = Some(kept);
            self.go(Page::SomeoneElse);
            self.error = error;
        }
    }

    /// The account exists: start its first download, and say how things stand.
    fn finish(&mut self, app: &mut MailApp, id: Id) {
        self.outcome = match app.execute("ui.sync", json!({"account": id})) {
            Ok(_) => Outcome::Downloading,
            Err(_) if app.services.sync.is_none() => Outcome::Offline,
            Err(_) => Outcome::Later,
        };
        self.go(Page::Done);
    }

    /// Save on Account Settings: run `account.edit` with what changed. `true` when it is done.
    fn save(&mut self, app: &mut MailApp) -> bool {
        if let Some(problem) = self.settings_problem(false) {
            self.error = problem;
            return false;
        }
        let Some(id) = self.edit.as_ref().map(|e| e.id) else { return true };
        if let Some((command, params)) = self.edit_command()
            && let Err(e) = app.execute(command, params)
        {
            self.error = e;
            return false;
        }
        // A window of mail kept offline: a shorter one asks first, in a question of its own (#143).
        if let Some(keep) = self.keep_offline.filter(|(was, keep)| was != keep).map(|(_, keep)| keep) {
            self.keep_offline = Some((keep, keep));
            let _ = app.run("ui.keepOffline", json!({"account": id, "keepOffline": keep.code()}));
        }
        // A new password is for this session's Send/Receive, like one typed when it is asked for.
        let password = self.password.take();
        if !password.is_empty() && self.oauth.is_none() {
            app.adopt_password(id, password);
        }
        true
    }

    // ---- what the host answered ----

    /// Take in what came back since the last frame.
    fn poll(&mut self, app: &mut MailApp) {
        match self.page {
            Page::Detecting => self.poll_lookup(app),
            Page::SignIn if self.waiting => self.poll_sign_in(app),
            // (A sign-in may still be waiting behind the approval page: the owner may sign in
            // with an administrator's account on Microsoft's page after all.)
            Page::Approval => {
                if self.waiting {
                    self.poll_sign_in(app);
                }
                if self.page == Page::Approval {
                    self.poll_approval(app);
                }
            }
            Page::Checking => self.poll_check(app),
            _ => {}
        }
    }

    fn poll_lookup(&mut self, app: &mut MailApp) {
        let answer = match self.lookup.as_ref().map(Receiver::try_recv) {
            Some(Err(TryRecvError::Empty)) => return,
            Some(Ok(answer)) => answer,
            // The host gave up, or nobody was asked.
            Some(Err(TryRecvError::Disconnected)) | None => Err(String::new()),
        };
        self.lookup = None;
        let found = answer
            .ok()
            .and_then(|mx| self.preset_command(Some(&mx), None))
            .and_then(|(command, params)| app.execute(command, params).ok())
            .map(|v| Preset::read(&v))
            .filter(|preset| preset.known);
        match found {
            Some(preset) => {
                self.take(preset);
                self.proceed(app);
            }
            // The guess made from the domain stays, for the owner to check.
            None => self.go(Page::Password),
        }
    }

    fn poll_sign_in(&mut self, app: &mut MailApp) {
        match app.take_setup_sign_in() {
            Some(Ok(tokens)) => {
                self.waiting = false;
                self.add_signed_in(app, tokens);
            }
            Some(Err(e)) => {
                self.waiting = false;
                self.sign_in_ended(e);
            }
            // Given up from elsewhere (Cancel Sign-In): back to the start, as Cancel here does.
            // (From the approval page there is nowhere to go back to: it says what to do.)
            None if !app.signing_in() => {
                self.waiting = false;
                if self.page == Page::SignIn {
                    self.go(Page::Address);
                }
            }
            None => {}
        }
    }

    fn poll_check(&mut self, app: &mut MailApp) {
        let answer = match self.check.as_ref().map(Receiver::try_recv) {
            Some(Err(TryRecvError::Empty)) => return,
            Some(Ok(answer)) => answer,
            Some(Err(TryRecvError::Disconnected)) | None => Err(tl!("Checking the account stopped unexpectedly").to_string()),
        };
        self.check = None;
        match answer {
            Ok(()) => self.add_with_password(app),
            Err(e) => {
                // The dialog says it once; the log keeps it for a problem report.
                log::warn!("add account: the servers couldn't be checked: {e}");
                let google = self.incoming.server().is_some_and(|s| is_google(&s.host));
                self.refuse(e);
                if google && self.error.starts_with("sign-in failed") {
                    self.error = tl!("Google didn't accept the password. An account with 2-Step Verification needs an app password: make one on Google's App passwords page, then paste it here in place of your usual password.").to_string();
                    self.app_passwords = true;
                }
            }
        }
    }

    /// Whether the host owes an answer: frames keep coming until it is in.
    fn busy(&self) -> bool {
        self.lookup.is_some() || self.check.is_some() || self.waiting || self.approval.approving
    }

    /// Whether the dialog is finished, by what one of its pages did outside a frame (a command
    /// that ran on it): whoever holds it lets it go.
    pub(crate) fn finished(&self) -> bool {
        self.done
    }

    // ---- leaving ----

    /// Blank the password, in the form and in what its field kept between frames.
    fn forget_password(&mut self) {
        self.password.wipe();
        self.reveal = false;
        if let Some(ctx) = &self.ctx {
            forget_field(ctx);
        }
    }

    /// The dialog is closing: stop what is under way and keep nothing that was typed in confidence.
    fn give_up(&mut self, app: &mut MailApp) {
        (self.lookup, self.check) = (None, None);
        self.stop_sign_in(app);
        // An administrator's approval that Microsoft's page is open for is given up with it.
        if std::mem::take(&mut self.approval.approving) {
            app.cancel_admin_consent();
        }
        self.forget_password();
    }

    /// A wizard that went while its owner was in the browser, in a way it couldn't see, left that
    /// sign-in waiting: give it up, so the next one can start.
    fn sweep(&mut self, app: &mut MailApp) {
        let Some(ctx) = &self.ctx else { return };
        let left = ctx.data_mut(|d| d.remove_temp::<bool>(left_waiting())).unwrap_or(false);
        if left && app.take_setup_sign_in().is_none() && app.signing_in() {
            let _ = app.execute("ui.cancelSignIn", json!({}));
        }
    }

    // ---- the pages ----

    /// Lay out the page in view. Returns `true` when the dialog should close.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        if self.ctx.is_none() {
            self.ctx = Some(ui.ctx().clone());
            self.sweep(app);
        }
        // Escape and the window's own close button close the dialog after this pass.
        if ui.input(|i| i.key_pressed(Key::Escape)) || closed_by_window(ui) {
            self.give_up(app);
            return true;
        }
        self.poll(app);
        if self.done {
            return true;
        }
        if self.busy() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        let mut pass = Pass::new(ui, &app.session, &mut self.focus);
        let action = match self.page {
            Page::Address => self.address_page(ui, &mut pass),
            Page::Detecting => waiting_page(ui, &pass, self.page, tl!("Finding your account settings…"), &shown(self.email.trim(), 120)),
            Page::Choose => self.choose_page(ui, &pass),
            Page::ApplicationId => self.application_id_page(ui, &mut pass),
            Page::SignIn if self.waiting => self.signing_in_page(app, ui, &pass),
            Page::SignIn => self.sign_in_ended_page(app, ui, &pass),
            Page::Approval => self.approval_page(app, ui, &pass),
            Page::SomeoneElse => self.someone_else_page(ui, &pass),
            Page::Password => self.password_page(app, ui, &mut pass),
            Page::Checking => {
                let server = shown(self.incoming.host.trim(), 120);
                waiting_page(ui, &pass, self.page, tl!("Checking the account…"), &fmt(tl!("Signing in to {server}"), &[("server", &server)]))
            }
            Page::Done => self.done_page(ui, &pass),
            Page::Settings => self.settings_page(app, ui, &mut pass),
        };
        action.is_some_and(|action| self.act(app, action))
    }

    fn address_page(&mut self, ui: &mut Ui, pass: &mut Pass) -> Option<Action> {
        let t = pass.t;
        page(ui, &t, |ui| {
            heading(ui, tl!("Add an account"), &t);
            note(ui, tl!("Type your address. MailCraft finds your mail servers and how you sign in."), t.text_dim);
            if field(ui, pass, "email", tl!("Email address"), &mut self.email, tl!("you@example.org"), LINE).changed() {
                self.error.clear();
            }
            error(ui, &self.error);
            ui.add_space(2.0);
            disclosure(ui, tl!("Advanced options"), &mut self.advanced, &t);
            if self.advanced {
                indented(ui, |ui| {
                    ui.checkbox(&mut self.manual, tl!("Let me set up my account manually"));
                    field(ui, pass, "name", tl!("Your name"), &mut self.name, tl!("As the people you write to see it"), LINE);
                });
            }
        });
        let ready = !self.email.trim().is_empty();
        let mut go = false;
        strip(ui, self.page, |ui| go = button(ui, tl!("Connect"), true, ready).clicked());
        (ready && (go || entered(ui, pass))).then_some(Action::Go)
    }

    fn choose_page(&mut self, ui: &mut Ui, pass: &Pass) -> Option<Action> {
        let t = pass.t;
        let mut action = None;
        page(ui, &t, |ui| {
            heading(ui, tl!("Choose the kind of account"), &t);
            note(ui, &shown(self.email.trim(), 120), t.text_dim);
            ui.add_space(4.0);
            let gap = ui.spacing().item_spacing.x;
            let half = ((ui.available_width() - gap) / 2.0).max(40.0);
            for pair in Kind::HOSTED.chunks(2) {
                ui.horizontal(|ui| {
                    for kind in pair {
                        if tile(ui, *kind, half, &t).clicked() {
                            action = Some(Action::Pick(*kind));
                        }
                    }
                });
            }
            let whole = ui.available_width().max(40.0);
            if tile(ui, Kind::Local, whole, &t).clicked() {
                action = Some(Action::Pick(Kind::Local));
            }
            error(ui, &self.error);
        });
        strip(ui, self.page, |ui| {
            if button(ui, tl!("Back"), false, true).clicked() {
                action = Some(Action::Back);
            }
        });
        action
    }

    fn application_id_page(&mut self, ui: &mut Ui, pass: &mut Pass) -> Option<Action> {
        let t = pass.t;
        page(ui, &t, |ui| {
            heading(ui, tl!("Microsoft needs an application ID"), &t);
            note(
                ui,
                tl!(
                    "Microsoft only lets a registered application show its sign-in page, and this build of MailCraft has no registration of its own yet."
                ),
                t.text,
            );
            note(
                ui,
                tl!(
                    "If you have one, paste its application (client) ID here. It comes from a Microsoft Entra app registration: a public client with the redirect URI http://localhost, for accounts in any directory and personal accounts."
                ),
                t.text,
            );
            if field(ui, pass, "clientId", tl!("Application (client) ID"), &mut self.client_id, "00000000-0000-0000-0000-000000000000", 200).changed()
            {
                self.error.clear();
            }
            note(ui, tl!("It is an identifier, not a secret. MailCraft keeps it in its settings (File › Options)."), t.text_dim);
            error(ui, &self.error);
        });
        let ready = !self.client_id.trim().is_empty();
        let (mut go, mut back) = (false, false);
        strip(ui, self.page, |ui| {
            go = button(ui, tl!("Continue"), true, ready).clicked();
            back = button(ui, tl!("Back"), false, true).clicked();
        });
        if back { Some(Action::Back) } else { (ready && (go || entered(ui, pass))).then_some(Action::Go) }
    }

    fn password_page(&mut self, app: &MailApp, ui: &mut Ui, pass: &mut Pass) -> Option<Action> {
        let t = pass.t;
        // A build that can't reach a mail server has no use for a password.
        let asks = app.services.test_account.is_some() || app.services.sync.is_some();
        let title = if self.service.is_empty() {
            tl!("Sign in to your account").to_string()
        } else {
            fmt(tl!("{service} account"), &[("service", &self.service)])
        };
        page(ui, &t, |ui| {
            heading(ui, &title, &t);
            note(ui, &shown(self.email.trim(), 120), t.text_dim);
            if self.guessed {
                note(ui, tl!("MailCraft couldn't find the settings for this address. Check the servers below before you connect."), t.orange);
            }
            if !self.note.is_empty() {
                note(ui, tl!(&self.note), t.text);
            }
            if asks {
                self.password_field(ui, pass, app.secret_watch(), "");
                note(ui, kept_where(app), t.text_dim);
            } else {
                note(ui, tl!("This build can't connect to mail servers. The account can still be added."), t.text_dim);
            }
            ui.add_space(2.0);
            disclosure(ui, tl!("Change account settings"), &mut self.settings_open, &t);
            if self.settings_open {
                indented(ui, |ui| self.settings(ui, pass));
            }
        });
        problem(ui, &self.error);
        if self.app_passwords {
            egui::Frame::NONE.inner_margin(Margin { left: PAD, right: PAD, top: 0, bottom: 0 }).show(ui, |ui| {
                ui.hyperlink(GOOGLE_APP_PASSWORDS);
            });
        }
        let ready = !asks || !self.password.0.is_empty();
        let (mut go, mut back) = (false, false);
        strip(ui, self.page, |ui| {
            go = button(ui, tl!("Connect"), true, ready).clicked();
            back = button(ui, tl!("Back"), false, true).clicked();
        });
        if back { Some(Action::Back) } else { (ready && (go || entered(ui, pass))).then_some(Action::Go) }
    }

    /// The owner signed in as somebody other than the address typed (#103): who, and the two
    /// ways on. Nothing is added until they say.
    fn someone_else_page(&mut self, ui: &mut Ui, pass: &Pass) -> Option<Action> {
        let t = pass.t;
        let who = self.someone_else.as_ref().map(|tokens| line(&tokens.email, LINE)).unwrap_or_default();
        let typed = self.email.trim();
        page(ui, &t, |ui| {
            heading(ui, tl!("This isn't the account you typed"), &t);
            let said = fmt(tl!("You signed in as {signedIn}, not {typed}."), &[("signedIn", &shown(&who, 120)), ("typed", &shown(typed, 120))]);
            ui.label(RichText::new(said).font(medium(13.0)).color(t.text));
            ui.add_space(2.0);
            let next = fmt(
                tl!("Sign in again and pick {typed} on the sign-in page, or set up the account you signed in as instead."),
                &[("typed", &shown(typed, 120))],
            );
            note(ui, &next, t.text_dim);
            ui.add_space(4.0);
        });
        problem(ui, &self.error);
        let usable = address(&who).is_some();
        let use_label = fmt(tl!("Use {address}"), &[("address", &shown(&who, 40))]);
        let (mut again, mut take) = (false, false);
        strip(ui, self.page, |ui| {
            again = button(ui, tl!("Sign in again"), true, true).clicked();
            take = button(ui, &use_label, false, usable).clicked();
        });
        if take { Some(Action::UseSignedIn) } else { (again || entered(ui, pass)).then_some(Action::Go) }
    }

    fn done_page(&mut self, ui: &mut Ui, pass: &Pass) -> Option<Action> {
        let t = pass.t;
        let next = match self.outcome {
            Outcome::Downloading => tl!("Your mail is downloading now. The first time can take a few minutes."),
            Outcome::Later => tl!("Press Send/Receive to download your mail."),
            Outcome::Offline => {
                tl!(
                    "This build can't connect to mail servers, so the settings were kept without being tried. Bring mail in with File › Import, or use the desktop app."
                )
            }
            Outcome::Local => tl!("It is on this device only: nothing is sent to or fetched from a server."),
        };
        page(ui, &t, |ui| {
            ui.add_space(6.0);
            ui.vertical_centered(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 26.0, t.accent);
                icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(40.0, 40.0)), "tick", t.on_accent, t.on_accent);
                ui.add_space(6.0);
                let done = fmt(tl!("{address} is set up"), &[("address", &shown(self.email.trim(), 120))]);
                ui.label(RichText::new(done).font(semibold(16.0)).color(t.text));
                ui.label(RichText::new(next).color(t.text_dim));
            });
            ui.add_space(6.0);
        });
        let (mut done, mut another) = (false, false);
        strip(ui, self.page, |ui| {
            done = button(ui, tl!("Done"), true, true).clicked();
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add_space(f32::from(PAD));
                another = ui.link(tl!("Add another account")).clicked();
            });
        });
        if another { Some(Action::Another) } else { (done || entered(ui, pass)).then_some(Action::Go) }
    }

    fn settings_page(&mut self, app: &MailApp, ui: &mut Ui, pass: &mut Pass) -> Option<Action> {
        if self.newer {
            return self.newer_page(ui, pass);
        }
        let t = pass.t;
        let asks = self.oauth.is_none() && (app.services.test_account.is_some() || app.services.sync.is_some());
        let how = match &self.oauth {
            Some(oauth) => fmt(tl!("Signs in on the {provider} page in your browser"), &[("provider", oauth.provider.label())]),
            None if self.incoming.host.trim().is_empty() && self.outgoing.host.trim().is_empty() => {
                tl!("On this device only: no mail server").to_string()
            }
            None => tl!("Signs in with a password").to_string(),
        };
        page(ui, &t, |ui| {
            heading(ui, &shown(self.email.trim(), 120), &t);
            note(ui, &how, t.text_dim);
            if self.graph {
                note(ui, tl!("Its mail is kept in step through Microsoft Graph, without mail servers"), t.text_dim);
            }
            field(ui, pass, "name", tl!("Your name"), &mut self.name, tl!("As the people you write to see it"), LINE);
            if asks {
                self.password_field(ui, pass, app.secret_watch(), tl!("Leave empty to keep the one in use"));
            }
            // (An account still on IMAP that signs in with Microsoft keeps them: it uses them.)
            if !self.graph {
                self.settings(ui, pass);
            }
            if self.oauth.is_some() {
                field(ui, pass, "clientId", tl!("Application (client) ID"), &mut self.client_id, tl!("The one in File › Options"), 200);
                field(ui, pass, "tenant", tl!("Tenant"), &mut self.tenant, "common", 64);
            }
            if let Some((_, keep)) = self.keep_offline.as_mut() {
                caption(ui, tl!("Mail to keep offline"), &t);
                ui.horizontal(|ui| {
                    let steps = KeepOffline::STEPS;
                    let last = steps.len().saturating_sub(1);
                    let mut index = steps.iter().position(|k| k == keep).unwrap_or(last);
                    crate::keep_offline::step_slider(ui, &mut index, last + 1).on_hover_text(tl!(
                        "How far back mail is kept in full on this computer. Older mail keeps its headers and a preview, and downloads when you open it. It stays on the server."
                    ));
                    *keep = steps.get(index).copied().unwrap_or(*keep);
                    ui.label(crate::keep_offline::label(*keep));
                });
            }
        });
        problem(ui, &self.error);
        let (mut save, mut cancel) = (false, false);
        strip(ui, self.page, |ui| {
            save = button(ui, tl!("Save"), true, true).clicked();
            cancel = button(ui, tl!("Cancel"), false, true).clicked();
        });
        if cancel { Some(Action::Back) } else { (save || entered(ui, pass)).then_some(Action::Go) }
    }

    /// The settings of an account a newer MailCraft added: what it is, and nothing to change (its
    /// settings go back into the mailbox file as that version wrote them).
    fn newer_page(&mut self, ui: &mut Ui, pass: &Pass) -> Option<Action> {
        let t = pass.t;
        page(ui, &t, |ui| {
            heading(ui, &shown(self.email.trim(), 120), &t);
            note(
                ui,
                tl!(
                    "This account was added by a newer version of MailCraft. This version keeps it as it is and doesn't send or receive with it: use the newer version for that, and to change its settings."
                ),
                t.text_dim,
            );
            note(
                ui,
                tl!("Its mail can be read here. What you change in it is kept in this mailbox, and this version tells no mail server about it."),
                t.text_dim,
            );
        });
        let mut close = false;
        strip(ui, self.page, |ui| close = button(ui, tl!("Close"), true, true).clicked());
        (close || entered(ui, pass)).then_some(Action::Back)
    }

    /// The password, masked. It is shown only while the owner has clicked its eye, and is never
    /// copied or spoken (`secret_field`).
    fn password_field(&mut self, ui: &mut Ui, pass: &mut Pass, watch: Watch, hint: &str) {
        caption(ui, tl!("Password"), &pass.t);
        let resp = secret_field::field(ui, &mut self.password.0, &mut self.reveal, watch, FIELD_MARGIN, |edit| {
            edit.id(password_id()).char_limit(SECRET).hint_text(hint.to_owned())
        });
        pass.field(&resp);
    }

    /// The user name and the two servers.
    fn settings(&mut self, ui: &mut Ui, pass: &mut Pass) {
        field(ui, pass, "username", tl!("User name"), &mut self.username, tl!("Usually the whole address"), LINE);
        server_row(ui, pass, tl!("Incoming mail (IMAP)"), "incoming", &mut self.incoming, "imap.example.org", (993, 143));
        server_row(ui, pass, tl!("Outgoing mail (SMTP)"), "outgoing", &mut self.outgoing, "smtp.example.org", (465, 587));
    }
}

/// Whether Enter asked for the page's main button. A key that is held down repeats, and a repeat
/// doesn't count: it mustn't carry the owner from page to page.
fn entered(ui: &Ui, pass: &Pass) -> bool {
    pass.submit && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Key { key: Key::Enter, pressed: true, repeat: false, .. })))
}

/// Where a password that works is kept, for the line under its field.
fn kept_where(app: &MailApp) -> &'static str {
    if app.ui.remember_sign_in && app.services.secrets.is_some() && app.secrets_trouble.is_none() {
        tl!("Once it has worked, your system's credential store keeps it for next time. It is never written to the mailbox.")
    } else {
        tl!("MailCraft keeps it in memory until you close the app. It is never written to disk.")
    }
}

/// A page that waits for the host: a spinner, what is awaited, and Cancel.
fn waiting_page(ui: &mut Ui, pass: &Pass, page_in_view: Page, text: &str, detail: &str) -> Option<Action> {
    let t = pass.t;
    page(ui, &t, |ui| {
        ui.add_space(14.0);
        ui.vertical_centered(|ui| {
            ui.add(egui::Spinner::new().size(30.0).color(t.accent));
            ui.add_space(8.0);
            ui.label(RichText::new(text).font(medium(13.5)).color(t.text));
            if !detail.is_empty() {
                ui.label(RichText::new(detail).color(t.text_dim));
            }
        });
        ui.add_space(14.0);
    });
    let mut cancel = false;
    strip(ui, page_in_view, |ui| cancel = button(ui, tl!("Cancel"), false, true).clicked());
    cancel.then_some(Action::Back)
}

/// A page: the mail glyph in an accent circle beside the product's name, then what `add` lays
/// out. It scrolls when the window is short, so the buttons under it stay in view.
fn page(ui: &mut Ui, t: &Tokens, add: impl FnOnce(&mut Ui)) {
    body(ui, |ui| {
        egui::Frame::NONE.inner_margin(Margin { left: PAD, right: 0, top: 6, bottom: 4 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().max(0.0), 38.0), Sense::hover());
            let center = pos2(r.min.x + 18.0, r.center().y);
            ui.painter().circle_filled(center, 18.0, t.accent);
            icons::paint(ui.painter(), Rect::from_center_size(center, vec2(20.0, 20.0)), "mail", t.on_accent, t.on_accent);
            ui.painter().text(pos2(r.min.x + 46.0, r.center().y), Align2::LEFT_CENTER, "MailCraft", semibold(17.0), t.text);
            ui.add_space(10.0);
            add(ui);
        });
    });
}

fn heading(ui: &mut Ui, text: &str, t: &Tokens) {
    ui.label(RichText::new(text).font(semibold(16.0)).color(t.text));
}

/// A sentence or two, wrapped to the page.
fn note(ui: &mut Ui, text: &str, color: egui::Color32) {
    ui.label(RichText::new(text).color(color));
}

/// The name of a field, above it.
fn caption(ui: &mut Ui, text: &str, t: &Tokens) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).font(medium(12.0)).color(t.text_dim));
}

/// A one-line field as wide as the page, with its name above it.
fn field(ui: &mut Ui, pass: &mut Pass, key: &'static str, label: &str, value: &mut String, hint: &str, limit: usize) -> Response {
    caption(ui, label, &pass.t);
    let edit = TextEdit::singleline(value)
        .id(field_id(("account", key)))
        .desired_width(f32::INFINITY)
        .margin(FIELD_MARGIN)
        .char_limit(limit)
        .hint_text(hint.to_owned());
    let resp = ui.add(edit);
    pass.field(&resp);
    resp
}

/// One server: its name, its port and how the connection is encrypted. `ports` are the usual
/// ports with and without TLS from the first byte, shown as the hint.
fn server_row(ui: &mut Ui, pass: &mut Pass, label: &str, key: &'static str, server: &mut ServerFields, host_hint: &str, ports: (u16, u16)) {
    caption(ui, label, &pass.t);
    ui.horizontal(|ui| {
        let mut choices = vec![(Security::Tls, "TLS"), (Security::StartTls, "STARTTLS")];
        // An account that came with an unencrypted server (one on this computer) shows it.
        if server.security == Security::None {
            choices.push((Security::None, tl!("None")));
        }
        let gap = ui.spacing().item_spacing.x;
        let width = (ui.available_width() - 64.0 - 78.0 * choices.len() as f32 - 2.0 * gap).max(80.0);
        let host = TextEdit::singleline(&mut server.host)
            .id(field_id(("account", key, "host")))
            .desired_width(width)
            .margin(FIELD_MARGIN)
            .char_limit(HOST)
            .hint_text(host_hint.to_owned());
        let resp = ui.add(host).on_hover_text(tl!("The server's name"));
        pass.field(&resp);
        let usual = if server.security == Security::Tls { ports.0 } else { ports.1 };
        let port = TextEdit::singleline(&mut server.port)
            .id(field_id(("account", key, "port")))
            .desired_width(64.0)
            .margin(FIELD_MARGIN)
            .char_limit(5)
            .hint_text(usual.to_string());
        let resp = ui.add(port).on_hover_text(tl!("The port; leave it empty for the usual one"));
        pass.field(&resp);
        // A port is a number of at most five digits.
        if server.port.chars().any(|c| !c.is_ascii_digit()) || server.port.chars().count() > 5 {
            server.port = server.port.chars().filter(char::is_ascii_digit).take(5).collect();
        }
        encryption(ui, key, &mut server.security, &choices, resp.rect.height(), &pass.t);
    });
}

/// How the connection to a server is encrypted: a choice as buttons joined in a row, as tall as
/// the fields beside them.
fn encryption(ui: &mut Ui, key: &'static str, value: &mut Security, choices: &[(Security, &str)], height: f32, t: &Tokens) {
    let gap = std::mem::replace(&mut ui.spacing_mut().item_spacing.x, 0.0);
    for (i, (choice, label)) in choices.iter().enumerate() {
        let (_, r) = ui.allocate_space(vec2(78.0, height));
        let resp = ui.interact(r, ui.scope_id().with((key, choice.code())), Sense::click());
        let on = *value == *choice;
        resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, on, *label));
        let (left, right) = (if i == 0 { 4 } else { 0 }, if i + 1 == choices.len() { 4 } else { 0 });
        let corners = egui::CornerRadius { nw: left, sw: left, ne: right, se: right };
        let fill = if on {
            t.checked
        } else if resp.hovered() {
            t.hover
        } else {
            t.input
        };
        ui.painter().rect(r, corners, fill, Stroke::new(1.0, if on || resp.has_focus() { t.accent } else { t.input_border }), StrokeKind::Inside);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *label, regular(12.0), if on { t.text } else { t.text_dim });
        if resp.clicked() {
            *value = *choice;
        }
    }
    ui.spacing_mut().item_spacing.x = gap;
}

/// What sits under a disclosure, set in from it.
fn indented(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE.inner_margin(Margin { left: 20, right: 0, top: 0, bottom: 2 }).show(ui, add);
}

/// What went wrong, in red above the buttons, where it stays in view however long the page is.
/// Where a Google account's app passwords are made.
const GOOGLE_APP_PASSWORDS: &str = "https://myaccount.google.com/apppasswords";

/// Whether `host` is Google's mail server (Gmail, Google Workspace).
fn is_google(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    host == "gmail.com" || host.ends_with(".gmail.com") || host.ends_with(".googlemail.com")
}

fn problem(ui: &mut Ui, message: &str) {
    if message.is_empty() {
        return;
    }
    egui::Frame::NONE.inner_margin(Margin { left: PAD, right: PAD, top: 0, bottom: 0 }).show(ui, |ui| error(ui, message));
}

/// The buttons under a page, laid out from the right. They are that page's own: a button that
/// has the keyboard doesn't hand it to the next page's.
fn strip(ui: &mut Ui, page_in_view: Page, add: impl FnOnce(&mut Ui)) {
    ui.add_space(6.0);
    ui.allocate_ui_with_layout(vec2(ui.available_width().max(0.0), 30.0), Layout::right_to_left(Align::Center), |ui| {
        ui.add_space(f32::from(PAD));
        ui.push_id(page_in_view, add);
    });
    ui.add_space(4.0);
}

fn text_width(ui: &Ui, text: &str, font: egui::FontId) -> f32 {
    ui.painter().layout_no_wrap(text.to_string(), font, egui::Color32::WHITE).size().x
}

/// A button of the wizard, a little taller than a dialog's; the main one is filled in the app
/// colour.
fn button(ui: &mut Ui, label: &str, primary: bool, enabled: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let width = text_width(ui, label, medium(12.5)).max(52.0) + 30.0;
    let (_, r) = ui.allocate_space(vec2(width, 30.0));
    // Known by what it says, not by where it sits.
    let resp = ui.interact(r, ui.scope_id().with(label), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    let lit = if resp.hovered() { t.accent_text } else { t.accent };
    let (fill, edge, ink) = match (primary, enabled) {
        (_, false) => (t.pressed, t.pressed, t.text_disabled),
        (true, true) => (lit, lit, t.on_accent),
        (false, true) => (if resp.hovered() { t.hover } else { t.input }, t.input_border, t.text),
    };
    ui.painter().rect(r, 4.0, fill, Stroke::new(1.0, edge), StrokeKind::Inside);
    if resp.has_focus() {
        ui.painter().rect_stroke(r.expand(2.0), 6.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    ui.painter().text(r.center(), Align2::CENTER_CENTER, label, medium(12.5), ink);
    resp
}

/// A line that shows or hides what is under it.
fn disclosure(ui: &mut Ui, label: &str, open: &mut bool, t: &Tokens) {
    let width = text_width(ui, label, regular(12.5)) + 22.0;
    let (r, resp) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    let ink = if resp.hovered() || resp.has_focus() { t.accent } else { t.accent_text };
    icons::paint(
        ui.painter(),
        Rect::from_center_size(pos2(r.min.x + 6.0, r.center().y), vec2(12.0, 12.0)),
        if *open { "expanded" } else { "collapsed" },
        ink,
        ink,
    );
    ui.painter().text(pos2(r.min.x + 18.0, r.center().y), Align2::LEFT_CENTER, label, regular(12.5), ink);
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        *open = !*open;
    }
}

/// A kind of account to pick: a glyph, its name and a line about it.
fn tile(ui: &mut Ui, kind: Kind, width: f32, t: &Tokens) -> Response {
    let (title, detail) = (tl!(kind.title()), tl!(kind.detail()));
    let (r, resp) = ui.allocate_exact_size(vec2(width, 50.0), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, title));
    let lit = resp.hovered() || resp.has_focus();
    ui.painter().rect(r, 6.0, if lit { t.hover } else { t.panel }, Stroke::new(1.0, if lit { t.accent } else { t.border }), StrokeKind::Inside);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 24.0, r.center().y), vec2(24.0, 24.0)), kind.icon(), t.icon, t.accent);
    paint_clipped(ui.painter(), pos2(r.min.x + 46.0, r.min.y + 17.0), r.width() - 54.0, title, semibold(12.5), t.text);
    paint_clipped(ui.painter(), pos2(r.min.x + 46.0, r.min.y + 34.0), r.width() - 54.0, detail, regular(11.0), t.text_dim);
    resp
}

#[cfg(test)]
mod pop_tests;

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};

    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;
    use mailcraft_engine::Session;
    use mailcraft_model::sync::{SyncRequest, SyncResult, Tokens as SignedIn};

    use super::*;
    use crate::dialogs::Dialog;
    use crate::forms::Form;
    use crate::{Services, SignInRequest};

    /// What the wizard says about a sign-in on Microsoft's page, and the approval help (M4.4).
    mod help;
    /// The password's eye (#438).
    mod reveal;

    /// Wednesday 2026-10-14 16:00 UTC.
    const NOW: i64 = 1_791_993_600;
    /// What the tests type where a password goes. It opens nothing.
    const TYPED: &str = "not-a-real-password";
    /// An application ID in the shape Microsoft hands them out; this one is nobody's.
    const APP_ID: &str = "11111111-2222-3333-4444-555555555555";
    /// The mail exchanger of a company whose mail Microsoft 365 takes.
    const MICROSOFT_MX: &str = "example-org.mail.protection.outlook.com";

    type Log<T> = Arc<Mutex<Vec<T>>>;

    /// The app over the sample mailbox, or over the empty one of a first run.
    fn app(sample: bool) -> MailApp {
        let mailbox = if sample { mailcraft_engine::sample::mailbox(NOW, 0) } else { mailcraft_model::Mailbox::with_local_account("", "") };
        let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
        // Nothing happens by itself: what a test sees synchronised, it or the wizard asked for.
        (app.autosave, app.auto_sync) = (false, false);
        app
    }

    /// A host that looks mail exchangers up: it answers `answer`, or nothing at all for `None`.
    /// The log is what it was asked.
    fn lookups(app: &mut MailApp, answer: Option<Result<Vec<&str>, &str>>) -> Log<String> {
        let asked: Log<String> = Default::default();
        let log = asked.clone();
        let answer = answer.map(|a| a.map(|mx| mx.into_iter().map(str::to_string).collect::<Vec<_>>()).map_err(str::to_string));
        let silent: Log<mpsc::Sender<Result<Vec<String>, String>>> = Default::default();
        app.services.lookup_mx = Some(Box::new(move |domain| {
            log.lock().unwrap().push(domain);
            let (tx, rx) = mpsc::channel();
            match &answer {
                Some(answer) => tx.send(answer.clone()).unwrap(),
                None => silent.lock().unwrap().push(tx),
            }
            rx
        }));
        asked
    }

    /// What a sign-in was asked for: the address, the application ID, and the flag that stops it.
    type SignIns = Log<(String, String, Arc<AtomicBool>)>;

    /// A host that signs owners in: it answers `answer`, or leaves the owner in the browser.
    fn sign_ins(app: &mut MailApp, answer: Option<Result<SignedIn, String>>) -> SignIns {
        let asked: SignIns = Default::default();
        let log = asked.clone();
        let silent: Log<mpsc::Sender<Result<SignedIn, String>>> = Default::default();
        app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
            log.lock().unwrap().push((req.email.clone(), req.oauth.client_id.clone(), req.cancel.clone()));
            let (tx, rx) = mpsc::channel();
            match &answer {
                Some(answer) => tx.send(answer.clone()).unwrap(),
                None => silent.lock().unwrap().push(tx),
            }
            rx
        }));
        asked
    }

    fn signed_in(name: &str) -> SignedIn {
        SignedIn {
            access: "tok-1".into(),
            refresh: "ref-1".into(),
            expires_at: mailcraft_time::now() + 3600,
            name: name.into(),
            email: "noor@example.org".into(),
            tenant: String::new(),
        }
    }

    /// A host that tries accounts on their servers: it answers `answer`, or keeps trying. The log
    /// is the accounts it was handed, each with its secret.
    fn checks(app: &mut MailApp, answer: Option<Result<(), &str>>) -> Log<(Account, String)> {
        let asked: Log<(Account, String)> = Default::default();
        let log = asked.clone();
        let answer = answer.map(|a| a.map_err(str::to_string));
        let silent: Log<mpsc::Sender<Result<(), String>>> = Default::default();
        app.services.test_account = Some(Box::new(move |account, secret| {
            log.lock().unwrap().push((account, secret));
            let (tx, rx) = mpsc::channel();
            match &answer {
                Some(answer) => tx.send(answer.clone()).unwrap(),
                None => silent.lock().unwrap().push(tx),
            }
            rx
        }));
        asked
    }

    /// A host that synchronises, and is never done. The log is each account it was asked to
    /// synchronise, with the access token and the password that came along.
    fn syncs(app: &mut MailApp) -> Log<(Account, String, String)> {
        let asked: Log<(Account, String, String)> = Default::default();
        let log = asked.clone();
        let running: Log<mpsc::Sender<SyncResult>> = Default::default();
        app.services.sync = Some(Box::new(move |req: SyncRequest| {
            log.lock().unwrap().push((req.account.clone(), req.access_token.clone(), req.password.clone()));
            let (tx, rx) = mpsc::channel();
            running.lock().unwrap().push(tx);
            rx
        }));
        asked
    }

    /// The whole window, laid out a few frames so the dialog is up and its first field has the keyboard.
    fn window(app: MailApp, size: (f32, f32)) -> Harness<'static, MailApp> {
        let mut harness =
            Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(size.0, size.1)).build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        harness.run_steps(6);
        harness
    }

    /// The window with the wizard open.
    fn open(mut app: MailApp, mut params: Value) -> Harness<'static, MailApp> {
        params["name"] = json!("account");
        app.execute("ui.dialog", params).unwrap();
        window(app, (1280.0, 800.0))
    }

    fn wizard<'a>(h: &'a Harness<'static, MailApp>) -> Option<&'a AccountSetup> {
        match h.state().dialog.as_ref() {
            Some(Dialog::Form(Form::Account(w))) => Some(w),
            _ => None,
        }
    }

    fn wizard_mut<'a>(h: &'a mut Harness<'static, MailApp>) -> &'a mut AccountSetup {
        match h.state_mut().dialog.as_mut() {
            Some(Dialog::Form(Form::Account(w))) => w,
            _ => panic!("the wizard isn't open"),
        }
    }

    /// The page in view; `None` when the dialog is closed.
    fn page(h: &Harness<'static, MailApp>) -> Option<Page> {
        wizard(h).map(|w| w.page)
    }

    fn type_text(h: &mut Harness<'static, MailApp>, text: &str) {
        h.event(egui::Event::Text(text.to_string()));
        h.run_steps(3);
    }

    fn press(h: &mut Harness<'static, MailApp>, key: Key) {
        h.key_press(key);
        h.run_steps(5);
    }

    fn click(h: &mut Harness<'static, MailApp>, label: &str) {
        h.get_by_role_and_label(Role::Button, label).click();
        h.run_steps(5);
    }

    /// Type into the password field.
    fn type_password(h: &mut Harness<'static, MailApp>) {
        h.get_by_role(Role::PasswordInput).focus();
        h.run_steps(2);
        type_text(h, TYPED);
    }

    /// Whether some text on screen contains `text`.
    fn says(h: &Harness<'static, MailApp>, text: &str) -> bool {
        h.query_all_by_label_contains(text).next().is_some()
    }

    fn addresses(h: &Harness<'static, MailApp>) -> Vec<String> {
        h.state().session.mailbox.accounts.iter().map(|a| a.email.clone()).collect()
    }

    /// Type an address on the first page and press Enter.
    fn connect(h: &mut Harness<'static, MailApp>, address: &str) {
        assert_eq!(page(h), Some(Page::Address));
        type_text(h, address);
        press(h, Key::Enter);
    }

    /// Tick "Let me set up my account manually" and press Connect.
    fn connect_manually(h: &mut Harness<'static, MailApp>) {
        click(h, "Advanced options");
        h.get_by_role_and_label(Role::CheckBox, "Let me set up my account manually").click();
        h.run_steps(3);
        click(h, "Connect");
        assert_eq!(page(h), Some(Page::Choose));
    }

    #[test]
    fn addresses_are_checked_before_anything_is_asked() {
        for good in ["noor@example.org", " noor.haddad+news@mail.lumen.example ", "NOOR@EXAMPLE.ORG", "нур@example.org", "a@b.example"] {
            assert!(address(good).is_some(), "{good}");
        }
        assert_eq!(address(" Noor@Example.org\n"), Some(("Noor@Example.org", "Example.org")));
        let long = format!("{}@example.org", "a".repeat(250));
        let megabyte = format!("{}@example.org", "a".repeat(1 << 20));
        for bad in [
            "",
            "   ",
            "noor",
            "noor@",
            "@example.org",
            "noor@example",
            "noor@@example.org",
            "noor@example..org",
            "noor@.example.org",
            "noor@exa mple.org",
            "no or@example.org",
            "noor\t@example.org",
            "no\u{0}or@example.org",
            "noor@exam\u{7}ple.org",
            "noor@example.org\u{0}",
            "Noor <noor@example.org>",
            "noor@example.org, ada@example.org",
            "noor@example.org/inbox",
            long.as_str(),
            megabyte.as_str(),
        ] {
            assert_eq!(address(bad), None, "{:?}", bad.chars().take(40).collect::<String>());
        }
        // Text from outside becomes one short line before it is anyone's address or name.
        assert_eq!(line(" Noor\r\nHaddad\u{0} ", 254), "Noor  Haddad");
        assert_eq!(line(&"é".repeat(10_000), 5), "ééééé");
        assert!(is_identifier(APP_ID, 200) && is_identifier("", 200) && !is_identifier("an id", 200) && !is_identifier(APP_ID, 8));
    }

    #[test]
    fn each_page_says_what_it_would_run() {
        let a = app(false);
        let mut s = Session::empty();
        let mut w = AccountSetup::new(&a, &json!({"email": " noor@example.org "}));
        // Connect; and once more when the host knows who takes the domain's mail.
        assert_eq!(w.preset_command(None, None), Some(("account.preset", json!({"email": "noor@example.org"}))));
        let mx = vec![MICROSOFT_MX.to_string()];
        let detect = w.preset_command(Some(&mx), None).unwrap();
        assert_eq!(detect, ("account.preset", json!({"email": "noor@example.org", "mx": [MICROSOFT_MX]})));
        // A tile of the Choose page names the service; "This device only" adds a mailbox without servers.
        assert_eq!(
            w.preset_command(None, Kind::Google.service()),
            Some(("account.preset", json!({"email": "noor@example.org", "service": "google"})))
        );
        assert_eq!((Kind::Other.service(), Kind::Local.service()), (None, None));
        w.name = " Noor Haddad ".into();
        assert_eq!(w.local_command(), Some(("account.add", json!({"email": "noor@example.org", "displayName": "Noor Haddad"}))));
        // No servers yet: nothing to connect to.
        assert_eq!(w.add_command(""), None);
        assert!(w.account().is_none());

        // What detection answered goes into the fields, and from there into `account.add`.
        w.take(Preset::read(&s.run(detect.0, &detect.1).unwrap()));
        assert_eq!((w.service.as_str(), w.oauth.as_ref().map(|o| o.provider)), ("Microsoft 365", Some(OAuthProvider::Microsoft)));
        let add = w.add_command("Name From Microsoft").unwrap();
        assert_eq!(
            add,
            (
                "account.add",
                json!({"email": "noor@example.org", "displayName": "Noor Haddad", "username": "noor@example.org",
                    "incoming": {"host": "outlook.office365.com", "port": 993, "security": "tls"},
                    "outgoing": {"host": "smtp.office365.com", "port": 587, "security": "starttls"},
                    "auth": "oauth2", "provider": "microsoft"})
            )
        );
        // Without a typed name, the one the provider knows the owner by.
        w.name.clear();
        assert_eq!(w.add_command(" Noor H.\n").unwrap().1["displayName"], "Noor H.");
        let id = s.run(add.0, &add.1).unwrap()["id"].as_u64().unwrap();
        let account = s.mailbox.account(id).unwrap();
        assert_eq!((account.kind, account.oauth.as_ref().map(|o| o.provider)), (AccountKind::Imap, Some(OAuthProvider::Microsoft)));

        // A password account, with servers the owner corrected.
        let mut w = AccountSetup::new(&a, &json!({"email": "ada@lumen.example", "displayName": "Ada"}));
        let guess = w.preset_command(None, None).unwrap();
        w.take(Preset::read(&s.run(guess.0, &guess.1).unwrap()));
        assert_eq!((w.service.as_str(), w.oauth.is_none(), w.incoming.host.as_str()), ("", true, "imap.lumen.example"));
        w.username = " ada ".into();
        w.incoming = ServerFields { host: " mail.lumen.example ".into(), port: String::new(), security: Security::StartTls };
        w.outgoing.host.clear();
        let add = w.add_command("").unwrap();
        assert_eq!(
            add.1,
            json!({"email": "ada@lumen.example", "displayName": "Ada", "username": "ada",
                "incoming": {"host": "mail.lumen.example", "security": "starttls"}, "auth": "password"})
        );
        // The host tries the same account that is then added.
        let tried = w.account().unwrap();
        let id = s.run(add.0, &add.1).unwrap()["id"].as_u64().unwrap();
        let added = s.mailbox.account(id).unwrap();
        assert_eq!(
            (&tried.username, &tried.incoming, &tried.outgoing, &tried.oauth),
            (&added.username, &added.incoming, &added.outgoing, &added.oauth)
        );
        assert_eq!(tried.incoming, Some(Server { host: "mail.lumen.example".into(), port: 0, security: Security::StartTls }));
        // An empty user name signs in with the address.
        w.username.clear();
        assert_eq!(w.add_command("").unwrap().1["username"], "ada@lumen.example");

        // The application ID page keeps what was pasted.
        w.client_id = format!(" {APP_ID} ");
        assert_eq!(w.client_id_command(), Some(("ui.microsoftClientId", json!({"value": APP_ID}))));
    }

    #[test]
    fn hostile_input_never_makes_a_command() {
        let mut a = app(true);
        let before = a.session.mailbox.accounts.len();
        let junk = "\u{0}\u{202e}é\n@".repeat(200_000);
        assert!(junk.len() > 1 << 20);
        // What a script hands the dialog is cut to what a field takes.
        let mut w = AccountSetup::open(&a, &json!({"email": junk, "displayName": junk, "id": junk})).unwrap();
        assert!(w.email.chars().count() <= LINE && w.name.chars().count() <= LINE && w.page == Page::Address);
        // Past the fields' limits, as if it had got in anyway.
        w.email.clone_from(&junk);
        w.name.clone_from(&junk);
        assert_eq!(w.preset_command(None, None), None);
        assert_eq!(w.preset_command(Some(std::slice::from_ref(&junk)), Some(&junk)), None);
        assert_eq!(w.add_command(&junk), None);
        assert_eq!(w.local_command(), None);
        assert!(w.account().is_none());
        // Connect and every tile say what is wrong, briefly, and stay where they are.
        assert!(!w.act(&mut a, Action::Go));
        assert!(w.page == Page::Address && w.error.contains("isn't an e-mail address") && w.error.chars().count() < 200);
        for kind in Kind::HOSTED.into_iter().chain([Kind::Local]) {
            (w.page, w.error) = (Page::Choose, String::new());
            assert!(!w.act(&mut a, Action::Pick(kind)));
            assert!(w.page == Page::Choose && !w.error.is_empty(), "{kind:?}");
        }
        assert_eq!(a.session.mailbox.accounts.len(), before);

        // A good address, and servers that can't be.
        w.email = "noor@example.org".into();
        w.incoming = ServerFields { host: "imap.example.org".into(), port: "993".into(), security: Security::Tls };
        assert!(w.add_command("").is_some() && w.account().is_some());
        for port in ["0", "65536", "99999", "-1", "12a", "\u{ff11}\u{ff12}", "9 9", junk.as_str()] {
            w.incoming.port = port.into();
            assert!(w.add_command("").is_none() && w.account().is_none(), "port {:?}", port.chars().take(10).collect::<String>());
            (w.page, w.error) = (Page::Password, String::new());
            assert!(!w.act(&mut a, Action::Go));
            assert!(w.page == Page::Password && w.error.contains("port") && w.settings_open);
        }
        w.incoming.port.clear();
        let long = "a".repeat(300);
        for host in
            ["imap.example.org/inbox", "imap example", "user@imap.example.org", "imap.example.org\u{0}", "é.example", long.as_str(), junk.as_str()]
        {
            for outgoing in [false, true] {
                w.incoming.host = if outgoing { "imap.example.org".into() } else { host.into() };
                w.outgoing.host = if outgoing { host.into() } else { String::new() };
                assert!(w.add_command("").is_none() && w.account().is_none(), "host {:?}", host.chars().take(30).collect::<String>());
            }
        }
        assert_eq!(a.session.mailbox.accounts.len(), before);

        // An application ID that isn't one.
        for id in ["", "   ", "not an id!", "two words", junk.as_str()] {
            w.client_id = id.into();
            assert_eq!(w.client_id_command(), None);
        }
        (w.page, w.client_id) = (Page::ApplicationId, "not an id!".into());
        assert!(!w.act(&mut a, Action::Go));
        assert!(w.page == Page::ApplicationId && w.error.contains("application ID") && a.ui.microsoft_client_id.is_empty());

        // An answer that isn't one reads as nothing, and a long one is cut.
        for v in [json!(null), json!([1, 2]), json!("preset"), json!({"incoming": 7, "outgoing": [], "auth": 5, "known": "yes", "provider": {}})] {
            assert_eq!(Preset::read(&v), Preset::default(), "{v}");
        }
        let odd = json!({"known": true, "service": junk, "note": junk, "username": junk, "auth": "oauth2", "provider": junk,
            "incoming": {"host": junk, "port": u64::MAX, "security": junk}, "outgoing": {"host": "smtp.example.org", "port": 0, "security": "STARTTLS"}});
        let p = Preset::read(&odd);
        assert!(p.service.chars().count() <= 60 && p.note.chars().count() <= 300 && p.username.chars().count() <= LINE);
        assert!(p.incoming.host.chars().count() <= HOST && p.incoming.port.is_empty() && p.incoming.security == Security::Tls);
        assert_eq!(p.outgoing, ServerFields { host: "smtp.example.org".into(), port: String::new(), security: Security::StartTls });
        assert_eq!(p.oauth, Some(OAuthProvider::Microsoft));

        // What a hostile name server answers is cut down before it is passed on.
        let exchangers = vec![junk.chars().take(5000).collect::<String>(); 500];
        let (_, params) = w.preset_command(Some(&exchangers), None).unwrap();
        let passed = params["mx"].as_array().unwrap();
        assert!(passed.len() == 20 && passed.iter().all(|host| host.as_str().unwrap().chars().count() <= 255));
        assert!(a.session.run("account.preset", &params).is_ok());
    }

    #[test]
    fn account_settings_send_only_what_changed() {
        let mut a = app(true);
        let server = json!({"email": "noor@lumen.example", "displayName": "Noor", "username": "noor",
            "incoming": {"host": "imap.lumen.example", "port": 993}, "outgoing": {"host": "smtp.lumen.example", "port": 587, "security": "starttls"}});
        let id = a.execute("account.add", server).unwrap()["id"].as_u64().unwrap();
        let mut w = AccountSetup::open(&a, &json!({"id": id})).unwrap();
        assert_eq!((w.page, w.title().as_str(), w.email.as_str()), (Page::Settings, "Account Settings", "noor@lumen.example"));
        assert_eq!((w.incoming.port.as_str(), w.outgoing.security), ("993", Security::StartTls));
        // Nothing changed: nothing to run, and Save just closes.
        assert_eq!(w.edit_command(), None);
        assert!(w.act(&mut a, Action::Go));

        w.name = "Noor Haddad".into();
        (w.outgoing.port, w.outgoing.security) = ("465".into(), Security::Tls);
        let expected = json!({"id": id, "displayName": "Noor Haddad", "outgoing": {"host": "smtp.lumen.example", "port": 465, "security": "tls"}});
        assert_eq!(w.edit_command(), Some(("account.edit", expected)));
        assert!(w.act(&mut a, Action::Go));
        let account = a.session.mailbox.account(id).unwrap();
        assert_eq!((account.display_name.as_str(), account.username.as_str()), ("Noor Haddad", "noor"));
        assert_eq!(account.outgoing, Some(Server { host: "smtp.lumen.example".into(), port: 465, security: Security::Tls }));

        // A server whose name is cleared goes; a port that isn't one keeps the page open.
        let mut w = AccountSetup::open(&a, &json!({"id": id})).unwrap();
        w.outgoing.host.clear();
        assert_eq!(w.edit_command(), Some(("account.edit", json!({"id": id, "outgoing": null}))));
        w.incoming.port = "0".into();
        assert_eq!(w.edit_command(), None);
        assert!(!w.act(&mut a, Action::Go));
        assert!(w.error.contains("port") && a.session.mailbox.account(id).unwrap().outgoing.is_some());
        // Cancel changes nothing.
        assert!(w.act(&mut a, Action::Back));
        assert!(a.session.mailbox.account(id).unwrap().outgoing.is_some());

        // An account that signs in on its provider's page also has an application ID and a tenant.
        let cloud = json!({"email": "ada@lumen.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}});
        let cloud = a.execute("account.add", cloud).unwrap()["id"].as_u64().unwrap();
        let mut w = AccountSetup::open(&a, &json!({"id": cloud})).unwrap();
        (w.client_id, w.tenant) = (APP_ID.into(), "organizations".into());
        assert_eq!(w.edit_command(), Some(("account.edit", json!({"id": cloud, "clientId": APP_ID, "tenant": "organizations"}))));
        w.tenant = "not a tenant".into();
        assert_eq!(w.edit_command(), None);
        w.tenant = "organizations".into();
        assert!(w.act(&mut a, Action::Go));
        let oauth = a.session.mailbox.account(cloud).unwrap().oauth.clone().unwrap();
        assert_eq!((oauth.client_id.as_str(), oauth.tenant.as_str()), (APP_ID, "organizations"));

        // There is nothing to open for an account that isn't there, nor for the nameless mailbox
        // of a first run.
        assert!(AccountSetup::open(&a, &json!({"id": 999_999})).is_none());
        let first_run = app(false);
        let placeholder = first_run.session.mailbox.accounts[0].id;
        assert!(AccountSetup::open(&first_run, &json!({"id": placeholder})).is_none());
        assert!(AccountSetup::open(&first_run, &json!({"id": 0})).is_some_and(|w| w.page == Page::Address));
    }

    #[test]
    fn the_address_and_the_name_can_be_given() {
        // Someone who writes from here already most likely adds another address of their own.
        let a = app(true);
        let own = a.session.mailbox.default_account().unwrap().display_name.clone();
        assert!(!own.is_empty());
        let w = AccountSetup::open(&a, &json!({"email": " noor@example.org "})).unwrap();
        assert_eq!((w.email.as_str(), w.name.as_str(), w.title().as_str()), ("noor@example.org", own.as_str(), "Add Account"));
        let w = AccountSetup::open(&a, &json!({"displayName": "Noor Haddad"})).unwrap();
        assert_eq!((w.email.as_str(), w.name.as_str()), ("", "Noor Haddad"));
        // The nameless mailbox of a first run has no name to offer.
        assert_eq!(AccountSetup::open(&app(false), &json!({})).unwrap().name, "");
        // In the window, the address is in its field and Connect is ready.
        let mut h = open(app(false), json!({"email": "noor@example.org"}));
        assert!(h.get_all_by_role(Role::TextInput).any(|field| field.value().as_deref() == Some("noor@example.org")));
        click(&mut h, "Connect");
        assert_eq!(page(&h), Some(Page::Password));
    }

    /// (a) The owner's case: an address at a company's own domain whose mail Microsoft 365 takes.
    #[test]
    fn a_company_domain_on_microsoft_365_signs_in_and_is_added() {
        let mut a = app(false);
        a.services.microsoft_client_id = APP_ID.into();
        let asked = lookups(&mut a, Some(Ok(vec![MICROSOFT_MX])));
        let signed = sign_ins(&mut a, Some(Ok(signed_in("Noor Haddad"))));
        let synced = syncs(&mut a);
        let jobs: Log<Option<mailcraft_model::sync::GraphMailRequest>> = Default::default();
        let log = jobs.clone();
        a.services.graph = Some(Box::new(move |req: crate::GraphRequest| {
            log.lock().unwrap().push(req.mail.clone());
            mpsc::channel().1
        }));
        let mut h = open(a, json!({}));
        connect(&mut h, "Noor@Example.org");
        h.run_steps(4);
        assert_eq!(page(&h), Some(Page::Done));
        // Only the domain went to the name server, never the mailbox's name.
        assert_eq!(*asked.lock().unwrap(), ["example.org"]);
        {
            let signed = signed.lock().unwrap();
            assert_eq!((signed.len(), signed[0].0.as_str(), signed[0].1.as_str()), (1, "Noor@Example.org", APP_ID));
        }
        // The account is the only one (the nameless mailbox made way), kept in step through
        // Microsoft Graph (R3), with the name Microsoft knows the owner by.
        let s = &h.state().session;
        assert_eq!(s.mailbox.accounts.len(), 1);
        let account = &s.mailbox.accounts[0];
        assert_eq!(
            (account.email.as_str(), account.display_name.as_str(), account.username.as_str()),
            ("Noor@Example.org", "Noor Haddad", "Noor@Example.org")
        );
        assert_eq!(
            (account.kind, account.oauth.as_ref().map(|o| (o.provider, o.client_id.as_str()))),
            (AccountKind::Microsoft, Some((OAuthProvider::Microsoft, "")))
        );
        assert_eq!((account.incoming.as_ref(), account.outgoing.as_ref()), (None, None));
        // The first download started, through Graph: no IMAP, and no password.
        assert!(synced.lock().unwrap().is_empty());
        assert_eq!(jobs.lock().unwrap().iter().filter_map(|mail| mail.as_ref().map(|m| m.account)).collect::<Vec<_>>(), [account.id]);
        assert!(!h.state().signing_in());
        assert!(says(&h, "Noor@Example.org is set up") && says(&h, "Your mail is downloading now"));
        click(&mut h, "Done");
        assert_eq!(page(&h), None);
    }

    /// (R3.1) A Microsoft account is kept in step through Microsoft Graph: signed in for Graph
    /// with its mail, no mail servers, and its first run a Graph job.
    #[test]
    fn a_microsoft_account_is_kept_in_step_through_graph() {
        let mut a = app(false);
        a.services.microsoft_client_id = APP_ID.into();
        lookups(&mut a, Some(Ok(vec![MICROSOFT_MX])));
        let asked: Log<(bool, bool)> = Default::default();
        let log = asked.clone();
        a.services.sign_in = Some(Box::new(move |req: SignInRequest| {
            log.lock().unwrap().push((req.graph, req.mail_write));
            let (tx, rx) = mpsc::channel();
            tx.send(Ok(signed_in("Noor Haddad"))).unwrap();
            rx
        }));
        let imap = syncs(&mut a);
        let jobs: Log<(String, String, Option<mailcraft_model::sync::GraphMailRequest>)> = Default::default();
        let log = jobs.clone();
        a.services.graph = Some(Box::new(move |req: crate::GraphRequest| {
            log.lock().unwrap().push((req.access.clone(), req.refresh.clone(), req.mail.clone()));
            mpsc::channel().1
        }));
        let mut h = open(a, json!({}));
        connect(&mut h, "Noor@Example.org");
        h.run_steps(4);
        assert_eq!(page(&h), Some(Page::Done));
        assert_eq!(*asked.lock().unwrap(), [(true, true)]);
        let account = h.state().session.mailbox.accounts[0].clone();
        assert_eq!((account.kind, account.incoming.is_none(), account.outgoing.is_none()), (AccountKind::Microsoft, true, true));
        // No IMAP: one Graph job, its token renewed for it, with the folders to list.
        assert!(imap.lock().unwrap().is_empty());
        let jobs = jobs.lock().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!((jobs[0].0.as_str(), jobs[0].1.as_str()), ("", "ref-1"));
        assert_eq!(jobs[0].2.as_ref().map(|mail| (mail.account, mail.email.as_str())), Some((account.id, "Noor@Example.org")));
    }

    /// (a, continued) What can go wrong on the way, and the ways on from there.
    #[test]
    fn a_sign_in_that_fails_can_be_tried_again_or_set_up_by_hand() {
        let mut a = app(false);
        a.services.microsoft_client_id = APP_ID.into();
        lookups(&mut a, Some(Ok(vec![MICROSOFT_MX])));
        let signed = sign_ins(&mut a, Some(Err("Microsoft didn't sign you in: the request was declined".into())));
        let mut h = open(a, json!({}));
        connect(&mut h, "noor@example.org");
        h.run_steps(4);
        assert_eq!((page(&h), wizard(&h).map(|w| w.waiting)), (Some(Page::SignIn), Some(false)));
        assert!(says(&h, "the request was declined") && addresses(&h) == [""]);
        click(&mut h, "Try again");
        assert_eq!((page(&h), signed.lock().unwrap().len()), (Some(Page::SignIn), 2));
        // The next try goes through, but the address is taken meanwhile: said here, nothing lost.
        sign_ins(h.state_mut(), Some(Ok(signed_in("Noor Haddad"))));
        h.state_mut().execute("account.add", json!({"email": "noor@example.org"})).unwrap();
        click(&mut h, "Try again");
        assert_eq!(page(&h), Some(Page::SignIn));
        assert!(says(&h, "already set up") && addresses(&h) == ["noor@example.org"]);
        click(&mut h, "Set up manually");
        assert_eq!(page(&h), Some(Page::Choose));
        click(&mut h, "Back");
        assert_eq!(page(&h), Some(Page::Address));
    }

    /// The tokens of a sign-in as `email`.
    fn signed_in_as(email: &str) -> SignedIn {
        SignedIn { name: "Ada Admin".into(), email: email.into(), ..signed_in("Noor Haddad") }
    }

    /// (#103) The owner types one address and, on Microsoft's page, picks another account: the
    /// wizard says so and adds nothing. "Sign in again" goes back to the provider's page for the
    /// address typed (where the owner picks who signs in, every time).
    #[test]
    fn a_sign_in_as_somebody_else_is_asked_about_and_nothing_is_added() {
        let mut a = app(false);
        a.services.microsoft_client_id = APP_ID.into();
        lookups(&mut a, Some(Ok(vec![MICROSOFT_MX])));
        let signed = sign_ins(&mut a, Some(Ok(signed_in_as("Admin@Example.org"))));
        let synced = syncs(&mut a);
        let mut h = open(a, json!({}));
        connect(&mut h, "noor@example.org");
        h.run_steps(4);
        assert_eq!(page(&h), Some(Page::SomeoneElse));
        assert!(says(&h, "You signed in as Admin@Example.org, not noor@example.org."));
        // Nothing is added, nothing is kept and nothing is downloaded.
        assert_eq!(addresses(&h), [""]);
        assert!(h.state().tokens.is_empty() && synced.lock().unwrap().is_empty());
        click(&mut h, "Sign in again");
        // Asked for the address typed again; the page asks who signs in.
        {
            let signed = signed.lock().unwrap();
            assert_eq!(signed.iter().map(|(email, ..)| email.as_str()).collect::<Vec<_>>(), ["noor@example.org", "noor@example.org"]);
        }
        assert_eq!(page(&h), Some(Page::SomeoneElse));
        // This time the owner picks the right account: it is added as before.
        sign_ins(h.state_mut(), Some(Ok(signed_in_as("NOOR@EXAMPLE.ORG"))));
        click(&mut h, "Sign in again");
        assert_eq!(page(&h), Some(Page::Done));
        let account = h.state().session.mailbox.accounts[0].clone();
        assert_eq!((account.email.as_str(), account.username.as_str()), ("noor@example.org", "noor@example.org"));
        assert!(h.state().tokens.contains_key(&account.id));
    }

    /// (#103) "Use …": the account set up is the one that signed in, by its own address and the
    /// name Microsoft gave, so what the window shows is who it is. (An address that is another
    /// name of the same mailbox gets there the same way.)
    #[test]
    fn use_sets_up_the_account_that_signed_in() {
        let mut a = app(false);
        a.services.microsoft_client_id = APP_ID.into();
        lookups(&mut a, Some(Ok(vec![MICROSOFT_MX])));
        sign_ins(&mut a, Some(Ok(signed_in_as("admin@example.org"))));
        let synced = syncs(&mut a);
        let jobs: Log<Option<mailcraft_model::sync::GraphMailRequest>> = Default::default();
        let log = jobs.clone();
        a.services.graph = Some(Box::new(move |req: crate::GraphRequest| {
            log.lock().unwrap().push(req.mail.clone());
            mpsc::channel().1
        }));
        let mut h = open(a, json!({"displayName": "Noor Haddad"}));
        connect(&mut h, "noor@example.org");
        h.run_steps(4);
        assert_eq!(page(&h), Some(Page::SomeoneElse));
        // Taken meanwhile: said on the page, which stays, for the address typed.
        h.state_mut().execute("account.add", json!({"email": "admin@example.org"})).unwrap();
        click(&mut h, "Use admin@example.org");
        assert_eq!(page(&h), Some(Page::SomeoneElse));
        assert!(says(&h, "already set up") && says(&h, "not noor@example.org"));
        let taken = h.state().session.mailbox.accounts.iter().find(|a| a.email == "admin@example.org").map(|a| a.id).unwrap();
        h.state_mut().execute("account.edit", json!({"id": taken, "email": "noor.old@example.org"})).unwrap();
        click(&mut h, "Use admin@example.org");
        assert_eq!(page(&h), Some(Page::Done));
        assert!(says(&h, "admin@example.org is set up"));
        let account = h.state().session.mailbox.accounts.iter().find(|a| a.email == "admin@example.org").cloned().unwrap();
        assert_eq!((account.display_name.as_str(), account.username.as_str()), ("Ada Admin", "admin@example.org"));
        assert!(!h.state().session.mailbox.accounts.iter().any(|a| a.email == "noor@example.org"));
        assert_eq!(h.state().tokens.get(&account.id).map(|t| t.access.as_str()), Some("tok-1"));
        // Its first run is through Graph (R3): nothing by IMAP.
        assert!(synced.lock().unwrap().is_empty());
        assert_eq!(jobs.lock().unwrap().iter().filter_map(|mail| mail.as_ref().map(|m| m.account)).collect::<Vec<_>>(), [account.id]);
    }

    /// (#103) A sign-in that doesn't say who signed in is taken as the address typed, as before.
    #[test]
    fn a_sign_in_that_names_nobody_adds_the_address_typed() {
        let mut a = app(false);
        a.services.microsoft_client_id = APP_ID.into();
        lookups(&mut a, Some(Ok(vec![MICROSOFT_MX])));
        sign_ins(&mut a, Some(Ok(signed_in_as(""))));
        syncs(&mut a);
        let mut h = open(a, json!({}));
        connect(&mut h, "noor@example.org");
        h.run_steps(4);
        assert_eq!(page(&h), Some(Page::Done));
        assert_eq!(addresses(&h), ["noor@example.org"]);
    }

    /// (#44) Google refuses an everyday password where 2-Step Verification is on: the page says
    /// an app password is wanted, in plain words, and links to where one is made.
    #[test]
    fn google_asks_for_an_app_password_in_plain_words() {
        let mut a = app(false);
        let refusal = "sign-in failed: [ALERT] Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)";
        let _tried = checks(&mut a, Some(Err(refusal)));
        let mut h = open(a, json!({}));
        connect(&mut h, "noor@gmail.com");
        type_password(&mut h);
        press(&mut h, Key::Enter);
        assert_eq!(page(&h), Some(Page::Password));
        assert!(says(&h, "Google didn't accept the password") && says(&h, GOOGLE_APP_PASSWORDS) && !says(&h, "[ALERT]"));
        // Going back clears it.
        click(&mut h, "Back");
        assert!(wizard(&h).is_some_and(|w| !w.app_passwords));
        assert!(is_google("imap.gmail.com") && is_google("IMAP.GMAIL.COM.") && !is_google("imap.example.org") && !is_google("notgmail.com"));
    }

    /// (b) A provider that takes a password: the servers are tried before the account is added.
    #[test]
    fn a_password_account_is_tried_before_it_is_added() {
        let logged = crate::graph_switch::tests::logged();
        let mut a = app(false);
        lookups(&mut a, Some(Ok(vec!["aspmx.l.google.com"])));
        let tried = checks(&mut a, Some(Err("imap.gmail.com refused the sign-in")));
        let synced = syncs(&mut a);
        let mut h = open(a, json!({}));
        connect(&mut h, "noor@example.org");
        assert_eq!(page(&h), Some(Page::Password));
        assert!(says(&h, "Google Workspace account") && says(&h, "app password") && !says(&h, "couldn't find the settings"));
        assert_eq!(wizard(&h).map(|w| (w.settings_open, w.guessed)), Some((false, false)));
        // Connect waits for a password.
        press(&mut h, Key::Enter);
        assert_eq!((page(&h), tried.lock().unwrap().len()), (Some(Page::Password), 0));
        type_password(&mut h);
        // The password is on screen as dots only.
        assert!(!says(&h, TYPED) && h.get_by_role(Role::PasswordInput).value().is_none_or(|shown| !shown.contains(TYPED)));
        press(&mut h, Key::Enter);
        // Refused: back on the page with the reason and the servers in view, and no account.
        assert_eq!(page(&h), Some(Page::Password));
        assert!(says(&h, "refused the sign-in") && wizard(&h).is_some_and(|w| w.settings_open) && addresses(&h) == [""]);
        // The log keeps why, for a problem report, and never the password.
        {
            let log = logged.lock().unwrap().join("\n");
            assert!(log.contains("add account: the servers couldn't be checked: imap.gmail.com refused the sign-in"), "{log}");
            assert!(!log.contains(TYPED), "the password reached the log");
        }
        {
            let tried = tried.lock().unwrap();
            let (account, secret) = &tried[0];
            assert_eq!(
                (tried.len(), account.email.as_str(), account.username.as_str(), secret.as_str()),
                (1, "noor@example.org", "noor@example.org", TYPED)
            );
            assert_eq!(account.incoming, Some(Server { host: "imap.gmail.com".into(), port: 993, security: Security::Tls }));
            assert_eq!(account.outgoing, Some(Server { host: "smtp.gmail.com".into(), port: 465, security: Security::Tls }));
            assert!(account.oauth.is_none());
        }
        // The servers take it now: the account is added, and its first download has the password.
        let tried = checks(h.state_mut(), Some(Ok(())));
        click(&mut h, "Connect");
        assert_eq!((page(&h), tried.lock().unwrap().len()), (Some(Page::Done), 1));
        let s = &h.state().session;
        assert_eq!(s.mailbox.accounts.len(), 1);
        let account = &s.mailbox.accounts[0];
        assert_eq!((account.email.as_str(), account.kind, account.oauth.is_none()), ("noor@example.org", AccountKind::Imap, true));
        assert_eq!(account.incoming.as_ref().map(|s| s.host.as_str()), Some("imap.gmail.com"));
        {
            let synced = synced.lock().unwrap();
            assert_eq!((synced.len(), synced[0].0.id, synced[0].1.as_str(), synced[0].2.as_str()), (1, account.id, "", TYPED));
        }
        assert!(says(&h, "Your mail is downloading now") && h.state().syncing());
        // Nothing of the password is left in the wizard or in what its field remembered.
        assert!(wizard(&h).is_some_and(|w| w.password.0.is_empty()));
        assert!(TextEdit::load_state(&h.ctx, password_id()).is_none());
        press(&mut h, Key::Enter);
        assert_eq!(page(&h), None);
    }

    /// (b, continued) A domain nobody knows gets the usual server names, for the owner to check.
    #[test]
    fn an_unknown_domain_gets_a_guess_to_check() {
        for answer in [Some(Ok(vec!["mx1.lumen.example", "mx2.lumen.example"])), Some(Err("no such domain")), Some(Ok(Vec::new()))] {
            let mut a = app(false);
            let asked = lookups(&mut a, answer);
            checks(&mut a, None);
            let mut h = open(a, json!({}));
            connect(&mut h, "noor@lumen.example");
            assert_eq!(page(&h), Some(Page::Password));
            assert_eq!(*asked.lock().unwrap(), ["lumen.example"]);
            assert!(says(&h, "Sign in to your account") && says(&h, "couldn't find the settings"));
            let w = wizard(&h).unwrap();
            assert!(w.settings_open && w.guessed && w.oauth.is_none());
            assert_eq!(
                (w.username.as_str(), w.incoming.host.as_str(), w.outgoing.host.as_str()),
                ("noor@lumen.example", "imap.lumen.example", "smtp.lumen.example")
            );
            assert_eq!(
                (w.incoming.port.as_str(), w.incoming.security, w.outgoing.port.as_str(), w.outgoing.security),
                ("993", Security::Tls, "587", Security::StartTls)
            );
            // While the host is trying the servers, Cancel returns to them.
            type_password(&mut h);
            press(&mut h, Key::Enter);
            assert_eq!(page(&h), Some(Page::Checking));
            assert!(says(&h, "Checking the account") && says(&h, "Signing in to imap.lumen.example"));
            click(&mut h, "Cancel");
            assert_eq!((page(&h), addresses(&h)), (Some(Page::Password), vec![String::new()]));
            // Back gives the address up, and the password with it.
            click(&mut h, "Back");
            assert_eq!(page(&h), Some(Page::Address));
            assert!(wizard(&h).is_some_and(|w| w.password.0.is_empty()));
        }
    }

    /// (c) Manual setup: every tile leads where its kind of account signs in.
    #[test]
    fn each_kind_of_account_can_be_picked_by_hand() {
        for (kind, service, incoming, outgoing) in [
            (Kind::Microsoft365, "Microsoft 365", "outlook.office365.com", "smtp.office365.com"),
            (Kind::OutlookCom, "Outlook.com", "outlook.office365.com", "smtp-mail.outlook.com"),
            (Kind::Google, "Gmail", "imap.gmail.com", "smtp.gmail.com"),
            (Kind::Yahoo, "Yahoo", "imap.mail.yahoo.com", "smtp.mail.yahoo.com"),
            (Kind::ICloud, "iCloud", "imap.mail.me.com", "smtp.mail.me.com"),
            (Kind::Other, "", "imap.example.org", "smtp.example.org"),
        ] {
            let mut a = app(true);
            a.services.microsoft_client_id = APP_ID.into();
            // Nobody is asked whose the domain is when the owner says so.
            let looked = lookups(&mut a, None);
            let signed = sign_ins(&mut a, None);
            checks(&mut a, None);
            let before = a.session.mailbox.accounts.len();
            let mut h = open(a, json!({"email": "noor@example.org"}));
            connect_manually(&mut h);
            click(&mut h, kind.title());
            let microsoft = matches!(kind, Kind::Microsoft365 | Kind::OutlookCom);
            assert_eq!(page(&h), Some(if microsoft { Page::SignIn } else { Page::Password }), "{kind:?}");
            let w = wizard(&h).unwrap();
            assert_eq!((w.service.as_str(), w.incoming.host.as_str(), w.outgoing.host.as_str()), (service, incoming, outgoing), "{kind:?}");
            assert_eq!((w.oauth.is_some(), w.waiting, w.settings_open, w.guessed), (microsoft, microsoft, kind == Kind::Other, false), "{kind:?}");
            assert_eq!((signed.lock().unwrap().len(), h.state().signing_in()), (usize::from(microsoft), microsoft));
            assert!(looked.lock().unwrap().is_empty() && h.state().session.mailbox.accounts.len() == before);
            if microsoft {
                click(&mut h, "Cancel");
                assert_eq!(page(&h), Some(Page::Address));
                assert!(!h.state().signing_in());
            } else {
                // Back returns to the tiles.
                click(&mut h, "Back");
                assert_eq!(page(&h), Some(Page::Choose));
            }
        }
    }

    /// (d) A mailbox on this device only: no server is asked anything.
    #[test]
    fn this_device_only_adds_a_local_mailbox() {
        let mut a = app(false);
        let looked = lookups(&mut a, None);
        let tried = checks(&mut a, None);
        let synced = syncs(&mut a);
        let mut h = open(a, json!({"email": "noor@example.org", "displayName": "Noor Haddad"}));
        connect_manually(&mut h);
        click(&mut h, Kind::Local.title());
        assert_eq!(page(&h), Some(Page::Done));
        assert!(says(&h, "noor@example.org is set up") && says(&h, "nothing is sent to or fetched from a server"));
        let s = &h.state().session;
        assert_eq!(s.mailbox.accounts.len(), 1);
        let account = &s.mailbox.accounts[0];
        assert_eq!((account.email.as_str(), account.display_name.as_str(), account.kind), ("noor@example.org", "Noor Haddad", AccountKind::Local));
        assert!(account.incoming.is_none() && account.outgoing.is_none() && account.oauth.is_none() && account.username.is_empty());
        assert!(looked.lock().unwrap().is_empty() && tried.lock().unwrap().is_empty() && synced.lock().unwrap().is_empty() && !h.state().syncing());
        // One more: the wizard starts over, with the owner's name from the account just added.
        h.get_by_role_and_label(Role::Link, "Add another account").click();
        h.run_steps(4);
        assert_eq!(wizard(&h).map(|w| (w.page, w.email.as_str(), w.name.as_str(), w.manual)), Some((Page::Address, "", "Noor Haddad", false)));
        // The same address again is refused where it is asked for.
        type_text(&mut h, "NOOR@example.org");
        connect_manually(&mut h);
        click(&mut h, Kind::Local.title());
        assert_eq!(page(&h), Some(Page::Choose));
        assert!(says(&h, "already set up") && addresses(&h) == ["noor@example.org"]);
    }

    /// (e) Microsoft shows no sign-in page without an application ID: the wizard asks for one.
    #[test]
    fn microsoft_without_an_application_id_asks_for_one() {
        let mut a = app(false);
        lookups(&mut a, Some(Ok(vec![MICROSOFT_MX])));
        let signed = sign_ins(&mut a, None);
        let mut h = open(a, json!({}));
        connect(&mut h, "noor@example.org");
        assert_eq!(page(&h), Some(Page::ApplicationId));
        assert!(says(&h, "registered application") && says(&h, "http://localhost") && says(&h, "Microsoft Entra"));
        assert!(signed.lock().unwrap().is_empty() && !h.state().signing_in());
        // Something that isn't an ID is refused in place.
        type_text(&mut h, "not an id!");
        press(&mut h, Key::Enter);
        assert_eq!(page(&h), Some(Page::ApplicationId));
        assert!(says(&h, "letters, digits, dots and dashes") && h.state().ui.microsoft_client_id.is_empty());
        // A real one is kept in the settings, and the sign-in goes out with it.
        wizard_mut(&mut h).client_id = APP_ID.into();
        click(&mut h, "Continue");
        assert_eq!((page(&h), wizard(&h).map(|w| w.waiting)), (Some(Page::SignIn), Some(true)));
        assert_eq!(h.state().ui.microsoft_client_id, APP_ID);
        assert!(
            h.state().signing_in() && says(&h, "Microsoft's sign-in page is open in your browser") && says(&h, "Sign in there as noor@example.org")
        );
        {
            let signed = signed.lock().unwrap();
            assert_eq!((signed.len(), signed[0].0.as_str(), signed[0].1.as_str()), (1, "noor@example.org", APP_ID));
        }
        // Next time the ID is there: straight to the sign-in.
        click(&mut h, "Cancel");
        click(&mut h, "Connect");
        assert_eq!((page(&h), signed.lock().unwrap().len()), (Some(Page::SignIn), 2));

        // Back from the ID page returns to the address without anything kept.
        let mut a = app(false);
        lookups(&mut a, Some(Ok(vec![MICROSOFT_MX])));
        sign_ins(&mut a, None);
        let mut h = open(a, json!({}));
        connect(&mut h, "noor@example.org");
        type_text(&mut h, APP_ID);
        click(&mut h, "Back");
        assert_eq!(page(&h), Some(Page::Address));
        assert!(h.state().ui.microsoft_client_id.is_empty() && addresses(&h) == [""]);
    }

    /// (f) Giving up a wait leaves no account behind and nobody waiting in the browser.
    #[test]
    fn giving_up_a_wait_leaves_nothing_behind() {
        // While the name server is asked.
        let mut a = app(false);
        let looked = lookups(&mut a, None);
        let mut h = open(a, json!({}));
        connect(&mut h, "noor@example.org");
        assert_eq!((page(&h), looked.lock().unwrap().len()), (Some(Page::Detecting), 1));
        assert!(says(&h, "Finding your account settings"));
        click(&mut h, "Cancel");
        assert_eq!((page(&h), addresses(&h)), (Some(Page::Address), vec![String::new()]));
        assert!(wizard(&h).is_some_and(|w| w.lookup.is_none() && !w.busy()));

        // While the owner is in the browser: Cancel, Escape, the window's close button, and Cancel
        // Sign-In from elsewhere.
        let waiting = || {
            let mut a = app(false);
            a.services.microsoft_client_id = APP_ID.into();
            lookups(&mut a, Some(Ok(vec![MICROSOFT_MX])));
            let signed = sign_ins(&mut a, None);
            let mut h = open(a, json!({}));
            connect(&mut h, "noor@example.org");
            assert_eq!((page(&h), wizard(&h).map(|w| w.waiting), h.state().signing_in()), (Some(Page::SignIn), Some(true), true));
            (h, signed)
        };
        let stopped = |signed: &SignIns| signed.lock().unwrap().iter().all(|(_, _, stop)| stop.load(Ordering::Relaxed));

        let (mut h, signed) = waiting();
        click(&mut h, "Cancel");
        assert_eq!((page(&h), h.state().signing_in(), stopped(&signed)), (Some(Page::Address), false, true));
        assert_eq!(addresses(&h), [""]);

        let (mut h, signed) = waiting();
        press(&mut h, Key::Escape);
        assert_eq!((page(&h), h.state().signing_in(), stopped(&signed)), (None, false, true));
        assert_eq!(addresses(&h), [""]);

        let (mut h, signed) = waiting();
        h.get_by_label("Close window").click();
        h.run_steps(4);
        assert_eq!((page(&h), h.state().signing_in(), stopped(&signed)), (None, false, true));

        let (mut h, signed) = waiting();
        h.state_mut().execute("ui.cancelSignIn", json!({})).unwrap();
        h.run_steps(3);
        assert_eq!((page(&h), h.state().signing_in(), stopped(&signed)), (Some(Page::Address), false, true));

        // A script that closes the dialog doesn't tell it: the next wizard gives the wait up.
        let (mut h, signed) = waiting();
        h.state_mut().execute("ui.closeDialog", json!({})).unwrap();
        h.run_steps(3);
        assert_eq!(page(&h), None);
        h.state_mut().execute("ui.dialog", json!({"name": "account"})).unwrap();
        h.run_steps(3);
        assert_eq!((page(&h), h.state().signing_in(), stopped(&signed)), (Some(Page::Address), false, true));

        // Another account's own sign-in is nobody's to give up.
        let (mut h, _) = waiting();
        click(&mut h, "Cancel");
        let id = h
            .state_mut()
            .execute("account.add", json!({"email": "ada@example.org", "auth": "oauth2", "incoming": {"host": "imap.example.org"}}))
            .unwrap()["id"]
            .clone();
        syncs(h.state_mut());
        assert_eq!(h.state_mut().execute("ui.sync", json!({"account": id})).unwrap()["pending"], "signIn");
        press(&mut h, Key::Escape);
        h.state_mut().execute("ui.dialog", json!({"name": "account"})).unwrap();
        h.run_steps(3);
        assert!(h.state().signing_in());
    }

    /// (g) A build without sockets (the browser): nothing is asked of a host, and an account
    /// with servers is added as it stands.
    #[test]
    fn a_build_without_services_adds_the_account_untried() {
        let mut h = open(app(false), json!({}));
        connect(&mut h, "noor@lumen.example");
        assert_eq!(page(&h), Some(Page::Password));
        // No password is asked for where none can be used.
        assert!(h.query_by_role(Role::PasswordInput).is_none() && says(&h, "The account can still be added"));
        click(&mut h, "Connect");
        assert_eq!(page(&h), Some(Page::Done));
        assert!(says(&h, "This build can't connect to mail servers, so the settings were kept without being tried"));
        let account = &h.state().session.mailbox.accounts[0];
        assert_eq!(
            (account.email.as_str(), account.kind, account.username.as_str()),
            ("noor@lumen.example", AccountKind::Imap, "noor@lumen.example")
        );
        assert_eq!(account.incoming, Some(Server { host: "imap.lumen.example".into(), port: 993, security: Security::Tls }));
        assert!(!h.state().syncing());
        click(&mut h, "Done");
        assert_eq!(page(&h), None);

        // A Microsoft account can't sign in here, and says so instead of asking for an application ID.
        let mut h = open(app(false), json!({"email": "ada@lumen.example"}));
        connect_manually(&mut h);
        click(&mut h, Kind::Microsoft365.title());
        assert_eq!((page(&h), wizard(&h).map(|w| w.waiting)), (Some(Page::SignIn), Some(false)));
        assert!(says(&h, "can't sign in") && !h.state().signing_in() && addresses(&h) == [""]);
        click(&mut h, "Set up manually");
        click(&mut h, Kind::Local.title());
        assert_eq!((page(&h), addresses(&h)), (Some(Page::Done), vec!["ada@lumen.example".to_string()]));
    }

    /// (g′) An account that signs in with Microsoft is done once it is added and its mail is under
    /// way, whichever kind it is: there is no step about Outlook's Favorites, which are read
    /// through Microsoft Graph once the owner agrees to it under the Favorites heading. Which kind
    /// of account it is goes down with it.
    #[test]
    fn an_account_that_signs_in_with_microsoft_is_done_once_it_is_added() {
        const ORGANISATION: &str = "3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b";
        // A wizard that has just signed `noor@example.org` in to the directory `tenant`, with a
        // host that could read Outlook's Favorites.
        let signed_in_to = |tenant: &str| {
            let mut a = app(false);
            a.services.microsoft_client_id = APP_ID.into();
            lookups(&mut a, Some(Ok(vec![MICROSOFT_MX])));
            let signed = sign_ins(&mut a, Some(Ok(SignedIn { tenant: tenant.into(), ..signed_in("Noor Haddad") })));
            let synced = syncs(&mut a);
            a.services.graph = Some(Box::new(|request| {
                let (tx, rx) = mpsc::channel();
                tx.send(crate::GraphResult { account: request.account.id, ..Default::default() }).unwrap();
                rx
            }));
            let mut h = open(a, json!({}));
            connect(&mut h, "noor@example.org");
            h.run_steps(6);
            (h, signed, synced)
        };
        let directory = |h: &Harness<'static, MailApp>| h.state().session.mailbox.accounts[0].oauth.as_ref().unwrap().directory;
        for (tenant, kind) in
            [(ORGANISATION, Directory::Organisation), (Directory::PERSONAL_TENANT, Directory::Personal), ("", Directory::Organisation)]
        {
            let (mut h, signed, synced) = signed_in_to(tenant);
            assert_eq!((page(&h), directory(&h)), (Some(Page::Done), kind), "{tenant}");
            // (Its first run is through Graph, R3: nothing by IMAP.)
            assert_eq!((addresses(&h), signed.lock().unwrap().len(), synced.lock().unwrap().len()), (vec!["noor@example.org".to_string()], 1, 0));
            for never in ["Show Outlook's Favorites?", "Exchange Web Services", "Not now"] {
                assert!(!says(&h, never), "{tenant}: {never}");
            }
            assert!(says(&h, "noor@example.org is set up"));
            click(&mut h, "Done");
            assert_eq!(page(&h), None);
        }
        // What was once the page of that step opens an account's settings like any other.
        let mut a = app(true);
        let work = json!({"email": "ada@lumen.example", "auth": "oauth2", "provider": "microsoft", "directory": "organisation", "incoming": {"host": "outlook.office365.com"}});
        let work = a.execute("account.add", work).unwrap()["id"].as_u64().unwrap();
        let w = AccountSetup::open(&a, &json!({"id": work, "page": "favorites"})).unwrap();
        assert_eq!(w.page, Page::Settings);
        assert!(AccountSetup::open(&a, &json!({"id": 999_999, "page": "favorites"})).is_none());
    }

    /// (h) Every page lays out, in both themes, in a comfortable window, a small one and one far
    /// too small, with ordinary text and with hostile text.
    #[test]
    fn every_page_lays_out_in_both_themes_and_sizes() {
        let junk = "\u{0}\u{202e}é\t<b>&amp;\n".repeat(300);
        let pages = [
            Page::Address,
            Page::Detecting,
            Page::Choose,
            Page::ApplicationId,
            Page::SignIn,
            Page::Approval,
            Page::SomeoneElse,
            Page::Password,
            Page::Checking,
            Page::Done,
            Page::Settings,
        ];
        for theme in ["light", "dark"] {
            for size in [(1280.0, 800.0), (760.0, 480.0), (320.0, 200.0)] {
                // Each page twice: as it comes, and full of hostile text. The sign-in page is
                // waiting the first time and has come to nothing the second; the approval page
                // says what to do, then what Microsoft answered a try at it; Account Settings is
                // for a password account, then for one that signs in on Microsoft's page.
                for (page_in_view, hostile) in pages.into_iter().flat_map(|p| [(p, false), (p, true)]) {
                    let mut a = app(true);
                    a.execute("ui.theme", json!({"value": theme})).unwrap();
                    sign_ins(&mut a, None);
                    checks(&mut a, None);
                    let cloud = json!({"email": "ada@lumen.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}});
                    let cloud = a.execute("account.add", cloud).unwrap()["id"].clone();
                    let params = if page_in_view == Page::Settings && hostile {
                        json!({"name": "account", "id": cloud})
                    } else {
                        json!({"name": "account", "email": "noor@example.org"})
                    };
                    a.execute("ui.dialog", params).unwrap();
                    let mut h = window(a, size);
                    let (_lookup, lookup) = mpsc::channel();
                    let (_check, check) = mpsc::channel();
                    let w = wizard_mut(&mut h);
                    if page_in_view == Page::Settings {
                        if !hostile {
                            let local = Server { host: "localhost".into(), port: 1143, security: Security::None };
                            *w =
                                AccountSetup::edit(&Account { id: 1, email: "noor@example.org".into(), incoming: Some(local), ..Default::default() });
                        }
                        drop((lookup, check));
                    } else {
                        w.page = page_in_view;
                        (w.advanced, w.settings_open, w.guessed) = (true, true, hostile);
                        w.oauth =
                            matches!(page_in_view, Page::SignIn | Page::ApplicationId | Page::Approval | Page::SomeoneElse).then(OAuth::default);
                        let who = if hostile { junk.clone() } else { "admin@example.org".to_string() };
                        w.someone_else = (page_in_view == Page::SomeoneElse).then(|| SignedIn { email: who, name: junk.clone(), ..signed_in("") });
                        (w.service, w.approval.said) = ("Microsoft 365".into(), hostile);
                        w.approval.refused = hostile.then(|| (junk.clone(), junk.clone()));
                        (w.ended, w.approved) = (hostile.then_some(SignInEnd::TimedOut), !hostile);
                        (w.lookup, w.check) = (Some(lookup), Some(check));
                        w.outcome = if hostile { Outcome::Offline } else { Outcome::Downloading };
                    }
                    if hostile {
                        for text in [&mut w.error, &mut w.note, &mut w.service, &mut w.name, &mut w.username, &mut w.client_id, &mut w.tenant] {
                            text.clone_from(&junk);
                        }
                        (w.incoming.host, w.incoming.port, w.outgoing.host) = (junk.clone(), junk.clone(), junk.clone());
                        if page_in_view != Page::Address {
                            w.email.clone_from(&junk);
                        }
                    } else if page_in_view == Page::SignIn {
                        h.state_mut().begin_setup_sign_in(OAuth::default(), "noor@example.org", false).unwrap();
                        wizard_mut(&mut h).waiting = true;
                    }
                    h.run_steps(4);
                    assert_eq!(page(&h), Some(page_in_view), "{theme} {size:?} {hostile}");
                    // Keys reach whatever has the keyboard, and nothing falls over.
                    type_text(&mut h, &junk);
                    press(&mut h, Key::Tab);
                    press(&mut h, Key::Enter);
                    press(&mut h, Key::Escape);
                    assert_eq!(page(&h), None);
                }
            }
        }
    }

    /// Account Settings in the window: the servers at once, a password field only for an account
    /// that has one, and Save.
    #[test]
    fn account_settings_open_on_the_servers() {
        let mut a = app(true);
        checks(&mut a, None);
        let synced = syncs(&mut a);
        let server = json!({"email": "noor@lumen.example", "displayName": "Noor", "incoming": {"host": "imap.lumen.example"}, "outgoing": {"host": "smtp.lumen.example"}});
        let id = a.execute("account.add", server).unwrap()["id"].as_u64().unwrap();
        let cloud = json!({"email": "ada@lumen.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}});
        let cloud = a.execute("account.add", cloud).unwrap()["id"].as_u64().unwrap();

        let mut h = open(a, json!({"id": id}));
        assert_eq!((page(&h), wizard(&h).map(|w| w.title())), (Some(Page::Settings), Some("Account Settings".to_string())));
        assert!(says(&h, "Signs in with a password") && says(&h, "Incoming mail (IMAP)") && !says(&h, "Tenant"));
        assert!(h.query_by_role(Role::PasswordInput).is_some());
        assert!(h.get_all_by_role(Role::TextInput).any(|field| field.value().as_deref() == Some("imap.lumen.example")));
        // A new name and a new password: the name goes into the account, the password to this
        // session's Send/Receive, and neither into the other's place.
        wizard_mut(&mut h).name = "Noor Haddad".into();
        type_password(&mut h);
        click(&mut h, "Save");
        assert_eq!(page(&h), None);
        assert!(TextEdit::load_state(&h.ctx, password_id()).is_none());
        let account = h.state().session.mailbox.account(id).unwrap().clone();
        assert_eq!((account.display_name.as_str(), account.incoming.as_ref().map(|s| s.host.as_str())), ("Noor Haddad", Some("imap.lumen.example")));
        assert!(!serde_json::to_string(&h.state().session.mailbox.accounts).unwrap().contains(TYPED));
        h.state_mut().execute("ui.sync", json!({"account": id})).unwrap();
        assert_eq!(synced.lock().unwrap()[0].2, TYPED);

        // An account that signs in on Microsoft's page has no password to type.
        h.state_mut().execute("ui.dialog", json!({"name": "account", "id": cloud})).unwrap();
        h.run_steps(4);
        assert_eq!(page(&h), Some(Page::Settings));
        assert!(says(&h, "Signs in on the Microsoft page") && says(&h, "Application (client) ID") && says(&h, "Tenant"));
        assert!(h.query_by_role(Role::PasswordInput).is_none());
        // Still on IMAP, it keeps its servers (#312).
        assert!(says(&h, "Incoming mail (IMAP)") && !says(&h, "through Microsoft Graph"));
        click(&mut h, "Cancel");
        assert_eq!(page(&h), None);
        // An account that isn't there has no settings to open.
        assert!(h.state_mut().execute("ui.dialog", json!({"name": "account", "id": 999_999})).is_err());
    }

    /// (#312) The settings of an account kept in step through Microsoft Graph show what applies
    /// to it: its name, its Microsoft sign-in and its mail to keep offline, and say how its mail
    /// comes; no user name and no mail servers, which it doesn't have. Save keeps it on Graph. A
    /// shared mailbox opened through it has no settings to open (#70).
    #[test]
    fn graph_account_settings_have_no_mail_servers() {
        let mut a = app(true);
        let id = a.execute("account.add", json!({"email": "ada@lumen.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        let mut h = open(a, json!({"id": id}));
        assert_eq!(page(&h), Some(Page::Settings));
        assert!(says(&h, "Signs in on the Microsoft page") && says(&h, "kept in step through Microsoft Graph"));
        assert!(says(&h, "Application (client) ID") && says(&h, "Tenant") && says(&h, "Mail to keep offline"));
        assert!(!says(&h, "User name") && !says(&h, "Incoming mail (IMAP)") && !says(&h, "Outgoing mail (SMTP)"));
        assert!(h.query_by_role(Role::PasswordInput).is_none());
        wizard_mut(&mut h).name = "Ada Lovelace".into();
        click(&mut h, "Save");
        assert_eq!(page(&h), None);
        let account = h.state().session.mailbox.account(id).unwrap();
        assert_eq!((account.display_name.as_str(), account.kind), ("Ada Lovelace", AccountKind::Microsoft));
        assert_eq!((account.incoming.as_ref(), account.outgoing.as_ref()), (None, None));

        let shared = h.state_mut().execute("account.addSharedMailbox", json!({"account": id, "address": "team@lumen.example"})).unwrap()["id"]
            .as_u64()
            .unwrap();
        assert!(h.state_mut().execute("ui.dialog", json!({"name": "account", "id": shared})).is_err());
        h.run_steps(4);
        assert_eq!(page(&h), None);
    }

    /// The settings of an account a newer MailCraft added say what it is and offer nothing to
    /// change: Close, or Enter, shuts them, and nothing is run.
    #[test]
    fn the_settings_of_an_account_a_newer_version_added_say_so_and_change_nothing() {
        let mut a = app(true);
        // (Nothing else on screen has a Close button.)
        let _ = a.execute("ui.dismissReminder", json!({"all": true}));
        let id = a.session.mailbox.add_account(Account { email: "ada@jmap.example".into(), kind: AccountKind::Unknown, ..Default::default() });
        let before = a.session.mailbox.accounts.clone();
        let mut h = open(a, json!({"id": id}));
        assert_eq!((page(&h), wizard(&h).map(|w| w.title())), (Some(Page::Settings), Some("Account Settings".to_string())));
        assert!(says(&h, "This account was added by a newer version of MailCraft") && says(&h, "this version tells no mail server"));
        assert!(!says(&h, "User name") && !says(&h, "Incoming mail (IMAP)") && h.query_by_role(Role::PasswordInput).is_none());
        assert!(wizard(&h).and_then(AccountSetup::edit_command).is_none());
        click(&mut h, "Close");
        assert_eq!(page(&h), None);
        h.state_mut().execute("ui.dialog", json!({"name": "account", "id": id})).unwrap();
        h.run_steps(4);
        press(&mut h, Key::Enter);
        assert_eq!(page(&h), None);
        assert_eq!(h.state().session.mailbox.accounts, before);
    }

    /// The password is typed in confidence: it is in no command, no debug text, and gone from the
    /// window's memory however the dialog goes.
    #[test]
    fn the_password_stays_out_of_reach() {
        assert_eq!((format!("{:?}", Secret(TYPED.into())), format!("{:?}", Secret::default())), ("…".to_string(), String::new()));
        let mut secret = Secret(TYPED.into());
        assert_eq!(secret.take(), TYPED);
        assert!(secret.0.is_empty());

        // Typed and left: by Escape, by Back, and by a script that closes the dialog.
        for leave in ["escape", "back", "closeDialog"] {
            let mut a = app(false);
            checks(&mut a, None);
            let mut h = open(a, json!({}));
            connect(&mut h, "noor@lumen.example");
            type_password(&mut h);
            let w = wizard(&h).unwrap();
            assert_eq!(w.password.0, TYPED);
            assert!(TextEdit::load_state(&h.ctx, password_id()).is_some());
            // The command the page runs, and the account the host is handed, don't carry it.
            let (_, params) = w.add_command("").unwrap();
            assert!(!params.to_string().contains(TYPED) && !format!("{:?}", w.account()).contains(TYPED));
            match leave {
                "escape" => press(&mut h, Key::Escape),
                "back" => click(&mut h, "Back"),
                _ => {
                    h.state_mut().execute("ui.closeDialog", json!({})).unwrap();
                    h.run_steps(2);
                }
            }
            assert!(TextEdit::load_state(&h.ctx, password_id()).is_none(), "{leave}");
            assert!(wizard(&h).is_none_or(|w| w.password.0.is_empty()), "{leave}");
            assert_eq!(addresses(&h), [""]);
        }
    }

    /// A key held down repeats; it must not carry the owner from page to page.
    #[test]
    fn a_held_enter_key_stays_on_the_page() {
        let mut h = open(app(false), json!({"email": "noor@lumen.example"}));
        // The press is Connect; the repeats that follow it are nothing on the page that comes.
        for _ in 0..8 {
            h.event(egui::Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: true, modifiers: egui::Modifiers::NONE });
            h.run_steps(3);
        }
        assert_eq!((page(&h), addresses(&h)), (Some(Page::Password), vec![String::new()]));
        h.event(egui::Event::Key { key: Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(2);
        // Let go and pressed again, it is Connect once more.
        press(&mut h, Key::Enter);
        assert_eq!((page(&h), addresses(&h)), (Some(Page::Done), vec!["noor@lumen.example".to_string()]));
    }
}
