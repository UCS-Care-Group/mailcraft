// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Optional native attachment renderers. An invocation gets a private directory, a
//! generated input filename and the attachment bytes. No mailbox path is passed to it.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_ui_egui::attachment_preview::{Error, Format, Host, MAX_PAGES, MAX_SIDE, Page, Request};

const DEADLINE: Duration = Duration::from_secs(12);
const MAX_PNG_BYTES: u64 = 16 * 1024 * 1024;
const MAX_INFO_BYTES: u64 = 64 * 1024;

/// Expose the detected native helpers and reserve one renderer slot per window.
pub(crate) fn service(ctx: &egui::Context) -> Option<Host> {
    let pdf = executable("MAILCRAFT_PDFCRAFT_CLI", "PdfCraft", "pdfcraft-cli");
    let photo = executable("MAILCRAFT_PHOTOCRAFT_CLI", "PhotoCraft", "photocraft-cli");
    if pdf.is_none() && photo.is_none() {
        return None;
    }
    let (has_pdf, has_photo) = (pdf.is_some(), photo.is_some());
    let busy = Arc::new(AtomicBool::new(false));
    let ctx = ctx.clone();
    Some(Host {
        pdf: has_pdf,
        images: has_photo,
        render: Box::new(move |request| {
            let program = if request.format == Format::Pdf { pdf.clone() } else { photo.clone() };
            // The guard is dropped even if spawning the worker fails.
            let Some(slot) = Slot::acquire(&busy) else {
                let (send, receive) = std::sync::mpsc::channel();
                let _ = send.send(Err(Error::Busy));
                return receive;
            };
            crate::background(&ctx, "attachment preview", move || {
                let _slot = slot;
                render(&program.ok_or(Error::Unavailable)?, request)
            })
        }),
    })
}

struct Slot(Arc<AtomicBool>);

impl Slot {
    /// Reserve the renderer until the worker finishes or its guard is dropped.
    fn acquire(busy: &Arc<AtomicBool>) -> Option<Self> {
        busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).ok().map(|_| Self(busy.clone()))
    }
}

impl Drop for Slot {
    /// Make the slot available to the next preview request.
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Resolve an explicit override, a platform installation, or an absolute PATH entry.
fn executable(variable: &str, product: &str, tool: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(variable) {
        return absolute_file(PathBuf::from(path));
    }
    let name = format!("{tool}{}", std::env::consts::EXE_SUFFIX);
    let installed = if cfg!(windows) {
        ["ProgramFiles", "ProgramW6432"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|dir| PathBuf::from(dir).join(product).join(&name))
            .find_map(absolute_file)
    } else if cfg!(target_os = "macos") {
        absolute_file(PathBuf::from("/Applications").join(format!("{product}.app")).join("Contents/MacOS").join(&name))
    } else {
        None
    };
    installed.or_else(|| {
        std::env::var_os("PATH")
            .and_then(|paths| std::env::split_paths(&paths).filter(|path| path.is_absolute()).map(|path| path.join(&name)).find_map(absolute_file))
    })
}

/// Canonicalize an absolute file path, requiring an .exe extension on Windows.
fn absolute_file(path: PathBuf) -> Option<PathBuf> {
    let allowed_type = !cfg!(windows) || path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));
    if !path.is_absolute() || !allowed_type {
        return None;
    }
    let path = path.canonicalize().ok()?;
    path.is_file().then_some(path)
}

/// Stage one attachment and render the requested page under the shared deadline.
fn render(program: &Path, request: Request) -> Result<Page, Error> {
    if request.page == 0
        || request.page > MAX_PAGES
        || request.pages.is_some_and(|n| n > MAX_PAGES || request.page > n)
        || request.format != Format::Pdf && request.page != 1
    {
        return Err(Error::Invalid);
    }

    let deadline = Instant::now() + DEADLINE;
    if request.cancel.load(Ordering::Relaxed) {
        return Err(Error::TimedOut);
    }
    let bytes = request.load()?;
    let dir = tempfile::Builder::new().prefix("mailcraft-preview-").tempdir().map_err(|_| Error::Failed)?;
    let input = dir.path().join(format!("attachment.{}", request.format.extension()));
    let output = dir.path().join("preview.png");
    mailcraft_engine::mark::write_new_from_mail(&input, &bytes).map_err(|_| Error::Failed)?;

    let run =
        |args: &[OsString], watched: &Path, limit, stdout| run_helper(program, dir.path(), args, watched, limit, stdout, deadline, &request.cancel);
    let pages = match request.format {
        Format::Pdf => {
            let pages = if let Some(n) = request.pages {
                n
            } else {
                let (file, info) = tempfile::NamedTempFile::new_in(dir.path()).map_err(|_| Error::Failed)?.into_parts();
                run(&["info".into(), "attachment.pdf".into()], &info, MAX_INFO_BYTES, Stdio::from(file))?;
                pdf_pages(&read_bounded(&info, MAX_INFO_BYTES)?)?
            };
            if request.page > pages {
                return Err(Error::Invalid);
            }
            run(
                &[
                    "render".into(),
                    "attachment.pdf".into(),
                    "--page".into(),
                    request.page.to_string().into(),
                    "--dpi".into(),
                    "96".into(),
                    "--out".into(),
                    "preview.png".into(),
                ],
                &output,
                MAX_PNG_BYTES,
                Stdio::null(),
            )?;
            pages
        }
        _ => {
            run(
                &[
                    "convert".into(),
                    format!("attachment.{}", request.format.extension()).into(),
                    "preview.png".into(),
                    "--no-gpu".into(),
                    "--metadata".into(),
                    "none".into(),
                ],
                &output,
                MAX_PNG_BYTES,
                Stdio::null(),
            )?;
            1
        }
    };
    let png = read_bounded(&output, MAX_PNG_BYTES)?;
    let rgba = decode_png(&png)?;
    Ok(Page { image: egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()), pages })
}

/// Read a bounded page count from either supported PdfCraft metadata shape.
fn pdf_pages(data: &[u8]) -> Result<u32, Error> {
    let info: serde_json::Value = serde_json::from_slice(data).map_err(|_| Error::Invalid)?;
    let value = info.get("pages").or_else(|| info.get("page_count")).ok_or(Error::Invalid)?;
    let n = value.as_u64().or_else(|| value.as_array().map(|pages| pages.len() as u64)).ok_or(Error::Invalid)?;
    u32::try_from(n).ok().filter(|n| (1..=MAX_PAGES).contains(n)).ok_or(Error::Invalid)
}

/// Prepare a direct child process with temporary configuration and scratch paths.
fn command(program: &Path, dir: &Path) -> Command {
    let mut command = Command::new(program);
    command.current_dir(dir).env_clear().stdin(Stdio::null()).stderr(Stdio::null());
    for name in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "TEMP", "TMP", "TMPDIR"] {
        command.env(name, dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Run one helper command, ending the child on cancellation, timeout or oversized output.
#[allow(clippy::too_many_arguments)]
fn run_helper(
    program: &Path,
    dir: &Path,
    args: &[OsString],
    output: &Path,
    limit: u64,
    stdout: Stdio,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    if cancel.load(Ordering::Relaxed) || Instant::now() >= deadline {
        return Err(Error::TimedOut);
    }
    let mut child = command(program, dir).args(args).stdout(stdout).spawn().map_err(|_| Error::Unavailable)?;
    let result = loop {
        if cancel.load(Ordering::Relaxed) || Instant::now() >= deadline {
            break Err(Error::TimedOut);
        }
        if fs::symlink_metadata(output).is_ok_and(|meta| !meta.is_file() || meta.len() > limit) {
            break Err(Error::TooLarge);
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(Error::Failed),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break Err(Error::Failed),
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Read a regular output file with an independent cap on bytes read.
fn read_bounded(path: &Path, cap: u64) -> Result<Vec<u8>, Error> {
    let meta = fs::symlink_metadata(path).map_err(|_| Error::Failed)?;
    if !meta.is_file() || meta.len() > cap {
        return Err(Error::TooLarge);
    }
    let mut data = Vec::new();
    File::open(path).and_then(|file| file.take(cap + 1).read_to_end(&mut data)).map_err(|_| Error::Failed)?;
    if data.len() as u64 > cap { Err(Error::TooLarge) } else { Ok(data) }
}

/// Decode only PNG output within the pixel and allocation limits.
fn decode_png(data: &[u8]) -> Result<image::RgbaImage, Error> {
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(Error::Invalid);
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(data), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(48 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map(|image| image.into_rgba8()).map_err(|_| Error::Invalid)
}

#[cfg(test)]
#[path = "attachment_render_tests.rs"]
mod tests;
