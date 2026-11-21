// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a draft of a Microsoft account answers goes up with it while the server hasn't got it, so
//! that one made by itself is written with its `In-Reply-To` and `References` (#1814); once the
//! server has it, nothing of them is said again. Every id is invented.

use mailcraft_model::Message;
use mailcraft_model::sync::{ServerDraft, ServerMessage};

use super::graph_drafts::{draft_up, draft_up_whole};

fn answer() -> Message {
    Message {
        id: 7,
        draft: true,
        in_reply_to: "agenda-2@example.org".into(),
        references: vec!["agenda-1@example.org".into(), "agenda-2@example.org".into()],
        server_draft: Some(ServerDraft { marker: "m".into(), ..Default::default() }),
        ..Default::default()
    }
}

#[test]
fn a_draft_the_server_hasnt_got_says_what_it_answers() {
    let m = answer();
    for up in [draft_up(&m).unwrap(), draft_up_whole(&m).unwrap()] {
        assert!(up.id.is_empty());
        assert_eq!(up.in_reply_to, "agenda-2@example.org");
        assert_eq!(up.references, ["agenda-1@example.org", "agenda-2@example.org"]);
    }
}

#[test]
fn a_draft_the_server_has_says_nothing_of_it_again() {
    let m = Message { server: Some(ServerMessage { id: "AAkA-draft".into(), ..Default::default() }), ..answer() };
    for up in [draft_up(&m).unwrap(), draft_up_whole(&m).unwrap()] {
        assert_eq!(up.id, "AAkA-draft");
        assert!(up.in_reply_to.is_empty() && up.references.is_empty());
    }
}
