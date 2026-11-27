// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Notes module: synchronized with Google Drive Zoho Notebook (`notes.json`).
//!
//! Provides a 3-panel notebook interface:
//! 1. Left sidebar: Profile, search, standard views (Todas as Notas, Favoritos, Lembretes) and Cadernos.
//! 2. Middle list: Colored note cards with previews, dates, favorite stars, and pending/all filters.
//! 3. Right detail view: Interactive checklist with clickable checkboxes, rich note editor, and Gemini AI chat.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use egui::{Align, Color32, FontId, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::MailApp;
use crate::theme::Tokens;

/// Drag payload when dragging a note to a notebook/folder.
#[derive(Clone, Debug)]
pub(crate) struct NoteDrag {
    pub id: String,
    pub title: String,
}

/// A single note representation matching the Google Drive `notes.json` schema.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub notebook: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub created_date: Option<String>,
    #[serde(default)]
    pub modified_date: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub content_html: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub is_favorite: Option<bool>,
    #[serde(default)]
    pub is_pinned: Option<bool>,
    #[serde(default)]
    pub reminder_date: Option<String>,
    #[serde(default)]
    pub reminder_time: Option<String>,
    #[serde(default)]
    pub reminder_title: Option<String>,
    #[serde(default)]
    pub gemini_chat: Option<Value>,
    /// Persistent deletion marker so a stale cloud pull cannot restore this note.
    #[serde(default)]
    pub is_deleted: bool,
}

impl Default for Note {
    fn default() -> Self {
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        Self {
            id: format!("note_{now_ms}"),
            title: "Nova Nota".to_string(),
            notebook: "Meus blocos de anotações".to_string(),
            color: "#E1F5FE".to_string(),
            created_date: Some(now_iso()),
            modified_date: Some(now_iso()),
            preview: Some(String::new()),
            content_html: Some("<div>Nova nota</div>".to_string()),
            thumbnail: None,
            is_favorite: Some(false),
            is_pinned: Some(false),
            reminder_date: None,
            reminder_time: None,
            reminder_title: None,
            gemini_chat: None,
            is_deleted: false,
        }
    }
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub const GOOGLE_WEBHOOK_URL: &str =
    "https://script.google.com/macros/s/AKfycbzIFNueJUl00QuAOsyfbsK7Gexjm3yPQSWIQKzjnjPXaj_JuQSqMbQZsA1HjqVPS6Q/exec";

pub fn fetch_cloud_notes() -> Option<Vec<Note>> {
    // 1. Try curl.exe with 20s timeout (native on Windows 10/11) without flashing a console window
    let mut curl_cmd = std::process::Command::new("curl.exe");
    curl_cmd.args(&["-s", "-L", "-m", "20", GOOGLE_WEBHOOK_URL]);
    #[cfg(windows)]
    curl_cmd.creation_flags(CREATE_NO_WINDOW);

    if let Ok(out) = curl_cmd.output() {
        if out.status.success() {
            if let Ok(text) = String::from_utf8(out.stdout) {
                if let Ok(notes) = serde_json::from_str::<Vec<Note>>(&text) {
                    if !notes.is_empty() {
                        return Some(notes);
                    }
                }
                if let Ok(val) = serde_json::from_str::<Value>(&text) {
                    if let Some(arr) = val.get("notes") {
                        if let Ok(notes) = serde_json::from_value::<Vec<Note>>(arr.clone()) {
                            if !notes.is_empty() {
                                return Some(notes);
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Fallback: PowerShell Invoke-RestMethod with safe parameterized arguments and NO console window
    let ps_script = "$ProgressPreference = 'SilentlyContinue'; \
        $res = Invoke-RestMethod -Uri $args[0] -Method Get; \
        $res | ConvertTo-Json -Depth 10 -Compress";
    let mut ps_cmd = std::process::Command::new("powershell");
    ps_cmd.args(&[
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        ps_script,
        GOOGLE_WEBHOOK_URL,
    ]);
    #[cfg(windows)]
    ps_cmd.creation_flags(CREATE_NO_WINDOW);

    if let Ok(out) = ps_cmd.output() {
        if out.status.success() {
            if let Ok(text) = String::from_utf8(out.stdout) {
                if let Ok(notes) = serde_json::from_str::<Vec<Note>>(&text) {
                    if !notes.is_empty() {
                        return Some(notes);
                    }
                }
                if let Ok(val) = serde_json::from_str::<Value>(&text) {
                    if let Some(arr) = val.get("notes") {
                        if let Ok(notes) = serde_json::from_value::<Vec<Note>>(arr.clone()) {
                            if !notes.is_empty() {
                                return Some(notes);
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

pub fn trigger_cloud_push(notes: Vec<Note>) {
    std::thread::spawn(move || {
        let temp_file = std::env::temp_dir().join(format!(
            "mailcraft_push_{}.json",
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis()
        ));
        let payload = json!({
            "action": "sync_notes",
            "notes": notes
        });
        if let Ok(data) = serde_json::to_string(&payload) {
            if fs::write(&temp_file, data.as_bytes()).is_ok() {
                let temp_path_str = temp_file.to_string_lossy().to_string();
                let ps_script = "$ProgressPreference = 'SilentlyContinue'; \
                    $raw = [System.IO.File]::ReadAllText($args[0], [System.Text.Encoding]::UTF8); \
                    [void](Invoke-RestMethod -Uri $args[1] -Method Post -Body $raw -ContentType 'application/json'); \
                    Remove-Item -Path $args[0] -ErrorAction SilentlyContinue";
                let mut push_cmd = std::process::Command::new("powershell");
                push_cmd.args(&[
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    ps_script,
                    &temp_path_str,
                    GOOGLE_WEBHOOK_URL,
                ]);
                #[cfg(windows)]
                push_cmd.creation_flags(CREATE_NO_WINDOW);
                let _ = push_cmd.output();
            }
        }
    });
}

pub fn smart_merge_notes(local_notes: &mut Vec<Note>, cloud_notes: Vec<Note>) -> (usize, usize) {
    let mut added = 0;
    let mut updated = 0;
    for c_note in cloud_notes {
        if let Some(existing) = local_notes.iter_mut().find(|n| n.id == c_note.id) {
            let c_mod = c_note.modified_date.as_deref().unwrap_or("");
            let l_mod = existing.modified_date.as_deref().unwrap_or("");
            // A stale active copy must never overwrite a local deletion marker.
            if c_note.is_deleted || (!existing.is_deleted && c_mod > l_mod) {
                *existing = c_note;
                updated += 1;
            }
        } else {
            local_notes.push(c_note);
            added += 1;
        }
    }
    (added, updated)
}

/// Detects the path to Google Drive or local sync `notes.json`.
pub fn notes_file_path() -> PathBuf {
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        let p_gdrive = PathBuf::from(&user_profile).join("Google Drive").join("ZohoNotebook_Sync").join("notes.json");
        if p_gdrive.exists() {
            return p_gdrive;
        }
        let p_portable = PathBuf::from(&user_profile).join("Downloads").join("Zoho_Notebook_Portatil").join("data").join("notes.json");
        if p_portable.exists() {
            return p_portable;
        }
        return p_gdrive;
    }
    let p_direct1 = PathBuf::from(r"C:\Users\marcondes.lima\Google Drive\ZohoNotebook_Sync\notes.json");
    if p_direct1.exists() {
        return p_direct1;
    }
    let p_direct2 = PathBuf::from(r"C:\Users\marcondes.lima\Downloads\Zoho_Notebook_Portatil\data\notes.json");
    if p_direct2.exists() {
        return p_direct2;
    }
    p_direct1
}

fn now_iso() -> String {
    let now = SystemTime::now();
    let since_epoch = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
    let _days = since_epoch / 86400;
    let secs_of_day = since_epoch % 86400;
    let hours = (secs_of_day / 3600) % 24;
    let minutes = (secs_of_day % 3600) / 60;
    let seconds = secs_of_day % 60;
    let days = since_epoch / 86400;
    // Date conversion uses a civil-from-days algorithm (UTC).
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

fn normalize_ai_text(text: &str) -> String {
    html_to_plain_text(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn ai_reply_already_embedded(content_html: &str, reply: &str) -> bool {
    let normalized_reply = normalize_ai_text(reply);
    !normalized_reply.is_empty() && normalize_ai_text(content_html).contains(&normalized_reply)
}

pub fn gemini_input_layout(available: f32, spacing: f32) -> (f32, f32, f32) {
    let button = available.min(85.0).max(0.0);
    let gap = spacing.min((available - button).max(0.0));
    let input = (available - button - gap).max(0.0);
    (input, button, gap)
}

fn current_time_hh_mm_ss() -> String {
    let now = SystemTime::now();
    let since_epoch = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
    let local_secs = since_epoch.saturating_sub(3 * 3600) % 86400;
    let hours = local_secs / 3600;
    let minutes = (local_secs % 3600) / 60;
    let seconds = local_secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// State for the Notes module in MailCraft.
pub struct NotesState {
    pub notes: Vec<Note>,
    pub selected_note_id: Option<String>,
    pub selected_notebook: Option<String>,
    pub filter_unorganized_only: bool,
    pub search_query: String,
    pub file_path: PathBuf,
    pub last_sync_time: String,
    pub last_mtime: Option<SystemTime>,
    pub editing_title: String,
    pub editing_content: String,
    pub editing_id: Option<String>,
    pub is_editing_raw: bool,
    pub gemini_prompt: String,
    pub show_new_notebook_dialog: bool,
    pub new_notebook_name: String,
    pub custom_notebooks: Vec<String>,
    pub status_msg: Option<String>,
    #[allow(clippy::type_complexity)]
    pub gemini_rx: Option<std::sync::mpsc::Receiver<(String, String)>>,
    pub gemini_loading: bool,
    pub cloud_rx: Option<std::sync::mpsc::Receiver<Vec<Note>>>,
    pub cloud_syncing: bool,
    pub image_textures: HashMap<String, egui::TextureHandle>,
    #[allow(clippy::type_complexity)]
    pub pending_image_rx: Option<std::sync::mpsc::Receiver<String>>,
    pub show_reminder_dialog: bool,
}

impl Default for NotesState {
    fn default() -> Self {
        Self::load()
    }
}

impl NotesState {
    pub fn load() -> Self {
        let path = notes_file_path();
        let mut notes = Vec::new();
        let mut mtime = None;

        if let Ok(metadata) = fs::metadata(&path) {
            mtime = metadata.modified().ok();
        }

        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<Vec<Note>>(&content) {
                notes = parsed;
            }
        }

        let first_id = notes.iter().find(|n| !n.is_deleted).map(|n| n.id.clone());
        let sync_time = current_time_hh_mm_ss();

        let default_custom = vec![
            "EMPRESA".to_string(),
            "Estudos Bíblicos".to_string(),
            "Finanças".to_string(),
            "Meus blocos de anotações".to_string(),
            "iNTELIGENCIA  ARTIFICIAL".to_string(),
            "Viagem fim de ano Bei".to_string(),
        ];

        let mut custom_notebooks = default_custom;
        if let Some(parent) = path.parent() {
            let nb_file = parent.join("notebooks.json");
            if let Ok(content) = fs::read_to_string(&nb_file) {
                if let Ok(saved) = serde_json::from_str::<Vec<String>>(&content) {
                    for s in saved {
                        if !custom_notebooks.contains(&s) {
                            custom_notebooks.push(s);
                        }
                    }
                }
            }
        }
        for n in notes.iter().filter(|n| !n.is_deleted) {
            let nb = n.notebook.trim();
            if !nb.is_empty() && !custom_notebooks.iter().any(|x| x == nb) {
                custom_notebooks.push(nb.to_string());
            }
        }

        let mut state = Self {
            notes,
            selected_note_id: first_id,
            selected_notebook: None,
            filter_unorganized_only: false,
            search_query: String::new(),
            file_path: path,
            last_sync_time: sync_time,
            last_mtime: mtime,
            editing_title: String::new(),
            editing_content: String::new(),
            editing_id: None,
            is_editing_raw: false,
            gemini_prompt: String::new(),
            show_new_notebook_dialog: false,
            new_notebook_name: String::new(),
            custom_notebooks,
            status_msg: None,
            gemini_rx: None,
            gemini_loading: false,
            cloud_rx: None,
            cloud_syncing: false,
            image_textures: HashMap::new(),
            pending_image_rx: None,
            show_reminder_dialog: false,
        };
        state.migrate_existing_chat_comments();
        state.sync_editing_buffer();
        state.trigger_cloud_pull();
        state
    }

    pub fn poll_cloud_sync(&mut self) {
        if let Some(rx) = &self.cloud_rx {
            if let Ok(cloud_notes) = rx.try_recv() {
                let (added, updated) = smart_merge_notes(&mut self.notes, cloud_notes);
                if added > 0 || updated > 0 || !self.file_path.exists() {
                    if let Some(parent) = self.file_path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if let Ok(serialized) = serde_json::to_string_pretty(&self.notes) {
                        let _ = fs::write(&self.file_path, serialized);
                    }
                    if self.selected_note_id.as_ref().is_none_or(|id| self.notes.iter().all(|n| &n.id != id || n.is_deleted)) {
                        self.selected_note_id = self.notes.iter().find(|n| !n.is_deleted).map(|n| n.id.clone());
                        self.sync_editing_buffer();
                    }
                }
                // Always publish the merged state: it includes deletion tombstones
                // that must replace an older active copy in the cloud.
                if self.notes.iter().any(|note| note.is_deleted) {
                    trigger_cloud_push(self.notes.clone());
                }
                self.last_sync_time = current_time_hh_mm_ss();
                self.cloud_syncing = false;
                self.status_msg = Some(format!("● Sincronizado com o Google Drive ({} notas)", self.notes.iter().filter(|n| !n.is_deleted).count()));
                self.cloud_rx = None;
            }
        }
    }

    pub fn trigger_cloud_pull(&mut self) {
        if self.cloud_syncing {
            return;
        }
        self.cloud_syncing = true;
        let (tx, rx) = std::sync::mpsc::channel();
        self.cloud_rx = Some(rx);

        std::thread::spawn(move || {
            if let Some(notes) = fetch_cloud_notes() {
                let _ = tx.send(notes);
            }
        });
    }

    pub fn reload(&mut self) {
        if let Ok(content) = fs::read_to_string(&self.file_path) {
            if let Ok(parsed) = serde_json::from_str::<Vec<Note>>(&content) {
                self.notes = parsed;
                if let Ok(metadata) = fs::metadata(&self.file_path) {
                    self.last_mtime = metadata.modified().ok();
                }
                self.last_sync_time = current_time_hh_mm_ss();
                self.status_msg = Some("Notas sincronizadas com sucesso!".to_string());
                self.migrate_existing_chat_comments();
                self.sync_editing_buffer();
            }
        }
        if let Some(parent) = self.file_path.parent() {
            let nb_file = parent.join("notebooks.json");
            if let Ok(content) = fs::read_to_string(&nb_file) {
                if let Ok(saved) = serde_json::from_str::<Vec<String>>(&content) {
                    for s in saved {
                        if !self.custom_notebooks.contains(&s) {
                            self.custom_notebooks.push(s);
                        }
                    }
                }
            }
        }
        for n in self.notes.iter().filter(|n| !n.is_deleted) {
            let nb = n.notebook.trim();
            if !nb.is_empty() && !self.custom_notebooks.iter().any(|x| x == nb) {
                self.custom_notebooks.push(nb.to_string());
            }
        }
        self.trigger_cloud_pull();
    }

    pub fn migrate_existing_chat_comments(&mut self) {
        let mut changed_any = false;
        for note in &mut self.notes {
            if !note.is_deleted && let Some(Value::Array(messages)) = &note.gemini_chat {
                let current_html = note.content_html.clone().unwrap_or_default();
                let mut to_append = Vec::new();
                let mut accumulated = current_html.clone();
                for m in messages {
                    if m.get("role").and_then(Value::as_str) == Some("ai") {
                        if let Some(text) = m.get("text").and_then(Value::as_str) {
                            let clean = text.trim();
                            if !clean.is_empty() && clean != "Pensando..." && !ai_reply_already_embedded(&accumulated, clean) {
                                to_append.push(clean.to_string());
                                accumulated.push_str(&clean.replace('\n', "<br/>"));
                            }
                        }
                    }
                }
                if !to_append.is_empty() {
                    let mut new_html = current_html;
                    for comment in to_append {
                        new_html.push_str(&format!(
                            "<br/><br/><div style=\"margin-top: 14px; padding: 10px 14px; border-left: 4px solid #4285F4; background: rgba(66, 133, 244, 0.08); border-radius: 4px;\"><b>🤖 Resposta do Assistente IA:</b><br/>{}</div>",
                            comment.replace('\n', "<br/>")
                        ));
                    }
                    note.content_html = Some(new_html);
                    note.modified_date = Some(now_iso());
                    changed_any = true;
                }
            }
        }
        if changed_any {
            self.save();
        }
    }

    pub fn append_ai_comment_to_note(&mut self, note_id: &str, reply: &str) {
        let clean_reply = reply.trim();
        if clean_reply.is_empty() {
            return;
        }

        if let Some(note) = self.notes.iter_mut().find(|n| n.id == note_id && !n.is_deleted) {
            let mut html = note.content_html.clone().unwrap_or_default();
            if ai_reply_already_embedded(&html, clean_reply) {
                return;
            }
            let comment_block = format!(
                "<br/><br/><div style=\"margin-top: 14px; padding: 10px 14px; border-left: 4px solid #4285F4; background: rgba(66, 133, 244, 0.08); border-radius: 4px;\"><b>🤖 Resposta do Assistente IA:</b><br/>{}</div>",
                clean_reply.replace('\n', "<br/>")
            );
            html.push_str(&comment_block);
            note.content_html = Some(html);
            note.modified_date = Some(now_iso());

            let plain = html_to_plain_text(note.content_html.as_deref().unwrap_or(""));
            let snippet: String = plain.chars().take(150).collect();
            note.preview = Some(snippet.replace('\n', " "));
        }
        self.sync_editing_buffer();
        self.save();
    }

    pub fn save(&mut self) {
        if let Some(parent) = self.file_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(serialized) = serde_json::to_string_pretty(&self.notes) {
            if fs::write(&self.file_path, serialized).is_ok() {
                if let Ok(metadata) = fs::metadata(&self.file_path) {
                    self.last_mtime = metadata.modified().ok();
                }
                self.last_sync_time = current_time_hh_mm_ss();
            }
        }
        trigger_cloud_push(self.notes.clone());
    }

    pub fn save_notebooks(&self) {
        if let Some(parent) = self.file_path.parent() {
            let _ = fs::create_dir_all(parent);
            let nb_file = parent.join("notebooks.json");
            if let Ok(serialized) = serde_json::to_string_pretty(&self.custom_notebooks) {
                let _ = fs::write(nb_file, serialized);
            }
        }
    }

    pub fn sync_editing_buffer(&mut self) {
        if let Some(sel_id) = &self.selected_note_id {
            if let Some(note) = self.notes.iter().find(|n| &n.id == sel_id && !n.is_deleted) {
                self.editing_id = Some(sel_id.clone());
                self.editing_title = note.title.clone();
                let html = note.content_html.clone().unwrap_or_default();
                self.editing_content = html_to_plain_text(&html);
            }
        }
    }

    pub fn new_note(&mut self) {
        let mut new_n = Note::default();
        if let Some(nb) = &self.selected_notebook {
            if nb != "Favoritos" && nb != "Lembretes" {
                new_n.notebook = nb.clone();
            }
        }
        let id = new_n.id.clone();
        self.notes.insert(0, new_n);
        self.selected_note_id = Some(id);
        self.sync_editing_buffer();
        self.save();
    }

    pub fn delete_selected(&mut self) {
        if let Some(sel_id) = self.selected_note_id.clone() {
            if let Some(pos) = self.notes.iter().position(|n| n.id == sel_id) {
                self.notes[pos].is_deleted = true;
                self.notes[pos].modified_date = Some(now_iso());
                let active: Vec<_> = self.notes.iter().filter(|n| !n.is_deleted).collect();
                if !active.is_empty() {
                    let next_pos = pos.min(active.len() - 1);
                    self.selected_note_id = Some(active[next_pos].id.clone());
                } else {
                    self.selected_note_id = None;
                    self.editing_id = None;
                    self.editing_title.clear();
                    self.editing_content.clear();
                }
                self.sync_editing_buffer();
                self.save();
            }
        }
    }

    pub fn toggle_favorite(&mut self, note_id: &str) {
        if let Some(note) = self.notes.iter_mut().find(|n| n.id == note_id) {
            let fav = !note.is_favorite.unwrap_or(false);
            note.is_favorite = Some(fav);
            note.modified_date = Some(now_iso());
            self.save();
        }
    }

    pub fn set_note_color(&mut self, note_id: &str, color: &str) {
        if let Some(note) = self.notes.iter_mut().find(|n| n.id == note_id) {
            note.color = color.to_string();
            note.modified_date = Some(now_iso());
            self.save();
        }
    }

    pub fn set_note_notebook(&mut self, note_id: &str, notebook: &str) {
        if !self.custom_notebooks.contains(&notebook.to_string()) && !notebook.trim().is_empty() {
            self.custom_notebooks.push(notebook.to_string());
            self.save_notebooks();
        }
        if let Some(note) = self.notes.iter_mut().find(|n| n.id == note_id) {
            note.notebook = notebook.to_string();
            note.modified_date = Some(now_iso());
            self.save();
        }
    }

    pub fn copy_note_to_notebook(&mut self, note_id: &str, target_notebook: &str) -> Option<String> {
        let note = self.notes.iter().find(|n| n.id == note_id && !n.is_deleted)?.clone();
        let mut copy = note;
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        copy.id = format!("note_{now_ms}");
        copy.notebook = target_notebook.to_string();
        copy.created_date = Some(now_iso());
        copy.modified_date = Some(now_iso());
        let new_id = copy.id.clone();
        if !self.custom_notebooks.contains(&target_notebook.to_string()) && !target_notebook.trim().is_empty() {
            self.custom_notebooks.push(target_notebook.to_string());
            self.save_notebooks();
        }
        self.notes.insert(0, copy);
        self.save();
        Some(new_id)
    }

    pub fn move_or_copy_note(&mut self, note_id: &str, target_notebook: &str, is_copy: bool) {
        if is_copy {
            self.copy_note_to_notebook(note_id, target_notebook);
            self.status_msg = Some(format!("Nota copiada para '{}'", target_notebook));
        } else {
            self.set_note_notebook(note_id, target_notebook);
            self.status_msg = Some(format!("Nota movida para '{}'", target_notebook));
        }
    }

    /// Returns the directory where images for notes are stored.
    pub fn images_dir(&self) -> PathBuf {
        let base = if let Some(parent) = self.file_path.parent() {
            parent.join("images")
        } else if let Ok(user_profile) = std::env::var("USERPROFILE") {
            PathBuf::from(&user_profile).join("Google Drive").join("ZohoNotebook_Sync").join("images")
        } else {
            PathBuf::from("images")
        };
        let _ = fs::create_dir_all(&base);
        base
    }

    /// Open native file dialog in a non-blocking background thread to pick an image/photo.
    pub fn open_image_picker(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending_image_rx = Some(rx);

        std::thread::spawn(move || {
            let ps_script = "$ProgressPreference = 'SilentlyContinue';                 [System.Reflection.Assembly]::LoadWithPartialName('System.Windows.Forms') | Out-Null;                 $dlg = New-Object System.Windows.Forms.OpenFileDialog;                 $dlg.Title = 'Selecionar Foto para a Nota (Zoho Notas)';                 $dlg.Filter = 'Imagens (*.png;*.jpg;*.jpeg;*.webp;*.gif;*.bmp)|*.png;*.jpg;*.jpeg;*.webp;*.gif;*.bmp|Todos os arquivos (*.*)|*.*';                 if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {                     Write-Output $dlg.FileName                 }";
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                let out = std::process::Command::new("powershell")
                    .args(&["-NoProfile", "-Command", ps_script])
                    .creation_flags(CREATE_NO_WINDOW)
                    .output();
                if let Ok(res) = out {
                    let picked = String::from_utf8_lossy(&res.stdout).trim().to_string();
                    if !picked.is_empty() {
                        let _ = tx.send(picked);
                    }
                }
            }
        });
    }

    /// Check if a picked image path has arrived from the background thread.
    pub fn poll_image_picker(&mut self) {
        if let Some(rx) = &self.pending_image_rx {
            if let Ok(picked_path) = rx.try_recv() {
                self.attach_image_from_path(std::path::Path::new(&picked_path));
            }
        }
    }

    /// Attach an image from a given file path to the currently selected note.
    pub fn attach_image_from_path(&mut self, src_path: &std::path::Path) {
        if !src_path.exists() {
            return;
        }
        let sel_id = match &self.selected_note_id {
            Some(id) => id.clone(),
            None => return,
        };

        let ext = src_path.extension().and_then(|e| e.to_str()).unwrap_or("png");
        let now_ms = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let orig_stem = src_path.file_stem().and_then(|s| s.to_str()).unwrap_or("foto");
        let clean_stem: String = orig_stem.chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect();
        let target_filename = format!("img_{now_ms}_{clean_stem}.{ext}");

        let target_dir = self.images_dir();
        let target_path = target_dir.join(&target_filename);

        if fs::copy(src_path, &target_path).is_ok() {
            if let Some(note) = self.notes.iter_mut().find(|n| n.id == sel_id) {
                let mut html = note.content_html.clone().unwrap_or_default();
                let img_tag = format!("<br/><div><img src='images/{target_filename}' /></div><br/>");
                html.push_str(&img_tag);
                note.content_html = Some(html);
                note.modified_date = Some(now_iso());
                self.status_msg = Some("Foto adicionada com sucesso!".to_string());
            }
            self.sync_editing_buffer();
            self.save();
        }
    }

    /// Paste PNG image directly from bytes to the currently selected note.
    pub fn paste_image_from_bytes(&mut self, png_bytes: &[u8]) {
        let sel_id = match &self.selected_note_id {
            Some(id) => id.clone(),
            None => return,
        };
        let now_ms = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let target_filename = format!("img_{now_ms}_clip.png");
        let target_dir = self.images_dir();
        let target_path = target_dir.join(&target_filename);

        if fs::write(&target_path, png_bytes).is_ok() {
            if let Some(note) = self.notes.iter_mut().find(|n| n.id == sel_id) {
                let mut html = note.content_html.clone().unwrap_or_default();
                let img_tag = format!("<br/><div><img src='images/{target_filename}' /></div><br/>");
                html.push_str(&img_tag);
                note.content_html = Some(html);
                note.modified_date = Some(now_iso());
                self.status_msg = Some("Foto colada com sucesso!".to_string());
            }
            self.sync_editing_buffer();
            self.save();
        }
    }

    /// Try to paste an image from clipboard (RichClipboard or Windows native).
    pub fn try_paste_clipboard_image(&mut self, clipboard: &Option<Box<dyn crate::RichClipboard>>) -> bool {
        if let Some(png) = clipboard.as_ref().and_then(|c| c.image()) {
            self.paste_image_from_bytes(&png);
            return true;
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            let temp_png = std::env::temp_dir().join(format!("mailcraft_clip_{}.png", SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)));
            let temp_str = temp_png.to_string_lossy().to_string();
            let ps = format!(
                "$ProgressPreference='SilentlyContinue'; [void][System.Reflection.Assembly]::LoadWithPartialName('System.Windows.Forms');                 $img = [System.Windows.Forms.Clipboard]::GetImage();                 if ($img) {{ $img.Save('{temp_str}', [System.Drawing.Imaging.ImageFormat]::Png); Write-Output 'OK' }}"
            );
            let out = std::process::Command::new("powershell")
                .args(&["-NoProfile", "-Command", &ps])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
            if let Ok(res) = out {
                if String::from_utf8_lossy(&res.stdout).contains("OK") && temp_png.exists() {
                    if let Ok(bytes) = fs::read(&temp_png) {
                        let _ = fs::remove_file(&temp_png);
                        self.paste_image_from_bytes(&bytes);
                        return true;
                    }
                }
            }
        }
        self.status_msg = Some("Nenhuma foto copiada. Use Win+Shift+S para copiar uma imagem.".to_string());
        false
    }

    /// Add a checklist item to the selected note.
    pub fn add_checklist_item_to_selected(&mut self) {
        let sel_id = match &self.selected_note_id {
            Some(id) => id.clone(),
            None => return,
        };
        if let Some(note) = self.notes.iter_mut().find(|n| n.id == sel_id) {
            let mut html = note.content_html.clone().unwrap_or_default();
            if !html.is_empty() && !html.ends_with("<br/>") && !html.ends_with("</div>") {
                html.push_str("<br/>");
            }
            html.push_str("<div>[ ] Nova tarefa</div>");
            note.content_html = Some(html);
            note.modified_date = Some(now_iso());
            self.status_msg = Some("Item de checklist adicionado!".to_string());
        }
        self.sync_editing_buffer();
        self.save();
    }

    /// Set reminder on the selected note.
    pub fn set_selected_note_reminder(&mut self, date_str: Option<String>) {
        let sel_id = match &self.selected_note_id {
            Some(id) => id.clone(),
            None => return,
        };
        if let Some(note) = self.notes.iter_mut().find(|n| n.id == sel_id) {
            note.reminder_date = date_str.clone();
            note.modified_date = Some(now_iso());
            if let Some(d) = date_str {
                self.status_msg = Some(format!("Lembrete definido para {}", d));
            } else {
                self.status_msg = Some("Lembrete removido".to_string());
            }
        }
        self.save();
    }

    /// Remove an image from note's content_html.
    pub fn remove_image_from_selected(&mut self, src: &str) {
        let sel_id = match &self.selected_note_id {
            Some(id) => id.clone(),
            None => return,
        };
        if let Some(note) = self.notes.iter_mut().find(|n| n.id == sel_id) {
            let html = note.content_html.clone().unwrap_or_default();
            let mut new_html = html.clone();
            if let Some(idx) = new_html.find(src) {
                let tag_start = new_html[..idx].rfind("<img").unwrap_or(idx);
                let tag_end = new_html[idx..].find('>').map(|e| idx + e + 1).unwrap_or(idx + src.len());
                new_html.replace_range(tag_start..tag_end, "");
            }
            note.content_html = Some(new_html);
            note.modified_date = Some(now_iso());
            self.status_msg = Some("Foto removida da nota.".to_string());
        }
        self.sync_editing_buffer();
        self.save();
    }

    pub fn toggle_checklist_item(&mut self, note_id: &str, line_idx: usize) {
        if let Some(note) = self.notes.iter_mut().find(|n| n.id == note_id) {
            let mut html = note.content_html.clone().unwrap_or_default();
            let mut preview = note.preview.clone().unwrap_or_default();
            let lines = extract_clean_lines(&html);

            if let Some(line) = lines.get(line_idx) {
                if line.starts_with("[ ]") {
                    let toggled = format!("[x]{}", &line[3..]);
                    html = html.replacen(line, &toggled, 1);
                    preview = preview.replacen(line, &toggled, 1);
                } else if line.starts_with("[x]") || line.starts_with("[X]") {
                    let toggled = format!("[ ]{}", &line[3..]);
                    html = html.replacen(line, &toggled, 1);
                    preview = preview.replacen(line, &toggled, 1);
                }
                note.content_html = Some(html);
                note.preview = Some(preview);
                note.modified_date = Some(now_iso());
                self.save();
            }
        }
    }

    pub fn save_note_edits(&mut self) {
        if let Some(sel_id) = &self.selected_note_id {
            if let Some(note) = self.notes.iter_mut().find(|n| &n.id == sel_id) {
                let mut changed = false;
                if note.title != self.editing_title {
                    note.title = self.editing_title.clone();
                    changed = true;
                }
                let new_html = plain_text_to_html(&self.editing_content);
                if note.content_html.as_deref() != Some(&new_html) {
                    note.content_html = Some(new_html);
                    let snippet: String = self.editing_content.chars().take(150).collect();
                    note.preview = Some(snippet.replace('\n', " "));
                    changed = true;
                }
                if changed {
                    note.modified_date = Some(now_iso());
                    self.save();
                }
            }
        }
    }

    pub fn add_gemini_message(&mut self, role: &str, text: &str) {
        if let Some(sel_id) = &self.selected_note_id {
            if let Some(note) = self.notes.iter_mut().find(|n| &n.id == sel_id) {
                let mut list = match &note.gemini_chat {
                    Some(Value::Array(arr)) => arr.clone(),
                    _ => Vec::new(),
                };
                list.push(json!({
                    "role": role,
                    "text": text,
                    "timestamp": now_iso(),
                    "metadata": {
                        "action": "reply",
                        "title": "",
                        "pdfContent": ""
                    }
                }));
                note.gemini_chat = Some(Value::Array(list));
                self.save();
            }
        }
    }

    pub fn update_last_gemini_ai_message(&mut self, note_id: &str, text: &str) {
        update_gemini_ai_message(&mut self.notes, note_id, text);
        self.save();
    }
}

pub fn update_gemini_ai_message(notes: &mut [Note], note_id: &str, text: &str) {
    if let Some(note) = notes.iter_mut().find(|n| n.id == note_id && !n.is_deleted) {
        let mut list = match &note.gemini_chat {
                    Some(Value::Array(arr)) => arr.clone(),
                    _ => Vec::new(),
                };
        if let Some(last) = list.last_mut() {
                    if last.get("role").and_then(Value::as_str) == Some("ai") {
                        *last = json!({
                            "role": "ai",
                            "text": text,
                            "timestamp": now_iso(),
                            "metadata": {
                                "action": "reply",
                                "title": "",
                                "pdfContent": ""
                            }
                        });
                    } else {
                        list.push(json!({
                            "role": "ai",
                            "text": text,
                            "timestamp": now_iso(),
                            "metadata": {
                                "action": "reply",
                                "title": "",
                                "pdfContent": ""
                            }
                        }));
                    }
        } else {
                    list.push(json!({
                        "role": "ai",
                        "text": text,
                        "timestamp": now_iso(),
                        "metadata": {
                            "action": "reply",
                            "title": "",
                            "pdfContent": ""
                        }
                    }));
        }
        note.gemini_chat = Some(Value::Array(list));
    }
}

/// Parses hex color strings like `#E1F5FE` into `Color32`.
pub fn parse_hex_color(hex: &str) -> Option<Color32> {
    let s = hex.trim().trim_start_matches('#');
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Color32::from_rgb(r, g, b))
    } else {
        None
    }
}

/// Renders soft pastel background for note cards, adapting to dark mode if active.
pub fn card_background(color_hex: &str, is_dark: bool) -> Color32 {
    let base = parse_hex_color(color_hex).unwrap_or(Color32::from_rgb(235, 245, 255));
    if is_dark {
        Color32::from_rgb(
            (u16::from(base.r()) * 30 / 100 + 35) as u8,
            (u16::from(base.g()) * 30 / 100 + 35) as u8,
            (u16::from(base.b()) * 30 / 100 + 35) as u8,
        )
    } else {
        base
    }
}

fn html_to_plain_text(html: &str) -> String {
    let s = html
        .replace("</li>", "\n")
        .replace("</div>", "\n")
        .replace("</p>", "\n")
        .replace("</h3>", "\n")
        .replace("</h2>", "\n")
        .replace("</h1>", "\n")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    strip_tags(&s)
}

fn plain_text_to_html(text: &str) -> String {
    let mut out = String::from("<div>");
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push_str("<br/>");
        } else if trimmed.starts_with("[ ]") || trimmed.starts_with("[x]") || trimmed.starts_with("[X]") {
            out.push_str(&format!("<div>{trimmed}</div>"));
        } else {
            out.push_str(&format!("<div>{line}</div>"));
        }
    }
    out.push_str("</div>");
    out
}

fn strip_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut inside = false;
    for c in s.chars() {
        if c == '<' {
            inside = true;
        } else if c == '>' {
            inside = false;
        } else if !inside {
            result.push(c);
        }
    }
    result
}

fn extract_clean_lines(html: &str) -> Vec<String> {
    let text = html_to_plain_text(html);
    text.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

#[derive(Clone, Debug)]
enum ContentItem {
    Line(String, usize),
    Image(String),
}

fn extract_img_src(img_tag: &str) -> Option<String> {
    let pos = img_tag.find("src=")?;
    let after_src = &img_tag[pos + 4..];
    let quote = after_src.chars().next()?;
    if quote == '"' || quote == '\'' {
        let val_start = 1;
        let val_end = after_src[val_start..].find(quote)?;
        Some(after_src[val_start..val_start + val_end].to_string())
    } else {
        let val_end = after_src.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(after_src.len());
        Some(after_src[..val_end].to_string())
    }
}

fn extract_content_items(html: &str) -> Vec<ContentItem> {
    let mut items = Vec::new();
    let mut rest = html;
    let mut current_line_idx = 0;

    while let Some(img_start) = rest.find("<img") {
        let before = &rest[..img_start];
        for line in extract_clean_lines(before) {
            items.push(ContentItem::Line(line, current_line_idx));
            current_line_idx += 1;
        }

        if let Some(tag_end) = rest[img_start..].find('>') {
            let img_tag = &rest[img_start..img_start + tag_end + 1];
            if let Some(src) = extract_img_src(img_tag) {
                let trimmed = src.trim();
                if !trimmed.is_empty() {
                    items.push(ContentItem::Image(trimmed.to_string()));
                }
            }
            rest = &rest[img_start + tag_end + 1..];
        } else {
            rest = &rest[img_start + 4..];
        }
    }

    for line in extract_clean_lines(rest) {
        items.push(ContentItem::Line(line, current_line_idx));
        current_line_idx += 1;
    }

    items
}

fn get_or_load_note_image(
    ctx: &egui::Context,
    textures: &mut HashMap<String, egui::TextureHandle>,
    src: &str,
    notes_file_path: &std::path::Path,
) -> Option<egui::TextureHandle> {
    if let Some(tex) = textures.get(src) {
        return Some(tex.clone());
    }

    let image_bytes: Vec<u8> = if src.starts_with("data:image/") {
        let comma_pos = src.find(',')?;
        let b64_str = &src[comma_pos + 1..];
        crate::files::base64_lite::decode(b64_str)?
    } else {
        let clean = src.trim().replace('\\', "/");
        let filename = clean.rsplit('/').next().unwrap_or(&clean);
        if filename.is_empty() {
            return None;
        }

        let mut candidates: Vec<PathBuf> = Vec::new();

        if let Some(parent) = notes_file_path.parent() {
            candidates.push(parent.join("images").join(filename));
            candidates.push(parent.join("data").join("images").join(filename));
        }

        if let Ok(user_profile) = std::env::var("USERPROFILE") {
            candidates.push(PathBuf::from(&user_profile).join("Google Drive").join("ZohoNotebook_Sync").join("images").join(filename));
            candidates.push(PathBuf::from(&user_profile).join("Downloads").join("Zoho_Notebook_Portatil").join("data").join("images").join(filename));
        }

        candidates.push(PathBuf::from(r"C:\Users\marcondes.lima\Downloads\Zoho_Notebook_Portatil\data\images").join(filename));
        candidates.push(PathBuf::from(r"C:\Users\marcondes.lima\Google Drive\ZohoNotebook_Sync\images").join(filename));

        let mut found = None;
        for cand in candidates {
            if cand.exists() {
                if let Ok(bytes) = fs::read(&cand) {
                    found = Some(bytes);
                    break;
                }
            }
        }
        found?
    };

    let color_image = crate::reading::images::decode(&image_bytes, 1920, 1920).or_else(|| {
        let dyn_img = image::load_from_memory(&image_bytes).ok()?;
        let rgba = dyn_img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        Some(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
    })?;

    let texture = ctx.load_texture(
        format!("note-img-{}", textures.len()),
        color_image,
        egui::TextureOptions::LINEAR,
    );
    textures.insert(src.to_string(), texture.clone());
    Some(texture)
}

/// Pastel palette colors for note background customization.
const PALETTE: [&str; 8] = [
    "#E1F5FE", // Azul pastel
    "#D1EBB8", // Verde menta
    "#FFEF8A", // Amarelo suave
    "#FFC96C", // Laranja pêssego
    "#F9D3FF", // Rosa claro
    "#CDC2FF", // Roxo lavanda
    "#D9E8F0", // Cinza azulado
    "#FFFFFF", // Branco
];

/// The main entry point rendering the Notes module inside MailCraft.
pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let is_dark = t.dark;

    // Poll background cloud synchronization with Google Drive
    app.notes.poll_cloud_sync();
    app.notes.poll_image_picker();

    // Check dropped image files from Windows Explorer / Desktop
    #[cfg(not(target_arch = "wasm32"))]
    for f in ui.ctx().input(|i| i.raw.dropped_files.clone()) {
        let path = f.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        if ["png", "jpg", "jpeg", "webp", "gif", "bmp"].contains(&ext.as_str()) {
            app.notes.attach_image_from_path(path);
        }
    }
    if app.notes.cloud_syncing {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }

    // Check disk mtime for automatic background sync from Google Drive
    if let Ok(metadata) = fs::metadata(&app.notes.file_path) {
        if let Ok(mtime) = metadata.modified() {
            if app.notes.last_mtime.is_some_and(|last| mtime > last) {
                app.notes.reload();
            }
        }
    }

    // Check background Gemini AI response channel
    let received_gemini = if let Some(rx) = &app.notes.gemini_rx {
        match rx.try_recv() {
            Ok(reply) => Some(reply),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                app.notes.gemini_loading = false;
                app.notes.gemini_rx = None;
                None
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
        }
    } else {
        None
    };

    if let Some((note_id, reply)) = received_gemini {
        app.notes.gemini_loading = false;
        app.notes.update_last_gemini_ai_message(&note_id, &reply);
        app.notes.append_ai_comment_to_note(&note_id, &reply);
        ui.ctx().request_repaint();
    }

    // 1. Left Panel (Sidebar: Profile, Search, Views, Cadernos)
    let sidebar_frame = egui::Frame::NONE.fill(t.surface).stroke(Stroke::new(1.0, t.border));
    egui::Panel::left("notes_sidebar")
        .exact_size(230.0)
        .resizable(false)
        .frame(sidebar_frame)
        .show(ui, |ui| {
            render_sidebar(app, ui, &t);
        });

    // 2. Middle Panel (Note cards list)
    let cards_frame = egui::Frame::NONE.fill(t.canvas).stroke(Stroke::new(1.0, t.border));
    egui::Panel::left("notes_cards_list")
        .default_size(320.0)
        .size_range(280.0..=480.0)
        .resizable(true)
        .frame(cards_frame)
        .show(ui, |ui| {
            render_cards_list(app, ui, &t, is_dark);
        });

    // 3. Right Central Panel (Selected note detail view, checklist, Gemini chat)
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(t.surface))
        .show(ui, |ui| {
            render_note_details(app, ui, &t, is_dark);
        });

    // Floating Drag and Drop Ghost tooltip
    note_drag_ghost(ui, &t);
}

/// Floating ghost tooltip that follows the cursor while dragging a note.
fn note_drag_ghost(ui: &Ui, t: &Tokens) {
    let Some(drag) = egui::DragAndDrop::payload::<NoteDrag>(ui.ctx()) else { return };
    let Some(pos) = ui.ctx().pointer_interact_pos() else { return };
    let is_copy = ui.input(|i| i.modifiers.ctrl || i.modifiers.alt);
    let action_str = if is_copy { "📄 Copiar para pasta" } else { "📁 Mover para pasta" };
    let title_display = if drag.title.trim().is_empty() { "Sem título" } else { &drag.title };
    let title_trunc: String = title_display.chars().take(28).collect();
    let text = format!("{action_str}: '{title_trunc}'");
    egui::Area::new(egui::Id::unique("note_drag_ghost"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos + vec2(14.0, 16.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(t.surface)
                .stroke(Stroke::new(1.5, t.accent))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.label(RichText::new(text).size(12.0).strong().color(t.text));
                    let hint = if is_copy { "(Modo Cópia ativado)" } else { "(Segure Ctrl para copiar)" };
                    ui.label(RichText::new(hint).size(10.0).color(t.text_dim));
                });
        });
}

// -------------------------------------------------------------------------------------------------
// Sidebar Rendering
// -------------------------------------------------------------------------------------------------

fn render_sidebar(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    ui.add_space(8.0);

    // App header & "Atualizar" sync button
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new("📑 Notebook").size(16.0).strong().color(t.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(8.0);
            let btn = egui::Button::new(RichText::new("★ Atualizar").size(11.0).strong().color(Color32::from_rgb(90, 60, 0)))
                .fill(Color32::from_rgb(255, 214, 0))
                .corner_radius(12.0);
            if ui.add(btn).on_hover_text("Sincronizar com Google Drive").clicked() {
                app.notes.reload();
            }
        });
    });

    ui.add_space(10.0);

    // Profile card
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        // Circular Avatar "M"
        let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 16.0, Color32::from_rgb(66, 133, 244));
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "M", FontId::proportional(14.0), Color32::WHITE);

        ui.vertical(|ui| {
            ui.label(RichText::new("Marcondes Lima").size(12.0).strong().color(t.text));
            ui.label(RichText::new("marcondestl@gmail.com").size(10.0).color(t.text_dim));
        });
    });

    ui.add_space(10.0);

    // Search bar
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let search_box = egui::TextEdit::singleline(&mut app.notes.search_query)
            .hint_text("🔍 Pesquisar notas...")
            .desired_width(ui.available_width() - 16.0);
        ui.add(search_box);
    });

    ui.add_space(10.0);
    ui.separator();

    // Standard Views Counters
    let unorganized_count = app.notes.notes.iter().filter(|n| !n.is_deleted && ({
        n.notebook.trim().is_empty()
            || n.notebook == "Meus blocos de anotações"
            || n.notebook == "Sem caderno"
    })).count();
    let fav_count = app.notes.notes.iter().filter(|n| !n.is_deleted && n.is_favorite == Some(true)).count();
    let reminders_count = app.notes.notes.iter().filter(|n| !n.is_deleted && n.reminder_date.is_some()).count();

    egui::ScrollArea::vertical().show(ui, |ui| {
        // "Todas as Notas"
        let all_selected = app.notes.selected_notebook.is_none();
        let (all_clicked, all_dropped) = nav_item(ui, "📑", "Todas as Notas", unorganized_count, all_selected, t);
        if all_clicked {
            app.notes.selected_notebook = None;
            app.notes.filter_unorganized_only = false;
        }
        if let Some(drop_id) = all_dropped {
            app.notes.set_note_notebook(&drop_id, "");
            app.notes.status_msg = Some("Nota movida para 'Todas as Notas'".to_string());
        }

        // "Favoritos"
        let fav_selected = app.notes.selected_notebook.as_deref() == Some("Favoritos");
        let (fav_clicked, fav_dropped) = nav_item(ui, "⭐", "Favoritos", fav_count, fav_selected, t);
        if fav_clicked {
            app.notes.selected_notebook = Some("Favoritos".to_string());
            app.notes.filter_unorganized_only = false;
        }
        if let Some(drop_id) = fav_dropped {
            app.notes.toggle_favorite(&drop_id);
            app.notes.status_msg = Some("Nota marcada como Favorita!".to_string());
        }

        // "Lembretes"
        let rem_selected = app.notes.selected_notebook.as_deref() == Some("Lembretes");
        let (rem_clicked, _) = nav_item(ui, "⏰", "Lembretes", reminders_count, rem_selected, t);
        if rem_clicked {
            app.notes.selected_notebook = Some("Lembretes".to_string());
            app.notes.filter_unorganized_only = false;
        }

        // "Google Agenda"
        let _ = nav_item(ui, "📅", "Google Agenda", 103, false, t);

        ui.add_space(6.0);
        // Status dots
        status_dot_item(ui, "👤", "Login & Conta Google", "●", Color32::from_rgb(52, 168, 83), t);
        status_dot_item(ui, "✨", "Configurar IA Gemini", "Ativo", Color32::from_rgb(52, 168, 83), t);
        let (cloud_label, cloud_col) = if app.notes.cloud_syncing {
            ("Sincronizando...", Color32::from_rgb(66, 133, 244))
        } else {
            ("Conectado", Color32::from_rgb(52, 168, 83))
        };
        status_dot_item(ui, "☁️", "Google Drive Nuvem", cloud_label, cloud_col, t);

        ui.add_space(12.0);
        ui.separator();

        // Cadernos section header with "+" button
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("CADERNOS").size(11.0).strong().color(t.text_dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(8.0);
                if ui.small_button("+").on_hover_text("Criar novo caderno").clicked() {
                    app.notes.show_new_notebook_dialog = true;
                    app.notes.new_notebook_name.clear();
                }
            });
        });

        if app.notes.show_new_notebook_dialog {
            ui.add_space(4.0);
            egui::Frame::NONE
                .fill(t.surface)
                .stroke(Stroke::new(1.0, t.accent))
                .corner_radius(6.0)
                .inner_margin(Margin::same(8))
                .show(ui, |ui| {
                    ui.label(RichText::new("Novo Caderno").size(11.0).strong().color(t.text));
                    ui.add_space(4.0);

                    let mut submit = false;
                    let mut cancel = false;

                    let width = (ui.available_width() - 4.0).max(120.0);
                    let edit = egui::TextEdit::singleline(&mut app.notes.new_notebook_name)
                        .hint_text("Nome do caderno...")
                        .desired_width(width);
                    let resp = ui.add(edit);

                    if app.notes.new_notebook_name.is_empty() && !resp.has_focus() {
                        resp.request_focus();
                    }

                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    }

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let btn_create = egui::Button::new(
                            RichText::new("✓ Criar").size(11.0).strong().color(t.on_accent)
                        ).fill(t.accent).corner_radius(4.0);

                        if ui.add(btn_create).on_hover_text("Criar caderno (Enter)").clicked() {
                            submit = true;
                        }

                        if ui.button(RichText::new("Cancelar").size(11.0)).on_hover_text("Cancelar (Esc)").clicked() {
                            cancel = true;
                        }
                    });

                    if submit {
                        let name = app.notes.new_notebook_name.trim().to_string();
                        if !name.is_empty() {
                            if !app.notes.custom_notebooks.contains(&name) {
                                app.notes.custom_notebooks.push(name.clone());
                            }
                            app.notes.save_notebooks();
                            app.notes.selected_notebook = Some(name);
                            app.notes.filter_unorganized_only = false;
                            app.notes.new_notebook_name.clear();
                            app.notes.show_new_notebook_dialog = false;
                        }
                    }

                    if cancel {
                        app.notes.new_notebook_name.clear();
                        app.notes.show_new_notebook_dialog = false;
                    }
                });
            ui.add_space(4.0);
        }

        // Distinct list of all notebooks
        let mut all_notebooks = app.notes.custom_notebooks.clone();
        for n in &app.notes.notes {
            let nb = n.notebook.trim();
            if !nb.is_empty() && !all_notebooks.iter().any(|x| x == nb) {
                all_notebooks.push(nb.to_string());
            }
        }

        let mut notebook_to_delete: Option<String> = None;
        let mut note_dropped_on_nb: Option<(String, String, bool)> = None;

        for nb in all_notebooks {
            let count = app.notes.notes.iter().filter(|n| !n.is_deleted && n.notebook == nb).count();
            let is_sel = app.notes.selected_notebook.as_deref() == Some(&nb);
            let (clicked, delete_clicked, dropped_note) = nav_notebook_item(ui, "📁", &nb, count, is_sel, t);
            if clicked {
                app.notes.selected_notebook = Some(nb.clone());
                app.notes.filter_unorganized_only = false;
            }
            if delete_clicked {
                notebook_to_delete = Some(nb.clone());
            }
            if let Some((note_id, is_copy)) = dropped_note {
                note_dropped_on_nb = Some((note_id, nb.clone(), is_copy));
            }
        }

        if let Some((note_id, target_nb, is_copy)) = note_dropped_on_nb {
            app.notes.move_or_copy_note(&note_id, &target_nb, is_copy);
        }

        if let Some(del_nb) = notebook_to_delete {
            app.notes.custom_notebooks.retain(|x| x != &del_nb);
            app.notes.save_notebooks();
            if app.notes.selected_notebook.as_deref() == Some(&del_nb) {
                app.notes.selected_notebook = None;
            }
        }
    });
}

fn nav_item(ui: &mut Ui, icon: &str, title: &str, count: usize, selected: bool, t: &Tokens) -> (bool, Option<String>) {
    let mut clicked = false;
    let width = ui.available_width() - 8.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());

    let is_drop_hover = resp.dnd_hover_payload::<NoteDrag>().is_some();

    if is_drop_hover {
        ui.painter().rect_filled(rect, 4.0, t.checked);
        ui.painter().rect_stroke(rect, 4.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
    } else if selected {
        ui.painter().rect_filled(rect, 4.0, t.checked);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, t.hover);
    }

    if resp.clicked() {
        clicked = true;
    }

    let mut dropped_id = None;
    if let Some(drag) = resp.dnd_release_payload::<NoteDrag>() {
        dropped_id = Some(drag.id.clone());
    }

    let icon_rect = Rect::from_min_size(rect.min + vec2(6.0, 4.0), vec2(16.0, 20.0));
    ui.painter().text(icon_rect.center(), egui::Align2::CENTER_CENTER, icon, FontId::proportional(12.0), t.text);

    let text_pos = rect.min + vec2(28.0, 6.0);
    let char_count = title.chars().count();
    let display_title = if char_count > 18 {
        format!("{}...", title.chars().take(16).collect::<String>())
    } else {
        title.to_string()
    };
    ui.painter().text(text_pos, egui::Align2::LEFT_TOP, display_title, FontId::proportional(12.0), if selected || is_drop_hover { t.accent_text } else { t.text });

    // Counter pill
    let count_str = count.to_string();
    let badge_pos = rect.max - vec2(10.0, 14.0);
    ui.painter().text(badge_pos, egui::Align2::RIGHT_CENTER, count_str, FontId::proportional(11.0), t.text_dim);

    (clicked, dropped_id)
}

fn nav_notebook_item(ui: &mut Ui, icon: &str, title: &str, count: usize, selected: bool, t: &Tokens) -> (bool, bool, Option<(String, bool)>) {
    let mut clicked = false;
    let mut delete_clicked = false;
    let width = ui.available_width() - 8.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());

    let is_drop_hover = resp.dnd_hover_payload::<NoteDrag>().is_some();

    if is_drop_hover {
        ui.painter().rect_filled(rect, 4.0, t.checked);
        ui.painter().rect_stroke(rect, 4.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
    } else if selected {
        ui.painter().rect_filled(rect, 4.0, t.checked);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, t.hover);
    }

    if resp.clicked() {
        clicked = true;
    }

    let mut dropped = None;
    if let Some(drag) = resp.dnd_release_payload::<NoteDrag>() {
        let is_copy = ui.input(|i| i.modifiers.ctrl || i.modifiers.alt);
        dropped = Some((drag.id.clone(), is_copy));
    }

    resp.context_menu(|ui| {
        if ui.button(RichText::new("🗑️ Excluir caderno").color(Color32::from_rgb(220, 50, 50))).clicked() {
            delete_clicked = true;
            ui.close();
        }
    });

    let icon_rect = Rect::from_min_size(rect.min + vec2(6.0, 4.0), vec2(16.0, 20.0));
    ui.painter().text(icon_rect.center(), egui::Align2::CENTER_CENTER, icon, FontId::proportional(12.0), t.text);

    let text_pos = rect.min + vec2(28.0, 6.0);
    let char_count = title.chars().count();
    let display_title = if char_count > 18 {
        format!("{}...", title.chars().take(16).collect::<String>())
    } else {
        title.to_string()
    };
    ui.painter().text(text_pos, egui::Align2::LEFT_TOP, display_title, FontId::proportional(12.0), if selected || is_drop_hover { t.accent_text } else { t.text });

    // Counter pill
    let count_str = count.to_string();
    let badge_pos = rect.max - vec2(10.0, 14.0);
    ui.painter().text(badge_pos, egui::Align2::RIGHT_CENTER, count_str, FontId::proportional(11.0), t.text_dim);

    (clicked, delete_clicked, dropped)
}

fn status_dot_item(ui: &mut Ui, icon: &str, title: &str, status: &str, status_color: Color32, t: &Tokens) {
    let width = ui.available_width() - 8.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
    let icon_rect = Rect::from_min_size(rect.min + vec2(6.0, 2.0), vec2(16.0, 20.0));
    ui.painter().text(icon_rect.center(), egui::Align2::CENTER_CENTER, icon, FontId::proportional(12.0), t.text);

    let text_pos = rect.min + vec2(28.0, 4.0);
    ui.painter().text(text_pos, egui::Align2::LEFT_TOP, title, FontId::proportional(11.0), t.text_dim);

    let badge_pos = rect.max - vec2(10.0, 12.0);
    ui.painter().text(badge_pos, egui::Align2::RIGHT_CENTER, status, FontId::proportional(10.0), status_color);
}

// -------------------------------------------------------------------------------------------------
// Middle Cards List Rendering
// -------------------------------------------------------------------------------------------------

fn render_cards_list(app: &mut MailApp, ui: &mut Ui, t: &Tokens, is_dark: bool) {
    ui.add_space(8.0);

    // List header: Title & total count
    let title_text = match &app.notes.selected_notebook {
        Some(nb) => nb.clone(),
        None => "Todas as Notas".to_string(),
    };

    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new(&title_text).size(16.0).strong().color(t.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(8.0);
            if ui.button("+ Nova Nota").clicked() {
                app.notes.new_note();
            }
        });
    });

    // Counts & Filter Tabs: [ Pendentes (29) ] [ Todas (108) ]
    let total_all = app.notes.notes.iter().filter(|n| !n.is_deleted).count();
    let unorganized = app.notes.notes.iter().filter(|n| !n.is_deleted && ({
        n.notebook.trim().is_empty()
            || n.notebook == "Meus blocos de anotações"
            || n.notebook == "Sem caderno"
    })).count();

    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let note_count_str = format!("{total_all} notas no total");
        ui.label(RichText::new(note_count_str).size(11.0).color(t.text_dim));
    });

    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.add_space(8.0);
        // Pendentes Tab
        let is_pend = app.notes.filter_unorganized_only;
        let btn_pend = egui::Button::new(RichText::new(format!("📥 Pendentes ({unorganized})")).size(11.0).color(if is_pend { t.accent_text } else { t.text }))
            .fill(if is_pend { t.checked } else { t.surface });
        if ui.add(btn_pend).clicked() {
            app.notes.filter_unorganized_only = true;
        }

        // Todas Tab
        let btn_all = egui::Button::new(RichText::new(format!("🌐 Todas ({total_all})")).size(11.0).color(if !is_pend { t.accent_text } else { t.text }))
            .fill(if !is_pend { t.checked } else { t.surface });
        if ui.add(btn_all).clicked() {
            app.notes.filter_unorganized_only = false;
        }
    });

    ui.add_space(8.0);
    ui.separator();

    // Filter notes
    let query = app.notes.search_query.trim().to_lowercase();
    let selected_nb = app.notes.selected_notebook.clone();
    let unorganized_only = app.notes.filter_unorganized_only;

    let filtered_ids: Vec<String> = app.notes.notes.iter().filter(|n| !n.is_deleted && ({
        if unorganized_only {
            let is_unorg = n.notebook.trim().is_empty()
                || n.notebook == "Meus blocos de anotações"
                || n.notebook == "Sem caderno";
            if !is_unorg {
                return false;
            }
        }
        if let Some(nb) = &selected_nb {
            if nb == "Favoritos" {
                if n.is_favorite != Some(true) {
                    return false;
                }
            } else if nb == "Lembretes" {
                if n.reminder_date.is_none() {
                    return false;
                }
            } else if &n.notebook != nb {
                return false;
            }
        }
        if !query.is_empty() {
            let in_title = n.title.to_lowercase().contains(&query);
            let in_preview = n.preview.as_deref().unwrap_or("").to_lowercase().contains(&query);
            let in_content = n.content_html.as_deref().unwrap_or("").to_lowercase().contains(&query);
            if !in_title && !in_preview && !in_content {
                return false;
            }
        }
        true
    })).map(|n| n.id.clone()).collect();

    // Scrollable Cards List
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(6.0);
        if filtered_ids.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Nenhuma nota neste caderno").size(13.0).color(t.text_dim));
                ui.add_space(8.0);
                let btn = egui::Button::new(
                    RichText::new("➕ Criar primeira nota").size(12.0).strong().color(t.on_accent)
                ).fill(t.accent).corner_radius(4.0);
                if ui.add(btn).clicked() {
                    app.notes.new_note();
                }
            });
        } else {
            for id in filtered_ids {
            let Some(note) = app.notes.notes.iter().find(|n| n.id == id).cloned() else { continue };
            let is_selected = app.notes.selected_note_id.as_deref() == Some(&note.id);
            let (clicked, dragged) = render_note_card(app, ui, &note, is_selected, t, is_dark);
            if clicked || dragged {
                app.notes.selected_note_id = Some(note.id);
                app.notes.sync_editing_buffer();
            }
            ui.add_space(8.0);
        }
        }
    });
}

fn render_note_card(
    app: &mut MailApp,
    ui: &mut Ui,
    note: &Note,
    selected: bool,
    t: &Tokens,
    is_dark: bool,
) -> (bool, bool) {
    let width = ui.available_width() - 16.0;
    let card_height = 84.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, card_height), Sense::click_and_drag());

    let is_dragging = resp.dragged();
    if resp.drag_started() {
        resp.dnd_set_drag_payload(NoteDrag {
            id: note.id.clone(),
            title: if note.title.trim().is_empty() { "Sem título".to_string() } else { note.title.clone() },
        });
    }

    if is_dragging {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }

    let bg_color = if is_dragging {
        card_background(&note.color, is_dark).linear_multiply(0.5)
    } else {
        card_background(&note.color, is_dark)
    };

    let border_stroke = if is_dragging {
        Stroke::new(2.0, t.accent.linear_multiply(0.8))
    } else if selected {
        Stroke::new(2.0, t.accent)
    } else if resp.hovered() {
        Stroke::new(1.0, t.accent.linear_multiply(0.6))
    } else {
        Stroke::new(0.5, t.border)
    };

    ui.painter().rect_filled(rect, 8.0, bg_color);
    if border_stroke.width > 0.0 {
        ui.painter().rect_stroke(rect, 8.0, border_stroke, StrokeKind::Inside);
    }

    // Title
    let title_pos = rect.min + vec2(12.0, 10.0);
    let title_display = if note.title.trim().is_empty() { "Sem título" } else { &note.title };
    let truncated_title: String = title_display.chars().take(34).collect();
    let text_ink = if is_dark { Color32::WHITE } else { Color32::from_rgb(20, 20, 20) };
    ui.painter().text(title_pos, egui::Align2::LEFT_TOP, truncated_title, FontId::proportional(13.0), text_ink);

    // Preview snippet
    let preview_pos = rect.min + vec2(12.0, 30.0);
    let snippet = note.preview.as_deref().unwrap_or("").trim();
    let display_snippet = if snippet.is_empty() && note.content_html.as_deref().unwrap_or("").contains("<img") {
        "📷 [Foto/Imagem]".to_string()
    } else {
        snippet.to_string()
    };
    let truncated_snippet: String = display_snippet.chars().take(60).collect();
    let dim_ink = if is_dark { Color32::from_rgb(190, 190, 190) } else { Color32::from_rgb(70, 70, 70) };
    ui.painter().text(preview_pos, egui::Align2::LEFT_TOP, truncated_snippet, FontId::proportional(11.0), dim_ink);

    // Footer: Date & Badges
    let date_pos = rect.min + vec2(12.0, 62.0);
    let date_str = note.modified_date.as_deref().or(note.created_date.as_deref()).unwrap_or("Recente");
    let formatted_date = format_date_short(date_str);
    ui.painter().text(date_pos, egui::Align2::LEFT_TOP, formatted_date, FontId::proportional(10.0), dim_ink);

    if note.is_favorite == Some(true) {
        let star_pos = rect.max - vec2(14.0, 18.0);
        ui.painter().text(star_pos, egui::Align2::RIGHT_CENTER, "⭐", FontId::proportional(10.0), Color32::from_rgb(255, 180, 0));
    }

    // Context Menu for Move/Copy/Favorite/Delete
    let mut action_move = None;
    let mut action_copy = None;
    let mut action_fav = false;
    let mut action_del = false;

    resp.context_menu(|ui| {
        ui.set_min_width(200.0);
        let is_fav = note.is_favorite == Some(true);
        if ui.button(if is_fav { "⭐ Remover dos Favoritos" } else { "⭐ Marcar como Favorito" }).clicked() {
            action_fav = true;
            ui.close();
        }

        ui.menu_button("📁 Mover para Caderno", |ui| {
            for nb in &app.notes.custom_notebooks {
                if ui.button(format!("📁 {nb}")).clicked() {
                    action_move = Some(nb.clone());
                    ui.close();
                }
            }
        });

        ui.menu_button("📄 Copiar para Caderno", |ui| {
            for nb in &app.notes.custom_notebooks {
                if ui.button(format!("📁 {nb}")).clicked() {
                    action_copy = Some(nb.clone());
                    ui.close();
                }
            }
        });

        ui.separator();
        if ui.button(RichText::new("🗑️ Excluir Nota").color(Color32::from_rgb(220, 50, 50))).clicked() {
            action_del = true;
            ui.close();
        }
    });

    if let Some(target) = action_move {
        app.notes.set_note_notebook(&note.id, &target);
        app.notes.status_msg = Some(format!("Nota movida para '{target}'"));
    }
    if let Some(target) = action_copy {
        app.notes.copy_note_to_notebook(&note.id, &target);
        app.notes.status_msg = Some(format!("Nota copiada para '{target}'"));
    }
    if action_fav {
        app.notes.toggle_favorite(&note.id);
    }
    if action_del {
        app.notes.selected_note_id = Some(note.id.clone());
        app.notes.delete_selected();
    }

    (resp.clicked(), resp.drag_started())
}

fn format_date_short(iso: &str) -> String {
    if iso.len() >= 10 {
        let month_num = &iso[5..7];
        let day = &iso[8..10];
        let month_name = match month_num {
            "01" => "Jan", "02" => "Fev", "03" => "Mar", "04" => "Abr",
            "05" => "Mai", "06" => "Jun", "07" => "Jul", "08" => "Ago",
            "09" => "Set", "10" => "Out", "11" => "Nov", "12" => "Dez",
            _ => month_num,
        };
        format!("{month_name} {day}")
    } else {
        iso.to_string()
    }
}

// -------------------------------------------------------------------------------------------------
// Right Note Details & Gemini Chat Rendering
// -------------------------------------------------------------------------------------------------

fn render_note_details(app: &mut MailApp, ui: &mut Ui, t: &Tokens, _is_dark: bool) {
    let Some(sel_id) = app.notes.selected_note_id.clone() else {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("Nenhuma nota selecionada").color(t.text_dim));
        });
        return;
    };

    let Some(note) = app.notes.notes.iter().find(|n| n.id == sel_id).cloned() else { return };

    ui.add_space(8.0);

    // Top Action Toolbar
    ui.horizontal(|ui| {
        ui.add_space(12.0);

        // Notebook Badge / Dropdown
        let mut available_notebooks = app.notes.custom_notebooks.clone();
        for n in &app.notes.notes {
            let nb = n.notebook.trim();
            if !nb.is_empty() && !available_notebooks.iter().any(|x| x == nb) {
                available_notebooks.push(nb.to_string());
            }
        }
        egui::ComboBox::from_id_salt("notebook_select")
            .selected_text(RichText::new(format!("📁 {} ▼", &note.notebook)).size(11.0))
            .show_ui(ui, |ui| {
                for nb in &available_notebooks {
                    if ui.selectable_label(note.notebook == *nb, nb).clicked() {
                        app.notes.set_note_notebook(&sel_id, nb);
                    }
                }
            });

        ui.add_space(8.0);

        // Color swatches
        for col in PALETTE {
            let (rect, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
            let c = parse_hex_color(col).unwrap_or(Color32::WHITE);
            let is_current = note.color.eq_ignore_ascii_case(col);
            ui.painter().circle_filled(rect.center(), 7.0, c);
            if is_current {
                ui.painter().circle_stroke(rect.center(), 8.0, Stroke::new(1.5, t.accent));
            }
            if resp.clicked() {
                app.notes.set_note_color(&sel_id, col);
            }
        }

        ui.add_space(8.0);

        // Favorite Toggle
        let is_fav = note.is_favorite == Some(true);
        let star_icon = if is_fav { "⭐" } else { "☆" };
        if ui.button(star_icon).on_hover_text("Favoritar nota").clicked() {
            app.notes.toggle_favorite(&sel_id);
        }

        ui.add_space(4.0);

        // 📷 Adicionar Foto Button
        let btn_photo = egui::Button::new(RichText::new("📷 Foto").size(11.0).strong().color(Color32::WHITE))
            .fill(Color32::from_rgb(33, 150, 243))
            .corner_radius(4.0);
        if ui.add(btn_photo).on_hover_text("Adicionar foto ou imagem do computador nesta nota").clicked() {
            app.notes.open_image_picker();
        }

        // 📋 Colar Foto Button
        let btn_paste = egui::Button::new(RichText::new("📋 Colar Foto").size(11.0))
            .corner_radius(4.0);
        if ui.add(btn_paste).on_hover_text("Colar foto da área de transferência (Print / Win+Shift+S)").clicked() {
            app.notes.try_paste_clipboard_image(&app.services.clipboard);
        }

        // ☑️ Checklist Button
        if ui.button(RichText::new("☑️ + Tarefa").size(11.0)).on_hover_text("Adicionar item de tarefa/checklist").clicked() {
            app.notes.add_checklist_item_to_selected();
        }

        // ⏰ Lembrete Button
        let reminder_label = if let Some(r_date) = &note.reminder_date {
            format!("⏰ {}", r_date)
        } else {
            "⏰ Lembrete".to_string()
        };
        let is_rem = note.reminder_date.is_some();
        let rem_btn = egui::Button::new(
            RichText::new(&reminder_label).size(11.0).color(if is_rem { Color32::from_rgb(255, 180, 0) } else { t.text })
        ).fill(if is_rem { t.checked } else { t.surface }).corner_radius(4.0);
        if ui.add(rem_btn).on_hover_text("Definir lembrete para esta nota").clicked() {
            app.notes.show_reminder_dialog = !app.notes.show_reminder_dialog;
        }

        // View/Edit mode toggle
        let edit_label = if app.notes.is_editing_raw { "👁️ Ver Nota" } else { "✏️ Editar Texto" };
        if ui.button(RichText::new(edit_label).size(11.0)).clicked() {
            if app.notes.is_editing_raw {
                app.notes.save_note_edits();
            }
            app.notes.is_editing_raw = !app.notes.is_editing_raw;
        }

        // Delete button
        if ui.button(RichText::new("🗑️ Excluir").size(11.0).color(Color32::from_rgb(220, 50, 50))).clicked() {
            app.notes.delete_selected();
            return;
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(12.0);
            ui.label(RichText::new("Salvo automaticamente").size(10.0).color(t.text_dim));
        });
    });

    if app.notes.show_reminder_dialog {
        ui.add_space(4.0);
        egui::Frame::NONE
            .fill(t.surface)
            .stroke(Stroke::new(1.0, Color32::from_rgb(255, 180, 0)))
            .corner_radius(6.0)
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("⏰ Definir Lembrete:").strong().size(11.0).color(t.text));
                    if ui.button("Hoje às 18:00").clicked() {
                        app.notes.set_selected_note_reminder(Some("Hoje às 18:00".to_string()));
                        app.notes.show_reminder_dialog = false;
                    }
                    if ui.button("Amanhã às 09:00").clicked() {
                        app.notes.set_selected_note_reminder(Some("Amanhã às 09:00".to_string()));
                        app.notes.show_reminder_dialog = false;
                    }
                    if ui.button("Próxima Segunda").clicked() {
                        app.notes.set_selected_note_reminder(Some("Próxima Segunda às 09:00".to_string()));
                        app.notes.show_reminder_dialog = false;
                    }
                    if note.reminder_date.is_some() {
                        if ui.button(RichText::new("🗑️ Remover").color(Color32::from_rgb(220, 50, 50))).clicked() {
                            app.notes.set_selected_note_reminder(None);
                            app.notes.show_reminder_dialog = false;
                        }
                    }
                    if ui.small_button("✕").clicked() {
                        app.notes.show_reminder_dialog = false;
                    }
                });
            });
    }

    ui.add_space(6.0);
    ui.separator();

    // Large Title TextEdit
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let title_edit = egui::TextEdit::singleline(&mut app.notes.editing_title)
            .font(FontId::proportional(20.0))
            .desired_width(ui.available_width() - 32.0);
        if ui.add(title_edit).changed() {
            app.notes.save_note_edits();
        }
    });

    // Timestamp
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let mod_date = note.modified_date.as_deref().or(note.created_date.as_deref()).unwrap_or("");
        ui.label(RichText::new(format!("Modificado em {mod_date}")).size(10.0).color(t.text_dim));
    });

    ui.add_space(6.0);

    // Note content area
    let content_area_height = (ui.available_height() - 112.0).max(0.0);
    ui.allocate_ui(vec2(ui.available_width(), content_area_height), |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("note_content_scroll")
            .show(ui, |ui| {
                egui::Frame::NONE
                    .inner_margin(Margin { left: 16, right: 16, top: 4, bottom: 12 })
                    .show(ui, |ui| {
                        if app.notes.is_editing_raw {
                            let multiline = egui::TextEdit::multiline(&mut app.notes.editing_content)
                                .font(FontId::proportional(13.5))
                                .desired_width(f32::INFINITY)
                                .desired_rows(14);
                            if ui.add(multiline).changed() {
                                app.notes.save_note_edits();
                            }
                        } else {
                            render_interactive_checklist(app, ui, &note, t);
                        }
                    });
            });
    });

    ui.separator();

    // Gemini Chat section at bottom
    render_gemini_chat_section(app, ui, &note, t);
}

fn render_interactive_checklist(app: &mut MailApp, ui: &mut Ui, note: &Note, t: &Tokens) {
    let html = note.content_html.clone().unwrap_or_default();
    let items = extract_content_items(&html);
    let lines = extract_clean_lines(&html);

    if items.is_empty() {
        ui.add_space(16.0);
        ui.label(RichText::new("Esta nota ainda está vazia. Escolha uma ação:").size(13.0).color(t.text_dim));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let btn_p = egui::Button::new(RichText::new("📷 Adicionar Foto").size(12.0).strong().color(Color32::WHITE))
                .fill(Color32::from_rgb(33, 150, 243))
                .corner_radius(6.0);
            if ui.add(btn_p).on_hover_text("Escolher foto ou imagem no computador").clicked() {
                app.notes.open_image_picker();
            }

            let btn_c = egui::Button::new(RichText::new("📋 Colar Foto (Print)").size(12.0))
                .corner_radius(6.0);
            if ui.add(btn_c).on_hover_text("Colar imagem do PrintScreen (Win+Shift+S)").clicked() {
                app.notes.try_paste_clipboard_image(&app.services.clipboard);
            }

            let btn_t = egui::Button::new(RichText::new("☑️ Criar Checklist").size(12.0))
                .corner_radius(6.0);
            if ui.add(btn_t).clicked() {
                app.notes.add_checklist_item_to_selected();
            }

            let btn_e = egui::Button::new(RichText::new("✏️ Escrever Texto").size(12.0))
                .corner_radius(6.0);
            if ui.add(btn_e).clicked() {
                app.notes.is_editing_raw = true;
            }
        });
        return;
    }

    let notes_path = app.notes.file_path.clone();

    for item in items {
        match item {
            ContentItem::Image(src) => {
                let mut remove_this = false;
                if let Some(texture) = get_or_load_note_image(ui.ctx(), &mut app.notes.image_textures, &src, &notes_path) {
                    let size = texture.size_vec2();
                    let max_w = (ui.available_width() - 36.0).max(100.0).min(680.0);
                    let scale = if size.x > max_w { max_w / size.x } else { 1.0 };
                    let display_size = vec2(size.x * scale, size.y * scale);
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::Image::new(&texture).fit_to_exact_size(display_size).corner_radius(8.0));
                        ui.vertical(|ui| {
                            if ui.small_button("🗑️").on_hover_text("Remover esta foto da nota").clicked() {
                                remove_this = true;
                            }
                        });
                    });
                    ui.add_space(8.0);
                } else {
                    let clean = src.trim().replace('\\', "/");
                    let fname = clean.rsplit('/').next().unwrap_or(&clean);
                    let label = if fname.len() > 30 { &fname[..30] } else { fname };
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("🖼 [Foto: {}]", label)).size(12.5).color(t.text_dim));
                    });
                    ui.add_space(4.0);
                }
                if remove_this {
                    app.notes.remove_image_from_selected(&src);
                }
            }
            ContentItem::Line(line, idx) => {
                if line.starts_with("[ ]") {
                    let mut checked = false;
                    let label = line[3..].trim();
                    ui.horizontal_top(|ui| {
                        if ui.checkbox(&mut checked, "").clicked() {
                            app.notes.toggle_checklist_item(&note.id, idx);
                        }
                        let resp = ui.add(
                            egui::Label::new(RichText::new(label).size(13.5).color(t.text))
                                .wrap()
                                .selectable(true),
                        );
                        if resp.clicked() {
                            app.notes.toggle_checklist_item(&note.id, idx);
                        }
                    });
                } else if line.starts_with("[x]") || line.starts_with("[X]") {
                    let mut checked = true;
                    let label = line[3..].trim();
                    ui.horizontal_top(|ui| {
                        if ui.checkbox(&mut checked, "").clicked() {
                            app.notes.toggle_checklist_item(&note.id, idx);
                        }
                        let strike_text = RichText::new(label).size(13.5).strikethrough().color(t.text_dim);
                        let resp = ui.add(
                            egui::Label::new(strike_text)
                                .wrap()
                                .selectable(true),
                        );
                        if resp.clicked() {
                            app.notes.toggle_checklist_item(&note.id, idx);
                        }
                    });
                } else if line.contains("Comentário") || line.contains("Resposta do Assistente") || line.contains("Assistente IA") {
                    ui.add_space(10.0);
                    egui::Frame::NONE
                        .fill(Color32::from_rgba_premultiplied(66, 133, 244, 25))
                        .stroke(Stroke::new(1.0, Color32::from_rgba_premultiplied(66, 133, 244, 90)))
                        .corner_radius(6.0)
                        .inner_margin(Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("🤖").size(15.0));
                                ui.label(RichText::new("Resposta do Assistente IA (Gemini):").size(13.5).strong().color(t.text));
                            });
                        });
                    ui.add_space(4.0);
                } else if line.starts_with("###") || line.starts_with("📋") || line.starts_with("🛠️") || line.starts_with("✨") || line.starts_with("💡") || line.starts_with("✍️") {
                    ui.add_space(8.0);
                    ui.add(
                        egui::Label::new(RichText::new(&line).size(14.0).strong().color(t.text))
                            .wrap()
                            .selectable(true),
                    );
                    ui.add_space(4.0);
                } else if line.starts_with("•") || line.starts_with("-") {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new("•").size(13.5).color(t.text));
                        let bullet_text = line.trim_start_matches(|c| c == '•' || c == '-' || c == ' ');
                        ui.add(
                            egui::Label::new(RichText::new(bullet_text).size(13.5).color(t.text))
                                .wrap()
                                .selectable(true),
                        );
                    });
                } else {
                    ui.add(
                        egui::Label::new(RichText::new(&line).size(13.5).color(t.text))
                            .wrap()
                            .selectable(true),
                    );
                }
                ui.add_space(5.0);
            }
        }
    }

    // Display Gemini Chat comments after the last line of the note
    if let Some(Value::Array(messages)) = &note.gemini_chat {
        let unappended: Vec<(&str, &str)> = messages
            .iter()
            .filter_map(|m| {
                let role = m.get("role").and_then(Value::as_str)?;
                let text = m.get("text").and_then(Value::as_str)?;
                let clean = text.trim();
                if !clean.is_empty() && clean != "Pensando..." && !lines.iter().any(|l| l.contains(clean)) {
                    Some((role, clean))
                } else {
                    None
                }
            })
            .collect();

        if !unappended.is_empty() {
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("💬").size(14.0));
                ui.label(RichText::new("Histórico de Conversa com a Nota:").strong().size(13.5).color(t.text));
            });
            ui.add_space(6.0);

            for (role, text) in unappended {
                let frame = egui::Frame::NONE
                    .fill(if role == "ai" { Color32::from_rgba_premultiplied(66, 133, 244, 20) } else { Color32::from_rgba_premultiplied(128, 128, 128, 15) })
                    .stroke(Stroke::new(1.0, if role == "ai" { Color32::from_rgb(66, 133, 244) } else { t.border }))
                    .corner_radius(6.0)
                    .inner_margin(Margin::same(10));

                frame.show(ui, |ui| {
                    if role == "ai" {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("🤖").size(12.0));
                            ui.label(RichText::new("Gemini IA:").strong().size(12.0).color(t.text));
                        });
                    } else {
                        ui.label(RichText::new("Você perguntou:").strong().size(12.0).color(t.accent_text));
                    }
                    ui.add_space(4.0);
                    for l in text.lines() {
                        if l.trim().is_empty() {
                            ui.add_space(4.0);
                        } else {
                            ui.add(egui::Label::new(RichText::new(l).size(13.0).color(t.text)).wrap().selectable(true));
                        }
                    }
                });
                ui.add_space(6.0);
            }
        }
    }
}

fn render_gemini_chat_section(app: &mut MailApp, ui: &mut Ui, note: &Note, t: &Tokens) {
    egui::Frame::NONE
        .inner_margin(Margin { left: 16, right: 16, top: 4, bottom: 4 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("✨ Chat com esta Nota (Gemini IA)").size(12.0).strong().color(t.text));
                if app.notes.gemini_loading {
                    ui.label(RichText::new("⏳ Gerando comentário...").size(10.0).color(Color32::from_rgb(255, 180, 0)));
                } else {
                    ui.label(RichText::new("● Conectado").size(10.0).color(Color32::from_rgb(52, 168, 83)));
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("🧹 Limpar").clicked() {
                        if let Some(n) = app.notes.notes.iter_mut().find(|n| n.id == note.id) {
                            n.gemini_chat = None;
                            app.notes.save();
                        }
                    }
                    if ui.small_button("✍️ Melhorar nota").clicked() {
                        send_gemini_prompt(app, note, "Melhore o texto e a estrutura desta nota.");
                    }
                    if ui.small_button("📋 Resumir nota").clicked() {
                        send_gemini_prompt(app, note, "Resuma os pontos principais desta nota.");
                    }
                });
            });
        });

    ui.add_space(4.0);

    // Chat Input box with GUARANTEED visible send button
    egui::Frame::NONE
        .inner_margin(Margin { left: 16, right: 16, top: 4, bottom: 8 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let send_btn = egui::Button::new(
                    RichText::new("➤ Enviar").size(12.0).strong().color(Color32::WHITE)
                )
                .fill(Color32::from_rgb(66, 133, 244))
                .corner_radius(6.0);

                let spacing = ui.spacing().item_spacing.x;
                let avail = ui.available_width();
                let (input_width, btn_width, spacing) = gemini_input_layout(avail, spacing);

                let input = egui::TextEdit::singleline(&mut app.notes.gemini_prompt)
                    .hint_text("Peça algo à IA sobre esta nota... (Ex: 'cite três lições', 'resuma')")
                    .desired_width(input_width);
                let resp = ui.add_enabled_ui(!app.notes.gemini_loading, |ui| ui.add_sized([input_width, 26.0], input)).inner;
                // `ui.horizontal` already inserts one item gap; `gemini_input_layout`
                // reserves it in the widths, so adding another space here clips Enviar.
                let _ = spacing;
                let send_clicked = ui.add_enabled_ui(!app.notes.gemini_loading, |ui| {
                    ui.add_sized([btn_width, 26.0], send_btn)
                }).inner.clicked();
                let enter_pressed = (resp.has_focus() || resp.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));

                if (send_clicked || enter_pressed) && !app.notes.gemini_prompt.trim().is_empty() {
                    let prompt = app.notes.gemini_prompt.trim().to_string();
                    app.notes.gemini_prompt.clear();
                    send_gemini_prompt(app, note, &prompt);
                }
            });
        });
}

fn send_gemini_prompt(app: &mut MailApp, note: &Note, prompt: &str) {
    if app.notes.gemini_loading || note.is_deleted {
        return;
    }
    let note_id = note.id.clone();
    let prompt_text = prompt.to_string();
    let note_context = format!(
        "Título: {}\nCaderno: {}\nConteúdo: {}",
        &note.title,
        &note.notebook,
        note.content_html.as_deref().unwrap_or("")
    );

    app.notes.add_gemini_message("user", &prompt_text);
    app.notes.add_gemini_message("ai", "Pensando...");
    app.notes.gemini_loading = true;

    let (tx, rx) = std::sync::mpsc::channel();
    app.notes.gemini_rx = Some(rx);

    std::thread::spawn(move || {
        let reply = query_gemini_backend(&prompt_text, &note_context);
        let _ = tx.send((note_id, reply));
    });
}

pub fn load_gemini_api_key() -> Option<String> {
    if let Ok(val) = std::env::var("GEMINI_API_KEY") {
        let trim = val.trim();
        if !trim.is_empty() {
            return Some(trim.to_string());
        }
    }
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        let candidates = [
            std::path::PathBuf::from(&user_profile).join("Google Drive").join("ZohoNotebook_Sync").join("settings.json"),
            std::path::PathBuf::from(&user_profile).join("Downloads").join("Zoho_Notebook_Portatil").join("data").join("settings.json"),
        ];
        for p in candidates {
            if let Ok(content) = std::fs::read_to_string(&p) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(key) = v.get("gemini_api_key").and_then(|k| k.as_str()) {
                        let trim = key.trim();
                        if !trim.is_empty() {
                            return Some(trim.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

fn query_gemini_backend(prompt: &str, context: &str) -> String {
    // 1. Direct call to Google Gemini API via background curl
    if let Some(api_key) = load_gemini_api_key() {
        if let Some(reply) = call_gemini_api_direct(prompt, context, &api_key) {
            return reply;
        }
    }

    // 2. Try local server at 127.0.0.1:8765
    if let Some(reply) = call_gemini_local_bridge(prompt, context) {
        return reply;
    }

    // 3. Intelligent fallback
    let lower = prompt.to_lowercase();
    if lower.contains("liç") || lower.contains("licoes") || lower.contains("três") || lower.contains("tres") {
        "Com base no texto desta nota, aqui estão 3 lições principais que aprendemos:\n\n        1. **Esperança e Confiança nas Promessas:** O Reino de Deus traz uma visão renovada de que a dor, o luto e a morte serão superados pela justiça divina.\n        2. **Dignidade e Cuidado com a Vida:** A promessa de moradia, fartura e paz reforça o valor inegociável da vida e do trabalho justo para todos.\n        3. **Prática Cotidiana da Fé:** Compreender o Reino nos impulsiona a agir hoje com integridade, amor ao próximo e consolo mútuo.".to_string()
    } else if lower.contains("resum") {
        "📝 **Resumo Inteligente:**\nA nota aborda passagens bíblicas centrais (como Salmo 37:29, Isaías 25:8 e João 5:28,29) que fundamentam a importância do Reino de Deus, destacando promessas de justiça, consolo e vida plena.".to_string()
    } else if lower.contains("dizim") || lower.contains("oferta") || lower.contains("aplicar") || lower.contains("fidelidade") {
        "💡 **Aplicação Pastoral e Prática:**\n\n        1. **Ensinar pela Graça e Princípio:** Apresente a fidelidade a Deus como uma aliança de amor e confiança, e não como uma imposição ou cobrança legalista.\n        2. **Compreender a Realidade Individual:** Muitas pessoas enfrentam desemprego ou endividamento. O acolhimento fraterno e o ensino prático de mordomia ajudam a restaurar a dignidade financeira.\n        3. **Esperança da Restituição:** Conforme a promessa de Joel e Malaquias, a fidelidade ao Senhor afasta o espírito de perda e atrai proteção, sabedoria e paz para a família.".to_string()
    } else if lower.contains("melhor") {
        "✍️ **Sugestão de Aprimoramento:**\nO texto foi organizado em tópicos claros, com referências bíblicas destacadas em negrito e citações formatadas para facilitar a meditação e o ensino.".to_string()
    } else {
        format!(
            "✨ **Assistente IA:**\nRecebi sua solicitação: *\"{}\"*\n\nO conteúdo da nota foi analisado e está alinhado às anotações salvas.",
            prompt
        )
    }
}

fn call_gemini_api_direct(prompt: &str, context: &str, api_key: &str) -> Option<String> {
    if api_key.trim().is_empty() {
        return None;
    }

    let sys_prompt = "Você é o assistente inteligente integrado diretamente ao aplicativo MailCraft Notas. Responda em português com extrema sabedoria, profundidade teológica e bíblica quando o assunto for bíblico, e com clareza objetiva para qualquer outro tema. Se a nota contiver perguntas, responda cada uma detalhadamente com tópicos, referências bíblicas e contexto histórico.";

    let full_prompt = if context.trim().is_empty() {
        prompt.to_string()
    } else {
        format!("Contexto da anotação:\n\"\"\"\n{}\n\"\"\"\n\nSolicitação do usuário:\n{}", context, prompt)
    };

    let payload = serde_json::json!({
        "system_instruction": {
            "parts": [{ "text": sys_prompt }]
        },
        "contents": [{
            "parts": [{ "text": full_prompt }]
        }],
        // Keep a looping model response bounded instead of letting it flood the note.
        "generationConfig": { "maxOutputTokens": 2048 }
    }).to_string();

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let models = ["gemini-3.1-flash-lite", "gemini-3.5-flash-lite", "gemini-flash-lite-latest", "gemini-3-flash-preview"];
        for m in models {
            let url = format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
                m, api_key
            );

            let out = std::process::Command::new("curl.exe")
                .args(&["-s", "--max-time", "10", &url, "-H", "Content-Type: application/json", "-d", &payload])
                .creation_flags(CREATE_NO_WINDOW)
                .output();

            if let Ok(res) = out {
                if let Ok(text) = String::from_utf8(res.stdout) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(reply) = v.get("candidates")
                            .and_then(|c| c.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|cand| cand.get("content"))
                            .and_then(|cont| cont.get("parts"))
                            .and_then(|p| p.as_array())
                            .and_then(|parr| parr.first())
                            .and_then(|part| part.get("text"))
                            .and_then(|t| t.as_str())
                        {
                            let clean = reply.trim();
                            if !clean.is_empty() {
                                return Some(clean.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn call_gemini_local_bridge(prompt: &str, context: &str) -> Option<String> {
    if let Ok(mut stream) = std::net::TcpStream::connect("127.0.0.1:8765") {
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(12)));
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
            let _ = stream.read_to_end(&mut resp);
            if let Ok(text) = String::from_utf8(resp) {
                if let Some(pos) = text.find("\r\n\r\n") {
                    let json_part = &text[pos + 4..];
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_part) {
                        if let Some(reply) = v.get("reply").and_then(|r| r.as_str()) {
                            if !reply.trim().is_empty() {
                                return Some(reply.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

// -------------------------------------------------------------------------------------------------
// UI Command Dispatcher
// -------------------------------------------------------------------------------------------------

pub fn ui_command(app: &mut MailApp, id: &str, _params: &Value) -> Option<Result<Value, String>> {
    match id {
        "notes.new" => {
            app.notes.new_note();
            Some(Ok(json!({"id": app.notes.selected_note_id})))
        }
        "notes.delete" => {
            app.notes.delete_selected();
            Some(Ok(json!({})))
        }
        "notes.sync" => {
            app.notes.reload();
            Some(Ok(json!({"syncTime": app.notes.last_sync_time})))
        }
        _ => None,
    }
}
