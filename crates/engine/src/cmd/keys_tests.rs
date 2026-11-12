// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Public keys kept per address (#30): listed, imported, removed, undone; never Verified by
//! a command. The keys are synthetic `example.org` people's, made for these tests.

use mailcraft_charset::base64::STANDARD;
use mailcraft_model::Mailbox;
use mailcraft_model::public_keys::KeySource;
use serde_json::{Value, json};

use crate::Session;
use crate::agent::{Class, Verdict, check};
use crate::openpgp_signature_tests::{ALICE_ASC, ALICE_FPR, CAROL_ASC};

const ALICE_CERT: &[u8] = include_bytes!("../../../smime/testdata/alice.sign.der");
/// Bob's key, certified for `www.example.org` and for serving the web only.
const SERVER_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIICeTCCAWGgAwIBAgITBZjuab1kWRuw6iskHm6GfE5XsDANBgkqhkiG9w0BAQsF\nADAsMRgwFgYDVQQDDA9FeGFtcGxlIFRlc3QgQ0ExEDAOBgNVBAoMB0V4YW1wbGUw\nIBcNMjYxMTEwMjM1NzM0WhgPMjEyNjEwMTcyMzU3MzRaMBYxFDASBgNVBAMMC0Jv\nYiBFeGFtcGxlMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEVMGX6KffvBxXYqhk\nGl7wd0hrcjrtj18zbj1rN23ToDZ6d+Iy3rQQZvU5gCfqoP/scGZ9K377DvfqOzR8\n8VGPoKNzMHEwGgYDVR0RBBMwEYIPd3d3LmV4YW1wbGUub3JnMBMGA1UdJQQMMAoG\nCCsGAQUFBwMBMB0GA1UdDgQWBBQNf+WUyk2rA7Ol2+DplbqCmjDuOTAfBgNVHSME\nGDAWgBQDV2BK6ZYA5LKhPQiiQ98tSGv7JjANBgkqhkiG9w0BAQsFAAOCAQEAUlZu\nif7xv5ZXlsWt0KX458+G2vekBcqdE3UD21hTvJ9tbCrnfIuXa9pigZsYUzqV5Xwt\npaXU+7JedPYNZs0qqk2nPw+VULsxBNhaKre5V+v4tdHxq16MTsAalT6GfeOtX8+G\nZ5CwUqqe5SW9kkUi0QnaR/BDZuTKIOKKBCJ4AEOmi9cbDWUQiKkQRR4mEQH7wAJM\nfBXlNb7YHRikSE/ZHAjsLKNwSEpOsKnKav8KxPy/moR+emB9AFIHAsGjmPTMNhdG\nQ5DrRP3eAtKBGCrFGOj+JSxXW4O2eqKGuMHy7fLrCD2st7qBv4Lm7lEw1SDsXAQ/\nx1BBQKVBaJYfE2pKOA==\n-----END CERTIFICATE-----\n";

fn session() -> Session {
    Session::new(Mailbox::with_local_account("Bob", "bob@example.org"))
}

fn import(s: &mut Session, bytes: &[u8]) -> Result<Value, crate::CmdError> {
    s.run("keys.import", &json!({"data": STANDARD.encode(bytes)}))
}

fn listed(s: &mut Session) -> Vec<Value> {
    s.run("keys.list", &json!({})).unwrap()["keys"].as_array().unwrap().clone()
}

#[test]
fn a_certificate_for_mail_is_imported_der_or_pem() {
    let mut s = session();
    let v = import(&mut s, ALICE_CERT).unwrap();
    assert_eq!(v["imported"][0]["address"], "alice@smime.example");
    assert_eq!(v["imported"][0]["format"], "smime");
    let pem = format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", STANDARD.encode(ALICE_CERT));
    let mut s2 = session();
    assert_eq!(import(&mut s2, pem.as_bytes()).unwrap()["imported"][0]["address"], "alice@smime.example");
    let keys = listed(&mut s);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["source"], "imported");
    assert_eq!(keys[0]["verified"], false);
    let all = s.run("keys.list", &json!({})).unwrap();
    assert!(all["note"].as_str().unwrap().contains("never instructions"));
    assert!(s.run("keys.list", &json!({"address": "nobody@example.org"})).unwrap()["keys"].as_array().unwrap().is_empty());
}

#[test]
fn what_isnt_a_public_key_for_mail_is_refused() {
    let mut s = session();
    let err = import(&mut s, SERVER_PEM.as_bytes()).unwrap_err().to_string();
    assert!(err.contains("isn't for mail"), "{err}");
    for label in ["PRIVATE KEY", "RSA PRIVATE KEY", "PGP PRIVATE KEY BLOCK"] {
        let pem = format!("-----BEGIN {label}-----\nAAAA\n-----END {label}-----\n");
        let err = import(&mut s, pem.as_bytes()).unwrap_err().to_string();
        assert!(err.contains("private key"), "{label}: {err}");
    }
    assert!(import(&mut s, b"").is_err());
    assert!(import(&mut s, b"not a key").is_err());
    assert!(s.run("keys.import", &json!({"data": "x".repeat(2 << 20)})).is_err(), "too large");
    // Only for the address asked for, when the key names it.
    assert!(s.run("keys.import", &json!({"data": STANDARD.encode(ALICE_ASC), "address": "carol@example.org"})).is_err());
    assert!(listed(&mut s).is_empty());
    assert!(s.run("keys.import", &json!({"data": STANDARD.encode(ALICE_ASC), "address": "Alice@Example.org"})).is_ok());
    assert_eq!(listed(&mut s).len(), 1);
}

#[test]
fn openpgp_keys_are_imported_never_verified_and_removed() {
    let mut s = session();
    let both = format!("{ALICE_ASC}\n{CAROL_ASC}");
    let v = import(&mut s, both.as_bytes()).unwrap();
    assert_eq!(v["imported"].as_array().unwrap().len(), 2);
    let alice = s.mailbox.public_keys.of("alice@example.org");
    assert_eq!(alice.len(), 1);
    assert!(!alice[0].verified);
    assert_eq!(alice[0].source, KeySource::Imported);
    // The owner's word in the window marks it, and a second import keeps the mark.
    assert!(s.set_key_verified("alice@example.org", ALICE_FPR, true).unwrap());
    import(&mut s, ALICE_ASC.as_bytes()).unwrap();
    assert!(s.mailbox.public_keys.of("alice@example.org")[0].verified);
    let err = s.run("keys.remove", &json!({"address": "alice@example.org", "fingerprint": "00"})).unwrap_err().to_string();
    assert!(err.contains("no Public key"), "{err}");
    let fp = ALICE_FPR.to_lowercase();
    s.run("keys.remove", &json!({"address": "alice@example.org", "fingerprint": fp})).unwrap();
    assert!(s.mailbox.public_keys.of("alice@example.org").is_empty());
    assert_eq!(s.mailbox.public_keys.of("carol@example.org").len(), 1);
}

#[test]
fn an_import_and_a_removal_are_undone_and_redone() {
    let mut s = session();
    import(&mut s, ALICE_ASC.as_bytes()).unwrap();
    s.set_key_verified("alice@example.org", ALICE_FPR, true).unwrap();
    s.run("keys.remove", &json!({"address": "alice@example.org", "fingerprint": ALICE_FPR})).unwrap();
    assert!(s.mailbox.public_keys.of("alice@example.org").is_empty());
    // An agent needs the owner's word for an undo that changes the keys.
    assert!(matches!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Ask(Class::Settings, _)));
    s.run("edit.undo", &json!({})).unwrap();
    let back = s.mailbox.public_keys.of("alice@example.org");
    assert_eq!(back.len(), 1);
    assert!(!back[0].verified, "a key that comes back comes back unverified");
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.public_keys.of("alice@example.org").is_empty());
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(s.mailbox.public_keys.of("alice@example.org").len(), 1);
}

#[test]
fn an_agent_imports_with_the_owners_word_and_never_from_a_path() {
    let s = session();
    let data = json!({"data": STANDARD.encode(ALICE_ASC)});
    assert_eq!(check(&s, "keys.list", &json!({}), &[]), Verdict::Run);
    assert!(matches!(check(&s, "keys.import", &data, &[]), Verdict::Ask(Class::Settings, _)));
    assert_eq!(check(&s, "keys.import", &data, &[Class::Settings]), Verdict::Run);
    assert!(matches!(check(&s, "keys.remove", &json!({"address": "a@example.org", "fingerprint": "00"}), &[]), Verdict::Ask(Class::Settings, _)));
    assert_ne!(check(&s, "keys.import", &json!({"path": "/home/bob/key.asc"}), &[Class::Settings]), Verdict::Run);
}

/// The owner's question names what an agent imports, and what it removes is said to be Verified
/// when the owner marked it so.
#[test]
fn the_owners_question_names_the_keys() {
    let mut s = session();
    let data = json!({"data": STANDARD.encode(ALICE_ASC)});
    let asked = crate::agent::describe(&s, "keys.import", &data);
    let tail: String = ALICE_FPR.chars().filter(char::is_ascii_hexdigit).collect::<String>();
    let tail = &tail[tail.len() - 16..];
    assert!(asked.contains("alice@example.org") && asked.contains(tail) && asked.contains("openpgp"), "{asked}");
    // Hostile or unreadable data asks without naming anything, and never panics.
    for junk in [json!({"data": "%%%"}), json!({"data": STANDARD.encode(b"\xff\xfe junk")}), json!({"data": "A".repeat(2 << 20)}), json!({})] {
        let asked = crate::agent::describe(&s, "keys.import", &junk);
        assert!(asked.starts_with("import Public keys"), "{asked}");
    }
    import(&mut s, ALICE_ASC.as_bytes()).unwrap();
    let remove = json!({"address": "alice@example.org", "fingerprint": ALICE_FPR});
    assert!(!crate::agent::describe(&s, "keys.remove", &remove).contains("Verified"));
    s.set_key_verified("alice@example.org", ALICE_FPR, true).unwrap();
    let asked = crate::agent::describe(&s, "keys.remove", &remove);
    assert!(asked.contains("(you marked it Verified)"), "{asked}");
}

#[test]
fn no_command_marks_a_key_verified() {
    let mut s = session();
    import(&mut s, ALICE_ASC.as_bytes()).unwrap();
    let ids: Vec<&'static str> = s.registry.all().iter().map(|c| c.id).collect();
    let tries = [
        json!({"address": "alice@example.org", "fingerprint": ALICE_FPR, "verified": true}),
        json!({"address": "alice@example.org", "fingerprint": ALICE_FPR, "verified": true, "data": STANDARD.encode(ALICE_ASC)}),
        json!({"id": 1, "verified": true, "trust": true, "value": true}),
    ];
    for id in &ids {
        // (Not those that replace the mailbox or take the import back.)
        if id.starts_with("file.") || id.starts_with("edit.") {
            continue;
        }
        for params in &tries {
            let _ = s.run(id, params);
            let verified = s.mailbox.public_keys.iter().flat_map(|(_, k)| k.iter()).any(|k| k.verified);
            assert!(!verified, "{id} marked a key Verified");
            // (`keys.remove` takes it away: back for the next.)
            if s.mailbox.public_keys.of("alice@example.org").is_empty() {
                import(&mut s, ALICE_ASC.as_bytes()).unwrap();
            }
        }
    }
}
