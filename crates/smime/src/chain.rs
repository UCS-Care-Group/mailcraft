// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whether a signer's certificate rests on a trust anchor (RFC 5280 §6, as S/MIME needs it): a
//! path from it through certification authorities' certificates that came with the message, to
//! a certificate the host trusts. Each link's signature is checked, and every certificate on the
//! path must be valid at the time of the check. Bounded: a path is at most [`MAX_PATH`] long and
//! at most [`MAX_SIGNATURES`] signatures are checked looking for one.

use std::collections::HashMap;

use mailcraft_model::digital_signature::Why;
use mailcraft_time::Timestamp;

use crate::algorithms::Fault;
use crate::cert::Cert;

/// The most certificates between a signer's and a trust anchor.
pub const MAX_PATH: usize = 6;
/// The most signatures checked to find a path.
pub const MAX_SIGNATURES: usize = 48;
/// The most trust anchors kept.
pub const MAX_ANCHORS: usize = 4_000;

/// The certificates the host trusts (the system's certification authorities, handed in: this
/// crate never reads a store of its own).
#[derive(Clone, Debug, Default)]
pub struct Anchors {
    certs: Vec<Cert>,
    by_subject: HashMap<Vec<u8>, Vec<usize>>,
}

impl Anchors {
    /// Anchors from certificates in DER; one that isn't a certificate is left out, as is
    /// everything past [`MAX_ANCHORS`].
    pub fn new<I, B>(ders: I) -> Anchors
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        let mut anchors = Anchors::default();
        for der in ders {
            if anchors.certs.len() >= MAX_ANCHORS {
                break;
            }
            if let Some(cert) = Cert::from_der(der.as_ref()) {
                anchors.by_subject.entry(cert.subject().to_vec()).or_default().push(anchors.certs.len());
                anchors.certs.push(cert);
            }
        }
        anchors
    }

    pub fn len(&self) -> usize {
        self.certs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.certs.is_empty()
    }

    fn named(&self, subject: &[u8]) -> impl Iterator<Item = &Cert> {
        self.by_subject.get(subject).into_iter().flatten().filter_map(|i| self.certs.get(*i))
    }

    fn holds(&self, cert: &Cert) -> bool {
        self.named(cert.subject()).any(|a| a == cert)
    }
}

/// Whether `leaf` rests on one of `anchors` through the certificates of `pool`, at `now`.
pub fn trust(leaf: &Cert, pool: &[Cert], anchors: &Anchors, now: Timestamp) -> Result<(), Why> {
    let mut search = Search { pool, anchors, budget: MAX_SIGNATURES, weak: false };
    let mut path = Vec::new();
    if !search.walk(leaf, &mut path) {
        return Err(if search.weak {
            Why::WeakAlgorithm
        } else if leaf.self_issued() {
            Why::SelfSigned
        } else {
            Why::UnknownIssuer
        });
    }
    if !leaf.valid_at(now) || path.iter().any(|c| !c.valid_at(now)) {
        return Err(Why::Expired);
    }
    Ok(())
}

struct Search<'a> {
    pool: &'a [Cert],
    anchors: &'a Anchors,
    budget: usize,
    /// A link was signed with an algorithm that's no longer safe.
    weak: bool,
}

impl<'a> Search<'a> {
    /// Whether `cert` rests on an anchor; `path` gets the certificates above it, the anchor last.
    fn walk(&mut self, cert: &'a Cert, path: &mut Vec<&'a Cert>) -> bool {
        // A certificate the host trusts itself.
        if self.anchors.holds(cert) {
            return true;
        }
        for anchor in self.anchors.named(cert.issuer()) {
            if self.signed(cert, anchor) {
                path.push(anchor);
                return true;
            }
        }
        if path.len() >= MAX_PATH {
            return false;
        }
        for issuer in self.pool.iter().filter(|c| c.subject() == cert.issuer() && *c != cert && c.is_ca()) {
            // (No certificate twice on a path.)
            if path.contains(&issuer) || !self.signed(cert, issuer) {
                continue;
            }
            path.push(issuer);
            if self.walk(issuer, path) {
                return true;
            }
            path.pop();
        }
        false
    }

    fn signed(&mut self, cert: &Cert, by: &Cert) -> bool {
        if self.budget == 0 {
            return false;
        }
        self.budget -= 1;
        match cert.signed_by(by) {
            Ok(()) => true,
            Err(Fault::Weak) => {
                self.weak = true;
                false
            }
            Err(_) => false,
        }
    }
}
