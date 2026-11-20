// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a job's sign-in is made of, decided with no I/O: which application id a request carries,
//! whose sign-in a token is, which refresh token a job starts with and how long an access token
//! is kept. Secrets are handed in and out as values; none is logged, and `Tokens` has no
//! readable `Debug`.

use mailcraft_model::sync::Tokens;
use mailcraft_model::{Account, OAuth, OAuthProvider};

/// The application ids a build knows: the owner's own choice (File › Options) and the build's.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClientIds<'a> {
    pub microsoft_chosen: &'a str,
    pub microsoft_built_in: &'a str,
    pub google_chosen: &'a str,
    pub google_built_in: &'a str,
}

/// `oauth` with the application id filled in where the account names none: the owner's own
/// choice for the provider, else the build's.
pub fn effective_oauth(mut oauth: OAuth, ids: &ClientIds) -> OAuth {
    if oauth.client_id.is_empty() {
        let (chosen, built_in) = match oauth.provider {
            OAuthProvider::Microsoft => (ids.microsoft_chosen, ids.microsoft_built_in),
            OAuthProvider::Google => (ids.google_chosen, ids.google_built_in),
        };
        oauth.client_id = if chosen.is_empty() { built_in } else { chosen }.to_string();
    }
    oauth
}

/// Whether `tokens` are somebody else's than `account`'s: the provider named who signed in, and it
/// is not the account. A Google identity must match the account's address, not an alternate IMAP
/// user name (that name is plain editable mailbox data and cannot authorise another owner).
pub fn signed_in_as_other(account: &Account, tokens: &Tokens) -> bool {
    if account.oauth.as_ref().is_some_and(|o| o.provider == OAuthProvider::Google) && !tokens.email.trim().is_empty() {
        !tokens.email.trim().eq_ignore_ascii_case(account.email.trim())
    } else {
        tokens.someone_else(&account.email, &account.username)
    }
}

/// The refresh token a job starts with: the one in memory, when there is one, else the one the
/// credential store gave back. Empty when there is none: the owner is to sign in.
pub fn refresh_token(kept: Option<&str>, remembered: Option<String>) -> String {
    kept.filter(|r| !r.is_empty()).map(str::to_string).or(remembered).unwrap_or_default()
}

/// How long an access token is kept in memory: a day at most, and not from before `now`.
pub const KEEP_ACCESS_S: i64 = 86_400;

/// `tokens` as they are kept for the session: the expiry clamped to between `now` and a day on.
pub fn kept_for_session(tokens: Tokens, now: i64) -> Tokens {
    Tokens { expires_at: tokens.expires_at.clamp(now, now.saturating_add(KEEP_ACCESS_S)), ..tokens }
}
