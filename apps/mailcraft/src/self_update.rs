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
//!   is signed, the new one must pass `codesign --verify` first.
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

/// Take the lock a new version waits on, for as long as this program runs.
pub fn hold_lock(folders: &Folders) -> bool {
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
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
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

/// Copy a file, or a folder and everything in it (an app), keeping links as links and the
/// permissions; at most [`MAX_ENTRIES`] entries, [`MAX_DEPTH`] deep.
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
        fs::set_permissions(to, meta.permissions())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

/// Put `new` (a file or an app folder) in the place of `running`. It is copied beside first, so
/// that the last step is a rename on one disk; a file copied must have the SHA-256 `expected`.
/// The one replaced goes to `keep` (copied first for a file, which stays where it is until the
/// rename; moved after for a folder), or is deleted. Whatever fails leaves `running` as it was.
pub fn replace(running: &Path, new: &Path, keep: Option<&Path>, expected: Option<[u8; 32]>) -> io::Result<()> {
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
    if let Err(e) = staged_ok {
        let _ = remove_any(&staged);
        return Err(e);
    }
    if fs::symlink_metadata(running)?.is_dir() {
        let aside = beside(running, "old")?;
        remove_any(&aside)?;
        if let Err(e) = fs::rename(running, &aside) {
            let _ = remove_any(&staged);
            return Err(e);
        }
        if let Err(e) = fs::rename(&staged, running) {
            let _ = fs::rename(&aside, running);
            let _ = remove_any(&staged);
            return Err(e);
        }
        if let Some(keep) = keep {
            remove_any(keep)?;
            if fs::rename(&aside, keep).is_err() {
                copy_tree(&aside, keep)?;
            }
        }
        remove_any(&aside)
    } else {
        if let Some(keep) = keep {
            remove_any(keep)?;
            if let Err(e) = fs::copy(running, keep) {
                let _ = remove_any(&staged);
                return Err(e);
            }
        }
        fs::rename(&staged, running).inspect_err(|_| {
            let _ = remove_any(&staged);
        })
    }
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
                    replace(&target, &file, None, Some(kept.sha256.ok_or("the kept AppImage has no checksum")?)).map_err(|e| e.to_string())?;
                    start_program(&target)
                }
                Install::MacApp => {
                    // The kept app is a folder with no signed SHA-256 to check it by: the disk
                    // image it came from was deleted once it was installed, and a checksum of
                    // our own, kept beside it in the data folder, would be no better than the
                    // app itself against whoever can change that folder. It is copied into
                    // place by this program (`replace`); no other program is handed it.
                    replace(&target, &file, None, None).map_err(|e| e.to_string())?;
                    open_app(&target)
                }
                Install::NoticeOnly => return Err("this copy doesn't update itself".into()),
            };
            let _ = fs::remove_dir_all(&previous);
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
    let _ = fs::remove_dir_all(&previous);
    fs::create_dir_all(&previous).map_err(|e| e.to_string())?;
    let name = image.file_name().and_then(OsStr::to_str).filter(|n| plain_name(n)).unwrap_or("MailCraft.AppImage").to_string();
    let keep = previous.join(&name);
    replace(image, new, Some(&keep), Some(sha256)).map_err(|e| format!("the AppImage couldn't be replaced: {e}"))?;
    let kept_sum = mailcraft_net::update_download::sha256(&keep);
    let _ = write_kept(&previous, &Kept { version: running.into(), file: name, sha256: kept_sum });
    let _ = fs::remove_file(new);
    Ok(())
}

/// macOS: the app in the disk image in place of the running one, the old one kept.
fn install_mac_app(folders: &Folders, app: &Path, dmg: &Path, sha256: [u8; 32], running: &str) -> Result<(), String> {
    install_mac_app_with(folders, app, dmg, sha256, running, &mut |run| run.wait())
}

/// [`install_mac_app`] with `run` running `hdiutil`.
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
    let result = mac_swap(folders, app, &mount, running);
    let _ = run(&hdiutil_detach(&mount));
    drop(handed);
    let _ = fs::remove_dir(&mount);
    if result.is_ok() {
        let _ = fs::remove_file(dmg);
    }
    result
}

fn mac_swap(folders: &Folders, app: &Path, mount: &Path, running: &str) -> Result<(), String> {
    let new = app_in(mount).ok_or("the disk image has no app")?;
    // A signed MailCraft is replaced only by one whose signature holds. (Any valid signature,
    // for now: the project has no Apple Team ID yet. Once it has, the new app's signature is to
    // be required to be the project's: `-R '=anchor apple generic and certificate leaf[subject.OU]
    // = "<Team ID>"'`.)
    if codesign_verify(app).wait() == Some(0) && codesign_verify(&new).wait() != Some(0) {
        return Err("the new app's signature doesn't hold".into());
    }
    let previous = folders.previous();
    let _ = fs::remove_dir_all(&previous);
    fs::create_dir_all(&previous).map_err(|e| e.to_string())?;
    let name = app.file_name().and_then(OsStr::to_str).filter(|n| plain_name(n)).unwrap_or("MailCraft.app").to_string();
    replace(app, &new, Some(&previous.join(&name)), None).map_err(|e| format!("the app couldn't be replaced: {e}"))?;
    let _ = write_kept(&previous, &Kept { version: running.into(), file: name, sha256: None });
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
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mailcraft-self-update-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sum(bytes: &[u8]) -> [u8; 32] {
        ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().try_into().unwrap()
    }

    #[test]
    fn the_installer_is_run_with_fixed_arguments() {
        let run = msiexec(Path::new(r"C:\Users\a\AppData\Roaming\MailCraft\updates\mailcraft-0.5.0-windows-x64.msi"), Path::new(r"C:\x\install.log"));
        assert!(run.program.ends_with("msiexec.exe"));
        assert!(run.program.to_string_lossy().contains("System32"));
        let args: Vec<String> = run.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            [
                r"/i",
                r"C:\Users\a\AppData\Roaming\MailCraft\updates\mailcraft-0.5.0-windows-x64.msi",
                "/passive",
                "/norestart",
                "/log",
                r"C:\x\install.log"
            ]
        );
        let attach = hdiutil_attach(Path::new("/d/m.dmg"), Path::new("/d/mount"));
        assert_eq!(attach.program, Path::new("/usr/bin/hdiutil"));
        assert_eq!(attach.args, ["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint", "/d/mount", "/d/m.dmg"]);
        assert_eq!(hdiutil_detach(Path::new("/d/mount")).args, ["detach", "/d/mount", "-force"]);
        assert_eq!(codesign_verify(Path::new("/A/MailCraft.app")).args, ["--verify", "--deep", "--strict", "/A/MailCraft.app"]);
        assert_eq!(open_app(Path::new("/A/MailCraft.app")).args, ["-n", "/A/MailCraft.app", "--args", AFTER_UPDATE]);
        assert_eq!(start_program(Path::new("/h/m.AppImage")).args, [AFTER_UPDATE]);
    }

    #[test]
    fn the_helpers_job_goes_through_its_arguments_and_nothing_else_does() {
        let base = if cfg!(windows) { r"C:\data\updates" } else { "/data/updates" };
        let job = Job {
            msi: Path::new(base).join("mailcraft-0.5.0-windows-x64.msi"),
            sha256: sum(b"x"),
            from: "0.4.3".into(),
            to: "0.5.0".into(),
            back: false,
            start: Some(Path::new(base).join("mailcraft.exe")),
        };
        assert_eq!(Job::parse(&job.args()), Some(job.clone()));
        let back = Job { back: true, start: None, ..job.clone() };
        assert_eq!(Job::parse(&back.args()), Some(back));
        let mut bad = job.args();
        bad[2] = "zz".into();
        assert_eq!(Job::parse(&bad), None);
        let mut bad = job.args();
        bad[1] = "relative.msi".into();
        assert_eq!(Job::parse(&bad), None);
        let mut bad = job.args();
        bad[1] = Path::new(base).join("evil.exe").into_os_string();
        assert_eq!(Job::parse(&bad), None);
        let mut bad = job.args();
        bad[4] = "0.5.0\r\n".into();
        assert_eq!(Job::parse(&bad), None);
        assert_eq!(Job::parse(&[]), None);
        assert_eq!(Job::parse(&["--sample".into()]), None);
    }

    #[test]
    fn a_failed_start_twice_running_offers_going_back() {
        let dir = scratch("starts");
        let folders = Folders::new(dir.clone());
        // Nothing installed: nothing is said, nothing written.
        assert_eq!(at_start(&folders, "0.5.0"), UpdateStart::default());
        assert!(!folders.starts().exists());
        // 0.4.3 installed 0.5.0 and kept itself.
        write_record(&folders, &Record { from: "0.4.3".into(), to: "0.5.0".into(), back: false, settled: false }).unwrap();
        fs::create_dir_all(folders.previous()).unwrap();
        write_kept(&folders.previous(), &Kept { version: "0.4.3".into(), file: "m.AppImage".into(), sha256: None }).unwrap();
        // Two starts that didn't last; the third offers.
        assert_eq!(at_start(&folders, "0.5.0"), UpdateStart::default());
        assert_eq!(at_start(&folders, "0.5.0"), UpdateStart::default());
        assert_eq!(at_start(&folders, "0.5.0").didnt_start, Some(("0.5.0".into(), "0.4.3".into())));
        // A run that lasted: the start worked, and nothing more is counted.
        settle(&folders, "0.5.0");
        assert!(!folders.starts().exists());
        assert_eq!(at_start(&folders, "0.5.0"), UpdateStart::default());
        assert!(!folders.starts().exists());
        // Without a kept version to go back to, nothing is offered.
        write_record(&folders, &Record { from: "0.4.3".into(), to: "0.5.0".into(), back: false, settled: false }).unwrap();
        fs::remove_dir_all(folders.previous()).unwrap();
        for _ in 0..4 {
            assert_eq!(at_start(&folders, "0.5.0").didnt_start, None);
        }
        // Gone back: the version gone back to isn't offered going back again.
        write_record(&folders, &Record { from: "0.5.0".into(), to: "0.4.3".into(), back: true, settled: false }).unwrap();
        for _ in 0..4 {
            assert_eq!(at_start(&folders, "0.4.3").didnt_start, None);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_update_that_didnt_happen_is_said_once() {
        let dir = scratch("declined");
        let folders = Folders::new(dir.clone());
        write_record(&folders, &Record { from: "0.4.3".into(), to: "0.5.0".into(), back: false, settled: false }).unwrap();
        // Windows' prompt answered no: 0.4.3 starts again.
        assert_eq!(at_start(&folders, "0.4.3").not_installed.as_deref(), Some("0.5.0"));
        assert_eq!(at_start(&folders, "0.4.3"), UpdateStart::default());
        write_record(&folders, &Record { from: "0.5.0".into(), to: "0.4.3".into(), back: true, settled: false }).unwrap();
        assert_eq!(at_start(&folders, "0.5.0").not_gone_back.as_deref(), Some("0.4.3"));
        // A hostile or broken record is none.
        fs::write(folders.record(), b"{\"from\": \"0.4.3\", \"to\": \"../../x\"}").unwrap();
        assert_eq!(at_start(&folders, "0.4.3"), UpdateStart::default());
        fs::write(folders.record(), vec![b'['; 100_000]).unwrap();
        assert_eq!(at_start(&folders, "0.4.3"), UpdateStart::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_installed_msi_is_kept_one_back() {
        let dir = scratch("msi");
        let folders = Folders::new(dir.join("data"));
        let download = |name: &str| {
            let updates = dir.join("data").join("updates");
            fs::create_dir_all(&updates).unwrap();
            let path = updates.join(name);
            fs::write(&path, name.as_bytes()).unwrap();
            path
        };
        let job =
            |msi: PathBuf, from: &str, to: &str, back: bool| Job { sha256: sum(b"x"), msi, from: from.into(), to: to.into(), back, start: None };
        // The first update: there was no `.msi` of the version it replaced.
        keep_installed_msi(&folders, &job(download("m-0.5.0.msi"), "0.4.3", "0.5.0", false)).unwrap();
        assert_eq!(read_kept(&folders.this_version()).unwrap().version, "0.5.0");
        assert!(folders.this_version().join("m-0.5.0.msi").is_file());
        assert!(read_kept(&folders.previous()).is_none());
        // The next: 0.5.0's is kept back.
        keep_installed_msi(&folders, &job(download("m-0.6.0.msi"), "0.5.0", "0.6.0", false)).unwrap();
        assert_eq!(read_kept(&folders.this_version()).unwrap().version, "0.6.0");
        let kept = read_kept(&folders.previous()).unwrap();
        assert_eq!((kept.version.as_str(), kept.file.as_str()), ("0.5.0", "m-0.5.0.msi"));
        assert!(folders.previous().join("m-0.5.0.msi").is_file());
        // Going back: 0.5.0's is the one the running version came from, and none is kept back.
        let back = folders.previous().join("m-0.5.0.msi");
        keep_installed_msi(&folders, &job(back, "0.6.0", "0.5.0", true)).unwrap();
        assert_eq!(read_kept(&folders.this_version()).unwrap().version, "0.5.0");
        assert!(folders.this_version().join("m-0.5.0.msi").is_file());
        assert!(!folders.previous().exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// What is left in the folders of the hand-overs.
    fn left_handed(folders: &Folders) -> usize {
        fs::read_dir(folders.handed()).map(|d| d.count()).unwrap_or(0)
    }

    #[test]
    fn the_installer_is_handed_a_checked_copy_and_never_the_download() {
        let dir = scratch("handed-msi");
        let folders = Folders::new(dir.join("data"));
        let msi = folders.updates().join("mailcraft-0.5.0-windows-x64.msi");
        fs::create_dir_all(folders.updates()).unwrap();
        fs::write(&msi, b"the checked installer").unwrap();
        let job = Job { msi: msi.clone(), sha256: sum(b"the checked installer"), from: "0.4.3".into(), to: "0.5.0".into(), back: false, start: None };
        let mut handed = Vec::new();
        let installed = install_msi(&job, &folders, &mut |run| {
            let given = PathBuf::from(&run.args[1]);
            assert_ne!(given, msi, "the installer is given the copy");
            assert!(given.starts_with(folders.handed()), "{}", given.display());
            assert_eq!(given.file_name(), msi.file_name(), "and by the same name");
            // The download changed while the installer runs doesn't change what it reads.
            fs::write(&msi, b"swapped").unwrap();
            assert_eq!(fs::read(&given).unwrap(), b"the checked installer");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                assert_eq!(fs::metadata(given.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
            }
            handed.push(given);
            Some(0)
        });
        assert!(installed);
        assert_eq!(handed.len(), 1);
        assert!(!handed[0].exists() && left_handed(&folders) == 0, "the copy and its folder are gone");
        // A file that isn't the one checked (here, the one swapped above) isn't run at all.
        let installed = install_msi(&job, &folders, &mut |_| panic!("the installer was run on a file that wasn't checked"));
        assert!(!installed);
        assert_eq!(left_handed(&folders), 0);
        // Windows Installer's answer is believed only for what it says installed.
        fs::write(&msi, b"the checked installer").unwrap();
        assert!(!install_msi(&job, &folders, &mut |_| Some(1603)));
        assert!(install_msi(&job, &folders, &mut |_| Some(3010)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_signed_digest_is_trusted_even_when_the_download_changes_afterwards() {
        let dir = scratch("signed-digest");
        let folders = Folders::new(dir.join("data"));
        let release =
            mailcraft_model::update::Release { version: Version::parse("9.9.9").unwrap(), page: String::new(), security: false, assets: Vec::new() };
        // The release's signed list gives this digest; the file is swapped once it is in place,
        // before anything reads it back.
        let fetch = |_: &mailcraft_model::update::Release, name: &str, d: &Path| {
            fs::create_dir_all(d).unwrap();
            let path = d.join(name);
            fs::write(&path, b"swapped after the download").unwrap();
            Ok(mailcraft_net::update_download::Downloaded { path, sha256: sum(b"the signed installer") })
        };
        let verified = crate::updater::download(&release, Install::Msi, &folders.updates(), fetch).unwrap();
        assert_eq!(verified.sha256, sum(b"the signed installer"));
        assert_eq!(verified.version, release.version);
        // So what is installed later is checked against what the release signed, and refused.
        let job = Job { msi: verified.path, sha256: verified.sha256, from: "0.4.3".into(), to: "9.9.9".into(), back: false, start: None };
        assert!(!install_msi(&job, &folders, &mut |_| panic!("the installer was run on a file that wasn't signed")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_disk_image_is_attached_from_a_checked_copy() {
        let dir = scratch("handed-dmg");
        let folders = Folders::new(dir.join("data"));
        let dmg = folders.updates().join("mailcraft-0.5.0-macos-universal.dmg");
        fs::create_dir_all(folders.updates()).unwrap();
        fs::write(&dmg, b"the checked image").unwrap();
        let app = dir.join("Applications").join("MailCraft.app");
        let mut asked = Vec::new();
        let got = install_mac_app_with(&folders, &app, &dmg, sum(b"the checked image"), "0.4.3", &mut |run| {
            let given = PathBuf::from(run.args.last().unwrap());
            assert_eq!(run.args[0], "attach");
            assert_ne!(given, dmg);
            assert!(given.starts_with(folders.handed()), "{}", given.display());
            assert_eq!(fs::read(&given).unwrap(), b"the checked image");
            asked.push(given);
            // (Not attached here: nothing more is done.)
            Some(1)
        });
        assert_eq!(got, Err("the disk image couldn't be attached".to_string()));
        assert_eq!(asked.len(), 1);
        assert_eq!(left_handed(&folders), 0);
        // One that changed since it was checked: `hdiutil` is never asked.
        let got = install_mac_app_with(&folders, &app, &dmg, sum(b"another image"), "0.4.3", &mut |_| panic!("hdiutil was asked"));
        assert!(got.is_err());
        assert_eq!(left_handed(&folders), 0);
        assert!(dmg.is_file(), "the download is left for the next try");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_lock_is_waited_for() {
        let dir = scratch("lock");
        let folders = Folders::new(dir.clone());
        assert!(wait_for_lock(&folders, Duration::from_millis(10)), "nobody holds it");
        let held = open_lock(&folders).unwrap();
        held.try_lock().unwrap();
        // (Another open file of the same process: a lock of its own, as another program's is.)
        assert!(!wait_for_lock(&folders, Duration::from_millis(200)));
        drop(held);
        assert!(wait_for_lock(&folders, Duration::from_millis(200)));
        let _ = fs::remove_dir_all(&dir);
    }

    // Unix files and links: the macOS app and the AppImage.
    #[test]
    #[cfg(unix)]
    fn an_appimage_is_replaced_and_the_old_one_kept() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = scratch("appimage");
        let image = dir.join("Apps").join("mailcraft-0.4.3-linux-x86_64.AppImage");
        fs::create_dir_all(image.parent().unwrap()).unwrap();
        fs::write(&image, b"old").unwrap();
        let new = dir.join("data").join("updates").join("mailcraft-0.5.0-linux-x86_64.AppImage");
        fs::create_dir_all(new.parent().unwrap()).unwrap();
        fs::write(&new, b"new").unwrap();
        let folders = Folders::new(dir.join("data"));
        // A copy that isn't the file checked is refused, and nothing changes.
        assert!(install_appimage(&folders, &image, &new, sum(b"other"), "0.4.3").is_err());
        assert_eq!(fs::read(&image).unwrap(), b"old");
        assert!(!beside(&image, "new").unwrap().exists());
        install_appimage(&folders, &image, &new, sum(b"new"), "0.4.3").unwrap();
        assert_eq!(fs::read(&image).unwrap(), b"new");
        assert_eq!(fs::metadata(&image).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(!new.exists(), "the download is gone");
        let kept = read_kept(&folders.previous()).unwrap();
        assert_eq!(kept.version, "0.4.3");
        assert_eq!(kept.sha256, Some(sum(b"old")));
        assert_eq!(fs::read(folders.previous().join(&kept.file)).unwrap(), b"old");
        // Going back: the kept one in place again.
        replace(&image, &folders.previous().join(&kept.file), None, kept.sha256).unwrap();
        assert_eq!(fs::read(&image).unwrap(), b"old");
        assert_eq!(fs::read_dir(image.parent().unwrap()).unwrap().count(), 1, "nothing left beside it");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn a_mac_app_is_swapped_by_renaming_and_the_old_one_kept() {
        let dir = scratch("macapp");
        let make = |root: &Path, text: &str| {
            let macos = root.join("Contents").join("MacOS");
            fs::create_dir_all(&macos).unwrap();
            fs::write(macos.join("mailcraft"), text).unwrap();
            fs::create_dir_all(root.join("Contents").join("Resources")).unwrap();
            std::os::unix::fs::symlink("../MacOS/mailcraft", root.join("Contents").join("Resources").join("link")).unwrap();
        };
        let app = dir.join("Applications").join("MailCraft.app");
        make(&app, "old");
        let mount = dir.join("mount");
        make(&mount.join("MailCraft.app"), "new");
        fs::create_dir_all(mount.join(".background")).unwrap();
        assert_eq!(app_in(&mount), Some(mount.join("MailCraft.app")));
        assert_eq!(app_in(&dir.join("nothing")), None);
        let folders = Folders::new(dir.join("data"));
        // (Unsigned here: `codesign` is asked about neither, or isn't there.)
        mac_swap(&folders, &app, &mount, "0.4.3").unwrap();
        assert_eq!(fs::read(app.join("Contents/MacOS/mailcraft")).unwrap(), b"new");
        assert_eq!(fs::read_link(app.join("Contents/Resources/link")).unwrap(), Path::new("../MacOS/mailcraft"));
        let kept = read_kept(&folders.previous()).unwrap();
        assert_eq!((kept.version.as_str(), kept.file.as_str()), ("0.4.3", "MailCraft.app"));
        assert_eq!(fs::read(folders.previous().join("MailCraft.app/Contents/MacOS/mailcraft")).unwrap(), b"old");
        // The disk image is left as it was.
        assert_eq!(fs::read(mount.join("MailCraft.app/Contents/MacOS/mailcraft")).unwrap(), b"new");
        // Going back.
        replace(&app, &folders.previous().join("MailCraft.app"), None, None).unwrap();
        assert_eq!(fs::read(app.join("Contents/MacOS/mailcraft")).unwrap(), b"old");
        assert_eq!(fs::read_dir(app.parent().unwrap()).unwrap().count(), 1, "nothing left beside it");
        // What can't be put in place leaves the running app as it was.
        let missing = dir.join("no-such.app");
        assert!(replace(&app, &missing, None, None).is_err());
        assert_eq!(fs::read(app.join("Contents/MacOS/mailcraft")).unwrap(), b"old");
        let _ = fs::remove_dir_all(&dir);
    }
}
