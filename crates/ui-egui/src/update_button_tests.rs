// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Update button under the notice that a newer version is out (#876): for a copy that updates
//! itself it downloads and checks the release through the host's own download, with progress and
//! Cancel, whatever Install updates automatically says; any other copy opens its file in the
//! browser. Only the owner's click does either.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use mailcraft_engine::Session;
use mailcraft_model::update::{Asset, Release, Version};
use serde_json::json;

use super::*;
use crate::{MailApp, Services, UiState};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn newer() -> Version {
    let running = running().unwrap();
    Version { major: running.major, minor: running.minor + 1, patch: 0, suffix: String::new() }
}

fn address(version: &Version, name: &str) -> String {
    format!("{REPOSITORY}/releases/download/v{version}/{name}")
}

/// A newer release with a few of the files a real one has.
fn release_of(version: Version) -> Release {
    let names = [
        format!("mailcraft-{version}-windows-x64.msi"),
        format!("mailcraft-{version}-windows-x64-portable.zip"),
        format!("mailcraft-{version}-linux-x86_64.deb"),
        format!("mailcraft-{version}-linux-x86_64.rpm"),
        "SHA256SUMS.txt".to_string(),
    ];
    let assets = names.iter().map(|name| Asset { name: name.clone(), size: 9, url: address(&version, name) }).collect();
    Release { page: page_of(&version), version, security: false, assets }
}

/// A stand-in host. Every check answers `answer`, and is counted. A download is held until the
/// test answers it through `finish`; what it was asked for and its progress are kept.
#[derive(Clone, Default)]
struct Host {
    answer: Arc<Mutex<Option<Release>>>,
    checks: Arc<AtomicUsize>,
    downloads: Arc<Mutex<Vec<(String, Progress, Sender<Result<PathBuf, String>>)>>>,
}

impl Host {
    fn asked(&self) -> Vec<String> {
        self.downloads.lock().unwrap().iter().map(|(v, _, _)| v.clone()).collect()
    }

    fn progress(&self) -> Progress {
        self.downloads.lock().unwrap().last().unwrap().1.clone()
    }

    fn finish(&self, answer: Result<PathBuf, String>) {
        let _ = self.downloads.lock().unwrap().last().unwrap().2.send(answer);
    }
}

/// `self_updating`: the host gives a download (the `.msi`, an AppImage).
fn host_services(host: &Host, self_updating: bool) -> Services {
    let (answer, checks) = (host.answer.clone(), host.checks.clone());
    let downloads = host.downloads.clone();
    Services {
        update_check: Some(Box::new(move |_| {
            checks.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(answer.lock().unwrap().clone().ok_or_else(|| "GitHub couldn't be reached".to_string()));
            rx
        })),
        update_download: self_updating.then(|| {
            Box::new(move |release: Release, progress: Progress| {
                let (tx, rx) = std::sync::mpsc::channel();
                downloads.lock().unwrap().push((release.version.to_string(), progress, tx));
                rx
            }) as Box<dyn Fn(Release, Progress) -> Receiver<Result<PathBuf, String>>>
        }),
        ..Default::default()
    }
}

fn start(services: Services, saved: &UiState) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), services);
    app.autosave = false;
    app.apply_prefs(saved.clone());
    app
}

fn frames(app: &mut MailApp) {
    for _ in 0..3 {
        app.updates_tick(&egui::Context::default());
    }
}

/// The daily check and Install updates automatically both off: the owner's press alone counts.
fn both_off() -> UiState {
    UiState { update_check: UpdateCheck::Off, update_install: false, ..Default::default() }
}

/// A copy that updates itself, both switches off, which the owner's Check for Updates has told
/// of a newer release.
fn told(host: &Host, saved: &UiState) -> MailApp {
    *host.answer.lock().unwrap() = Some(release_of(newer()));
    let mut app = start(host_services(host, true), saved);
    frames(&mut app);
    app.run(CHECK, json!({})).unwrap();
    frames(&mut app);
    assert!(app.newer_release().is_some());
    app
}

#[test]
fn update_downloads_and_offers_restart_whatever_install_automatically_says() {
    let host = Host::default();
    let mut app = told(&host, &both_off());
    // Pressing Check for Updates with the switches off downloads nothing by itself.
    assert!(host.asked().is_empty());
    assert_eq!(update_state(&app)["newer"]["offer"], "update");
    assert_eq!(app.run(DOWNLOAD, json!({})).unwrap()["pending"], "download");
    assert_eq!(host.asked(), [newer().to_string()]);
    // Pressed again while it comes: still the one download.
    app.run(DOWNLOAD, json!({})).unwrap();
    assert_eq!(host.asked().len(), 1);
    // Its progress, as the host tells it.
    assert!(host.progress().note(25, 100));
    assert_eq!(update_state(&app)["install"]["progress"], 0.25);
    host.finish(Ok(PathBuf::from("/updates/x")));
    frames(&mut app);
    assert_eq!(app.update_ready().map(ToString::to_string), Some(newer().to_string()));
    assert_eq!(update_state(&app)["install"]["ready"], newer().to_string());
    assert_eq!(app.status_msg.as_ref().map(|(s, _)| s.clone()), Some(format!("MailCraft {} is downloaded and checked: restart to update.", newer())));
    // Installed when the owner quits, or now with Restart to update.
    assert_eq!(app.update_on_exit(), Some(OnExit::Install { version: newer(), restart: false }));
    // "MailCraft now installs its updates itself" would not be true: not said.
    assert!(app.ready_by_choice());
    assert_eq!(app.run(RESTART, json!({})).unwrap()["version"], newer().to_string());
    assert_eq!(app.update_on_exit(), Some(OnExit::Install { version: newer(), restart: true }));
    // Nothing of the choice is kept.
    assert!(!serde_json::to_string(&app.prefs()).unwrap().contains("chosen"));
}

#[test]
fn update_takes_over_a_download_already_on_its_way() {
    let host = Host::default();
    *host.answer.lock().unwrap() = Some(release_of(newer()));
    let mut app = start(host_services(&host, true), &UiState { update_check: UpdateCheck::On, ..Default::default() });
    frames(&mut app);
    // The background download, by itself.
    assert_eq!(host.asked().len(), 1);
    assert_eq!(update_state(&app)["install"]["downloading"], true);
    assert!(!app.updates.downloading.as_ref().unwrap().owner);
    assert_eq!(app.run(DOWNLOAD, json!({})).unwrap()["pending"], "download");
    assert_eq!(host.asked().len(), 1, "no second download");
    assert!(app.updates.downloading.as_ref().unwrap().owner);
}

#[test]
fn agents_and_scripts_cant_press_update_or_cancel() {
    let host = Host::default();
    let mut app = told(&host, &both_off());
    for id in [DOWNLOAD, CANCEL] {
        assert_eq!(app.execute(id, json!({})), Err(UPDATE_OWNERS_CLICK.to_string()), "{id}");
        assert_eq!(app.execute(id, json!({"confirmed": true})), Err(UPDATE_OWNERS_CLICK.to_string()), "{id}");
        assert_eq!(mailcraft_engine::agent::access(id), Some(mailcraft_engine::agent::Access::Never), "{id}");
        let (request, _answer) = crate::ControlRequest::new("engine.execute", json!({"command": id, "params": {}}));
        let request = request.from_agent(Vec::new());
        let Some(crate::control::Outcome::Done(reply)) = crate::agent_gate::screen(&mut app, &request, &[]) else { panic!("settled by the gate") };
        assert_eq!(reply["ok"], false, "{reply}");
    }
    assert!(host.asked().is_empty());
    assert!(app.updates.chosen.is_none());
    // A script's check never starts one either, the switches on or off.
    let host = Host::default();
    *host.answer.lock().unwrap() = Some(release_of(newer()));
    let mut app = start(host_services(&host, true), &UiState { update_check: UpdateCheck::On, update_checked: NOW, ..Default::default() });
    app.updates.resumed = true;
    app.ui.update_install = false;
    app.execute(CHECK, json!({})).unwrap();
    frames(&mut app);
    assert!(app.newer_release().is_some() && host.asked().is_empty());
    // The owner pressed Update and an agent cancels: refused, and the download goes on.
    app.run(DOWNLOAD, json!({})).unwrap();
    assert!(app.execute(CANCEL, json!({})).is_err());
    assert!(!host.progress().cancelled());
}

#[test]
fn the_organisation_turning_updates_off_still_wins() {
    let host = Host::default();
    *host.answer.lock().unwrap() = Some(release_of(newer()));
    let mut services = host_services(&host, true);
    services.updates_turned_off_by_organisation = true;
    let mut app = start(services, &both_off());
    app.run(CHECK, json!({})).unwrap();
    frames(&mut app);
    let release = app.newer_release().unwrap();
    assert_eq!(app.offer(&release), None);
    assert_eq!(update_state(&app)["newer"]["offer"], "none");
    assert_eq!(app.run(DOWNLOAD, json!({})), Err(TURNED_OFF.to_string()));
    assert!(host.asked().is_empty());
    // Even a version somehow downloaded and chosen is not installed.
    app.updates.chosen = Some(newer());
    app.updates.downloaded = Some(newer());
    assert!(app.update_ready().is_none() && app.update_on_exit().is_none());
    // A copy that doesn't update itself has no button either.
    let host = Host::default();
    *host.answer.lock().unwrap() = Some(release_of(newer()));
    let mut services = host_services(&host, false);
    services.updates_turned_off_by_organisation = true;
    let mut app = start(services, &both_off());
    app.run(CHECK, json!({})).unwrap();
    frames(&mut app);
    assert_eq!(app.run(DOWNLOAD, json!({})), Err(TURNED_OFF.to_string()));
    assert!(app.updates.opened.is_none());
}

#[test]
fn the_version_gone_back_from_comes_only_when_pressed_for() {
    let host = Host::default();
    // Gone back from the newer version: the daily check and Install automatically on, but it is
    // not downloaded by itself.
    let saved = UiState { update_check: UpdateCheck::On, update_skip: newer().to_string(), ..Default::default() };
    *host.answer.lock().unwrap() = Some(release_of(newer()));
    let mut app = start(host_services(&host, true), &saved);
    frames(&mut app);
    assert_eq!(host.checks.load(Ordering::Relaxed), 1);
    assert!(host.asked().is_empty());
    // Downloaded somehow, it is still not installed by itself.
    app.updates.downloaded = Some(newer());
    assert!(app.update_ready().is_none());
    app.updates.downloaded = None;
    // Pressed for: downloaded, and installed.
    app.run(DOWNLOAD, json!({})).unwrap();
    host.finish(Ok(PathBuf::from("/updates/x")));
    frames(&mut app);
    assert_eq!(app.update_on_exit(), Some(OnExit::Install { version: newer(), restart: false }));
    // The skip is left as it was: the next version gone back from is still not taken by itself.
    assert_eq!(app.prefs().update_skip, newer().to_string());
}

#[test]
fn a_pre_release_is_offered_as_a_file_and_never_downloaded_here() {
    let host = Host::default();
    let version = Version { suffix: "beta.1".into(), ..newer() };
    *host.answer.lock().unwrap() = Some(release_of(version.clone()));
    let mut app = start(host_services(&host, true), &both_off());
    app.run(CHECK, json!({})).unwrap();
    frames(&mut app);
    let release = app.newer_release().unwrap();
    assert_eq!(app.offer(&release), Some(Offer::Download));
    let opened = app.run(DOWNLOAD, json!({})).unwrap();
    assert_eq!(opened["opened"], page_of(&version), "no file named: its page");
    assert!(host.asked().is_empty());
    // Not even if pressed for by hand somehow.
    app.start_download(&release, true);
    assert!(host.asked().is_empty());
}

#[test]
fn a_notice_from_an_earlier_run_asks_github_first() {
    // Told of in an earlier run: no files are known.
    let saved = UiState { update_told: newer().to_string(), ..both_off() };
    let host = Host::default();
    *host.answer.lock().unwrap() = Some(release_of(newer()));
    let mut app = start(host_services(&host, true), &saved);
    frames(&mut app);
    assert!(app.newer_release().is_some_and(|r| r.assets.is_empty()));
    assert_eq!(host.checks.load(Ordering::Relaxed), 0);
    assert_eq!(app.run(DOWNLOAD, json!({})).unwrap()["pending"], "updateCheck");
    assert_eq!(host.checks.load(Ordering::Relaxed), 1);
    frames(&mut app);
    assert_eq!(host.asked(), [newer().to_string()]);
    host.finish(Ok(PathBuf::from("/updates/x")));
    frames(&mut app);
    assert!(app.update_ready().is_some());

    // The same for a copy that opens the file: opened once the answer is in.
    let host = Host::default();
    *host.answer.lock().unwrap() = Some(release_of(newer()));
    let mut services = host_services(&host, false);
    services.release_file = Some(Box::new(|v: &Version| Some(format!("mailcraft-{v}-linux-x86_64.deb"))));
    let mut app = start(services, &saved);
    frames(&mut app);
    assert_eq!(app.run(DOWNLOAD, json!({})).unwrap()["pending"], "updateCheck");
    frames(&mut app);
    assert_eq!(update_state(&app)["install"]["opened"], address(&newer(), &format!("mailcraft-{}-linux-x86_64.deb", newer())));

    // No answer: said, and nothing done.
    let host = Host::default();
    let mut app = start(host_services(&host, true), &saved);
    frames(&mut app);
    app.run(DOWNLOAD, json!({})).unwrap();
    frames(&mut app);
    assert!(host.asked().is_empty());
    assert_eq!(app.updates.said.as_deref(), Some("Couldn't find out whether a new version is out: GitHub couldn't be reached."));
}

#[test]
fn cancel_stops_the_download_and_nothing_of_it_is_installed() {
    let host = Host::default();
    let mut app = told(&host, &both_off());
    assert!(app.run(CANCEL, json!({})).is_err(), "nothing on its way");
    app.run(DOWNLOAD, json!({})).unwrap();
    assert_eq!(app.run(CANCEL, json!({})).unwrap()["cancelled"], newer().to_string());
    // The host is told to stop at its next piece.
    assert!(host.progress().cancelled());
    assert!(!host.progress().note(10, 100));
    assert_eq!(update_state(&app)["install"]["cancelling"], true);
    // Pressed again while it stops: not a second download into the same folder.
    assert!(app.run(DOWNLOAD, json!({})).is_err());
    assert_eq!(host.asked().len(), 1);
    // It ends (even with the whole file, had it come): nothing is ready, and it is said.
    host.finish(Ok(PathBuf::from("/updates/x")));
    frames(&mut app);
    assert!(app.update_ready().is_none() && app.update_on_exit().is_none());
    assert_eq!(app.updates.said, Some(format!("The download of MailCraft {} was cancelled.", newer())));
    // Not started again by itself in this run, the switches on.
    app.ui.update_check = UpdateCheck::On;
    app.ui.update_install = true;
    app.run(CHECK, json!({})).unwrap();
    frames(&mut app);
    assert_eq!(host.asked().len(), 1);
    // The owner's press starts it again.
    app.run(DOWNLOAD, json!({})).unwrap();
    assert_eq!(host.asked().len(), 2);
}

#[test]
fn a_failed_download_the_owner_asked_for_is_said() {
    let host = Host::default();
    let mut app = told(&host, &both_off());
    app.run(DOWNLOAD, json!({})).unwrap();
    host.finish(Err("the update was refused: the release isn't signed by MailCraft's release key".into()));
    frames(&mut app);
    let said = format!("MailCraft {} couldn't be downloaded: the update was refused: the release isn't signed by MailCraft's release key.", newer());
    assert_eq!(app.updates.said.as_deref(), Some(said.as_str()));
    assert!(app.update_ready().is_none() && app.update_on_exit().is_none());
}

#[test]
fn download_update_opens_this_platforms_file() {
    let deb = |v: &Version| Some(format!("mailcraft-{v}-linux-x86_64.deb"));
    let host = Host::default();
    *host.answer.lock().unwrap() = Some(release_of(newer()));
    let mut services = host_services(&host, false);
    services.release_file = Some(Box::new(deb));
    let mut app = start(services, &both_off());
    app.run(CHECK, json!({})).unwrap();
    frames(&mut app);
    assert_eq!(update_state(&app)["newer"]["offer"], "download");
    let opened = app.run(DOWNLOAD, json!({})).unwrap();
    assert_eq!(opened["opened"], address(&newer(), &format!("mailcraft-{}-linux-x86_64.deb", newer())));
    assert_eq!(opened["file"], true);
    // A package the host can't tell (a development build), or a file the release hasn't: the page.
    for name_of in [None, Some(Box::new(|_: &Version| None) as Box<dyn Fn(&Version) -> Option<String>>)] {
        app.services.release_file = name_of;
        assert_eq!(app.run(DOWNLOAD, json!({})).unwrap()["opened"], page_of(&newer()));
    }
    app.services.release_file = Some(Box::new(|v: &Version| Some(format!("mailcraft-{v}-linux-riscv64.deb"))));
    assert_eq!(app.run(DOWNLOAD, json!({})).unwrap()["file"], false);
    // A development build opens the page whatever it is given: it never downloads either.
    app.services.development_build = true;
    app.services.release_file = None;
    assert_eq!(app.run(DOWNLOAD, json!({})).unwrap()["opened"], page_of(&newer()));
}

#[test]
fn only_this_projects_download_address_is_opened() {
    let version = newer();
    let name = format!("mailcraft-{version}-linux-x86_64.deb");
    let with = |url: &str| Release {
        page: page_of(&version),
        version: version.clone(),
        security: false,
        assets: vec![Asset { name: name.clone(), size: 9, url: url.to_string() }],
    };
    let good = address(&version, &name);
    assert_eq!(file_address(&with(&good), &name), Some(good.clone()));
    // The owner and the repository in any case, and a tag without its `v`.
    let (owner, repo) = REPOSITORY.strip_prefix("https://github.com/").unwrap().split_once('/').unwrap();
    let shouted = format!("https://github.com/{}/{}/releases/download/v{version}/{name}", owner.to_uppercase(), repo.to_uppercase());
    assert_eq!(file_address(&with(&shouted), &name), Some(shouted.clone()));
    let bare = format!("https://github.com/{owner}/{repo}/releases/download/{version}/{name}");
    assert_eq!(file_address(&with(&bare), &name), Some(bare.clone()));
    let path = format!("{owner}/{repo}/releases/download/v{version}/{name}");
    for hostile in [
        format!("http://github.com/{path}"),
        format!("https://github.com.evil.example/{path}"),
        format!("https://github.com@evil.example/{path}"),
        format!("https://evil.example@github.com/{path}"),
        format!("https://github.com:8443/{path}"),
        format!("https://objects.githubusercontent.com/{path}"),
        format!("https://github.com/someone-else/{repo}/releases/download/v{version}/{name}"),
        format!("https://github.com/{owner}/other/releases/download/v{version}/{name}"),
        format!("https://github.com/{owner}/{repo}/releases/download/v0.0.1/{name}"),
        format!("https://github.com/{owner}/{repo}/releases/download/latest/{name}"),
        format!("https://github.com/{owner}/{repo}/releases/download/v{version}/other.deb"),
        format!("https://github.com/{owner}/{repo}/releases/download/v{version}/{name}?x=1"),
        format!("https://github.com/{owner}/{repo}/releases/download/v{version}/{name}#x"),
        format!("https://github.com/{owner}/{repo}/releases/download/v{version}/{name}/"),
        format!("https://github.com/{owner}/{repo}/releases/download/v{version}/%2e%2e/{name}"),
        format!("https://github.com/{owner}/{repo}/releases/download/v{version}/../{name}"),
        format!("https://github.com/{owner}/{repo}/releases/tag/v{version}/{name}"),
        format!("https://github.com/{owner}/{repo}/releases/download/v{version}/{name}\r\nX: y"),
        format!("https://github.com/{owner}/{repo}/releases/download/v{version}/{name} "),
        format!("https://github.com/{owner}/{repo}\\releases/download/v{version}/{name}"),
        format!("https://ｇithub.com/{path}"),
        format!("javascript:alert(1)//github.com/{path}"),
        format!("file:///github.com/{path}"),
        format!("https://github.com/{}{path}", "a/".repeat(300)),
        String::new(),
    ] {
        assert_eq!(file_address(&with(&hostile), &name), None, "{hostile:?}");
    }
    // A name the host gives that isn't this version's, or isn't a file's name.
    for wrong in ["mailcraft-0.0.1-linux-x86_64.deb".to_string(), "../mailcraft.deb".into(), String::new(), "SHA256SUMS.txt".into()] {
        let release = Release { assets: vec![Asset { name: wrong.clone(), size: 9, url: address(&version, &wrong) }], ..with(&good) };
        assert_eq!(file_address(&release, &wrong), None, "{wrong}");
    }
    // A name GitHub lists twice is no single file.
    let twice = Release { assets: vec![with(&good).assets[0].clone(), with(&good).assets[0].clone()], ..with(&good) };
    assert_eq!(file_address(&twice, &name), None);
}

/// The window, on a copy that updates itself (`self_updating`) or one that doesn't, told of a
/// newer release by a press of Check for Updates.
fn window(host: &Host, self_updating: bool, organisation_off: bool) -> Harness<'static, MailApp> {
    *host.answer.lock().unwrap() = Some(release_of(newer()));
    let mut services = host_services(host, self_updating);
    services.updates_turned_off_by_organisation = organisation_off;
    let mut app = start(services, &both_off());
    app.run(CHECK, json!({})).unwrap();
    let mut window = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1400.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    window.run_steps(4);
    window
}

fn about(window: &mut Harness<'static, MailApp>) {
    window.state_mut().run("ui.backstage", json!({"value": true, "page": "about"})).unwrap();
    window.run_steps(4);
}

fn click(window: &mut Harness<'static, MailApp>, kind: &str) {
    let at = window.state().hit.rect(kind, 0).unwrap_or_else(|| panic!("{kind} on screen")).center();
    crate::people_view::testing::click(window, at, egui::PointerButton::Primary, 1);
    window.run_steps(3);
}

#[test]
fn the_status_bar_has_update_under_its_notice_and_then_progress_and_cancel() {
    let host = Host::default();
    let mut w = window(&host, true, false);
    assert!(w.state().hit.rect("updateAvailable", 0).is_some());
    click(&mut w, "statusUpdate");
    assert_eq!(host.asked(), [newer().to_string()]);
    assert!(host.progress().note(40, 100));
    w.run_steps(2);
    assert!(w.state().hit.rect("updateDownloadBar", 0).is_some());
    assert!(w.state().hit.rect("statusUpdate", 0).is_none());
    click(&mut w, "cancelUpdateDownload");
    assert!(host.progress().cancelled());
    host.finish(Err(mailcraft_net_free_cancelled()));
    w.run_steps(3);
    assert!(w.state().updates.downloading.is_none());
    // Offered again.
    assert!(w.state().hit.rect("statusUpdate", 0).is_some());
}

/// What the host says of a download that was stopped (`mailcraft_net`'s words, which this crate
/// doesn't see).
fn mailcraft_net_free_cancelled() -> String {
    "the update couldn't be downloaded: the download was cancelled".to_string()
}

#[test]
fn about_has_update_under_the_notice_then_restart_to_update() {
    let host = Host::default();
    let mut w = window(&host, true, false);
    about(&mut w);
    assert!(w.state().hit.rect("aboutDownloadUpdate", 0).is_none());
    click(&mut w, "aboutUpdate");
    assert_eq!(host.asked().len(), 1);
    assert!(w.state().hit.rect("aboutCancelUpdate", 0).is_some());
    assert!(w.state().hit.rect("aboutUpdateProgress", 0).is_some());
    host.finish(Ok(PathBuf::from("/updates/x")));
    w.run_steps(4);
    assert!(w.state().hit.rect("aboutRestartToUpdate", 0).is_some());
    assert!(w.state().hit.rect("aboutUpdate", 0).is_none());
    // The button sits under the notice, above Check for Updates.
    let check = w.state().hit.rect("checkForUpdates", 0).unwrap();
    assert!(w.state().hit.rect("aboutRestartToUpdate", 0).unwrap().bottom() <= check.top());
    // Back in the window, the status bar has Restart to update too, and not the line that says
    // MailCraft now installs its updates itself.
    w.state_mut().run("ui.backstage", json!({"value": false})).unwrap();
    w.run_steps(4);
    assert!(w.state().hit.rect("restartToUpdate", 0).is_some());
    assert!(w.state().hit.rect("installsItselfOk", 0).is_none());
}

#[test]
fn about_has_download_update_for_any_other_copy_and_nothing_when_the_organisation_says_no() {
    let host = Host::default();
    let mut w = window(&host, false, false);
    about(&mut w);
    assert!(w.state().hit.rect("aboutUpdate", 0).is_none());
    click(&mut w, "aboutDownloadUpdate");
    assert_eq!(w.state().updates.opened.as_deref(), Some(page_of(&newer()).as_str()));
    assert!(host.asked().is_empty());
    let host = Host::default();
    let mut w = window(&host, true, true);
    assert!(w.state().hit.rect("updateAvailable", 0).is_none());
    about(&mut w);
    for kind in ["aboutUpdate", "aboutDownloadUpdate", "statusUpdate"] {
        assert!(w.state().hit.rect(kind, 0).is_none(), "{kind}");
    }
}
