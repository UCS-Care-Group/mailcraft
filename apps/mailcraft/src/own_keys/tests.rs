// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host side of the owner's own keys (#30 R13): GnuPG's listings read, and imports kept in
//! a stand-in credential store, with synthetic `example.org` keys made for these tests alone.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mailcraft_charset::base64::STANDARD;
use mailcraft_engine::model::digital_signature::SignatureFormat;
use mailcraft_host::SecretStore;
use mailcraft_ui_egui::own_keys::{Imported, KeyFile, KeyPlace, OwnKeysAnswer, OwnKeysJob, Passphrase};
use zeroize::Zeroizing;

use super::fixtures_tests::{CAROL_SECRET, COMPONENTS, DAN_SECRET, EC_P12, ERIN_SECRET, LIST_KEYS, LIST_SECRET};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use super::fixtures_tests::{PASS, RSA_P12};
use super::gnupg::{gpg_in, secret_in, valid_in};

#[derive(Default)]
struct Store(Mutex<HashMap<String, String>>);

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), mailcraft_host::SecretError> {
        self.0.lock().unwrap().insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

fn import(format: SignatureFormat, bytes: &[u8], passphrase: Option<&str>, store: Option<Arc<Store>>) -> Result<Imported, String> {
    let job = OwnKeysJob::Import { format, file: KeyFile(Zeroizing::new(bytes.to_vec())), passphrase: passphrase.map(|p| Passphrase::new(p.into())) };
    match super::run(job, store.map(|s| s as Arc<dyn SecretStore>)) {
        OwnKeysAnswer::Imported(result) => result,
        other => panic!("{other:?}"),
    }
}

#[test]
fn gpg_is_found_in_gpgconfs_list() {
    // (An absolute path is the system's own kind: Gpg4win's has a drive, its colon as `%3a`.)
    let unix = gpg_in(COMPONENTS.as_bytes());
    #[cfg(not(windows))]
    assert_eq!(unix.unwrap().to_str(), Some("/usr/bin/gpg"));
    #[cfg(windows)]
    assert!(unix.is_none(), "not a path on Windows");
    #[cfg(windows)]
    assert_eq!(
        gpg_in(b"gpgconf:GPG Configuration:C%3a\\Program Files\\GnuPG\\bin\\gpgconf.exe\ngpg:OpenPGP:C%3a\\Program Files\\GnuPG\\bin\\gpg.exe\n")
            .unwrap()
            .to_str(),
        Some("C:\\Program Files\\GnuPG\\bin\\gpg.exe")
    );
    assert!(gpg_in(b"gpg:OpenPGP:relative/gpg\n").is_none());
    assert!(gpg_in(b"gpgsm:S/MIME:/usr/bin/gpgsm\n").is_none());
}

#[test]
fn the_secret_listing_is_read() {
    let keys = secret_in(LIST_SECRET.as_bytes(), 0);
    assert_eq!(keys.len(), 3);
    assert_eq!(keys[0].fingerprint, "B218 EB03 0C44 1424 207A A6C0 10C3 5688 F1AD A5A2");
    assert_eq!(keys[0].addresses, ["carol@example.org"]);
    assert_eq!(keys[0].name, "Carol Example <carol@example.org>");
    assert!(keys.iter().all(|k| k.can_sign && !k.expired && k.place == KeyPlace::Gnupg));
    let revoked =
        "sec:r:255:22:AA:1::::::scSC:::+:\nfpr:::::::::B218EB030C441424207AA6C010C35688F1ADA5A2:\nuid:r::::::::Mallory \\x3a <m@example.org>:\n";
    let k = &secret_in(revoked.as_bytes(), 0)[0];
    assert!(k.expired);
    assert_eq!(k.name, "Mallory : <m@example.org>");
    assert!(secret_in(b"sec:u:::\nuid:u::::::::x <x@example.org>:\n", 0).is_empty());
    assert!(secret_in(&[0xff, b':', b'\n'], 0).is_empty());
}

#[test]
fn only_full_and_ultimate_user_ids_are_valid() {
    let pairs = valid_in(LIST_KEYS.as_bytes());
    assert_eq!(pairs.len(), 3);
    assert_eq!(pairs[0], ("B218EB030C441424207AA6C010C35688F1ADA5A2".into(), "carol@example.org".into()));
    let marginal = "pub:m:::\nfpr:::::::::B218EB030C441424207AA6C010C35688F1ADA5A2:\nuid:m::::::::Carol <carol@example.org>:\n";
    assert!(valid_in(marginal.as_bytes()).is_empty());
    let revoked = "pub:r:::\nfpr:::::::::B218EB030C441424207AA6C010C35688F1ADA5A2:\nuid:f::::::::Carol <carol@example.org>:\n";
    assert!(valid_in(revoked.as_bytes()).is_empty());
}

/// GnuPG is asked under its direct trust model whatever its options say, and only what that
/// gives `f` or `u` counts: as GnuPG 2.4 lists a keyring of synthetic keys under it (the owner's
/// own key, one whose ownertrust the owner set to full, one the owner certified but set nothing
/// on, one a fully trusted key certified).
#[test]
fn validity_is_read_under_the_direct_trust_model_only() {
    assert_eq!(super::gnupg::VALID, ["--trust-model=direct", "--list-keys"]);
    let direct = "tru:t:1:1794361287:0:3:1:5\n\
        pub:u:255:22:AA:1::::::scSC:\nfpr:::::::::B218EB030C441424207AA6C010C35688F1ADA5A2:\nuid:u::::::::Owner <owner@example.org>:\n\
        pub:f:255:22:BB:1::::::scSC:\nfpr:::::::::8BC23215100DCD66578744B8F679705104FB7CCB:\nuid:f::::::::Pal <pal@example.org>:\n\
        pub:e:255:22:CC:1::::::scSC:\nfpr:::::::::8B04E32F9C92601AD5A4D5D612D680A3D331558F:\nuid:e::::::::Set <set@example.org>:\n\
        pub:-:255:22:DD:1::::::scSC:\nfpr:::::::::04928A3DA30AE43BCCACAB6C054BC24260C4ECDD:\nuid:-::::::::Unknown <unk@example.org>:\n";
    let addresses: Vec<String> = valid_in(direct.as_bytes()).into_iter().map(|(_, a)| a).collect();
    assert_eq!(addresses, ["owner@example.org", "pal@example.org"]);
}

/// The runner stops a program that takes too long, or writes more than is read, at once.
#[cfg(unix)]
#[test]
fn a_program_is_stopped_past_its_time_or_its_output() {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use super::gnupg::run_within;
    let started = Instant::now();
    let err = run_within(Path::new("/bin/sleep"), &["30"], Duration::from_millis(300), 1024).unwrap_err();
    assert!(err.contains("didn't finish in time"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10));
    if let Some(yes) = ["/usr/bin/yes", "/bin/yes"].into_iter().map(Path::new).find(|p| p.exists()) {
        let started = Instant::now();
        let err = run_within(yes, &[], Duration::from_secs(20), 4096).unwrap_err();
        assert!(err.contains("listed more"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10), "stopped at once, not at the time limit");
    }
    if let Some(echo) = ["/bin/echo", "/usr/bin/echo"].into_iter().map(Path::new).find(|p| p.exists()) {
        assert_eq!(run_within(echo, &["example"], Duration::from_secs(20), 4096).unwrap(), b"example\n");
    }
}

#[test]
fn an_openpgp_key_is_kept_by_its_fingerprint_with_its_public_half() {
    let store = Arc::new(Store::default());
    let Imported::Kept { key, public } = import(SignatureFormat::OpenPgp, CAROL_SECRET.as_bytes(), None, Some(store.clone())).unwrap() else {
        panic!("not kept")
    };
    assert_eq!(key.store_key, "key:openpgp:b218eb030c441424207aa6c010c35688f1ada5a2");
    assert_eq!(key.addresses, ["carol@example.org"]);
    assert!(public.is_some_and(|p| !p.is_empty()));
    let kept = store.get(&key.store_key).unwrap();
    assert_eq!(STANDARD.decode(kept).unwrap(), CAROL_SECRET.as_bytes());
    assert_eq!(store.0.lock().unwrap().len(), 1);
}

#[test]
fn what_the_rules_refuse_is_never_kept() {
    let store = Arc::new(Store::default());
    assert!(import(SignatureFormat::OpenPgp, DAN_SECRET.as_bytes(), None, Some(store.clone())).unwrap_err().contains("no passphrase"));
    assert!(import(SignatureFormat::OpenPgp, ERIN_SECRET.as_bytes(), None, Some(store.clone())).unwrap_err().contains("RSA"));
    assert!(import(SignatureFormat::OpenPgp, CAROL_SECRET.as_bytes(), None, None).unwrap_err().contains("no credential store"));
    assert!(import(SignatureFormat::Smime, &STANDARD.decode(EC_P12).unwrap(), None, Some(store.clone())).is_err());
    assert!(store.0.lock().unwrap().is_empty());
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[test]
fn a_p256_certificate_is_kept_and_rsa_refused_where_tokens_are_the_store() {
    let store = Arc::new(Store::default());
    let rsa = STANDARD.decode(RSA_P12).unwrap();
    assert!(import(SignatureFormat::Smime, &rsa, Some(PASS), Some(store.clone())).unwrap_err().contains("RSA"));
    let ec = STANDARD.decode(EC_P12).unwrap();
    assert!(import(SignatureFormat::Smime, &ec, Some("wrong"), Some(store.clone())).unwrap_err().contains("passphrase is wrong"));
    assert!(store.0.lock().unwrap().is_empty());
    let Imported::Kept { key, public } = import(SignatureFormat::Smime, &ec, Some(PASS), Some(store.clone())).unwrap() else { panic!("not kept") };
    assert!(key.store_key.starts_with("key:smime:") && key.store_key.len() == 10 + 64, "{}", key.store_key);
    assert_eq!(key.addresses, ["bob@example.org"]);
    assert!(public.is_none());
    // The file as it came, still protected by its passphrase: never the passphrase itself.
    let kept = store.get(&key.store_key).unwrap();
    assert_eq!(STANDARD.decode(&kept).unwrap(), ec);
    assert!(!store.0.lock().unwrap().values().any(|v| v.contains(PASS)));
}
