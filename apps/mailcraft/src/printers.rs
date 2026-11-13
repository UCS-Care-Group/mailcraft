// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Printing on the system's printers (File › Print, #32) through CUPS, which macOS, Linux and
//! FreeBSD print with: `lpstat` lists the printers, `lp` sends the PDF the engine laid out. Both
//! are started as programs, never through a shell, with every value its own argument. The PDF is
//! written into MailCraft's own folder of opened files first (`opened_files`), which only this
//! user can read and which is emptied when MailCraft quits. Windows prints through the system's
//! PDF viewer for now: there is no host here, and the window opens the PDF instead.

// The parsing is CUPS's: on Windows only its tests use it.
#![cfg_attr(not(unix), allow(dead_code))]

#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use mailcraft_ui_egui::printing::{Host, Job, PrintError, Printer, valid_range};

/// How long `lpstat` may take to list the printers, and `lp` to take a job.
#[cfg(unix)]
const WAIT: Duration = Duration::from_secs(10);

/// The most printers listed.
const MAX_PRINTERS: usize = 200;

/// Printing through CUPS, when the system has it.
#[cfg(unix)]
pub fn host(opened: &'static crate::opened_files::Opened) -> Option<mailcraft_ui_egui::printing::Host> {
    Some(Host { printers: Arc::new(printers), submit: Arc::new(move |job| submit(opened, job)) })
}

/// No printing of our own on Windows yet: the window opens the PDF in the system's viewer.
#[cfg(not(unix))]
pub fn host(_opened: &'static crate::opened_files::Opened) -> Option<mailcraft_ui_egui::printing::Host> {
    None
}

/// How a program run for printing ended.
#[cfg(unix)]
enum Ran {
    /// Well, with its standard output.
    Done(String),
    /// It couldn't be started, or it said no.
    Failed,
    /// It didn't end within [`WAIT`], and was stopped.
    TimedOut,
}

/// `program args`, waited for at most [`WAIT`]; one that takes longer is stopped (never left
/// running behind the window's back).
#[cfg(unix)]
fn output(program: &str, args: &[&str]) -> Ran {
    use std::io::Read;
    let Ok(mut child) = Command::new(program).args(args).env("LC_ALL", "C").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
    else {
        return Ran::Failed;
    };
    // Read as it comes, so that a long answer never fills the pipe and holds the program up.
    let reader = child.stdout.take().map(|mut out| {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            let _ = (&mut out).take(1 << 20).read_to_end(&mut text);
            text
        })
    });
    let deadline = Instant::now() + WAIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Ran::TimedOut;
            }
        }
    };
    let text = reader.and_then(|r| r.join().ok()).unwrap_or_default();
    if status.success() { Ran::Done(String::from_utf8_lossy(&text).into_owned()) } else { Ran::Failed }
}

/// The standard output of `program args` when it ended well.
#[cfg(unix)]
fn answer(program: &str, args: &[&str]) -> Option<String> {
    match output(program, args) {
        Ran::Done(text) => Some(text),
        Ran::Failed | Ran::TimedOut => None,
    }
}

/// The printers CUPS knows, the default first, each with its description where it has one.
#[cfg(unix)]
fn printers() -> Vec<Printer> {
    let Some(names) = answer("lpstat", &["-e"]) else { return Vec::new() };
    let default = answer("lpstat", &["-d"]).and_then(|d| parse_default(&d));
    let described = answer("lpstat", &["-l", "-p"]).map(|d| parse_descriptions(&d)).unwrap_or_default();
    parse_printers(&names, default.as_deref(), &described)
}

/// The printer `lpstat -d` names as the default.
pub fn parse_default(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.contains("default destination"))?;
    let name = line.rsplit_once(':')?.1.trim();
    (!name.is_empty() && !name.contains(' ')).then(|| name.to_string())
}

/// Each printer's description in `lpstat -l -p`.
pub fn parse_descriptions(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("printer ") {
            current = rest.split_whitespace().next().map(str::to_string);
        } else if let (Some(name), Some(desc)) = (&current, line.trim().strip_prefix("Description:")) {
            let desc = desc.trim();
            if !desc.is_empty() {
                out.push((name.clone(), desc.chars().filter(|c| !c.is_control()).take(120).collect()));
            }
        }
    }
    out
}

/// The printers of `lpstat -e`, the default first, labelled by their descriptions.
pub fn parse_printers(names: &str, default: Option<&str>, described: &[(String, String)]) -> Vec<mailcraft_ui_egui::printing::Printer> {
    use mailcraft_ui_egui::printing::Printer;
    let mut out: Vec<Printer> = Vec::new();
    for name in names.lines().map(str::trim).filter(|n| usable_name(n)).take(MAX_PRINTERS) {
        if out.iter().any(|p| p.name == name) {
            continue;
        }
        let label = described.iter().find(|(n, _)| n == name).map_or_else(|| name.replace('_', " "), |(_, d)| d.clone());
        out.push(Printer { name: name.to_string(), label, default: default == Some(name) });
    }
    out.sort_by_key(|p| !p.default);
    out
}

/// A printer name that can go to `lp` as it is: no option-like start, no white space or control
/// characters.
pub fn usable_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 127 && !name.starts_with('-') && !name.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The arguments of `lp` for `job`, the file at `path` last.
pub fn lp_args(job: &mailcraft_ui_egui::printing::Job, path: &str) -> Vec<String> {
    let title: String = job.title.chars().filter(|c| !c.is_control()).take(100).collect();
    let mut args = vec!["-d".to_string(), job.printer.clone(), "-n".into(), job.copies.clamp(1, 99).to_string()];
    if !title.trim().is_empty() {
        args.extend(["-t".to_string(), title]);
    }
    if let Some(pages) = &job.pages {
        args.extend(["-P".to_string(), pages.clone()]);
    }
    args.extend(["-o".to_string(), format!("media={}", job.paper)]);
    args.extend(["-o".to_string(), if job.two_sided { "sides=two-sided-long-edge".into() } else { "sides=one-sided".into() }]);
    if !job.color {
        args.extend(["-o".to_string(), "print-color-mode=monochrome".into(), "-o".into(), "ColorModel=Gray".into()]);
    }
    args.extend(["--".to_string(), path.to_string()]);
    args
}

/// Send `job` to its printer. When `lp` doesn't answer in time it is stopped, and the owner is
/// told the job may print all the same, so that printing again isn't the first thing they do.
#[cfg(unix)]
fn submit(opened: &crate::opened_files::Opened, job: Job) -> Result<(), PrintError> {
    if !usable_name(&job.printer) {
        return Err(PrintError::Printer);
    }
    if job.pages.as_deref().is_some_and(|p| !valid_range(p)) {
        return Err(PrintError::Pages);
    }
    let path = opened
        .write(&format!("{}.pdf", if job.title.trim().is_empty() { "message" } else { job.title.as_str() }), &job.pdf)
        .map_err(PrintError::File)?;
    let path = path.to_str().ok_or_else(|| PrintError::File(String::new()))?.to_string();
    let args = lp_args(&job, &path);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match output("lp", &refs) {
        Ran::Done(_) => Ok(()),
        Ran::Failed => {
            log::warn!("lp refused a print job");
            Err(PrintError::Refused)
        }
        Ran::TimedOut => {
            log::warn!("lp didn't answer a print job in time and was stopped");
            Err(PrintError::NoAnswer)
        }
    }
}

#[cfg(test)]
#[path = "printers_tests.rs"]
mod tests;
