// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `mailcraft-cli`: MailCraft from the command line.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fmt;
use std::io::Write;
use std::process::ExitCode;

use mailcraft_engine::Session;
use mailcraft_model::Mailbox;
use serde_json::{Value, json};

const USAGE: &str = "\
mailcraft-cli — MailCraft from the command line

USAGE:
  mailcraft-cli info <mailbox>                  accounts and counts (JSON)
  mailcraft-cli folders <mailbox>               the folder tree with counts (JSON)
  mailcraft-cli list <mailbox> [--folder PATH] [--query Q] [--limit N]
                                                messages, newest first (JSON)
  mailcraft-cli show <mailbox> <id>             one message in full (JSON)
  mailcraft-cli import <mailbox> <file>... [--folder PATH]
                                                bring .eml, .mbox, .ics and .vcf files into a
                                                mailbox (made if it doesn't exist) and save it
  mailcraft-cli export <mailbox> <out> [--folder PATH] [--id N]
                                                by extension: .mbox (a folder), .eml (message --id),
                                                .ics (the calendar), .vcf (the contacts),
                                                .mailcraft (everything, in one file)
  mailcraft-cli convert <in> <out>              between .mailcraft, .eml, .mbox, .ics and .vcf
  mailcraft-cli run [--file F | --sample] --cmd 'id={json}' [--cmd ...] [--save OUT] [--print]
                                                run commands headlessly, then save
  mailcraft-cli sync <mailbox> [--account EMAIL]
                                                Send/Receive with the account's mail server; the
                                                password is read from MAILCRAFT_PASSWORD. An account
                                                that signs in on its provider's page prints the
                                                page to open (application id: MAILCRAFT_MS_CLIENT_ID),
                                                or takes a token from MAILCRAFT_ACCESS_TOKEN
  mailcraft-cli detect <address> [--offline]    the mail servers and sign-in an address usually
                                                takes; for a domain it doesn't know it asks the
                                                DNS who takes the domain's mail (only the domain
                                                is sent)
  mailcraft-cli commands [--json]               list every command
  mailcraft-cli parity [--markdown]             feature-catalog parity
  mailcraft-cli mcp [--connect HOST:PORT | --file F | --sample] [--files DIR] [--allow CLASS,...]
                                                MCP server on stdio (headless, or bridged to the app).
                                                Agents get only what the owner allows: files only
                                                inside --files DIR; sending, deleting for good,
                                                accounts, the mailbox file and options are asked
                                                about in the app (bridged) or refused (headless),
                                                unless allowed with --allow send, delete, accounts,
                                                mailbox, settings or input (clicking and typing).
                                                Leaving or throwing away what the owner is writing
                                                or changing is asked about in the app every time
                                                (with input, the agent clicks as the owner does)
  mailcraft-cli --version
";

/// A session on the mailbox file at `path`: one self-contained file, or a mailbox as the app
/// keeps it, with its messages' content in the folder `<file>.blobs` beside it.
fn open(path: &str) -> Result<Session, String> {
    let opened = mailcraft_engine::store::open(std::path::Path::new(path))?;
    // Whatever of it couldn't be read: damaged saves left out, missing content.
    if let Some(notice) = opened.notice() {
        eprintln!("warning: {path}: {notice}");
    }
    let mut s = Session::new(opened.mailbox);
    s.path = Some(path.into());
    Ok(s)
}

/// Like [`open`], but a file that doesn't exist yet starts an empty mailbox there.
fn open_or_new(path: &str) -> Result<Session, String> {
    if std::path::Path::new(path).exists() {
        return open(path);
    }
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    s.path = Some(path.into());
    Ok(s)
}

fn sample() -> Session {
    let now = mailcraft_time::now();
    Session::new(mailcraft_engine::sample::mailbox(now, mailcraft_time::local_offset_minutes(now)))
}

/// Every `--allow` value, split at commas: what the owner lets an agent do without asking.
fn allow_classes(args: &[String]) -> Result<Vec<mailcraft_mcp::Class>, String> {
    let mut out = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a != "--allow" {
            continue;
        }
        for name in args.get(i + 1).map(String::as_str).unwrap_or("").split(',').filter(|n| !n.trim().is_empty()) {
            let names: Vec<&str> = mailcraft_mcp::Class::ALL.iter().map(|c| c.as_str()).collect();
            let c = mailcraft_mcp::Class::parse(name).ok_or_else(|| format!("mcp: --allow {name}: not one of {}", names.join(", ")))?;
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
    Ok(out)
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

#[derive(Clone, Copy)]
struct OptionSpec {
    name: &'static str,
    takes_value: bool,
}

const fn flag(name: &'static str) -> OptionSpec {
    OptionSpec { name, takes_value: false }
}

const fn value(name: &'static str) -> OptionSpec {
    OptionSpec { name, takes_value: true }
}

/// Check the options and return the positional arguments (everything that isn't an option or an
/// option's value).
fn positionals(command: &str, args: &[String], specs: &[OptionSpec]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if let Some(name) = arg.strip_prefix("--") {
            let Some(spec) = specs.iter().find(|spec| spec.name == arg) else {
                return Err(format!("{command}: unknown option --{name}"));
            };
            if spec.takes_value {
                if args.get(i + 1).is_none() {
                    return Err(format!("{command}: --{name} needs a value"));
                }
                i += 1;
            }
        } else {
            out.push(arg.clone());
        }
        i += 1;
    }
    Ok(out)
}

/// Why a CLI run stopped early.
#[derive(Debug, PartialEq)]
enum Failure {
    /// A real error, reported on stderr with a failing exit status.
    Error(String),
    /// Whoever read stdout closed it (`mailcraft-cli commands --json | head`): there is nobody left
    /// to write to, so stop quietly and exit 0 like other command-line tools.
    StdoutClosed,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Error(e) => f.write_str(e),
            Failure::StdoutClosed => f.write_str("stdout closed"),
        }
    }
}

impl From<String> for Failure {
    fn from(e: String) -> Self {
        Failure::Error(e)
    }
}

impl From<&str> for Failure {
    fn from(e: &str) -> Self {
        Failure::Error(e.into())
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::BrokenPipe { Failure::StdoutClosed } else { Failure::Error(e.to_string()) }
    }
}

impl From<mailcraft_engine::CmdError> for Failure {
    fn from(e: mailcraft_engine::CmdError) -> Self {
        Failure::Error(e.to_string())
    }
}

/// Write to stdout through one locked handle and flush. Unlike `println!`, a closed stdout comes
/// back as an error (`Failure::StdoutClosed`) instead of a panic.
fn write_stdout(write: impl FnOnce(&mut std::io::StdoutLock<'static>) -> std::io::Result<()>) -> Result<(), Failure> {
    let mut out = std::io::stdout().lock();
    write(&mut out)?;
    out.flush()?;
    Ok(())
}

/// What `sync` says when the owner signed in on the provider's page as somebody other than the
/// account (#103): nothing of that sign-in is used. (It names nobody: this is printed.)
const SOMEONE_ELSE: &str = "sync: you signed in as somebody other than this account; sign in again with the account's own address";

/// `sync` for an account kept in step through Microsoft Graph: the folder changes made here go
/// up and the folders are listed. The token for Graph is taken from `MAILCRAFT_ACCESS_TOKEN`, or
/// the owner signs in on Microsoft's page (for Graph, with `Mail.ReadWrite`).
fn sync_graph(s: &mut Session, account: mailcraft_model::Id, path: &str) -> Result<(), Failure> {
    let request = mailcraft_engine::cmd::graph_mail::graph_mail_request(s, account)?;
    let token = match std::env::var("MAILCRAFT_ACCESS_TOKEN") {
        Ok(token) => token,
        Err(_) => {
            // A shared mailbox is read with the sign-in of the account it is opened through (#70).
            let signer = s.mailbox.owner_of(account).map_or(account, |owner| owner.id);
            let hint = s.mailbox.account(signer).map(|a| a.email.clone()).unwrap_or_default();
            let mut oauth = s.mailbox.account(signer).and_then(|a| a.oauth.clone()).unwrap_or_default();
            if oauth.client_id.is_empty() {
                let built_in = option_env!("MAILCRAFT_MS_CLIENT_ID").unwrap_or_default().to_string();
                oauth.client_id = std::env::var("MAILCRAFT_MS_CLIENT_ID").unwrap_or(built_in);
            }
            let open = |url: &str| eprintln!("sync: open this page in a browser to sign in:\n{url}");
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let tokens = if request.shared && !request.sends.is_empty() {
                // (And for sending from it, when something waits in its Outbox.)
                let signed = mailcraft_net::oauth::sign_in_for_shared_send(&oauth, &hint, &open, &cancel);
                signed.map_err(|refusal| format!("sync: sending from shared mailboxes wasn't allowed ({refusal:?})"))?
            } else if request.shared {
                let signed = mailcraft_net::oauth::sign_in_for_shared(&oauth, &hint, &open, &cancel);
                signed.map_err(|refusal| format!("sync: shared mailboxes weren't allowed ({refusal:?})"))?
            } else {
                let resource = mailcraft_net::oauth::Resource::GraphMail;
                mailcraft_net::oauth::sign_in_for(&oauth, &hint, &open, &cancel, resource).map_err(|e| format!("sync: {e}"))?
            };
            // (Whoever signs in has to be the account that reads it: a shared mailbox's owner.)
            let username = s.mailbox.account(signer).map(|a| a.username.clone()).unwrap_or_default();
            if tokens.someone_else(&hint, &username) {
                return Err(SOMEONE_ELSE.to_string().into());
            }
            tokens.access
        }
    };
    let result = mailcraft_net::graph::sync_folders(&token, &request);
    let failed = !result.errors.is_empty();
    for e in &result.errors {
        eprintln!("sync: {e}");
    }
    let applied = mailcraft_engine::cmd::graph_mail::apply_graph_mail(s, result)?;
    s.run("file.save", &json!({"path": path}))?;
    print_json(&applied)?;
    if failed { Err(Failure::Error("sync: finished with errors".into())) } else { Ok(()) }
}

fn print_json(v: &Value) -> Result<(), Failure> {
    let text = serde_json::to_string_pretty(v).unwrap_or_default();
    write_stdout(|o| writeln!(o, "{text}"))
}

/// `id` or `id={json}` from a `--cmd` value.
fn parse_cmd(text: &str) -> Result<(String, Value), String> {
    match text.split_once('=') {
        Some((id, params)) => {
            let v: Value = serde_json::from_str(params).map_err(|e| format!("--cmd {id}: the parameters aren't JSON ({e})"))?;
            Ok((id.trim().to_string(), v))
        }
        None => Ok((text.trim().to_string(), json!({}))),
    }
}

/// Go to the folder `--folder` names (a path such as `Inbox/Projects`), when given.
fn go_to_folder(s: &mut Session, args: &[String]) -> Result<(), Failure> {
    if let Some(path) = arg_value(args, "--folder") {
        s.run("folder.select", &json!({"path": path}))?;
    }
    Ok(())
}

/// The export command for an output file, by its extension.
fn export_command(out: &str, id: Option<u64>) -> Result<(&'static str, Value), String> {
    let ext = out.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    Ok(match ext.as_str() {
        "mailcraft" | "json" => ("file.saveAs", json!({"path": out})),
        "mbox" | "mbx" => ("file.exportFolder", json!({"path": out})),
        "eml" => match id {
            Some(id) => ("file.exportMessage", json!({"path": out, "id": id})),
            None => return Err(format!("{out}: which message? pass --id N (see `mailcraft-cli list`)")),
        },
        "ics" | "ical" => ("file.exportCalendar", json!({"path": out})),
        "vcf" | "vcard" => ("file.exportContacts", json!({"path": out})),
        _ => return Err(format!("{out}: don't know that format (use .mailcraft, .mbox, .eml, .ics or .vcf)")),
    })
}

fn run(args: &[String]) -> Result<(), Failure> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    let missing = || Failure::Error(format!("missing argument\n\n{USAGE}"));
    match cmd {
        "info" => {
            let pos = positionals("info", &rest, &[])?;
            let mut s = open(pos.first().ok_or_else(missing)?)?;
            print_json(&s.run("file.info", &json!({}))?)
        }
        "folders" => {
            let pos = positionals("folders", &rest, &[])?;
            let mut s = open(pos.first().ok_or_else(missing)?)?;
            print_json(&s.run("folder.list", &json!({}))?)
        }
        "list" => {
            let pos = positionals("list", &rest, &[value("--folder"), value("--query"), value("--limit")])?;
            let mut s = open(pos.first().ok_or_else(missing)?)?;
            go_to_folder(&mut s, &rest)?;
            if let Some(q) = arg_value(&rest, "--query") {
                s.run("view.search", &json!({"query": q}))?;
            }
            let limit = match arg_value(&rest, "--limit") {
                Some(n) => n.parse::<u64>().map_err(|_| "list: --limit takes a number")?,
                None => 200,
            };
            print_json(&s.run("mail.list", &json!({"limit": limit}))?)
        }
        "show" => {
            let pos = positionals("show", &rest, &[])?;
            let mut s = open(pos.first().ok_or_else(missing)?)?;
            let id: u64 = pos.get(1).ok_or_else(missing)?.parse().map_err(|_| "show: the message id is a number")?;
            print_json(&s.run("mail.get", &json!({"id": id}))?)
        }
        "import" => {
            let pos = positionals("import", &rest, &[value("--folder")])?;
            let mailbox = pos.first().ok_or_else(missing)?;
            if pos.len() < 2 {
                return Err(missing());
            }
            let mut s = open_or_new(mailbox)?;
            go_to_folder(&mut s, &rest)?;
            for file in pos.iter().skip(1) {
                let r = s.run("file.import", &json!({"path": file})).map_err(|e| format!("{file}: {e}"))?;
                eprintln!("{file}: {r}");
            }
            s.run("file.save", &json!({"path": mailbox}))?;
            eprintln!("wrote {mailbox}");
            Ok(())
        }
        "export" => {
            let pos = positionals("export", &rest, &[value("--folder"), value("--id")])?;
            let mut s = open(pos.first().ok_or_else(missing)?)?;
            let out = pos.get(1).ok_or_else(missing)?;
            go_to_folder(&mut s, &rest)?;
            let id = match arg_value(&rest, "--id") {
                Some(n) => Some(n.parse::<u64>().map_err(|_| "export: --id takes a number")?),
                None => None,
            };
            let (command, params) = export_command(out, id)?;
            s.run(command, &params)?;
            eprintln!("wrote {out}");
            Ok(())
        }
        "convert" => {
            let pos = positionals("convert", &rest, &[])?;
            let (input, out) = (pos.first().ok_or_else(missing)?, pos.get(1).ok_or_else(missing)?);
            // A mailbox opens; anything else is imported into an empty one.
            let mut s = match mailcraft_engine::io::Format::from_extension(input) {
                Some(mailcraft_engine::io::Format::Mailbox) => open(input)?,
                _ => {
                    let mut s = Session::new(Mailbox::with_local_account("", ""));
                    s.run("file.import", &json!({"path": input})).map_err(|e| format!("{input}: {e}"))?;
                    s
                }
            };
            // One message converts as itself; the rest by what the output holds.
            let only = if s.mailbox.messages.len() == 1 { s.mailbox.messages.keys().next().copied() } else { None };
            let (command, params) = export_command(out, only)?;
            s.run(command, &params)?;
            eprintln!("wrote {out}");
            Ok(())
        }
        "run" => {
            positionals("run", &rest, &[value("--file"), flag("--sample"), value("--cmd"), value("--save"), flag("--print")])?;
            let mut s = match (arg_value(&rest, "--file"), rest.iter().any(|a| a == "--sample")) {
                (Some(f), _) => open(&f)?,
                (None, true) => sample(),
                (None, false) => Session::new(Mailbox::with_local_account("", "")),
            };
            let print = rest.iter().any(|a| a == "--print");
            let mut i = 0;
            while let Some(a) = rest.get(i) {
                if a == "--cmd" {
                    let (id, params) = parse_cmd(rest.get(i + 1).ok_or_else(missing)?)?;
                    let r = s.run(&id, &params).map_err(|e| format!("{id}: {e}"))?;
                    if print {
                        print_json(&json!({"command": id, "result": r}))?;
                    }
                    i += 1;
                }
                i += 1;
            }
            if let Some(out) = arg_value(&rest, "--save") {
                s.run("file.saveAs", &json!({"path": out}))?;
                eprintln!("wrote {out}");
            }
            Ok(())
        }
        "sync" => {
            let pos = positionals("sync", &rest, &[value("--account")])?;
            let path = pos.first().ok_or_else(missing)?;
            let mut s = open(path)?;
            let account = match arg_value(&rest, "--account") {
                Some(email) => s
                    .mailbox
                    .accounts
                    .iter()
                    .find(|a| a.email.eq_ignore_ascii_case(&email))
                    .map(|a| a.id)
                    .ok_or_else(|| format!("sync: no account {email} in {path}"))?,
                // An account a run is made for before a JMAP account, for which none is made here
                // yet (and the engine says so when it is the only one).
                None => {
                    let accounts = &s.mailbox.accounts;
                    accounts
                        .iter()
                        .find(|a| a.kind.has_server() && a.kind != mailcraft_model::AccountKind::Jmap)
                        .or_else(|| accounts.iter().find(|a| a.kind.has_server()))
                        .map(|a| a.id)
                        .ok_or("sync: no account in this mailbox is connected to a mail server")?
                }
            };
            // An account kept in step through Microsoft Graph (R3):
            // its folders, through Graph alone.
            if s.mailbox.account(account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Microsoft) {
                return sync_graph(&mut s, account, path);
            }
            let mut request = mailcraft_engine::cmd::account::sync_request(&s, account)?;
            // Secrets come from the environment or a sign-in, never from the command line (which
            // other users can list).
            match request.account.oauth.clone() {
                // An account that signs in on its provider's page: a token that was handed in, or
                // the sign-in itself, with the page's address printed for a browser.
                Some(mut oauth) => {
                    request.access_token = match std::env::var("MAILCRAFT_ACCESS_TOKEN") {
                        Ok(token) => token,
                        Err(_) => {
                            if oauth.client_id.is_empty() {
                                // The one named when this runs, else the one the build was given.
                                let built_in = option_env!("MAILCRAFT_MS_CLIENT_ID").unwrap_or_default().to_string();
                                oauth.client_id = std::env::var("MAILCRAFT_MS_CLIENT_ID").unwrap_or(built_in);
                            }
                            let open = |url: &str| {
                                eprintln!(
                                    "sync: open this page in a browser to sign in:
{url}"
                                )
                            };
                            let cancel = std::sync::atomic::AtomicBool::new(false);
                            let tokens =
                                mailcraft_net::oauth::sign_in(&oauth, &request.account.email, &open, &cancel).map_err(|e| format!("sync: {e}"))?;
                            if tokens.someone_else(&request.account.email, &request.account.username) {
                                return Err(SOMEONE_ELSE.to_string().into());
                            }
                            tokens.access
                        }
                    };
                }
                None => {
                    request.password = std::env::var("MAILCRAFT_PASSWORD").map_err(|_| "sync: set MAILCRAFT_PASSWORD to the account's password")?
                }
            }
            let result = mailcraft_net::sync(&request);
            let failed = !result.errors.is_empty();
            for e in &result.errors {
                eprintln!("sync: {e}");
            }
            let applied = mailcraft_engine::cmd::account::apply_sync(&mut s, result)?;
            s.run("file.save", &json!({"path": path}))?;
            print_json(&applied)?;
            if failed { Err(Failure::Error("sync: finished with errors".into())) } else { Ok(()) }
        }
        "commands" => {
            positionals("commands", &rest, &[flag("--json")])?;
            let s = Session::empty();
            if rest.iter().any(|a| a == "--json") {
                print_json(&s.registry.describe())
            } else {
                write_stdout(|o| {
                    for c in s.registry.all() {
                        writeln!(o, "{:<28} {:<30} {:<26} {}", c.id, c.label, c.location, c.shortcut)?;
                    }
                    Ok(())
                })
            }
        }
        "detect" => {
            let pos = positionals("detect", &rest, &[flag("--offline")])?;
            let address = pos.first().ok_or_else(missing)?;
            let mut s = Session::empty();
            // From the address alone first; a domain nobody here knows is asked about (only the
            // domain goes out, never the mailbox name).
            let mut found = s.run("account.preset", &json!({"email": address}))?;
            if found["known"] != true
                && !rest.iter().any(|a| a == "--offline")
                && let Some((_, domain)) = address.rsplit_once('@')
            {
                match mailcraft_net::lookup_mx(domain) {
                    Ok(mx) => {
                        found = s.run("account.preset", &json!({"email": address, "mx": mx}))?;
                        found["mx"] = json!(mx);
                    }
                    Err(e) => eprintln!("detect: couldn't look up {domain}: {e}"),
                }
            }
            print_json(&found)
        }
        "parity" => {
            positionals("parity", &rest, &[flag("--markdown")])?;
            let s = Session::empty();
            if rest.iter().any(|a| a == "--markdown") {
                let md = mailcraft_engine::catalog::parity_markdown(&s.registry);
                write_stdout(|o| write!(o, "{md}"))
            } else {
                print_json(&mailcraft_engine::catalog::parity(&s.registry))
            }
        }
        "mcp" => {
            positionals("mcp", &rest, &[value("--connect"), value("--file"), flag("--sample"), value("--files"), value("--allow")])?;
            let allow = allow_classes(&rest)?;
            let files = match arg_value(&rest, "--files") {
                Some(dir) => Some(mailcraft_mcp::files::FilesDir::new(std::path::Path::new(&dir)).map_err(|e| format!("mcp: {e}"))?),
                None => None,
            };
            let backend: Box<dyn mailcraft_mcp::Backend> = match arg_value(&rest, "--connect") {
                Some(addr) => Box::new(
                    mailcraft_mcp::Remote::connect(&addr)
                        .map_err(|e| format!("mcp: can't reach the MailCraft app at {addr}: {e}"))?
                        .with_allow(allow.clone()),
                ),
                None => {
                    let session = match (arg_value(&rest, "--file"), rest.iter().any(|a| a == "--sample")) {
                        (Some(f), _) => open(&f)?,
                        (None, true) => sample(),
                        (None, false) => Session::empty(),
                    };
                    Box::new(mailcraft_mcp::Headless::new(session, allow.clone()))
                }
            };
            let policy = mailcraft_mcp::tools::Policy { files, allow };
            // A client that hangs up ends the server quietly (`Failure::StdoutClosed`).
            mailcraft_mcp::Server::with_policy(backend, policy).serve(std::io::stdin().lock(), std::io::stdout().lock())?;
            Ok(())
        }
        "--version" | "-V" => write_stdout(|o| writeln!(o, "mailcraft-cli {}", env!("CARGO_PKG_VERSION"))),
        "" | "-h" | "--help" | "help" => write_stdout(|o| write!(o, "{USAGE}")),
        other => Err(Failure::Error(format!("unknown command `{other}`\n\n{USAGE}"))),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) | Err(Failure::StdoutClosed) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn options_are_checked_and_positionals_found() {
        let specs = [value("--folder"), flag("--print")];
        assert_eq!(positionals("x", &strings(&["a.mailcraft", "--folder", "Inbox", "b.eml", "--print"]), &specs).unwrap(), ["a.mailcraft", "b.eml"]);
        assert!(positionals("x", &strings(&["--nope"]), &specs).unwrap_err().contains("unknown option --nope"));
        assert!(positionals("x", &strings(&["--folder"]), &specs).unwrap_err().contains("needs a value"));
    }

    #[test]
    fn cmd_values_parse() {
        assert_eq!(parse_cmd("mail.markAllRead").unwrap(), ("mail.markAllRead".to_string(), json!({})));
        assert_eq!(parse_cmd(r#"view.search={"query":"a=b"}"#).unwrap(), ("view.search".to_string(), json!({"query": "a=b"})));
        assert!(parse_cmd("x={nope").is_err());
    }

    #[test]
    fn exports_go_by_extension() {
        assert_eq!(export_command("out.MBOX", None).unwrap().0, "file.exportFolder");
        assert_eq!(export_command("cal.ics", None).unwrap().0, "file.exportCalendar");
        assert_eq!(export_command("m.eml", Some(4)).unwrap().1["id"], 4);
        assert!(export_command("m.eml", None).is_err());
        assert!(export_command("x.docx", None).is_err());
    }

    #[test]
    fn a_scripted_run_changes_the_sample() {
        let dir = std::env::temp_dir().join(format!("mailcraft-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.mailcraft").to_string_lossy().to_string();
        run(&strings(&[
            "run",
            "--sample",
            "--cmd",
            "mail.markAllRead",
            "--cmd",
            r#"folder.new={"name":"From the CLI","parent":null}"#,
            "--save",
            &out,
        ]))
        .unwrap();
        let s = open(&out).unwrap();
        assert!(s.mailbox.folders.iter().any(|f| f.name == "From the CLI"));
        assert_eq!(s.mailbox.unread_count(s.view.folder), 0);
        // Out as .mbox and back in through `convert`.
        let mbox = dir.join("inbox.mbox").to_string_lossy().to_string();
        run(&strings(&["export", &out, &mbox, "--folder", "Inbox"])).unwrap();
        let back = dir.join("back.mailcraft").to_string_lossy().to_string();
        run(&strings(&["convert", &mbox, &back])).unwrap();
        assert_eq!(open(&back).unwrap().mailbox.messages.len(), 13);
        assert!(run(&strings(&["frobnicate"])).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_mailbox_kept_with_its_content_folder_opens_exports_and_converts() {
        let dir = std::env::temp_dir().join(format!("mailcraft-cli-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str| dir.join(name).to_string_lossy().to_string();
        let long: String = (0..200).map(|i| format!("Line {i} of a long message about the quarter's figures.\r\n")).collect();
        let eml = file("long.eml");
        std::fs::write(&eml, format!("From: Ada <ada@example.org>\r\nTo: avery@example.org\r\nSubject: Figures\r\n\r\n{long}")).unwrap();

        // A mailbox that is worked on (`import` saves it in place) keeps its content beside it.
        let kept = file("kept.mailcraft");
        run(&strings(&["import", &kept, &eml])).unwrap();
        assert!(std::path::Path::new(&format!("{kept}.blobs")).is_dir());
        assert!(!String::from_utf8_lossy(&std::fs::read(&kept).unwrap()).contains("Line 150 of a long message"));
        let s = open(&kept).unwrap();
        let wrote = long.replace("\r\n", "\n");
        let text = |s: &Session| s.mailbox.messages.values().next().unwrap().body.text.replace("\r\n", "\n");
        assert!(text(&s).trim_end() == wrote.trim_end());
        // Again into the same mailbox: the first message's content is found, not written again.
        run(&strings(&["import", &kept, &eml])).unwrap();
        assert_eq!(open(&kept).unwrap().mailbox.messages.len(), 2);
        assert_eq!(std::fs::read_dir(format!("{kept}.blobs")).unwrap().count(), 1);

        // Exported or converted, a mailbox is one file with everything in it.
        let (one, two) = (file("one.mailcraft"), file("two.mailcraft"));
        run(&strings(&["export", &kept, &one])).unwrap();
        run(&strings(&["convert", &kept, &two])).unwrap();
        for single in [&one, &two] {
            assert!(!std::path::Path::new(&format!("{single}.blobs")).exists());
            assert!(std::fs::read_to_string(single).unwrap().contains("Line 150 of a long message"));
            let s = open(single).unwrap();
            assert!(s.mailbox.messages.len() == 2 && text(&s).trim_end() == wrote.trim_end());
        }
        // And it is read wherever a mailbox is.
        run(&strings(&["info", &kept])).unwrap();
        run(&strings(&["list", &one])).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A JMAP account (M6): `sync` makes no run of one yet. As the only account with a server, or
    /// named, it is refused with its reason before anything is asked of a server, and the mailbox
    /// file is left as it was.
    #[test]
    fn sync_makes_no_run_of_a_jmap_account_yet() {
        let dir = std::env::temp_dir().join(format!("mailcraft-cli-jmap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("jmap.mailcraft").to_string_lossy().to_string();
        let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
        let server = mailcraft_model::JmapServer { session: "https://jmap.example/.well-known/jmap".into(), bearer: true };
        let jmap = mailcraft_model::Account {
            email: "ada@jmap.example".into(),
            kind: mailcraft_model::AccountKind::Jmap,
            jmap: Some(server),
            ..Default::default()
        };
        mb.add_account(jmap);
        let bytes = mailcraft_engine::io::write_mailbox(&mb).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let refusal = |args: &[&str]| run(&strings(args)).unwrap_err().to_string();
        let (alone, named) = (refusal(&["sync", &path]), refusal(&["sync", &path, "--account", "ada@jmap.example"]));
        let reason = "ada@jmap.example is a JMAP account, and this version doesn't send and receive with one yet";
        assert!(alone.contains(reason) && named.contains(reason), "{alone} / {named}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
