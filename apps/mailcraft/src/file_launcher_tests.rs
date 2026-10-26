// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The arguments the openers are given: the path alone, never as an option, never through a shell;
//! and which opener is started where some aren't installed (#445).

use super::*;

fn args(c: &Command) -> Vec<String> {
    c.get_args().map(|a| a.to_string_lossy().to_string()).collect()
}

/// The one command `opener` has for `path`, on a platform with a single opener.
fn only(opener: Opener, path: &str) -> Command {
    let mut commands = commands_for(opener, Path::new(path));
    assert_eq!(commands.len(), 1, "{opener:?}");
    commands.remove(0)
}

fn programs(commands: &[Command]) -> Vec<String> {
    commands.iter().map(|c| c.get_program().to_string_lossy().to_string()).collect()
}

fn not_found() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::NotFound, "no such program")
}

#[test]
fn the_desktop_openers_are_tried_in_turn_each_with_the_path_alone() {
    let p = "/home/u/.cache/MailCraft/Attachments/1/a b.pdf";
    let commands = commands_for(Opener::XdgOpen, Path::new(p));
    assert_eq!(programs(&commands), ["xdg-open", "gio", "kde-open", "gnome-open"]);
    let all: Vec<Vec<String>> = commands.iter().map(args).collect();
    assert_eq!(all, [vec![p], vec!["open", p], vec![p], vec![p]]);
}

#[test]
fn an_opener_that_is_not_installed_gives_way_to_the_next() {
    let commands = commands_for(Opener::XdgOpen, Path::new("/tmp/x.pdf"));
    let mut tried = Vec::new();
    let started = first_started(commands, |c| {
        let program = c.get_program().to_string_lossy().to_string();
        tried.push(program.clone());
        if program == "xdg-open" { Err(not_found()) } else { Ok(program) }
    });
    assert_eq!(started.unwrap(), "gio");
    assert_eq!(tried, ["xdg-open", "gio"]);
}

#[test]
fn any_other_failure_is_the_answer_and_nothing_after_it_is_tried() {
    let commands = commands_for(Opener::XdgOpen, Path::new("/tmp/x.pdf"));
    let mut tried = 0;
    let started: std::io::Result<()> = first_started(commands, |_| {
        tried += 1;
        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "not allowed"))
    });
    assert_eq!(started.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(tried, 1);
}

#[test]
fn when_no_opener_is_installed_the_answer_is_not_found() {
    let commands = commands_for(Opener::XdgOpen, Path::new("/tmp/x.pdf"));
    let mut tried = 0;
    let started: std::io::Result<()> = first_started(commands, |_| {
        tried += 1;
        Err(not_found())
    });
    assert_eq!(started.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    assert_eq!(tried, 4);
}

#[test]
fn macos_open_ends_its_options_first() {
    let c = only(Opener::MacOpen, "/Users/u/Library/Caches/x.pdf");
    assert_eq!(c.get_program(), "open");
    assert_eq!(args(&c), ["--", "/Users/u/Library/Caches/x.pdf"]);
}

#[test]
fn explorer_gets_the_path_alone_and_no_shell_is_named() {
    for p in [r"C:\Users\u\AppData\Local\x.pdf", r"\\server\share\x.pdf", "D:/x.pdf"] {
        let c = only(Opener::Explorer, p);
        assert_eq!(c.get_program(), "explorer.exe");
        assert_eq!(args(&c), [p]);
    }
}

#[test]
fn shell_metacharacters_stay_inside_the_one_argument() {
    let p = "/tmp/a; rm -rf ~ $(x) `y` & | > z.pdf";
    for c in commands_for(Opener::XdgOpen, Path::new(p)) {
        assert_eq!(args(&c).last().map(String::as_str), Some(p));
    }
}

#[test]
fn a_path_that_could_be_an_option_or_is_not_absolute_is_refused() {
    for p in ["", "-h", "--help", "relative/x.pdf", "./x", "file.pdf", "/\0x", "~/x"] {
        let unix = Path::new(p);
        if !p.starts_with('/') || p.contains('\0') {
            assert!(commands_for(Opener::XdgOpen, unix).is_empty(), "{p:?}");
            assert!(commands_for(Opener::MacOpen, unix).is_empty(), "{p:?}");
        }
    }
    for p in ["", "/select,C:\\x", "/e,C:\\", "-x", "x.pdf", "C:x.pdf", "1:\\x", "\\x", "C:\\x\0"] {
        assert!(commands_for(Opener::Explorer, Path::new(p)).is_empty(), "{p:?}");
    }
}

#[test]
fn a_relative_path_is_never_started() {
    assert_eq!(open_detached(Path::new("x.pdf")).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
}
