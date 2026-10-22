**What's new in 0.6.0** (an alpha for testers; sign up in [#11](https://github.com/GITHUB_REPOSITORY/issues/11)): shared mailboxes for everyone, four interface languages in preview, and older mail brought down when you ask for more.
- **Shared mailboxes.** No longer behind a switch for testing. On the card of a work or school Microsoft account in File › Info, Add Shared Mailbox opens a shared mailbox you have access to: its mail is read, searched, changed, written and sent from its own address, with a signature of its own, and moved or copied to and from your own mailbox. Microsoft asks once for each permission it needs, the first time it's needed.
- **Interface languages (preview).** French, German, Spanish and Italian, in File › Options › General › Language (Automatic follows the system's language). Every text is translated, with dates, the clock and the first day of the week as each language has them. Each is marked Preview ("Deutsch (Vorschau)") until a native speaker has reviewed it: corrections are welcome, and `docs/translating.md` says how ([#254](https://github.com/GITHUB_REPOSITORY/issues/254)).
- **More mail when you ask for it.** Raising File › Options › Messages to download per folder (or setting it to All) now brings down the older mail a smaller limit left out, at the next Send/Receive of an IMAP account (#223, thanks to @vvaltson).
- **Favorites** name two accounts' folders of one name with their account ("Inbox - Work"), thanks to @mickeyr.
- **Report a Problem**, in the title bar beside Copy Diagnostics and in Help: say what went wrong, and MailCraft starts the report for you, on GitHub (the problem form filled in, in your browser, with the diagnostics saved to a file to attach; public) or by email (a new message to the maintainers from your own account, with the diagnostics attached; private). You read everything first, and nothing goes until you submit it or press Send.
- **Reminder windows** follow the main window's light or dark theme, and carry its icon.

**Fixes**
- Move to Folder, Copy to Folder and the other folder menus have a search box at the top, so a folder is found by typing part of its name ([#74](https://github.com/GITHUB_REPOSITORY/issues/74), thanks to @strikerz5).
- Show in Groups (View › Arrange By) turns the date headings off for one continuous list, and Single Line (View › Message Preview) shows each message on one line; thanks to @Schintilla for asking ([#214](https://github.com/GITHUB_REPOSITORY/issues/214)).
- Removing an account now removes its mail, attachments and remembered sign-in from this computer, and File › Info › Remove MailCraft's data from this computer removes everything MailCraft keeps here, for an installed or a portable copy; mail on the servers is never touched ([#177](https://github.com/GITHUB_REPOSITORY/issues/177)).
- A shared mailbox, and the next account, no longer wait for your own calendar and contacts to download before their mail starts: calendar and contacts follow once the mail of every account is in, still at start-up and every 15 minutes ([#276](https://github.com/GITHUB_REPOSITORY/issues/276)).
- Ctrl+Q marks the selected messages read and no longer also closes the window ([#269](https://github.com/GITHUB_REPOSITORY/issues/269)).
- Calendar › View shows the four calendar views in place of the messages' controls ([#239](https://github.com/GITHUB_REPOSITORY/issues/239)).
- Add Shared Mailbox no longer waits for a Send/Receive already under way, and once the new mailbox is added the rest of its download no longer holds up your own Send/Receive: your changes go out first, a sign-in left open in the browser no longer makes Add wait, and a Send/Receive you pressed still shows its errors ([#70](https://github.com/GITHUB_REPOSITORY/issues/70), [#257](https://github.com/GITHUB_REPOSITORY/issues/257), [#274](https://github.com/GITHUB_REPOSITORY/issues/274); thanks to @strikerz5 for #257).
- A picture added to a signature that is over the size limit is made smaller instead of refused; when it still can't be taken, the Signatures page says why ([#270](https://github.com/GITHUB_REPOSITORY/issues/270), thanks to @redfoxx15).
- The Appointment window no longer turns the whole program light when Appearance follows a dark system ([#266](https://github.com/GITHUB_REPOSITORY/issues/266)).
- Right-clicking a selected time range in the calendar opens a menu with New Appointment, New Meeting and New All Day Event for that range ([#261](https://github.com/GITHUB_REPOSITORY/issues/261), thanks to @redfoxx15).
- HTML mail laid out with a width, a maximum width, or centring by automatic margins (the usual centred newsletter card) now displays at that width and centred, not full width and misaligned ([#150](https://github.com/GITHUB_REPOSITORY/issues/150)).
- Newsletters, statements and notices keep their padding, margins, lines, rounded corners and line spacing, so their boxes and buttons no longer run into the text ([#71](https://github.com/GITHUB_REPOSITORY/issues/71), thanks to @strikerz5).
- Send/Receive brings new mail in every Microsoft Inbox, shared mailboxes included, first, before the folders and the rest ([#276](https://github.com/GITHUB_REPOSITORY/issues/276)).
- Send/Receive is quicker on a Microsoft account with many folders: your own folders whose message and unread counts haven't changed are skipped, and every folder is still read at the first Send/Receive after opening and once an hour, so flag and category changes there arrive by then ([#276](https://github.com/GITHUB_REPOSITORY/issues/276)).
- A Microsoft account's Send/Receive keeps its connection to Microsoft open between requests instead of opening a new one for each, so it waits less ([#276](https://github.com/GITHUB_REPOSITORY/issues/276)).
- A Microsoft account with many folders lists them in a few requests instead of one for every folder that has subfolders, so Send/Receive gets to the mail sooner ([#276](https://github.com/GITHUB_REPOSITORY/issues/276)).
- Arranged by size, each message in the list says its size under its time ([#273](https://github.com/GITHUB_REPOSITORY/issues/273)).
- Send/Receive of a Microsoft account no longer stops part-way through its folders with "InvalidAuthenticationToken" when the sign-in runs out during the run (a long first download, or the computer asleep in the middle of one): the sign-in is renewed once and the run goes on, and one about to run out is renewed before a job starts ([#306](https://github.com/GITHUB_REPOSITORY/issues/306)).

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
