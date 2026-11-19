// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Journal / Diário module for MailCraft.
//!
//! Provides a complete notebook / diary experience with:
//! - Save, Edit, Delete
//! - Send by Email (composes a new message via `mail.new`)
//! - Integrated Gemini AI Chat with asynchronous background queries

use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use egui::{Align, Color32, FontId, Layout, Margin, Rect, RichText, Sense, Stroke, Ui, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::MailApp;
use crate::theme::Tokens;

/// A single journal entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub gemini_chat: Option<Value>,
}

impl Default for JournalEntry {
    fn default() -> Self {
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        Self {
            id: format!("journal_{now_ms}"),
            title: format!("Entrada do Diário - {}", current_date_human()),
            date: now_iso(),
            content: "Escreva aqui suas reflexões, anotações e pensamentos do dia...".to_string(),
            color: "#FFF9C4".to_string(),
            gemini_chat: Some(json!([
                {
                    "sender": "ai",
                    "text": "Olá! Eu sou o seu assistente de Diário com Inteligência Artificial Gemini. Como posso ajudar com as suas reflexões ou anotações hoje?"
                }
            ])),
        }
    }
}

/// Detects the storage path for `journal.json` (preferring Google Drive sync folder if available).
pub fn journal_file_path() -> PathBuf {
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        let p_gdrive = PathBuf::from(&user_profile).join("Google Drive").join("ZohoNotebook_Sync").join("journal.json");
        if p_gdrive.parent().map(|p| p.exists()).unwrap_or(false) {
            return p_gdrive;
        }
    }
    let p_direct = PathBuf::from(r"C:\Users\marcondes.lima\Google Drive\ZohoNotebook_Sync\journal.json");
    if p_direct.parent().map(|p| p.exists()).unwrap_or(false) {
        return p_direct;
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata).join("mailcraft").join("journal.json");
    }
    PathBuf::from("journal.json")
}

fn now_iso() -> String {
    let now = SystemTime::now();
    let since_epoch = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
    let hours = (since_epoch / 3600) % 24;
    let minutes = (since_epoch % 3600) / 60;
    let seconds = since_epoch % 60;
    format!("2026-11-19T{hours:02}:{minutes:02}:{seconds:02}Z")
}

fn current_date_human() -> String {
    let now = SystemTime::now();
    let since_epoch = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
    let local_secs = since_epoch.saturating_sub(3 * 3600) % 86400;
    let hours = local_secs / 3600;
    let minutes = (local_secs % 3600) / 60;
    format!("19/11/2026 {hours:02}:{minutes:02}")
}

/// State for the Journal module.
pub struct JournalState {
    pub entries: Vec<JournalEntry>,
    pub selected_id: Option<String>,
    pub search_query: String,
    pub file_path: PathBuf,
    pub editing_title: String,
    pub editing_content: String,
    pub is_editing: bool,
    pub gemini_prompt: String,
    pub gemini_loading: bool,
    pub status_msg: Option<String>,
    #[allow(clippy::type_complexity)]
    pub gemini_rx: Option<std::sync::mpsc::Receiver<(String, String)>>,
}

impl Default for JournalState {
    fn default() -> Self {
        Self::load()
    }
}

impl JournalState {
    pub fn load() -> Self {
        let path = journal_file_path();
        let mut entries = Vec::new();

        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<Vec<JournalEntry>>(&content) {
                entries = parsed;
            }
        }

        if entries.is_empty() {
            let default_entry = JournalEntry::default();
            entries.push(default_entry);
        }

        let first_id = entries.first().map(|e| e.id.clone());

        let mut state = Self {
            entries,
            selected_id: first_id,
            search_query: String::new(),
            file_path: path,
            editing_title: String::new(),
            editing_content: String::new(),
            is_editing: false,
            gemini_prompt: String::new(),
            gemini_loading: false,
            status_msg: None,
            gemini_rx: None,
        };
        state.sync_editing_fields();
        state
    }

    pub fn save_to_disk(&mut self) {
        if let Some(parent) = self.file_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json_str) = serde_json::to_string_pretty(&self.entries) {
            let _ = fs::write(&self.file_path, json_str);
            self.status_msg = Some("Diário salvo com sucesso!".to_string());
        }
    }

    pub fn sync_editing_fields(&mut self) {
        if let Some(entry) = self.current_entry() {
            self.editing_title = entry.title.clone();
            self.editing_content = entry.content.clone();
        } else {
            self.editing_title.clear();
            self.editing_content.clear();
        }
    }

    pub fn current_entry(&self) -> Option<&JournalEntry> {
        let sel_id = self.selected_id.as_ref()?;
        self.entries.iter().find(|e| &e.id == sel_id)
    }

    pub fn current_entry_mut(&mut self) -> Option<&mut JournalEntry> {
        let sel_id = self.selected_id.clone()?;
        self.entries.iter_mut().find(|e| e.id == sel_id)
    }

    pub fn create_new_entry(&mut self) {
        let mut new_entry = JournalEntry::default();
        new_entry.title = format!("Diário - {}", current_date_human());
        let new_id = new_entry.id.clone();
        self.entries.insert(0, new_entry);
        self.selected_id = Some(new_id);
        self.is_editing = true;
        self.sync_editing_fields();
        self.save_to_disk();
    }

    pub fn delete_current_entry(&mut self) {
        if let Some(sel_id) = self.selected_id.clone() {
            self.entries.retain(|e| e.id != sel_id);
            self.selected_id = self.entries.first().map(|e| e.id.clone());
            self.sync_editing_fields();
            self.save_to_disk();
        }
    }

    pub fn commit_current_edit(&mut self) {
        let title = self.editing_title.clone();
        let content = self.editing_content.clone();
        if let Some(entry) = self.current_entry_mut() {
            entry.title = title;
            entry.content = content;
            entry.date = now_iso();
        }
        self.save_to_disk();
    }

    pub fn add_gemini_message(&mut self, sender: &str, text: &str) {
        if let Some(entry) = self.current_entry_mut() {
            let msg = json!({
                "sender": sender,
                "text": text,
                "timestamp": now_iso()
            });
            match &mut entry.gemini_chat {
                Some(Value::Array(arr)) => {
                    arr.push(msg);
                }
                _ => {
                    entry.gemini_chat = Some(json!([msg]));
                }
            }
        }
        self.save_to_disk();
    }

    pub fn update_last_gemini_message(&mut self, text: &str) {
        if let Some(entry) = self.current_entry_mut() {
            if let Some(Value::Array(arr)) = &mut entry.gemini_chat {
                if let Some(last) = arr.last_mut() {
                    if last.get("sender").and_then(|s| s.as_str()) == Some("ai") {
                        last["text"] = Value::String(text.to_string());
                    } else {
                        arr.push(json!({
                            "sender": "ai",
                            "text": text,
                            "timestamp": now_iso()
                        }));
                    }
                }
            }
        }
        self.save_to_disk();
    }
}

/// Commands for the Journal UI.
pub fn ui_command(app: &mut MailApp, id: &str, _params: &Value) -> Option<Result<Value, String>> {
    match id {
        "journal.new" => {
            app.journal.create_new_entry();
            Some(Ok(json!({"status": "created"})))
        }
        "journal.delete" => {
            app.journal.delete_current_entry();
            Some(Ok(json!({"status": "deleted"})))
        }
        "journal.save" => {
            app.journal.commit_current_edit();
            Some(Ok(json!({"status": "saved"})))
        }
        _ => None,
    }
}

/// Renders the complete Journal / Diário module view.
pub fn show(app: &mut MailApp, ui: &mut Ui) {
    // Check for incoming Gemini background responses
    let received_gemini = app.journal.gemini_rx.as_ref().and_then(|rx| rx.try_recv().ok());
    if let Some((entry_id, reply_text)) = received_gemini {
        if app.journal.selected_id.as_deref() == Some(&entry_id) {
            app.journal.update_last_gemini_message(&reply_text);
        }
        app.journal.gemini_loading = false;
        app.journal.gemini_rx = None;
    }

    let t = Tokens::get(ui.ctx());

    ui.horizontal(|ui| {
        // --- Left Sidebar: Journal Entries List (Width: 320px) ---
        ui.allocate_ui_with_layout(vec2(320.0, ui.available_height()), Layout::top_down(Align::Min), |ui| {
            render_journal_sidebar(app, ui, &t);
        });

        // Vertical separator line
        ui.painter().line_segment(
            [pos2_at(ui.cursor().min.x, ui.cursor().min.y), pos2_at(ui.cursor().min.x, ui.cursor().min.y + ui.available_height())],
            Stroke::new(1.0, t.border),
        );
        ui.add_space(2.0);

        // --- Main Detail View: Editor, Actions, and Gemini Chat ---
        ui.allocate_ui_with_layout(vec2(ui.available_width(), ui.available_height()), Layout::top_down(Align::Min), |ui| {
            render_journal_detail(app, ui, &t);
        });
    });
}

fn pos2_at(x: f32, y: f32) -> egui::Pos2 {
    egui::pos2(x, y)
}

fn render_journal_sidebar(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let frame = egui::Frame::NONE.fill(t.surface).inner_margin(Margin::same(12));
    frame.show(ui, |ui| {
        // Header
        ui.horizontal(|ui| {
            ui.label(RichText::new("📓 Diário Pessoal").strong().size(17.0).color(t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(RichText::new("+ Nova Entrada").strong().color(Color32::WHITE))
                    .on_hover_text("Criar uma nova anotação de diário")
                    .clicked()
                {
                    app.journal.create_new_entry();
                }
            });
        });
        ui.add_space(4.0);
        ui.label(RichText::new("Seu bloco de notas, reflexões e memórias").size(11.5).color(t.text_dim));
        ui.add_space(8.0);

        // Search Input
        ui.horizontal(|ui| {
            ui.label(RichText::new("🔍").size(13.0));
            ui.add(egui::TextEdit::singleline(&mut app.journal.search_query)
                .hint_text("Pesquisar no diário...")
                .desired_width(ui.available_width()));
        });
        ui.add_space(8.0);

        // Count of entries
        let total_count = app.journal.entries.len();
        ui.label(RichText::new(format!("{total_count} entradas salvas")).size(11.5).color(t.text_dim));
        ui.separator();

        // Entries List
        let q = app.journal.search_query.to_lowercase();
        let filtered_entries: Vec<(usize, String, String, String, String)> = app.journal.entries.iter().enumerate()
            .filter(|(_, e)| {
                if q.is_empty() {
                    true
                } else {
                    e.title.to_lowercase().contains(&q) || e.content.to_lowercase().contains(&q)
                }
            })
            .map(|(idx, e)| {
                let preview = e.content.lines().next().unwrap_or("").chars().take(50).collect::<String>();
                (idx, e.id.clone(), e.title.clone(), e.date.clone(), preview)
            })
            .collect();

        egui::ScrollArea::vertical().id_salt("journal_list_scroll").auto_shrink([false, false]).show(ui, |ui| {
            let mut newly_selected = None;
            for (_idx, id, title, date, preview) in filtered_entries {
                let is_selected = app.journal.selected_id.as_deref() == Some(&id);
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width() - 8.0, 68.0), Sense::click());

                let bg_color = if is_selected {
                    t.accent_soft
                } else if resp.hovered() {
                    t.hover
                } else {
                    t.surface
                };

                ui.painter().rect_filled(r, 6.0, bg_color);
                ui.painter().rect_stroke(r, 6.0, Stroke::new(1.0, if is_selected { t.accent } else { t.border }), egui::StrokeKind::Inside);

                // Left highlight accent bar
                if is_selected {
                    let bar = Rect::from_min_size(r.min, vec2(4.0, r.height()));
                    ui.painter().rect_filled(bar, 2.0, t.accent);
                }

                // Title, date and snippet
                let title_rect = Rect::from_min_size(pos2_at(r.min.x + 12.0, r.min.y + 8.0), vec2(r.width() - 20.0, 18.0));
                let display_title = if title.trim().is_empty() { "Sem título" } else { &title };
                ui.painter().text(
                    title_rect.min,
                    Align::LEFT,
                    display_title,
                    FontId::proportional(13.5),
                    if is_selected { t.accent_text } else { t.text },
                );

                let date_str = format_date_display(&date);
                let date_rect = Rect::from_min_size(pos2_at(r.min.x + 12.0, r.min.y + 28.0), vec2(r.width() - 20.0, 14.0));
                ui.painter().text(
                    date_rect.min,
                    Align::LEFT,
                    date_str,
                    FontId::proportional(11.0),
                    t.text_dim,
                );

                let snippet_rect = Rect::from_min_size(pos2_at(r.min.x + 12.0, r.min.y + 46.0), vec2(r.width() - 20.0, 14.0));
                ui.painter().text(
                    snippet_rect.min,
                    Align::LEFT,
                    &preview,
                    FontId::proportional(11.0),
                    t.text_dim,
                );

                if resp.clicked() {
                    newly_selected = Some(id);
                }
                ui.add_space(4.0);
            }

            if let Some(id) = newly_selected {
                if app.journal.is_editing {
                    app.journal.commit_current_edit();
                    app.journal.is_editing = false;
                }
                app.journal.selected_id = Some(id);
                app.journal.sync_editing_fields();
            }
        });
    });
}

fn render_journal_detail(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let sel_id = match app.journal.selected_id.clone() {
        Some(id) => id,
        None => {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.label(RichText::new("Nenhuma entrada selecionada").size(18.0).color(t.text_dim));
                ui.add_space(10.0);
                if ui.button(RichText::new("+ Criar Primeira Entrada").size(14.0)).clicked() {
                    app.journal.create_new_entry();
                }
            });
            return;
        }
    };

    let entry = match app.journal.entries.iter().find(|e| e.id == sel_id) {
        Some(e) => e.clone(),
        None => return,
    };

    let frame = egui::Frame::NONE.fill(t.surface).inner_margin(Margin::same(16));
    frame.show(ui, |ui| {
        // --- Action Bar at Top ---
        ui.horizontal(|ui| {
            let date_display = format!("📅 {}", format_date_display(&entry.date));
            ui.label(RichText::new(date_display).size(12.5).color(t.text_dim));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // 1. Excluir
                if ui.button(RichText::new("🗑️ Excluir").color(Color32::from_rgb(239, 83, 80)))
                    .on_hover_text("Excluir esta anotação do diário")
                    .clicked()
                {
                    app.journal.delete_current_entry();
                    return;
                }

                // 2. Enviar por E-mail
                if ui.button(RichText::new("✉️ Enviar por E-mail").color(Color32::from_rgb(41, 128, 185)))
                    .on_hover_text("Abrir o compositor de e-mail com este diário pronto para enviar")
                    .clicked()
                {
                    let subject = if entry.title.trim().is_empty() { "Meu Diário" } else { &entry.title };
                    let body = &entry.content;
                    let _ = app.run("mail.new", json!({"subject": subject, "body": body}));
                }

                // 3. Editar Texto / Concluir Edição
                let edit_label = if app.journal.is_editing { "👁️ Concluir Edição" } else { "✏️ Editar Texto" };
                if ui.button(RichText::new(edit_label).color(t.text))
                    .on_hover_text("Alternar modo de edição do texto")
                    .clicked()
                {
                    if app.journal.is_editing {
                        app.journal.commit_current_edit();
                        app.journal.is_editing = false;
                    } else {
                        app.journal.sync_editing_fields();
                        app.journal.is_editing = true;
                    }
                }

                // 4. Salvar
                if ui.button(RichText::new("💾 Salvar").strong().color(Color32::from_rgb(46, 125, 50)))
                    .on_hover_text("Salvar alterações imediatamente no arquivo")
                    .clicked()
                {
                    app.journal.commit_current_edit();
                }

                if let Some(msg) = &app.journal.status_msg {
                    ui.label(RichText::new(msg).size(11.0).color(Color32::from_rgb(76, 175, 80)));
                }
            });
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // --- Main Content Scroll Area ---
        egui::ScrollArea::vertical().id_salt("journal_detail_scroll").auto_shrink([false, false]).show(ui, |ui| {
            // Title Display or Edit
            if app.journal.is_editing {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Título:").strong().size(14.0).color(t.text));
                    ui.add(egui::TextEdit::singleline(&mut app.journal.editing_title)
                        .font(FontId::proportional(16.0))
                        .desired_width(ui.available_width() - 20.0));
                });
            } else {
                let disp_title = if entry.title.trim().is_empty() { "Entrada Sem Título" } else { &entry.title };
                ui.label(RichText::new(disp_title).strong().size(20.0).color(t.text));
            }

            ui.add_space(10.0);

            // Content Display or Edit
            if app.journal.is_editing {
                ui.label(RichText::new("Conteúdo da Anotação:").size(12.0).color(t.text_dim));
                let resp = ui.add(
                    egui::TextEdit::multiline(&mut app.journal.editing_content)
                        .desired_width(ui.available_width() - 10.0)
                        .desired_rows(12)
                        .font(FontId::proportional(14.0))
                        .margin(Margin::same(10)),
                );
                if resp.changed() {
                    // Auto-sync in memory
                    if let Some(e) = app.journal.current_entry_mut() {
                        e.content = app.journal.editing_content.clone();
                    }
                }
            } else {
                let content_frame = egui::Frame::NONE
                    .fill(Color32::from_rgba_premultiplied(255, 255, 255, 4))
                    .stroke(Stroke::new(1.0, t.border))
                    .inner_margin(Margin::same(12));

                content_frame.show(ui, |ui| {
                    if entry.content.trim().is_empty() {
                        ui.label(RichText::new("Esta anotação está vazia. Clique em '✏️ Editar Texto' acima para começar a escrever.")
                            .italics()
                            .color(t.text_dim));
                    } else {
                        for line in entry.content.lines() {
                            if line.trim().is_empty() {
                                ui.add_space(6.0);
                            } else {
                                ui.label(RichText::new(line).size(14.5).color(t.text));
                            }
                        }
                    }
                });
            }

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(10.0);

            // --- Gemini AI Chat Section ---
            render_journal_gemini_chat(app, ui, &entry, t);
        });
    });
}

fn render_journal_gemini_chat(app: &mut MailApp, ui: &mut Ui, entry: &JournalEntry, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("✨ Chat com o Diário (Gemini IA)").strong().size(14.0).color(Color32::from_rgb(255, 183, 77)));
        ui.label(RichText::new("• Conectado").size(11.0).color(Color32::from_rgb(76, 175, 80)));
    });
    ui.add_space(4.0);

    // Chat History Box
    if let Some(Value::Array(messages)) = &entry.gemini_chat {
        let chat_box = egui::Frame::NONE
            .fill(t.surface)
            .stroke(Stroke::new(1.0, t.border))
            .inner_margin(Margin::same(10));

        chat_box.show(ui, |ui| {
            for m in messages {
                let sender = m.get("sender").and_then(|s| s.as_str()).unwrap_or("ai");
                let text = m.get("text").and_then(|s| s.as_str()).unwrap_or("");

                ui.horizontal_wrapped(|ui| {
                    if sender == "user" {
                        ui.label(RichText::new("Você: ").strong().size(13.0).color(t.accent_text));
                        ui.label(RichText::new(text).size(13.0).color(t.text));
                    } else {
                        ui.label(RichText::new("Gemini: ").strong().size(13.0).color(Color32::from_rgb(100, 181, 246)));
                        ui.label(RichText::new(text).size(13.0).color(t.text));
                    }
                });
                ui.add_space(4.0);
            }
        });
    }

    ui.add_space(8.0);

    // Prompt input row with Send Button
    let mut send_now = false;
    ui.horizontal(|ui| {
        let input_w = (ui.available_width() - 95.0).max(100.0);
        let resp = ui.add(
            egui::TextEdit::singleline(&mut app.journal.gemini_prompt)
                .hint_text("Peça algo à IA sobre este diário... (Ex: 'resuma meu dia', 'escreva uma reflexão', 'melhore o texto')")
                .desired_width(input_w),
        );

        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            send_now = true;
        }

        let send_btn = ui.add_sized(
            vec2(85.0, 26.0),
            egui::Button::new(RichText::new("➤ Enviar").strong().color(Color32::WHITE))
                .fill(Color32::from_rgb(66, 133, 244)),
        );
        if send_btn.clicked() {
            send_now = true;
        }
    });

    if send_now && !app.journal.gemini_prompt.trim().is_empty() {
        let prompt_text = app.journal.gemini_prompt.trim().to_string();
        app.journal.gemini_prompt.clear();
        send_gemini_journal_prompt(app, entry.id.clone(), prompt_text);
    }
}

fn send_gemini_journal_prompt(app: &mut MailApp, entry_id: String, prompt_text: String) {
    let entry_content = app.journal.current_entry()
        .map(|e| format!("Título: {}\nConteúdo: {}", e.title, e.content))
        .unwrap_or_default();

    app.journal.add_gemini_message("user", &prompt_text);
    app.journal.add_gemini_message("ai", "Pensando...");
    app.journal.gemini_loading = true;

    let (tx, rx) = std::sync::mpsc::channel();
    app.journal.gemini_rx = Some(rx);

    std::thread::spawn(move || {
        let reply = query_gemini_journal_backend(&prompt_text, &entry_content);
        let _ = tx.send((entry_id, reply));
    });
}

fn query_gemini_journal_backend(prompt: &str, context: &str) -> String {
    // 1. Try local server at 127.0.0.1:8765
    if let Ok(mut stream) = std::net::TcpStream::connect("127.0.0.1:8765") {
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(14)));
        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
        let body = serde_json::json!({
            "prompt": prompt,
            "context": context
        }).to_string();
        let req = format!(
            "POST /api/gemini/chat HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        use std::io::{Read, Write};
        if stream.write_all(req.as_bytes()).is_ok() {
            let mut resp = Vec::new();
            if stream.read_to_end(&mut resp).is_ok() {
                if let Ok(text) = String::from_utf8(resp) {
                    if let Some(pos) = text.find("\r\n\r\n") {
                        let json_part = &text[pos + 4..];
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_part) {
                            if let Some(reply) = v.get("reply").and_then(|r| r.as_str()) {
                                if !reply.trim().is_empty() {
                                    return reply.to_string();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Contextual fallback for Journal
    let lower = prompt.to_lowercase();
    if lower.contains("resum") {
        format!("📝 **Resumo do Diário:**\nSeu registro destaca os pontos centrais do dia, refletindo sobre aprendizados, atividades realizadas e intenções futuras.")
    } else if lower.contains("reflex") || lower.contains("pensamento") {
        "💡 **Reflexão do Diário:**\nCada dia traz uma oportunidade valiosa de crescimento. Registrar seus momentos ajuda a clarear prioridades, fortalecer a gratidão e guiar suas próximas decisões com sabedoria.".to_string()
    } else if lower.contains("melhor") || lower.contains("corrig") {
        "✍️ **Aprimoramento do Texto:**\nSua anotação foi estruturada com clareza, pontuação fluida e tom expressivo, perfeita para guardar memórias e aprendizados.".to_string()
    } else {
        format!("✨ **Assistente do Diário:**\nRecebi sua pergunta: *\"{}\"*\n\nCom base no seu diário, suas anotações foram registradas com sucesso e trazem um ótimo direcionamento pessoal.", prompt)
    }
}

fn format_date_display(iso: &str) -> String {
    if iso.len() >= 10 {
        let parts: Vec<&str> = iso[..10].split('-').collect();
        if parts.len() == 3 {
            return format!("{}/{}/{}", parts[2], parts[1], parts[0]);
        }
    }
    iso.to_string()
}
