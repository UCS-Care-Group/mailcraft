// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The entries that start MailCraft at sign-in: what they hold, and that a path of any shape
//! stays one argument and never breaks out of its line.

use super::*;

#[test]
fn a_desktop_entry_runs_the_program_in_the_background() {
    let entry = desktop_entry(Path::new("/opt/MailCraft/mailcraft")).unwrap();
    assert!(entry.starts_with("[Desktop Entry]\n"));
    assert!(entry.contains("\nExec=\"/opt/MailCraft/mailcraft\" --background\n"), "{entry}");
    assert!(entry.contains("\nType=Application\n"));
    assert!(entry.contains("\nIcon=io.github.billsecond.mailcraft\n"));
    assert!(desktop_entry_on(&entry));
}

#[test]
fn a_path_with_reserved_characters_stays_one_quoted_argument() {
    let entry = desktop_entry(Path::new("/home/o\"ne/$HOME/`x`/100%/back\\slash/sp ace/mailcraft")).unwrap();
    let exec = entry.lines().find_map(|l| l.strip_prefix("Exec=")).unwrap();
    // Argument escapes (\" \$ \` \\), then the string value's (\ doubled); `%` is `%%`.
    assert_eq!(exec, r#""/home/o\\"ne/\\$HOME/\\`x\\`/100%%/back\\\\slash/sp ace/mailcraft" --background"#);
    assert_eq!(entry.lines().count(), 8);
}

#[test]
fn a_path_that_could_break_an_entry_is_refused() {
    for bad in ["relative/mailcraft", "", "/tmp/a\nExec=/bin/sh", "/tmp/a\rb", "/tmp/nul\0"] {
        assert!(desktop_entry(Path::new(bad)).is_err(), "{bad:?}");
        assert!(launch_agent(Path::new(bad)).is_err(), "{bad:?}");
        assert!(run_value(Path::new(bad)).is_err(), "{bad:?}");
    }
    let long = format!("/{}", "a".repeat(MOST_PATH));
    assert!(desktop_entry(Path::new(&long)).is_err());
}

#[test]
fn an_entry_the_desktop_was_told_to_skip_is_off() {
    let entry = desktop_entry(Path::new("/usr/bin/mailcraft")).unwrap();
    assert!(!desktop_entry_on(&format!("{entry}Hidden=true\n")));
    assert!(!desktop_entry_on(&entry.replace("X-GNOME-Autostart-enabled=true", "X-GNOME-Autostart-enabled = false")));
    assert!(!desktop_entry_on("[Desktop Entry]\nType=Application\n"));
    assert!(!desktop_entry_on(""));
    assert!(desktop_entry_on(&format!("{entry}Hidden=false\n")));
}

#[test]
fn a_launch_agent_names_the_program_escaped() {
    let agent = launch_agent(Path::new("/Applications/A & <B>.app/Contents/MacOS/mailcraft")).unwrap();
    assert!(agent.contains("<string>/Applications/A &amp; &lt;B&gt;.app/Contents/MacOS/mailcraft</string>"), "{agent}");
    assert!(agent.contains("<string>--background</string>"));
    assert!(agent.contains("<key>RunAtLoad</key>\n\t<true/>"));
    assert!(agent.contains("<string>io.github.billsecond.mailcraft</string>"));
}

#[test]
fn a_run_value_quotes_the_program() {
    assert_eq!(run_value(Path::new("/Program Files/MailCraft/mailcraft.exe")).unwrap(), "\"/Program Files/MailCraft/mailcraft.exe\" --background");
}

#[test]
fn the_autostart_folder_follows_xdg_config_home() {
    let id = "autostart/io.github.billsecond.mailcraft.desktop";
    assert_eq!(autostart_path(Some("/x/config".into()), Some("/home/o".into())), Some(PathBuf::from("/x/config").join(id)));
    // A relative XDG_CONFIG_HOME is ignored, as the specification says.
    assert_eq!(autostart_path(Some("rel".into()), Some("/home/o".into())), Some(PathBuf::from("/home/o/.config").join(id)));
    assert_eq!(autostart_path(None, None), None);
}

#[test]
fn an_entry_is_written_whole_and_removed_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("autostart").join("io.github.billsecond.mailcraft.desktop");
    let entry = desktop_entry(Path::new("/usr/bin/mailcraft")).unwrap();
    write_entry(&path, &entry).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), entry);
    write_entry(&path, &entry.replace("/usr/bin", "/opt")).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("\"/opt/mailcraft\""));
    // Nothing else is left in the folder.
    assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    remove_entry(&path).unwrap();
    assert!(!path.exists());
    remove_entry(&path).unwrap();
}
