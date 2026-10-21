# Releasing MailCraft

> **Last reviewed:** 2026-10-18 · **Last updated:** 2026-10-21 · **Change:** minor (a Fixes section in the release notes, each issue linked, checked against what merged since the last tag; build provenance as a file beside each release; reproducible builds, checked weekly; one pinned Rust version, 1.98.1, for every build; what to do when a release key leaks: there is no taking it back from copies already out; v0.5.0, the first release with MailCraft's own signature; how a copy that updates itself installs a release; `DISABLEAUTOMATICUPDATES=1`; MailCraft's own release signature: `SHA256SUMS.txt.sig`, the `MAILCRAFT_RELEASE_KEY` secret, `cargo xtask release-key`) · **Status:** releases v0.1.0 to v0.5.0 were built and published by `release.yml` and `package.yml` on GitHub's runners, for every platform in the table below; v0.5.0 is the first with MailCraft's own release signature (`SHA256SUMS.txt.sig`, verified by the in-use key), and none is signed by the operating systems' signatures yet (gap #18).

The canonical recipe is [craftrules `release/playbook.md`](https://github.com/storytold/craftrules/blob/main/release/playbook.md);
this file has the MailCraft specifics. The scripts in `packaging/` are WordCraft's with MailCraft's
names, and differ from them only where the program does (what it opens, its description, its
permissions, its own installer identity), so a release has exactly the files a Craft app's has,
under the same names.

MailCraft lives outside the storytold organisation, so it cannot use the organisation's shared
release workflow (that one needs the organisation's runners and its `release` environment).
`.github/workflows/release.yml` and `package.yml` are MailCraft's own and run on GitHub's
standard runners in any public repository, a fork included. If the repository moves into the
organisation, call `storytold/craft-common-actions/.github/workflows/release.yml` instead, as
WordCraft does.

1. Release notes: `packaging/release-notes.md` is the release's text on GitHub and on the
   website's changelog, which shows it as written. Under **Fixes**, one line for each user-visible
   fix, each linking the issue it resolves (or its PR when there was no issue) in full,
   `[#123](https://github.com/GITHUB_REPOSITORY/issues/123)`, because the website does not link
   a bare `#123`; thank whoever reported it. Check the list against everything merged since the
   last tag, `gh pr list --state merged --search "merged:>YYYY-MM-DD" --json number,title,closingIssuesReferences`:
   every user-visible change is in the notes, and a security fix always is. Lines are added as
   fixes merge and cleared when the next release's notes start.
2. Bump the version: `cargo xtask version set 0.2.0` (the one source of truth is
   `[workspace.package] version`; the app shows it in `--version` and About, with the commit from
   `MAILCRAFT_BUILD_SHA`), commit, and get `cargo xtask ci` green.
3. Tag that commit `v0.2.0` and push the tag. The workflow refuses a tag that isn't the version in
   `Cargo.toml`, then builds:

| Platform | Artifacts | Signing |
|---|---|---|
| macOS | universal `MailCraft.app` in a `.dmg`, universal CLI zip | Developer ID + notarization (`APPLE_*` secrets) |
| Windows | x64, x86 and arm64 `.msi` + portable `.zip` | Azure Trusted Signing (`AZURE_*` secrets) or a certificate (`WINDOWS_CERTIFICATE*`) |
| Linux | x86_64 and aarch64 `.AppImage` (+ `.zsync` for AppImageUpdate), `.flatpak` bundle, `.deb`, `.rpm`, `.tar.gz` | checksums |
| FreeBSD | x86_64 `.tar.gz` (built in a FreeBSD VM; on a tag always, on a dry run only when asked for) | checksums |
| Web | `mailcraft-web-<version>.zip` (static site: `index.html`, wasm, glue) | none |

4. The run makes ONE **draft** GitHub release, `MailCraft v<version>` on that tag, with every file
   and a `SHA256SUMS.txt` (`sha256sum` format; `packaging/release-notes.md` is its text). Running
   the same tag again replaces the files of that draft; a release that is already published is
   never changed by the workflow (the run fails instead). A version with a suffix
   (`0.2.0-rc.1`) is marked as a pre-release.
5. A maintainer checks the draft (files, names, checksums, notes) and publishes it by hand:
   Releases › the draft › Edit › Publish release, or `gh release edit v0.2.0 --draft=false`.
   Nothing is public until then.

Signing secrets (`APPLE_*`, `WINDOWS_CERTIFICATE*` and `AZURE_*`) are secrets of the `release`
environment, not of the repository. That environment accepts version tags (`v*`) only and waits
for a maintainer's approval; the Windows and macOS jobs that sign, and the job that makes the
draft, run in it, and only for a pushed version tag. Without the secrets the builds are unsigned
(ad-hoc on macOS) and the scripts say so, so forks still build. None is set yet, so every
release so far is unsigned. There is no riscv64 Linux build, and no Flathub manifest is published.

## The release signature (`SHA256SUMS.txt.sig`)

Besides the operating systems' signatures, every release is signed by MailCraft itself: the job
that makes the draft signs `SHA256SUMS.txt` with an ed25519 key and puts the signature beside it
as `SHA256SUMS.txt.sig` (64 bytes in base64, one line). A copy of MailCraft that updates itself
takes a downloaded file only when that signature verifies by one of the two public keys it carries
(`RELEASE_KEYS` in `crates/net/src/update.rs`: the key in use and a spare), the file's SHA-256 is
the one the signed list gives for its name, and its version is newer than the running one. Until
the program carries real keys, it downloads nothing.

Such a file is installed when the owner presses Restart to update or quits: on Windows by Windows
Installer (`msiexec /i <file> /passive /norestart`, from a copy of the program in the data folder
once MailCraft has gone), on macOS by attaching the disk image read-only and swapping the app (it
must pass `codesign --verify` when the running app is signed), and for an AppImage by renaming the
new file over the old. Windows Installer and `hdiutil` are given a copy of the file in a folder
made for it alone, checked against the signed list's SHA-256 as it is opened, never the download
itself. So a release's packages must keep their names
(`mailcraft-<version>-<os>-<arch>.<ext>`), the `.dmg` must hold the app at its top, and the `.msi`
must keep its `UpgradeCode` and `AllowDowngrades` (going back installs the older `.msi` over the
newer). An organisation installs the `.msi` with `DISABLEAUTOMATICUPDATES=1` to turn updating by
itself off for every user of the computer.

- **Making the keys (once):** `cargo xtask release-key new <folder>` writes two private keys,
  `mailcraft-release-key-in-use.pkcs8.b64` and `mailcraft-release-key-spare.pkcs8.b64` (base64
  PKCS#8, readable by their owner only), and prints the two public keys. It never writes over
  keys that are there. Keep both private keys offline, in two places; the spare is never used
  until the key in use is lost or stolen.
- **The secret:** the key in use goes into the `release` environment as `MAILCRAFT_RELEASE_KEY`
  (Settings › Environments › release › Environment secrets, or
  `gh secret set MAILCRAFT_RELEASE_KEY --env release < mailcraft-release-key-in-use.pkcs8.b64`),
  and nowhere else. The public keys go into `RELEASE_KEYS`; they are not secret.
- **Signing:** `packaging/sign-sums.sh <folder>` runs `cargo xtask release-key sign
  <folder>/SHA256SUMS.txt` with the key from the environment (built from the checkout with the
  locked crates: no tool is downloaded). The key is never printed; the step names the public key
  it signed with. Without the secret (a fork, a dry run) it says "unsigned" and the run goes on:
  such a release has no `.sig`, and no copy of MailCraft installs it by itself.
- **Checking by hand:** the signature is a plain ed25519 signature over the bytes of
  `SHA256SUMS.txt`; any ed25519 tool verifies it with the public key from `RELEASE_KEYS`.
- **A lost or stolen key:** there is no way to take a key back from copies already out. Every
  copy shipped trusts both keys in its `RELEASE_KEYS` for good, and asks nobody whether one of
  them has been withdrawn, so whoever holds a leaked key can sign a release those copies will
  install, for as long as they run. When the key in use leaks (or may have):
  1. Take `MAILCRAFT_RELEASE_KEY` out of the `release` environment at once, and put the spare in
     its place; the leaked key never signs again.
  2. Make a new spare (`cargo xtask release-key new <another folder>`; keep its private half
     offline as before), and set `RELEASE_KEYS` to the old spare and the new one, dropping the
     leaked key.
  3. Ship that as a release straight away, signed with the old spare, said loudly in its notes:
     which key leaked (its public key) and from when.
  4. Tell owners, on the release page, the website and wherever else they listen, to update by
     hand from the release page rather than wait for the copy to do it, and to check what they
     download: its SHA-256 against `SHA256SUMS.txt`, the signature against the new public keys,
     and the build provenance (`gh attestation verify`, below). A copy that has installed this
     release no longer trusts the leaked key.

  Copies that never install that release, by hand or by themselves, go on trusting the leaked
  key; nothing can be done about them from here. When the spare leaks, the same steps, keeping
  the key in use and replacing the spare. When both leak, the release that drops them can't be
  signed by a key the copies out there trust: owners can only be told to install it by hand.

Every file of a release also gets a build provenance attestation (`actions/attest-build-provenance`):
a signed statement, kept by GitHub outside the release, of the workflow run and commit that made
it. Check a download with `gh attestation verify <file> -R UCS-Care-Group/mailcraft`.

What the build fetches is pinned: every action by full commit SHA (Dependabot moves them, a week
after their release), the shared workflows of `storytold/craft-common-actions` by full commit SHA
(moved by hand), and every tool downloaded by URL by exact version and
sha256, checked before it is used: nfpm (`package.yml`), appimagetool and the AppImage runtime
(`packaging/linux/package.sh`), the Trusted Signing client (`packaging/windows/sign.ps1`),
rustup-init on FreeBSD and wasm-opt (`package.yml`). trunk and wasm-bindgen come pinned through
`taiki-e/install-action`, and trunk runs offline so it downloads nothing itself; the
wasm-bindgen version in `package.yml` follows the one in `Cargo.lock`. To move a tool, change
its version and its sha256 together, taken from the file itself.

Dry run: `gh workflow run release.yml --ref <branch>` builds everything on any branch, unsigned,
and keeps the files as workflow artifacts (FreeBSD only when asked for: it is slow), and their
`SHA256SUMS.txt` as `dry-run-sums` (the signing step says "unsigned"). A dry run names no
environment, is given no secrets and publishes nothing.


Each release also carries `mailcraft-<version>.intoto.jsonl`: the build provenance GitHub attests
(which workflow run, at which commit, made each file), as a file, so it can be checked without
GitHub's command line (`gh attestation verify <file> -R UCS-Care-Group/mailcraft` does the same
online).
## ArtCraft Launcher

The launcher installs a Craft app from its GitHub releases, finding the right file by name
(`<app>-<version>-<os>-<arch>.<ext>`) and checking it against `SHA256SUMS.txt`. What it needs of
MailCraft, and where each part stands:

| Needs | State |
|---|---|
| One release per version with Craft file names and `SHA256SUMS.txt` | The workflow above produces it; seven have been published, v0.1.0 to v0.4.1 |
| An icon in the Craft style: an engraved animal in black ink and cream on one flat app colour, in the rounded tile (`rx` 112 of 512, the figure clipped 6 units in) | `cargo xtask icon` builds every size from `assets/app-icon/source/mailcraft-art.png` once the artwork is there; until then the icon is the drawn placeholder |
| A row in the launcher's catalog (`crates/core/src/catalog.rs` in `storytold/craft-launcher`): id, name, tagline, what it is like, colours, `owner/repo`; for an app outside the organisation also its own reverse-DNS id and publisher (the "community apps" group) | A pull request on that repository, after the first release; the 128 px icon goes with it |

## What the installers do

- **Windows (`.msi`, WiX 5):** per-machine install into Program Files, a Start Menu shortcut, an
  App Paths entry, and the registrations that let the owner choose MailCraft in Settings › Default
  apps for `mailto:` links and `.eml`, `.ics` and `.vcf` files ("Open with" for `.mbox`). It never
  takes a type over from another program; only MailCraft's own `.mailcraft` opens with it by
  default. `mailcraft.exe` is a Windows program with no console window; `mailcraft-cli.exe` is the
  console program. The portable `.zip` holds the same two files and registers nothing.
- **Linux (`.deb`, `.rpm`, `.AppImage`, `.tar.gz`):** `/usr/bin/mailcraft` and `mailcraft-cli`, a
  desktop entry that offers MailCraft for `mailto:` links, messages, calendar files and contact
  cards, AppStream metadata and the icons. The binaries need glibc 2.35 or later.
- **One window per mailbox:** started a second time (the shortcut again, a link, a file from the
  file manager), MailCraft hands what it was started with to the window already open and exits
  (`apps/mailcraft/src/single_instance.rs`), so two windows never save over each other.
- **Product identity:** the MSI's UpgradeCode and component GUIDs are MailCraft's own. Never copy
  them from or into a sibling Craft app (Windows would treat the two as one product).

## Building the packages on your own machine

Windows (needs the MSVC Rust toolchain, the Windows SDK and the .NET SDK):

```
dotnet tool install --global wix --version 5.0.2
wix extension add -g WixToolset.UI.wixext/5.0.2 WixToolset.Util.wixext/5.0.2
pwsh packaging/windows/package.ps1 -Arch x64
```

Linux (build on Ubuntu 22.04 so the result runs on newer systems; these are the packages the
release workflow installs):

```
sudo apt-get install -y build-essential pkg-config libxkbcommon-dev libwayland-dev libx11-dev \
  libxrandr-dev libxi-dev libgl1-mesa-dev desktop-file-utils appstream zsync
# nfpm 2.47.0 from https://github.com/goreleaser/nfpm/releases, then:
packaging/linux/package.sh
```

Both write to `dist/release/`. Also: `packaging/macos/package.sh --arch universal`,
`packaging/linux/flatpak-bundle.sh` (after `package.sh`) and `packaging/web/package.sh`.

## The Rust toolchain

Every workflow that builds MailCraft (CI, the packages, the release, FreeBSD) uses one pinned Rust
version, **1.98.1**, not whatever `stable` is that day, so a release built again from its tag uses
the same compiler, with `Cargo.lock` pinning the crates and the actions pinned by commit. Fuzzing
alone uses nightly, as cargo-fuzz needs. Moving to a newer Rust is a change of its own: replace the
version in `.github/workflows/ci.yml`, `package.yml` and `release.yml` together, and run
`cargo xtask ci` with it first.

## Reproducible builds

The same source builds to the same bytes. The Reproducible build workflow
(`.github/workflows/reproducible.yml`, weekly and on demand) builds the Linux app and CLI twice,
from two checkouts in different folders, and fails when they differ. What makes it hold: one Rust
version for every build, `Cargo.lock` with `--locked`, actions pinned by commit, and the folders
a build runs in mapped to fixed names (`--remap-path-prefix`). To check a release yourself, build
its tag the same way and compare the SHA-256 with `SHA256SUMS.txt`. The packages around the
programs (the `.msi`, `.dmg` and AppImage) carry signatures and timestamps of their own, so it is
the programs inside them that compare.

## What has been checked, and what has not

Checked on 2026-10-12, on a Windows 11 machine and an Ubuntu 22.04 WSL distribution:

- `package.ps1 -Arch x64` builds `mailcraft-<version>-windows-x64.msi` and the portable zip; the
  MSI was read back (identity, files, shortcut, registrations) and unpacked; the program in it is
  a GUI program and the CLI answers `--version`.
- `package.sh` builds the AppImage (with `.zsync`), `.deb`, `.rpm` and `.tar.gz`; the `.deb` was
  installed on a bare system, the CLI ran on the sample mailbox, the window started under a
  virtual X server, answered the control channel and drew itself, and the package uninstalled.
- Found and fixed on the way: the MSI carried WordCraft's UpgradeCode and component GUID; on X11
  the window needs `libxkbcommon-x11`, which the packages did not ask for; the desktop entry
  could not receive a `mailto:` link (`%F`) and named two main categories.

Since then, on GitHub's runners with every release: the x64 MSI is installed, checked and
uninstalled, the x86_64 and aarch64 `.deb` installed, started and removed, and the `.dmg` checked;
the x86 and arm64 Windows builds, the Flatpak bundles and the FreeBSD tarball are built.

Not checked yet: the x86 and arm64 Windows installers, the Flatpak and the FreeBSD tarball
installed and run on a machine of their own; macOS on a real Mac (and it has no DMG artwork yet:
`packaging/macos/dmg/README.md`); signing. On
macOS, `mailto:` links are not handed to the program yet (no `CFBundleURLTypes`). On Linux a
sign-in remembered in the Secret Service across a restart is still to be seen on GNOME and KDE,
and in the Flatpak (which is granted `org.freedesktop.secrets` for it).

## Revision history

| Date | Change |
|---|---|
| 2026-10-21 | Release notes: a Fixes section, each fix's issue linked in full for the website's changelog; checked against what merged since the last tag |
| 2026-10-20 | Build provenance attached to each release as `mailcraft-<version>.intoto.jsonl` |
| 2026-10-20 | Reproducible builds: the Linux app and CLI built twice in different folders and compared, weekly |
| 2026-10-20 | One pinned Rust version, 1.98.1, for CI, the packages and the release |
| 2026-10-19 | v0.5.0, the first release signed with MailCraft's own release key |
| 2026-10-19 | A leaked release key: copies already out trust both keys for good; moving to the spare, a release that drops the leaked key, owners told to update by hand and check what they download |
| 2026-10-18 | How a copy that updates itself installs a release (Windows Installer, the disk image, the AppImage), what the packages must keep for it, and `DISABLEAUTOMATICUPDATES=1` |
| 2026-10-18 | MailCraft's own release signature: `SHA256SUMS.txt.sig` made in the `release` environment with `MAILCRAFT_RELEASE_KEY`, `cargo xtask release-key new` and `sign`, `packaging/sign-sums.sh`; the dry run's checksums |
| 2026-10-16 | Status as it is: v0.1.0 to v0.4.1 published by `release.yml` and `package.yml`, unsigned; riscv64 and the Flathub manifest out of the table (neither is built); what the runners check on every release |
| 2026-10-16 | Signing secrets in the protected `release` environment, used only for a pushed version tag; dry runs unsigned with no secrets; the workflow makes a draft that a maintainer publishes |
| 2026-10-16 | Pinned build tools (version and sha256) and build provenance for every release file |
| 2026-10-15 | Linux packages: the Secret Service for remembered sign-ins (the Flatpak may talk to `org.freedesktop.secrets`; the .deb and .rpm suggest a keyring); what is still to be checked |
| 2026-10-12 | First local builds of the Windows and Linux packages; installer identity, file types and `mailto:`, one window per mailbox, X11 dependency; what is and is not checked |
| 2026-10-12 | A release workflow of MailCraft's own (tag-triggered, standard runners), FreeBSD script, release notes; what ArtCraft Launcher needs |
| 2026-10-11 | First version |
