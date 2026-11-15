// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The way to gpg-agent, against a stand-in agent of the test's own on a socket in a temporary
//! folder or on the loopback address: never a real agent or a real key.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;

use mailcraft_assuan::{Keygrip, PinentryHints};

use super::gpg_agent::{ANSWER_TIMEOUT, Gpgconf, SHORT_TIMEOUT, Unavailable, find_gpgconf, gpgconf_candidates, open, owner_wait, socket_from_output};

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
fn gpgconf_is_looked_for_on_the_path_first_but_on_windows_where_its_installers_put_it() {
    let path = std::env::join_paths([temp("path-a"), PathBuf::from("relative"), PathBuf::from("."), PathBuf::new()]).unwrap();
    let env = move |name: &str| match name {
        "PATH" => Some(path.clone()),
        "ProgramFiles(x86)" => Some("C:\\Program Files (x86)".into()),
        // Not absolute, so not used.
        "ProgramFiles" => Some("Program Files".into()),
        _ => None,
    };
    let candidates = gpgconf_candidates(&env);
    assert!(candidates.iter().all(|c| c.is_absolute()), "{candidates:?}");
    assert!(!candidates.iter().any(|c| c.starts_with("relative") || c.starts_with(".")));
    #[cfg(not(windows))]
    {
        assert_eq!(candidates.first(), Some(&temp("path-a").join("gpgconf")));
        assert_eq!(candidates.get(1), Some(&PathBuf::from("/usr/local/MacGPG2/bin/gpgconf")));
        assert_eq!(candidates.last(), Some(&PathBuf::from("/usr/bin/gpgconf")));
    }
    #[cfg(windows)]
    {
        assert_eq!(candidates.first(), Some(&PathBuf::from("C:\\Program Files (x86)\\GnuPG\\bin\\gpgconf.exe")));
        assert_eq!(candidates.get(1), Some(&PathBuf::from("C:\\Program Files (x86)\\Gpg4win\\bin\\gpgconf.exe")));
        assert_eq!(candidates.last(), Some(&temp("path-a").join("gpgconf.exe")));
    }
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

#[test]
fn a_gpgconf_that_cant_be_started_is_gnupg_not_working() {
    let dir = temp("unstartable");
    let e = Gpgconf::at(dir.join("gpgconf")).agent_socket().unwrap_err();
    assert!(matches!(&e, Unavailable::NotWorking(why) if why.contains("couldn't be started")), "{e:?}");
    assert!(e.to_string().starts_with("GnuPG is installed but doesn't work"));
}

#[test]
fn the_connection_waits_briefly_but_long_while_the_owner_is_at_the_pinentry() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let dir = temp("timeouts");
    let mut file = format!("{port}\n").into_bytes();
    file.extend_from_slice(b"0123456789abcdef");
    std::fs::write(dir.join("S.gpg-agent"), &file).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut nonce = [0u8; 16];
        stream.read_exact(&mut nonce).unwrap();
        serve(stream)
    });
    let mut agent = open(&dir.join("S.gpg-agent"), &PinentryHints::default()).unwrap();
    assert_eq!(agent.get_mut().read_timeout().unwrap(), Some(SHORT_TIMEOUT));
    owner_wait(agent.get_mut(), true);
    assert_eq!(agent.get_mut().read_timeout().unwrap(), Some(ANSWER_TIMEOUT));
    owner_wait(agent.get_mut(), false);
    assert_eq!(agent.get_mut().read_timeout().unwrap(), Some(SHORT_TIMEOUT));
    drop(agent.close());
    server.join().unwrap();
}

/// A stand-in `gpgconf`: a shell script run by `/bin/sh` (so no new program is executed, which
/// can race with another test's fork).
#[cfg(unix)]
fn script(dir: &std::path::Path, body: &str, timeout: std::time::Duration) -> Gpgconf {
    let path = dir.join("gpgconf.sh");
    std::fs::write(&path, body).unwrap();
    Gpgconf::script(PathBuf::from("/bin/sh"), path, timeout)
}

#[cfg(unix)]
#[test]
fn a_gpgconf_that_fails_hangs_or_names_nothing_is_gnupg_not_working() {
    use std::time::{Duration, Instant};
    let dir = temp("failing");
    let not_working = |g: &Gpgconf| match g.agent_socket() {
        Err(Unavailable::NotWorking(why)) => why,
        other => panic!("{other:?}"),
    };
    assert_eq!(not_working(&script(&dir, "exit 3\n", Duration::from_secs(5))), "gpgconf ended with error 3");
    assert_eq!(not_working(&script(&dir, "exit 0\n", Duration::from_secs(5))), "gpgconf names no agent socket");
    assert_eq!(not_working(&script(&dir, "echo relative/S.gpg-agent\n", Duration::from_secs(5))), "gpgconf names no agent socket");
    let started = Instant::now();
    assert_eq!(not_working(&script(&dir, "exec sleep 30\n", Duration::from_secs(1))), "gpgconf didn't finish in time");
    assert!(started.elapsed() < Duration::from_secs(10));
    let gpgconf = script(&dir, "exit 3\n", Duration::from_secs(5));
    assert!(matches!(super::gpg_agent::connect_with(&gpgconf, &hints()), Err(Unavailable::NotWorking(_))));
}

#[cfg(unix)]
#[test]
fn a_program_gpgconf_leaves_running_doesnt_hold_it_up() {
    use std::time::{Duration, Instant};
    let dir = temp("lingering");
    // What gpgconf starts keeps its output open long after gpgconf has ended.
    let gpgconf = script(&dir, "sleep 30 &\necho /run/S.gpg-agent\n", Duration::from_secs(3));
    let started = Instant::now();
    assert_eq!(gpgconf.agent_socket().unwrap(), PathBuf::from("/run/S.gpg-agent"));
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    // Without a line feed the output never ends while the program it left runs: that is waited
    // for no longer than gpgconf's own time, not until the program ends.
    let gpgconf = script(&dir, "sleep 30 &\nprintf /run/S.gpg-agent\n", Duration::from_secs(2));
    let started = Instant::now();
    assert_eq!(gpgconf.agent_socket(), Err(Unavailable::NotWorking("gpgconf's output didn't end".into())));
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    let gpgconf = script(&dir, "sleep 30 &\n", Duration::from_secs(3));
    let started = Instant::now();
    gpgconf.launch().unwrap();
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
}

#[cfg(unix)]
#[test]
fn a_socket_name_that_isnt_utf8_is_taken_as_its_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let path = socket_from_output(b"/tmp/gnupg-\xff/S.gpg-agent\n").unwrap();
    assert_eq!(path.as_os_str().as_bytes(), b"/tmp/gnupg-\xff/S.gpg-agent");
}

/// `gpgconf` that names `socket`, and whose `--launch` waits for the test to bring the agent back
/// (the test writes `ready`).
#[cfg(unix)]
fn launching(dir: &std::path::Path, socket: &std::path::Path) -> Gpgconf {
    let body = format!(
        "case \"$1\" in\n--list-dirs) echo '{}' ;;\n--launch) touch '{dir}/launched'; i=0; while [ ! -e '{dir}/ready' ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done ;;\nesac\n",
        socket.display(),
        dir = dir.display()
    );
    script(dir, &body, std::time::Duration::from_secs(15))
}

#[cfg(unix)]
fn wait_for(path: &std::path::Path) {
    for _ in 0..400 {
        if path.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("{} never came", path.display());
}

#[cfg(unix)]
#[test]
fn a_stale_socket_is_launched_past() {
    let dir = temp("stale-unix");
    let socket = dir.join("S.gpg-agent");
    // An agent that went away leaves its socket behind.
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    assert!(socket.exists());
    let gpgconf = launching(&dir, &socket);
    let agent_dir = dir.clone();
    let server = std::thread::spawn(move || {
        wait_for(&agent_dir.join("launched"));
        std::fs::remove_file(agent_dir.join("S.gpg-agent")).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(agent_dir.join("S.gpg-agent")).unwrap();
        std::fs::write(agent_dir.join("ready"), b"").unwrap();
        let (stream, _) = listener.accept().unwrap();
        serve(stream)
    });
    let mut agent = super::gpg_agent::connect_with(&gpgconf, &PinentryHints::default()).unwrap();
    assert!(agent.have_key(&Keygrip::parse(GRIP).unwrap()).unwrap());
    drop(agent.close());
    assert_eq!(server.join().unwrap(), [format!("HAVEKEY {GRIP}"), "BYE".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_stale_port_file_is_launched_past() {
    let dir = temp("stale-port");
    let socket = dir.join("S.gpg-agent");
    // A port nothing listens on any more.
    let gone = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut file = format!("{gone}\n").into_bytes();
    file.extend_from_slice(b"oldnonce-oldnonc");
    std::fs::write(&socket, &file).unwrap();
    let gpgconf = launching(&dir, &socket);
    let agent_dir = dir.clone();
    let server = std::thread::spawn(move || {
        wait_for(&agent_dir.join("launched"));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut file = format!("{}\n", listener.local_addr().unwrap().port()).into_bytes();
        file.extend_from_slice(b"newnonce-newnonc");
        std::fs::write(agent_dir.join("S.gpg-agent"), &file).unwrap();
        std::fs::write(agent_dir.join("ready"), b"").unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let mut nonce = [0u8; 16];
        stream.read_exact(&mut nonce).unwrap();
        (nonce, serve(stream))
    });
    let mut agent = super::gpg_agent::connect_with(&gpgconf, &PinentryHints::default()).unwrap();
    assert!(agent.have_key(&Keygrip::parse(GRIP).unwrap()).unwrap());
    drop(agent.close());
    let (nonce, lines) = server.join().unwrap();
    assert_eq!(&nonce, b"newnonce-newnonc");
    assert_eq!(lines, [format!("HAVEKEY {GRIP}"), "BYE".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn the_agent_is_launched_before_the_first_open() {
    let dir = temp("launch-first");
    let socket = dir.join("S.gpg-agent");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let body = format!(
        "case \"$1\" in\n--list-dirs) echo '{}' ;;\n--launch) echo launch >> '{}' ;;\nesac\n",
        socket.display(),
        dir.join("launches").display()
    );
    let gpgconf = script(&dir, &body, std::time::Duration::from_secs(5));
    let server = std::thread::spawn(move || serve(listener.accept().unwrap().0));
    let agent = super::gpg_agent::connect_with(&gpgconf, &PinentryHints::default()).unwrap();
    drop(agent.close());
    server.join().unwrap();
    // Once, before the socket was opened, and not again: the first open worked.
    assert_eq!(std::fs::read_to_string(dir.join("launches")).unwrap(), "launch\n");
    let _ = std::fs::remove_dir_all(&dir);
}
