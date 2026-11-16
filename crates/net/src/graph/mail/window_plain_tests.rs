// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A phone's window (MB3.16): the plain list that follows a filtered round cut at Graph's limit is
//! read as a listing afresh, so that what the server deleted meanwhile is not left here. Through the
//! strict Graph stand-in; nothing leaves this machine and every message is invented.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::sync::{GraphMailRequest, GraphMailResult};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;
const DAY: i64 = 86_400;

fn run(server: &StandIn) -> Vec<GraphMailResult> {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), since: Some(NOW - 30 * DAY), ..Default::default() };
    whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false))
}

#[test]
fn the_plain_list_after_a_cut_round_is_a_listing_afresh_that_ends_with_the_held_link() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 5_150, NOW);
    let pieces = run(&server);
    assert!(pieces.last().unwrap().whole, "{:?}", pieces.last().unwrap().errors);
    let pages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).filter(|p| p.folder == inbox).collect();
    let afresh = pages.iter().filter(|p| p.afresh).count();
    assert_eq!(afresh, 1, "one page begins the listing: it marks what is stored gone, the rest confirm");
    let first_afresh = pages.iter().position(|p| p.afresh).unwrap();
    let cut = pages.iter().position(|p| p.link.is_empty() && !p.caught_up).unwrap();
    assert!(first_afresh > cut, "after the cut round's last page, never before it");
    let last = pages.last().unwrap();
    assert!(last.caught_up && last.link.contains("deltatoken"), "the held delta link is given at the list's end alone");
    assert!(pages.iter().take(pages.len() - 1).all(|p| !p.link.contains("deltatoken")));
}
