// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of `self_update.rs`.

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
    let job = |msi: PathBuf, from: &str, to: &str, back: bool| Job { sha256: sum(b"x"), msi, from: from.into(), to: to.into(), back, start: None };
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
    replace(&image, &folders.previous().join(&kept.file), None, kept.sha256, &mut |_| true).unwrap();
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
    mac_swap(&folders, &app, &mount, "0.4.3", &mut |_| None).unwrap();
    assert_eq!(fs::read(app.join("Contents/MacOS/mailcraft")).unwrap(), b"new");
    assert_eq!(fs::read_link(app.join("Contents/Resources/link")).unwrap(), Path::new("../MacOS/mailcraft"));
    let kept = read_kept(&folders.previous()).unwrap();
    assert_eq!((kept.version.as_str(), kept.file.as_str()), ("0.4.3", "MailCraft.app"));
    assert_eq!(fs::read(folders.previous().join("MailCraft.app/Contents/MacOS/mailcraft")).unwrap(), b"old");
    // The disk image is left as it was.
    assert_eq!(fs::read(mount.join("MailCraft.app/Contents/MacOS/mailcraft")).unwrap(), b"new");
    // Going back.
    replace(&app, &folders.previous().join("MailCraft.app"), None, None, &mut |_| true).unwrap();
    assert_eq!(fs::read(app.join("Contents/MacOS/mailcraft")).unwrap(), b"old");
    assert_eq!(fs::read_dir(app.parent().unwrap()).unwrap().count(), 1, "nothing left beside it");
    // What can't be put in place leaves the running app as it was.
    let missing = dir.join("no-such.app");
    assert!(replace(&app, &missing, None, None, &mut |_| true).is_err());
    assert_eq!(fs::read(app.join("Contents/MacOS/mailcraft")).unwrap(), b"old");
    let _ = fs::remove_dir_all(&dir);
}

/// A folder of mode `mode`.
#[cfg(unix)]
fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// Whether a folder the owner can't write to is written to all the same (root): the tests of
/// what can't be written to are then skipped.
#[cfg(unix)]
fn writes_anywhere(dir: &Path) -> bool {
    let locked = dir.join("locked-probe");
    fs::create_dir_all(&locked).unwrap();
    chmod(&locked, 0o555);
    let wrote = fs::write(locked.join("x"), b"x").is_ok();
    chmod(&locked, 0o755);
    let _ = fs::remove_dir_all(&locked);
    wrote
}

/// An app folder `root` with `text` in its program.
#[cfg(unix)]
fn make_app(root: &Path, text: &str) {
    let macos = root.join("Contents").join("MacOS");
    fs::create_dir_all(&macos).unwrap();
    fs::write(macos.join("mailcraft"), text).unwrap();
}

/// The names in `dir` that start with a dot.
#[cfg(unix)]
fn hidden_in(dir: &Path) -> Vec<String> {
    fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with('.')).collect()
}

// #497: a place to keep the old one that can't be written to doesn't make an update that is in
// place a failure, and leaves nothing hidden beside the app.
#[test]
#[cfg(unix)]
fn a_keep_that_cant_be_written_still_replaces() {
    let dir = scratch("keep-locked");
    if writes_anywhere(&dir) {
        return;
    }
    let apps = dir.join("Applications");
    let app = apps.join("MailCraft.app");
    make_app(&app, "old");
    let new = dir.join("new").join("MailCraft.app");
    make_app(&new, "new");
    let locked = dir.join("previous");
    fs::create_dir_all(&locked).unwrap();
    chmod(&locked, 0o555);
    let got = replace(&app, &new, Some(&locked.join("MailCraft.app")), None, &mut |_| true);
    chmod(&locked, 0o755);
    assert!(got.is_ok(), "{got:?}");
    assert_eq!(got.unwrap(), Replaced { kept: false });
    assert_eq!(fs::read(app.join("Contents/MacOS/mailcraft")).unwrap(), b"new");
    assert_eq!(hidden_in(&apps), Vec::<String>::new());
    // Where it can be kept, it is.
    let keep = dir.join("kept").join("MailCraft.app");
    fs::create_dir_all(keep.parent().unwrap()).unwrap();
    make_app(&new, "newer");
    assert_eq!(replace(&app, &new, Some(&keep), None, &mut |_| true).unwrap(), Replaced { kept: true });
    assert_eq!(fs::read(keep.join("Contents/MacOS/mailcraft")).unwrap(), b"new");
    assert_eq!(hidden_in(&apps), Vec::<String>::new());
    let _ = fs::remove_dir_all(&dir);
}

// #497: what an update that stopped partway left beside the app is removed at start, even with
// folders the owner can't write to (as 0.8.1 copied them from the disk image).
#[test]
#[cfg(unix)]
fn leftovers_beside_the_app_are_removed_at_start() {
    let dir = scratch("leftovers");
    let apps = dir.join("Applications");
    let app = apps.join("MailCraft.app");
    make_app(&app, "app");
    make_app(&apps.join(".MailCraft.app.new"), "new");
    let old = apps.join(".MailCraft.app.old");
    make_app(&old, "old");
    chmod(&old.join("Contents").join("MacOS"), 0o555);
    chmod(&old.join("Contents"), 0o555);
    make_app(&apps.join(".Other.app.old"), "somebody else's");
    let folders = Folders::new(dir.join("data"));
    // Another program installing holds the update lock: nothing is touched meanwhile.
    let installing = open_lock(&folders).unwrap();
    installing.try_lock().unwrap();
    assert!(!clear_leftovers(&folders, &app));
    assert_eq!(hidden_in(&apps).len(), 3);
    drop(installing);
    assert!(clear_leftovers(&folders, &app));
    assert_eq!(hidden_in(&apps), vec![".Other.app.old".to_string()]);
    assert_eq!(fs::read(app.join("Contents/MacOS/mailcraft")).unwrap(), b"app");
    // Nothing there: nothing done.
    assert!(clear_leftovers(&folders, &app));
    let _ = fs::remove_dir_all(&dir);
}

// #497: a swap that couldn't put the old version back leaves both where they are, says where,
// and the next start keeps them until the owner has put it right.
#[test]
#[cfg(unix)]
fn an_old_version_that_couldnt_be_put_back_is_kept() {
    let dir = scratch("put-back");
    let apps = dir.join("Applications");
    let app = apps.join("MailCraft.app");
    let (old, new, marker) = (apps.join(".MailCraft.app.old"), apps.join(".MailCraft.app.new"), apps.join(".MailCraft.app.unfinished"));
    // It can be put back: it is, then the new one goes.
    make_app(&old, "old");
    make_app(&new, "new");
    let e = put_back(&app, &old, &new, io::Error::other("cause"));
    assert_eq!(e.to_string(), "cause");
    assert_eq!(fs::read(app.join("Contents/MacOS/mailcraft")).unwrap(), b"old");
    assert_eq!(hidden_in(&apps), Vec::<String>::new());
    // It can't (here: something is in its place): the new one stays, and the error says where.
    make_app(&old, "older");
    make_app(&new, "new");
    let e = put_back(&app, &old, &new, io::Error::other("cause")).to_string();
    assert!(e.contains(&old.display().to_string()) && e.contains(&new.display().to_string()), "{e}");
    assert!(new.is_dir() && old.is_dir() && marker.is_file());
    let folders = Folders::new(dir.join("data"));
    assert!(clear_leftovers(&folders, &app));
    assert!(new.is_dir() && old.is_dir() && marker.is_file(), "kept for the owner");
    // Nor is it removed by the next update.
    let source = dir.join("source.app");
    make_app(&source, "newest");
    assert!(replace(&app, &source, None, None, &mut |_| true).is_err());
    assert!(old.is_dir());
    assert!(!fs::read(app.join("Contents/MacOS/mailcraft")).unwrap().starts_with(b"newest"));
    // The app isn't there at all: what is beside it is kept too, marked or not.
    fs::remove_file(&marker).unwrap();
    fs::rename(&app, dir.join("elsewhere.app")).unwrap();
    assert!(clear_leftovers(&folders, &app));
    assert!(old.is_dir());
    // Once the owner has put the old one back, the rest goes.
    fs::write(&marker, b"x").unwrap();
    fs::rename(&old, &app).unwrap();
    assert!(clear_leftovers(&folders, &app));
    assert_eq!(hidden_in(&apps), Vec::<String>::new());
    let _ = fs::remove_dir_all(&dir);
}

// #497: a Mac copy that doesn't install updates removes what earlier versions left in the data
// folder for them, and nothing else.
#[test]
#[cfg(unix)]
fn a_mac_copy_that_doesnt_update_clears_its_update_folders() {
    let dir = scratch("mac-data");
    let folders = Folders::new(dir.clone());
    fs::create_dir_all(folders.updates()).unwrap();
    fs::write(folders.updates().join("mailcraft-0.8.1-macos-universal.dmg"), b"image").unwrap();
    fs::write(folders.updates().join("other.txt"), b"other").unwrap();
    fs::create_dir_all(folders.mount()).unwrap();
    fs::create_dir_all(folders.handed().join("x")).unwrap();
    make_app(&folders.previous().join("MailCraft.app"), "kept");
    chmod(&folders.previous().join("MailCraft.app").join("Contents"), 0o555);
    fs::write(dir.join("mailbox.mailcraft"), b"mail").unwrap();
    clear_mac_data(&folders);
    assert!(!folders.mount().exists() && !folders.handed().exists() && !folders.previous().exists());
    let left: Vec<_> = fs::read_dir(folders.updates()).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(left, vec![OsString::from("other.txt")]);
    assert!(dir.join("mailbox.mailcraft").is_file());
    let _ = fs::remove_dir_all(&dir);
}

// #497: a signed app whose new copy doesn't pass `codesign` in its place is swapped back, and
// a version that isn't kept isn't offered to go back to.
#[test]
#[cfg(unix)]
fn a_new_app_that_fails_its_check_in_place_is_swapped_back() {
    let dir = scratch("check-in-place");
    let apps = dir.join("Applications");
    let app = apps.join("MailCraft.app");
    make_app(&app, "old");
    let mount = dir.join("mount");
    make_app(&mount.join("MailCraft.app"), "new");
    let folders = Folders::new(dir.join("data"));
    let mut asked = Vec::new();
    // Signed, and the new one holds in the disk image, but not once it is in place.
    let got = mac_swap(&folders, &app, &mount, "0.4.3", &mut |run| {
        assert_eq!(run.program, Path::new("/usr/bin/codesign"));
        assert_eq!(run.args[..3], [OsString::from("--verify"), OsString::from("--deep"), OsString::from("--strict")]);
        asked.push(PathBuf::from(run.args.last().unwrap()));
        if asked.len() < 3 { Some(0) } else { Some(1) }
    });
    assert!(got.is_err(), "{got:?}");
    assert_eq!(asked, vec![app.clone(), mount.join("MailCraft.app"), app.clone()]);
    assert_eq!(fs::read(app.join("Contents/MacOS/mailcraft")).unwrap(), b"old");
    assert_eq!(hidden_in(&apps), Vec::<String>::new());
    assert!(read_kept(&folders.previous()).is_none(), "nothing kept, nothing offered");
    // One that holds in place stays.
    mac_swap(&folders, &app, &mount, "0.4.3", &mut |_| Some(0)).unwrap();
    assert_eq!(fs::read(app.join("Contents/MacOS/mailcraft")).unwrap(), b"new");
    assert_eq!(hidden_in(&apps), Vec::<String>::new());
    assert_eq!(read_kept(&folders.previous()).map(|k| k.version), Some("0.4.3".to_string()));
    let _ = fs::remove_dir_all(&dir);
}

// #497: a hidden copy left by an earlier update that can't be removed stops the update before
// anything changes, and the copy staged for it is removed.
#[test]
#[cfg(unix)]
fn a_stuck_leftover_leaves_the_app_and_nothing_staged() {
    let dir = scratch("stuck-old");
    if writes_anywhere(&dir) {
        return;
    }
    let apps = dir.join("Applications");
    let app = apps.join("MailCraft.app");
    make_app(&app, "old");
    let new = dir.join("new").join("MailCraft.app");
    make_app(&new, "new");
    let old = apps.join(".MailCraft.app.old");
    make_app(&old, "older");
    chmod(&old.join("Contents"), 0o555);
    let got = replace(&app, &new, None, None, &mut |_| true);
    chmod(&old.join("Contents"), 0o755);
    assert!(got.is_err());
    assert_eq!(fs::read(app.join("Contents/MacOS/mailcraft")).unwrap(), b"old");
    assert!(!apps.join(".MailCraft.app.new").exists(), "the staged copy is removed");
    let _ = fs::remove_dir_all(&dir);
}

// #497: a folder that the owner can't write to (as in a disk image) is copied as one that can
// be, so that the copy can be removed.
#[test]
#[cfg(unix)]
fn a_read_only_folder_is_copied_as_one_that_can_be_removed() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = scratch("copy-locked");
    if writes_anywhere(&dir) {
        return;
    }
    let from = dir.join("from.app");
    make_app(&from, "app");
    chmod(&from.join("Contents").join("MacOS"), 0o555);
    chmod(&from.join("Contents"), 0o555);
    let to = dir.join("to.app");
    copy_tree(&from, &to).unwrap();
    chmod(&from.join("Contents"), 0o755);
    chmod(&from.join("Contents").join("MacOS"), 0o755);
    assert_eq!(fs::read(to.join("Contents/MacOS/mailcraft")).unwrap(), b"app");
    let mode = fs::metadata(to.join("Contents")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
    fs::remove_dir_all(&to).unwrap();
    let _ = fs::remove_dir_all(&dir);
}
