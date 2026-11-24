// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Named signature file sources (#1878). Paths belong to this installation's settings, never
//! to an imported mailbox. Only the host installs the engine's loader; agents cannot set paths.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;

use mailcraft_model::Signature;
use mailcraft_model::signature::{MAX_HTML, MAX_PICTURE_BYTES, MAX_TEXT};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{MailApp, files};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Setting {
    pub enabled: bool,
    pub path: String,
}

/// Only weak step identities and changed keys are kept, never aliases for removed signatures.
#[derive(Default)]
pub(crate) struct History(Vec<HistoryEntry>);

struct HistoryEntry {
    token: std::sync::Weak<()>,
    action: HistoryAction,
}

enum HistoryAction {
    Rename { from: String, to: String },
    Remove { key: String, source: Option<Setting> },
}

pub(crate) struct Change {
    key: Option<String>,
    created: Option<String>,
    step: Option<std::sync::Weak<()>>,
    moved: u64,
}

pub(crate) fn before_command(app: &MailApp, id: &str, p: &Value) -> Change {
    Change {
        key: matches!(id, "signature.rename" | "signature.remove")
            .then(|| p.get("name").and_then(Value::as_str).and_then(|name| key_for_name(app, name)))
            .flatten(),
        created: (id == "signature.set")
            .then(|| {
                let name = mailcraft_model::clean_name(p.get("name")?.as_str()?)?;
                app.session.mailbox.signature(&name).is_none().then_some(name)
            })
            .flatten(),
        step: matches!(id, "edit.undo" | "edit.redo").then(|| app.session.undo_token(id == "edit.redo")).flatten(),
        moved: app.session.undo_steps_moved(),
    }
}

pub(crate) fn after_command(app: &mut MailApp, id: &str, p: &Value, change: Change) {
    app.signature_file_history.0.retain(|entry| entry.token.strong_count() > 0);
    if app.session.undo_steps_moved() == change.moved {
        return;
    }
    if let Some(step) = change.step {
        // Undo may change the identity of any signature, even if it has no file source.
        files::invalidate_signature(app, None);
        if let Some(entry) = app.signature_file_history.0.iter_mut().find(|entry| entry.token.ptr_eq(&step)) {
            match &mut entry.action {
                HistoryAction::Rename { from, to } => {
                    let source = app.ui.signature_files.remove(from);
                    app.ui.signature_files.remove(to);
                    if let Some(source) = source {
                        app.ui.signature_files.insert(to.clone(), source);
                    }
                    std::mem::swap(from, to);
                }
                HistoryAction::Remove { key, source } => {
                    let previous = app.ui.signature_files.remove(key);
                    if let Some(value) = source.take() {
                        app.ui.signature_files.insert(key.clone(), value);
                    }
                    *source = previous;
                }
            }
        }
    } else if let Some(old) = change.key {
        files::invalidate_signature(app, p.get("name").and_then(Value::as_str));
        if id == "signature.rename" && p.get("to").and_then(Value::as_str).and_then(|name| key_for_name(app, name)).as_ref() == Some(&old) {
            app.signature_file_preview = None;
            return;
        }
        let source = app.ui.signature_files.remove(&old);
        let action = if id == "signature.rename"
            && let Some(key) = p.get("to").and_then(Value::as_str).and_then(|name| key_for_name(app, name))
        {
            app.ui.signature_files.remove(&key);
            if let Some(source) = source {
                app.ui.signature_files.insert(key.clone(), source);
            }
            HistoryAction::Rename { from: key, to: old }
        } else {
            HistoryAction::Remove { key: old, source }
        };
        if let Some(token) = app.session.undo_token(false) {
            app.signature_file_history.0.push(HistoryEntry { token, action });
        }
    } else if let Some(key) = change.created.and_then(|name| key_for_name(app, &name)) {
        // Undoing creation must detach files chosen later, while Redo restores only this
        // signature's binding. A replacement with the same name starts without an alias.
        app.ui.signature_files.remove(&key);
        if let Some(token) = app.session.undo_token(false) {
            app.signature_file_history.0.push(HistoryEntry { token, action: HistoryAction::Remove { key, source: None } });
        }
    }
    app.signature_file_preview = None;
}

/// Scope a local source to the mailbox and signature. Accounts assign it using their usual selectors.
pub(crate) fn key_for_name(app: &MailApp, name: &str) -> Option<String> {
    let signature = app.session.mailbox.signature(name)?;
    let mailbox = app.session.path.as_deref().unwrap_or_else(|| Path::new("<sample>"));
    Some(format!("{}|signature|{}", mailbox.display(), signature.name.to_ascii_lowercase()))
}

#[cfg(test)]
pub(crate) fn setting(app: &MailApp, account: mailcraft_model::Id) -> Setting {
    app.session.mailbox.account(account).map(|a| setting_for_name(app, &a.signature)).unwrap_or_default()
}

pub(crate) fn setting_for_name(app: &MailApp, name: &str) -> Setting {
    if cfg!(target_arch = "wasm32") {
        return Setting::default();
    }
    key_for_name(app, name).and_then(|k| app.ui.signature_files.get(&k).cloned()).unwrap_or_default()
}

/// Cache the preview between frames, but refresh immediately after choosing a file or pressing Refresh.
pub(crate) fn preview(app: &mut MailApp, name: &str) -> Option<Result<Signature, String>> {
    let setting = setting_for_name(app, name);
    if !setting.enabled {
        return None;
    }
    let key = key_for_name(app, name)?;
    if !app.signature_file_preview.as_ref().is_some_and(|(k, path, _)| *k == key && *path == setting.path) {
        app.signature_file_preview = Some((key, setting.path.clone(), load(Path::new(&setting.path))));
    }
    app.signature_file_preview.as_ref().map(|(_, _, result)| result.clone())
}

/// Refresh the trusted loader before commands, including commands addressed to another compose.
/// The bytes are not cached: existing drafts keep their own snapshot of the inserted signature.
pub(crate) fn prepare(app: &mut MailApp) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let paths: HashMap<String, String> = app
            .session
            .mailbox
            .signatures
            .iter()
            .filter_map(|signature| {
                let s = setting_for_name(app, &signature.name);
                s.enabled.then_some((signature.name.to_ascii_lowercase(), s.path))
            })
            .collect();
        app.session.signature_file = Some(Box::new(move |name| {
            paths
                .get(&name.to_ascii_lowercase())
                .map(|path| {
                    load(Path::new(path)).map(|mut signature| {
                        signature.name = name.into();
                        signature
                    })
                })
                .transpose()
        }));
    }
    #[cfg(target_arch = "wasm32")]
    {
        app.session.signature_file = None;
    }
}

pub(crate) fn command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if !matches!(id, "ui.signatureFile" | "ui.chooseSignatureFile") {
        return None;
    }
    Some(configure(app, id, p))
}

fn configure(app: &mut MailApp, id: &str, p: &Value) -> Result<Value, String> {
    if cfg!(target_arch = "wasm32") || app.session.agent.is_some() {
        return Err(tl!("Signature files can only be configured by the owner in the desktop app.").into());
    }
    let name = p
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| p.get("account").and_then(Value::as_u64).and_then(|id| app.session.mailbox.account(id)).map(|a| a.signature.clone()))
        .ok_or_else(|| "missing `name`".to_string())?;
    let key = key_for_name(app, &name).ok_or_else(|| "no such signature".to_string())?;
    if id == "ui.chooseSignatureFile" {
        return files::ask(app, files::FileDialogRequest::Open { purpose: "signatureFile".into() }, files::AfterPick::SignatureFile { name, key });
    }
    let mut value = setting_for_name(app, &name);
    if let Some(enabled) = p.get("enabled") {
        value.enabled = enabled.as_bool().ok_or_else(|| "`enabled` is a boolean".to_string())?;
    }
    if let Some(path) = p.get("path") {
        let path = path.as_str().ok_or_else(|| "`path` is text".to_string())?;
        if path.len() > 4096 || path.contains('\0') || (!path.is_empty() && !Path::new(path).is_absolute()) {
            return Err(tl!("Choose an absolute local path for the signature file.").into());
        }
        value.path = path.to_string();
    }
    // A removed/undone signature can reclaim its slot through Undo/Redo. Reserve those slots
    // so restoring a step cannot exceed the same limit used when loading preferences.
    let reserved: std::collections::HashSet<&String> = app
        .signature_file_history
        .0
        .iter()
        .filter(|entry| entry.token.strong_count() > 0)
        .filter_map(|entry| match &entry.action {
            HistoryAction::Remove { key, source: Some(_) } if !app.ui.signature_files.contains_key(key) => Some(key),
            _ => None,
        })
        .collect();
    if app.ui.signature_files.len().saturating_add(reserved.len()) >= 200 && !app.ui.signature_files.contains_key(&key) && !reserved.contains(&key) {
        return Err("too many signature file settings".into());
    }
    app.ui.signature_files.insert(key, value.clone());
    app.signature_file_preview = None;
    prepare(app);
    Ok(json!({"name": name, "enabled": value.enabled, "path": value.path}))
}

/// A bounded regular file, decoded using the existing signature model and HTML reader.
pub(crate) fn load(path: &Path) -> Result<Signature, String> {
    let read = || -> Result<Signature, String> {
        let metadata = std::fs::metadata(path).map_err(|_| tl!("The signature file could not be read.").to_string())?;
        if !metadata.is_file() || metadata.len() > MAX_PICTURE_BYTES as u64 {
            return Err(tl!("The signature file must be a regular file of at most 256 KB.").into());
        }
        let file = std::fs::File::open(path).map_err(|_| tl!("The signature file could not be read.").to_string())?;
        if !file.metadata().is_ok_and(|m| m.is_file()) {
            return Err(tl!("The signature file could not be read.").into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_PICTURE_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|_| tl!("The signature file could not be read.").to_string())?;
        if bytes.len() > MAX_PICTURE_BYTES {
            return Err(tl!("The signature file is too large.").into());
        }
        let mut signature = Signature::plain_text(tl!("File signature"), "");
        if mailcraft_model::signature::picture_mime(&bytes).is_some() {
            if crate::reading::images::decode(&bytes, 600, 600).is_none() {
                return Err(tl!("The signature image could not be decoded.").into());
            }
            signature.add_picture("signature", bytes, Some(tl!("Signature")), None).map_err(|e| e.to_string())?;
            return Ok(signature);
        }
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        if !matches!(extension.as_str(), "txt" | "text" | "html" | "htm") {
            return Err(tl!("Choose a text, HTML, PNG, JPEG or GIF signature file.").into());
        }
        let text =
            std::str::from_utf8(&bytes).map_err(|_| tl!("The signature file must contain UTF-8 text.").to_string())?.trim_start_matches('\u{feff}');
        if matches!(extension.as_str(), "htm" | "html") {
            if text.len() > MAX_HTML {
                return Err(tl!("An HTML signature file is at most 64 KB.").into());
            }
            signature.set_body(mailcraft_engine::mime::rich::from_html(text)).map_err(|e| e.to_string())?;
            signature.set_html(Some(text.to_string())).map_err(|e| e.to_string())?;
        } else {
            if text.chars().count() > MAX_TEXT {
                return Err(tl!("A text signature file is at most 10,000 characters.").into());
            }
            signature = Signature::plain_text(tl!("File signature"), text);
        }
        Ok(signature)
    };
    read().map_err(|reason| crate::i18n::fmt(tl!("{reason} Using the account's usual signature instead."), &[("reason", &reason)]))
}

#[cfg(test)]
mod tests;
