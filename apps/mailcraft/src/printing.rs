//! Printing (`Services::print`): the PDF the window laid out goes to a temporary file, and the
//! system prints it.
//!
//! - Linux and the BSDs: `lp` (CUPS) prints to the default printer; where there is no `lp`, or it
//!   refuses, the PDF is opened (`xdg-open`) so it can be printed from the viewer.
//! - macOS: `lpr` (CUPS), else `open`.
//! - Windows: the shell's Print verb on the file (PowerShell's `Start-Process -Verb Print`), else
//!   the PDF is opened.
//!
//! The window never waits: the program runs on a thread of its own and only its start is
//! checked. The temporary files are in a folder of their own under the system's temporary
//! folder, and those older than a day are cleared at the next print. Nothing of the print is
//! logged but that it went or why it didn't.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

/// The hook for `Services::print`.
pub fn hook() -> Box<dyn Fn(&str, &[u8]) -> Result<(), String>> {
    Box::new(|name: &str, bytes: &[u8]| {
        let path = write(name, bytes)?;
        let spawned = std::thread::Builder::new().name("print".into()).spawn(move || {
            if let Err(e) = hand_over(&path) {
                log::info!("printing: {e}");
            }
        });
        spawned.map(|_| ()).map_err(|e| format!("couldn't start printing ({e})"))
    })
}

fn folder() -> PathBuf {
    std::env::temp_dir().join("mailcraft-print")
}

/// The PDF in the temporary folder, under a name of its own.
fn write(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = folder();
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't make a place for the print ({e})"))?;
    clear_old(&dir);
    let stem: String =
        name.trim_end_matches(".pdf").chars().map(|c| if c.is_alphanumeric() || " -_.".contains(c) { c } else { '_' }).take(60).collect();
    let stem = stem.trim().trim_matches('.');
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let path = dir.join(format!("{}-{stamp}.pdf", if stem.is_empty() { "print" } else { stem }));
    std::fs::write(&path, bytes).map_err(|e| format!("couldn't write the print ({e})"))?;
    Ok(path)
}

/// Earlier prints, a day after they were made.
fn clear_old(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let day = Duration::from_secs(24 * 3600);
    for entry in entries.flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > day);
        if old && entry.path().extension().is_some_and(|e| e == "pdf") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Run `program` and wait for it; `Ok` when it ran and said it succeeded.
fn run(mut command: Command) -> Result<(), String> {
    let status = command.status().map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("ended with {status}")) }
}

#[cfg(target_os = "windows")]
fn hand_over(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // The path goes in as a PowerShell string in single quotes, where a quote is doubled.
    let quoted = path.to_string_lossy().replace('\'', "''");
    let mut print = Command::new("powershell");
    print
        .args(["-NoProfile", "-NonInteractive", "-Command", &format!("Start-Process -FilePath '{quoted}' -Verb Print")])
        .creation_flags(CREATE_NO_WINDOW);
    run(print).or_else(|e| {
        log::info!("printing: the shell's Print didn't take it ({e}); opening the PDF instead");
        let mut open = Command::new("explorer");
        open.arg(path);
        // Explorer says nothing useful in its exit code.
        open.spawn().map(|_| ()).map_err(|e| e.to_string())
    })
}

#[cfg(target_os = "macos")]
fn hand_over(path: &Path) -> Result<(), String> {
    let mut lpr = Command::new("lpr");
    lpr.arg(path);
    run(lpr).or_else(|e| {
        log::info!("printing: lpr didn't take it ({e}); opening the PDF instead");
        let mut open = Command::new("open");
        open.arg(path);
        run(open)
    })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn hand_over(path: &Path) -> Result<(), String> {
    let mut lp = Command::new("lp");
    // The path is absolute (the temporary folder), so it is never taken for an option.
    lp.arg(path);
    run(lp).or_else(|e| {
        log::info!("printing: lp didn't take it ({e}); opening the PDF instead");
        let mut open = Command::new("xdg-open");
        open.arg(path);
        run(open)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_print_is_written_under_a_safe_name_of_its_own() {
        let path = write("../Quarterly: plan?.pdf", b"%PDF-1.7\n").unwrap();
        assert_eq!(path.parent(), Some(folder().as_path()));
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("_Quarterly_ plan_-") && name.ends_with(".pdf"), "{name}");
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1.7\n");
        let _ = std::fs::remove_file(path);
    }
}
