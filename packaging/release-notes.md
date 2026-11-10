**MailCraft 0.11.1 in short**: a security and fixes release of the public beta, for Microsoft accounts (testers welcome; sign up in [#11](https://github.com/GITHUB_REPOSITORY/issues/11))
- Safer: the fixes from a third security review, three of them published as advisories ([GHSA-qwvc-cmh6-9h8x](https://github.com/GITHUB_REPOSITORY/security/advisories/GHSA-qwvc-cmh6-9h8x), [GHSA-ppcq-57r6-r2cm](https://github.com/GITHUB_REPOSITORY/security/advisories/GHSA-ppcq-57r6-r2cm), [GHSA-hqm2-x252-pj57](https://github.com/GITHUB_REPOSITORY/security/advisories/GHSA-hqm2-x252-pj57)): update.
- Sturdier: a message with a huge number of links, or a large mailbox, can't use up the computer's memory.
- On Windows and macOS, files saved from mail are marked as coming from the Internet, so the system can warn before opening them.
- Google sign-in for Gmail, for invited testers while Google reviews MailCraft: to join, email systems@ucscaregroup.co.uk ([#232](https://github.com/GITHUB_REPOSITORY/issues/232)).
- File › Open & Export › Save Message As saves a message as `.eml`.
- And more fixes, listed below.

**What's new**
- No new features in this release: it is fixes only.

**Fixes**
- Google sign-in is built in, open for now only to invited testers while Google reviews MailCraft: to join, email systems@ucscaregroup.co.uk. Everyone else adds Gmail with an app password, as before, which a new Gmail account now offers first, with Use Google sign-in instead as the second choice ([#232](https://github.com/GITHUB_REPOSITORY/issues/232)).
- The MCP server reads each request within a bound: an over-long one is answered with an error and the server carries on with the next (AU3-R4).
- With MCP connected to the running window, the bridge reads the window's answers within a bound, and an over-long answer from the control port is refused rather than read to the end (AU3-R3).
- With MCP connected to the running window, a command whose answer was lost is no longer sent a second time: the agent is told the app may have run it (AU3-R2).
- An agent connected through MCP now sees a message's source, the searches made from a message's subject or sender, an online meeting's link, and a contact's details and categories marked as text written by somebody else, like the rest of a message (AU3-R1) ([GHSA-hqm2-x252-pj57](https://github.com/GITHUB_REPOSITORY/security/advisories/GHSA-hqm2-x252-pj57)).
- File › Open & Export has Save Message As, and a message saved under a name typed without an extension is saved as `.eml`.
- A folder deleted for good in a Microsoft or Exchange server account is deleted on the server only when the server holds nothing more in it than you had: when mail arrived there since, or a folder was made in it elsewhere, it isn't deleted, comes back with what is new, and MailCraft says how many new items it has; when what the server holds there can't be checked, nothing is deleted (AU3-H7).
- On Windows, how much of a mailbox is read into memory now follows the computer's real memory instead of a fixed 8 GiB.
- Opening another mailbox while mail is being sent or received no longer lets that work change the newly opened mailbox, and a message whose sending was cut short (another mailbox opened, or MailCraft stopped) is held in the Outbox as one that may have been sent rather than sent a second time (AU3-08, AU3-09, AU3-10, AU3-21, AU3-26, AU3-V8).
- A draft begun on another device and sent from MailCraft is sent with the next Send/Receive instead of waiting in the Outbox, including one an earlier version left there (Microsoft and Exchange server accounts).
- Mail is vouched for only by its own server's word, also on Microsoft 365 and for mail moved onto a server; Safe Senders respect a failed check; server-kept meetings are changed only by their organizer; link warnings cover every layout of a message; a file is listed unless it is a picture the body shows (AU3-11, AU3-12, AU3-13, AU3-14, AU3-15, AU3-V5).
- A remembered password or sign-in is used only for the account it was remembered for, as that account is, a certificate it trusts included; a mailbox where two accounts have the same address asks you to confirm its servers before any is used; Remove all data removes every remembered sign-in it read, and removing an account removes its sign-in under every name it was kept by; Exchange server setup asks before your password goes to a server in your domain other than its Autodiscover server; a change to a meeting in a shared mailbox waits until sending from it is allowed (AU3-02, AU3-03, AU3-18, AU3-19, AU3-V1, AU3-V2).
- A late download of a message no longer changes a draft edited or queued to send meanwhile, and a change waiting through a server renumbering goes to the one matching message or is dropped with a word (AU3-16, AU3-17).
- Nine more kinds of file that run or hand off to a program that runs what is in them are never opened from mail, and a file kind with a non-ASCII letter is asked about before it opens instead of being taken for a harmless one (AU3-V3, AU3-V4).
- Security fixes AU3-24, AU3-25, AU3-23 and AU3-20: a yes to an agent's question now covers only what the question showed; anything that changed while it was asked is refused, and Empty Folder leaves mail and folders that arrived in the meantime. AU3-V6: on Windows and macOS, every file saved from mail, an agent's saves included, is marked as coming from the Internet when the system lets the mark be written; Linux and FreeBSD have no such mark ([GHSA-ppcq-57r6-r2cm](https://github.com/GITHUB_REPOSITORY/security/advisories/GHSA-ppcq-57r6-r2cm)).
- A mailbox file that would take more memory to open than MailCraft allows for it on this computer is refused with an error when it is opened, in every form it comes in, instead of using up the memory (AU3-05, AU3-06, AU3-07).
- Reading HTML mail and reopening a draft stay within bounded memory and time (AU3-01, AU3-22) ([GHSA-qwvc-cmh6-9h8x](https://github.com/GITHUB_REPOSITORY/security/advisories/GHSA-qwvc-cmh6-9h8x)).
- Safer releases: the signing steps are kept apart from the build (AU3-04).

**Public beta.** For Microsoft accounts (personal, or work or school) MailCraft is a working mail, calendar and contacts program, kept in step with Outlook on the web; keep Outlook to hand while you try it, and tell us in an issue if anything is lost or out of step. Much of what a daily mail program needs is still to come, and other kinds of account are welcome but not part of the beta. What works is in the [README](https://github.com/GITHUB_REPOSITORY#what-works-today).

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
| Any modern browser | `mailcraft-web-<ver>.zip` | The app as a static site (WebAssembly). From a browser it reaches JMAP servers only, to read ([docs/jmap.md](https://github.com/GITHUB_REPOSITORY/blob/main/docs/jmap.md)): bring other mail in with File › Import. |

MailCraft keeps your mailbox in its settings folder and saves as you go; one window per mailbox. With a Microsoft 365 or Outlook.com account you sign in on Microsoft's page, and the sign-in is kept by the system's credential store (Windows, macOS, and the Secret Service of GNOME Keyring or KWallet on Linux). What works, and how far each part is from Outlook, is in the [README](https://github.com/GITHUB_REPOSITORY#what-works-today) and in `docs/gaps.md`.

The builds are not signed with a publisher certificate yet: Windows SmartScreen and macOS Gatekeeper will ask before running them.
