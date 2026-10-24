// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask release-key new <dir>` and `cargo xtask release-key sign <file>`: the release
//! signature that tells a copy of MailCraft that an update is the project's own.
//!
//! - `new <dir>` makes two ed25519 key pairs (the one in use and the spare), writes each private
//!   key as base64 PKCS#8 into `<dir>` (a folder of the maintainer's, kept offline), and prints
//!   the two public keys in base64 for `RELEASE_KEYS` in `crates/net/src/update.rs`. It never
//!   writes over a file that is there, and on Unix the folder and the files are the owner's only.
//! - `sign <file>` signs the file's bytes with the private key in the environment variable
//!   `MAILCRAFT_RELEASE_KEY` (base64 PKCS#8, as `new` wrote it) and writes the signature, in
//!   base64 on one line, to `<file>.sig`. The release workflow signs `SHA256SUMS.txt` with it.
//!
//! The private key is never printed, logged or put into an error; only the public key is said.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use mailcraft_charset::base64::STANDARD;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};

/// Where `sign` takes the private key from.
pub const KEY_VARIABLE: &str = "MAILCRAFT_RELEASE_KEY";
/// The names of the private keys `new` writes.
pub const IN_USE: &str = "mailcraft-release-key-in-use.pkcs8.b64";
pub const SPARE: &str = "mailcraft-release-key-spare.pkcs8.b64";
/// Largest key text taken from the environment (a PKCS#8 ed25519 key is 85 bytes, 116 in base64).
const MAX_KEY_TEXT: usize = 1024;

pub fn run(args: &[&str]) -> Result<(), String> {
    match args {
        ["new", dir] => {
            let (in_use, spare) = new(Path::new(dir))?;
            println!("Two release keys are in {dir}: {IN_USE} (in use) and {SPARE} (the spare).");
            println!("Keep both private keys offline, in two places. Put the one in use in the `release` environment as {KEY_VARIABLE}.");
            println!();
            println!("The public keys, for RELEASE_KEYS in crates/net/src/update.rs (not secret):");
            println!("  in use: {in_use}");
            println!("  spare:  {spare}");
            Ok(())
        }
        ["sign", file] => {
            let key = std::env::var(KEY_VARIABLE).map_err(|_| format!("{KEY_VARIABLE} isn't set"))?;
            let (sig, public) = sign(Path::new(file), &key)?;
            println!("signed {file} with the release key {public}: {}", sig.display());
            Ok(())
        }
        _ => Err("usage: cargo xtask release-key new <dir> | sign <file>".into()),
    }
}

/// Make the two key pairs, write the private keys into `dir`, and give the public keys in base64.
pub fn new(dir: &Path) -> Result<(String, String), String> {
    let (a, b) = (dir.join(IN_USE), dir.join(SPARE));
    if a.exists() || b.exists() {
        return Err(format!("{} already has release keys: nothing was written over", dir.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let rng = SystemRandom::new();
    let mut public = Vec::new();
    for path in [&a, &b] {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "no random bytes for a key".to_string())?;
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| "the new key doesn't read back".to_string())?;
        write_private(path, &STANDARD.encode(pkcs8.as_ref()))?;
        public.push(STANDARD.encode(pair.public_key().as_ref()));
    }
    let spare = public.pop().unwrap_or_default();
    let in_use = public.pop().unwrap_or_default();
    Ok((in_use, spare))
}

/// A private key into a new file that only its owner may read.
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(format!("{text}\n").as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// The key pair in `text` (base64 PKCS#8), in words that say nothing of it when it isn't one.
pub fn key_pair(text: &str) -> Result<Ed25519KeyPair, String> {
    let bad = || format!("{KEY_VARIABLE} isn't an ed25519 private key in base64 PKCS#8 (as `cargo xtask release-key new` writes it)");
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if text.is_empty() || text.len() > MAX_KEY_TEXT {
        return Err(bad());
    }
    let der = STANDARD.decode(text.as_bytes()).map_err(|_| bad())?;
    Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der).map_err(|_| bad())
}

/// Sign `file` with `key` (base64 PKCS#8) into `<file>.sig`; the signature's path and the public key.
pub fn sign(file: &Path, key: &str) -> Result<(PathBuf, String), String> {
    let pair = key_pair(key)?;
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let signature = pair.sign(&bytes);
    let mut sig = file.as_os_str().to_owned();
    sig.push(".sig");
    let sig = PathBuf::from(sig);
    std::fs::write(&sig, format!("{}\n", STANDARD.encode(signature.as_ref()))).map_err(|e| format!("{}: {e}", sig.display()))?;
    Ok((sig, STANDARD.encode(pair.public_key().as_ref())))
}

#[cfg(test)]
mod tests {
    use ring::signature::{ED25519, UnparsedPublicKey};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mailcraft-release-key-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn sign_then_verify() {
        let dir = scratch("roundtrip");
        let (in_use, spare) = new(&dir).unwrap();
        assert_ne!(in_use, spare);
        for public in [&in_use, &spare] {
            assert_eq!(STANDARD.decode(public).unwrap().len(), 32);
        }
        let sums = dir.join("SHA256SUMS.txt");
        std::fs::write(&sums, "0123  mailcraft-9.9.9-linux-x86_64.AppImage\n").unwrap();
        let key = std::fs::read_to_string(dir.join(IN_USE)).unwrap();
        let (sig, public) = sign(&sums, &key).unwrap();
        assert_eq!(public, in_use);
        assert_eq!(sig, dir.join("SHA256SUMS.txt.sig"));
        let text = std::fs::read_to_string(&sig).unwrap();
        assert!(text.ends_with('\n') && text.trim().len() == 88, "{text:?}");
        let signature = STANDARD.decode(text.trim()).unwrap();
        let bytes = std::fs::read(&sums).unwrap();
        let by = |public: &str| UnparsedPublicKey::new(&ED25519, STANDARD.decode(public).unwrap());
        assert!(by(&in_use).verify(&bytes, &signature).is_ok());
        // Not by the spare, and not over anything else.
        assert!(by(&spare).verify(&bytes, &signature).is_err());
        assert!(by(&in_use).verify(b"something else", &signature).is_err());
        // The spare signs too.
        let key = std::fs::read_to_string(dir.join(SPARE)).unwrap();
        let (sig, public) = sign(&sums, &key).unwrap();
        assert_eq!(public, spare);
        let signature = STANDARD.decode(std::fs::read_to_string(&sig).unwrap().trim()).unwrap();
        assert!(by(&spare).verify(&bytes, &signature).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_are_never_written_over_and_are_the_owners_only() {
        let dir = scratch("again");
        new(&dir).unwrap();
        let before = std::fs::read(dir.join(IN_USE)).unwrap();
        assert!(new(&dir).unwrap_err().contains("already has release keys"));
        assert_eq!(std::fs::read(dir.join(IN_USE)).unwrap(), before);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(dir.join(IN_USE)).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_key_that_isnt_one_is_refused_without_saying_it() {
        let dir = scratch("bad");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("SHA256SUMS.txt");
        std::fs::write(&file, "x").unwrap();
        for bad in ["", "not base64 !!", "c2VjcmV0LXRleHQ=", &"A".repeat(5000)] {
            let why = sign(&file, bad).unwrap_err();
            assert!(why.contains(KEY_VARIABLE), "{why}");
            if !bad.is_empty() {
                assert!(!why.contains(bad), "{why}");
            }
        }
        assert!(!dir.join("SHA256SUMS.txt.sig").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
