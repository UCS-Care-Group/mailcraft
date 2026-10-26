// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Installing a new version of MailCraft, and going back to the one before, for a copy that
//! updates itself (`install.rs`: the Windows `.msi`, the macOS app, a Linux AppImage).
//!
//! The window says when a new version is downloaded and checked (`mailcraft_ui_egui::updates`);
//! once it has closed (the mailbox and the settings saved), [`on_exit`] installs it:
//!
//! - **Windows:** the `.msi` is installed by Windows Installer (`msiexec /i <file> /passive
//!   /norestart`), whose administrator prompt comes once per update. That can't happen while
//!   MailCraft runs (its own file is in use), so a copy of this program, in the data folder,
//!   waits for this one to have gone ([`FINISH_UPDATE`], [`finish`]), checks the file again,
//!   runs the installer, keeps the `.msi` for going back and starts MailCraft again when it was
//!   asked to. A no to the prompt leaves the old version, which says so at its next start.
//! - **macOS:** the disk image is attached read-only where nothing else sees it (`hdiutil attach
//!   -nobrowse -readonly`), the app in it copied beside the running one and put in its place by
//!   renaming; the old one goes to the data folder, the image is detached. When the running app
//!   is signed, the new one must pass `codesign --verify` before the swap and in its place after
//!   it, or it is swapped back. (Off for now, until the builds are signed with a Developer ID and
//!   notarized: `install::MAC_SELF_INSTALL`. A Mac copy is then told of new versions only.)
//! - **AppImage:** the new file is copied beside `$APPIMAGE`, made executable and renamed over
//!   it; the old one is copied to the data folder first.
//!
//! Nothing is run before it is checked: the file installed is the one downloaded and checked by
//! the release's signature in this run, and its SHA-256, the one the signed list gives, is
//! worked out again right before it is used. A file handed to another program (Windows
//! Installer, `hdiutil`) is a copy of it in a folder made for that alone, checked as it is
//! opened and held open until the program is done ([`Handed`]). Programs are started through `std::process::Command` with fixed arguments, never a
//! shell. A new version that is started waits for the old one to have gone ([`AFTER_UPDATE`]),
//! which holds a lock in the data folder ([`hold_lock`]) until it exits.
//!
//! **Going back:** the version that was replaced is kept, one back (`previous-version/` in the
//! data folder; on Windows the `.msi` the running version came from is in `this-version/`, so
//! there is one to go back to from the second update on). A record of the last update
//! (`update-record.json`) and a count of the starts of the new version that didn't run a minute
//! (`update-starts.json`) say at the next start what came of it ([`at_start`]): a version that
//! wasn't installed after all, or one that didn't start twice running, which offers to go back.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mailcraft_model::update::Version;
use mailcraft_ui_egui::updates::{OnExit, UpdateStart};
use serde_json::{Value, json};

use crate::install::Install;

/// Started with this, MailCraft waits for the version it replaced to have gone before it opens
/// the mailbox.
pub const AFTER_UPDATE: &str = "--after-update";
/// Started with this (a copy of the program in the data folder, on Windows), it installs an
/// `.msi` once the program that started it has gone ([`finish`]), and does nothing else.
pub const FINISH_UPDATE: &str = "--finish-update";
/// How long a run has to last for its start to count as one that worked.
pub const SETTLED_AFTER: Duration = Duration::from_secs(60);
/// Starts of a newly installed version left unfinished before going back is offered.
const UNFINISHED: u64 = 2;
/// How long a new version waits for the old one to have gone.
const WAIT_FOR_OLD: Duration = Duration::from_secs(30);
/// Largest record read.
const MAX_JSON: u64 = 64 * 1024;
/// Most files and folders copied with an app, and how deep.
const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 32;
/// What Windows Installer answers when it installed (3010, 1641: a restart of the computer is
/// wanted, or was started).
const MSI_DONE: [i32; 3] = [0, 3010, 1641];

/// The places in the data folder the updating keeps things in.
#[derive(Clone, Debug)]
pub struct Folders {
    root: PathBuf,
}

impl Folders {
    pub fn new(root: PathBuf) -> Folders {
        Folders { root }
    }
    /// The version that was replaced (an app, an AppImage or an `.msi`) and `kept.json`.
    /// Downloads, until they are installed or refused.
    pub fn updates(&self) -> PathBuf {
        self.root.join(crate::updater::FOLDER)
    }
    fn previous(&self) -> PathBuf {
        self.root.join("previous-version")
    }
    /// On Windows: the `.msi` the running version came from, when it came by an update.
    fn this_version(&self) -> PathBuf {
        self.root.join("this-version")
    }
    fn record(&self) -> PathBuf {
        self.root.join("update-record.json")
    }
    fn starts(&self) -> PathBuf {
        self.root.join("update-starts.json")
    }
    fn lock(&self) -> PathBuf {
        self.root.join("update.lock")
    }
    /// On Windows: the copy of the program that installs the `.msi`, and the installer's log.
    fn helper(&self) -> PathBuf {
        self.root.join("update-helper")
    }
    /// On macOS: where the disk image is attached.
    fn mount(&self) -> PathBuf {
        self.root.join("update-mount")
    }
    /// The folders a checked file is copied into, each made for one hand-over ([`Handed`]).
    fn handed(&self) -> PathBuf {
        self.updates().join("handed")
    }
}

/// A file downloaded and checked by the release's signature in this run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub version: Version,
    pub path: PathBuf,
    pub sha256: [u8; 32],
}

/// What the download hands the installing at exit.
pub type Shared = Arc<Mutex<Option<Verified>>>;

/// The record of the last update: from which version to which, whether it was going back, and
/// whether the new version has run long enough to count as one that starts.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Record {
    from: String,
    to: String,
    back: bool,
    settled: bool,
}

/// What `kept.json` says of a version kept: its version, the name of the file or app beside it,
/// and (a file's) SHA-256.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Kept {
    version: String,
    file: String,
    sha256: Option<[u8; 32]>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    let text = text.as_bytes();
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let pair = text.get(i * 2..i * 2 + 2)?;
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

/// A version as written in a record of ours, when it is one.
fn version(text: Option<&str>) -> Option<String> {
    Version::parse(text?).map(|v| v.to_string())
}

/// A plain name of a file in a folder: no separators, not hidden, not `..`.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && Path::new(name).file_name() == Some(OsStr::new(name)) && name.len() <= 255
}

fn read_json(path: &Path) -> Option<Value> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_JSON {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Written whole beside and renamed over.
fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        // (The settings folder, when it isn't there yet: for the owner alone.)
        mailcraft_engine::store::create_private_dir_all(dir)?;
    }
    let mut part = path.as_os_str().to_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    fs::write(&part, value.to_string())?;
    fs::rename(&part, path)
}

fn read_record(folders: &Folders) -> Option<Record> {
    let v = read_json(&folders.record())?;
    Some(Record {
        from: version(v.get("from").and_then(Value::as_str))?,
        to: version(v.get("to").and_then(Value::as_str))?,
        back: v.get("back").and_then(Value::as_bool).unwrap_or(false),
        settled: v.get("settled").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn write_record(folders: &Folders, r: &Record) -> io::Result<()> {
    write_json(&folders.record(), &json!({"from": r.from, "to": r.to, "back": r.back, "settled": r.settled}))
}

fn read_kept(dir: &Path) -> Option<Kept> {
    let v = read_json(&dir.join("kept.json"))?;
    let file = v.get("file").and_then(Value::as_str).filter(|f| plain_name(f))?.to_string();
    Some(Kept { version: version(v.get("version").and_then(Value::as_str))?, file, sha256: v.get("sha256").and_then(Value::as_str).and_then(unhex) })
}

fn write_kept(dir: &Path, kept: &Kept) -> io::Result<()> {
    write_json(&dir.join("kept.json"), &json!({"version": kept.version, "file": kept.file, "sha256": kept.sha256.map(|s| hex(&s))}))
}

/// At the start of version `running`, before the window: what came of the last update, and a
/// start of a newly installed version counted until it has run a minute ([`settle`]).
pub fn at_start(folders: &Folders, running: &str) -> UpdateStart {
    let mut out = UpdateStart::default();
    // What is left of an `.msi` installed from a copy of the program: it has done its work.
    let _ = fs::remove_dir_all(folders.helper().join("program"));
    let _ = fs::remove_dir_all(folders.mount());
    let _ = fs::remove_dir_all(folders.handed());
    let Some(record) = read_record(folders) else { return out };
    if record.to == running {
        if !record.settled {
            let starts = read_json(&folders.starts());
            let same = starts.as_ref().and_then(|s| s.get("version")).and_then(Value::as_str) == Some(running);
            let unfinished = if same { starts.as_ref().and_then(|s| s.get("unfinished")).and_then(Value::as_u64).unwrap_or(0) } else { 0 };
            let kept = read_kept(&folders.previous()).filter(|k| k.version == record.from);
            if unfinished >= UNFINISHED && !record.back && kept.is_some() {
                out.didnt_start = Some((running.to_string(), record.from.clone()));
            }
            if let Err(e) = write_json(&folders.starts(), &json!({"version": running, "unfinished": unfinished.saturating_add(1).min(1_000)})) {
                log::warn!("couldn't note this start of a newly installed MailCraft: {}", e.kind());
            }
        }
    } else {
        if record.from == running {
            // What was to be installed when MailCraft last closed wasn't.
            if record.back {
                out.not_gone_back = Some(record.to.clone());
            } else {
                out.not_installed = Some(record.to.clone());
            }
            log::warn!("MailCraft {} was to be installed and wasn't; {running} is still the one here", record.to);
        }
        let _ = fs::remove_file(folders.record());
        let _ = fs::remove_file(folders.starts());
    }
    out
}

/// Version `running` has run long enough, or was quit as it should be: its start worked.
pub fn settle(folders: &Folders, running: &str) {
    let Some(mut record) = read_record(folders) else { return };
    if record.to != running || record.settled {
        return;
    }
    record.settled = true;
    let _ = write_record(folders, &record);
    let _ = fs::remove_file(folders.starts());
}

/// Held by a program that has installed a new version and is about to exit, until it has.
static HELD: OnceLock<File> = OnceLock::new();

fn open_lock(folders: &Folders) -> io::Result<File> {
    mailcraft_engine::store::create_private_dir_all(&folders.root)?;
    fs::OpenOptions::new().create(true).truncate(false).write(true).open(folders.lock())
}

/// Take the lock a new version waits on, for as long as this program runs. Taken once, it is
/// held: a second open of the file would be a lock of its own, refused.
pub fn hold_lock(folders: &Folders) -> bool {
    if HELD.get().is_some() {
        return true;
    }
    let Ok(file) = open_lock(folders) else { return false };
    if file.try_lock().is_err() {
        return false;
    }
    HELD.set(file).is_ok() || HELD.get().is_some()
}

/// Wait (at most `most`) for the program that holds the lock to have gone. Whether it has.
pub fn wait_for_lock(folders: &Folders, most: Duration) -> bool {
    let Ok(file) = open_lock(folders) else { return true };
    let began = Instant::now();
    loop {
        if file.try_lock().is_ok() {
            let _ = file.unlock();
            return true;
        }
        if began.elapsed() >= most {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// [`wait_for_lock`] as a new version does it, once.
pub fn wait_for_the_old_one(folders: &Folders) {
    if !wait_for_lock(folders, WAIT_FOR_OLD) {
        log::warn!("the version this one replaced was still running after {} seconds", WAIT_FOR_OLD.as_secs());
    }
}

/// A program and its arguments, run without a shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl Run {
    fn new(program: impl Into<PathBuf>, args: &[&OsStr]) -> Run {
        Run { program: program.into(), args: args.iter().map(|a| a.to_os_string()).collect() }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        command
    }

    /// Run it and wait: its exit code (`None`: it couldn't be started, or was stopped by a signal).
    fn wait(&self) -> Option<i32> {
        match self.command().status() {
            Ok(status) => status.code(),
            Err(e) => {
                log::warn!("{} couldn't be started: {}", self.name(), e.kind());
                None
            }
        }
    }

    /// Start it and leave it running after this program has gone.
    fn spawn_detached(&self) -> io::Result<()> {
        let mut command = self.command();
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt as _;
            // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
            command.creation_flags(0x0000_0008 | 0x0000_0200);
        }
        command.spawn().map(|_| ())
    }

    /// The program's name, for the log (never a path of the owner's).
    fn name(&self) -> String {
        self.program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

/// Windows Installer, from the system's own folder (never one found on `PATH`).
fn msiexec_program() -> PathBuf {
    let root = std::env::var_os("SystemRoot").filter(|r| Path::new(r).is_absolute()).unwrap_or_else(|| OsString::from(r"C:\Windows"));
    PathBuf::from(root).join("System32").join("msiexec.exe")
}

/// `msiexec /i <msi> /passive /norestart /log <log>`: progress shown, no questions but Windows'
/// own administrator prompt, never a restart of the computer.
pub fn msiexec(msi: &Path, log: &Path) -> Run {
    Run::new(
        msiexec_program(),
        &[OsStr::new("/i"), msi.as_os_str(), OsStr::new("/passive"), OsStr::new("/norestart"), OsStr::new("/log"), log.as_os_str()],
    )
}

pub fn hdiutil_attach(dmg: &Path, mountpoint: &Path) -> Run {
    Run::new(
        "/usr/bin/hdiutil",
        &[
            OsStr::new("attach"),
            OsStr::new("-nobrowse"),
            OsStr::new("-readonly"),
            OsStr::new("-noautoopen"),
            OsStr::new("-mountpoint"),
            mountpoint.as_os_str(),
            dmg.as_os_str(),
        ],
    )
}

pub fn hdiutil_detach(mountpoint: &Path) -> Run {
    Run::new("/usr/bin/hdiutil", &[OsStr::new("detach"), mountpoint.as_os_str(), OsStr::new("-force")])
}

pub fn codesign_verify(app: &Path) -> Run {
    Run::new("/usr/bin/codesign", &[OsStr::new("--verify"), OsStr::new("--deep"), OsStr::new("--strict"), app.as_os_str()])
}

/// A macOS app started as a new instance (the old one may not have gone yet).
pub fn open_app(app: &Path) -> Run {
    Run::new("/usr/bin/open", &[OsStr::new("-n"), app.as_os_str(), OsStr::new("--args"), OsStr::new(AFTER_UPDATE)])
}

/// The program (an AppImage, `mailcraft.exe`) started again.
pub fn start_program(program: &Path) -> Run {
    Run::new(program, &[OsStr::new(AFTER_UPDATE)])
}

/// What the copy of the program that installs an `.msi` is to do ([`finish`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub msi: PathBuf,
    pub sha256: [u8; 32],
    pub from: String,
    pub to: String,
    /// Going back to a kept version.
    pub back: bool,
    /// The program to start once it is installed.
    pub start: Option<PathBuf>,
}

impl Job {
    /// The arguments it is started with.
    pub fn args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            FINISH_UPDATE.into(),
            self.msi.clone().into_os_string(),
            hex(&self.sha256).into(),
            self.from.clone().into(),
            self.to.clone().into(),
            if self.back { "back" } else { "forward" }.into(),
        ];
        if let Some(start) = &self.start {
            args.push(start.clone().into_os_string());
        }
        args
    }

    /// The job its arguments (after the program's name) say, when they say one.
    pub fn parse(args: &[OsString]) -> Option<Job> {
        let text = |i: usize| args.get(i).and_then(|a| a.to_str());
        if text(0) != Some(FINISH_UPDATE) || args.len() > 7 {
            return None;
        }
        let msi = PathBuf::from(args.get(1)?);
        let back = match text(5)? {
            "back" => true,
            "forward" => false,
            _ => return None,
        };
        let job = Job {
            sha256: unhex(text(2)?)?,
            from: version(text(3))?,
            to: version(text(4))?,
            back,
            start: args.get(6).map(PathBuf::from).filter(|p| p.is_absolute()),
            msi,
        };
        (job.msi.is_absolute() && job.msi.extension() == Some(OsStr::new("msi"))).then_some(job)
    }
}

/// The copy of the program in the data folder, on Windows: wait for the program that started
/// it to have gone, check the `.msi`, install it, keep it for going back, and start MailCraft
/// again when it was asked to. Returns the process's exit code.
pub fn finish(job: &Job, folders: &Folders) -> i32 {
    wait_for_the_old_one(folders);
    let installed = install_msi(job, folders, &mut |run| run.wait());
    if installed {
        let _ = keep_installed_msi(folders, job);
    }
    if let Some(start) = &job.start {
        let _ = start_program(start).spawn_detached();
    }
    if installed { 0 } else { 1 }
}

/// The `.msi` installed by Windows Installer, run by `run`, from a checked copy of it
/// ([`Handed`]), never from the file in the data folder; whether it was installed.
fn install_msi(job: &Job, folders: &Folders, run: &mut dyn FnMut(&Run) -> Option<i32>) -> bool {
    let handed = match Handed::new(&job.msi, job.sha256, &folders.handed()) {
        Ok(handed) => handed,
        Err(e) => {
            log::warn!("the installer wasn't run: {e}");
            return false;
        }
    };
    let code = run(&msiexec(&handed.path, &folders.helper().join("install.log")));
    // (The copy is held open until here, and its folder deleted after.)
    drop(handed);
    code.is_some_and(|c| MSI_DONE.contains(&c))
}

/// A checked file handed to a program of the system's (Windows Installer, `hdiutil`): copied
/// into a folder made for this hand-over alone (for the owner only, on Unix), checked once more
/// by its SHA-256 as it is opened, and held open until the program is done with it (on Windows
/// shared for reading only, so that nothing changes it meanwhile). The program is given the
/// copy, so that what it reads is what was checked rather than a file in the data folder that
/// may have been changed since. The folder goes when this does.
struct Handed {
    dir: PathBuf,
    path: PathBuf,
    held: Option<File>,
}

impl Handed {
    /// Most tries at a name for the folder that nothing has.
    const TRIES: u32 = 8;

    /// `file`, which must have the SHA-256 `expected`, copied into a new folder in `within`.
    fn new(file: &Path, expected: [u8; 32], within: &Path) -> io::Result<Handed> {
        let name = file.file_name().and_then(OsStr::to_str).filter(|n| plain_name(n)).ok_or_else(|| io::Error::other("no name"))?;
        mailcraft_engine::store::create_private_dir_all(within)?;
        let dir = Handed::fresh_dir(within)?;
        let mut handed = Handed { path: dir.join(name), dir, held: None };
        fs::copy(file, &handed.path)?;
        let mut held = Handed::open(&handed.path)?;
        if mailcraft_net::update_download::sha256_of(&mut held) != Some(expected) {
            // (Dropped: the folder and the copy go.)
            return Err(io::Error::other("the copy isn't the file that was checked"));
        }
        handed.held = Some(held);
        Ok(handed)
    }

    /// A folder that wasn't there, made now: for its owner only on Unix; on Windows it takes what
    /// the data folder in the user's own profile allows, which is the user alone.
    fn fresh_dir(within: &Path) -> io::Result<PathBuf> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        for n in 0..Handed::TRIES {
            let dir = within.join(format!("{}-{nanos:09}-{n}", std::process::id()));
            match builder.create(&dir) {
                Ok(()) => return Ok(dir),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("no new folder could be made for the update"))
    }

    /// The copy opened for reading; on Windows nobody may write to it or delete it while it is.
    fn open(path: &Path) -> io::Result<File> {
        let mut open = fs::OpenOptions::new();
        open.read(true);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            // FILE_SHARE_READ
            open.share_mode(0x0000_0001);
        }
        open.open(path)
    }
}

impl Drop for Handed {
    fn drop(&mut self) {
        // Closed first: on Windows an open file can't be deleted.
        drop(self.held.take());
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// After an `.msi` installed: the one the running version came from becomes the one kept back,
/// and this one the one it came from. Going back, the kept one becomes the one it came from, and
/// none is kept back.
fn keep_installed_msi(folders: &Folders, job: &Job) -> io::Result<()> {
    let (this, previous) = (folders.this_version(), folders.previous());
    let name = job.msi.file_name().and_then(OsStr::to_str).filter(|n| plain_name(n)).ok_or_else(|| io::Error::other("no name"))?.to_string();
    let kept = Kept { version: job.to.clone(), file: name.clone(), sha256: Some(job.sha256) };
    if job.back {
        let _ = fs::remove_dir_all(&this);
        fs::rename(&previous, &this)?;
        return write_kept(&this, &kept);
    }
    let _ = fs::remove_dir_all(&previous);
    if read_kept(&this).is_some() {
        fs::rename(&this, &previous)?;
    } else {
        let _ = fs::remove_dir_all(&this);
    }
    fs::create_dir_all(&this)?;
    move_file(&job.msi, &this.join(&name))?;
    write_kept(&this, &kept)
}

/// Rename, or copy and delete when the two places are on different disks.
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to)?;
    fs::remove_file(from)
}

/// `.<name>.<what>` beside `path`.
fn beside(path: &Path, what: &str) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| io::Error::other("no name"))?;
    let mut hidden = OsString::from(".");
    hidden.push(name);
    hidden.push(".");
    hidden.push(what);
    Ok(path.with_file_name(hidden))
}

/// Remove a file, a link or a folder and what is in it; nothing there is no error.
fn remove_any(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// [`remove_any`], and when that fails, once more after giving the owner write to every folder
/// in it: an app copied by 0.8.1 and earlier kept the folders of a read-only disk image as they
/// were, and nothing could be removed from them (#497).
fn remove_stubborn(path: &Path) -> io::Result<()> {
    if remove_any(path).is_ok() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        let mut left = MAX_ENTRIES;
        let_owner_in(path, 0, &mut left);
    }
    remove_any(path)
}

/// Give the owner read, write and search on `path` and every folder in it (not through links),
/// at most [`MAX_ENTRIES`] entries, [`MAX_DEPTH`] deep.
#[cfg(unix)]
fn let_owner_in(path: &Path, depth: usize, left: &mut usize) {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(rest) = left.checked_sub(1) else { return };
    *left = rest;
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(meta) = fs::symlink_metadata(path) else { return };
    if !meta.is_dir() {
        return;
    }
    let mode = meta.permissions().mode();
    if mode & 0o700 != 0o700 {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o700));
    }
    let Ok(entries) = fs::read_dir(path) else { return };
    for entry in entries.flatten() {
        let_owner_in(&entry.path(), depth.saturating_add(1), left);
    }
}

/// Copy a file, or a folder and everything in it (an app), keeping links as links and the
/// permissions, except that the owner can always write to a folder copied (one from a
/// read-only disk image often can't be), so that the copy can be removed; at most
/// [`MAX_ENTRIES`] entries, [`MAX_DEPTH`] deep.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let mut left = MAX_ENTRIES;
    copy_entry(from, to, 0, &mut left)
}

fn copy_entry(from: &Path, to: &Path, depth: usize, left: &mut usize) -> io::Result<()> {
    *left = left.checked_sub(1).ok_or_else(|| io::Error::other("too many files"))?;
    if depth > MAX_DEPTH {
        return Err(io::Error::other("folders too deep"));
    }
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(fs::read_link(from)?, to);
        #[cfg(not(unix))]
        return Err(io::Error::other("a link where none was expected"));
    }
    if meta.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_entry(&entry.path(), &to.join(entry.file_name()), depth.saturating_add(1), left)?;
        }
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut permissions = meta.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        fs::set_permissions(to, permissions)
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

/// What [`replace`] did besides putting the new one in place: whether the one replaced was
/// kept where it was asked to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Replaced {
    pub kept: bool,
}

/// Put `new` (a file or an app folder) in the place of `running`. It is copied beside first, so
/// that the last step is a rename on one disk; a file copied must have the SHA-256 `expected`,
/// and `check` must hold for what is put in place (an app: in its place, after the swap; a
/// file: before it). Whatever fails before the new one is in place leaves `running` as it was
/// and nothing staged beside it; a version that can't be put back is an error that says where
/// it is.
///
/// Once the new one is in place, the update has happened: the one replaced goes to `keep`
/// (copied first for a file, which stays where it is until the rename; moved after for a
/// folder) or is deleted, both as far as they can be, which is logged and never an error
/// ([`Replaced::kept`] says whether it was kept).
pub fn replace(
    running: &Path,
    new: &Path,
    keep: Option<&Path>,
    expected: Option<[u8; 32]>,
    check: &mut dyn FnMut(&Path) -> bool,
) -> io::Result<Replaced> {
    let staged = beside(running, "new")?;
    remove_any(&staged)?;
    let staged_ok = copy_tree(new, &staged).and_then(|()| {
        if staged.is_file() {
            if let Some(expected) = expected
                && mailcraft_net::update_download::sha256(&staged) != Some(expected)
            {
                return Err(io::Error::other("the copy isn't the file that was checked"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
            }
        }
        Ok(())
    });
    // From here until the new one is in place, the copy staged goes with every error.
    let fail = |e: io::Error| {
        let _ = remove_any(&staged);
        e
    };
    staged_ok.map_err(fail)?;
    if fs::symlink_metadata(running).map_err(fail)?.is_dir() {
        let aside = beside(running, "old").map_err(fail)?;
        // What a swap that didn't finish left is the owner's to put right, not to be removed.
        if beside(running, "unfinished").is_ok_and(|marker| fs::symlink_metadata(marker).is_ok()) && fs::symlink_metadata(&aside).is_ok() {
            return Err(fail(io::Error::other(format!("an earlier update didn't finish; the version it replaced is at {}", aside.display()))));
        }
        remove_any(&aside).map_err(fail)?;
        fs::rename(running, &aside).map_err(fail)?;
        if let Err(e) = fs::rename(&staged, running) {
            return Err(put_back(running, &aside, &staged, e));
        }
        if !check(running) {
            // Out of its place again (where it was staged), and the one replaced back.
            if let Err(e) = fs::rename(running, &staged) {
                mark_unfinished(running);
                log::error!("the new app doesn't pass its check in its place and couldn't be taken out of it: {e}");
                return Err(io::Error::other(format!(
                    "the new copy doesn't pass its check in its place and couldn't be taken out of it ({e}); the version that was there is at {}",
                    aside.display()
                )));
            }
            return Err(put_back(running, &aside, &staged, io::Error::other("the new copy doesn't pass its check in its place")));
        }
        // In place: what follows is tidying.
        let kept = match keep {
            Some(keep) => match keep_folder(&aside, keep) {
                Ok(()) => true,
                Err(e) => {
                    log::warn!("the version replaced couldn't be kept for going back: {e}");
                    false
                }
            },
            None => false,
        };
        if let Err(e) = remove_stubborn(&aside) {
            log::warn!("the version replaced, hidden beside the new one, couldn't be removed: {e}");
        }
        Ok(Replaced { kept })
    } else {
        if !check(&staged) {
            return Err(fail(io::Error::other("the new copy doesn't pass its check")));
        }
        let kept = match keep {
            Some(keep) => match remove_stubborn(keep).and_then(|()| fs::copy(running, keep)) {
                Ok(_) => true,
                Err(e) => {
                    let _ = remove_any(keep);
                    log::warn!("the version replaced couldn't be kept for going back: {e}");
                    false
                }
            },
            None => false,
        };
        fs::rename(&staged, running).map_err(fail)?;
        Ok(Replaced { kept })
    }
}

/// The folder `aside` moved (or copied) to `keep`; a copy that didn't finish is removed.
fn keep_folder(aside: &Path, keep: &Path) -> io::Result<()> {
    remove_stubborn(keep)?;
    if fs::rename(aside, keep).is_ok() {
        return Ok(());
    }
    copy_tree(aside, keep).inspect_err(|_| {
        let _ = remove_stubborn(keep);
    })
}

/// The new one isn't in `running` after all: the one moved `aside` goes back, then the copy
/// `staged` is removed, and `cause` is the error. When the one moved aside can't be put back,
/// both are left where they are, the swap is marked unfinished (so that [`clear_leftovers`]
/// keeps them), and the error says where they are.
fn put_back(running: &Path, aside: &Path, staged: &Path, cause: io::Error) -> io::Error {
    match fs::rename(aside, running) {
        Ok(()) => {
            let _ = remove_any(staged);
            cause
        }
        Err(back) => {
            mark_unfinished(running);
            log::error!("the version that was in place couldn't be put back ({back}); it is at {}", aside.display());
            io::Error::other(format!(
                "{cause}; and the version that was in place couldn't be put back ({back}): it is at {}, to be renamed {}; the new one is at {}",
                aside.display(),
                running.display(),
                staged.display()
            ))
        }
    }
}

/// A swap of `running` that didn't finish: `.<name>.unfinished` beside it, so that what is
/// beside it is kept until the owner has put it right. Best effort.
fn mark_unfinished(running: &Path) {
    if let Ok(marker) = beside(running, "unfinished")
        && let Err(e) = fs::write(&marker, b"An update of MailCraft didn't finish: see the log.\n")
    {
        log::warn!("the unfinished update couldn't be marked: {}", e.kind());
    }
}

/// What an update that stopped partway may have left beside `target` (`.<name>.new`,
/// `.<name>.old`), removed at start; best effort, logged. Not while another program holds the
/// update lock (one that is installing, [`on_exit`]): whether it ran. Not either while the old
/// version is there and a swap is marked unfinished or `target` isn't there: that is the
/// owner's copy to put back, which the error of that swap named.
pub fn clear_leftovers(folders: &Folders, target: &Path) -> bool {
    let Ok(lock) = open_lock(folders) else { return false };
    if lock.try_lock().is_err() {
        log::info!("an update is being installed; what is beside MailCraft is left for now");
        return false;
    }
    let (Ok(marker), Ok(new), Ok(old)) = (beside(target, "unfinished"), beside(target, "new"), beside(target, "old")) else {
        return true;
    };
    let there = |path: &Path| fs::symlink_metadata(path).is_ok();
    if there(&old) && (there(&marker) || !there(target)) {
        log::warn!("an earlier update didn't finish: the version that was in place is kept at {}", old.display());
        let _ = lock.unlock();
        return true;
    }
    for (what, path) in [("unfinished", &marker), ("new", &new), ("old", &old)] {
        if !there(path) {
            continue;
        }
        match remove_stubborn(path) {
            Ok(()) => log::info!("removed what an earlier update left beside MailCraft (.{what})"),
            Err(e) => log::warn!("what an earlier update left beside MailCraft (.{what}) couldn't be removed: {}", e.kind()),
        }
    }
    let _ = lock.unlock();
    true
}

/// A Mac copy that doesn't install updates itself ([`crate::install::MAC_SELF_INSTALL`] off):
/// what earlier versions that did left in the data folder (disk images downloaded, the place
/// one was attached, the copies handed over, the version kept for going back), removed at start;
/// best effort, and not while another program holds the update lock.
pub fn clear_mac_data(folders: &Folders) {
    let Ok(lock) = open_lock(folders) else { return };
    if lock.try_lock().is_err() {
        return;
    }
    if let Ok(entries) = fs::read_dir(folders.updates()) {
        for entry in entries.flatten().take(1_000) {
            let path = entry.path();
            if path.extension() == Some(OsStr::new("dmg")) {
                let _ = remove_any(&path);
            }
        }
    }
    for path in [folders.mount(), folders.handed(), folders.previous()] {
        if fs::symlink_metadata(&path).is_ok()
            && let Err(e) = remove_stubborn(&path)
        {
            log::debug!("what an earlier update left in the data folder couldn't be removed: {}", e.kind());
        }
    }
    let _ = lock.unlock();
}

/// What this copy is installed as: the AppImage file, the `.app` folder, or `mailcraft.exe`.
pub fn target(install: Install) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    match install {
        Install::Msi => Some(exe),
        // `<name>.app/Contents/MacOS/<program>`
        Install::MacApp => exe.parent()?.parent()?.parent().map(Path::to_path_buf),
        Install::AppImage => std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_absolute()),
        Install::NoticeOnly => None,
    }
}

/// Once the window has closed and the mailbox and the settings are saved: do what the window
/// says ([`OnExit`]). `verified`: what was downloaded and checked in this run; `mailbox`: the
/// mailbox file, whose copy from before a newer format is put back when going back.
pub fn on_exit(folders: &Folders, install: Install, what: OnExit, verified: Option<Verified>, mailbox: Option<&Path>) -> Result<(), String> {
    let running = env!("CARGO_PKG_VERSION");
    let target = target(install).ok_or("this copy doesn't update itself")?;
    // Held from here until this program has gone: another start leaves what is beside the app
    // alone meanwhile ([`clear_leftovers`]), and a new version waits for it.
    if !hold_lock(folders) {
        log::warn!("the update lock couldn't be taken; another start may tidy beside MailCraft meanwhile");
    }
    match what {
        OnExit::Install { version, restart } => {
            let verified = verified.filter(|v| v.version == version).ok_or("nothing downloaded and checked for that version")?;
            if mailcraft_net::update_download::sha256(&verified.path) != Some(verified.sha256) {
                let _ = fs::remove_file(&verified.path);
                return Err("the download changed since it was checked; it is deleted".into());
            }
            let to = version.to_string();
            write_record(folders, &Record { from: running.into(), to: to.clone(), back: false, settled: false }).map_err(|e| e.kind().to_string())?;
            let started = match install {
                Install::Msi => {
                    let job =
                        Job { msi: verified.path, sha256: verified.sha256, from: running.into(), to, back: false, start: restart.then_some(target) };
                    return windows_helper(folders, &job);
                }
                Install::AppImage => {
                    install_appimage(folders, &target, &verified.path, verified.sha256, running)?;
                    start_program(&target)
                }
                Install::MacApp => {
                    install_mac_app(folders, &target, &verified.path, verified.sha256, running)?;
                    open_app(&target)
                }
                Install::NoticeOnly => return Err("this copy doesn't update itself".into()),
            };
            if restart {
                relaunch(folders, &started);
            }
            Ok(())
        }
        OnExit::GoBack { to } => {
            let previous = folders.previous();
            let kept = read_kept(&previous).filter(|k| k.version == to).ok_or("the version to go back to isn't kept")?;
            if let Some(mailbox) = mailbox {
                match mailcraft_engine::store_before::put_back(mailbox, running) {
                    Ok(true) => log::info!("the mailbox file from before MailCraft {running} changed its format is put back"),
                    Ok(false) => {}
                    Err(e) => return Err(format!("the mailbox file from before couldn't be put back ({})", e.kind())),
                }
            }
            write_record(folders, &Record { from: running.into(), to: to.clone(), back: true, settled: false }).map_err(|e| e.kind().to_string())?;
            let file = previous.join(&kept.file);
            let started = match install {
                Install::Msi => {
                    let sha256 = kept.sha256.ok_or("the kept installer has no checksum")?;
                    let job = Job { msi: file, sha256, from: running.into(), to, back: true, start: Some(target) };
                    return windows_helper(folders, &job);
                }
                Install::AppImage => {
                    replace(&target, &file, None, Some(kept.sha256.ok_or("the kept AppImage has no checksum")?), &mut |_| true)
                        .map_err(|e| e.to_string())?;
                    start_program(&target)
                }
                Install::MacApp => {
                    // The kept app is a folder with no signed SHA-256 to check it by: the disk
                    // image it came from was deleted once it was installed, and a checksum of
                    // our own, kept beside it in the data folder, would be no better than the
                    // app itself against whoever can change that folder. It is copied into
                    // place by this program (`replace`); no other program is handed it.
                    replace(&target, &file, None, None, &mut |_| true).map_err(|e| e.to_string())?;
                    open_app(&target)
                }
                Install::NoticeOnly => return Err("this copy doesn't update itself".into()),
            };
            let _ = remove_stubborn(&previous);
            relaunch(folders, &started);
            Ok(())
        }
    }
}

/// Start the new version, which waits for this one to have gone.
fn relaunch(folders: &Folders, run: &Run) {
    if !hold_lock(folders) {
        log::warn!("the new version may start before this one has gone");
    }
    if let Err(e) = run.spawn_detached() {
        log::error!("the new version couldn't be started: {}", e.kind());
    }
}

/// AppImage: the new file in place of the running one, the old one kept.
fn install_appimage(folders: &Folders, image: &Path, new: &Path, sha256: [u8; 32], running: &str) -> Result<(), String> {
    let previous = folders.previous();
    let _ = remove_stubborn(&previous);
    fs::create_dir_all(&previous).map_err(|e| e.to_string())?;
    let name = image.file_name().and_then(OsStr::to_str).filter(|n| plain_name(n)).unwrap_or("MailCraft.AppImage").to_string();
    let keep = previous.join(&name);
    let replaced = replace(image, new, Some(&keep), Some(sha256), &mut |_| true).map_err(|e| format!("the AppImage couldn't be replaced: {e}"))?;
    // Only a version that was kept is offered to go back to.
    if replaced.kept {
        let kept_sum = mailcraft_net::update_download::sha256(&keep);
        let _ = write_kept(&previous, &Kept { version: running.into(), file: name, sha256: kept_sum });
    }
    let _ = fs::remove_file(new);
    Ok(())
}

/// macOS: the app in the disk image in place of the running one, the old one kept.
fn install_mac_app(folders: &Folders, app: &Path, dmg: &Path, sha256: [u8; 32], running: &str) -> Result<(), String> {
    install_mac_app_with(folders, app, dmg, sha256, running, &mut |run| run.wait())
}

/// [`install_mac_app`] with `run` running `hdiutil` and `codesign`.
fn install_mac_app_with(
    folders: &Folders,
    app: &Path,
    dmg: &Path,
    sha256: [u8; 32],
    running: &str,
    run: &mut dyn FnMut(&Run) -> Option<i32>,
) -> Result<(), String> {
    // Attached from a copy made for it and checked once more ([`Handed`]), held open until the
    // image is detached. The app copied out of it is what that copy holds: an image attached
    // read-only can't be changed while it is attached.
    let handed = Handed::new(dmg, sha256, &folders.handed()).map_err(|e| format!("the disk image wasn't attached: {e}"))?;
    let mount = folders.mount();
    let _ = remove_any(&mount);
    fs::create_dir_all(&mount).map_err(|e| e.to_string())?;
    if run(&hdiutil_attach(&handed.path, &mount)) != Some(0) {
        let _ = fs::remove_dir(&mount);
        return Err("the disk image couldn't be attached".into());
    }
    let result = mac_swap(folders, app, &mount, running, run);
    let _ = run(&hdiutil_detach(&mount));
    drop(handed);
    let _ = fs::remove_dir(&mount);
    if result.is_ok() {
        let _ = fs::remove_file(dmg);
    }
    result
}

/// The app in the disk image attached at `mount` in place of `app`, with `run` running
/// `codesign`. A signed MailCraft is replaced only by an app whose signature holds, checked
/// before the swap and again in its place after it; one that doesn't hold there is swapped back.
fn mac_swap(folders: &Folders, app: &Path, mount: &Path, running: &str, run: &mut dyn FnMut(&Run) -> Option<i32>) -> Result<(), String> {
    let new = app_in(mount).ok_or("the disk image has no app")?;
    // (Any valid signature, for now: the project has no Apple Team ID yet. Once it has, the new
    // app's signature is to be required to be the project's: `-R '=anchor apple generic and
    // certificate leaf[subject.OU] = "<Team ID>"'`; see `install::MAC_SELF_INSTALL`.)
    let signed = run(&codesign_verify(app)) == Some(0);
    if signed && run(&codesign_verify(&new)) != Some(0) {
        return Err("the new app's signature doesn't hold".into());
    }
    let previous = folders.previous();
    let _ = remove_stubborn(&previous);
    fs::create_dir_all(&previous).map_err(|e| e.to_string())?;
    let name = app.file_name().and_then(OsStr::to_str).filter(|n| plain_name(n)).unwrap_or("MailCraft.app").to_string();
    let mut holds = |placed: &Path| !signed || run(&codesign_verify(placed)) == Some(0);
    let replaced = replace(app, &new, Some(&previous.join(&name)), None, &mut holds).map_err(|e| format!("the app couldn't be replaced: {e}"))?;
    // Only a version that was kept is offered to go back to.
    if replaced.kept {
        let _ = write_kept(&previous, &Kept { version: running.into(), file: name, sha256: None });
    }
    Ok(())
}

/// The app in an attached disk image: a folder `<name>.app` at its top.
pub fn app_in(mount: &Path) -> Option<PathBuf> {
    fs::read_dir(mount)
        .ok()?
        .flatten()
        .take(100)
        .map(|e| e.path())
        .find(|p| p.extension() == Some(OsStr::new("app")) && fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()) && p.join("Contents").is_dir())
}

/// Windows: a copy of this program in the data folder installs the `.msi` once this one has
/// gone, holding the lock it waits on until then.
fn windows_helper(folders: &Folders, job: &Job) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = folders.helper().join("program");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let copy = dir.join("mailcraft-update.exe");
    fs::copy(&exe, &copy).map_err(|e| format!("the installer's helper couldn't be made: {}", e.kind()))?;
    if !hold_lock(folders) {
        log::warn!("the installer may start before MailCraft has gone");
    }
    Run { program: copy, args: job.args() }.spawn_detached().map_err(|e| format!("the installer couldn't be started: {}", e.kind()))
}

#[cfg(test)]
#[path = "self_update_tests.rs"]
mod self_update_tests;
