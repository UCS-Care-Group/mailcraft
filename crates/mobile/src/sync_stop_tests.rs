// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A round called off (MB3.16): what it brought is kept within the phone's limits at once, not at
//! the next hourly check. A core over a fake platform; nothing here leaves this machine.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_host::graph::{GraphRequest, GraphResult};
use mailcraft_model::sync::ServerMessage;
use mailcraft_model::{FolderKind, Message, ServerFolder};

use crate::fake_tests::block_on;
use crate::sync_tests::{Runner, signed_in, stand_in};

#[test]
fn a_round_called_off_has_the_mailbox_kept_within_the_limits() {
    let hang = Arc::new(AtomicBool::new(false));
    let (quick, _) = stand_in();
    let hanging = hang.clone();
    let runner: Arc<Runner> = Arc::new(move |req: GraphRequest| {
        if !hanging.load(Ordering::SeqCst) {
            return quick(req);
        }
        let until = Instant::now() + Duration::from_secs(30);
        while !req.cancel.load(Ordering::Relaxed) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        GraphResult { account: req.account.id, ..Default::default() }
    });
    let (core, _, account) = signed_in(runner);
    // A phone with a month's window; the first round ends and the trimming it asked for is done.
    block_on(core.call(move |s| {
        s.run("account.edit", &serde_json::json!({"id": account, "keepOffline": "1m"}))?;
        s.run("account.deviceLimits", &serde_json::json!({"storage": mailcraft_model::PHONE_STORAGE}))?;
        Ok(())
    }))
    .unwrap();
    assert!(block_on(core.background_sync(10_000)).unwrap().finished);
    core.enqueue_tick();
    std::thread::sleep(Duration::from_millis(300));
    // Mail from long ago that the server has, arrives with the next round's first pages; the
    // round is then called off.
    let old = block_on(core.call(move |s| {
        let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).ok_or(crate::CoreError::Internal)?;
        if let Ok(f) = s.mailbox.folder_mut(inbox) {
            f.server = Some(ServerFolder { id: "in".into(), ..Default::default() });
        }
        let server = ServerMessage { id: "OLD".into(), folder: "in".into(), ..Default::default() };
        let m = Message { account, folder: inbox, received: s.now() - 400 * 86_400, server: Some(server), ..Default::default() };
        Ok(s.mailbox.add_message(m).unwrap())
    }))
    .unwrap();
    hang.store(true, Ordering::SeqCst);
    assert!(block_on(core.sync_now()).unwrap().started);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !block_on(core.sync_status()).unwrap().running {
        assert!(Instant::now() < deadline, "the round never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    let sync = core.sync.clone();
    block_on(core.call(move |_| {
        sync.stop_later(sync.round());
        Ok(())
    }))
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        core.enqueue_tick();
        let there = block_on(core.call(move |s| Ok(s.mailbox.message(old).is_some()))).unwrap();
        if !there {
            break;
        }
        assert!(Instant::now() < deadline, "the mailbox was not kept within the limits after the stop");
        std::thread::sleep(Duration::from_millis(20));
    }
}
