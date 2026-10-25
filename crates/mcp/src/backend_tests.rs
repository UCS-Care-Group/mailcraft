// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The bridge to a running window: what it sends, to whom, and what it refuses.

use super::*;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};

#[test]
fn key_from_env_then_key_file() {
    let dir = std::env::temp_dir().join(format!("mailcraft-mcp-key-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = mailcraft_control_key::key_path(&dir, 7981);
    // Neither: the error names the variable and the file it looked for.
    let e = lookup_key(None, Some(&dir), Some(7981)).unwrap_err();
    assert!(e.contains("MAILCRAFT_CONTROL_KEY") && e.contains(&path.display().to_string()), "{e}");
    // The key file, trimmed; an empty variable does not count.
    std::fs::write(&path, "  file-key\n").unwrap();
    assert_eq!(lookup_key(None, Some(&dir), Some(7981)).unwrap(), "file-key");
    assert_eq!(lookup_key(Some(" ".into()), Some(&dir), Some(7981)).unwrap(), "file-key");
    // The variable wins.
    assert_eq!(lookup_key(Some(" env-key ".into()), Some(&dir), Some(7981)).unwrap(), "env-key");
    assert_eq!(lookup_key(Some("env-key".into()), None, None).unwrap(), "env-key");
    // Read again every time: the app writes a new key at every start.
    std::fs::write(&path, "new-key").unwrap();
    assert_eq!(lookup_key(None, Some(&dir), Some(7981)).unwrap(), "new-key");
    // Another port, another file.
    assert!(lookup_key(None, Some(&dir), Some(7982)).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn key_only_for_loopback_addresses() {
    let peer = |s: &str| s.parse::<SocketAddr>().ok();
    for (addr, at) in [
        ("127.0.0.1:7981", "127.0.0.1:7981"),
        ("127.0.0.2:7981", "127.0.0.2:7981"),
        ("[::1]:7981", "[::1]:7981"),
        ("[::ffff:127.0.0.1]:7981", "[::ffff:127.0.0.1]:7981"),
        ("localhost:7981", "127.0.0.1:7981"),
        ("localhost:7981", "[::1]:7981"),
        ("LocalHost:7981", "127.0.0.1:7981"),
    ] {
        assert!(may_send_key(addr, peer(at)), "{addr} at {at}");
    }
    for (addr, at) in [
        ("192.0.2.20:7981", "192.0.2.20:7981"),
        ("10.0.0.7:7981", "10.0.0.7:7981"),
        ("[fe80::1]:7981", "[fe80::1]:7981"),
        ("0.0.0.0:7981", "127.0.0.1:7981"),
        // Host names other than localhost: even when they lead to this machine.
        ("workstation:7981", "127.0.1.1:7981"),
        ("example.org:7981", "93.184.215.14:7981"),
        ("localhost.example.org:7981", "127.0.0.1:7981"),
        // `localhost` that does not lead to a loopback address.
        ("localhost:7981", "192.0.2.20:7981"),
    ] {
        assert!(!may_send_key(addr, peer(at)), "{addr} at {at}");
    }
    // No peer address: no key.
    assert!(!may_send_key("127.0.0.1:7981", None));
}

/// A stand-in for the app on 127.0.0.1: every request line goes to the returned channel and
/// gets `answer(request)`: a reply and whether to close the connection after it, or `None`
/// to close it without a reply (the app quit).
fn fake_app(answer: impl Fn(&Value) -> Option<(Value, bool)> + Send + Sync + 'static) -> (u16, Receiver<Value>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = channel();
    let answer = Arc::new(answer);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (tx, answer) = (tx.clone(), answer.clone());
            std::thread::spawn(move || {
                let mut out = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines() {
                    let Ok(req) = serde_json::from_str::<Value>(&line.unwrap_or_default()) else { return };
                    // The challenge is the bridge's own; the tests look at the requests.
                    if req["method"] != "auth.challenge" {
                        let _ = tx.send(req.clone());
                    }
                    let Some((mut reply, close)) = answer(&req) else { return };
                    reply["id"] = req["id"].clone();
                    if writeln!(out, "{reply}").is_err() || close {
                        return;
                    }
                }
            });
        }
    });
    (port, rx)
}

/// What the real app answers to a challenge, holding `key`.
fn prove(req: &Value, key: &str) -> Value {
    json!({"ok": true, "result": {"proof": mailcraft_control_key::proof::proof(key, req["params"]["nonce"].as_str().unwrap_or(""))}})
}

fn fixed_key(key: &'static str) -> KeyFinder {
    Box::new(move |_| Ok(key.to_string()))
}

const WAIT: Duration = Duration::from_secs(5);

#[test]
fn sends_the_key_to_loopback() {
    let (port, seen) =
        fake_app(|req| Some(if req["method"] == "auth.challenge" { (prove(req, "k1"), false) } else { (json!({"ok": true, "result": 7}), false) }));
    for host in ["127.0.0.1", "localhost"] {
        let mut r = Remote::connect_with(&format!("{host}:{port}"), fixed_key("k1")).unwrap();
        assert!(r.key_warning().is_none(), "{host}");
        assert_eq!(r.call("mailbox.inspect", json!({})).unwrap(), json!(7));
        assert_eq!(seen.recv_timeout(WAIT).unwrap()["key"], "k1", "{host}");
    }
}

#[test]
fn every_request_is_marked_as_an_agents() {
    let (port, seen) =
        fake_app(|req| Some(if req["method"] == "auth.challenge" { (prove(req, "k1"), false) } else { (json!({"ok": true, "result": 1}), false) }));
    let mut r = Remote::connect_with(&format!("127.0.0.1:{port}"), fixed_key("k1")).unwrap().with_allow(vec![mailcraft_engine::agent::Class::Input]);
    // Whatever the params say.
    r.call("engine.execute", json!({"command": "mail.flag", "agent": null, "params": {"agent": false}})).unwrap();
    let req = seen.recv_timeout(WAIT).unwrap();
    assert_eq!(req["agent"], json!({"allow": ["input"]}));
}

#[test]
fn reads_the_key_again_on_a_new_connection() {
    // The app restarted with a new key: the old key is refused (and the connection closed).
    let (port, seen) = fake_app(|req| {
        Some(if req["method"] == "auth.challenge" {
            (prove(req, "new"), false)
        } else if req["key"] == "new" {
            (json!({"ok": true, "result": 1}), false)
        } else {
            (json!({"ok": false, "error": "unauthorized"}), true)
        })
    });
    let reads = Arc::new(AtomicUsize::new(0));
    let n = reads.clone();
    let finder: KeyFinder = Box::new(move |_| Ok(if n.fetch_add(1, Ordering::SeqCst) == 0 { "old" } else { "new" }.to_string()));
    let mut r = Remote::connect_with(&format!("127.0.0.1:{port}"), finder).unwrap();
    assert_eq!(r.call("mailbox.inspect", json!({})).unwrap(), json!(1));
    // The stale key is never sent: the app cannot prove it holds it.
    assert_eq!(seen.recv_timeout(WAIT).unwrap()["key"], "new");
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    // The new key is kept for the next requests on the same connection.
    assert_eq!(r.call("mailbox.inspect", json!({})).unwrap(), json!(1));
    assert_eq!(reads.load(Ordering::SeqCst), 2);

    // The app quits mid-session (connection closed): the retry reads the key again.
    let challenges = Arc::new(AtomicUsize::new(0));
    let c = challenges.clone();
    let (port, seen) = fake_app(move |req| {
        if req["method"] == "auth.challenge" {
            return Some((prove(req, &format!("key-{}", c.fetch_add(1, Ordering::SeqCst))), false));
        }
        if req["id"] == 2 { None } else { Some((json!({"ok": true, "result": 1}), false)) }
    });
    let reads = Arc::new(AtomicUsize::new(0));
    let n = reads.clone();
    let finder: KeyFinder = Box::new(move |_| Ok(format!("key-{}", n.fetch_add(1, Ordering::SeqCst))));
    let mut r = Remote::connect_with(&format!("127.0.0.1:{port}"), finder).unwrap();
    r.call("mailbox.inspect", json!({})).unwrap();
    r.call("mailbox.inspect", json!({})).unwrap_err();
    let keys: Vec<Value> = seen.try_iter().map(|v| v["key"].clone()).collect();
    assert_eq!(keys.first(), Some(&json!("key-0")), "{keys:?}");
    assert_eq!(keys.last(), Some(&json!("key-1")), "{keys:?}");
}

#[test]
fn no_key_for_other_hosts_and_a_clear_error() {
    // There is no LAN address to test with here: same stand-in, with the decision a LAN
    // address gets.
    let (port, seen) = fake_app(|req| {
        Some(if req.get("key").is_some() { (json!({"ok": true, "result": 1}), false) } else { (json!({"ok": false, "error": "unauthorized"}), true) })
    });
    let finder: KeyFinder = Box::new(|_| Err("the key must not be read for another host".to_string()));
    let mut r = Remote::connect_with(&format!("127.0.0.1:{port}"), finder).unwrap();
    r.send_key = false;
    assert!(r.key_warning().is_some_and(|w| w.contains("not a loopback address")));
    let e = r.call("mailbox.inspect", json!({})).unwrap_err();
    assert!(e.contains("not a loopback address") && e.contains("not sent"), "{e}");
    assert!(seen.recv_timeout(WAIT).unwrap().get("key").is_none());
}
