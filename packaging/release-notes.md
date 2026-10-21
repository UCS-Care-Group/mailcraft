**What's new in 0.6.0** (an alpha for testers; sign up in [#11](https://github.com/GITHUB_REPOSITORY/issues/11)): shared mailboxes for everyone, four interface languages in preview, and older mail brought down when you ask for more.
- **Shared mailboxes.** No longer behind a switch for testing. On the card of a work or school Microsoft account in File › Info, Add Shared Mailbox opens a shared mailbox you have access to: its mail is read, searched, changed, written and sent from its own address, with a signature of its own, and moved or copied to and from your own mailbox. Microsoft asks once for each permission it needs, the first time it's needed.
- **Interface languages (preview).** French, German, Spanish and Italian, in File › Options › General › Language (Automatic follows the system's language). Every text is translated, with dates, the clock and the first day of the week as each language has them. Each is marked Preview ("Deutsch (Vorschau)") until a native speaker has reviewed it: corrections are welcome, and `docs/translating.md` says how ([#254](https://github.com/GITHUB_REPOSITORY/issues/254)).
- **More mail when you ask for it.** Raising File › Options › Messages to download per folder (or setting it to All) now brings down the older mail a smaller limit left out, at the next Send/Receive of an IMAP account (#223, thanks to @vvaltson).
- **Favorites** name two accounts' folders of one name with their account ("Inbox - Work"), thanks to @mickeyr.
- **Reminder windows** follow the main window's light or dark theme, and carry its icon.

**Early alpha.** The foundations are in place and tested, but much of what a daily mail program needs is not there yet, and most of the Microsoft 365 behaviour has only been tested against stand-ins. What works is in the [README](https://github.com/GITHUB_REPOSITORY#what-works-today).

MailCraft for Windows, macOS, Linux, FreeBSD and the web. Every file is listed with its SHA-256 in `SHA256SUMS.txt`.

| Platform | File | |
|---|---|---|
| Windows 10/11 (x64, x86, ARM64) | `mailcraft-<ver>-windows-<arch>.msi` | Installer for all users: Program Files\MailCraft and the Start menu; offers MailCraft for `mailto:` links and for `.eml`, `.ics` and `.vcf` files without taking them over (choose it in Settings › Default apps); uninstall from Installed apps. `mailcraft-cli.exe` is installed next to the app. |
| | `mailcraft-<ver>-windows-<arch>-portable.zip` | Portable: unzip anywhere and run `mailcraft.exe`; includes `mailcraft-cli.exe`. Registers nothing. |
| macOS 11+ (Apple silicon and Intel) | `mailcraft-<ver>-macos-universal.dmg` | Drag MailCraft to Applications. |
| | `mailcraft-cli-<ver>-macos-universal.zip` | The command-line tool and MCP server alone. |
| Linux x86_64 and aarch64 (glibc 2.35+) | `mailcraft-<ver>-linux-<arch>.AppImage` | `chmod +x` and run (if it does not start, add `--appimage-extract-and-run`). The `.zsync` next to it enables delta updates. |
| | `mailcraft-<ver>-linux-<arch>.deb` / `.rpm` | `sudo apt install ./mailcraft-*.deb` or `sudo dnf install ./mailcraft-*.rpm`. |
| | `mailcraft-<ver>-linux-<arch>.flatpak` | `flatpak install --user mailcraft-*.flatpak` (the freedesktop runtime comes from Flathub). |
| | `mailcraft-<ver>-linux-<arch>.tar.gz` | `bin/mailcraft`, `bin/mailcraft-cli`, the desktop entry, metainfo and icons under `share/`. |
| FreeBSD 15 (x86_64) | `mailcraft-<ver>-freebsd-x86_64.tar.gz` | The two programs, the desktop entry and the icons. |
| Any modern browser | `mailcraft-web-<ver>.zip` | The app as a static site (WebAssembly). It cannot reach mail servers from a browser: bring mail in with File › Import. |

MailCraft keeps your mailbox in its settings folder and saves as you go; one window per mailbox. With a Microsoft 365 or Outlook.com account you sign in on Microsoft's page, and the sign-in is kept by the system's credential store (Windows, macOS, and the Secret Service of GNOME Keyring or KWallet on Linux). What works, and how far each part is from Outlook, is in the [README](https://github.com/GITHUB_REPOSITORY#what-works-today) and in `docs/gaps.md`.

The builds are not signed with a publisher certificate yet: Windows SmartScreen and macOS Gatekeeper will ask before running them.
