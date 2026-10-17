//! Help › Copy Diagnostics (`ui.copyDiagnostics`, R7.2): what an owner pastes into a problem
//! report. The version, the platform and the end of this run's log and the run before (a crash
//! survives in that one), with every message part of a log line passed through [`redact`]: no
//! address, name, path, host, quoted text, link or long id reaches the copy. Nothing is sent: the
//! text goes to the clipboard and the owner reads it before pasting it anywhere.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The end of each log file that is read (a log is capped at 16 MB; the copy is for an issue).
const TAIL_BYTES: u64 = 64 * 1024;
/// Lines kept of this run's log and of the one before.
const LINES_NOW: usize = 150;
const LINES_BEFORE: usize = 80;
/// A redacted line longer than this is cut (a backtrace or a server's answer on one line).
const MAX_LINE: usize = 400;

/// The whole copy: header, then the redacted tails of `mailcraft.log` and `mailcraft.1.log` in
/// `log_dir` (none without one: runs without preferences).
pub fn report(version: &str, build: &str, log_dir: Option<&Path>) -> String {
    let mut out = String::from("### MailCraft diagnostics\n\n");
    out.push_str(&format!("- Version: {version} ({build})\n"));
    out.push_str(&format!("- Platform: {}\n", platform()));
    out.push_str("- Addresses, names, paths, hosts, links, ids and quoted text are taken out of the log.\n");
    let Some(dir) = log_dir else {
        out.push_str("\nNo log is kept for this run.\n");
        return out;
    };
    for (file, title, lines) in [("mailcraft.log", "This run", LINES_NOW), ("mailcraft.1.log", "The run before", LINES_BEFORE)] {
        out.push_str(&format!("\n{title}:\n\n```text\n"));
        match tail(&dir.join(file), lines) {
            Some(text) if !text.is_empty() => {
                for line in text.lines() {
                    out.push_str(&cut(&redact_line(line)));
                    out.push('\n');
                }
            }
            _ => out.push_str("(none)\n"),
        }
        out.push_str("```\n");
    }
    out
}

/// The operating system, the processor and, on Linux and the BSDs, the desktop and the display
/// server: no version of anything that would single a machine out.
fn platform() -> String {
    let mut parts = vec![format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)];
    if cfg!(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd")) {
        for var in ["XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE"] {
            if let Ok(value) = std::env::var(var) {
                // Short, plain words only; anything else is not a desktop's name.
                let value: String = value.chars().filter(|c| c.is_ascii_alphanumeric() || *c == ':' || *c == '-').take(40).collect();
                if !value.is_empty() {
                    parts.push(value);
                }
            }
        }
    }
    if std::env::var_os("FLATPAK_ID").is_some() {
        parts.push("Flatpak".into());
    }
    parts.join(", ")
}

/// The last `lines` lines of the file at `path`, read from its last [`TAIL_BYTES`]; `None` when it
/// can't be read.
fn tail(path: &Path, lines: usize) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let from = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut all: Vec<&str> = text.lines().collect();
    if from > 0 && !all.is_empty() {
        all.remove(0); // begun mid-line
    }
    let skip = all.len().saturating_sub(lines);
    Some(all.into_iter().skip(skip).collect::<Vec<_>>().join("\n"))
}

fn cut(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE) {
        Some((at, _)) => format!("{}…", line.get(..at).unwrap_or_default()),
        None => line.to_owned(),
    }
}

/// One log line (`<time> <LEVEL> [<thread>] <target>: <message>`) with its message redacted; a
/// line not of that shape is redacted whole.
pub fn redact_line(line: &str) -> String {
    let head = line.find("] ").and_then(|close| {
        let after = close + 2;
        line.get(after..)?.find(": ").map(|colon| after + colon + 2)
    });
    match head.and_then(|at| Some((line.get(..at)?, line.get(at..)?))) {
        Some((head, message)) => format!("{head}{}", redact(message)),
        None => redact(line),
    }
}

/// `text` with what could be the owner's taken out: quoted text, then every word that is an
/// address, a link, a path, an IP address, a host or file name, or a long id.
///
/// ponytail: pattern rules, not a list of allowed words. A name the program logs bare (a folder
/// called `Accounts`) would pass; the program's own lines name folders and calendars by id, and
/// server errors quote names, which are caught. Move to an allow-list if a report shows a leak.
pub fn redact(text: &str) -> String {
    unquote(text, true).split_inclusive(char::is_whitespace).map(|w| word(w, true)).collect()
}

/// What the log file itself is told of a message (every line, as it is written): `text` with
/// quoted text (`"…"` and `'…'`), addresses, links and long ids with a digit in them (a token, a
/// code) taken out. The program's own words, hosts, paths, numbers and `` `…` `` stay: the file is
/// for finding a fault on this computer, and [`redact`] takes out the rest when it is copied.
pub fn scrub(text: &str) -> String {
    unquote(text, false).split_inclusive(char::is_whitespace).map(|w| word(w, false)).collect()
}

/// Quoted spans (`"…"`, `` `…` ``, and `'…'` where the quote doesn't follow a letter, so
/// "couldn't" stays) become `"…"`; an unpaired quote takes the rest of the text with it.
/// `` `…` `` only with `backticks`.
fn unquote(text: &str, backticks: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut before: Option<char> = None;
    while let Some(c) = chars.next() {
        let opens = c == '"' || (backticks && c == '`') || (c == '\'' && !before.is_some_and(char::is_alphanumeric));
        if !opens {
            out.push(c);
            before = Some(c);
            continue;
        }
        // Up to the matching quote (for `'`, one not followed by a letter).
        let mut closed = false;
        let mut escaped = false;
        while let Some(inner) = chars.next() {
            if escaped {
                escaped = false;
            } else if inner == '\\' {
                escaped = true;
            } else if inner == c && (c != '\'' || !chars.peek().is_some_and(|n| n.is_alphanumeric())) {
                closed = true;
                break;
            }
        }
        out.push_str(if closed { "\"…\"" } else { "…" });
        before = Some('"');
    }
    out
}

/// One word (with the whitespace after it), kept or replaced by what it was: anything [`kind`]
/// names with `all`, else only an address, a link or a long id with a digit in it.
fn word(raw: &str, all: bool) -> String {
    let body = raw.trim_end();
    let space = raw.get(body.len()..).unwrap_or_default();
    let edge = |c: char| matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';' | ':' | '.' | '!' | '?');
    let core = body.trim_start_matches(edge);
    let lead = body.get(..body.len() - core.len()).unwrap_or_default();
    let core = core.trim_end_matches(edge);
    let trail = body.get(lead.len() + core.len()..).unwrap_or_default();
    // A host with its port (`example.net:993`): the port stays.
    let (host, port) = match core.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && !host.contains(':') && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            (host, core.get(host.len()..).unwrap_or_default())
        }
        _ => (core, ""),
    };
    let taken = |what: &str| all || matches!(what, "address" | "link") || (what == "id" && host.chars().any(|c| c.is_ascii_digit()));
    // A server's code for a refusal, as the program logs it (`code=ErrorInvalidRequest`): kept.
    if is_logged_code(core) {
        return raw.to_owned();
    }
    match kind(host) {
        Some(what) if taken(what) => format!("{lead}<{what}>{port}{trail}{space}"),
        _ => raw.to_owned(),
    }
}

/// Whether `core` is `code=<name>`, the way the program logs the short name a server gives a
/// refusal (Microsoft Graph's `ErrorInvalidRequest`, `BadRequest`, `ErrorMimeContentInvalidBase64String`):
/// words run together, `_` and `.` allowed, a few digits at most, 64 at most. An id or a token,
/// letters of either case and digits at random, isn't one.
fn is_logged_code(core: &str) -> bool {
    let Some(name) = core.strip_prefix("code=") else { return false };
    let digits = name.bytes().filter(u8::is_ascii_digit).count();
    // Words run together: every run of small letters is two or more long (`Error`, `Mime`), as
    // the case of an id's letters, which changes at random, seldom has it.
    let words = name.split(|c: char| !c.is_ascii_lowercase()).filter(|run| !run.is_empty()).collect::<Vec<_>>();
    (1..=64).contains(&name.len())
        && name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.'))
        && digits <= 4
        && !words.is_empty()
        && words.iter().all(|run| run.len() >= 2)
}

/// What `core` is, when it is something to take out.
fn kind(core: &str) -> Option<&'static str> {
    if core.is_empty() {
        return None;
    }
    if core.contains('@') {
        return Some("address");
    }
    if core.contains("://") {
        return Some("link");
    }
    let drive = matches!(core.as_bytes(), [d, b':', b'\\' | b'/', ..] if d.is_ascii_alphabetic());
    let source = !core.starts_with('/') && core.contains(".rs"); // a place in MailCraft's own code
    if drive
        || core.starts_with(['/', '~', '\\'])
        || core.contains('\\')
        || (core.contains('/') && !source && !core.eq_ignore_ascii_case("send/receive"))
    {
        return Some("path");
    }
    let labels: Vec<&str> = core.split('.').collect();
    if labels.len() == 4 && labels.iter().all(|l| !l.is_empty() && l.len() <= 3 && l.bytes().all(|b| b.is_ascii_digit())) {
        return (core != "127.0.0.1").then_some("ip");
    }
    let hex = core.chars().all(|c| c.is_ascii_hexdigit() || c == ':');
    if hex && core.matches(':').count() >= 2 && (core.contains("::") || core.chars().any(|c| c.is_ascii_alphabetic())) && core != "::1" {
        return Some("ip");
    }
    let plain = |l: &&str| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_');
    let last = labels.last().copied().unwrap_or_default();
    if labels.len() >= 2 && labels.iter().all(plain) && last.chars().count() >= 2 && last.chars().all(char::is_alphabetic) {
        return Some("name");
    }
    if core.chars().count() >= 20 && core.chars().all(|c| c.is_ascii_alphanumeric() || "-_+/=.".contains(c)) {
        return Some("id");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIVATE: &[&str] = &[
        "someone@example.org",
        "Branding",
        "Advertising",
        "outlook.office365.com",
        "/home/owner",
        "C:\\Users\\owner",
        "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9",
        "hunter2secret",
        "Quarterly figures",
        "192.0.2.7",
        "2001:db8::1",
        "https://login.example.net/?code=abc",
        "Annual Accounts",
        "mailcraft.log",
    ];

    fn log() -> String {
        [
            "2026-10-15T10:00:00.000Z INFO  [main] mailcraft: MailCraft 0.3.0 (abcdef12), log file /home/owner/.config/mailcraft/logs/mailcraft.log",
            "2026-10-15T10:00:01.000Z WARN  [send/receive] mailcraft_net::sync: send/receive, account 3: NO [TRYCREATE] \"Branding/Advertising\" doesn't exist",
            "2026-10-15T10:00:02.000Z WARN  [send/receive] mailcraft_net::smtp: 550 5.1.1 <someone@example.org>: recipient rejected",
            "2026-10-15T10:00:03.000Z INFO  [send/receive] mailcraft_net::transport: connecting to outlook.office365.com:993 (Imap, tls) from 192.0.2.7 and 2001:db8::1",
            "2026-10-15T10:00:04.000Z ERROR [main] mailcraft_engine::store: saving C:\\Users\\owner\\mail.mailcraft failed: denied",
            "2026-10-15T10:00:05.000Z WARN  [main] mailcraft: sign-in: token eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9 refused, password 'hunter2secret' wrong",
            "2026-10-15T10:00:06.000Z WARN  [main] mailcraft_mime: subject `Quarterly figures` couldn't be decoded",
            "2026-10-15T10:00:07.000Z WARN  [main] mailcraft: redirect to https://login.example.net/?code=abc",
            "2026-10-15T10:00:08.000Z WARN  [main] mailcraft: folder \"Annual Accounts",
        ]
        .join("\n")
    }

    #[test]
    fn nothing_private_reaches_the_copy() {
        let dir = std::env::temp_dir().join(format!("mailcraft-diagnostics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mailcraft.log"), log()).unwrap();
        std::fs::write(dir.join("mailcraft.1.log"), log()).unwrap();
        let copy = report("0.3.0", "abcdef12", Some(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
        for private in PRIVATE {
            assert!(!copy.contains(private), "{private} reached the copy:\n{copy}");
        }
        // What a report needs stays.
        for kept in [
            "0.3.0",
            "WARN  [send/receive] mailcraft_net::sync:",
            "doesn't exist",
            "550 5.1.1",
            "recipient rejected",
            ":993",
            "<address>",
            "<path>",
            "<name>",
            "<ip>",
            "<link>",
            "<id>",
        ] {
            assert!(copy.contains(kept), "{kept} is missing:\n{copy}");
        }
    }

    #[test]
    fn mailcrafts_own_words_and_places_stay() {
        assert_eq!(redact("couldn't start Send/Receive: no answer"), "couldn't start Send/Receive: no answer");
        assert_eq!(redact("panicked at crates/engine/src/cmd.rs:12:5"), "panicked at crates/engine/src/cmd.rs:12:5");
        assert_eq!(redact("waiting on port 7991 for 127.0.0.1"), "waiting on port 7991 for 127.0.0.1");
        assert_eq!(redact("version 0.3.0, e.g. this"), "version 0.3.0, e.g. this");
    }

    #[test]
    fn the_log_file_keeps_hosts_paths_and_code_names_but_no_address() {
        assert_eq!(scrub("connecting to outlook.office365.com:993 (Imap, tls)"), "connecting to outlook.office365.com:993 (Imap, tls)");
        assert_eq!(scrub("log file /home/owner/.config/mailcraft/logs/mailcraft.log"), "log file /home/owner/.config/mailcraft/logs/mailcraft.log");
        assert_eq!(scrub("command `mail.flag` panicked; InvalidAuthenticationToken"), "command `mail.flag` panicked; InvalidAuthenticationToken");
        assert_eq!(scrub("550 <bob@example.org>: no 'Ada Lovelace'"), "550 <<address>>: no \"…\"");
        for text in ["", "\"", "'", "`", "é@", "a\u{0}b", "[x] y: \"é", "::::"] {
            let _ = scrub(text);
        }
    }

    #[test]
    fn graphs_codes_stay_and_ids_beside_them_go() {
        let line = "graph changes: a change of a message was refused (status 400, code=ErrorInvalidPropertyValue)";
        assert_eq!(redact(line), line);
        assert_eq!(scrub(line), line);
        for code in ["ErrorInvalidRequest", "BadRequest", "ErrorMimeContentInvalidBase64String", "Request_BadRequest", "InvalidRequest"] {
            let text = format!("graph mail: Microsoft Graph refused a request (status 400, code={code})");
            assert_eq!(redact(&text), text);
        }
        // An id is no code, with `code=` before it or not.
        let id = "AAMkAGVmMDEzMTM4LTZmYWUtNDdkNC1hMDZiLTU1OGY5OTZhYmY4OABGAAAAAAAiQ8W967B7TKBjgx9rVEURBwAiIsqMbYjsT5e-T7KzowPTAAAAAAEMAAA=";
        for text in [format!("refused {id}"), format!("refused code={id}"), format!("refused code={}", &id[..40])] {
            assert!(!redact(&text).contains("AAMkAG"), "{}", redact(&text));
            assert!(!scrub(&text).contains("AAMkAG"), "{}", scrub(&text));
        }
        assert_eq!(redact("code=AAMk1234567890abcdefghijk"), "<id>");
    }

    #[test]
    fn hostile_text_never_panics() {
        for text in ["", "\"", "'", "`", "\\", "é@", "a\u{0}b", "] : ", "[x] y: \"é", "ï:\\", "::::", "日本語.テスト", "C:", "'''"] {
            let _ = redact_line(text);
            let _ = redact(text);
        }
    }

    #[test]
    fn without_a_log_the_copy_says_so() {
        assert!(report("0.3.0", "dev", None).contains("No log is kept"));
        let missing = std::env::temp_dir().join("mailcraft-diagnostics-none");
        assert!(report("0.3.0", "dev", Some(&missing)).contains("(none)"));
    }
}
