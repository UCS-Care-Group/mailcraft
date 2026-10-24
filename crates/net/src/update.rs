// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whether a new version of MailCraft is out: one question to GitHub's API about the newest
//! release of the repository the build names (`CARGO_PKG_REPOSITORY`), and nothing else.
//!
//! The request is a GET over HTTPS with the crate's fixed `User-Agent` (`MailCraft`, no version)
//! and an `Accept` header: no account, address, mailbox detail, identifier or version of this
//! program goes with it. Nothing is downloaded here: the release's list of files is read
//! (names, sizes, and addresses that must be that release's own on `github.com`), and a copy
//! that updates itself hands it to [`crate::update_download`].
//!
//! The answer is as hostile as any server's: at most 512 KB, ten seconds, no redirects. Its
//! `tag_name` is read strictly as a version; a pre-release is passed over unless the one asking
//! runs a pre-release; its `html_url` is taken only when it is this repository's page for that
//! tag (else the page is made from the tag); and of its notes only whether they name a
//! published security advisory (`GHSA-xxxx-xxxx-xxxx`, `RUSTSEC-yyyy-nnnn`) is kept. No text of
//! the release is handed on or logged, and an error says what went wrong without the address.

use std::time::Duration;

use mailcraft_model::update::{Asset, Release, Version, is_file_name};
use serde_json::Value;

use crate::NetError;
use crate::http::{self, Request, Url};

/// The public keys of MailCraft's release signature, in base64 (32 bytes each): the key the
/// release workflow signs `SHA256SUMS.txt` with, and a spare whose private half is kept offline
/// until the first is lost or stolen. A release signed by either is one of the project's; a copy
/// that updates itself takes nothing that isn't (`update_download`). An empty one is no key:
/// with neither, nothing is ever downloaded. `cargo xtask release-key new` prints both
/// (docs/releasing.md).
pub const RELEASE_KEYS: [&str; 2] = ["Mu5RF5oJpFhDGfa5wpgznC39tDC8vbUbyNkiAMdluPQ=", "XV6WRgW06/mGhZCHhFVKMIEZDmLevQduZPLSj/UW5zI="];

/// The repository this build came from: `https://github.com/{owner}/{repo}`.
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
/// GitHub's API.
const API: &str = "https://api.github.com";
/// How long connecting may take, and so may the answer: ten seconds together at most.
const PATIENCE: Duration = Duration::from_secs(5);
/// Most bytes of the answer read. GitHub's answer lists every package of the release and its
/// notes: 0.4.2's was 44 KB, so this leaves room to grow.
const MAX_ANSWER: usize = 512 * 1024;
/// Longest owner or repository name taken from the build.
const MAX_NAME: usize = 100;
/// Most files of a release looked at (0.4.2 had 21).
const MAX_ASSETS: usize = 200;

/// The newest release of this build's repository, when GitHub says there is one that this
/// build could run. `pre_releases`: the running build is a pre-release itself, so one is not
/// passed over. Blocks for ten seconds at most.
pub fn latest(pre_releases: bool) -> Result<Release, NetError> {
    latest_at(API, REPOSITORY, pre_releases)
}

/// [`latest`], asking the API at `api` (an `https://` address, or `http://` on this machine)
/// about `repository` (`https://github.com/{owner}/{repo}`).
pub fn latest_at(api: &str, repository: &str, pre_releases: bool) -> Result<Release, NetError> {
    let (owner, repo) = owner_and_repo(repository).ok_or_else(|| said("this build names no repository on GitHub"))?;
    let url = Url::parse(&format!("{}/repos/{owner}/{repo}/releases/latest", api.trim().trim_end_matches('/')))
        .map_err(|_| said("the address to ask isn't one"))?;
    let headers = [("Accept", "application/vnd.github+json")];
    let reply = http::request(&url, &Request { method: "GET", headers: &headers, body: None, patience: PATIENCE, max_body: MAX_ANSWER })
        .map_err(|e| without_address(&e))?;
    match reply.status {
        200 => release(&reply.body, owner, repo, pre_releases),
        404 => Err(said("GitHub has no release of MailCraft")),
        403 | 429 => Err(said("GitHub asked to be asked later")),
        status => Err(said(&format!("GitHub answered {status}"))),
    }
}

/// What an error is called, without the address or the host it came from.
fn without_address(error: &NetError) -> NetError {
    if http::is_too_slow(error) {
        said("GitHub took too long to answer")
    } else if http::is_too_long(error) {
        said("GitHub's answer was too long")
    } else if error.is_unreachable() {
        said("GitHub couldn't be reached")
    } else {
        said("GitHub's answer wasn't one")
    }
}

fn said(text: &str) -> NetError {
    NetError::Config(text.to_string())
}

/// `{owner}` and `{repo}` of `https://github.com/{owner}/{repo}`: letters, digits, `-`, `_`, `.`.
fn owner_and_repo(repository: &str) -> Option<(&str, &str)> {
    let rest = repository.trim().strip_prefix("https://github.com/")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, repo) = rest.split_once('/')?;
    let name = |s: &str| {
        !s.is_empty() && s.len() <= MAX_NAME && !s.starts_with('.') && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    (name(owner) && name(repo)).then_some((owner, repo))
}

/// The release in GitHub's answer, when it is one this build could run.
fn release(body: &[u8], owner: &str, repo: &str, pre_releases: bool) -> Result<Release, NetError> {
    let junk = || said("GitHub's answer wasn't one");
    let json: Value = serde_json::from_slice(body).map_err(|_| junk())?;
    let tag = json.get("tag_name").and_then(Value::as_str).ok_or_else(junk)?;
    let version = Version::parse(tag).ok_or_else(junk)?;
    let marked = json.get("prerelease").and_then(Value::as_bool).unwrap_or(false);
    if (marked || version.is_pre_release()) && !pre_releases {
        return Err(said("GitHub's newest release is a pre-release"));
    }
    // The tag has been read as a version: it holds nothing that could leave the address.
    let page = format!("https://github.com/{owner}/{repo}/releases/tag/{tag}");
    let given = json.get("html_url").and_then(Value::as_str).unwrap_or_default();
    let page = if is_release_page(given, owner, repo, tag) { given.to_string() } else { page };
    let security = json.get("body").and_then(Value::as_str).is_some_and(names_an_advisory);
    let assets = json.get("assets").and_then(Value::as_array).map(|list| assets(list, owner, repo, tag)).unwrap_or_default();
    Ok(Release { version, page, security, assets })
}

/// The files of release `tag` in GitHub's list of them: those with a name that will do as one
/// ([`is_file_name`]), a size, and the address of that name in that release on `github.com`.
/// Any other is left out; so is a name that comes twice, and what is beyond [`MAX_ASSETS`].
fn assets(list: &[Value], owner: &str, repo: &str, tag: &str) -> Vec<Asset> {
    let mut found: Vec<Asset> = Vec::new();
    let mut twice: Vec<String> = Vec::new();
    for asset in list.iter().take(MAX_ASSETS) {
        let Some(name) = asset.get("name").and_then(Value::as_str).filter(|name| is_file_name(name)) else { continue };
        let Some(size) = asset.get("size").and_then(Value::as_u64) else { continue };
        let url = asset.get("browser_download_url").and_then(Value::as_str).unwrap_or_default();
        if !is_download_address(url, owner, repo, tag, name) {
            continue;
        }
        if found.iter().any(|a| a.name == name) {
            twice.push(name.to_string());
            continue;
        }
        found.push(Asset { name: name.to_string(), size, url: url.to_string() });
    }
    found.retain(|a| !twice.contains(&a.name));
    found
}

/// Whether `given` is `https://github.com/{owner}/{repo}/releases/download/{tag}/{name}`: the
/// owner and the repository in any case, everything else exactly.
fn is_download_address(given: &str, owner: &str, repo: &str, tag: &str, name: &str) -> bool {
    let Some(rest) = given.strip_prefix("https://github.com/") else { return false };
    let parts: Vec<&str> = rest.splitn(6, '/').collect();
    matches!(parts.as_slice(), [o, r, "releases", "download", t, n]
        if o.eq_ignore_ascii_case(owner) && r.eq_ignore_ascii_case(repo) && *t == tag && *n == name)
}

/// Whether `given` is the page of release `tag` of `owner`/`repo`: the owner and the repository
/// in any case (GitHub doesn't mind), everything else exactly.
fn is_release_page(given: &str, owner: &str, repo: &str, tag: &str) -> bool {
    let Some(rest) = given.strip_prefix("https://github.com/") else { return false };
    let parts: Vec<&str> = rest.splitn(5, '/').collect();
    matches!(parts.as_slice(), [o, r, "releases", "tag", t] if o.eq_ignore_ascii_case(owner) && r.eq_ignore_ascii_case(repo) && *t == tag)
}

/// Whether `notes` name a published advisory: `GHSA-xxxx-xxxx-xxxx` (lower-case letters and
/// digits) or `RUSTSEC-yyyy-nnnn` (digits), standing on their own.
fn names_an_advisory(notes: &str) -> bool {
    let bytes = notes.as_bytes();
    let alone = |at: usize, len: usize| {
        let before = at.checked_sub(1).and_then(|i| bytes.get(i));
        let after = bytes.get(at.saturating_add(len));
        let joins = |b: Option<&u8>| b.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-');
        !joins(before) && !joins(after)
    };
    let shaped = |at: usize, shape: &[u8]| {
        shape.len() <= bytes.len().saturating_sub(at)
            && shape.iter().enumerate().all(|(i, want)| {
                let got = bytes.get(at.saturating_add(i)).copied().unwrap_or(0);
                match want {
                    b'x' => got.is_ascii_digit() || got.is_ascii_lowercase(),
                    b'n' => got.is_ascii_digit(),
                    other => got == *other,
                }
            })
    };
    const GHSA: &[u8] = b"GHSA-xxxx-xxxx-xxxx";
    const RUSTSEC: &[u8] = b"RUSTSEC-nnnn-nnnn";
    (0..bytes.len()).any(|at| (shaped(at, GHSA) && alone(at, GHSA.len())) || (shaped(at, RUSTSEC) && alone(at, RUSTSEC.len())))
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::TcpListener;

    use proptest::prelude::*;

    use super::*;
    use crate::fake::{FakeHttp, json_answer};

    const REPO: &str = "https://github.com/UCS-Care-Group/mailcraft";

    fn answer(release: &Value) -> Vec<u8> {
        json_answer("200 OK", &release.to_string())
    }

    fn ask(server: &FakeHttp, pre_releases: bool) -> Result<Release, NetError> {
        latest_at(&server.url(""), REPO, pre_releases)
    }

    fn release_of(tag: &str) -> Value {
        serde_json::json!({
            "tag_name": tag,
            "name": format!("MailCraft {tag}"),
            "prerelease": false,
            "draft": false,
            "html_url": format!("https://github.com/UCS-Care-Group/mailcraft/releases/tag/{tag}"),
            "body": "What's new: faster search.",
            "assets": [{"name": "mailcraft.dmg", "browser_download_url": "https://github.com/x"}],
        })
    }

    fn config(result: Result<Release, NetError>) -> String {
        match result {
            Err(NetError::Config(why)) => why,
            other => panic!("expected an error that says why, got {other:?}"),
        }
    }

    #[test]
    fn newer_same_and_older_releases_are_read() {
        for tag in ["v0.9.0", "v0.4.2", "v0.1.0", "1.0.0"] {
            let server = FakeHttp::answering(answer(&release_of(tag)));
            let got = ask(&server, false).unwrap();
            assert_eq!(got.version, Version::parse(tag).unwrap(), "{tag}");
            assert_eq!(got.page, format!("https://github.com/UCS-Care-Group/mailcraft/releases/tag/{tag}"));
            assert!(!got.security);
        }
    }

    #[test]
    fn the_request_carries_nothing_of_the_owner() {
        let server = FakeHttp::answering(answer(&release_of("v0.9.0")));
        ask(&server, false).unwrap();
        assert_eq!(server.asked(), 1);
        let sent = server.request(0);
        let mut lines = sent.split("\r\n");
        assert_eq!(lines.next(), Some("GET /repos/UCS-Care-Group/mailcraft/releases/latest HTTP/1.1"));
        // Exactly these headers, and no body.
        let headers: Vec<&str> = lines.take_while(|l| !l.is_empty()).collect();
        let host = format!("Host: 127.0.0.1:{}", server.port);
        assert_eq!(headers, [host.as_str(), "User-Agent: MailCraft", "Accept: application/vnd.github+json", "Connection: close"]);
        assert!(sent.ends_with("\r\n\r\n"), "{sent:?}");
        // No version of this program, no query, nothing of an account.
        let version = env!("CARGO_PKG_VERSION");
        for nothing in [version, "?", "Cookie", "Authorization", "Referer", "@", "example.org"] {
            assert!(!sent.contains(nothing), "{nothing:?} in {sent:?}");
        }
    }

    #[test]
    fn pre_releases_are_passed_over_unless_this_is_one() {
        let mut marked = release_of("v0.9.0");
        marked["prerelease"] = Value::Bool(true);
        for release in [marked, release_of("v0.9.0-beta.1")] {
            let server = FakeHttp::answering(answer(&release));
            assert_eq!(config(ask(&server, false)), "GitHub's newest release is a pre-release");
            assert!(ask(&server, true).is_ok());
        }
    }

    #[test]
    fn junk_is_no_answer() {
        let mut no_tag = release_of("v0.9.0");
        no_tag.as_object_mut().unwrap().remove("tag_name");
        let mut number_tag = release_of("v0.9.0");
        number_tag["tag_name"] = serde_json::json!(9);
        let answers = [
            json_answer("200 OK", "not json"),
            json_answer("200 OK", "[]"),
            json_answer("200 OK", ""),
            answer(&no_tag),
            answer(&number_tag),
            answer(&release_of("latest")),
            answer(&release_of("v0.9")),
            answer(&release_of("v0.9.0 ")),
            answer(&release_of("v0.9.0/../../evil")),
            answer(&release_of("v0.9.0?x=1")),
            b"<html>hello</html>\n".to_vec(),
        ];
        for bytes in answers {
            let server = FakeHttp::answering(bytes.clone());
            assert_eq!(config(ask(&server, false)), "GitHub's answer wasn't one", "{}", String::from_utf8_lossy(&bytes));
        }
        // What isn't a 200 is no release either, and a redirect is not followed.
        for (status, why) in [
            ("404 Not Found", "GitHub has no release of MailCraft"),
            ("403 Forbidden", "GitHub asked to be asked later"),
            ("500 Oops", "GitHub answered 500"),
        ] {
            let server = FakeHttp::answering(json_answer(status, "{}"));
            assert_eq!(config(ask(&server, false)), why);
        }
        let moved = b"HTTP/1.1 301 Moved\r\nLocation: http://127.0.0.1:1/x\r\nContent-Length: 0\r\n\r\n".to_vec();
        let server = FakeHttp::answering(moved);
        assert_eq!(config(ask(&server, false)), "GitHub answered 301");
        assert_eq!(server.asked(), 1);
    }

    #[test]
    fn oversized_answers_are_refused() {
        let mut release = release_of("v0.9.0");
        release["body"] = Value::String("x".repeat(MAX_ANSWER));
        let server = FakeHttp::answering(answer(&release));
        assert_eq!(config(ask(&server, false)), "GitHub's answer was too long");
        let endless = format!("HTTP/1.1 200 OK\r\n\r\n{}", "x".repeat(MAX_ANSWER + 10)).into_bytes();
        let server = FakeHttp::answering(endless);
        assert_eq!(config(ask(&server, false)), "GitHub's answer was too long");
    }

    #[test]
    fn slow_answers_are_given_up() {
        // A server that takes the request and says nothing.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                held.push(stream);
            }
        });
        let started = std::time::Instant::now();
        let got = latest_at(&format!("http://127.0.0.1:{port}"), REPO, false);
        assert_eq!(config(got), "GitHub took too long to answer");
        assert!(started.elapsed() < Duration::from_secs(12));
        // One that stops halfway.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 500\r\n\r\n{\"tag");
                std::thread::sleep(Duration::from_secs(12));
            }
        });
        assert_eq!(config(latest_at(&format!("http://127.0.0.1:{port}"), REPO, false)), "GitHub took too long to answer");
        // Nobody there at all.
        let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(config(latest_at(&format!("http://127.0.0.1:{closed}"), REPO, false)), "GitHub couldn't be reached");
    }

    #[test]
    fn a_wrong_link_is_replaced_by_the_one_made_from_the_tag() {
        let made = "https://github.com/UCS-Care-Group/mailcraft/releases/tag/v0.9.0";
        for wrong in [
            "https://evil.example/UCS-Care-Group/mailcraft/releases/tag/v0.9.0",
            "http://github.com/UCS-Care-Group/mailcraft/releases/tag/v0.9.0",
            "https://github.com/someone-else/mailcraft/releases/tag/v0.9.0",
            "https://github.com/UCS-Care-Group/mailcraft/releases/tag/v0.9.1",
            "https://github.com/UCS-Care-Group/mailcraft/releases/tag/V0.9.0",
            "https://github.com/UCS-Care-Group/mailcraft/releases/tag/v0.9.0?download=1",
            "https://github.com/UCS-Care-Group/mailcraft/releases/download/v0.9.0/setup.exe",
            "javascript:alert(1)",
            "",
        ] {
            let mut release = release_of("v0.9.0");
            release["html_url"] = Value::String(wrong.into());
            let server = FakeHttp::answering(answer(&release));
            assert_eq!(ask(&server, false).unwrap().page, made, "{wrong}");
        }
        let mut release = release_of("v0.9.0");
        release.as_object_mut().unwrap().remove("html_url");
        let server = FakeHttp::answering(answer(&release));
        assert_eq!(ask(&server, false).unwrap().page, made);
        // The repository's name in another case is the same page.
        let mut release = release_of("v0.9.0");
        release["html_url"] = Value::String("https://github.com/ucs-care-group/MailCraft/releases/tag/v0.9.0".into());
        let server = FakeHttp::answering(answer(&release));
        assert_eq!(ask(&server, false).unwrap().page, "https://github.com/ucs-care-group/MailCraft/releases/tag/v0.9.0");
    }

    #[test]
    fn a_security_fix_is_marked_by_an_advisory_id_alone() {
        for (notes, security) in [
            ("Fixes GHSA-4m2p-q8vx-9rjc in the HTML reader.", true),
            ("See RUSTSEC-2026-0012.", true),
            ("(GHSA-abcd-0123-wxyz)", true),
            ("GHSA-abcd-0123-wxyz", true),
            ("Security: a fix.", false),
            ("security fix, CVE-2026-1234", false),
            ("GHSA-ABCD-0123-WXYZ", false),
            ("GHSA-abc-0123-wxyz", false),
            ("GHSA-abcd-0123-wxyz9", false),
            ("XGHSA-abcd-0123-wxyz", false),
            ("RUSTSEC-26-0012", false),
            ("RUSTSEC-2026-00123", false),
            ("RUSTSEC-2026-0012-x", false),
            ("ghsa-abcd-0123-wxyz", false),
            ("", false),
        ] {
            let mut release = release_of("v0.9.0");
            release["body"] = Value::String(notes.into());
            let server = FakeHttp::answering(answer(&release));
            assert_eq!(ask(&server, false).unwrap().security, security, "{notes:?}");
        }
        let mut release = release_of("v0.9.0");
        release["body"] = Value::Null;
        let server = FakeHttp::answering(answer(&release));
        assert!(!ask(&server, false).unwrap().security);
    }

    #[test]
    fn the_files_of_the_release_are_read_and_checked() {
        let download = |name: &str| format!("https://github.com/UCS-Care-Group/mailcraft/releases/download/v0.9.0/{name}");
        let mut release = release_of("v0.9.0");
        release["assets"] = serde_json::json!([
            {"name": "SHA256SUMS.txt", "size": 2000, "browser_download_url": download("SHA256SUMS.txt")},
            {"name": "SHA256SUMS.txt.sig", "size": 89, "browser_download_url": download("SHA256SUMS.txt.sig")},
            {"name": "mailcraft-0.9.0-linux-x86_64.AppImage", "size": 40_000_000u64, "browser_download_url": download("mailcraft-0.9.0-linux-x86_64.AppImage")},
            // Another case of the repository's name is the same repository.
            {"name": "mailcraft-0.9.0-windows-x64.msi", "size": 9, "browser_download_url": "https://github.com/ucs-care-group/MailCraft/releases/download/v0.9.0/mailcraft-0.9.0-windows-x64.msi"},
            // Left out: another host, another repository, another tag, another name, no size, a bad name.
            {"name": "evil.msi", "size": 9, "browser_download_url": "https://evil.example/UCS-Care-Group/mailcraft/releases/download/v0.9.0/evil.msi"},
            {"name": "other.msi", "size": 9, "browser_download_url": "https://github.com/someone/mailcraft/releases/download/v0.9.0/other.msi"},
            {"name": "old.msi", "size": 9, "browser_download_url": "https://github.com/UCS-Care-Group/mailcraft/releases/download/v0.1.0/old.msi"},
            {"name": "named.msi", "size": 9, "browser_download_url": download("renamed.msi")},
            {"name": "sizeless.msi", "browser_download_url": download("sizeless.msi")},
            {"name": "../up.msi", "size": 9, "browser_download_url": download("../up.msi")},
            {"name": "http.msi", "size": 9, "browser_download_url": "http://github.com/UCS-Care-Group/mailcraft/releases/download/v0.9.0/http.msi"},
            {"name": "query.msi", "size": 9, "browser_download_url": format!("{}?x=1", download("query.msi"))},
            // A name that comes twice is neither.
            {"name": "twice.dmg", "size": 9, "browser_download_url": download("twice.dmg")},
            {"name": "twice.dmg", "size": 10, "browser_download_url": download("twice.dmg")},
            "junk",
        ]);
        let server = FakeHttp::answering(answer(&release));
        let got = ask(&server, false).unwrap();
        let names: Vec<&str> = got.assets.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["SHA256SUMS.txt", "SHA256SUMS.txt.sig", "mailcraft-0.9.0-linux-x86_64.AppImage", "mailcraft-0.9.0-windows-x64.msi"]);
        assert_eq!(got.assets[2].size, 40_000_000);
        assert_eq!(got.assets[2].url, download("mailcraft-0.9.0-linux-x86_64.AppImage"));
        // Not a list at all: no files.
        let mut release = release_of("v0.9.0");
        release["assets"] = serde_json::json!({"name": "x"});
        let server = FakeHttp::answering(answer(&release));
        assert!(ask(&server, false).unwrap().assets.is_empty());
        // More than are looked at.
        let many: Vec<Value> = (0..MAX_ASSETS + 50)
            .map(|n| serde_json::json!({"name": format!("f{n}"), "size": 1, "browser_download_url": download(&format!("f{n}"))}))
            .collect();
        let mut release = release_of("v0.9.0");
        release["assets"] = Value::Array(many);
        let server = FakeHttp::answering(answer(&release));
        assert_eq!(ask(&server, false).unwrap().assets.len(), MAX_ASSETS);
    }

    #[test]
    fn the_shipped_release_keys_are_keys() {
        for key in RELEASE_KEYS {
            assert!(mailcraft_charset::base64::STANDARD.decode(key).is_ok_and(|k| k.len() == 32), "{key}");
        }
    }

    #[test]
    fn only_a_repository_on_github_is_asked_about() {
        assert_eq!(owner_and_repo(REPO), Some(("UCS-Care-Group", "mailcraft")));
        assert_eq!(owner_and_repo("https://github.com/a/b.git"), Some(("a", "b")));
        assert_eq!(owner_and_repo("https://github.com/a/b/"), Some(("a", "b")));
        for bad in [
            "",
            "https://gitlab.com/a/b",
            "http://github.com/a/b",
            "https://github.com/a",
            "https://github.com/a/b/c",
            "https://github.com/a/../b",
            "https://github.com/a/b?x",
            "https://github.com/a b/c",
        ] {
            assert_eq!(owner_and_repo(bad), None, "{bad}");
        }
        let server = FakeHttp::answering(answer(&release_of("v0.9.0")));
        assert_eq!(config(latest_at(&server.url(""), "https://example.org/a/b", false)), "this build names no repository on GitHub");
        assert_eq!(server.asked(), 0);
        // The build's own is one.
        assert!(owner_and_repo(REPOSITORY).is_some());
    }

    proptest! {
        #[test]
        fn arbitrary_answers_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..400), tag in "\\PC{0,20}", notes in "\\PC{0,80}", url in "\\PC{0,80}") {
            let _ = release(&bytes, "o", "r", false);
            let json = serde_json::json!({"tag_name": tag, "body": notes, "html_url": url, "prerelease": false});
            if let Ok(found) = release(json.to_string().as_bytes(), "o", "r", true) {
                prop_assert!(found.page.starts_with("https://github.com/"));
            }
            let _ = names_an_advisory(&notes);
        }
    }
}
