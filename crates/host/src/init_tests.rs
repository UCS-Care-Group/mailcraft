// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::Arc;
use std::time::Duration;

use crate::{InitError, NetworkState, Platform, PlatformError, Redirect, init, memory_budget};

struct Fake(Option<u64>);

impl Platform for Fake {
    fn memory_budget(&self) -> Option<u64> {
        self.0
    }
}

#[test]
fn the_budget_is_the_lower_of_the_device_and_the_engine() {
    let engine = memory_budget(None);
    assert!(engine > 1 << 20);
    assert_eq!(engine, mailcraft_engine::store::body_budget());
    assert_eq!(memory_budget(Some(1 << 20)), 1 << 20);
    assert_eq!(memory_budget(Some(u64::MAX)), engine);
}

#[test]
fn init_takes_the_budget_and_refuses_a_platform_with_no_memory() {
    let host = init(Arc::new(Fake(Some(1 << 20)))).unwrap();
    assert_eq!(host.memory_budget(), 1 << 20);
    assert_eq!(host.platform().network(), NetworkState::Unknown);
    assert_eq!(init(Arc::new(Fake(Some(0)))).err(), Some(InitError::NoMemory));
}

#[test]
fn what_a_platform_leaves_out_says_so() {
    let p = Fake(None);
    assert!(p.secrets().is_none());
    assert_eq!(p.redirect(), Redirect::Loopback);
    assert_eq!(p.open_sign_in_page("https://login.example.org/"), Err(PlatformError::Unsupported));
    assert_eq!(p.open_file("a.txt", b"x"), Err(PlatformError::Unsupported));
}

#[test]
fn connect_defaults_to_plain_sockets_and_fails_typed() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    assert!(Fake(None).connect("127.0.0.1", port, Duration::from_secs(2)).is_ok());
    drop(listener);
    assert!(matches!(Fake(None).connect("127.0.0.1", port, Duration::from_secs(2)), Err(PlatformError::Io(_))));
    assert!(matches!(Fake(None).connect("", 0, Duration::from_millis(10)), Err(PlatformError::Io(_))));
}

#[test]
fn a_silent_peer_times_a_read_out_instead_of_hanging() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut stream = Fake(None).connect("127.0.0.1", port, Duration::from_millis(200)).unwrap();
    let _held = listener.accept().unwrap(); // accepted, never writes
    let started = std::time::Instant::now();
    let mut byte = [0u8; 1];
    assert!(std::io::Read::read(&mut stream, &mut byte).is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
}
