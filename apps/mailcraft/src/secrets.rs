//! The operating system's credential store: Windows Credential Manager, the macOS keychain, and
//! on Linux and the BSDs the Secret Service (GNOME Keyring, KWallet, KeePassXC…) on the session
//! bus.
//!
//! A password or a refresh token the owner asks to be remembered is kept here, encrypted by the
//! system for this user, and nowhere else: never in the mailbox file, the settings or a log.
//! Where no store answers (a Linux machine without a Secret Service running: a minimal window
//! manager, a remote session, a headless box), there is none: nothing is remembered, the owner
//! is asked for the password at each start as before, and File › Options says why.

use std::sync::mpsc;
use std::time::Duration;

use mailcraft_ui_egui::SecretStore;

/// The store on systems that have one.
pub fn store() -> Option<Box<dyn SecretStore>> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        Some(Box::new(os::OsStore))
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))]
    {
        // Asked once, at the start: a session bus that hangs must not hold the window up.
        if answers_in_time(secret_service::answers, PROBE_WAIT) {
            Some(Box::new(secret_service::Store))
        } else {
            log::info!("no Secret Service answered on the session bus: sign-ins aren't remembered between runs");
            None
        }
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "netbsd",
        target_os = "openbsd"
    )))]
    {
        None
    }
}

/// How long the start waits for the Secret Service to answer. Starting one that is installed but
/// not running (D-Bus activation) takes well under this.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")),
    allow(dead_code)
)]
const PROBE_WAIT: Duration = Duration::from_secs(3);

/// Whether `probe` says yes within `wait`. It runs on a thread of its own: one that hangs is left
/// to finish by itself, and one that fails, panics or can't be started is a no.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd", test)),
    allow(dead_code)
)]
fn answers_in_time(probe: impl FnOnce() -> bool + Send + 'static, wait: Duration) -> bool {
    let (tx, rx) = mpsc::channel();
    let started = std::thread::Builder::new().name("credential store".into()).spawn(move || {
        let _ = tx.send(probe());
    });
    if started.is_err() {
        return false;
    }
    rx.recv_timeout(wait).unwrap_or(false)
}

/// The Secret Service (freedesktop.org's API, which GNOME Keyring, KWallet and KeePassXC answer
/// to) over D-Bus, spoken by `secret-service` on `zbus`: Rust throughout, no C library, the
/// secrets encrypted on their way over the bus (Diffie-Hellman session, AES). Each call opens
/// its own connection, so a store that was restarted since works on the next call. Any failure
/// is a secret not found or not kept: the owner is asked for the password, as without a store.
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))]
mod secret_service {
    use std::collections::HashMap;

    use mailcraft_ui_egui::SecretStore;
    use secret_service::EncryptionType;
    use secret_service::blocking::SecretService;

    /// What the items are filed under: `application` is what the store's own windows (Seahorse,
    /// KWalletManager) show beside the label, `key` which secret it is (`password:<address>`).
    const APPLICATION: &str = "MailCraft";

    /// Most bytes kept for one secret; a refresh token is a few kilobytes.
    const MAX_SECRET: usize = 64 * 1024;

    pub struct Store;

    fn attributes(key: &str) -> HashMap<&str, &str> {
        HashMap::from([("application", APPLICATION), ("key", key)])
    }

    fn connect() -> Result<SecretService<'static>, secret_service::Error> {
        SecretService::connect(EncryptionType::Dh)
    }

    /// Whether a Secret Service is there to talk to.
    pub fn answers() -> bool {
        connect().is_ok()
    }

    impl SecretStore for Store {
        fn get(&self, key: &str) -> Option<String> {
            let service = connect().ok()?;
            let found = service.search_items(attributes(key)).ok()?;
            let item = found.unlocked.into_iter().next().or_else(|| found.locked.into_iter().next())?;
            // A locked keyring asks its owner to unlock it, the way the system does.
            item.ensure_unlocked().ok()?;
            let secret = item.get_secret().ok()?;
            if secret.len() > MAX_SECRET {
                return None;
            }
            String::from_utf8(secret).ok()
        }

        fn set(&self, key: &str, secret: &str) -> Result<(), String> {
            // The error text names the store's complaint, never the secret.
            let refused = |e: secret_service::Error| format!("the system's keyring refused: {e}");
            if secret.len() > MAX_SECRET {
                return Err("it is too long for the system's keyring".into());
            }
            let service = connect().map_err(refused)?;
            let collection = service.get_default_collection().or_else(|_| service.get_any_collection()).map_err(refused)?;
            collection.ensure_unlocked().map_err(refused)?;
            let label = format!("{APPLICATION}: {key}");
            collection.create_item(&label, attributes(key), secret.as_bytes(), true, "text/plain").map_err(refused)?;
            Ok(())
        }

        fn delete(&self, key: &str) {
            let Ok(service) = connect() else { return };
            let Ok(found) = service.search_items(attributes(key)) else { return };
            for item in found.unlocked.iter().chain(&found.locked) {
                if let Err(e) = item.delete() {
                    log::debug!("couldn't remove a remembered sign-in from the keyring: {e}");
                }
            }
        }
    }
}

/// Windows keeps at most 2560 bytes per credential (1280 UTF-16 units), and a refresh token can
/// be longer, so a secret is kept in pieces: `<key>#n` holds how many, `<key>#0`… hold them.
#[cfg_attr(not(any(target_os = "windows", target_os = "macos", test)), allow(dead_code))]
const PIECE: usize = 1000;
/// More pieces than any secret needs; a count beyond it is damage, not a secret.
#[cfg_attr(not(any(target_os = "windows", target_os = "macos", test)), allow(dead_code))]
const MAX_PIECES: usize = 16;

/// `secret` cut at character boundaries into pieces of at most [`PIECE`] characters.
#[cfg_attr(not(any(target_os = "windows", target_os = "macos", test)), allow(dead_code))]
fn pieces(secret: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = secret.chars().collect();
    let pieces: Vec<String> = chars.chunks(PIECE).map(|c| c.iter().collect()).collect();
    (pieces.len() <= MAX_PIECES).then_some(pieces)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod os {
    use keyring::Entry;
    use mailcraft_ui_egui::SecretStore;

    use super::{MAX_PIECES, pieces};

    /// What the entries are filed under in the system's store.
    const SERVICE: &str = "MailCraft";

    pub struct OsStore;

    fn entry(key: &str, part: &str) -> Option<Entry> {
        Entry::new(SERVICE, &format!("{key}#{part}")).ok()
    }

    fn count(key: &str) -> Option<usize> {
        entry(key, "n")?.get_password().ok()?.parse::<usize>().ok().filter(|n| (1..=MAX_PIECES).contains(n))
    }

    impl SecretStore for OsStore {
        fn get(&self, key: &str) -> Option<String> {
            let mut secret = String::new();
            for i in 0..count(key)? {
                secret.push_str(&entry(key, &i.to_string())?.get_password().ok()?);
            }
            Some(secret)
        }

        fn set(&self, key: &str, secret: &str) -> Result<(), String> {
            // The error text names the store's complaint, never the secret.
            let refused = |e: keyring::Error| format!("the system's credential store refused: {e}");
            let pieces = pieces(secret).ok_or("it is too long for the system's credential store")?;
            self.delete(key);
            for (i, piece) in pieces.iter().enumerate() {
                Entry::new(SERVICE, &format!("{key}#{i}")).and_then(|e| e.set_password(piece)).map_err(refused)?;
            }
            Entry::new(SERVICE, &format!("{key}#n")).and_then(|e| e.set_password(&pieces.len().to_string())).map_err(refused)
        }

        fn delete(&self, key: &str) {
            let pieces = count(key).unwrap_or(0);
            for part in (0..pieces).map(|i| i.to_string()).chain(["n".to_string()]) {
                if let Some(e) = entry(key, &part) {
                    let _ = e.delete_credential();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_secret_is_cut_at_character_boundaries_and_goes_back_together() {
        let secret: String = "töken-ñ".chars().cycle().take(PIECE * 2 + 7).collect();
        let cut = pieces(&secret).unwrap();
        assert_eq!(cut.len(), 3);
        assert!(cut.iter().all(|p| p.chars().count() <= PIECE));
        assert_eq!(cut.concat(), secret);
        assert_eq!(pieces("short").unwrap(), ["short"]);
        assert_eq!(pieces("").unwrap().len(), 0);
        // Longer than any secret: refused rather than filling the store.
        assert!(pieces(&"x".repeat(PIECE * MAX_PIECES + 1)).is_none());
    }

    /// No Secret Service (none installed, none running, no session bus at all): there is no
    /// store, and the app goes on without one, asking for the password at each start.
    #[test]
    fn a_store_that_doesnt_answer_is_no_store() {
        let wait = Duration::from_millis(200);
        assert!(answers_in_time(|| true, wait));
        // It said no (nothing on the bus by that name).
        assert!(!answers_in_time(|| false, wait));
        // It never answered (a session bus that hangs): the start doesn't wait for it.
        let started = std::time::Instant::now();
        assert!(!answers_in_time(
            || {
                std::thread::sleep(Duration::from_secs(5));
                true
            },
            wait
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        // It fell over: still only a no.
        assert!(!answers_in_time(|| panic!("the bus went away"), wait));
    }

    /// Against the real store of the machine running the test, with a made-up key that is removed
    /// again. Run it by name: `cargo test -p mailcraft -- --ignored the_systems_store`.
    #[test]
    #[ignore = "writes to (and cleans up in) this user's credential store"]
    fn the_systems_store_keeps_a_long_secret_and_lets_it_go() {
        let store = store().unwrap();
        let key = "refresh:store-test@mailcraft.example";
        // Longer than one Windows credential holds, like a real refresh token.
        let secret: String = "0.AXoA-made_up.token~".chars().cycle().take(3_300).collect();
        store.delete(key);
        assert_eq!(store.get(key), None);
        store.set(key, &secret).unwrap();
        assert_eq!(store.get(key).as_deref(), Some(secret.as_str()));
        // A shorter one over it leaves nothing of the longer one behind.
        store.set(key, "short").unwrap();
        assert_eq!(store.get(key).as_deref(), Some("short"));
        store.delete(key);
        assert_eq!(store.get(key), None);
        store.delete(key);
    }
}
