// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The arguments the openers are given: the path alone, never as an option, never through a shell.

use super::*;

fn args(c: &Command) -> Vec<String> {
    c.get_args().map(|a| a.to_string_lossy().to_string()).collect()
}

#[test]
fn xdg_open_gets_the_path_alone() {
    let c = command_for(Opener::XdgOpen, Path::new("/home/u/.cache/MailCraft/Attachments/1/a b.pdf")).unwrap();
    assert_eq!(c.get_program(), "xdg-open");
    assert_eq!(args(&c), ["/home/u/.cache/MailCraft/Attachments/1/a b.pdf"]);
}

#[test]
fn macos_open_ends_its_options_first() {
    let c = command_for(Opener::MacOpen, Path::new("/Users/u/Library/Caches/x.pdf")).unwrap();
    assert_eq!(c.get_program(), "open");
    assert_eq!(args(&c), ["--", "/Users/u/Library/Caches/x.pdf"]);
}

#[test]
fn explorer_gets_the_path_alone_and_no_shell_is_named() {
    for p in [r"C:\Users\u\AppData\Local\x.pdf", r"\\server\share\x.pdf", "D:/x.pdf"] {
        let c = command_for(Opener::Explorer, Path::new(p)).unwrap();
        assert_eq!(c.get_program(), "explorer.exe");
        assert_eq!(args(&c), [p]);
    }
}

#[test]
fn shell_metacharacters_stay_inside_the_one_argument() {
    let p = "/tmp/a; rm -rf ~ $(x) `y` & | > z.pdf";
    let c = command_for(Opener::XdgOpen, Path::new(p)).unwrap();
    assert_eq!(args(&c), [p]);
}

#[test]
fn a_path_that_could_be_an_option_or_is_not_absolute_is_refused() {
    for p in ["", "-h", "--help", "relative/x.pdf", "./x", "file.pdf", "/\0x", "~/x"] {
        let unix = Path::new(p);
        if !p.starts_with('/') || p.contains('\0') {
            assert!(command_for(Opener::XdgOpen, unix).is_none(), "{p:?}");
            assert!(command_for(Opener::MacOpen, unix).is_none(), "{p:?}");
        }
    }
    for p in ["", "/select,C:\\x", "/e,C:\\", "-x", "x.pdf", "C:x.pdf", "1:\\x", "\\x", "C:\\x\0"] {
        assert!(command_for(Opener::Explorer, Path::new(p)).is_none(), "{p:?}");
    }
}

#[test]
fn a_relative_path_is_never_started() {
    assert!(open_detached(Path::new("x.pdf")).is_err());
}
