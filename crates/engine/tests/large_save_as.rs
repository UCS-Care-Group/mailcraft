// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Save As must not retain the content of every message before it starts writing.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use mailcraft_engine::{Session, io::ContentSource};
use mailcraft_model::{Attachment, ContentRef, Mailbox, Message};
use serde_json::json;

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        // Use a fresh directory; never remove a previous run's files to make room.
        for attempt in 0..100 {
            let dir = std::env::temp_dir().join(format!("mailcraft-large-save-as-{}-{attempt}", std::process::id()));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Self(dir),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("cannot create synthetic mailbox directory: {e}"),
            }
        }
        panic!("cannot find an unused synthetic mailbox directory");
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("copy.mailcraft"));
        let _ = std::fs::remove_dir(&self.0);
    }
}

#[derive(Default)]
struct Content {
    previous: Mutex<Weak<Vec<u8>>>,
}

impl ContentSource for Content {
    fn fetch(&self, _: &Path, r: &ContentRef) -> Result<Arc<Vec<u8>>, String> {
        let mut previous = self.previous.lock().unwrap();
        if previous.upgrade().is_some() {
            return Err("the export retained the previous message's attachment".into());
        }
        let bytes = Arc::new(vec![r.sha256[0]; r.len as usize]);
        *previous = Arc::downgrade(&bytes);
        Ok(bytes)
    }
}

#[test]
fn forty_thousand_messages_are_exported_without_retaining_their_content() {
    let dir = Temp::new();
    let target = dir.0.join("copy.mailcraft");
    let source = dir.0.join("source.mailcraft");
    let mut mb = Mailbox::with_local_account("Synthetic", "synthetic@example.org");
    let folder = mb.inbox().unwrap();
    for i in 0..40_000 {
        mb.add_message(Message {
            folder,
            subject: format!("Synthetic {i}"),
            attachments: vec![Attachment {
                name: "synthetic.bin".into(),
                kept: Some(ContentRef { sha256: [i as u8; 32], len: 16 }),
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
    }
    let mut s = Session::new(mb);
    s.path = Some(source.clone());
    s.content_source = Some(Arc::new(Content::default()));
    let result = s.run("file.saveAs", &json!({"path": target}));
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(result.unwrap()["copy"], true);
    assert_eq!(s.path.as_ref(), Some(&source));
    assert!(s.mailbox.messages.values().all(|m| m.attachments[0].data.is_empty()));
    let back = mailcraft_engine::io::open_path(&target).unwrap();
    assert_eq!(back.messages.len(), 40_000);
    for (i, m) in back.messages.values().enumerate() {
        assert_eq!(m.subject, format!("Synthetic {i}"));
        assert_eq!(*m.attachments[0].data, vec![i as u8; 16]);
    }
}
