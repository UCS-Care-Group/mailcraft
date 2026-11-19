// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bringing attachments kept on disk into memory by a choice (`Session::load_where`, #834): the
//! choice is asked once about each attachment that is not in memory, in order, so that a choice
//! that counts (one that stops after so many) picks the first of them. Synthetic files by the
//! reader's test writer.

use std::cell::Cell;

use crate::import_pst_memory_tests::{Temp, file_of, import, kept_session, mail, noise};

#[test]
fn a_choice_that_stops_after_so_many_brings_in_the_first_of_them_only() {
    let dir = Temp::new("load-where-count");
    let mut s = kept_session(&dir);
    // One message naming one content thirty times: a mailbox file or a .pst stores it once.
    let same = noise(7, 5000);
    let pst = dir.pst(&file_of(vec![mail("Copies", (0..30).map(|i| (format!("copy{i}.bin"), same.clone())).collect())]));
    import(&mut s, &pst).unwrap();
    let id = *s.mailbox.messages.keys().next().unwrap();
    let kept = s.mailbox.message(id).unwrap().attachments.iter().filter(|a| a.kept.is_some() && a.data.is_empty()).count();
    assert_eq!(kept, 30, "all of them are on disk");
    let left = Cell::new(3);
    let asked = Cell::new(0);
    let brought = s
        .load_where(id, |a| {
            asked.set(asked.get() + 1);
            let it = left.get() > 0 && a.kept.is_some();
            if it {
                left.set(left.get() - 1);
            }
            it
        })
        .unwrap();
    assert!(brought);
    // Asked once about each of the thirty, none twice.
    assert_eq!(asked.get(), 30);
    let m = s.mailbox.message(id).unwrap();
    let in_memory: Vec<usize> = m.attachments.iter().enumerate().filter(|(_, a)| !a.data.is_empty()).map(|(i, _)| i).collect();
    assert_eq!(in_memory, [0, 1, 2]);
    // What was brought in no longer says it is kept; the others still do.
    assert!(m.attachments.iter().take(3).all(|a| a.kept.is_none() && a.data.as_slice() == same.as_slice()));
    assert!(m.attachments.iter().skip(3).all(|a| a.kept.is_some() && a.data.is_empty()));
    // Nothing chosen, nothing brought in, and the message is left alone.
    assert!(!s.load_where(id, |_| false).unwrap());
}
