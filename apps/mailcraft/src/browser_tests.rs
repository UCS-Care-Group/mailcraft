// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pages opened in the system's browser: what is allowed out, what it is written as, and the
//! commands that launch it (never through a shell, never with an address an option could be).

use super::*;

/// Records what it was asked to open, and with which token.
#[derive(Default)]
struct Fake(Mutex<Vec<(String, Option<String>)>>);

impl Launch for Fake {
    fn launch(&self, url: &str, token: Option<&str>) -> bool {
        self.0.lock().unwrap().push((url.to_string(), token.map(str::to_string)));
        true
    }
}

fn browser() -> (Arc<Fake>, Browser<u32>) {
    let fake = Arc::new(Fake::default());
    (fake.clone(), Browser::new(fake))
}

#[test]
fn a_page_opens_with_the_token_the_window_was_given() {
    let (fake, browser) = browser();
    let (_, rx) = browser.open("https://login.example/authorize");
    let now = Instant::now();
    let next = browser.pump(&mut || Some(7), now);
    assert_eq!(next, Some(TOKEN_WAIT));
    assert!(fake.0.lock().unwrap().is_empty(), "waits for its token");
    browser.token_arrived(&8, "other".into());
    browser.token_arrived(&7, "tok-7".into());
    assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    assert_eq!(*fake.0.lock().unwrap(), vec![("https://login.example/authorize".to_string(), Some("tok-7".to_string()))]);
    // Nothing left waiting, and a late answer for it opens nothing more.
    browser.token_arrived(&7, "tok-7".into());
    assert_eq!(browser.pump(&mut || None, now), None);
}

#[test]
fn without_a_token_a_page_opens_at_once_or_when_the_wait_is_over() {
    let (fake, browser) = browser();
    let (_, rx) = browser.open("https://one.example/");
    browser.pump(&mut || None, Instant::now());
    assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    let (_, rx) = browser.open("https://two.example/");
    let then = Instant::now();
    browser.pump(&mut || Some(1), then);
    assert_eq!(browser.pump(&mut || Some(2), then + TOKEN_WAIT / 2), Some(TOKEN_WAIT / 2));
    assert_eq!(browser.pump(&mut || Some(3), then + TOKEN_WAIT), None);
    assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    let opened = fake.0.lock().unwrap().clone();
    assert_eq!(opened, vec![("https://one.example/".to_string(), None), ("https://two.example/".to_string(), None)]);
}

#[test]
fn followed_links_wait_for_tokens_too_and_their_number_is_capped() {
    let (fake, browser) = browser();
    browser.follow((0..40).map(|i| format!("https://site.example/{i}")).collect());
    let mut serial = 0;
    browser.pump(
        &mut || {
            serial += 1;
            Some(serial)
        },
        Instant::now(),
    );
    // The ones past the cap don't wait.
    let deadline = Instant::now() + Duration::from_secs(5);
    while fake.0.lock().unwrap().len() < 40 - MAX_WAITING && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fake.0.lock().unwrap().len(), 40 - MAX_WAITING);
    assert!(fake.0.lock().unwrap().iter().all(|(_, token)| token.is_none()));
    browser.token_arrived(&1, "t1".into());
    let deadline = Instant::now() + Duration::from_secs(5);
    while fake.0.lock().unwrap().len() < 40 - MAX_WAITING + 1 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(fake.0.lock().unwrap().contains(&("https://site.example/0".to_string(), Some("t1".to_string()))));
}

#[test]
fn a_page_the_window_never_takes_is_opened_by_whoever_waits() {
    let (fake, browser) = browser();
    // No window pumps: the page is withdrawn and opened here, without a token.
    assert!(browser.open_and_wait("https://login.example/"));
    assert_eq!(*fake.0.lock().unwrap(), vec![("https://login.example/".to_string(), None)]);
    assert_eq!(browser.pump(&mut || Some(1), Instant::now()), None, "nothing left behind");
}

#[test]
fn the_windows_links_are_taken_out_of_its_output() {
    let ctx = egui::Context::default();
    let mut taken = Vec::new();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.ctx().open_url(egui::OpenUrl::new_tab("https://news.example/a"));
        ui.ctx().copy_text("kept".into());
        ui.ctx().open_url(egui::OpenUrl::same_tab("https://news.example/b"));
        taken = take_links(ui.ctx());
    });
    assert_eq!(taken, ["https://news.example/a", "https://news.example/b"]);
    output.textures_delta.clear();
    assert!(matches!(output.platform_output.commands.as_slice(), [egui::OutputCommand::CopyText(t)] if t == "kept"));
}

#[test]
fn web_addresses_the_parser_rejects_are_refused() {
    // `webbrowser` would take these for relative file paths and open a `file:` address.
    for url in ["http://a b/../../../etc/hosts", "https://x:99999/", "http://[bad/x", "http://", "https://"] {
        assert!(!openable(url), "{url:?}");
        assert!(!system_open(url), "{url:?}");
    }
    assert!(openable("https://a.example:8443/x?y#z"));
}

#[test]
fn a_followed_link_is_checked_and_launched_as_the_parser_writes_it() {
    // AU2-V1: every way in passes the same last check, on every system.
    let (fake, browser) = browser();
    browser.follow(vec!["file:///etc/passwd".into(), "http://[bad/x".into(), "HTTP://EXAMPLE.org/a b".into(), "mailto:a@example.org".into()]);
    let _ = browser.pump(&mut || None, Instant::now());
    for _ in 0..100 {
        if fake.0.lock().unwrap().len() >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(50));
    let mut launched: Vec<String> = fake.0.lock().unwrap().iter().map(|(u, _)| u.clone()).collect();
    launched.sort();
    assert_eq!(launched, ["http://example.org/a%20b", "mailto:a@example.org"]);
    assert_eq!(normalised("file:///x"), None);
    assert_eq!(normalised("HTTPS://Example.org").as_deref(), Some("https://example.org/"));
}

#[test]
fn only_web_mail_and_phone_addresses_reach_the_system() {
    for url in ["https://a.example/", "HTTP://a.example", "mailto:ada@example.org", "tel:+441234"] {
        assert!(openable(url), "{url:?}");
    }
    for url in
        ["file:///etc/passwd", "smb://host/share", "ms-msdt:/id", "search-ms:query=x", "javascript:x", "data:,x", "cid:x", "", "https://a.example/\n"]
    {
        assert!(!openable(url), "{url:?}");
        assert!(!system_open(url), "{url:?}");
    }
}

#[test]
fn the_opener_is_given_the_token_and_nothing_an_address_could_misuse() {
    let command = opener_command("https://login.example/authorize?x=1", "abc_123").unwrap();
    assert_eq!(command.get_program(), "xdg-open");
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["https://login.example/authorize?x=1"]);
    let envs: Vec<_> = command.get_envs().map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned()))).collect();
    assert!(envs.contains(&("XDG_ACTIVATION_TOKEN".into(), Some("abc_123".into()))));
    assert!(envs.contains(&("DESKTOP_STARTUP_ID".into(), Some("abc_123".into()))));
    for url in ["-x https://a.example/", "mailto:a@example.org", "file:///etc/passwd", "https://a.example/\nb", "", "ht"] {
        assert!(opener_command(url, "abc").is_none(), "{url:?}");
    }
    assert!(opener_command("https://a.example/", "a b").is_none());
    assert!(opener_command("https://a.example/", "").is_none());
}

/// Every page the program opens comes through this module: nothing else names the opener.
#[test]
fn nothing_else_opens_a_page() {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let roots = [here.join("src"), here.join("../../crates")];
    let mut stack: Vec<std::path::PathBuf> = roots.to_vec();
    let mut offenders = Vec::new();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        // ponytail: opened_files.rs opens saved attachments (files, not pages) with `open`;
        // it could take the activation token too if those programs should come in front.
        } else if path.extension().is_some_and(|e| e == "rs")
            && !path.ends_with("src/browser.rs")
            && !path.ends_with("src/browser_tests.rs")
            && !path.ends_with("src/opened_files.rs")
            && !path.ends_with("src/file_launcher.rs")
            && !path.ends_with("src/file_launcher_tests.rs")
        {
            let text = std::fs::read_to_string(&path).unwrap();
            if text.contains(concat!("webbrowser", "::")) || text.contains(concat!("open", "::that")) || text.contains(concat!("\"xdg", "-open\"")) {
                offenders.push(path);
            }
        }
    }
    assert!(offenders.is_empty(), "pages opened outside browser.rs: {offenders:?}");
}

#[test]
fn mail_and_phone_addresses_are_written_without_spaces_or_quotes() {
    assert_eq!(normalised("MAILTO:ada@example.org?subject=Hi there").as_deref(), Some("mailto:ada@example.org?subject=Hi%20there"));
    assert_eq!(normalised("mailto:\"A B\" <a@example.org>").as_deref(), Some("mailto:%22A%20B%22%20%3Ca@example.org%3E"));
    assert_eq!(normalised("tel:+44 1234").as_deref(), Some("tel:+44%201234"));
    for url in ["mailto:", "tel:", "mailto:a\\b@example.org", "mailto:a@example.org\n", "mailto"] {
        assert_eq!(normalised(url), None, "{url:?}");
    }
}

/// What every launcher is handed: one argument, the scheme first, nothing a command line splits.
fn arguments(command: &std::process::Command) -> Vec<String> {
    command.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
}

const TRICKS: [&str; 14] = [
    "https://bank.example@evil.example/",
    "https://a.example/?a=1&b=2|c^d%41",
    "https://a.example/x\"; calc.exe; \"",
    "https://a.example/ -inject --flag",
    "https://a.example/\" --new-window file:///etc/passwd",
    "https://b\u{fc}cher.example/a b",
    "https://[::1]:8080/",
    "http://2130706433/",
    "mailto:a@example.org?subject=\"x\" & dir",
    "mailto:a@example.org?body=100% sure ^ done",
    "tel:+44 1234",
    "-https://a.example/",
    "--help",
    "/? https://a.example/",
];

#[test]
fn the_windows_command_is_a_program_with_the_address_as_its_one_argument() {
    let root = Some(std::ffi::OsString::from("C:\\Windows"));
    for tricky in TRICKS {
        let Some(url) = normalised(tricky) else { continue };
        let command = windows_command(&url, root.clone()).unwrap();
        assert_eq!(arguments(&command), ["url.dll,FileProtocolHandler".to_string(), url.clone()], "{tricky:?}");
        assert!(url.bytes().all(|b| b.is_ascii_graphic() && b != b'"'), "{url:?}");
        assert!(url.starts_with("http:") || url.starts_with("https:") || url.starts_with("mailto:") || url.starts_with("tel:"), "{url:?}");
        let program = command.get_program().to_string_lossy().into_owned();
        assert!(program.ends_with("rundll32.exe") && program.starts_with("C:\\Windows"), "{program}");
    }
    assert_eq!(windows_command("https://a.example/", None).unwrap().get_program(), "rundll32.exe");
    assert_eq!(windows_command("https://a.example/", Some("".into())).unwrap().get_program(), "rundll32.exe");
}

#[test]
fn nothing_but_a_plain_checked_address_is_given_to_a_launcher() {
    for url in [
        "-x",
        "--help",
        "file:///etc/passwd",
        "https://a.example/ b",
        "https://a.example/\"q\"",
        "https://a.example/\n",
        "https://a.example/\\",
        "",
        "ht",
        "https:",
        "xhttps://a.example/",
        "https://caf\u{e9}.example/",
    ] {
        assert!(windows_command(url, None).is_none(), "{url:?}");
        assert!(macos_command(url).is_none(), "{url:?}");
        assert!(unix_commands(url, Some("firefox")).is_empty(), "{url:?}");
    }
    let long = format!("https://a.example/{}", "x".repeat(70_000));
    assert!(windows_command(&long, None).is_none());
}

#[test]
fn the_macos_command_is_open_with_the_address_alone() {
    for tricky in TRICKS {
        let Some(url) = normalised(tricky) else { continue };
        let command = macos_command(&url).unwrap();
        assert_eq!(command.get_program(), "/usr/bin/open");
        assert_eq!(arguments(&command), [url]);
    }
}

#[test]
fn the_other_systems_run_the_browser_variable_and_the_desktops_openers_without_a_shell() {
    let url = "https://a.example/?q=1&r=%3B";
    let commands = unix_commands(url, Some("firefox --new-tab %s:lynx"));
    let programs: Vec<_> = commands.iter().map(|c| c.get_program().to_string_lossy().into_owned()).collect();
    assert_eq!(programs[..3], ["firefox", "lynx", "xdg-open"]);
    assert_eq!(arguments(&commands[0]), ["--new-tab", url]);
    assert_eq!(arguments(&commands[1]), [url]);
    assert!(programs.contains(&"gio".to_string()));
    let gio = commands.iter().find(|c| c.get_program() == "gio").unwrap();
    assert_eq!(arguments(gio), ["open", url]);
    // No shell is ever the program, and the address is always one whole argument.
    assert!(!programs.iter().any(|p| matches!(p.as_str(), "sh" | "bash" | "cmd" | "cmd.exe" | "powershell")));
    assert!(commands.iter().all(|c| arguments(c).iter().filter(|a| a.contains(url)).count() == 1));
    // An empty or missing variable leaves the desktops' own.
    assert_eq!(unix_commands(url, None).len(), 6);
    assert_eq!(unix_commands(url, Some(": :")).len(), 6);
    // `%s` inside a word stays in that word.
    let commands = unix_commands("https://a.example/", Some("br --url=%s"));
    assert_eq!(arguments(&commands[0]), ["--url=https://a.example/"]);
}

#[test]
fn an_address_too_long_for_windows_command_line_is_refused_not_cut() {
    let fits = format!("https://a.example/{}", "x".repeat(WINDOWS_MAX_ARGUMENT - 30));
    assert!(windows_command(&fits, None).is_some());
    let too_long = format!("https://a.example/{}", "x".repeat(WINDOWS_MAX_ARGUMENT));
    assert!(normalised(&too_long).is_some(), "other systems still take it");
    assert!(windows_command(&too_long, None).is_none());
    // The least that is refused is under the limit of a command line, with the program in front.
    const { assert!(WINDOWS_MAX_ARGUMENT + 200 < 32_767) };
}

#[test]
fn an_address_over_the_link_limit_is_refused_before_it_is_written_out() {
    let huge = "%".repeat(mailcraft_link::MAX_LINK + 1);
    for scheme in ["mailto:", "tel:", "https://a.example/"] {
        assert!(normalised(&format!("{scheme}{huge}")).is_none(), "{scheme}");
    }
    assert!(normalised("mailto:noor@example.org?subject=100%").is_some_and(|u| u.ends_with("100%")));
}
