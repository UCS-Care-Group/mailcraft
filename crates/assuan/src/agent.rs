// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! gpg-agent's commands, as its documentation (the GnuPG manual's "Agent Protocol") gives them.

use std::io::{Read, Write};

use crate::client::{Answer, Client, Inquiry, Response};
use crate::line::push_escaped;
use crate::sexp::{self, Sexp};
use crate::{Error, ErrorKind, MAX_LINE};

/// Most keys [`GpgAgent::list_keys`] hands back.
pub const MAX_KEYS: usize = 1024;
/// Longest description of what a key is wanted for ([`GpgAgent::sign`], [`GpgAgent::decrypt`]).
pub const MAX_DESCRIPTION: usize = 300;

/// A keygrip: the 20 bytes by which gpg-agent knows a key, written as 40 hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Keygrip(pub [u8; 20]);

impl Keygrip {
    /// From its 40 hex digits (either case).
    pub fn parse(hex: &str) -> Option<Keygrip> {
        let bytes = hex.as_bytes();
        if bytes.len() != 40 {
            return None;
        }
        let mut grip = [0u8; 20];
        for (i, out) in grip.iter_mut().enumerate() {
            let h = crate::line::hex(*bytes.get(i.saturating_mul(2))?)?;
            let l = crate::line::hex(*bytes.get(i.saturating_mul(2).saturating_add(1))?)?;
            *out = (h << 4) | l;
        }
        Some(Keygrip(grip))
    }
}

impl std::fmt::Display for Keygrip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02X}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for Keygrip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keygrip({self})")
    }
}

/// The hashes a digest may be made with. MD5 and SHA-1 are not among them, so they can't be
/// asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlgorithm {
    /// Its number in `SETHASH` (OpenPGP's and libgcrypt's numbering).
    fn number(self) -> u8 {
        match self {
            HashAlgorithm::Sha256 => 8,
            HashAlgorithm::Sha384 => 9,
            HashAlgorithm::Sha512 => 10,
        }
    }

    /// The size of a digest.
    pub fn digest_len(self) -> usize {
        match self {
            HashAlgorithm::Sha256 => 32,
            HashAlgorithm::Sha384 => 48,
            HashAlgorithm::Sha512 => 64,
        }
    }
}

/// A digest to be signed: its hash and its bytes, of the hash's size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    algorithm: HashAlgorithm,
    bytes: Vec<u8>,
}

impl Digest {
    /// `None` when `bytes` isn't the size `algorithm` makes.
    pub fn new(algorithm: HashAlgorithm, bytes: &[u8]) -> Option<Digest> {
        (bytes.len() == algorithm.digest_len()).then(|| Digest { algorithm, bytes: bytes.to_vec() })
    }

    pub fn algorithm(&self) -> HashAlgorithm {
        self.algorithm
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Where gpg-agent keeps a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStorage {
    /// In a file on this computer.
    Disk,
    /// On a smartcard (or a token like one).
    Smartcard,
    /// The agent doesn't say.
    Unknown,
}

/// Whether a key on disk has a passphrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    Protected,
    Clear,
    Unknown,
}

/// What `KEYINFO` says of a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInfo {
    pub keygrip: Keygrip,
    pub storage: KeyStorage,
    /// The smartcard's serial number, for a key on one.
    pub card_serial: Option<String>,
    /// The key's name on the card (`OPENPGP.1`…), for a key on one.
    pub card_key: Option<String>,
    /// Whether its passphrase is held by the agent now (so no pinentry will show).
    pub passphrase_cached: bool,
    pub protection: Protection,
}

/// Parse the arguments of a `KEYINFO` status line:
/// `<keygrip> <type> <serialno> <idstr> <cached> <protection> …`.
fn key_info(args: &str) -> Option<KeyInfo> {
    let mut fields = args.split(' ').filter(|f| !f.is_empty());
    let keygrip = Keygrip::parse(fields.next()?)?;
    let storage = match fields.next()? {
        "D" => KeyStorage::Disk,
        "T" => KeyStorage::Smartcard,
        _ => KeyStorage::Unknown,
    };
    let text = |f: Option<&str>| f.filter(|f| *f != "-").map(|f| crate::tidy(f.as_bytes(), 64)).filter(|f| !f.is_empty());
    let card_serial = text(fields.next());
    let card_key = text(fields.next());
    let passphrase_cached = fields.next() == Some("1");
    let protection = match fields.next() {
        Some("P") => Protection::Protected,
        Some("C") => Protection::Clear,
        _ => Protection::Unknown,
    };
    Some(KeyInfo { keygrip, storage, card_serial, card_key, passphrase_cached, protection })
}

/// A signature as gpg-agent made it: `(sig-val (<algorithm> (<name> <value>)…))`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// The algorithm's name as the agent gives it (`rsa`, `ecdsa`, `eddsa`…).
    pub algorithm: String,
    /// The whole S-expression (the `sig-val` list).
    pub sexp: Sexp,
}

impl Signature {
    /// A part of the signature by its name (`s` for RSA; `r` and `s` for ECDSA and EdDSA).
    pub fn part(&self, name: &str) -> Option<&[u8]> {
        self.sexp.find(&self.algorithm)?.value(name)
    }

    fn read(data: &[u8]) -> Result<Signature, Error> {
        let sexp = sexp::parse(data).map_err(|e| Error::Protocol(format!("a signature that can't be read ({e})")))?;
        if sexp.name() != Some("sig-val") {
            return Err(Error::Protocol("a signature that isn't a sig-val".into()));
        }
        let algorithm = sexp
            .as_list()
            .and_then(|items| items.get(1))
            .and_then(Sexp::name)
            .map(str::to_string)
            .ok_or_else(|| Error::Protocol("a signature that names no algorithm".into()))?;
        Ok(Signature { algorithm, sexp })
    }
}

/// A session key gpg-agent decrypted. Wiped from memory when dropped; `Debug` shows its size alone.
#[derive(PartialEq, Eq)]
pub struct SessionKey {
    bytes: Vec<u8>,
    padding: Option<u32>,
}

impl SessionKey {
    /// The decrypted value as the agent handed it over (for OpenPGP: the algorithm octet, the
    /// key and its checksum, or what the public-key algorithm gives).
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The `PADDING` the agent said it took off, when it said one.
    pub fn padding(&self) -> Option<u32> {
        self.padding
    }
}

impl std::fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SessionKey(<{} bytes>)", self.bytes.len())
    }
}

impl Drop for SessionKey {
    fn drop(&mut self) {
        crate::wipe(&mut self.bytes);
    }
}

/// What the agent's pinentry needs to show on the owner's screen, passed with `OPTION`. Each is
/// the value of the environment variable of the same kind; none needs to be given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PinentryHints {
    /// `DISPLAY`.
    pub display: Option<String>,
    /// The terminal (`GPG_TTY`), for a pinentry in one.
    pub ttyname: Option<String>,
    /// `TERM`.
    pub ttytype: Option<String>,
    /// `XAUTHORITY`.
    pub xauthority: Option<String>,
    /// `LC_CTYPE`.
    pub lc_ctype: Option<String>,
    /// `LC_MESSAGES`.
    pub lc_messages: Option<String>,
    /// Other variables the pinentry is given (`OPTION putenv=NAME=VALUE`): `WAYLAND_DISPLAY`,
    /// `XDG_SESSION_TYPE`, `DBUS_SESSION_BUS_ADDRESS`.
    pub putenv: Vec<(String, String)>,
}

impl PinentryHints {
    /// The variables putenv may pass on.
    pub const PUTENV: &'static [&'static str] = &["WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "DBUS_SESSION_BUS_ADDRESS"];

    /// The hints from an environment, `var` reading one variable.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> PinentryHints {
        PinentryHints {
            display: var("DISPLAY"),
            ttyname: var("GPG_TTY"),
            ttytype: var("TERM"),
            xauthority: var("XAUTHORITY"),
            lc_ctype: var("LC_CTYPE"),
            lc_messages: var("LC_MESSAGES"),
            putenv: Self::PUTENV.iter().filter_map(|name| var(name).map(|v| ((*name).to_string(), v))).collect(),
        }
    }

    /// The `OPTION` lines, each value checked: one that is empty, holds a control character or
    /// would make the line too long is left out.
    fn options(&self) -> Vec<String> {
        let ok = |v: &str| !v.is_empty() && !v.chars().any(char::is_control) && v.len() < MAX_LINE.saturating_sub(64);
        let mut out = Vec::new();
        for (name, value) in [
            ("display", &self.display),
            ("ttyname", &self.ttyname),
            ("ttytype", &self.ttytype),
            ("xauthority", &self.xauthority),
            ("lc-ctype", &self.lc_ctype),
            ("lc-messages", &self.lc_messages),
        ] {
            if let Some(v) = value.as_deref().filter(|v| ok(v)) {
                out.push(format!("OPTION {name}={v}"));
            }
        }
        for (name, value) in &self.putenv {
            let name_ok = !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
            if name_ok && ok(value) {
                out.push(format!("OPTION putenv={name}={value}"));
            }
        }
        out
    }
}

/// `SETKEYDESC`'s argument: spaces as `+`; `+`, `%` and control characters escaped.
fn key_description(text: &str) -> Result<String, Error> {
    let mut out = Vec::new();
    for b in text.bytes() {
        match b {
            b' ' => out.push(b'+'),
            b'+' | b'%' | 0..=0x1F | 0x7F => push_escaped(&mut out, b),
            _ => out.push(b),
        }
    }
    if text.len() > MAX_DESCRIPTION {
        return Err(Error::Refused("a description of the key's use that is too long".into()));
    }
    // Only ASCII was escaped, so the UTF-8 of the rest is whole.
    String::from_utf8(out).map_err(|_| Error::Refused("a description that isn't text".into()))
}

/// The answer to inquiries during a signature or a decryption: the agent saying it started its
/// pinentry gets an empty answer, `CIPHERTEXT` gets the ciphertext, and anything else is refused.
fn answer(ciphertext: Option<&[u8]>) -> impl FnMut(&Inquiry) -> Answer + '_ {
    move |inquiry: &Inquiry| match (inquiry.keyword.as_str(), ciphertext) {
        ("PINENTRY_LAUNCHED", _) => Answer::Data(Vec::new()),
        ("CIPHERTEXT", Some(c)) => Answer::Data(c.to_vec()),
        _ => Answer::Cancel,
    }
}

/// A connection to gpg-agent.
///
/// Each call is one or a few commands and waits for the agent's answer, which for a signature or
/// a decryption may be while the owner types a PIN into the agent's pinentry: call it away from
/// the window's thread. A call that fails with anything but the agent's refusal leaves the
/// connection unusable ([`Error::Broken`]); open a new one.
#[derive(Debug)]
pub struct GpgAgent<S> {
    client: Client<S>,
}

impl<S: Read + Write> GpgAgent<S> {
    /// Take over a fresh connection to the agent (its greeting is read).
    pub fn open(stream: S) -> Result<Self, Error> {
        Ok(GpgAgent { client: Client::connect(stream)? })
    }

    /// Tell the agent where its pinentry is to show. An option the agent refuses is skipped (an
    /// older agent may not know it).
    pub fn set_pinentry_hints(&mut self, hints: &PinentryHints) -> Result<(), Error> {
        for option in hints.options() {
            match self.client.command(&option) {
                Ok(_) | Err(Error::Server(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// The agent's version (`GETINFO version`).
    pub fn version(&mut self) -> Result<String, Error> {
        let response = self.client.command("GETINFO version")?;
        Ok(crate::tidy(&response.data, 64))
    }

    /// Whether the agent holds the secret key for `keygrip` (`HAVEKEY`).
    pub fn have_key(&mut self, keygrip: &Keygrip) -> Result<bool, Error> {
        match self.client.command(&format!("HAVEKEY {keygrip}")) {
            Ok(_) => Ok(true),
            Err(Error::Server(e)) if matches!(e.kind(), ErrorKind::NoSecretKey | ErrorKind::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// What the agent says of the key for `keygrip` (`KEYINFO`); `None` when it has none.
    pub fn key_info(&mut self, keygrip: &Keygrip) -> Result<Option<KeyInfo>, Error> {
        let response = match self.client.command(&format!("KEYINFO {keygrip}")) {
            Ok(r) => r,
            Err(Error::Server(e)) if matches!(e.kind(), ErrorKind::NoSecretKey | ErrorKind::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let info = response.status.iter().filter(|s| s.keyword == "KEYINFO").find_map(|s| key_info(&s.args)).filter(|i| i.keygrip == *keygrip);
        info.map(Some).ok_or_else(|| Error::Protocol("no KEYINFO for the key asked about".into()))
    }

    /// Every key the agent holds (`KEYINFO --list`), at most [`MAX_KEYS`]; status lines that
    /// can't be read are skipped.
    pub fn list_keys(&mut self) -> Result<Vec<KeyInfo>, Error> {
        let response = self.client.command("KEYINFO --list")?;
        Ok(response.status.iter().filter(|s| s.keyword == "KEYINFO").filter_map(|s| key_info(&s.args)).take(MAX_KEYS).collect())
    }

    /// The public key for `keygrip`, as an S-expression (`READKEY`): `(public-key (rsa (n …)
    /// (e …)))`, `(public-key (ecc (curve …) (q …)))`….
    pub fn read_key(&mut self, keygrip: &Keygrip) -> Result<Sexp, Error> {
        let response = self.client.command(&format!("READKEY {keygrip}"))?;
        sexp::parse(&response.data).map_err(|e| Error::Protocol(format!("a public key that can't be read ({e})")))
    }

    /// Sign `digest` with the key for `keygrip` (`SIGKEY`, `SETKEYDESC`, `SETHASH`, `PKSIGN`).
    /// The agent asks for the PIN or passphrase in its own pinentry, which shows `description`
    /// (what the key is wanted for) when one is given; the owner cancelling it is
    /// [`ErrorKind::Canceled`].
    pub fn sign(&mut self, keygrip: &Keygrip, digest: &Digest, description: Option<&str>) -> Result<Signature, Error> {
        let description = description.map(key_description).transpose()?;
        self.client.command(&format!("SIGKEY {keygrip}"))?;
        if let Some(d) = description {
            self.client.command(&format!("SETKEYDESC {d}"))?;
        }
        let hex: String = digest.bytes.iter().map(|b| format!("{b:02X}")).collect();
        self.client.command(&format!("SETHASH {} {hex}", digest.algorithm.number()))?;
        let response = self.client.transact("PKSIGN", &mut answer(None))?;
        Signature::read(&response.data)
    }

    /// Decrypt `ciphertext` (an S-expression `(enc-val (<algorithm> (<name> <value>)…))`) with the
    /// key for `keygrip` (`SETKEY`, `SETKEYDESC`, `PKDECRYPT`, which asks for the ciphertext with
    /// `INQUIRE CIPHERTEXT`). The PIN or passphrase is asked for as for [`GpgAgent::sign`].
    pub fn decrypt(&mut self, keygrip: &Keygrip, ciphertext: &Sexp, description: Option<&str>) -> Result<SessionKey, Error> {
        if ciphertext.name() != Some("enc-val") {
            return Err(Error::Refused("a ciphertext that isn't an enc-val".into()));
        }
        let description = description.map(key_description).transpose()?;
        let canonical = ciphertext.to_canonical();
        self.client.command(&format!("SETKEY {keygrip}"))?;
        if let Some(d) = description {
            self.client.command(&format!("SETKEYDESC {d}"))?;
        }
        let mut response: Response = self.client.transact("PKDECRYPT", &mut answer(Some(&canonical)))?;
        let padding = response.status("PADDING").and_then(|p| p.trim().parse::<u32>().ok());
        let data = std::mem::take(&mut response.data);
        let parsed = sexp::parse(&data);
        let mut data = data;
        crate::wipe(&mut data);
        let value = parsed.map_err(|e| Error::Protocol(format!("a decrypted value that can't be read ({e})")))?;
        // `(value <bytes>)`.
        let bytes = match value.as_list() {
            Some([name, v]) if name.as_atom() == Some(b"value") => v.as_atom().map(<[u8]>::to_vec),
            _ => None,
        };
        let bytes = bytes.ok_or_else(|| Error::Protocol("a decrypted value that isn't a (value …)".into()))?;
        Ok(SessionKey { bytes, padding })
    }

    /// Forget the key chosen and the digest set (`RESET`).
    pub fn reset(&mut self) -> Result<(), Error> {
        self.client.command("RESET").map(drop)
    }

    /// The stream, for its owner to set up.
    pub fn get_mut(&mut self) -> &mut S {
        self.client.get_mut()
    }

    /// Say goodbye (`BYE`) and hand the stream back.
    pub fn close(mut self) -> S {
        let _ = self.client.command("BYE");
        self.client.into_inner()
    }
}
