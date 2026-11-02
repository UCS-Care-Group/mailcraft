// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading the contributor credits (`docs/contributors.md`):
//! `contributors/contributors.json` (commit stats per GitHub username, generated),
//! `contributors/people.toml` (the names people chose for themselves) and
//! `contributors/testers.toml` (the testers who said yes to being credited), turned into the Rust
//! tables the About window shows.
//!
//! This file is shared: `build.rs` includes it to bake the tables into the binary, and the tests
//! include it to check the reading. Nothing here runs in the program, and nothing is read from
//! disk or the network at run time.
//!
//! The files arrive in pull requests, so they are read as hostile: a malformed file is an error
//! (the build then shows no contributors and says why), a username that GitHub would not give is
//! left out, names are trimmed, stripped of control characters and clamped, and the list is capped.

use serde_json::Value;

/// At most this many contributors are built in.
pub const MAX_CONTRIBUTORS: usize = 10_000;
/// At most this many testers are built in.
pub const MAX_TESTERS: usize = 10_000;
/// A display or real name is cut to this many characters.
pub const MAX_NAME_CHARS: usize = 100;

/// One contributor as read from the files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub login: String,
    pub display_name: Option<String>,
    pub real_name: Option<String>,
    pub prs: u64,
    pub commits: u64,
    pub lines_added: u64,
    pub lines_deleted: u64,
    pub binary_added: u64,
    pub binary_deleted: u64,
    /// ISO 8601 UTC; empty for someone credited only through a merged PR.
    pub first_commit: String,
    pub last_commit: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Credits {
    pub total_commits: u64,
    pub contributors: Vec<Row>,
}

/// A GitHub username: 1 to 39 ASCII letters, digits and hyphens, not starting with a hyphen.
pub fn valid_login(s: &str) -> bool {
    (1..=39).contains(&s.len()) && !s.starts_with('-') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A name someone chose: trimmed, without control characters, at most [`MAX_NAME_CHARS`]
/// characters; `None` when nothing is left.
fn clean_name(s: Option<&str>) -> Option<String> {
    let s: String = s?.chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect();
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// An ISO 8601 date as the script writes it (`2026-10-15T17:34:39Z`); anything else is no date.
fn clean_date(s: Option<&str>) -> String {
    match s {
        Some(d) if d.len() <= 32 && d.bytes().all(|b| b.is_ascii_digit() || b"-:TZ.+".contains(&b)) => d.to_string(),
        _ => String::new(),
    }
}

/// A day as `YYYY-MM-DD`, written as a TOML date (`2026-10-19`) or as text; anything else,
/// a time of day included, is no date.
fn clean_day(v: Option<&toml::Value>) -> String {
    let s = match v {
        Some(toml::Value::String(s)) => s.clone(),
        Some(toml::Value::Datetime(d)) => d.to_string(),
        _ => return String::new(),
    };
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
    let shaped = b.len() == 10 && b.get(4) == Some(&b'-') && b.get(7) == Some(&b'-') && digits(0..4) && digits(5..7) && digits(8..10);
    let num = |r: std::ops::Range<usize>| s.get(r).and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
    let (year, month, day) = (num(0..4), num(5..7), num(8..10));
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    if shaped && (1..=days).contains(&day) { s } else { String::new() }
}

fn count(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}

/// The credits in `json` (contributors.json) with the names in `people` (people.toml). A name is
/// used only when the JSON says the script verified that person's own entry (`people_entry`).
pub fn parse(json: &str, people: &str) -> Result<Credits, String> {
    let json: Value = serde_json::from_str(json).map_err(|e| format!("contributors.json is not JSON: {e}"))?;
    match json.get("schema").and_then(Value::as_u64) {
        Some(1) => {}
        other => return Err(format!("contributors.json has schema {other:?}; this build reads schema 1")),
    }
    let list = json.get("contributors").and_then(Value::as_array).ok_or("contributors.json has no \"contributors\" list")?;
    let people: toml::Table = people.parse().map_err(|e| format!("people.toml is not TOML: {e}"))?;
    let entries = match people.get("people") {
        None => None,
        Some(toml::Value::Table(t)) => Some(t),
        Some(_) => return Err("people.toml: \"people\" is not a table".into()),
    };
    let mut out = Credits { total_commits: count(&json, "commits"), contributors: Vec::new() };
    for c in list {
        if out.contributors.len() >= MAX_CONTRIBUTORS {
            break;
        }
        let Some(login) = c.get("login").and_then(Value::as_str).filter(|l| valid_login(l)) else { continue };
        if out.contributors.iter().any(|r| r.login.eq_ignore_ascii_case(login)) {
            continue;
        }
        let verified = c.get("people_entry").and_then(Value::as_bool) == Some(true);
        let entry = entries.and_then(|p| p.get(login)).and_then(toml::Value::as_table).filter(|_| verified);
        let field = |k: &str| entry.and_then(|e| e.get(k)).and_then(toml::Value::as_str);
        let synced = entry.and_then(|e| e.get("sync_github_name")).and_then(toml::Value::as_bool) == Some(true);
        let display =
            clean_name(field("display_name")).or_else(|| if synced { clean_name(c.get("github_name").and_then(Value::as_str)) } else { None });
        out.contributors.push(Row {
            login: login.to_string(),
            display_name: display,
            real_name: clean_name(field("real_name")),
            prs: count(c, "prs"),
            commits: count(c, "commits"),
            lines_added: count(c, "lines_added"),
            lines_deleted: count(c, "lines_deleted"),
            binary_added: count(c, "binary_added"),
            binary_deleted: count(c, "binary_deleted"),
            first_commit: clean_date(c.get("first_commit").and_then(Value::as_str)),
            last_commit: clean_date(c.get("last_commit").and_then(Value::as_str)),
        });
    }
    Ok(out)
}

/// One tester as read from `testers.toml`. The issue where they said yes (`consent`) is checked
/// but not kept: it is for the maintainers' audit, never shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TesterRow {
    pub login: String,
    pub display_name: Option<String>,
    /// The day they became a tester, `YYYY-MM-DD`; empty when not given or not a date.
    pub since: String,
}

/// The testers in `testers.toml` (`[testers.<username>]` tables), in the order About shows them:
/// the earliest `since` first (no date last), then by username, A–Z whatever the case. An entry
/// whose username GitHub would not give, a second one for the same username, and one without the
/// number of the issue where its tester said yes (`consent`) are left out.
pub fn parse_testers(src: &str) -> Result<Vec<TesterRow>, String> {
    let file: toml::Table = src.parse().map_err(|e| format!("testers.toml is not TOML: {e}"))?;
    let entries = match file.get("testers") {
        None => return Ok(Vec::new()),
        Some(toml::Value::Table(t)) => t,
        Some(_) => return Err("testers.toml: \"testers\" is not a table".into()),
    };
    let mut out: Vec<TesterRow> = Vec::new();
    for (login, entry) in entries {
        if out.len() >= MAX_TESTERS {
            break;
        }
        let Some(entry) = entry.as_table() else { continue };
        if !valid_login(login) || out.iter().any(|r| r.login.eq_ignore_ascii_case(login)) {
            continue;
        }
        if !entry.get("consent").and_then(toml::Value::as_integer).is_some_and(|n| n > 0) {
            continue;
        }
        out.push(TesterRow {
            login: login.clone(),
            display_name: clean_name(entry.get("display_name").and_then(toml::Value::as_str)),
            since: clean_day(entry.get("since")),
        });
    }
    out.sort_by(|a, b| {
        (a.since.is_empty(), &a.since)
            .cmp(&(b.since.is_empty(), &b.since))
            .then_with(|| a.login.to_lowercase().cmp(&b.login.to_lowercase()))
            .then_with(|| a.login.cmp(&b.login))
    });
    Ok(out)
}

/// The testers as the Rust source `credits.rs` includes: `TESTERS`.
pub fn render_testers(list: &[TesterRow]) -> String {
    let mut o = String::from("pub static TESTERS: &[Tester] = &[\n");
    for r in list {
        o.push_str(&format!("    Tester {{ login: {:?}, display_name: {}, since: {:?} }},\n", r.login, opt(&r.display_name), r.since));
    }
    o.push_str("];\n");
    o
}

fn opt(v: &Option<String>) -> String {
    match v {
        Some(s) => format!("Some({s:?})"),
        None => "None".into(),
    }
}

/// The credits as the Rust source `credits.rs` includes: `TOTAL_COMMITS` and `CONTRIBUTORS`.
pub fn render(c: &Credits) -> String {
    let mut o = format!("pub static TOTAL_COMMITS: u64 = {};\npub static CONTRIBUTORS: &[Contributor] = &[\n", c.total_commits);
    for r in &c.contributors {
        o.push_str(&format!(
            "    Contributor {{ login: {:?}, display_name: {}, real_name: {}, prs: {}, commits: {}, lines_added: {}, lines_deleted: {}, binary_added: {}, binary_deleted: {}, first_commit: {:?}, last_commit: {:?} }},\n",
            r.login,
            opt(&r.display_name),
            opt(&r.real_name),
            r.prs,
            r.commits,
            r.lines_added,
            r.lines_deleted,
            r.binary_added,
            r.binary_deleted,
            r.first_commit,
            r.last_commit,
        ));
    }
    o.push_str("];\n");
    o
}
