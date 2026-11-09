// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The way to gpg-agent, against a stand-in agent of the test's own on a socket in a temporary
//! folder or on the loopback address: never a real agent or a real key.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;

use mailcraft_assuan::{Keygrip, PinentryHints};

use super::gpg_agent::{Unavailable, find_gpgconf, gpgconf_candidates, open, socket_from_output};

const GRIP: &str = "0123456789ABCDEF0123456789ABCDEF01234567";

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-gpg-agent-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A stand-in agent on one connection: greets, says OK to everything and has the key, and hands
/// back the lines it was sent.
fn serve(stream: impl Read + Write) -> Vec<String> {
    let mut reader = BufReader::new(stream);
    reader.get_mut().write_all(b"OK Pleased to meet you\n").unwrap();
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end().to_string();
        reader.get_mut().write_all(b"OK\n").unwrap();
        let bye = line == "BYE";
        lines.push(line);
        if bye {
            break;
        }
    }
    lines
}

fn hints() -> PinentryHints {
    PinentryHints { display: Some(":0".into()), ..PinentryHints::default() }
}

#[test]
fn the_socket_gpgconf_names() {
    assert_eq!(
        socket_from_output(b"/run/user/1000/gnupg/S.gpg-agent\n"),
        Some(PathBuf::from("/run/user/1000/gnupg/S.gpg-agent")).filter(|p| p.is_absolute())
    );
    assert_eq!(
        socket_from_output(b"/home/ann/.gnupg/S.gpg%25agent\r\nmore\n"),
        Some(PathBuf::from("/home/ann/.gnupg/S.gpg%agent")).filter(|p| p.is_absolute())
    );
    #[cfg(windows)]
    assert_eq!(
        socket_from_output(b"C%3a\\Users\\ann\\AppData\\Local\\gnupg\\S.gpg-agent\r\n"),
        Some(PathBuf::from("C:\\Users\\ann\\AppData\\Local\\gnupg\\S.gpg-agent"))
    );
    for junk in [&b""[..], b"\n", b"relative/S.gpg-agent\n", b"/a\x07b\n", b"/a%07b\n", b"\xff\xfe\n"] {
        assert_eq!(socket_from_output(junk), None, "{junk:?}");
    }
}

#[test]
fn gpgconf_is_looked_for_where_the_installers_put_it_then_on_the_path() {
    let path = std::env::join_paths([temp("path-a"), PathBuf::from("relative")]).unwrap();
    let env = move |name: &str| match name {
        "PATH" => Some(path.clone()),
        "ProgramFiles(x86)" => Some("C:\\Program Files (x86)".into()),
        _ => None,
    };
    let candidates = gpgconf_candidates(&env);
    assert!(candidates.last().unwrap().starts_with(temp("path-a")));
    assert!(candidates.iter().all(|c| c.is_absolute() || cfg!(windows)));
    assert!(!candidates.iter().any(|c| c.starts_with("relative")));
    #[cfg(not(windows))]
    assert_eq!(candidates.first(), Some(&PathBuf::from("/usr/local/MacGPG2/bin/gpgconf")));
    #[cfg(windows)]
    assert_eq!(candidates.first(), Some(&PathBuf::from("C:\\Program Files (x86)\\GnuPG\\bin\\gpgconf.exe")));
}

#[test]
fn no_gpgconf_is_no_gnupg() {
    let dir = temp("none");
    assert_eq!(find_gpgconf(&[dir.join("gpgconf"), dir.clone()]), None);
    std::fs::write(dir.join("gpgconf"), b"").unwrap();
    assert_eq!(find_gpgconf(&[dir.clone(), dir.join("gpgconf")]), Some(dir.join("gpgconf")));
    assert_eq!(Unavailable::NoGnupg.to_string(), "GnuPG isn't installed on this computer");
}

#[test]
fn a_missing_or_junk_socket_is_no_agent() {
    let dir = temp("junk");
    assert!(matches!(open(&dir.join("S.gpg-agent"), &hints()), Err(Unavailable::NoAgent(_))));
    std::fs::write(dir.join("S.junk"), b"not a socket file").unwrap();
    assert!(matches!(open(&dir.join("S.junk"), &hints()), Err(Unavailable::NoAgent(_))));
    std::fs::write(dir.join("S.big"), vec![b'1'; 10_000]).unwrap();
    assert!(matches!(open(&dir.join("S.big"), &hints()), Err(Unavailable::NoAgent(_))));
    // A redirect to a redirect is not followed.
    std::fs::write(dir.join("S.two"), format!("%Assuan%\nsocket={}\n", dir.join("S.one").display())).unwrap();
    std::fs::write(dir.join("S.one"), format!("%Assuan%\nsocket={}\n", dir.join("S.two").display())).unwrap();
    assert!(matches!(open(&dir.join("S.one"), &hints()), Err(Unavailable::NoAgent(_))));
}

#[test]
fn the_loopback_socket_file_sends_its_nonce_first() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let nonce = *b"0123456789abcdef";
    let dir = temp("loopback");
    let mut file = format!("{port}\n").into_bytes();
    file.extend_from_slice(&nonce);
    std::fs::write(dir.join("S.gpg-agent"), &file).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut got = [0u8; 16];
        stream.read_exact(&mut got).unwrap();
        (got, serve(stream))
    });
    let mut agent = open(&dir.join("S.gpg-agent"), &hints()).unwrap();
    assert!(agent.have_key(&Keygrip::parse(GRIP).unwrap()).unwrap());
    drop(agent.close());
    let (got, lines) = server.join().unwrap();
    assert_eq!(got, nonce);
    assert_eq!(lines, ["OPTION display=:0".to_string(), format!("HAVEKEY {GRIP}"), "BYE".into()]);
}

#[cfg(unix)]
#[test]
fn a_unix_socket_and_a_redirect_to_one() {
    let dir = temp("unix");
    let socket = dir.join("S.gpg-agent");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    std::fs::write(dir.join("S.redirect"), format!("%Assuan%\nsocket={}\n", socket.display())).unwrap();
    let server = std::thread::spawn(move || {
        let mut all = Vec::new();
        for _ in 0..2 {
            let (stream, _) = listener.accept().unwrap();
            all.push(serve(stream));
        }
        all
    });
    for name in ["S.gpg-agent", "S.redirect"] {
        let mut agent = open(&dir.join(name), &PinentryHints::default()).unwrap();
        assert!(agent.have_key(&Keygrip::parse(GRIP).unwrap()).unwrap());
        drop(agent.close());
    }
    let all = server.join().unwrap();
    assert_eq!(all, vec![vec![format!("HAVEKEY {GRIP}"), "BYE".into()]; 2]);
    let _ = std::fs::remove_dir_all(&dir);
}
