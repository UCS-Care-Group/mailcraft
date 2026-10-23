// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A copy that updates itself, the whole way: the window's daily check finds a newer release,
//! this host downloads the copy's file through `mailcraft-net` from a stand-in release server on
//! this machine, and it is kept only when the release's signature and checksum say it is the
//! release's own. Keys are made here; nothing leaves this machine, and nothing is installed.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::Mailbox;
use mailcraft_model::update::{Asset, Release, Version};
use mailcraft_ui_egui::{MailApp, Services, UiState, updates::UpdateCheck};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use serde_json::json;

use crate::install::Install;

const NOW: i64 = 1_791_993_600;

/// A release server on this machine: each path answers its bytes, whole; any other is a 404.
fn serve(files: Vec<(String, Vec<u8>)>) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let kept = asked.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let path = line.split(' ').nth(1).unwrap_or("").to_string();
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
            }
            kept.lock().unwrap().push(path.clone());
            let answer = match files.iter().find(|(p, _)| *p == path) {
                Some((_, body)) => {
                    let mut out = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                    out.extend_from_slice(body);
                    out
                }
                None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
            };
            let _ = stream.write_all(&answer);
        }
    });
    (port, asked)
}

fn key() -> Ed25519KeyPair {
    Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap().as_ref()).unwrap()
}

/// A version newer than the one running.
fn newer() -> Version {
    let running = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    Version { major: running.major + 1, minor: 0, patch: 0, suffix: String::new() }
}

/// A newer release with this copy's file `file`, its list of checksums signed by `signer`.
fn release(signer: &Ed25519KeyPair, file: &[u8]) -> (Release, String, Arc<Mutex<Vec<String>>>) {
    let version = newer();
    let name = Install::AppImage.file_name(&version, std::env::consts::ARCH).unwrap_or_else(|| format!("mailcraft-{version}-linux-x86_64.AppImage"));
    let digest: String = ring::digest::digest(&ring::digest::SHA256, file).as_ref().iter().map(|b| format!("{b:02x}")).collect();
    let sums = format!("{digest}  {name}\n").into_bytes();
    let signature = format!("{}\n", mailcraft_charset::base64::STANDARD.encode(signer.sign(&sums).as_ref())).into_bytes();
    let files =
        vec![("/d/SHA256SUMS.txt".to_string(), sums), ("/d/SHA256SUMS.txt.sig".to_string(), signature), (format!("/d/{name}"), file.to_vec())];
    let sizes: Vec<u64> = files.iter().map(|(_, b)| b.len() as u64).collect();
    let (port, asked) = serve(files);
    let asset = |n: &str, size: u64| Asset { name: n.to_string(), size, url: format!("http://127.0.0.1:{port}/d/{n}") };
    let assets = vec![asset("SHA256SUMS.txt", sizes[0]), asset("SHA256SUMS.txt.sig", sizes[1]), asset(&name, sizes[2])];
    (Release { page: String::new(), version, security: false, assets }, name, asked)
}

/// The window with this host's download, trusting `trusted`, into `dir`.
fn window(found: Release, trusted: [u8; 32], dir: &Path) -> MailApp {
    let dir = dir.to_path_buf();
    let services = Services {
        update_check: Some(Box::new(move |_| {
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(Ok(found.clone()));
            rx
        })),
        update_download: Some(Box::new(move |release: Release| {
            let (tx, rx) = std::sync::mpsc::channel();
            let dir = dir.clone();
            std::thread::spawn(move || {
                let fetch = |r: &Release, name: &str, d: &Path| mailcraft_net::update_download::download_from_stand_in(r, name, d, &[trusted]);
                let _ = tx.send(super::updater::download(&release, Install::AppImage, &dir, fetch).map(|v| v.path));
            });
            rx
        })),
        ..Default::default()
    };
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), services);
    app.autosave = false;
    app.auto_sync = false;
    app.apply_prefs(UiState { update_check: UpdateCheck::On, ..Default::default() });
    app
}

/// Frames until the download has come to something.
fn settle(app: &mut MailApp) -> serde_json::Value {
    let ctx = egui::Context::default();
    for _ in 0..1_000 {
        app.logic(&ctx);
        let state = app.execute("ui.updateCheck", json!({"value": true})).unwrap();
        if state["install"]["downloading"] == false && state["newer"].is_object() {
            return state;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the download didn't end");
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-update-flow-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_signed_newer_release_is_downloaded_and_kept() {
    let pair = key();
    let file = b"the next version of MailCraft".repeat(100);
    let (found, name, asked) = release(&pair, &file);
    let dir = scratch("signed");
    let mut app = window(found, pair.public_key().as_ref().try_into().unwrap(), &dir);
    let state = settle(&mut app);
    assert_eq!(state["install"]["downloaded"], newer().to_string(), "{state}");
    assert_eq!(std::fs::read(dir.join(&name)).unwrap(), file);
    assert_eq!(*asked.lock().unwrap(), ["/d/SHA256SUMS.txt", "/d/SHA256SUMS.txt.sig", format!("/d/{name}").as_str()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_release_signed_by_anyone_else_is_refused_and_nothing_kept() {
    let (pair, stranger) = (key(), key());
    let (found, name, asked) = release(&stranger, b"not ours");
    let dir = scratch("stranger");
    let mut app = window(found, pair.public_key().as_ref().try_into().unwrap(), &dir);
    let state = settle(&mut app);
    assert!(state["install"]["downloaded"].is_null(), "{state}");
    assert!(!dir.join(&name).exists());
    // The file itself was never asked for.
    assert_eq!(asked.lock().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
