// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A .pst file, read through every call: the store, the named properties, every node, the
//! folder tree, every folder's items, every message with its recipients and attachments, and
//! the messages attached to it; and the same bytes as a distribution list's stream of members
//! ([MS-OXOCNTC] 2.2.2.2.4), which the import reads out of a list. The header's CRCs are put
//! right first, so that a change to the header reaches what is behind it; the seeds in
//! `fuzz/seeds/pst/` (made by `cargo test -p mailcraft-pst -- --ignored write_fuzz_seeds`) give
//! the rest its shape.
#![no_main]

use std::io::{Cursor, Read};

use mailcraft_pst::{AttachmentContent, Limits, MessageRef, Pst, crc32};

/// Puts dwCRCPartial (and dwCRCFull of a Unicode file) right for the header's bytes.
fn fix_header(b: &mut [u8]) {
    let (Some(partial), Some(full)) = (b.get(8..479).map(crc32), b.get(8..524).map(crc32)) else { return };
    if let Some(at) = b.get_mut(4..8) {
        at.copy_from_slice(&partial.to_le_bytes());
    }
    let unicode = b.get(10..12).is_some_and(|v| u16::from_le_bytes([v[0], v[1]]) >= 23);
    if let (true, Some(at)) = (unicode, b.get_mut(524..528)) {
        at.copy_from_slice(&full.to_le_bytes());
    }
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = mailcraft_pst::list_stream(data, 1000);
    let _ = mailcraft_pst::appointment_recurrence(data);
    let _ = mailcraft_pst::time_zone_key(data);
    let mut data = data.to_vec();
    // ponytail: only the header's CRCs are put right; pages and blocks keep theirs, since the proptests already mutate with checksums put right. Add a repair mode if coverage stalls behind them.
    fix_header(&mut data);
    // Small caps, so that one input stays quick.
    let limits = Limits {
        max_item_bytes: 1 << 20,
        max_attachment_bytes: 1 << 20,
        max_inline_value: 1 << 16,
        max_listing_bytes: 4 << 20,
        read_floor: 16 << 20,
        ..Limits::default()
    };
    let Ok(mut pst) = Pst::open_with(Cursor::new(&data[..]), limits) else { return };
    // The password of the seed that has one (`write_fuzz_seeds`).
    let _ = pst.unlock("seed");
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
