// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signing in on the provider's page through the binding: the address the system hands the app is
//! hostile, and nothing of it is echoed. No provider is asked: the page is a fake platform's, and
//! every address given back here is refused before a code could be traded.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_engine::Session;
use mailcraft_host::sync::SendReceive;
use mailcraft_net::NetError;
use serde_json::json;

use crate::fake_tests::{FakePlatform, block_on};
use crate::sign_in::Redirects;
use crate::{Core, CoreError};

const BACK: &str = "org.example.mailcraft:/oauth2redirect";

fn session() -> Session {
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap();
    s.prefs.sync_minutes = 0;
    // (The build may carry no application id; the page needs one.)
    if let Some(a) = s.mailbox.accounts.first_mut() {
        a.oauth.get_or_insert_with(Default::default).client_id = "test-application".into();
    }
    s
}

fn core(platform: Arc<FakePlatform>) -> Arc<Core> {
    Core::start_with(platform, session, SendReceive::new()).unwrap()
}

fn account(core: &Core) -> u64 {
    block_on(core.call(|s| Ok(s.mailbox.accounts.first().map(|a| a.id).unwrap_or(0)))).unwrap()
}

/// Run a sign-in on a thread of its own; its end comes back through the handle.
fn sign_in_on_thread(core: &Arc<Core>, account: u64) -> std::thread::JoinHandle<Result<(), CoreError>> {
    let core = core.clone();
    std::thread::spawn(move || block_on(core.sign_in(account)))
}

fn opened_page(platform: &FakePlatform) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(url) = platform.opened.lock().unwrap().first() {
            return url.clone();
        }
        assert!(Instant::now() < deadline, "the page was never opened");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_page_is_opened_for_the_providers_sign_in_and_comes_back_to_the_apps_scheme() {
    let platform = FakePlatform::new();
    let core = core(platform.clone());
    let id = account(&core);
    let handle = sign_in_on_thread(&core, id);
    let url = opened_page(&platform);
    assert!(url.starts_with("https://login.microsoftonline.com/"), "{url}");
    assert!(url.contains("redirect_uri=org.example.mailcraft%3A%2Foauth2redirect") || url.contains("redirect_uri=org.example.mailcraft"), "{url}");
    assert!(url.contains("client_id=test-application"));
    // Another program's link, or a page for another sign-in, ends it without trading anything.
    assert!(core.sign_in_redirect(format!("{BACK}?state=somebody-elses&code=abc")));
    match handle.join().unwrap() {
        Err(CoreError::Failed { reason }) => assert!(!reason.contains("abc") && !reason.contains("somebody-elses"), "{reason}"),
        other => panic!("{other:?}"),
    }
    // Once it has ended nothing is taken.
    assert!(!core.sign_in_redirect(format!("{BACK}?state=x")));
    // …and another can begin.
    let again = sign_in_on_thread(&core, id);
    let deadline = Instant::now() + Duration::from_secs(10);
    while platform.opened.lock().unwrap().len() < 2 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    core.shutdown();
    assert!(again.join().unwrap().is_err());
}

#[test]
fn only_one_sign_in_is_open_at_a_time_and_dropping_it_calls_it_off() {
    let platform = FakePlatform::new();
    let core = core(platform.clone());
    let id = account(&core);
    let first = sign_in_on_thread(&core, id);
    opened_page(&platform);
    assert!(matches!(block_on(core.sign_in(id)), Err(CoreError::Failed { reason }) if reason.contains("already")));
    core.shutdown();
    assert!(first.join().unwrap().is_err());
}

#[test]
fn a_sign_in_for_an_account_that_is_not_microsoft_or_not_there_is_refused() {
    let platform = FakePlatform::new();
    let core = Core::start_with(
        platform.clone(),
        || {
            let mut s = Session::empty();
            s.run(
                "account.add",
                &json!({"email": "ada@example.org", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}}),
            )
            .unwrap();
            s
        },
        SendReceive::new(),
    )
    .unwrap();
    let id = account(&core);
    assert!(matches!(block_on(core.sign_in(id)), Err(CoreError::Failed { .. })));
    assert!(matches!(block_on(core.sign_in(0)), Err(CoreError::Failed { .. })));
    assert!(platform.opened.lock().unwrap().is_empty());
}

#[test]
fn the_redirect_slot_takes_the_first_address_only_while_armed() {
    let slot = Redirects::default();
    let cancel = AtomicBool::new(false);
    assert!(!slot.deliver("a"), "nobody waits");
    let attempt = slot.arm();
    assert!(slot.deliver("first"));
    assert!(!slot.deliver("second"), "only the first");
    assert_eq!(slot.wait(&cancel, Duration::from_secs(1)).unwrap(), "first");
    assert!(!slot.deliver(&"x".repeat(9 * 1024)), "too long");
    assert!(matches!(slot.wait(&cancel, Duration::from_millis(30)), Err(NetError::SignInTimedOut)));
    cancel.store(true, Ordering::Release);
    assert!(matches!(slot.wait(&cancel, Duration::from_secs(5)), Err(NetError::SignInCancelled)));
    slot.disarm(attempt);
    assert!(!slot.deliver("late"));
    // A patience that would overflow the clock is not a panic.
    assert!(matches!(slot.wait(&cancel, Duration::MAX), Err(NetError::SignInCancelled)));
}

#[test]
fn an_earlier_attempt_cannot_disarm_a_later_one() {
    let slot = Redirects::default();
    let first = slot.arm();
    let second = slot.arm();
    assert_ne!(first, second);
    // The first sign-in's worker ends late: the second one still listens.
    slot.disarm(first);
    assert!(slot.deliver("for-the-second"));
    slot.disarm(second);
    assert!(!slot.deliver("late"));
}
