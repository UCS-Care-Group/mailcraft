//! The credential of a JMAP account whose host holds it ([`mailcraft_ui_egui::Services::jmap_host_credentials`]):
//! the account's job carries no secret, and the runner asks the host for it, and for a new one when the server
//! refuses it (an access token that expired). Built on every target, so that the runner's tests use it natively.
// Outside the browser only the tests use it.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use mailcraft_jmap::Auth;

/// What the host gave when asked for a new credential. The build's own host holds none
/// ([`NoCredentials`]) and answers `Reload`; a page that embeds the build and holds credentials answers
/// with the others, which only it (and the tests) builds.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub enum Renewal {
    /// A new credential: the runner signs in again with it.
    Given(Auth),
    /// Nothing yet: the runner asks again a little later.
    Later,
    /// Nothing will come: only starting again (reloading the page) gives access again.
    Reload,
}

/// The credentials of the accounts a host holds, by the accounts' addresses.
#[allow(async_fn_in_trait)]
pub trait Credentials {
    /// The credential that opens the server of the account `email`, if the host has one.
    fn auth(&self, email: &str) -> Option<Auth>;
    /// The server refused the credential of the account `email`: a new one, or what to do instead.
    async fn renew(&self, email: &str) -> Renewal;
}

/// A host that holds no credential: every job carries the owner's own.
pub struct NoCredentials;

impl Credentials for NoCredentials {
    fn auth(&self, _email: &str) -> Option<Auth> {
        None
    }

    async fn renew(&self, _email: &str) -> Renewal {
        Renewal::Reload
    }
}
