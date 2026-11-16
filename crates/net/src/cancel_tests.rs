// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Stopping a download: a signal given before, or while the server says nothing, or while it
//! drips, ends it at once with `Cancelled`, and a signal never given changes nothing.
//!
//! Against a server on this machine that stands in for the internet; the hosts are invented.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::{Cancel, Limits, PictureError, Reach, fetch_with_cancel};

fn limits() -> Limits {
    Limits { patience: Duration::from_secs(30), connect: Duration::from_secs(3), ..Limits::default() }
}

fn names(_: &str, _: u16, _: Duration, _: &Cancel) -> Vec<SocketAddr> {
    Vec::new()
}

/// A server that runs `behave` on the first connection it gets.
fn serving(behave: impl FnOnce(std::net::TcpStream) + Send + 'static) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let at = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((peer, _)) = listener.accept() {
            behave(peer);
        }
    });
    at
}

#[test]
fn a_signal_given_before_asks_nobody() {
    let (seen, asked) = mpsc::channel::<()>();
    let at = serving(move |_| {
        let _ = seen.send(());
    });
    let cancel = Cancel::new();
    cancel.cancel();
    let reach = Reach { lookup: &names, stand_ins: &[at], dial: None };
    assert_eq!(fetch_with_cancel(&format!("http://{at}/a.png"), &reach, &limits(), &cancel), Err(PictureError::Cancelled));
    assert!(asked.recv_timeout(Duration::from_millis(300)).is_err(), "nothing connected");
}

#[test]
fn a_signal_ends_a_wait_for_a_server_that_says_nothing() {
    let (hold, held) = mpsc::channel::<()>();
    let at = serving(move |mut peer| {
        let mut sink = [0u8; 512];
        let _ = peer.read(&mut sink);
        let _ = held.recv();
    });
    let cancel = Cancel::new();
    let signal = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        signal.cancel();
    });
    let reach = Reach { lookup: &names, stand_ins: &[at], dial: None };
    let started = Instant::now();
    let result = fetch_with_cancel(&format!("http://{at}/a.png"), &reach, &limits(), &cancel);
    assert_eq!(result, Err(PictureError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    drop(hold);
}

#[test]
fn a_signal_ends_a_picture_that_drips() {
    let at = serving(|mut peer| {
        let _ = peer.write_all(b"HTTP/1.1 200 OK\r\n\r\n");
        while peer.write_all(b"x").is_ok() {
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let cancel = Cancel::new();
    let signal = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        signal.cancel();
    });
    let reach = Reach { lookup: &names, stand_ins: &[at], dial: None };
    let started = Instant::now();
    let result = fetch_with_cancel(&format!("http://{at}/a.png"), &reach, &limits(), &cancel);
    assert_eq!(result, Err(PictureError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
}

#[test]
fn a_signal_never_given_changes_nothing() {
    let at = serving(|mut peer| {
        let mut sink = [0u8; 512];
        let _ = peer.read(&mut sink);
        let _ = peer.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc");
    });
    let reach = Reach { lookup: &names, stand_ins: &[at], dial: None };
    let got = fetch_with_cancel(&format!("http://{at}/a.png"), &reach, &limits(), &Cancel::new());
    assert_eq!(got, Ok(b"abc".to_vec()));
}

#[test]
fn a_signal_ends_a_name_lookup_that_hangs() {
    let stuck = |_: &str, _: u16, patience: Duration, cancel: &Cancel| {
        super::within(patience, cancel, || std::thread::sleep(Duration::from_secs(5))).map(|()| Vec::new()).unwrap_or_default()
    };
    let cancel = Cancel::new();
    let signal = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        signal.cancel();
    });
    let reach = Reach { lookup: &stuck, stand_ins: &[], dial: None };
    let started = Instant::now();
    let result = fetch_with_cancel("https://stuck.example/a.png", &reach, &limits(), &cancel);
    assert_eq!(result, Err(PictureError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
}
