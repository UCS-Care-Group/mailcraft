// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Graph stand-in as a server on this computer's loopback address, for host tests (a phone
//! or an emulator reaching it through a port forward, a script, `curl`).
//!
//!     cargo run -p mailcraft-net --features stand-in --example standin -- --port 8780
//!
//! It listens on 127.0.0.1 only (there is no way to ask for another address), takes the bearer
//! token it prints, and holds synthetic `example.org` data: an Inbox with mail, a folder, a
//! calendar with an event, a To Do list with tasks and a few colour categories. Nothing leaves
//! this machine. Stop it with Ctrl-C.

use mailcraft_net::graph::standin::{StandIn, TOKEN};

/// A moment in the stand-in's own clock (2026-09-21), so that what it holds is the same every run.
const NOW: i64 = 1_790_000_000;

fn main() -> Result<(), String> {
    let mut port: u16 = 0;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                let value = args.next().ok_or("--port takes a number")?;
                port = value.parse().map_err(|_| format!("'{value}' is not a port"))?;
            }
            "-h" | "--help" => {
                println!("usage: standin [--port N]   (listens on 127.0.0.1 only; 0 picks a port)");
                return Ok(());
            }
            other => return Err(format!("unknown argument '{other}' (try --help)")),
        }
    }
    let standin = StandIn::start_on(port).map_err(|e| format!("can't listen on 127.0.0.1:{port}: {e}"))?;
    seed(&standin);
    println!("Graph stand-in at {}/v1.0 (loopback only)", standin.url());
    println!("Authorization: Bearer {TOKEN}");
    println!("owner: owner@example.org; Ctrl-C to stop");
    loop {
        std::thread::park();
    }
}

/// Synthetic data to have something to read and change.
fn seed(standin: &StandIn) {
    let mut state = standin.state();
    let inbox = state.well_known("inbox");
    state.mailbox.generate(&inbox, 25, NOW);
    let projects = state.add_folder("Projects", None);
    state.mailbox.generate(&projects, 5, NOW - 86_400);
    let calendar = state.main_calendar();
    state.add_event(&calendar, "Planning meeting", NOW + 86_400);
    state.add_category("Work", "preset0");
    state.add_category("Personal", "preset4");
    let list = state.default_todo_list();
    state.add_todo(&list, "Reply to the quarterly report");
    state.add_todo(&list, "Book the meeting room");
}
