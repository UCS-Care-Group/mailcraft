// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening an attachment with the program the computer opens that kind of file with (#126).
//!
//! The engine never opens anything: a message is hostile, and so is the name its sender gave a
//! file. This is what the desktop host and the window go by before they do:
//! - [`verdict`]: programs, scripts, shortcuts, installers and disk images are never opened
//!   (they can run as soon as they open); Office files with macros and web pages ask first
//!   (macros and scripts are what they are for); anything else opens.
//! - [`file_name`]: the name the file is written under, from the attachment's own name, made
//!   safe to put in a folder: no folders of its own, nothing hidden, nothing that reads as an
//!   option, no name Windows keeps for itself, not too long.
//!
//! `mail.openAttachment` (`cmd::mail`) answers with both and opens nothing itself: the window
//! does the opening, through the host (`Services::open_file`), and agents may not ask for it.

/// What may be done with an attachment the owner asked to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Open it.
    Open,
    /// An Office file that can carry macros: ask first.
    Macros,
    /// A web page or a drawing that can carry scripts, which open in the browser: ask first.
    Scripts,
    /// A program, a script, a shortcut, an installer or a disk image: never opened from mail.
    Blocked,
}

impl Verdict {
    /// Whether the owner is asked before it opens.
    pub fn asks(self) -> bool {
        matches!(self, Verdict::Macros | Verdict::Scripts)
    }
}

/// Kinds of file that run (or run what is in them) when they are opened. Windows, macOS, Linux
/// and the BSDs together; a file of one of these is saved, never opened.
pub const BLOCKED: &[&str] = &[
    // Windows programs, libraries, control panel items, screen savers, installers and updates.
    "exe",
    "com",
    "scr",
    "pif",
    "cpl",
    "dll",
    "ocx",
    "sys",
    "drv",
    "msi",
    "msp",
    "mst",
    "msu",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "application",
    "appref-ms",
    "appinstaller",
    "gadget",
    "xbap",
    "xll",
    "diagcab",
    "msc",
    "ins",
    "isp",
    "inf",
    "reg",
    "scf",
    // Windows scripts and their hosts.
    "bat",
    "cmd",
    "ps1",
    "ps1xml",
    "ps2",
    "ps2xml",
    "psc1",
    "psc2",
    "psd1",
    "psm1",
    "pssc",
    "msh",
    "msh1",
    "msh2",
    "mshxml",
    "msh1xml",
    "msh2xml",
    "vb",
    "vbe",
    "vbs",
    "vbp",
    "js",
    "jse",
    "ws",
    "wsc",
    "wsf",
    "wsh",
    "wsb",
    "hta",
    "htc",
    "sct",
    "shb",
    "shs",
    "chm",
    "hlp",
    "cnt",
    "mde",
    "mda",
    "mdb",
    "mdt",
    "mdw",
    "mdz",
    "ade",
    "adp",
    "accde",
    "bas",
    "fxp",
    "prg",
    "crt",
    "cer",
    "der",
    "udl",
    "printerexport",
    "settingcontent-ms",
    "theme",
    "themepack",
    "webpnp",
    "website",
    // Shortcuts and links that point somewhere else.
    "lnk",
    "url",
    "library-ms",
    "search-ms",
    "searchconnector-ms",
    "glk",
    "xnk",
    "jnlp",
    // Files that connect, fetch or run when Windows or Office opens them: remote desktop, web
    // queries and symbolic links of spreadsheets, Office add-ins, Access shortcuts, Outlook
    // profiles, help projects and Program Manager groups.
    "rdp",
    "iqy",
    "slk",
    "vsto",
    "mam",
    "maf",
    "mag",
    "maq",
    "mar",
    "mas",
    "mat",
    "mau",
    "mav",
    "maw",
    "prf",
    "hpj",
    "grp",
    // Disk images, which open as a drive with whatever is in them.
    "iso",
    "img",
    "vhd",
    "vhdx",
    "dmg",
    "sparseimage",
    "sparsebundle",
    // Java, Python, Perl, Ruby, PHP, Lua and the shells.
    "jar",
    "class",
    "py",
    "pyc",
    "pyo",
    "pyw",
    "pyz",
    "pyzw",
    "pl",
    "pm",
    "rb",
    "php",
    "lua",
    "tcl",
    "sh",
    "bash",
    "zsh",
    "ksh",
    "csh",
    "tcsh",
    "fish",
    "command",
    "tool",
    "run",
    "bin",
    "out",
    "elf",
    // macOS programs, installers, scripts and workflows.
    "app",
    "pkg",
    "mpkg",
    "scpt",
    "scptd",
    "applescript",
    "workflow",
    "action",
    "terminal",
    "webloc",
    "inetloc",
    "fileloc",
    "dylib",
    "kext",
    "prefpane",
    "osax",
    "plugin",
    "bundle",
    // Linux and BSD programs, installers and launchers.
    "desktop",
    "appimage",
    "flatpak",
    "flatpakref",
    "snap",
    "deb",
    "rpm",
    "so",
    "ko",
    // Add-in libraries, provisioning and profile packages, task and management files, and the
    // connection files and queries that connect when opened.
    "wll",
    "ppkg",
    "provxml",
    "job",
    "ops",
    "mof",
    "inx",
    "dqy",
    "oqy",
    "rqy",
    "ica",
    "mobileconfig",
    "mobileprovision",
    "rdg",
    "term",
    "wflow",
    "flatpakrepo",
    "cdxml",
    "swf",
    // Theme packs, location files that connect to a server, plug-ins and kinds that open in a
    // program that runs what they hold.
    "deskthemepack",
    "afploc",
    "ftploc",
    "vncloc",
    "wcx",
    "its",
    "cdr",
    "smi",
    // MailCraft's own mailbox: its accounts name servers, so one sent in mail is saved, never
    // opened from the message.
    "mailcraft",
];

/// Office files that can carry macros.
pub const MACROS: &[&str] = &[
    "docm", "dotm", "xlsm", "xltm", "xlam", "xlsb", "xla", "xlm", "pptm", "potm", "ppam", "ppsm", "sldm", "accdb", "one", "onepkg", "vsdm", "vstm",
    "vssm", "doc", "dot", "xls", "xlt", "xlw", "ppt", "pot", "pps", "ppa", "pub", "wbk", "vsd", "vss", "vst", "vdx",
];

/// Whether a file of this kind (the part after the last dot, in lower case) runs, or runs what is
/// in it, when it is opened: the one list ([`BLOCKED`]) the opener refuses by and the files an
/// agent may not have written are refused by.
pub fn is_launchable(ext: &str) -> bool {
    BLOCKED.contains(&ext)
}

/// Web pages and drawings that open in the browser, where what is in them runs.
pub const SCRIPTS: &[&str] = &["htm", "html", "xhtml", "shtml", "mht", "mhtml", "svg", "svgz", "xht", "xml", "xsl"];

/// Whether a file of this kind (the part after the last dot, in lower case) is one the opener
/// refuses or asks about first ([`BLOCKED`], [`MACROS`] and [`SCRIPTS`] together): the one list
/// the files an agent may not have written are refused by.
pub fn is_risky(ext: &str) -> bool {
    BLOCKED.contains(&ext) || MACROS.contains(&ext) || SCRIPTS.contains(&ext)
}

/// Names Windows keeps for devices: a file with one of these before its first dot is no file.
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "com¹", "com²", "com³", "lpt1", "lpt2",
    "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "lpt¹", "lpt²", "lpt³", "conin$", "conout$",
];

/// The longest a file name is made, in characters (a folder's path takes the rest).
pub const MAX_NAME: usize = 120;

/// The kind of a file name: what is after its last dot, in lower case (empty when nothing).
pub fn extension(name: &str) -> String {
    let name = name.trim_end_matches(['.', ' ']);
    match name.rsplit_once('.') {
        Some((_, ext)) => ext.trim().to_lowercase(),
        None => String::new(),
    }
}

/// What may be done with an attachment of this name when the owner asks to open it. It is read
/// from the name the file would be written under ([`file_name`]), which is what the computer
/// goes by when it chooses the program.
pub fn verdict(name: &str) -> Verdict {
    let safe = file_name(name);
    let ext = extension(&safe);
    if ext.is_empty() {
        // No kind to choose a program by: the computer guesses from what is in it.
        return Verdict::Blocked;
    }
    if BLOCKED.contains(&ext.as_str()) {
        Verdict::Blocked
    } else if MACROS.contains(&ext.as_str()) {
        Verdict::Macros
    } else if SCRIPTS.contains(&ext.as_str()) {
        Verdict::Scripts
    } else {
        Verdict::Open
    }
}

/// What the owner is told when an attachment isn't opened because of its kind.
pub fn blocked_message(name: &str) -> String {
    let shown: String = file_name(name).chars().take(60).collect();
    format!(
        "MailCraft doesn't open {shown}: a program, a script or a file of no known kind sent in mail can harm the computer. If you trust it, save it with Save As."
    )
}

/// The name an attachment is written under to be opened: only the last part of a path the
/// sender put in its name; no control characters or characters a folder can't have; no leading
/// dots, dashes or spaces (hidden files, options); no trailing dots or spaces (Windows drops
/// them, and with them what the name said it was); no characters that change how it looks
/// without being seen; not a name Windows keeps for a device; at most [`MAX_NAME`] characters
/// with its kind kept. Never empty. Every file named from mail is named through here (see
/// [`save_name`] and [`export_name`]), and so is an attachment where it is shown, so that the
/// name seen is the name written.
pub fn file_name(name: &str) -> String {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or_default();
    // Explorer reads a comma as the end of an argument and a semicolon is no better: neither
    // stays in a name an opener may be handed. (Names of exported files keep them, see [`export_name`].)
    let out = clean(&leaf.replace([',', ';'], "_"));
    if out.is_empty() { "attachment".to_string() } else { out }
}

/// The name an attachment is saved under (Save As): its [`file_name`], with `.bin` after it when
/// it has no short kind of letters and digits, so that the computer doesn't guess one.
pub fn save_name(name: &str) -> String {
    let safe = file_name(name);
    match safe.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.chars().count() <= 8 && ext.chars().all(char::is_alphanumeric) => safe,
        _ => format!("{safe}.bin"),
    }
}

/// The name a file made from the mailbox is offered under: `stem` (a message's subject, a
/// folder's name, an event's title; at most 80 characters of it) made safe as [`file_name`]
/// makes a name, a slash in it being a character and not a folder, then `.ext`. `untitled` when
/// nothing of `stem` is left ([`export_name_or`] names another).
pub fn export_name(stem: &str, ext: &str) -> String {
    export_name_or(stem, ext, "untitled")
}

/// [`export_name`], named `fallback` when nothing of `stem` is left.
pub fn export_name_or(stem: &str, ext: &str, fallback: &str) -> String {
    let stem: String = stem.chars().take(80).map(|c| if matches!(c, '/' | '\\') { '_' } else { c }).collect();
    let stem = clean(&stem);
    let stem = if stem.is_empty() { fallback } else { stem.as_str() };
    let ext: String = ext.chars().filter(|c| c.is_alphanumeric()).take(16).collect();
    if ext.is_empty() { clean(stem) } else { clean(&format!("{stem}.{ext}")) }
}

/// [`file_name`] of a name with no folders in it, which may come out empty.
fn clean(leaf: &str) -> String {
    let cleaned: String = leaf.chars().map(|c| if c.is_control() || "\\/:*?\"<>|".contains(c) || is_invisible(c) { '_' } else { c }).collect();
    let trimmed = cleaned.trim_start_matches(['.', '-', ' ', '_', '~']).trim_end_matches(['.', ' ']);
    let mut out = shorten(trimmed);
    let stem_lower = out.split('.').next().unwrap_or_default().trim_end().to_lowercase();
    if RESERVED.contains(&stem_lower.as_str()) {
        out = format!("_{out}");
    }
    out
}

/// Characters that change how a name looks without being seen (right-to-left overrides that turn
/// `gpj.exe` into what reads as `exe.jpg`, isolates, zero-width marks and joiners, the byte-order
/// mark, a soft hyphen, the Arabic letter mark, the Mongolian vowel separator, tags).
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{e0000}'..='\u{e007f}'
    )
}

/// At most [`MAX_NAME`] characters, the kind kept when there is a short one.
fn shorten(name: &str) -> String {
    if name.chars().count() <= MAX_NAME {
        return name.to_string();
    }
    let ext = match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() && ext.chars().count() <= 16 => format!(".{ext}"),
        _ => String::new(),
    };
    let keep = MAX_NAME.saturating_sub(ext.chars().count());
    let stem: String = name.chars().take(keep).collect();
    format!("{}{ext}", stem.trim_end_matches(['.', ' ']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_names_are_made_safe() {
        let cases = [
            ("report.pdf", "report.pdf"),
            ("../../etc/passwd", "passwd"),
            ("..\\..\\Windows\\System32\\evil.pdf", "evil.pdf"),
            ("C:\\Users\\x\\a.pdf", "a.pdf"),
            (".bashrc", "bashrc"),
            ("-rf.txt", "rf.txt"),
            ("--help", "help"),
            ("...", "attachment"),
            ("", "attachment"),
            ("   ", "attachment"),
            ("a\0b\r\n.txt", "a_b__.txt"),
            ("what?.txt", "what_.txt"),
            ("CON.txt", "_CON.txt"),
            ("nul", "_nul"),
            ("Lpt1.tar.gz", "_Lpt1.tar.gz"),
            ("console.txt", "console.txt"),
            ("invoice.exe.", "invoice.exe"),
            ("invoice.exe   ", "invoice.exe"),
            ("photo\u{202e}gpj.exe", "photo_gpj.exe"),
        ];
        for (name, want) in cases {
            assert_eq!(file_name(name), want, "{name:?}");
        }
        let long = format!("{}.pdf", "a".repeat(500));
        let short = file_name(&long);
        assert!(short.chars().count() <= MAX_NAME && short.ends_with(".pdf"), "{short}");
        let wide = "é".repeat(400);
        assert!(file_name(&wide).chars().count() <= MAX_NAME);
        for name in ["", "/", "\\", "././.", "a/b/c/", "\u{feff}", "COM1", "con.", "\u{1F600}.pdf"] {
            let safe = file_name(name);
            assert!(!safe.is_empty() && !safe.contains(['/', '\\']) && !safe.starts_with(['.', '-']), "{name:?} -> {safe:?}");
        }
    }

    #[test]
    fn saved_and_exported_names_are_made_safe() {
        let cases = [
            ("invoice\u{202e}fdp.exe", "invoice_fdp.exe"),
            ("in\u{2067}voice\u{2069}.pdf", "in_voice_.pdf"),
            ("re\u{200b}port.pdf", "re_port.pdf"),
            ("\u{feff}\u{200d}", "attachment.bin"),
            ("CON.txt", "_CON.txt"),
            ("../../x", "x.bin"),
            ("..\\..\\x.pdf", "x.pdf"),
            ("a:b", "a_b.bin"),
            ("a:b.txt", "a_b.txt"),
            ("", "attachment.bin"),
            ("notes", "notes.bin"),
            ("report.pdf", "report.pdf"),
            ("archive.tar.gz", "archive.tar.gz"),
            ("x.p\u{0434}f", "x.p\u{0434}f"),
            ("x.a b", "x.a b.bin"),
        ];
        for (name, want) in cases {
            assert_eq!(save_name(name), want, "{name:?}");
        }
        let cases = [
            ("Q1/Q2 report", "eml", "Q1_Q2 report.eml"),
            ("invoice\u{202e}fdp", "eml", "invoice_fdp.eml"),
            ("CON", "mbox", "_CON.mbox"),
            ("../../x", "ics", "x.ics"),
            ("a:b", "vcf", "a_b.vcf"),
            ("", "eml", "untitled.eml"),
            ("   ", "eml", "untitled.eml"),
            ("\u{200b}\u{202e}", "eml", "untitled.eml"),
            ("Hello.", "eml", "Hello.eml"),
            ("x", "e/m:l", "x.eml"),
        ];
        for (stem, ext, want) in cases {
            assert_eq!(export_name(stem, ext), want, "{stem:?}.{ext}");
        }
        assert_eq!(export_name(&"a".repeat(500), "eml").chars().count(), 84);
    }

    #[test]
    fn commas_and_semicolons_never_reach_the_opener() {
        // AU2-V3: Explorer takes a comma as the end of its argument; the verdict and the file
        // written are of the same name.
        for name in ["a,b.pdf", "x /select,C:\\Windows\\calc.exe.pdf", ",,evil.exe", "a;b.txt"] {
            let safe = file_name(name);
            assert!(!safe.contains([',', ';']), "{safe}");
            assert_eq!(verdict(name), verdict(&safe), "{name}");
        }
        assert_eq!(file_name("a,b;c.pdf"), "a_b_c.pdf");
        assert_eq!(save_name("a,b.pdf"), "a_b.pdf");
    }

    #[test]
    fn programs_are_never_opened() {
        for name in [
            "setup.exe",
            "run.BAT",
            "x.cmd",
            "a.com",
            "s.scr",
            "i.msi",
            "p.ps1",
            "v.vbs",
            "j.js",
            "k.jar",
            "s.sh",
            "Thing.app",
            "short.lnk",
            "disk.iso",
            "invoice.pdf.exe",
            "invoice.exe.",
            "photo\u{202e}gpj.exe",
            "noextension",
            "../x.exe",
            "office.rdp",
            "rates.IQY",
            "sheet.slk",
            "addin.vsto",
            "macro.mam",
            "outlook.prf",
            "help.hpj",
            "group.grp",
            "backup.mailcraft",
            "Backup.MAILCRAFT",
            "look.deskthemepack",
            "x.AFPLOC",
            "x.ftploc",
            "x.vncloc",
            "pack.wcx",
            "x.its",
            "drawing.cdr",
            "x.smi",
            "a,b.exe",
            "a;b.exe",
        ] {
            assert_eq!(verdict(name), Verdict::Blocked, "{name}");
        }
    }

    #[test]
    fn macros_and_pages_ask_and_the_rest_opens() {
        for name in ["budget.xlsm", "Letter.DOCM", "deck.pptm"] {
            assert_eq!(verdict(name), Verdict::Macros, "{name}");
        }
        for name in ["page.html", "x.htm", "logo.svg"] {
            assert_eq!(verdict(name), Verdict::Scripts, "{name}");
        }
        for name in ["report.pdf", "photo.JPG", "notes.txt", "sheet.xlsx", "letter.docx", "archive.zip", "invite.ics"] {
            assert_eq!(verdict(name), Verdict::Open, "{name}");
        }
        assert!(Verdict::Macros.asks() && Verdict::Scripts.asks() && !Verdict::Open.asks() && !Verdict::Blocked.asks());
    }
}
