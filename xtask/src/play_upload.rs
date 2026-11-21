// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask play-upload <file.aab>`: the signed Android App Bundle to Google Play's internal
//! track, as a draft release of `work.mailcraft.android` that the owner promotes in Play Console.
//!
//! Google Play Android Developer API v3, in five requests: a service account's signed JWT for an
//! access token (`oauth2.googleapis.com/token`), then `edits.insert`, `edits.bundles.upload`
//! (simple media upload), `edits.tracks.update` (internal, draft, the uploaded version code) and
//! `edits.commit`. The requests are made by the system's `curl`, HTTPS only.
//!
//! The service account's JSON comes from `PLAY_SERVICE_ACCOUNT_JSON` and nowhere else. It, its
//! private key and the access token are never written to disk, put on a command line, printed or
//! put into an error: curl reads each request (URL, headers, body) from its standard input as a
//! config file, and an error names the step and the HTTP status alone.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use mailcraft_charset::base64::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};

pub const ACCOUNT_VARIABLE: &str = "PLAY_SERVICE_ACCOUNT_JSON";
pub const PACKAGE: &str = "work.mailcraft.android";
pub const TRACK: &str = "internal";
pub const SCOPE: &str = "https://www.googleapis.com/auth/androidpublisher";
/// The token endpoint, which is also the JWT's audience.
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const API: &str = "https://androidpublisher.googleapis.com";
/// Largest service-account JSON and largest response read (a key file is about 2.3 KB).
const MAX_ACCOUNT: usize = 16 * 1024;
const MAX_RESPONSE: usize = 64 * 1024;

/// Where the requests go: Google, or a test's own server.
pub struct Endpoints {
    pub token: String,
    pub api: String,
}

/// One request. `Debug` is not derived: the headers and the body may hold a secret.
pub struct Request {
    pub step: &'static str,
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<String>,
    pub body: Body,
}

pub enum Body {
    Text(String),
    File(PathBuf),
}

/// What sends a request: its status and (bounded) body, or an error without the request in it.
pub trait Http {
    fn send(&self, request: &Request) -> Result<(u16, Vec<u8>), String>;
}

/// The service account: its address and its private key (PKCS#8 DER). No `Debug`.
pub struct Account {
    pub email: String,
    der: Vec<u8>,
}

pub fn run(args: &[&str]) -> Result<(), String> {
    let [aab] = args else {
        return Err("usage: cargo xtask play-upload <file.aab> (the account in PLAY_SERVICE_ACCOUNT_JSON)".into());
    };
    let json = std::env::var(ACCOUNT_VARIABLE).map_err(|_| format!("{ACCOUNT_VARIABLE} isn't set"))?;
    let account = Account::parse(&json)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| "the clock is before 1970")?.as_secs();
    let google = Endpoints { token: TOKEN_URL.into(), api: API.into() };
    let code = upload(&Curl { proto: "=https" }, &google, &account, Path::new(aab), now)?;
    println!("Google Play: {aab} (version code {code}) is a draft release on the {TRACK} track of {PACKAGE}");
    Ok(())
}

impl Account {
    /// `client_email` and `private_key` of a service account's JSON, in words that quote none of it.
    pub fn parse(json: &str) -> Result<Self, String> {
        let bad = |what: &str| format!("{ACCOUNT_VARIABLE} isn't a service account's JSON key ({what})");
        if json.len() > MAX_ACCOUNT {
            return Err(bad("too long"));
        }
        let value: serde_json::Value = serde_json::from_str(json).map_err(|_| bad("not JSON"))?;
        let field = |name: &str| value.get(name).and_then(|v| v.as_str()).ok_or_else(|| bad(&format!("no {name}")));
        let email = field("client_email")?;
        if email.is_empty() || !email.chars().all(|c| c.is_ascii_alphanumeric() || "@.-_".contains(c)) {
            return Err(bad("client_email"));
        }
        let pem = field("private_key")?;
        let inner = pem
            .split_once("-----BEGIN PRIVATE KEY-----")
            .and_then(|(_, rest)| rest.split_once("-----END PRIVATE KEY-----"))
            .map(|(inner, _)| inner)
            .ok_or_else(|| bad("private_key isn't PKCS#8 PEM"))?;
        let b64: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
        let der = STANDARD.decode(b64.as_bytes()).map_err(|_| bad("private_key isn't base64"))?;
        RsaKeyPair::from_pkcs8(&der).map_err(|_| bad("private_key isn't an RSA key"))?;
        Ok(Self { email: email.to_string(), der })
    }

    /// The signed JWT that asks for an access token, valid for an hour from `now`.
    pub fn jwt(&self, now: u64) -> Result<String, String> {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let claims = serde_json::json!({ "iss": self.email, "scope": SCOPE, "aud": TOKEN_URL, "iat": now, "exp": now.saturating_add(3600) });
        let input = format!("{header}.{}", URL_SAFE_NO_PAD.encode(claims.to_string()));
        let pair = RsaKeyPair::from_pkcs8(&self.der).map_err(|_| "the private key doesn't read")?;
        let mut signature = vec![0; pair.public().modulus_len()];
        pair.sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), input.as_bytes(), &mut signature).map_err(|_| "signing the JWT failed")?;
        Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(&signature)))
    }
}

/// The five steps; the version code Google read from the bundle.
pub fn upload(http: &dyn Http, at: &Endpoints, account: &Account, aab: &Path, now: u64) -> Result<u64, String> {
    if !aab.is_file() {
        return Err(format!("{} isn't a file", aab.display()));
    }
    let form = format!("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={}", account.jwt(now)?);
    let token = call(http, "token", "POST", at.token.clone(), vec!["Content-Type: application/x-www-form-urlencoded".into()], Body::Text(form))?;
    let token = text_field(&token, "access_token").filter(|t| safe(t, 4096)).ok_or("token: no usable access_token in the answer")?;
    let auth = format!("Authorization: Bearer {token}");
    let json = || vec![auth.clone(), "Content-Type: application/json".to_string()];
    let edits = format!("{}/androidpublisher/v3/applications/{PACKAGE}/edits", at.api);

    let edit = call(http, "edits.insert", "POST", edits.clone(), json(), Body::Text("{}".into()))?;
    let id = text_field(&edit, "id").filter(|id| safe(id, 256) && !id.contains(['/', '+', '='])).ok_or("edits.insert: no usable edit id")?;
    let bundle_url = format!("{}/upload/androidpublisher/v3/applications/{PACKAGE}/edits/{id}/bundles?uploadType=media", at.api);
    let headers = vec![auth.clone(), "Content-Type: application/octet-stream".to_string()];
    let bundle = call(http, "edits.bundles.upload", "POST", bundle_url, headers, Body::File(aab.to_path_buf()))?;
    let code = bundle.get("versionCode").and_then(serde_json::Value::as_u64).filter(|c| *c > 0).ok_or("edits.bundles.upload: no versionCode")?;
    let release = serde_json::json!({ "track": TRACK, "releases": [{ "versionCodes": [code.to_string()], "status": "draft" }] });
    call(http, "edits.tracks.update", "PUT", format!("{edits}/{id}/tracks/{TRACK}"), json(), Body::Text(release.to_string()))?;
    call(http, "edits.commit", "POST", format!("{edits}/{id}:commit"), json(), Body::Text(String::new()))?;
    Ok(code)
}

/// One request whose answer is a 2xx with JSON (or nothing); else the step and the status.
fn call(
    http: &dyn Http,
    step: &'static str,
    method: &'static str,
    url: String,
    headers: Vec<String>,
    body: Body,
) -> Result<serde_json::Value, String> {
    let (status, bytes) = http.send(&Request { step, method, url, headers, body })?;
    if !(200..300).contains(&status) {
        return Err(format!("{step}: HTTP {status}"));
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|_| format!("{step}: HTTP {status}, an answer that isn't JSON"))
}

fn text_field<'a>(value: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(serde_json::Value::as_str)
}

/// A token or id that may go into a URL or a curl config line: printable, no space, no quote.
pub fn safe(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && text.chars().all(|c| c.is_ascii_alphanumeric() || "-._~+/=".contains(c))
}

/// The system's curl. `proto` is `=https` for Google (a test's loopback server: `=http`).
pub struct Curl {
    pub proto: &'static str,
}

impl Curl {
    /// The command line: nothing of the request, which goes in on standard input.
    pub fn args(&self) -> Vec<String> {
        let fixed = ["--config", "-", "--silent", "--proto", self.proto, "--proto-redir", self.proto, "--max-time", "1800"];
        let mut args: Vec<String> = fixed.iter().map(|s| s.to_string()).collect();
        args.extend(["--max-filesize", &MAX_RESPONSE.to_string(), "--write-out", "\\n%{http_code}"].map(String::from));
        args
    }
}

/// The request as a curl config file (`--config -`), every value quoted.
pub fn curl_config(request: &Request) -> String {
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace(['\r', '\n'], ""));
    let mut out = format!("url = {}\nrequest = {}\n", quote(&request.url), quote(request.method));
    for header in &request.headers {
        out.push_str(&format!("header = {}\n", quote(header)));
    }
    match &request.body {
        Body::Text(text) => out.push_str(&format!("data-binary = {}\n", quote(text))),
        Body::File(path) => out.push_str(&format!("upload-file = {}\n", quote(&path.to_string_lossy()))),
    }
    out
}

/// curl's output (`--write-out "\n%{http_code}"`) as the status and the body.
pub fn split_output(out: &[u8]) -> Option<(u16, Vec<u8>)> {
    let at = out.iter().rposition(|b| *b == b'\n')?;
    let status = std::str::from_utf8(out.get(at + 1..)?).ok()?.trim().parse().ok()?;
    Some((status, out.get(..at)?.to_vec()))
}

impl Http for Curl {
    fn send(&self, request: &Request) -> Result<(u16, Vec<u8>), String> {
        let step = request.step;
        let mut child = Command::new("curl")
            .args(self.args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| format!("{step}: curl doesn't start"))?;
        let mut stdin = child.stdin.take().ok_or_else(|| format!("{step}: no pipe to curl"))?;
        stdin.write_all(curl_config(request).as_bytes()).map_err(|_| format!("{step}: curl didn't take the request"))?;
        drop(stdin);
        let mut out = Vec::new();
        let stdout = child.stdout.take().ok_or_else(|| format!("{step}: no pipe from curl"))?;
        stdout.take(MAX_RESPONSE as u64 + 16).read_to_end(&mut out).map_err(|_| format!("{step}: curl's answer didn't read"))?;
        if out.len() > MAX_RESPONSE {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{step}: the answer is too long"));
        }
        let exit = child.wait().map_err(|_| format!("{step}: curl didn't finish"))?;
        if !exit.success() {
            return Err(format!("{step}: curl failed (exit {})", exit.code().unwrap_or(-1)));
        }
        split_output(&out).ok_or_else(|| format!("{step}: no HTTP status from curl"))
    }
}

#[cfg(test)]
#[path = "play_upload_tests.rs"]
mod play_upload_tests;
