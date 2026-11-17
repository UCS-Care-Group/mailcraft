// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Save As must not retain the content of every message before it starts writing.

use std::path::Path;
use std::sync::{Arc, Mutex, Weak};

use mailcraft_engine::{Session, io::ContentSource};
use mailcraft_model::{Attachment, ContentRef, Mailbox, Message};
use serde_json::json;

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
    let dir = std::env::temp_dir().join(format!("mailcraft-large-save-as-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("copy.mailcraft");
    let source = dir.join("source.mailcraft");
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
    std::fs::remove_file(&target).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}
