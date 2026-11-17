// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MB3.17: an error answer's body is not a message too large for the device. Nothing here leaves
//! this machine; every message is invented.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 2, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;

#[test]
fn a_busy_answer_with_a_body_longer_than_the_devices_room_is_busy_not_a_message_too_large() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, 2, NOW);
    {
        let mut state = server.state();
        state.retry_after = Some(0);
        for _ in 0..10 {
            state.fail_next.push(("GET".into(), 503, "TooManyRequests".into()));
        }
    }
    let mut request = mailcraft_model::sync::GraphFetchRequest {
        account: 3,
        email: "owner@example.org".into(),
        ids: ids.clone(),
        background: true,
        ..Default::default()
    };
    // Graph's error JSON is longer than this room.
    request.most_bytes = 128;
    let got = super::fetch::fetch_messages_with(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    assert!(got.too_large.is_empty(), "{got:?}");
    assert!(got.failed.is_empty(), "{got:?}");
    assert!(got.fetched.is_empty());
}
