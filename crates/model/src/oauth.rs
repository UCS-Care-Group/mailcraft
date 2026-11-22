// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The settings of an OAuth 2.0 sign-in beyond who signs the owner in ([`OAuth`]): the values
//! built in for Microsoft and Google, and the owner's own for an identity provider of their own
//! ([`OAuthProvider::Custom`]), with what makes them usable.
//!
//! Nothing here goes anywhere: these are settings, checked as text. The host that holds the
//! sockets uses them (`mailcraft-net`); the tokens and any client secret are never here.
//!
//! **Built in or the owner's.** Microsoft's and Google's addresses, scopes and mechanism are
//! the program's own ([`OAuthProvider::defaults`]) and can't be changed on such an account: only
//! its client id, tenant and redirect port can. An account whose addresses are anybody else's is
//! a [`OAuthProvider::Custom`] one, so that what may be sent to Microsoft's and Google's hosts
//! is never loosened by a setting ([`OAuth::check`]).

use serde::{Deserialize, Serialize};

use crate::{ModelError, OAuth, OAuthProvider};

/// Where Microsoft signs people in.
pub const MICROSOFT_LOGIN: &str = "https://login.microsoftonline.com";
/// What a Microsoft account's mail sign-in asks for: IMAP, SMTP, a refresh token and who signs in.
pub const MICROSOFT_MAIL_SCOPE: &str =
    "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access openid profile email";
/// The mail servers a Microsoft account's token is for (as shown; the host decides by domain).
pub const MICROSOFT_HOSTS: [&str; 3] = ["outlook.office365.com", "smtp.office365.com", "smtp-mail.outlook.com"];
/// Google's identity provider.
pub const GOOGLE_ISSUER: &str = "https://accounts.google.com";
/// Google's sign-in page.
pub const GOOGLE_AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// Google's token endpoint.
pub const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
/// Where Google takes a refresh token back.
pub const GOOGLE_REVOKE: &str = "https://oauth2.googleapis.com/revoke";
/// What a Google account's sign-in asks for: Gmail's IMAP and SMTP, and who signs in.
pub const GOOGLE_SCOPE: &str = "https://mail.google.com/ openid email";
/// The mail servers a Google account's token is for.
pub const GOOGLE_HOSTS: [&str; 2] = ["imap.gmail.com", "smtp.gmail.com"];
/// What an own identity provider is always asked for: who signs in, their address, and a refresh
/// token so that the owner isn't asked again each time a token runs out.
pub const CUSTOM_SCOPE: &str = "openid email offline_access";

/// Longest address of an identity provider, its pages or its endpoints.
pub const MAX_ENDPOINT: usize = 2048;
/// Longest list of extra scopes, in bytes.
pub const MAX_SCOPES: usize = 1024;
/// Most extra scopes.
pub const MAX_SCOPE_COUNT: usize = 32;
/// Longest client id.
pub const MAX_CLIENT_ID: usize = 200;

/// How the mail servers are given an access token (SASL).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SaslChoice {
    /// OAUTHBEARER (RFC 7628) when the server offers it, else XOAUTH2.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    /// OAUTHBEARER only.
    #[serde(rename = "oauthbearer")]
    OAuthBearer,
    /// XOAUTH2 only.
    #[serde(rename = "xoauth2")]
    XOAuth2,
}

impl SaslChoice {
    pub fn is_auto(&self) -> bool {
        *self == SaslChoice::Auto
    }

    pub fn parse(text: &str) -> Option<SaslChoice> {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "auto" | "automatic" => Some(SaslChoice::Auto),
            "oauthbearer" => Some(SaslChoice::OAuthBearer),
            "xoauth2" => Some(SaslChoice::XOAuth2),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            SaslChoice::Auto => "auto",
            SaslChoice::OAuthBearer => "oauthbearer",
            SaslChoice::XOAuth2 => "xoauth2",
        }
    }
}

/// How a client with a secret proves who it is to the token and revocation addresses (RFC 6749
/// §2.3.1). A client without a secret names itself in the form either way (§3.2.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientAuth {
    /// `client_secret_basic`: the client id and secret in the `Authorization` header (HTTP Basic,
    /// each form-encoded first). Every provider must take it (§2.3.1), and it is what a provider
    /// that says nothing takes (RFC 8414 §2).
    #[default]
    #[serde(rename = "basic")]
    Basic,
    /// `client_secret_post`: the two in the form, for a provider that takes only that.
    #[serde(rename = "post")]
    Post,
}

impl ClientAuth {
    pub fn is_basic(&self) -> bool {
        *self == ClientAuth::Basic
    }

    pub fn parse(text: &str) -> Option<ClientAuth> {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "basic" | "client_secret_basic" | "header" => Some(ClientAuth::Basic),
            "post" | "client_secret_post" | "form" | "body" => Some(ClientAuth::Post),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            ClientAuth::Basic => "basic",
            ClientAuth::Post => "post",
        }
    }

    /// The name the standards and providers' descriptions give it.
    pub fn method(self) -> &'static str {
        match self {
            ClientAuth::Basic => "client_secret_basic",
            ClientAuth::Post => "client_secret_post",
        }
    }
}

/// Everything a sign-in uses, as it is used: built in for Microsoft and Google, the account's
/// own for an own identity provider ([`OAuth::effective`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OAuthDefaults {
    pub provider: OAuthProvider,
    /// The identity provider's own address; empty when there is none to show.
    pub issuer: String,
    pub authorize: String,
    pub token: String,
    /// Empty for none.
    pub revoke: String,
    /// Everything the tokens are asked for, separated by spaces.
    pub scopes: String,
    pub mechanism: SaslChoice,
    /// How a client secret is sent, when there is one.
    pub client_auth: ClientAuth,
    /// Whether the provider's page names itself (`iss`) when it sends the owner back (RFC 9207),
    /// so that an answer without it is refused.
    pub issuer_in_response: bool,
    /// The client id: the account's own, or empty for the build's own (Microsoft and Google only).
    pub client_id: String,
    /// Whether the client id used is the build's own.
    pub built_in_client: bool,
    /// The mail servers the token is for, when they are the provider's own; empty for an own
    /// identity provider, whose token goes to the account's own IMAP and SMTP servers only.
    pub hosts: Vec<String>,
    pub redirect_port: Option<u16>,
}

impl OAuthProvider {
    /// The values built in for this provider. `tenant` is Microsoft's directory (empty, or one
    /// that can't be part of an address, reads as `common`); the others have none. An own
    /// identity provider has no addresses built in: only what it is always asked for.
    pub fn defaults(self, tenant: &str) -> OAuthDefaults {
        match self {
            OAuthProvider::Microsoft => {
                let tenant = usable_tenant(tenant).unwrap_or("common");
                OAuthDefaults {
                    provider: self,
                    issuer: format!("{MICROSOFT_LOGIN}/{tenant}/v2.0"),
                    authorize: format!("{MICROSOFT_LOGIN}/{tenant}/oauth2/v2.0/authorize"),
                    token: format!("{MICROSOFT_LOGIN}/{tenant}/oauth2/v2.0/token"),
                    revoke: String::new(),
                    scopes: MICROSOFT_MAIL_SCOPE.into(),
                    mechanism: SaslChoice::XOAuth2,
                    client_auth: ClientAuth::Post,
                    issuer_in_response: false,
                    client_id: String::new(),
                    built_in_client: true,
                    hosts: MICROSOFT_HOSTS.iter().map(|h| h.to_string()).collect(),
                    redirect_port: None,
                }
            }
            OAuthProvider::Google => OAuthDefaults {
                provider: self,
                issuer: GOOGLE_ISSUER.into(),
                authorize: GOOGLE_AUTHORIZE.into(),
                token: GOOGLE_TOKEN.into(),
                revoke: GOOGLE_REVOKE.into(),
                scopes: GOOGLE_SCOPE.into(),
                mechanism: SaslChoice::XOAuth2,
                client_auth: ClientAuth::Post,
                issuer_in_response: false,
                client_id: String::new(),
                built_in_client: true,
                hosts: GOOGLE_HOSTS.iter().map(|h| h.to_string()).collect(),
                redirect_port: None,
            },
            OAuthProvider::Custom => {
                OAuthDefaults { provider: self, scopes: CUSTOM_SCOPE.into(), mechanism: SaslChoice::Auto, ..OAuthDefaults::default() }
            }
        }
    }
}

impl OAuth {
    /// Settings for an own identity provider with nothing filled in yet.
    pub fn custom() -> OAuth {
        OAuth { provider: OAuthProvider::Custom, ..OAuth::default() }
    }

    /// What a sign-in with these settings uses: the provider's built-in values with the
    /// account's client id, tenant and redirect port over them; for an own identity provider,
    /// the account's own values, with [`CUSTOM_SCOPE`] before its extra scopes.
    pub fn effective(&self) -> OAuthDefaults {
        let mut used = self.provider.defaults(&self.tenant);
        let client = self.client_id.trim();
        if !client.is_empty() {
            used.client_id = client.to_string();
            used.built_in_client = false;
        }
        used.redirect_port = self.redirect_port;
        if self.provider == OAuthProvider::Custom {
            used.built_in_client = false;
            used.issuer = self.issuer.trim().to_string();
            used.authorize = self.authorize.trim().to_string();
            used.token = self.token.trim().to_string();
            used.revoke = self.revoke.trim().to_string();
            used.mechanism = self.mechanism;
            used.client_auth = self.client_auth;
            used.issuer_in_response = self.issuer_in_response;
            used.scopes = scope_list(CUSTOM_SCOPE, &self.scopes);
        }
        used
    }

    /// Whether these settings can be saved: every value that is there is one that can be used
    /// (addresses `https://` ones, or `http://` ones on this computer; no user name or fragment
    /// in them; scopes of the characters RFC 6749 §3.3 allows; a client id, tenant and port that
    /// can be sent). An own identity provider needs a client id. Microsoft's and Google's
    /// accounts have no addresses, scopes or mechanism of their own: those are built in.
    ///
    /// It doesn't say whether a sign-in can start: [`OAuth::ready`] does.
    pub fn check(&self) -> Result<(), ModelError> {
        let invalid = |text: String| Err(ModelError::Invalid(text));
        let client = self.client_id.trim();
        if client.len() > MAX_CLIENT_ID || !client.bytes().all(|b| b.is_ascii_graphic()) {
            return invalid("the client id can only be letters, digits and punctuation, at most 200 of them".into());
        }
        if self.redirect_port == Some(0) {
            return invalid("the redirect port must be between 1 and 65535, or automatic".into());
        }
        if self.provider == OAuthProvider::Microsoft && !self.tenant.trim().is_empty() && usable_tenant(&self.tenant).is_none() {
            return invalid("use common, consumers, organizations, or a directory's id or domain as the tenant".into());
        }
        let own = [&self.issuer, &self.authorize, &self.token, &self.revoke, &self.scopes].iter().any(|value| !value.trim().is_empty())
            || !self.mechanism.is_auto()
            || !self.client_auth.is_basic()
            || self.issuer_in_response
            || self.has_secret;
        if self.provider != OAuthProvider::Custom {
            if own {
                return invalid(format!(
                    "{}'s addresses, scopes and mechanism are built in: an account with its own is set up for your own identity provider",
                    self.provider.label()
                ));
            }
            return Ok(());
        }
        if client.is_empty() {
            return invalid("enter the client id registered for MailCraft at your identity provider".into());
        }
        for (what, value) in
            [("identity provider (issuer)", &self.issuer), ("sign-in", &self.authorize), ("token", &self.token), ("revocation", &self.revoke)]
        {
            if let Err(why) = endpoint(value) {
                return invalid(format!("the {what} address {why}"));
            }
        }
        if !self.issuer.trim().is_empty() && self.issuer.contains(['?', '#']) {
            return invalid("the identity provider (issuer) address can't have a query or a fragment".into());
        }
        scopes(&self.scopes).map_err(ModelError::Invalid)
    }

    /// Whether a sign-in can start with these settings ([`OAuth::check`], and for an own
    /// identity provider both its sign-in and token addresses, found or typed).
    pub fn ready(&self) -> Result<(), ModelError> {
        self.check()?;
        if self.provider == OAuthProvider::Custom && (self.authorize.trim().is_empty() || self.token.trim().is_empty()) {
            return Err(ModelError::Invalid("find your identity provider's sign-in addresses (Discover), or enter them".into()));
        }
        Ok(())
    }

    /// The values a remembered sign-in belongs to: changing any of them needs a fresh sign-in on
    /// the provider's page, so that a changed setting never sends a remembered token anywhere new.
    pub fn binding(&self) -> String {
        let used = self.effective();
        format!("{}|{}|{}|{}", self.provider.code(), used.issuer, used.token, used.client_id)
    }
}

/// `base` and the owner's `extra` scopes, each once, in that order.
pub fn scope_list(base: &str, extra: &str) -> String {
    let mut all: Vec<&str> = Vec::new();
    for scope in base.split_ascii_whitespace().chain(extra.split_ascii_whitespace()) {
        if !all.contains(&scope) {
            all.push(scope);
        }
    }
    all.join(" ")
}

/// Whether `text` is a list of scopes that can be sent: at most [`MAX_SCOPE_COUNT`] of them in
/// [`MAX_SCOPES`] bytes, each of the characters RFC 6749 §3.3 allows.
pub fn scopes(text: &str) -> Result<(), String> {
    if text.len() > MAX_SCOPES {
        return Err("the extra scopes are too long".into());
    }
    let mut count = 0usize;
    for scope in text.split([' ', '\t']).filter(|s| !s.is_empty()) {
        count = count.saturating_add(1);
        if !scope.bytes().all(|b| b == 0x21 || (0x23..=0x5B).contains(&b) || (0x5D..=0x7E).contains(&b)) {
            return Err("a scope can only be letters, digits and punctuation other than \" and \\".into());
        }
    }
    if count > MAX_SCOPE_COUNT {
        return Err(format!("at most {MAX_SCOPE_COUNT} extra scopes"));
    }
    Ok(())
}

/// Whether `text` is an address an identity provider can be reached at: empty, or an absolute
/// `https://` one (an `http://` one only on this computer) with no user name or password and no
/// fragment, at most [`MAX_ENDPOINT`] bytes. The reason when not, to follow "the … address".
pub fn endpoint(text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    if text.len() > MAX_ENDPOINT {
        return Err("is too long".into());
    }
    if text.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("can't have spaces in it".into());
    }
    let Some(link) = mailcraft_link::WebLink::parse(text) else { return Err("isn't a web address".into()) };
    if !link.is_https() && !is_loopback(link.host()) {
        return Err("must start with https://".into());
    }
    let after_scheme = text.split_once("://").map_or("", |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err("can't have a user name or password in it".into());
    }
    if text.contains('#') {
        return Err("can't have a fragment (#) in it".into());
    }
    Ok(())
}

/// Whether `host` (as [`mailcraft_link::WebLink::host`] writes it) is this computer.
pub fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "[::1]" | "localhost")
}

/// Microsoft's directory when it can be part of an address: letters, digits, dots and hyphens.
fn usable_tenant(tenant: &str) -> Option<&str> {
    let tenant = tenant.trim();
    let plain = !tenant.is_empty() && tenant.len() <= 253 && tenant.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'));
    let edges = |b: Option<u8>| b.is_some_and(|b| b.is_ascii_alphanumeric());
    (plain && edges(tenant.bytes().next()) && edges(tenant.bytes().next_back()) && !tenant.contains("..")).then_some(tenant)
}

/// [`OAuth::provider`] as a file has it: one this version doesn't know is an own identity
/// provider with nothing set up, never Microsoft.
pub(crate) fn provider_read<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<OAuthProvider, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_str().and_then(OAuthProvider::parse).unwrap_or(OAuthProvider::Custom))
}

/// The kind an IMAP account that signs in with an own identity provider is written with, so that
/// an earlier MailCraft, which can't read such a sign-in, keeps the account whole as one of a kind
/// it doesn't know ([`crate::AccountKind::Unknown`]) instead of failing to open the mailbox.
pub(crate) const OWN_SIGN_IN_KIND: &str = "imapOwnSignIn";

#[cfg(test)]
#[path = "oauth_custom_tests.rs"]
mod oauth_custom_tests;
