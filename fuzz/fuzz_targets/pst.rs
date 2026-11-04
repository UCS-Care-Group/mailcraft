// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A .pst file, read through every call: the store, the named properties, every node, the
//! folder tree, every folder's items, every message with its recipients and attachments, and
//! the messages attached to it.
#![no_main]

use std::io::{Cursor, Read};

use mailcraft_pst::{AttachmentContent, Limits, MessageRef, Pst};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    // Small caps, so that one input stays quick.
    let limits = Limits { max_item_bytes: 1 << 20, max_attachment_bytes: 1 << 20, max_inline_value: 1 << 16, ..Limits::default() };
    let Ok(mut pst) = Pst::open_with(Cursor::new(data), limits) else { return };
    let _ = pst.unlock("");
    let _ = pst.store();
    let _ = pst.named_properties();
    let mut nodes = 0usize;
    let _ = pst.for_each_node(|_| {
        nodes += 1;
        nodes < 100_000
    });
    let Ok(tree) = pst.walk_folders() else { return };
    for entry in &tree.folders {
        let _ = pst.associated_items(entry.folder.id);
        let Ok(items) = pst.items(entry.folder.id) else { continue };
        for item in items.items {
            let mut queue = vec![MessageRef::item(item.id)];
            while let Some(m) = queue.pop() {
                let Ok(message) = pst.message(&m) else { continue };
                let _ = pst.read_property(&m, 0x1000, 1 << 20);
                for a in message.attachments {
                    match &a.content {
                        AttachmentContent::Message(inner) if queue.len() < 64 => queue.push(inner.clone()),
                        AttachmentContent::Data { .. } | AttachmentContent::Object { .. } => {
                            if let Ok(mut r) = pst.attachment_reader(&a.id) {
                                let _ = r.read_to_end(&mut Vec::new());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
});
