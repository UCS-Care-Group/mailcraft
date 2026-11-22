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
    let mut commands = commands_for(opener, Target::File, Path::new(path));
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
    let p = "/home/u/.config/MailCraft";
    let commands = commands_for(Opener::XdgOpen, Target::Folder, Path::new(p));
    assert_eq!(programs(&commands), ["xdg-open", "gio", "kde-open"]);
    let all: Vec<Vec<String>> = commands.iter().map(args).collect();
    assert_eq!(all, [vec![p], vec!["open", "--", p], vec!["--", p]]);
    // A file: never `xdg-open`, which types it by what is in it on a desktop it doesn't know.
    let p = "/home/u/.cache/MailCraft/Attachments/1/a b.pdf";
    let commands = commands_for(Opener::XdgOpen, Target::File, Path::new(p));
    assert_eq!(programs(&commands), ["gio", "kde-open", "kde-open5"]);
    let all: Vec<Vec<String>> = commands.iter().map(args).collect();
    assert_eq!(all, [vec!["open", "--", p], vec!["--", p], vec!["--", p]]);
    let typers = typers_for(Path::new(p));
    assert_eq!(programs(&typers), ["gio", "kmimetypefinder", "kmimetypefinder5"]);
    let all: Vec<Vec<String>> = typers.iter().map(args).collect();
    assert_eq!(all, [vec!["info", "--attributes=standard::content-type", "--", p], vec!["--", p], vec!["--", p]]);
    assert!(typers_for(Path::new("x.pdf")).is_empty() && typers_for(Path::new("-h")).is_empty());
}

const PNG: &[&str] = &["image/png", "image/apng"];

/// What `gio info` writes for a file of `kind`.
fn gio_says(kind: &str) -> Vec<u8> {
    format!("uri: file:///tmp/x.png\nlocal path: /tmp/x.png\nattributes:\n  standard::content-type: {kind}\n").into_bytes()
}

/// [`open_typed`] with stand-ins: the typers answer from `say` (by program), and what was run
/// comes back with what was started.
fn typed(kinds: &[&str], mut say: impl FnMut(&str) -> std::io::Result<Vec<u8>>) -> (std::io::Result<String>, Vec<String>) {
    let mut ran = Vec::new();
    let mut started = Vec::new();
    let r = open_typed(
        Path::new("/tmp/x.png"),
        kinds,
        |c| {
            let program = c.get_program().to_string_lossy().to_string();
            ran.push(program.clone());
            say(&program)
        },
        |c| {
            let program = c.get_program().to_string_lossy().to_string();
            started.push(program.clone());
            if program == "kde-open" { Err(not_found()) } else { Ok(program) }
        },
    );
    ran.extend(started.iter().map(|p| format!("started {p}")));
    (r, ran)
}

#[test]
fn a_file_the_desktop_takes_for_another_kind_is_never_opened() {
    // A program sent as `x.png`: GIO types a name with two kinds by what is in the file.
    for said in ["application/vnd.microsoft.portable-executable", "application/x-ms-dos-executable", "text/html", "image/svg+xml", "", "garbage"] {
        let (r, ran) = typed(PNG, |_| Ok(gio_says(said)));
        assert_eq!(r.unwrap_err().kind(), std::io::ErrorKind::InvalidData, "{said}");
        assert_eq!(ran, ["gio"], "{said}: nothing is started");
    }
    // No kind at all (a kind of no known name): nothing is ever accepted.
    let (r, ran) = typed(&[], |_| Ok(gio_says("image/png")));
    assert_eq!((r.unwrap_err().kind(), ran), (std::io::ErrorKind::InvalidData, vec!["gio".to_string()]));
    // A typer that fails is no yes.
    let (r, ran) = typed(PNG, |_| Err(std::io::Error::other("no")));
    assert!(r.is_err() && ran == ["gio"]);
}

#[test]
fn a_file_the_desktop_takes_for_its_kind_is_opened_by_the_same_desktop() {
    let (r, ran) = typed(PNG, |_| Ok(gio_says("image/png")));
    assert_eq!((r.unwrap(), ran), ("gio".to_string(), vec!["gio".to_string(), "started gio".to_string()]));
    // Types are compared without regard to case.
    assert!(typed(PNG, |_| Ok(gio_says("IMAGE/PNG"))).0.is_ok());
    // No GIO: KDE's typer, then its opener (here `kde-open` isn't installed, `kde-open5` is).
    let (r, ran) = typed(PNG, |p| if p == "gio" { Err(not_found()) } else { Ok(b"image/png\n".to_vec()) });
    assert_eq!(r.unwrap(), "kde-open5");
    assert_eq!(ran, ["gio", "kmimetypefinder", "kmimetypefinder5", "started kde-open", "started kde-open5"]);
    // KDE's typer says a program: refused.
    let (r, _) = typed(PNG, |p| if p == "gio" { Err(not_found()) } else { Ok(b"application/x-ms-dos-executable\n".to_vec()) });
    assert_eq!(r.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    // No typer installed: nothing is started, and the owner is told what is missing.
    let (r, ran) = typed(PNG, |_| Err(not_found()));
    assert_eq!(r.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    assert_eq!(ran, ["gio", "kmimetypefinder", "kmimetypefinder5"]);
    let told = mailcraft_ui_egui::no_kind_teller_text();
    assert!(told.contains("gio") && told.contains("kmimetypefinder") && told.contains("Save As"), "{told}");
    // A typer that takes too long is no yes either.
    let (r, ran) = typed(PNG, |_| Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "slow")));
    assert_eq!((r.unwrap_err().kind(), ran), (std::io::ErrorKind::TimedOut, vec!["gio".to_string()]));
}

#[test]
fn inside_flatpak_a_kind_of_several_types_is_not_opened() {
    // The portal types the file again outside, by the computer's own database.
    // Kinds of several types, and kinds an older computer's database doesn't know.
    for name in ["x.png", "x.json", "x.ogg", "x.html", "x.md", "x.jxl", "x.avif", "x.heic", "x.heif"] {
        assert!(sandbox_refused(&kinds_for(name, true).unwrap_err()), "{name}");
        assert!(!kinds_for(name, false).unwrap().is_empty(), "{name}");
    }
    assert_eq!(kinds_for("x.pdf", true).unwrap(), kinds_for("x.pdf", false).unwrap());
    assert!(kinds_for("x.pdf", true).unwrap().contains(&"application/pdf"));
    assert!(kinds_for("x.qzx", false).unwrap().is_empty() && kinds_for("noext", false).unwrap().is_empty());
}

#[test]
fn each_refusal_says_why_it_is() {
    use crate::opened_files::refusal;
    use std::io::ErrorKind;
    // Another kind than the name says: the kind's refusal.
    let e = refusal("x.png", &ErrorKind::InvalidData.into());
    assert!(e.contains("doesn't open x.png") && e.contains("Save As"), "{e}");
    // From the Flatpak sandbox: no word of harm, only what it can't make sure of.
    let e = refusal("x.png", &kinds_for("x.png", true).unwrap_err());
    assert_eq!(e, mailcraft_ui_egui::sandbox_kind_text());
    assert!(e.contains("Flatpak") && e.contains("Save As") && !e.contains("harm"), "{e}");
    // Nothing to ask what kind it is.
    assert_eq!(refusal("x.png", &ErrorKind::Unsupported.into()), mailcraft_ui_egui::no_kind_teller_text());
    // Anything else, a program the system's policy won't start among them: no program could be started.
    for why in [ErrorKind::PermissionDenied, ErrorKind::NotFound, ErrorKind::TimedOut, ErrorKind::Other] {
        let e = std::io::Error::new(why, "denied");
        assert!(!sandbox_refused(&e), "{why:?}");
        assert!(refusal("x.png", &e).starts_with("No program could be started"), "{why:?}");
    }
}

#[cfg(unix)]
#[test]
fn a_typer_past_its_deadline_is_ended_and_refused() {
    // `sleep` stands in for a typer that hangs (no desktop program is run).
    let begun = std::time::Instant::now();
    let mut slow = Command::new("sleep");
    slow.arg("30").stdin(Stdio::null()).stderr(Stdio::null());
    assert_eq!(typer_output_within(&mut slow, std::time::Duration::from_millis(100)).unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    assert!(begun.elapsed() < std::time::Duration::from_secs(10));
    // One that answers in time is read; one that fails is no answer.
    let mut quick = Command::new("echo");
    quick.arg("image/png");
    assert_eq!(said_kind(&typer_output_within(&mut quick, std::time::Duration::from_secs(5)).unwrap()), "image/png");
    assert!(typer_output_within(&mut Command::new("false"), std::time::Duration::from_secs(5)).is_err());
}

#[test]
fn what_a_typer_said_is_its_last_line() {
    assert_eq!(said_kind(&gio_says("image/png")), "image/png");
    assert_eq!(said_kind(b"image/png\n\n"), "image/png");
    // A name that looks like the attribute sits on the path's line, never on the last one.
    let tricky = b"local path: /tmp/standard::content-type: image/png\nattributes:\n  standard::content-type: application/x-ms-dos-executable\n";
    assert_eq!(said_kind(tricky), "application/x-ms-dos-executable");
    assert_eq!(said_kind(b""), "");
    assert_eq!(said_kind(&[0xff, 0xfe]), "\u{fffd}\u{fffd}");
}

/// Every attachment written to be opened goes through `Opened` (which checks the kind) and
/// `open_detached` (which, on Linux and the BSDs, asks the desktop what it takes it for).
#[test]
fn nothing_else_opens_a_file() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut stack = vec![here.join("src"), here.join("../../crates")];
    let mut offenders = Vec::new();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        } else if path.extension().is_some_and(|e| e == "rs")
            && !path.ends_with("src/file_launcher.rs")
            && !path.ends_with("src/file_launcher_tests.rs")
            && !path.ends_with("src/opened_files.rs")
            // Web addresses only (`nothing_else_opens_a_page`).
            && !path.ends_with("src/browser.rs")
            && !path.ends_with("src/browser_tests.rs")
        {
            let text = std::fs::read_to_string(&path).unwrap();
            let openers = [
                concat!("open", "_detached("),
                concat!("\"gio", "\""),
                concat!("\"kde", "-open"),
                concat!("\"xdg", "-open"),
                concat!("\"exo", "-open"),
                concat!("\"gnome", "-open"),
                concat!("\"mime", "open"),
                concat!("\"hand", "lr"),
                concat!("open", "::that"),
                concat!("opener", "::open"),
                concat!("web", "browser::"),
            ];
            if openers.iter().any(|o| text.contains(o)) {
                offenders.push(path);
            }
        }
    }
    assert!(offenders.is_empty(), "files opened outside file_launcher.rs: {offenders:?}");
}

#[test]
fn an_attachment_of_no_known_kind_is_not_written_where_the_desktop_types_by_content() {
    use mailcraft_engine::opening::Chooser;
    let dir = std::env::temp_dir().join(format!("mc-sec2-{}", std::process::id()));
    let opened = crate::opened_files::Opened::at(dir.clone());
    let r = opened.write("invoice.qzx", b"MZ");
    if Chooser::THIS == Chooser::NameOrContent {
        let e = r.unwrap_err();
        assert!(e.contains("doesn't open invoice.qzx") && e.contains("Save As"), "{e}");
    } else {
        assert!(r.is_ok());
    }
    opened.clear();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_opener_that_is_not_installed_gives_way_to_the_next() {
    let commands = commands_for(Opener::XdgOpen, Target::Folder, Path::new("/tmp/x.pdf"));
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
    let commands = commands_for(Opener::XdgOpen, Target::Folder, Path::new("/tmp/x.pdf"));
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
    let commands = commands_for(Opener::XdgOpen, Target::Folder, Path::new("/tmp/x.pdf"));
    let mut tried = 0;
    let started: std::io::Result<()> = first_started(commands, |_| {
        tried += 1;
        Err(not_found())
    });
    assert_eq!(started.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    assert_eq!(tried, 3);
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
    for c in commands_for(Opener::XdgOpen, Target::File, Path::new(p))
        .into_iter()
        .chain(commands_for(Opener::XdgOpen, Target::Folder, Path::new(p)))
        .chain(typers_for(Path::new(p)))
    {
        assert_eq!(args(&c).last().map(String::as_str), Some(p));
    }
}

#[test]
fn a_path_that_could_be_an_option_or_is_not_absolute_is_refused() {
    for p in ["", "-h", "--help", "relative/x.pdf", "./x", "file.pdf", "/\0x", "~/x"] {
        let unix = Path::new(p);
        if !p.starts_with('/') || p.contains('\0') {
            for target in [Target::File, Target::Folder] {
                assert!(commands_for(Opener::XdgOpen, target, unix).is_empty(), "{p:?}");
                assert!(commands_for(Opener::MacOpen, target, unix).is_empty(), "{p:?}");
            }
            assert!(typers_for(unix).is_empty(), "{p:?}");
        }
    }
    for p in ["", "/select,C:\\x", "/e,C:\\", "-x", "x.pdf", "C:x.pdf", "1:\\x", "\\x", "C:\\x\0"] {
        assert!(commands_for(Opener::Explorer, Target::File, Path::new(p)).is_empty(), "{p:?}");
    }
}

#[test]
fn a_relative_path_is_never_started() {
    assert_eq!(open_detached(Path::new("x.pdf")).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn a_folder_that_is_not_absolute_or_not_there_is_refused_before_anything_starts() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let started = std::cell::RefCell::new(Vec::new());
    let open = |p: &Path| {
        open_folder_with(p, |path: &Path| {
            started.borrow_mut().push(path.to_path_buf());
            Ok(())
        })
    };
    // `src` is a folder (the tests run in the crate's own), but not a whole path: without the
    // check it would reach the opener.
    assert!(Path::new("src").is_dir());
    for p in ["src", ".", "", "-h", "--help", "relative/folder", "~/x"] {
        assert_eq!(open(Path::new(p)), Err(FolderTrouble::NotThere), "{p:?}");
    }
    assert_eq!(open(&here.join("no-such-folder-460")), Err(FolderTrouble::NotThere));
    // A file is not a folder.
    assert_eq!(open(&here.join("Cargo.toml")), Err(FolderTrouble::NotThere));
    assert!(started.borrow().is_empty(), "{started:?}");
    // A whole path to a folder reaches it, as it is.
    let src = here.join("src");
    assert_eq!(open(&src), Ok(()));
    assert_eq!(*started.borrow(), [src]);
}

#[test]
fn a_file_manager_that_cannot_start_is_said() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(open_folder_with(here, |_| Err(not_found())), Err(FolderTrouble::NoFileManager));
}

#[test]
fn the_data_folder_is_the_settings_folder_that_removing_the_data_clears_and_none_without_one() {
    // (#460) The folder File › Info opens: where the mailbox and the settings are.
    let shown = crate::data_folder(true).map(|f| f.path);
    assert_eq!(shown, mailcraft_control_key::settings_dir());
    assert_eq!(shown, crate::wipe::Roots::of_this_program().settings);
    assert!(shown.is_none_or(|p| p.is_absolute()));
    // A run that keeps nothing has nothing to show.
    assert!(crate::data_folder(false).is_none());
}
