//! Outlook `.msg` files through the command line: `import` brings one into a mailbox, `convert`
//! makes an `.eml` of it. The file is made in code from synthetic content.
#![cfg(not(target_arch = "wasm32"))]

use std::process::Command;

use mailcraft_msg::writer::Props;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mailcraft-cli"))
}

struct TempDir(std::path::PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> TempDir {
    let dir = std::env::temp_dir().join(format!("mailcraft-cli-msg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn message() -> Vec<u8> {
    Props::new()
        .unicode(0x0037, "Catering for Thursday")
        .unicode(0x0C1A, "Sky Morgan")
        .unicode(0x5D01, "sky@example.org")
        .time(0x0039, 1_700_000_000)
        .unicode(0x1000, "Sandwiches for twelve.")
        .recipient(Props::new().int(0x0C15, 1).unicode(0x3001, "Avery Quinn").unicode(0x39FE, "avery@example.org"))
        .msg()
}

#[test]
fn import_and_convert_a_msg_file() {
    let dir = temp_dir();
    let msg = dir.0.join("Catering.msg");
    std::fs::write(&msg, message()).unwrap();
    let mailbox = dir.0.join("out.mailcraft");

    let out = cli().arg("import").arg(&mailbox).arg(&msg).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = cli().arg("list").arg(&mailbox).output().unwrap();
    let list = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && list.contains("Catering for Thursday") && list.contains("sky@example.org"), "{list}");

    let eml = dir.0.join("Catering.eml");
    let out = cli().arg("convert").arg(&msg).arg(&eml).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = std::fs::read_to_string(&eml).unwrap();
    assert!(
        text.contains("Subject: Catering for Thursday") && text.contains("avery@example.org") && text.contains("Sandwiches for twelve."),
        "{text}"
    );

    // Something that only says it is one.
    let fake = dir.0.join("fake.msg");
    std::fs::write(&fake, b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1 and nothing more").unwrap();
    let out = cli().arg("import").arg(&mailbox).arg(&fake).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && !stderr.contains("panicked") && stderr.contains("Outlook message"), "{stderr}");
}
