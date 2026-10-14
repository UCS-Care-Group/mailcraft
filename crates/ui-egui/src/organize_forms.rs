//! The forms for the ways the owner organises mail: New Search Folder, Quick Steps (one, and the
//! list of them), contact groups, and Delay Delivery. Each collects what is typed and runs the
//! engine's commands with it; what a command refuses is shown in the form, which stays open.

use egui::{Ui, vec2};
use mailcraft_model::{Id, StepAction};
use serde_json::{Value, json};

use crate::MailApp;
use crate::forms::kit::{LABEL_W, body, close_button, error, field_id, footer, row, shown};
use crate::theme::Tokens;
use crate::widgets::icon_button;

/// One of the forms of this module.
pub enum OrganizeForm {
    SearchFolder(SearchFolderForm),
    QuickStep(QuickStepForm),
    QuickSteps(QuickStepsForm),
    Group(GroupForm),
    Delay(DelayForm),
}

impl OrganizeForm {
    /// The form called `name`, when it is one of these: `newSearchFolder`, `editSearchFolder`,
    /// `quickStep` (new, or `params.id`), `quickSteps`, `contactGroup`, `editContactGroup`,
    /// `delayDelivery`.
    pub fn open(name: &str, app: &MailApp, params: &Value) -> Option<OrganizeForm> {
        let id = params.get("id").and_then(Value::as_u64);
        Some(match name {
            "newSearchFolder" => OrganizeForm::SearchFolder(SearchFolderForm::default()),
            "editSearchFolder" => OrganizeForm::SearchFolder(SearchFolderForm::edit(app, id.unwrap_or(app.session.view.folder))?),
            "quickStep" => OrganizeForm::QuickStep(match id {
                Some(id) => QuickStepForm::edit(app, id)?,
                None => QuickStepForm::new(),
            }),
            "quickSteps" => OrganizeForm::QuickSteps(QuickStepsForm::default()),
            "contactGroup" => OrganizeForm::Group(GroupForm::default()),
            "editContactGroup" => OrganizeForm::Group(GroupForm::edit(app, id.or(app.session.sel.group)?)?),
            "delayDelivery" => OrganizeForm::Delay(DelayForm::new(app)),
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            OrganizeForm::SearchFolder(f) if f.editing.is_some() => "editSearchFolder",
            OrganizeForm::SearchFolder(_) => "newSearchFolder",
            OrganizeForm::QuickStep(_) => "quickStep",
            OrganizeForm::QuickSteps(_) => "quickSteps",
            OrganizeForm::Group(f) if f.editing.is_some() => "editContactGroup",
            OrganizeForm::Group(_) => "contactGroup",
            OrganizeForm::Delay(_) => "delayDelivery",
        }
    }

    pub fn title(&self) -> String {
        match self {
            OrganizeForm::SearchFolder(f) if f.editing.is_some() => tl!("Customize Search Folder").to_string(),
            OrganizeForm::SearchFolder(_) => tl!("New Search Folder").to_string(),
            OrganizeForm::QuickStep(f) if f.editing.is_some() => tl!("Edit Quick Step").to_string(),
            OrganizeForm::QuickStep(_) => tl!("New Quick Step").to_string(),
            OrganizeForm::QuickSteps(_) => tl!("Manage Quick Steps").to_string(),
            OrganizeForm::Group(f) if f.editing.is_some() => tl!("Contact Group").to_string(),
            OrganizeForm::Group(_) => tl!("New Contact Group").to_string(),
            OrganizeForm::Delay(_) => tl!("Delay Delivery").to_string(),
        }
    }

    pub fn width(&self) -> f32 {
        match self {
            OrganizeForm::SearchFolder(_) => 480.0,
            OrganizeForm::QuickStep(_) | OrganizeForm::QuickSteps(_) => 560.0,
            OrganizeForm::Group(_) => 520.0,
            OrganizeForm::Delay(_) => 400.0,
        }
    }

    /// Lay out the form; `true` when the dialog should close.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let next = match self {
            OrganizeForm::SearchFolder(f) => return f.show(app, ui),
            OrganizeForm::QuickStep(f) => return f.show(app, ui),
            OrganizeForm::Group(f) => return f.show(app, ui),
            OrganizeForm::Delay(f) => return f.show(app, ui),
            OrganizeForm::QuickSteps(f) => match f.show(app, ui) {
                Managed::Stay => return false,
                Managed::Close => return true,
                Managed::Open(step) => step,
            },
        };
        // A step of the list is edited (or one is made) in the same dialog.
        *self = OrganizeForm::QuickStep(next);
        false
    }
}

// ---- search folders ----

/// The kinds of search folder New Search Folder offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Template {
    #[default]
    Unread,
    Flagged,
    Important,
    Attachments,
    Categorized,
    Large,
    From,
    Custom,
}

impl Template {
    const ALL: [Template; 8] = [
        Template::Unread,
        Template::Flagged,
        Template::Important,
        Template::Attachments,
        Template::Categorized,
        Template::Large,
        Template::From,
        Template::Custom,
    ];

    fn code(self) -> &'static str {
        match self {
            Template::Unread => "unread",
            Template::Flagged => "flagged",
            Template::Important => "important",
            Template::Attachments => "attachments",
            Template::Categorized => "categorized",
            Template::Large => "large",
            Template::From => "from",
            Template::Custom => "custom",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Template::Unread => tn!("Unread mail"),
            Template::Flagged => tn!("Mail flagged for follow up"),
            Template::Important => tn!("Important mail"),
            Template::Attachments => tn!("Mail with attachments"),
            Template::Categorized => tn!("Categorized mail"),
            Template::Large => tn!("Large mail"),
            Template::From => tn!("Mail from specific people"),
            Template::Custom => tn!("A search of my own"),
        }
    }
}

/// New Search Folder, and Customize for one that exists (its name and its search).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchFolderForm {
    pub editing: Option<Id>,
    pub template: Template,
    pub people: String,
    pub size: String,
    pub category: String,
    pub query: String,
    pub name: String,
    pub account: Option<Id>,
    pub error: String,
}

impl Default for SearchFolderForm {
    fn default() -> Self {
        SearchFolderForm {
            editing: None,
            template: Template::Unread,
            people: String::new(),
            size: "100".into(),
            category: String::new(),
            query: String::new(),
            name: String::new(),
            account: None,
            error: String::new(),
        }
    }
}

impl SearchFolderForm {
    fn edit(app: &MailApp, id: Id) -> Option<SearchFolderForm> {
        let f = mailcraft_engine::cmd::search_folders::find(&app.session, id)?;
        Some(SearchFolderForm {
            editing: Some(f.id),
            template: Template::Custom,
            query: f.query.clone(),
            name: f.name.clone(),
            account: f.account,
            ..Default::default()
        })
    }

    /// The command OK runs.
    pub fn command(&self) -> (&'static str, Value) {
        if let Some(id) = self.editing {
            return ("searchFolder.edit", json!({"id": id, "name": self.name, "query": self.query, "account": self.account}));
        }
        let mut v = json!({"template": self.template.code(), "account": self.account});
        if let Some(o) = v.as_object_mut() {
            if !self.name.trim().is_empty() {
                o.insert("name".into(), json!(self.name));
            }
            match self.template {
                Template::From => {
                    o.insert("people".into(), json!(self.people));
                }
                Template::Large => {
                    o.insert("size".into(), json!(self.size.trim().parse::<u64>().unwrap_or(100)));
                }
                Template::Categorized if !self.category.is_empty() => {
                    o.insert("category".into(), json!(self.category));
                }
                Template::Custom => {
                    o.insert("query".into(), json!(self.query));
                }
                _ => {}
            }
        }
        ("folder.newSearchFolder", v)
    }

    fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        body(ui, |ui| {
            if self.editing.is_none() {
                ui.label(tl!("Show mail that is:"));
                for template in Template::ALL {
                    ui.radio_value(&mut self.template, template, tl!(template.label()));
                }
                ui.add_space(6.0);
            }
            let width = (ui.available_width() - LABEL_W - 8.0).max(80.0);
            match self.template {
                Template::From => row(ui, tl!("From"), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.people)
                            .id(field_id("sf_people"))
                            .desired_width(width)
                            .hint_text(tl!("Names or addresses, separated by ;")),
                    );
                }),
                Template::Large => row(ui, tl!("Larger than (KB)"), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.size).id(field_id("sf_size")).desired_width(80.0));
                }),
                Template::Categorized => row(ui, tl!("Category"), |ui| {
                    let any = tl!("Any category");
                    let shown_name = if self.category.is_empty() { any.to_string() } else { self.category.clone() };
                    egui::ComboBox::from_id_salt(field_id("sf_category")).width(width.min(240.0)).selected_text(shown_name).show_ui(ui, |ui| {
                        if ui.selectable_label(self.category.is_empty(), any).clicked() {
                            self.category.clear();
                        }
                        for c in &app.session.mailbox.categories {
                            if ui.selectable_label(self.category == c.name, &c.name).clicked() {
                                self.category = c.name.clone();
                            }
                        }
                    });
                }),
                Template::Custom => {
                    row(ui, tl!("Search for"), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.query)
                                .id(field_id("sf_query"))
                                .desired_width(width)
                                .hint_text("is:unread from:ada larger:2mb"),
                        );
                    });
                    ui.label(
                        egui::RichText::new(tl!(
                            "Words, \"phrases\", from:, to:, subject:, category:, has:attachment, is:unread, is:flagged, larger:, before:, after:"
                        ))
                        .color(t.text_dim)
                        .small(),
                    );
                }
                _ => {}
            }
            row(ui, tl!("Name"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.name).id(field_id("sf_name")).desired_width(width).hint_text(tl!("After what it shows")));
            });
            let accounts: Vec<(Id, String)> = app.session.mailbox.accounts.iter().map(|a| (a.id, a.name.clone())).collect();
            if accounts.len() > 1 {
                row(ui, tl!("Look in"), |ui| {
                    let all = tl!("Every account");
                    let current = accounts.iter().find(|(id, _)| Some(*id) == self.account).map_or(all.to_string(), |(_, n)| n.clone());
                    egui::ComboBox::from_id_salt(field_id("sf_account")).width(width.min(240.0)).selected_text(current).show_ui(ui, |ui| {
                        if ui.selectable_label(self.account.is_none(), all).clicked() {
                            self.account = None;
                        }
                        for (id, name) in &accounts {
                            if ui.selectable_label(self.account == Some(*id), name).clicked() {
                                self.account = Some(*id);
                            }
                        }
                    });
                });
            }
        });
        error(ui, &self.error);
        let f = footer(ui, tl!("OK"), true, false);
        if f.cancel {
            return true;
        }
        if f.ok {
            let (command, params) = self.command();
            match app.execute(command, params) {
                Ok(_) => return true,
                Err(e) => self.error = e,
            }
        }
        false
    }
}

// ---- Quick Steps ----

/// What one row of a Quick Step does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StepKind {
    #[default]
    MoveTo,
    CopyTo,
    MarkRead,
    MarkUnread,
    Flag,
    ClearFlag,
    MarkComplete,
    Categorize,
    ClearCategories,
    Delete,
    Archive,
    Reply,
    ReplyAll,
    Forward,
}

impl StepKind {
    const ALL: [StepKind; 14] = [
        StepKind::MoveTo,
        StepKind::CopyTo,
        StepKind::MarkRead,
        StepKind::MarkUnread,
        StepKind::Flag,
        StepKind::ClearFlag,
        StepKind::MarkComplete,
        StepKind::Categorize,
        StepKind::ClearCategories,
        StepKind::Delete,
        StepKind::Archive,
        StepKind::Reply,
        StepKind::ReplyAll,
        StepKind::Forward,
    ];

    fn label(self) -> &'static str {
        match self {
            StepKind::MoveTo => tn!("Move to folder"),
            StepKind::CopyTo => tn!("Copy to folder"),
            StepKind::MarkRead => tn!("Mark as read"),
            StepKind::MarkUnread => tn!("Mark as unread"),
            StepKind::Flag => tn!("Flag"),
            StepKind::ClearFlag => tn!("Clear flag"),
            StepKind::MarkComplete => tn!("Mark complete"),
            StepKind::Categorize => tn!("Categorize"),
            StepKind::ClearCategories => tn!("Clear categories"),
            StepKind::Delete => tn!("Delete"),
            StepKind::Archive => tn!("Archive"),
            StepKind::Reply => tn!("Reply"),
            StepKind::ReplyAll => tn!("Reply all"),
            StepKind::Forward => tn!("Forward"),
        }
    }

    fn code(self) -> &'static str {
        match self {
            StepKind::MoveTo => "moveTo",
            StepKind::CopyTo => "copyTo",
            StepKind::MarkRead => "markRead",
            StepKind::MarkUnread => "markUnread",
            StepKind::Flag => "flag",
            StepKind::ClearFlag => "clearFlag",
            StepKind::MarkComplete => "markComplete",
            StepKind::Categorize => "categorize",
            StepKind::ClearCategories => "clearCategories",
            StepKind::Delete => "delete",
            StepKind::Archive => "archive",
            StepKind::Reply => "reply",
            StepKind::ReplyAll => "replyAll",
            StepKind::Forward => "forward",
        }
    }
}

/// One row of a Quick Step being made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepRow {
    pub kind: StepKind,
    pub folder: Id,
    pub category: String,
    pub to: String,
}

impl StepRow {
    fn of(action: &StepAction) -> StepRow {
        let row = |kind| StepRow { kind, ..Default::default() };
        match action {
            StepAction::MoveTo(f) => StepRow { folder: *f, ..row(StepKind::MoveTo) },
            StepAction::CopyTo(f) => StepRow { folder: *f, ..row(StepKind::CopyTo) },
            StepAction::MarkRead => row(StepKind::MarkRead),
            StepAction::MarkUnread => row(StepKind::MarkUnread),
            StepAction::Flag => row(StepKind::Flag),
            StepAction::ClearFlag => row(StepKind::ClearFlag),
            StepAction::MarkComplete => row(StepKind::MarkComplete),
            StepAction::Categorize(c) => StepRow { category: c.clone(), ..row(StepKind::Categorize) },
            StepAction::ClearCategories => row(StepKind::ClearCategories),
            StepAction::Delete => row(StepKind::Delete),
            StepAction::Archive => row(StepKind::Archive),
            StepAction::Reply => row(StepKind::Reply),
            StepAction::ReplyAll => row(StepKind::ReplyAll),
            StepAction::Forward(to) => StepRow { to: to.clone(), ..row(StepKind::Forward) },
        }
    }

    fn json(&self) -> Value {
        match self.kind {
            StepKind::MoveTo | StepKind::CopyTo => json!({"type": self.kind.code(), "value": self.folder}),
            StepKind::Categorize => json!({"type": self.kind.code(), "value": self.category}),
            StepKind::Forward => json!({"type": self.kind.code(), "value": self.to}),
            _ => json!({"type": self.kind.code()}),
        }
    }
}

/// A Quick Step, new or being edited: its name and what it does, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuickStepForm {
    pub editing: Option<Id>,
    pub name: String,
    pub rows: Vec<StepRow>,
    pub error: String,
}

impl QuickStepForm {
    fn new() -> QuickStepForm {
        QuickStepForm { rows: vec![StepRow::default()], ..Default::default() }
    }

    fn edit(app: &MailApp, id: Id) -> Option<QuickStepForm> {
        let q = app.session.mailbox.quick_steps.iter().find(|q| q.id == id)?;
        Some(QuickStepForm { editing: Some(q.id), name: q.name.clone(), rows: q.actions.iter().map(StepRow::of).collect(), error: String::new() })
    }

    /// The command OK runs.
    pub fn command(&self) -> (&'static str, Value) {
        let actions: Vec<Value> = self.rows.iter().map(StepRow::json).collect();
        match self.editing {
            Some(id) => ("quickSteps.edit", json!({"id": id, "name": self.name, "actions": actions})),
            None => ("quickSteps.new", json!({"name": self.name, "actions": actions})),
        }
    }

    fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let folders: Vec<(Id, String)> = app.session.mailbox.folders.iter().map(|f| (f.id, app.session.mailbox.folder_path(f.id))).collect();
        let categories: Vec<String> = app.session.mailbox.categories.iter().map(|c| c.name.clone()).collect();
        body(ui, |ui| {
            let width = (ui.available_width() - LABEL_W - 8.0).max(80.0);
            row(ui, tl!("Name"), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.name).id(field_id("qs_name")).desired_width(width).hint_text(tl!("What the button says")),
                );
            });
            ui.add_space(4.0);
            ui.label(tl!("Actions, in order:"));
            let mut remove = None;
            let count = self.rows.len();
            for (i, r) in self.rows.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt(field_id(("qs_kind", i))).width(150.0).selected_text(tl!(r.kind.label())).show_ui(ui, |ui| {
                        for kind in StepKind::ALL {
                            if ui.selectable_label(r.kind == kind, tl!(kind.label())).clicked() {
                                r.kind = kind;
                            }
                        }
                    });
                    let rest = (ui.available_width() - 34.0).max(60.0);
                    match r.kind {
                        StepKind::MoveTo | StepKind::CopyTo => {
                            let current =
                                folders.iter().find(|(id, _)| *id == r.folder).map_or(tl!("Choose a folder").to_string(), |(_, p)| shown(p, 60));
                            egui::ComboBox::from_id_salt(field_id(("qs_folder", i))).width(rest).selected_text(current).show_ui(ui, |ui| {
                                for (id, path) in &folders {
                                    if ui.selectable_label(r.folder == *id, shown(path, 80)).clicked() {
                                        r.folder = *id;
                                    }
                                }
                            });
                        }
                        StepKind::Categorize => {
                            let current = if r.category.is_empty() { tl!("Choose a category").to_string() } else { r.category.clone() };
                            egui::ComboBox::from_id_salt(field_id(("qs_category", i))).width(rest).selected_text(current).show_ui(ui, |ui| {
                                for c in &categories {
                                    if ui.selectable_label(r.category == *c, c).clicked() {
                                        r.category = c.clone();
                                    }
                                }
                            });
                        }
                        StepKind::Forward => {
                            ui.add(
                                egui::TextEdit::singleline(&mut r.to).id(field_id(("qs_to", i))).desired_width(rest).hint_text(tl!("To (optional)")),
                            );
                        }
                        _ => {
                            ui.add_space(rest);
                        }
                    }
                    if icon_button(ui, "delete", tl!("Remove this action"), 14.0, count > 1).clicked() && count > 1 {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.rows.remove(i);
            }
            if self.rows.len() < mailcraft_model::MAX_STEP_ACTIONS && ui.button(tl!("Add Action")).clicked() {
                self.rows.push(StepRow { kind: StepKind::MarkRead, ..Default::default() });
            }
        });
        error(ui, &self.error);
        let f = footer(ui, tl!("Save"), !self.name.trim().is_empty() && !self.rows.is_empty(), false);
        if f.cancel {
            return true;
        }
        if f.ok {
            let (command, params) = self.command();
            match app.execute(command, params) {
                Ok(_) => return true,
                Err(e) => self.error = e,
            }
        }
        false
    }
}

/// What came of the list of Quick Steps.
enum Managed {
    Stay,
    Close,
    /// Make or edit one, in the same dialog.
    Open(QuickStepForm),
}

/// Manage Quick Steps: the list, in the order the Home tab shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuickStepsForm {
    pub error: String,
}

impl QuickStepsForm {
    fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> Managed {
        let t = Tokens::get(ui.ctx());
        let steps: Vec<(Id, String, String)> = app.session.mailbox.quick_steps.iter().map(|q| (q.id, q.name.clone(), q.describe())).collect();
        let mut run: Option<(&'static str, Value)> = None;
        let mut open = None;
        body(ui, |ui| {
            if steps.is_empty() {
                ui.label(egui::RichText::new(tl!("No Quick Steps yet")).color(t.text_dim));
            }
            let last = steps.len().saturating_sub(1);
            let text_w = (ui.available_width() - 130.0).max(120.0);
            for (i, (id, name, text)) in steps.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_min_width(text_w);
                        ui.set_max_width(text_w);
                        ui.label(egui::RichText::new(shown(name, 80)).strong());
                        ui.label(egui::RichText::new(shown(text, 120)).color(t.text_dim).small());
                    });
                    if icon_button(ui, "edit", tl!("Edit"), 14.0, true).clicked() {
                        open = QuickStepForm::edit(app, *id);
                    }
                    if ui.add_enabled(i > 0, egui::Button::new("▲").small()).on_hover_text(tl!("Move up")).clicked() {
                        run = Some(("quickSteps.reorder", json!({"id": id, "index": i - 1})));
                    }
                    if ui.add_enabled(i < last, egui::Button::new("▼").small()).on_hover_text(tl!("Move down")).clicked() {
                        run = Some(("quickSteps.reorder", json!({"id": id, "index": i + 1})));
                    }
                    if icon_button(ui, "delete", tl!("Delete"), 14.0, true).clicked() {
                        run = Some(("quickSteps.delete", json!({"id": id})));
                    }
                });
                ui.add_space(2.0);
            }
            ui.add_space(4.0);
            if ui.add(egui::Button::new(tl!("New Quick Step…")).min_size(vec2(140.0, 24.0))).clicked() {
                open = Some(QuickStepForm::new());
            }
        });
        if let Some((command, params)) = run
            && let Err(e) = app.execute(command, params)
        {
            self.error = e;
        }
        error(ui, &self.error);
        if close_button(ui) {
            return Managed::Close;
        }
        match open {
            Some(form) => Managed::Open(form),
            None => Managed::Stay,
        }
    }
}

// ---- contact groups ----

/// A contact group, new or being edited: its name, its members, notes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupForm {
    pub editing: Option<Id>,
    pub name: String,
    pub notes: String,
    /// Members as `(name, email, contact)`.
    pub members: Vec<(String, String, Option<Id>)>,
    /// The members the group had when it was opened (their addresses).
    pub had: Vec<String>,
    pub adding: String,
    pub error: String,
}

impl GroupForm {
    fn edit(app: &MailApp, id: Id) -> Option<GroupForm> {
        let g = app.session.mailbox.contact_groups.iter().find(|g| g.id == id)?;
        let members: Vec<(String, String, Option<Id>)> =
            g.members.iter().zip(mailcraft_engine::cmd::groups::members(&app.session, g)).map(|(m, a)| (a.name, a.email, m.contact)).collect();
        let had = members.iter().map(|m| m.1.clone()).collect();
        Some(GroupForm { editing: Some(g.id), name: g.name.clone(), notes: g.notes.clone(), members, had, ..Default::default() })
    }

    fn add_typed(&mut self) -> Result<(), String> {
        let list = mailcraft_model::Address::parse_list(&self.adding);
        if let Some(bad) = list.iter().find(|a| !a.is_valid()) {
            return Err(crate::i18n::fmt(tl!("\"{text}\" isn't an e-mail address"), &[("text", &bad.email)]));
        }
        for a in list {
            if !self.members.iter().any(|m| m.1.eq_ignore_ascii_case(&a.email)) {
                self.members.push((a.name, a.email, None));
            }
        }
        self.adding.clear();
        Ok(())
    }

    /// The commands OK runs, in order.
    pub fn commands(&self) -> Vec<(&'static str, Value)> {
        let contacts: Vec<Id> = self.members.iter().filter_map(|m| m.2).collect();
        let typed = |members: &mut dyn Iterator<Item = &(String, String, Option<Id>)>| {
            members.map(|m| mailcraft_model::Address::new(m.0.clone(), m.1.clone())).collect::<Vec<_>>()
        };
        match self.editing {
            None => {
                let addresses = typed(&mut self.members.iter().filter(|m| m.2.is_none()));
                vec![(
                    "people.newGroup",
                    json!({"name": self.name, "notes": self.notes, "contacts": contacts, "addresses": mailcraft_model::format_list(&addresses)}),
                )]
            }
            Some(id) => {
                let mut out = vec![("people.editGroup", json!({"id": id, "name": self.name, "notes": self.notes}))];
                let gone: Vec<&String> = self.had.iter().filter(|e| !self.members.iter().any(|m| m.1.eq_ignore_ascii_case(e))).collect();
                if !gone.is_empty() {
                    out.push(("people.removeMembers", json!({"id": id, "emails": gone})));
                }
                let new: Vec<&(String, String, Option<Id>)> =
                    self.members.iter().filter(|m| !self.had.iter().any(|e| e.eq_ignore_ascii_case(&m.1))).collect();
                if !new.is_empty() {
                    let contacts: Vec<Id> = new.iter().filter_map(|m| m.2).collect();
                    let addresses = typed(&mut new.iter().copied().filter(|m| m.2.is_none()));
                    out.push(("people.addMembers", json!({"id": id, "contacts": contacts, "addresses": mailcraft_model::format_list(&addresses)})));
                }
                out
            }
        }
    }

    fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let people: Vec<(Id, String, String)> = {
            let mut list: Vec<(Id, String, String)> = app
                .session
                .mailbox
                .contacts
                .values()
                .filter_map(|c| c.address().filter(|a| a.is_valid()).map(|a| (c.id, c.name(), a.email)))
                .collect();
            list.sort_by_cached_key(|(_, name, _)| name.to_lowercase());
            list
        };
        body(ui, |ui| {
            let width = (ui.available_width() - LABEL_W - 8.0).max(80.0);
            row(ui, tl!("Name"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.name).id(field_id("group_name")).desired_width(width));
            });
            ui.add_space(4.0);
            ui.label(crate::i18n::fmt(tl!("Members ({n})"), &[("n", &self.members.len().to_string())]));
            let mut remove = None;
            egui::ScrollArea::vertical().max_height(200.0).id_salt(field_id("group_members")).show(ui, |ui| {
                if self.members.is_empty() {
                    ui.label(egui::RichText::new(tl!("No members yet")).color(t.text_dim));
                }
                for (i, (name, email, _)) in self.members.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let label = if name.trim().is_empty() { email.clone() } else { format!("{name} <{email}>") };
                        ui.label(shown(&label, 80));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icon_button(ui, "delete", tl!("Remove from the group"), 13.0, true).clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                }
            });
            if let Some(i) = remove {
                self.members.remove(i);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.adding)
                        .id(field_id("group_add"))
                        .desired_width((ui.available_width() - 70.0).max(80.0))
                        .hint_text(tl!("Add addresses, separated by ;")),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button(tl!("Add")).clicked() || enter) && !self.adding.trim().is_empty() {
                    match self.add_typed() {
                        Ok(()) => self.error.clear(),
                        Err(e) => self.error = e,
                    }
                }
            });
            if !people.is_empty() {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt(field_id("group_contacts")).width(260.0).selected_text(tl!("Add from Contacts…")).show_ui(
                        ui,
                        |ui| {
                            for (id, name, email) in &people {
                                let there = self.members.iter().any(|m| m.1.eq_ignore_ascii_case(email));
                                if ui.add_enabled(!there, egui::Button::selectable(false, shown(&format!("{name} <{email}>"), 70))).clicked() {
                                    self.members.push((name.clone(), email.clone(), Some(*id)));
                                }
                            }
                        },
                    );
                });
            }
            ui.add_space(4.0);
            row(ui, tl!("Notes"), |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.notes).id(field_id("group_notes")).desired_width(width).desired_rows(2));
            });
        });
        error(ui, &self.error);
        let f = footer(ui, tl!("Save"), !self.name.trim().is_empty(), self.editing.is_some());
        if f.cancel {
            return true;
        }
        if f.delete
            && let Some(id) = self.editing
        {
            match app.execute("people.deleteGroup", json!({"id": id})) {
                Ok(_) => return true,
                Err(e) => self.error = e,
            }
        }
        if f.ok {
            for (command, params) in self.commands() {
                if let Err(e) = app.execute(command, params) {
                    self.error = e;
                    return false;
                }
            }
            return true;
        }
        false
    }
}

// ---- Delay Delivery ----

/// Delay Delivery: the day and time before which the message is not to go.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DelayForm {
    pub date: String,
    pub time: String,
    pub error: String,
}

impl DelayForm {
    fn new(app: &MailApp) -> DelayForm {
        let s = &app.session;
        let at = s.compose.as_ref().and_then(|c| c.deliver_after).map(|ts| s.local(ts)).unwrap_or_else(|| {
            // Tomorrow at eight, as a first suggestion.
            let now = s.local_now();
            mailcraft_time::DateTime::new(now.date.add_days(1), 8, 0, 0).unwrap_or(now)
        });
        DelayForm { date: at.date.iso(), time: format!("{:02}:{:02}", at.hour, at.minute), error: String::new() }
    }

    /// `2026-10-15T08:30`, when the fields make a time.
    pub fn stamp(&self) -> Option<String> {
        let day = mailcraft_time::Date::parse_iso(self.date.trim())?;
        let (h, m) = crate::forms::kit::parse_clock(self.time.trim())?;
        Some(format!("{}T{h:02}:{m:02}", day.iso()))
    }

    fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        body(ui, |ui| {
            ui.label(tl!("Do not deliver before:"));
            row(ui, tl!("Date"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.date).id(field_id("delay_date")).desired_width(120.0).hint_text("2026-10-15"));
            });
            row(ui, tl!("Time"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.time).id(field_id("delay_time")).desired_width(120.0).hint_text("08:30"));
            });
            ui.label(egui::RichText::new(tl!("The message waits in the Outbox until then.")).small());
            if app.session.compose.as_ref().is_some_and(|c| c.deliver_after.is_some()) && ui.button(tl!("Send at once instead")).clicked() {
                match app.execute("compose.delay", json!({"at": null})) {
                    Ok(_) => self.error.clear(),
                    Err(e) => self.error = e,
                }
            }
        });
        error(ui, &self.error);
        let stamp = self.stamp();
        let f = footer(ui, tl!("OK"), stamp.is_some(), false);
        if f.cancel {
            return true;
        }
        if f.ok
            && let Some(at) = stamp
        {
            match app.execute("compose.delay", json!({"at": at})) {
                Ok(_) => return true,
                Err(e) => self.error = e,
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forms_make_the_commands_the_engine_reads() {
        let mut f = SearchFolderForm { template: Template::From, people: "ada@example.org".into(), ..Default::default() };
        assert_eq!(f.command(), ("folder.newSearchFolder", json!({"template": "from", "people": "ada@example.org", "account": null})));
        f.template = Template::Large;
        f.size = " 250 ".into();
        assert_eq!(f.command().1["size"], 250);
        let q = QuickStepForm {
            name: "Done".into(),
            rows: vec![
                StepRow { kind: StepKind::MoveTo, folder: 7, ..Default::default() },
                StepRow { kind: StepKind::MarkRead, ..Default::default() },
            ],
            ..Default::default()
        };
        assert_eq!(q.command(), ("quickSteps.new", json!({"name": "Done", "actions": [{"type": "moveTo", "value": 7}, {"type": "markRead"}]})));
        let g = GroupForm {
            editing: Some(4),
            name: "Team".into(),
            members: vec![("Ada".into(), "ada@example.org".into(), Some(9)), ("".into(), "new@example.org".into(), None)],
            had: vec!["ada@example.org".into(), "gone@example.org".into()],
            ..Default::default()
        };
        let commands = g.commands();
        assert_eq!(commands.iter().map(|c| c.0).collect::<Vec<_>>(), ["people.editGroup", "people.removeMembers", "people.addMembers"]);
        assert_eq!(commands[1].1["emails"], json!(["gone@example.org"]));
        assert_eq!(commands[2].1["addresses"], "new@example.org");
        let d = DelayForm { date: "2026-10-15".into(), time: "8:30".into(), error: String::new() };
        assert_eq!(d.stamp().as_deref(), Some("2026-10-15T08:30"));
        assert!(DelayForm { date: "soon".into(), ..d }.stamp().is_none());
    }
}
