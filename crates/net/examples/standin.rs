// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Graph stand-in as a server on this computer's loopback address, for host tests (a phone
//! or an emulator reaching it through a port forward, a script, `curl`).
//!
//!     cargo run -p mailcraft-net --features stand-in --example standin -- --port 8780
//!
//! With `--e2e` it holds what the phone's end-to-end run (MB3.15) reads: the same data dated now
//! (a phone keeps only the last month), and one message the run opens ([`E2E_SUBJECT`]). Its token
//! endpoint (`/{tenant}/oauth2/v2.0/token`) signs the run's app in as the owner.
//!
//! It listens on 127.0.0.1 only (there is no way to ask for another address), takes the bearer
//! token it prints, and holds synthetic `example.org` data: an Inbox with mail, a folder, a
//! calendar with an event, a To Do list with tasks and a few colour categories. Nothing leaves
//! this machine. Stop it with Ctrl-C.

use mailcraft_net::graph::standin::{StandIn, TOKEN};

/// A moment in the stand-in's own clock (2026-09-21), so that what it holds is the same every run.
const NOW: i64 = 1_790_000_000;
/// The message the end-to-end run opens and answers, and the words its body holds.
const E2E_SUBJECT: &str = "Quarterly figures for review";
const E2E_BODY: &str = "The quarterly figures are ready for your review before Friday.";

fn main() -> Result<(), String> {
    let mut port: u16 = 0;
    let mut e2e = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                let value = args.next().ok_or("--port takes a number")?;
                port = value.parse().map_err(|_| format!("'{value}' is not a port"))?;
            }
            "--e2e" => e2e = true,
            "-h" | "--help" => {
                println!("usage: standin [--port N] [--e2e]   (listens on 127.0.0.1 only; 0 picks a port)");
                return Ok(());
            }
            other => return Err(format!("unknown argument '{other}' (try --help)")),
        }
    }
    let standin = StandIn::start_on(port).map_err(|e| format!("can't listen on 127.0.0.1:{port}: {e}"))?;
    let clock = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().and_then(|d| i64::try_from(d.as_secs()).ok());
    let now = if e2e { clock.unwrap_or(NOW) } else { NOW };
    seed(&standin, now);
    if e2e {
        let mut state = standin.state();
        let inbox = state.well_known("inbox");
        let id = state.mailbox.add(&inbox, E2E_SUBJECT, now);
        state.mailbox.update(&id, |m| {
            m.from = "ada@example.org".into();
            m.preview = E2E_BODY.into();
        });
    }
    println!("Graph stand-in at {}/v1.0 (loopback only)", standin.url());
    println!("Authorization: Bearer {TOKEN}");
    println!("owner: owner@example.org; Ctrl-C to stop");
    // With `--e2e`, each request is written as it comes (its method, its path and its answer's
    // status; never a header or a body), for the run's log.
    let mut seen = 0;
    loop {
        if e2e {
            let state = standin.state();
            for asked in state.requests.iter().skip(seen) {
                let path = asked.target.split('?').next().unwrap_or_default();
                println!("{} {path} -> {}", asked.method, asked.status);
            }
            seen = state.requests.len();
            drop(state);
            std::thread::sleep(std::time::Duration::from_millis(500));
        } else {
            std::thread::park();
        }
    }
}

/// Synthetic data to have something to read and change.
fn seed(standin: &StandIn, now: i64) {
    let mut state = standin.state();
    let inbox = state.well_known("inbox");
    // (A minute before `now` at the newest: the end-to-end run's message is the newest of all.)
    state.mailbox.generate(&inbox, 25, now - 60);
    let projects = state.add_folder("Projects", None);
    state.mailbox.generate(&projects, 5, now - 86_400);
    let calendar = state.main_calendar();
    state.add_event(&calendar, "Planning meeting", now + 86_400);
    state.add_category("Work", "preset0");
    state.add_category("Personal", "preset4");
    let list = state.default_todo_list();
    state.add_todo(&list, "Reply to the quarterly report");
    state.add_todo(&list, "Book the meeting room");
}
