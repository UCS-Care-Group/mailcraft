// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The authorities trusted for mail in Mozilla's list of roots as NSS builds it into its own
//! PKCS#11 module, `libnssckbi.so` (#30): where the system extracts no bundle for mail (Debian,
//! Ubuntu, Linux Mint, FreeBSD's ports), its `libnss3` still carries that list with Mozilla's trust
//! for each purpose (web sites, mail, code), which the PEM bundles of `ca-certificates` drop.
//!
//! - **Only the real thing.** The module is loaded from the places the distributions put it, never
//!   from a path a message, a command or the environment names, and taken only when its token is
//!   NSS's own (`Builtin Object Token`): p11-kit's trust module, which some systems put in its
//!   place, has the system's set under another name and no trust for mail of its own.
//! - **Mail's trust alone.** A root counts when a trust object of it says it is an anchor for
//!   e-mail protection, in either form NSS writes: its own (`CKO_NSS_TRUST`,
//!   `CKA_NSS_TRUST_EMAIL_PROTECTION` = `CKT_NSS_TRUSTED_DELEGATOR`) or PKCS #11 3.2's (`CKO_TRUST`,
//!   `CKA_TRUST_EMAIL_PROTECTION` = `CKT_TRUST_ANCHOR`, NSS 3.114 and after), matched to the
//!   root by its issuer and serial number (two certificates of the module that share both count
//!   for neither, so no hash is needed to tell them apart). A root any trust
//!   object distrusts for mail doesn't count, whatever another says, and nor does one Mozilla
//!   distrusts for mail after a date (`CKA_NSS_EMAIL_DISTRUST_AFTER`): the check can't tell when a
//!   certificate under it was issued, so it is left out whole.
//! - **Distrust read whole or nothing.** A search for distrust that fails, a distrust object that
//!   can't be read whole, or more of them than are read, and the module gives no roots for mail at
//!   all: a list whose distrust is not known in full could trust a root Mozilla distrusts. (An
//!   anchor that can't be read is only left out: that trusts less, never more.)
//! - **Bounded.** At most [`MAX_OBJECTS`] objects of each kind are read, and what comes back is
//!   held to the sizes [`super::system_trust_anchors`] keeps.

use std::ffi::c_ulong;
use std::path::Path;

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error, RvError};
use cryptoki::object::{Attribute, AttributeType, CertificateType, ObjectClass, ObjectHandle};
use cryptoki::session::Session;

/// Where the distributions put NSS's module of roots: the multiarch places of Debian and its
/// derivatives, the older `nss/` subfolder, the 64-bit and plain places of the others, and
/// FreeBSD's `security/nss` port (`lib/libnssckbi.so` under its prefix). (One of another architecture doesn't load, and the next is tried.)
pub const NSS_MODULES: [&str; 12] = [
    "/usr/lib/x86_64-linux-gnu/libnssckbi.so",
    "/usr/lib/aarch64-linux-gnu/libnssckbi.so",
    "/usr/lib/riscv64-linux-gnu/libnssckbi.so",
    "/usr/lib/powerpc64le-linux-gnu/libnssckbi.so",
    "/usr/lib/s390x-linux-gnu/libnssckbi.so",
    "/usr/lib/arm-linux-gnueabihf/libnssckbi.so",
    "/usr/lib/i386-linux-gnu/libnssckbi.so",
    "/usr/lib/x86_64-linux-gnu/nss/libnssckbi.so",
    "/usr/lib64/libnssckbi.so",
    "/usr/lib/libnssckbi.so",
    "/usr/lib/nss/libnssckbi.so",
    "/usr/local/lib/libnssckbi.so",
];

/// The label of the token NSS's module of roots has (padded with spaces, as PKCS #11 has it).
pub(crate) const BUILTIN_TOKEN: &str = "Builtin Object Token";
/// Most objects of one kind read from the module (it holds a few hundred).
pub(crate) const MAX_OBJECTS: usize = 4_000;
/// Most slots looked at for the token.
const MAX_SLOTS: usize = 8;

// NSS's own constants (`pkcs11n.h`): its vendor number in the vendor-defined ranges.
const NSS: c_ulong = 0x8000_0000 | 0x4E53_4350;
/// `CKO_NSS_TRUST`.
pub(crate) const CKO_NSS_TRUST: c_ulong = NSS + 3;
/// `CKA_NSS_TRUST_EMAIL_PROTECTION` (`CKA_NSS_TRUST_BASE + 11`).
pub(crate) const CKA_NSS_TRUST_EMAIL_PROTECTION: c_ulong = NSS + 0x2000 + 11;
/// `CKA_NSS_EMAIL_DISTRUST_AFTER`: on a certificate, `CK_FALSE` or the time after which what it
/// issued isn't trusted for mail.
pub(crate) const CKA_NSS_EMAIL_DISTRUST_AFTER: c_ulong = NSS + 36;
/// `CKT_NSS_TRUSTED_DELEGATOR`: an anchor for the purpose.
pub(crate) const CKT_NSS_TRUSTED_DELEGATOR: c_ulong = NSS + 2;
/// `CKT_NSS_NOT_TRUSTED`: distrusted for the purpose.
pub(crate) const CKT_NSS_NOT_TRUSTED: c_ulong = NSS + 10;

// PKCS #11 3.2's (`pkcs11t.h`).
/// `CKO_TRUST`.
pub(crate) const CKO_TRUST: c_ulong = 11;
/// `CKA_TRUST_EMAIL_PROTECTION`.
pub(crate) const CKA_TRUST_EMAIL_PROTECTION: c_ulong = 1_583;
/// `CKT_TRUST_ANCHOR`.
pub(crate) const CKT_TRUST_ANCHOR: c_ulong = 2;
/// `CKT_NOT_TRUSTED`.
pub(crate) const CKT_NOT_TRUSTED: c_ulong = 3;

/// A root certificate of the module, as far as choosing it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Root {
    /// The issuer's name and the serial number, DER, as the module gives them: what a trust
    /// object names it by.
    pub issuer: Vec<u8>,
    pub serial: Vec<u8>,
    /// The certificate, DER.
    pub der: Vec<u8>,
    /// Mozilla distrusts what it issued for mail after a date.
    pub email_distrust_after: bool,
}

/// What a trust object says of mail (anything else it may say is never asked for).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Email {
    /// An anchor for e-mail protection.
    Anchor,
    /// Distrusted for it.
    Distrusted,
}

/// A trust object of the module, in either form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Trust {
    pub issuer: Vec<u8>,
    pub serial: Vec<u8>,
    pub email: Email,
}

impl Trust {
    /// Whether it is for `root`: the same issuer and serial number.
    fn is_for(&self, root: &Root) -> bool {
        self.issuer == root.issuer && self.serial == root.serial
    }
}

/// The searches that find what the module says of mail: for each form, the class, the attribute
/// of trust for e-mail protection, the value and what it means. (The module is asked for the
/// objects with that value rather than read: `cryptoki` reads no attribute of PKCS #11 3.2's
/// trust objects, but hands a search's numbers over as they are.)
pub(crate) const SEARCHES: [(c_ulong, c_ulong, c_ulong, Email); 4] = [
    (CKO_NSS_TRUST, CKA_NSS_TRUST_EMAIL_PROTECTION, CKT_NSS_TRUSTED_DELEGATOR, Email::Anchor),
    (CKO_NSS_TRUST, CKA_NSS_TRUST_EMAIL_PROTECTION, CKT_NSS_NOT_TRUSTED, Email::Distrusted),
    (CKO_TRUST, CKA_TRUST_EMAIL_PROTECTION, CKT_TRUST_ANCHOR, Email::Anchor),
    (CKO_TRUST, CKA_TRUST_EMAIL_PROTECTION, CKT_NOT_TRUSTED, Email::Distrusted),
];

/// The roots trusted for mail, DER, each once: an anchor for mail by a trust object for it,
/// distrusted for mail by none, and not distrusted for mail after a date. A root whose issuer and
/// serial number another certificate of the module has too counts not at all: which of the two a
/// trust object is for can't be told, and an issuer never gives two certificates one number.
pub(crate) fn chosen(roots: &[Root], trusts: &[Trust]) -> Vec<Vec<u8>> {
    let shared = |root: &Root| roots.iter().any(|o| o.issuer == root.issuer && o.serial == root.serial && o.der != root.der);
    let mut out: Vec<Vec<u8>> = Vec::new();
    for root in roots.iter().filter(|r| !r.email_distrust_after && !shared(r)) {
        let said: Vec<Email> = trusts.iter().filter(|t| t.is_for(root)).map(|t| t.email).collect();
        if said.contains(&Email::Anchor) && !said.contains(&Email::Distrusted) && !out.contains(&root.der) {
            out.push(root.der.clone());
        }
    }
    out
}

/// Whether `CKA_NSS_EMAIL_DISTRUST_AFTER`'s value names a time: `CK_FALSE` is one byte of 0.
pub(crate) fn distrusts_after(value: &[u8]) -> bool {
    !matches!(value, [] | [0])
}

/// The roots trusted for mail in the first of [`NSS_MODULES`] that is there and NSS's own; `None`
/// when there is none, or it can't be read, or it trusts none for mail.
///
/// Read once for the program, the first time it is asked for: the module is loaded, started,
/// read, finished and let go of by one thread alone. (Two threads at it at once crashed the
/// program: one finished and let go of it while the other was still reading it.) A caller that
/// asks meanwhile waits for that read and has what it found.
pub(crate) fn read() -> Option<Vec<Vec<u8>>> {
    static FOUND: std::sync::OnceLock<Option<Vec<Vec<u8>>>> = std::sync::OnceLock::new();
    FOUND.get_or_init(|| NSS_MODULES.iter().map(Path::new).filter(|p| p.is_file()).find_map(module)).clone()
}

/// The roots trusted for mail in the module at `path`.
fn module(path: &Path) -> Option<Vec<Vec<u8>>> {
    let context = Pkcs11::new(path).map_err(|e| log::debug!("NSS's list of roots couldn't be loaded: {e}")).ok()?;
    // (Initialised by somebody else in this process already: theirs to finalise.)
    let ours = match context.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
        Ok(()) => true,
        Err(Error::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => false,
        Err(e) => {
            log::debug!("NSS's list of roots couldn't be started: {e}");
            return None;
        }
    };
    let found = builtins(&context);
    if ours && let Err(e) = context.finalize() {
        log::debug!("NSS's list of roots couldn't be closed: {e}");
    }
    found.filter(|f| !f.is_empty())
}

/// The roots trusted for mail on the module's own token, when it has one.
fn builtins(context: &Pkcs11) -> Option<Vec<Vec<u8>>> {
    let slots = context.get_slots_with_token().ok()?;
    let slot = slots.into_iter().take(MAX_SLOTS).find(|slot| context.get_token_info(*slot).is_ok_and(|t| t.label().trim_end() == BUILTIN_TOKEN));
    let Some(slot) = slot else {
        log::info!("the module of roots at the NSS place isn't NSS's own: it is not read for mail");
        return None;
    };
    let session = context.open_ro_session(slot).ok()?;
    let roots = roots(&session);
    let mut trusts: Vec<Trust> = Vec::new();
    for &(of, purpose, value, means) in &SEARCHES {
        let Some(found) = kept(means, search(&session, of, purpose, value, means)) else {
            log::warn!("NSS's list of roots couldn't say in full what it distrusts for mail: it is not read for mail");
            return None;
        };
        trusts.extend(found);
    }
    Some(chosen(&roots, &trusts))
}

/// A search template's attribute `kind` of the `CK_ULONG` `value`, by their numbers
/// (`ObjectClass` takes only the standard classes, and `cryptoki` knows no trust attribute).
fn ulong(kind: AttributeType, value: c_ulong) -> Attribute {
    Attribute::VendorDefined((kind, value.to_ne_bytes().to_vec()))
}

/// The module's X.509 certificates.
fn roots(session: &Session) -> Vec<Root> {
    let template = [Attribute::Class(ObjectClass::CERTIFICATE), Attribute::CertificateType(CertificateType::X_509)];
    let wanted =
        [AttributeType::Issuer, AttributeType::SerialNumber, AttributeType::Value, AttributeType::VendorDefined(CKA_NSS_EMAIL_DISTRUST_AFTER)];
    let handles = handles(session, &template, MAX_OBJECTS).unwrap_or_default();
    let mut out = Vec::new();
    for handle in handles {
        let Ok(attributes) = session.get_attributes(handle, &wanted) else { continue };
        let mut root = Root { issuer: Vec::new(), serial: Vec::new(), der: Vec::new(), email_distrust_after: false };
        for attribute in attributes {
            match attribute {
                Attribute::Issuer(v) => root.issuer = v,
                Attribute::SerialNumber(v) => root.serial = v,
                Attribute::Value(v) => root.der = v,
                Attribute::VendorDefined((AttributeType::VendorDefined(CKA_NSS_EMAIL_DISTRUST_AFTER), v)) => {
                    root.email_distrust_after = distrusts_after(&v)
                }
                _ => {}
            }
        }
        if !root.der.is_empty() && !root.issuer.is_empty() && !root.serial.is_empty() {
            out.push(root);
        }
    }
    out
}

/// What one of [`SEARCHES`] found, for [`chosen`]: `found` is `None` where the search failed,
/// and holds `None` for an object that couldn't be read whole (at most one past [`MAX_OBJECTS`]
/// is looked at). Anchors are kept as far as they could be read; for distrust anything short of
/// all of it is `None`, and the module is then not read for mail.
pub(crate) fn kept(means: Email, found: Option<Vec<Option<Trust>>>) -> Option<Vec<Trust>> {
    let whole = means == Email::Distrusted;
    let Some(found) = found else { return (!whole).then(Vec::new) };
    if whole && (found.len() > MAX_OBJECTS || found.iter().any(Option::is_none)) {
        return None;
    }
    Some(found.into_iter().take(MAX_OBJECTS).flatten().collect())
}

/// The trust objects of class `of` whose attribute `purpose` is `value`, as saying `means` of
/// mail, as [`kept`] takes them (one past [`MAX_OBJECTS`] at most, to tell that there are more).
fn search(session: &Session, of: c_ulong, purpose: c_ulong, value: c_ulong, means: Email) -> Option<Vec<Option<Trust>>> {
    let template = [ulong(AttributeType::Class, of), ulong(AttributeType::VendorDefined(purpose), value)];
    let wanted = [AttributeType::Issuer, AttributeType::SerialNumber];
    let handles =
        handles(session, &template, MAX_OBJECTS.saturating_add(1)).map_err(|e| log::debug!("a search of NSS's list of roots failed: {e}")).ok()?;
    let read = |handle| {
        let attributes = session.get_attributes(handle, &wanted).ok()?;
        let mut trust = Trust { issuer: Vec::new(), serial: Vec::new(), email: means };
        for attribute in attributes {
            match attribute {
                Attribute::Issuer(v) => trust.issuer = v,
                Attribute::SerialNumber(v) => trust.serial = v,
                _ => {}
            }
        }
        (!trust.issuer.is_empty() && !trust.serial.is_empty()).then_some(trust)
    };
    Some(handles.into_iter().map(read).collect())
}

/// The first `most` objects of the session that match `template`, and the search over before
/// anything of them is read. (`cryptoki` takes the handles from the module ten at a time, so up to
/// nine more may be fetched and let go of.)
fn handles(session: &Session, template: &[Attribute], most: usize) -> cryptoki::error::Result<Vec<ObjectHandle>> {
    session.iter_objects(template)?.take(most).collect()
}
