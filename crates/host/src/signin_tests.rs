// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use mailcraft_model::sync::Tokens;

use crate::signin::{Endpoints, Page, SignIns, Why, forget, hint_of, keep_refresh, page, sensible_email, signed_in_address, tenant_of};
use crate::{Platform, PlatformError, Redirect, SecretError, SecretStore};

#[derive(Default)]
struct Kept(Mutex<HashMap<String, String>>);

impl SecretStore for Kept {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).get(key).cloned()
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).insert(key.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).remove(key);
    }
}

struct Phone(Option<Arc<Kept>>);

impl Platform for Phone {
    fn secrets(&self) -> Option<Arc<dyn SecretStore>> {
        self.0.clone().map(|k| k as Arc<dyn SecretStore>)
    }
}

#[test]
fn only_a_shape_of_address_is_one() {
    assert!(sensible_email("ada@lumen.example"));
    for bad in ["", "ada", "@lumen.example", "ada@", "a@b@c", "ada @lumen.example", "ada@lumen.example\u{7}"] {
        assert!(!sensible_email(bad), "{bad:?}");
    }
    assert!(!sensible_email(&format!("{}@lumen.example", "a".repeat(300))));
    assert_eq!(hint_of("  ada@lumen.example "), "ada@lumen.example");
    assert_eq!(hint_of("not an address"), "");
}

#[test]
fn the_directory_named_for_an_administrator_is_a_plain_domain_or_nothing() {
    assert_eq!(tenant_of("Ada@Lumen.Example"), "lumen.example");
    assert_eq!(tenant_of("ada@lumen.example/../x"), "");
    assert_eq!(tenant_of("nobody"), "");
}

#[test]
fn who_signed_in_is_what_microsoft_names_and_nothing_else() {
    let tokens = |email: &str| Tokens { email: email.to_string(), ..Default::default() };
    assert_eq!(signed_in_address(&tokens(" ada@lumen.example ")).as_deref(), Some("ada@lumen.example"));
    assert_eq!(signed_in_address(&tokens("")), None);
    assert_eq!(signed_in_address(&tokens("not an address")), None);
}

#[test]
fn one_sign_in_at_a_time_and_the_first_address_only() {
    let signins = SignIns::default();
    // Nobody waits: the address finds nothing to go to.
    assert!(!signins.deliver("x:/y".to_string()));
    let place = signins.begin().unwrap();
    assert!(signins.begin().is_none());
    let rx = signins.arm();
    assert!(!signins.deliver("x".repeat(9 * 1024)), "too long");
    assert!(signins.deliver("x:/first".to_string()));
    assert!(!signins.deliver("x:/second".to_string()));
    assert_eq!(rx.try_recv().unwrap(), "x:/first");
    signins.cancel();
    assert!(place.cancel_flag().load(std::sync::atomic::Ordering::Acquire));
    drop(place);
    assert!(signins.begin().is_some());
}

#[test]
fn a_refresh_token_is_kept_under_the_graph_name_and_forgotten_with_it() {
    let kept = Arc::new(Kept::default());
    let phone = Phone(Some(kept.clone()));
    keep_refresh(&phone, "Ada@Lumen.Example", "r1").unwrap();
    assert_eq!(kept.get("refresh:ada@lumen.example|graph").as_deref(), Some("r1"));
    kept.set("password:ada@lumen.example|graph", "p").unwrap();
    forget(&phone, "ada@lumen.example");
    assert!(kept.0.lock().unwrap().is_empty());
}

#[test]
fn nothing_is_kept_without_a_token_or_a_store() {
    let phone = Phone(Some(Arc::new(Kept::default())));
    assert!(keep_refresh(&phone, "ada@lumen.example", "").is_err());
    assert!(keep_refresh(&Phone(None), "ada@lumen.example", "r1").is_err());
}

#[test]
fn a_sign_in_that_tries_again_is_armed_for_a_second_redirect() {
    let signins = SignIns::default();
    let armer = signins.armer();
    let first = armer.arm();
    assert!(signins.deliver("x:/first".to_string()));
    assert_eq!(first.try_recv().unwrap(), "x:/first");
    // The first address took the slot: nothing more is taken until the sign-in arms it again.
    assert!(!signins.deliver("x:/early".to_string()));
    let second = armer.arm();
    assert!(signins.deliver("x:/second".to_string()));
    assert_eq!(second.try_recv().unwrap(), "x:/second");
}

/// A phone whose browser sends the owner straight back: the first time with a refusal of the
/// optional consent, the second with a code. Each address goes in through the same `SignIns`
/// slot the app's link handler uses.
struct Bouncing {
    signins: Arc<SignIns>,
    opened: Mutex<usize>,
    gave_up: Arc<std::sync::atomic::AtomicBool>,
}

impl Platform for Bouncing {
    fn redirect(&self) -> Redirect {
        Redirect::CustomScheme("work.mailcraft.android".to_string())
    }

    fn open_sign_in_page(&self, url: &str) -> Result<(), PlatformError> {
        let state = url.split(['?', '&']).find_map(|f| f.strip_prefix("state=")).unwrap_or_default().to_string();
        let n = {
            let mut opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
            *opened += 1;
            *opened
        };
        let tail = if n == 1 { "error=invalid_scope".to_string() } else { "code=second-code".to_string() };
        let taken = self.signins.deliver(format!("work.mailcraft.android://oauth2redirect?{tail}&state={state}"));
        if !taken {
            // (Nobody waits for this address: the sign-in would sit out its five minutes.)
            self.gave_up.store(true, std::sync::atomic::Ordering::Release);
        }
        Ok(())
    }
}

#[test]
fn a_retried_sign_in_takes_the_second_redirect_through_the_same_slot() {
    use std::io::{Read, Write};
    use std::sync::atomic::AtomicBool;

    // A token endpoint that answers once.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        // (The whole request: its head, then as many body bytes as it says, within a few seconds.)
        conn.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let mut seen: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 2048];
        loop {
            let text = String::from_utf8_lossy(&seen).into_owned();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
                    .unwrap_or(0);
                if seen.len() >= end + 4 + length {
                    break;
                }
            }
            match conn.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => seen.extend_from_slice(&chunk[..n]),
            }
        }
        let n = seen.len();
        let body = r#"{"token_type":"Bearer","expires_in":3600,"access_token":"acc-ess.tok_en","refresh_token":"re-fresh.tok_en"}"#;
        let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        conn.write_all(reply.as_bytes()).unwrap();
        String::from_utf8_lossy(&seen[..n]).into_owned()
    });
    let signins = Arc::new(SignIns::default());
    let gave_up = Arc::new(AtomicBool::new(false));
    let phone = Bouncing { signins: signins.clone(), opened: Mutex::new(0), gave_up: gave_up.clone() };
    let endpoints = Endpoints {
        authorize: "http://127.0.0.1:9/authorize".into(),
        token: format!("http://127.0.0.1:{port}/token"),
        scope: "mail.read offline_access".into(),
        consent: "graph.send".into(),
        lesser: String::new(),
    };
    let place = signins.begin().unwrap();
    let landed = signins.arm();
    // (Called off by the phone if the second address finds nobody waiting, so a fault fails and doesn't hang.)
    let cancel = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (gave_up, cancel) = (gave_up.clone(), cancel.clone());
        std::thread::spawn(move || {
            for _ in 0..200 {
                if gave_up.load(std::sync::atomic::Ordering::Acquire) {
                    cancel.store(true, std::sync::atomic::Ordering::Release);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            cancel.store(true, std::sync::atomic::Ordering::Release);
        })
    };
    let result = page(&phone, landed, &signins.armer(), &endpoints, "", Why::Account, &cancel);
    drop(place);
    let _ = watcher;
    match result {
        Page::Tokens(tokens) => assert_eq!(tokens.refresh, "re-fresh.tok_en"),
        _ => panic!("the second redirect did not complete the sign-in"),
    }
    assert!(!gave_up.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(*phone.opened.lock().unwrap(), 2);
    assert!(server.join().unwrap().contains("second-code"));
}

#[test]
fn a_called_off_sign_in_does_not_arm_the_slot_again() {
    let signins = SignIns::default();
    let armer = signins.armer();
    let live = std::sync::atomic::AtomicBool::new(false);
    assert!(armer.arm_unless(&live).is_some());
    let _ = signins.deliver("x://y".to_string());
    // Called off (the place dropped, the slot cleared), and a stale hand-back comes after.
    let called_off = std::sync::atomic::AtomicBool::new(true);
    assert!(armer.arm_unless(&called_off).is_none());
    assert!(!signins.deliver("x://z".to_string()), "nothing waits for it");
}
