// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Trust for one account's server by certificate fingerprint (#252, E5): a verifier that accepts
//! what the system trusts, or else exactly the one certificate whose SHA-256 fingerprint the
//! owner chose to trust for the account.
//!
//! It is the system's own verifier ([`crate::tls`], `rustls-platform-verifier`) with one extra
//! way in, and the extra way is narrow:
//!
//! - It opens only for two errors of the system's verifier: `UnknownIssuer` and `Other`. What
//!   the verifier says varies by system. On Linux and FreeBSD (webpki over the system's store) a
//!   chain with no trusted root is exactly `UnknownIssuer`, and a certificate not made for
//!   servers (`InvalidPurpose`) arrives as an `Other` whose text says "invalid extensions"; that
//!   one is final here. On macOS most trust errors arrive as `Other`, and on Windows an
//!   untrusted root, a bad chain signature and revocation that could not be looked up do too;
//!   those cannot be told apart from an unknown issuer there, so for them the owner's choice of
//!   exactly this certificate is the authority (the chain above the certificate is not what is
//!   trusted: the certificate is). A bad signature, a revocation, a purpose and protocol errors
//!   that the verifier names as such stay final.
//! - Whatever error the system gave, the dates and the name are checked here again, from the
//!   certificate itself. On macOS and Windows that re-check is the only thing that keeps an
//!   expired, not-yet-valid or wrong-name certificate from being accepted by its fingerprint.
//! - It compares the fingerprint of the end-entity certificate, the one the server shows first,
//!   with the one the owner chose (all 32 bytes; a fingerprint is public, so no care over
//!   timing). The server must still prove it holds the certificate's key: the handshake
//!   signature is checked by the system's algorithms, as for any certificate (a test presents
//!   the pinned certificate and signs with another key).
//! - It is never a switch that turns checking off. No fingerprint set means the system alone
//!   decides, and the verifier then only notes what it saw.
//!
//! When the certificate is not accepted, the verifier notes why ([`CertificateProblem`]) for the
//! caller to read after the handshake fails: the caller shows setup what it needs (fingerprint,
//! who it was issued to and by, dates), or says plainly that the certificate can't be trusted.

use std::cell::Cell;
use std::fmt;
use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::{CertificateError, DigitallySignedStruct, Error, SignatureScheme};

use crate::cert::{self, CertificateFault, CertificateProblem};

thread_local! {
    /// Set while the system's verifier looks at a certificate for a connection that has a
    /// fingerprint chosen for it: the verifier is then expected to refuse the certificate, and
    /// the line it logs about that (at error level) is noise (E8).
    static PIN_CHECK: Cell<bool> = const { Cell::new(false) };
}

/// Whether this thread is inside the system verifier's look at a certificate for a connection
/// with a trusted fingerprint. The program's logger leaves out the verifier's own line about a
/// certificate it doesn't trust while this is so; the outcome is reported by the caller.
pub fn pin_check_under_way() -> bool {
    PIN_CHECK.with(Cell::get)
}

/// Clears [`pin_check_under_way`] when the check ends, however it ends.
struct PinCheck(bool);

impl PinCheck {
    fn begin(pinned: bool) -> Self {
        let before = PIN_CHECK.with(|c| c.replace(pinned || c.get()));
        Self(before)
    }
}

impl Drop for PinCheck {
    fn drop(&mut self) {
        PIN_CHECK.with(|c| c.set(self.0));
    }
}

/// The system's verifier with the one extra way in.
pub(crate) struct Pinned {
    system: Arc<dyn ServerCertVerifier>,
    /// The SHA-256 fingerprint of the one certificate trusted besides the system's.
    pin: Option<[u8; 32]>,
    /// Why the last certificate was not accepted.
    noted: Mutex<Option<CertificateProblem>>,
}

impl fmt::Debug for Pinned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pinned").field("pin", &self.pin.is_some()).finish()
    }
}

impl Pinned {
    pub(crate) fn new(system: Arc<dyn ServerCertVerifier>, pin: Option<[u8; 32]>) -> Self {
        Self { system, pin, noted: Mutex::new(None) }
    }

    /// What the verifier noted of the certificate it refused, if it did; read once.
    pub(crate) fn take_problem(&self) -> Option<CertificateProblem> {
        self.noted.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
    }

    fn note(&self, problem: CertificateProblem) {
        *self.noted.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(problem);
    }

    /// What to do with a certificate the system refused with `error`.
    pub(crate) fn refused(
        &self,
        error: Error,
        end_entity: &CertificateDer<'_>,
        server_name: &ServerName<'_>,
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let Some(info) = cert::describe(end_entity.as_ref()) else { return Err(error) };
        // What the system itself says is wrong with the dates or the name is believed.
        let fault = match &error {
            Error::InvalidCertificate(CertificateError::Expired | CertificateError::ExpiredContext { .. }) => Some(CertificateFault::Expired),
            Error::InvalidCertificate(CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. }) => {
                Some(CertificateFault::NotYetValid)
            }
            Error::InvalidCertificate(CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. }) => {
                Some(CertificateFault::WrongName)
            }
            // A certificate not made for servers (webpki's purpose error, as the platform verifier
            // words it on Linux and FreeBSD) is not one a fingerprint changes.
            Error::InvalidCertificate(CertificateError::Other(other)) if other.to_string().contains("invalid extensions") => return Err(error),
            // The system does not trust the chain: the one thing a fingerprint may change.
            Error::InvalidCertificate(CertificateError::UnknownIssuer | CertificateError::Other(_)) => None,
            // Anything else (a bad signature, a revocation, a purpose, a protocol error) is final.
            _ => return Err(error),
        };
        // And the dates and the name are checked again, by this program, whatever the system said.
        let now = i64::try_from(now.as_secs()).unwrap_or(i64::MAX);
        let fault = fault
            .or((now < info.not_before).then_some(CertificateFault::NotYetValid))
            .or((now > info.not_after).then_some(CertificateFault::Expired))
            .or_else(|| match ParsedCertificate::try_from(end_entity) {
                Ok(parsed) => rustls::client::verify_server_name(&parsed, server_name).err().map(|_| CertificateFault::WrongName),
                Err(_) => Some(CertificateFault::WrongName),
            });
        if let Some(fault) = fault {
            self.note(CertificateProblem::Unacceptable(fault, info));
            return Err(error);
        }
        match self.pin {
            Some(pin) if pin == cert::sha256(end_entity.as_ref()) => Ok(ServerCertVerified::assertion()),
            Some(_) => {
                self.note(CertificateProblem::Changed(info));
                Err(error)
            }
            None => {
                self.note(CertificateProblem::NotTrusted(info));
                Err(error)
            }
        }
    }
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let checked = {
            // (Marked only when the certificate shown is the pinned one: the system will refuse it,
            // and that is expected. Another certificate is a real refusal and its line stays.)
            let _quiet = PinCheck::begin(self.pin.is_some_and(|pin| pin == cert::sha256(end_entity.as_ref())));
            self.system.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        };
        match checked {
            Ok(verified) => Ok(verified),
            Err(error) => self.refused(error, end_entity, server_name, now),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.system.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.system.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.system.supported_verify_schemes()
    }
}

#[cfg(test)]
#[path = "pinned_tests.rs"]
mod pinned_tests;
