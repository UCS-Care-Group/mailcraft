// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's GnuPG, asked two things (#30 R6, R13): which secret keys it has, and which User
//! IDs it holds at full or ultimate validity under its direct trust model ([`VALID`]: the
//! owner's own word on each key, never the web of trust). `gpg` is found through `gpgconf --list-components`
//! (`gpgconf` itself as `gpg_agent` finds it), run as a program with fixed arguments, never
//! through a shell, for at most [`TIMEOUT`] and [`MAX_OUTPUT`] bytes, and its colon listing
//! (GnuPG's `doc/DETAILS`) read for the few fields needed.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mailcraft_engine::model::digital_signature::SignatureFormat;
use mailcraft_ui_egui::own_keys::{Gnupg, KeyPlace, MAX_ADDRESSES, MAX_LISTED, OwnKey};

/// How long `gpgconf` or `gpg` may take.
const TIMEOUT: Duration = Duration::from_secs(20);
/// Most bytes of a listing read; a keyring that lists more is told as not working.
const MAX_OUTPUT: usize = 4 << 20;
/// Most valid (fingerprint, address) pairs taken.
const MAX_VALID: usize = 4096;
const LIST: [&str; 4] = ["--batch", "--no-tty", "--with-colons", "--fixed-list-mode"];
/// What the listing of valid User IDs adds: GnuPG's direct trust model, whatever its options
/// say, so that a key is valid only by what the owner set on that very key (its ownertrust, or
/// their own key's ultimate trust), never through the web of trust, TOFU or `trust-model
/// always`. (Under it GnuPG lists such a key's User IDs `f` or `u`, and every other `-`, `q`,
/// `m`, `n` or `e`; checked with a keyring of synthetic keys, GnuPG 2.4.)
pub const VALID: [&str; 2] = ["--trust-model=direct", "--list-keys"];

/// GnuPG's `gpg`.
pub struct Gpg(PathBuf);

/// `gpg`, when GnuPG is installed and its `gpgconf` names it.
pub fn find() -> Result<Gpg, Gnupg> {
    let gpgconf = crate::gpg_agent::find_gpgconf(&crate::gpg_agent::gpgconf_candidates(&|n| std::env::var_os(n))).ok_or(Gnupg::Missing)?;
    let out = run(&gpgconf, &["--list-components"]).map_err(Gnupg::NotWorking)?;
    gpg_in(&out).map(Gpg).ok_or_else(|| Gnupg::NotWorking("gpgconf names no gpg".into()))
}

/// The pairs, or none when GnuPG isn't installed.
pub fn valid() -> Result<Vec<(String, String)>, String> {
    match find() {
        Ok(gpg) => Ok(valid_in(&run(&gpg.0, &[&LIST[..], &VALID[..]].concat())?)),
        Err(Gnupg::NotWorking(why)) => Err(why),
        Err(_) => Ok(Vec::new()),
    }
}

impl Gpg {
    pub fn secret_keys(&self) -> Result<Vec<OwnKey>, String> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        Ok(secret_in(&run(&self.0, &[&LIST[..], &["--list-secret-keys"]].concat())?, now))
    }
}

/// `gpg`'s path in `gpgconf --list-components` (`gpg:OpenPGP:/usr/bin/gpg`, a colon in it
/// written `%3a`): absolute, or nothing.
pub fn gpg_in(out: &[u8]) -> Option<PathBuf> {
    let text = String::from_utf8_lossy(out);
    let line = text.lines().find(|l| l.starts_with("gpg:"))?;
    let path = PathBuf::from(line.splitn(3, ':').nth(2)?.trim_end().replace("%3a", ":").replace("%3A", ":"));
    path.is_absolute().then_some(path)
}

/// A field of the colon listing, `\xNN` escapes undone, control characters left out.
fn field(text: &str) -> String {
    let mut bytes = Vec::with_capacity(text.len());
    let raw = text.as_bytes();
    let mut i = 0usize;
    while let Some(&b) = raw.get(i) {
        let hex =
            raw.get(i.saturating_add(2)..i.saturating_add(4)).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b, raw.get(i.saturating_add(1)), hex) {
            (b'\\', Some(b'x'), Some(byte)) => {
                bytes.push(byte);
                i = i.saturating_add(4);
            }
            _ => {
                bytes.push(b);
                i = i.saturating_add(1);
            }
        }
    }
    String::from_utf8_lossy(&bytes).chars().filter(|c| !c.is_control()).collect()
}

/// The address of a User ID (`Name <a@b.example>`, or a bare address), lower case.
fn address(uid: &str) -> Option<String> {
    let inner = match (uid.rfind('<'), uid.rfind('>')) {
        (Some(open), Some(close)) if open < close => uid.get(open.saturating_add(1)..close)?,
        _ => uid.trim(),
    };
    (inner.contains('@') && !inner.contains(char::is_whitespace) && inner.len() <= 254).then(|| inner.to_lowercase())
}

/// A fingerprint of 40 or 64 hexadecimal digits, in groups of four upper-case.
fn grouped(hex: &str) -> Option<String> {
    (matches!(hex.len(), 40 | 64) && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| {
        let upper = hex.to_ascii_uppercase();
        let groups: Vec<&str> = (0..upper.len()).step_by(4).filter_map(|i| upper.get(i..i.saturating_add(4))).collect();
        groups.join(" ")
    })
}

/// The secret keys in `--list-secret-keys` output: each `sec` record, its first `fpr` and its
/// User IDs.
pub fn secret_in(out: &[u8], now: i64) -> Vec<OwnKey> {
    let text = String::from_utf8_lossy(out);
    let mut keys: Vec<OwnKey> = Vec::new();
    // Whether the record being read belongs to a `sec` (not to an `ssb` after it, for `fpr`).
    let mut primary = false;
    for line in text.lines() {
        let f: Vec<&str> = line.split(':').collect();
        let at = |i: usize| f.get(i).copied().unwrap_or("");
        match at(0) {
            "sec" => {
                if keys.len() >= MAX_LISTED {
                    break;
                }
                let not_after = at(6).parse::<i64>().unwrap_or(0);
                keys.push(OwnKey {
                    format: SignatureFormat::OpenPgp,
                    name: String::new(),
                    addresses: Vec::new(),
                    fingerprint: String::new(),
                    not_after,
                    expired: matches!(at(1), "r" | "e" | "i" | "d") || (not_after != 0 && not_after < now),
                    place: KeyPlace::Gnupg,
                    // ponytail: the key as a whole may sign ('S'); whether that subkey's secret is on a card or missing is gpg-agent's to say at signing.
                    can_sign: at(11).contains('S'),
                });
                primary = true;
            }
            "fpr" if primary => {
                if let Some(key) = keys.last_mut()
                    && let Some(fp) = grouped(at(9))
                {
                    key.fingerprint = fp;
                }
                primary = false;
            }
            "uid" => {
                if let Some(key) = keys.last_mut() {
                    let uid = field(at(9));
                    if key.name.is_empty() {
                        key.name = uid.chars().take(256).collect();
                    }
                    if let Some(a) = address(&uid)
                        && key.addresses.len() < MAX_ADDRESSES
                        && !key.addresses.contains(&a)
                    {
                        key.addresses.push(a);
                    }
                }
            }
            "ssb" | "sub" => primary = false,
            _ => {}
        }
    }
    keys.retain(|k| !k.fingerprint.is_empty());
    keys
}

/// The (fingerprint, address) pairs of `--list-keys` output whose User ID is at full (`f`) or
/// ultimate (`u`) validity, of keys that aren't revoked, expired or disabled.
pub fn valid_in(out: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(out);
    let mut pairs = Vec::new();
    let (mut fingerprint, mut usable, mut primary) = (String::new(), false, false);
    for line in text.lines() {
        let f: Vec<&str> = line.split(':').collect();
        let at = |i: usize| f.get(i).copied().unwrap_or("");
        match at(0) {
            "pub" => {
                fingerprint.clear();
                usable = !matches!(at(1), "r" | "e" | "i" | "d");
                primary = true;
            }
            "fpr" if primary => {
                fingerprint =
                    if matches!(at(9).len(), 40 | 64) && at(9).chars().all(|c| c.is_ascii_hexdigit()) { at(9).to_string() } else { String::new() };
                primary = false;
            }
            "uid" if usable && !fingerprint.is_empty() && matches!(at(1), "f" | "u") => {
                if let Some(a) = address(&field(at(9))) {
                    pairs.push((fingerprint.clone(), a));
                    if pairs.len() >= MAX_VALID {
                        break;
                    }
                }
            }
            "sub" => primary = false,
            _ => {}
        }
    }
    pairs
}

/// Run `program` with `args` (never a shell), its output read to at most [`MAX_OUTPUT`] bytes on
/// a thread of its own, for at most [`TIMEOUT`].
pub fn run(program: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    run_within(program, args, TIMEOUT, MAX_OUTPUT)
}

/// [`run`] with its limits: stopped (killed) past `timeout`, or as soon as it has written more
/// than `max` bytes.
pub fn run_within(program: &Path, args: &[&str], timeout: Duration, max: usize) -> Result<Vec<u8>, String> {
    let name = program.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashes up for it.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| format!("{name} couldn't be started ({})", e.kind()))?;
    let (sender, output) = mpsc::channel();
    if let Some(mut stdout) = child.stdout.take() {
        let spawned = std::thread::Builder::new().name("gnupg listing".into()).spawn(move || {
            let mut out = Vec::new();
            let read = (&mut stdout).take(u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1)).read_to_end(&mut out);
            let _ = sender.send(read.map(|_| out));
        });
        if spawned.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{name}'s output couldn't be read"));
        }
    }
    let too_much = || format!("{name} listed more than MailCraft reads");
    let started = Instant::now();
    let mut read: Option<std::io::Result<Vec<u8>>> = None;
    let status = loop {
        // (Output past the cap stops it at once, rather than leaving it blocked on a full pipe
        // until the time is up.)
        if read.is_none()
            && let Ok(got) = output.try_recv()
        {
            if got.as_ref().is_ok_and(|out| out.len() > max) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(too_much());
            }
            read = Some(got);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{name} didn't finish in time"));
            }
        }
    };
    let read = match read {
        Some(read) => Some(read),
        None => output.recv_timeout(timeout.saturating_sub(started.elapsed()).max(Duration::from_millis(200))).ok(),
    };
    match read {
        Some(Ok(out)) if out.len() > max => Err(too_much()),
        _ if !status.success() => Err(match status.code() {
            Some(code) => format!("{name} ended with error {code}"),
            None => format!("{name} was stopped"),
        }),
        Some(Ok(out)) => Ok(out),
        _ => Err(format!("{name}'s output didn't end")),
    }
}
