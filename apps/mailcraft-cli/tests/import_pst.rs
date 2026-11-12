// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `mailcraft-cli import-pst` (#815): a synthetic .pst file made by the reader's test writer
//! (invented people at `example.org`, never a real archive) comes into a mailbox file, its
//! report is printed and written beside it, a second import adds nothing, and a file with a
//! password takes it from the environment.
#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::process::Command;

use mailcraft_pst::test_writer::{self as w, FileSpec, FolderSpec, Layout, MessageSpec};
use mailcraft_pst::{Encryption, Format};
use serde_json::Value;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("mailcraft-cli-import-pst-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn file(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn note(subject: &str) -> MessageSpec {
    MessageSpec {
        class: "IPM.Note".into(),
        subject: subject.into(),
        body: format!("{subject}\r\n\r\nWritten for the tests."),
        delivered: 134_067_498_000_000_000,
        recipients: vec![("Ada Fernsby".into(), "ada@example.org".into(), 1)],
        ..Default::default()
    }
}

fn file(password: Option<&str>) -> Vec<u8> {
    let mut appointment = note("Dentist");
    appointment.class = "IPM.Appointment".into();
    w::build(&FileSpec {
        layout: Layout::new(Format::Unicode, Encryption::Permute),
        password: password.map(str::to_string),
        password_ansi: false,
        name: "Personal Folders".into(),
        folders: vec![
            FolderSpec {
                name: "Inbox".into(),
                class: Some("IPF.Note".into()),
                messages: vec![note("Shed roof"), note("Seed order")],
                children: vec![],
            },
            FolderSpec { name: "Calendar".into(), class: Some("IPF.Appointment".into()), messages: vec![appointment], children: vec![] },
        ],
        named: vec![],
        named_strings: vec![],
    })
}

fn cli(args: &[&str], password: Option<&str>) -> (bool, Value, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mailcraft-cli"));
    cmd.args(args).env_remove("MAILCRAFT_PST_PASSWORD");
    if let Some(p) = password {
        cmd.env("MAILCRAFT_PST_PASSWORD", p);
    }
    let out = cmd.output().unwrap();
    let json = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.success(), json, String::from_utf8_lossy(&out.stderr).to_string())
}

#[test]
fn a_pst_files_mail_comes_into_a_mailbox_once() {
    let dir = Scratch::new("once");
    let (pst, mailbox) = (dir.file("garden.pst"), dir.file("mail.mailcraft"));
    std::fs::write(&pst, file(None)).unwrap();

    let (ok, dry, err) = cli(&["import-pst", &mailbox, &pst, "--dry-run"], None);
    assert!(ok, "{err}");
    assert_eq!((dry["dryRun"].as_bool(), dry["added"].as_u64(), dry["leftOut"].as_u64()), (Some(true), Some(2), Some(1)), "{dry}");
    assert!(!std::path::Path::new(&mailbox).exists(), "a dry run saves nothing");

    let (ok, report, err) = cli(&["import-pst", &mailbox, &pst], None);
    assert!(ok, "{err}");
    assert_eq!(report["added"], 2, "{report}");
    let written = report["report"].as_str().unwrap();
    assert!(std::fs::read_to_string(written).unwrap().contains("Found 3 items: 2 brought in"));
    let (ok, again, err) = cli(&["import-pst", &mailbox, &pst], None);
    assert!(ok, "{err}");
    assert_eq!((again["added"].as_u64(), again["already"].as_u64()), (Some(0), Some(2)), "{again}");
}

#[test]
fn a_password_comes_from_the_environment() {
    let dir = Scratch::new("password");
    let (pst, mailbox) = (dir.file("locked.pst"), dir.file("locked.mailcraft"));
    std::fs::write(&pst, file(Some("compost"))).unwrap();
    let (ok, _, err) = cli(&["import-pst", &mailbox, &pst], None);
    assert!(!ok && err.contains("password"), "{err}");
    let (ok, _, err) = cli(&["import-pst", &mailbox, &pst], Some("mulch"));
    assert!(!ok && err.contains("isn't the password") && !err.contains("mulch"), "{err}");
    let (ok, report, err) = cli(&["import-pst", &mailbox, &pst], Some("compost"));
    assert!(ok, "{err}");
    assert_eq!(report["added"], 2);
}
