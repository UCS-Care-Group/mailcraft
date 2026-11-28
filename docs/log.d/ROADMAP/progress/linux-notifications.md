# Findings: Linux notifications (investigation)

**Question**: which function handles notifications on Linux, library or custom?

**Answer**: Linux/BSD desktop alerts are handled by the `freedesktop::Service` in `apps/mailcraft/src/alerts.rs` (module gated on `target_os = "linux" | freebsd | openbsd | netbsd | dragonfly`).

**How it works**
- `apps/mailcraft/src/alerts.rs:35` `host(ctx)` dispatches per OS; on Linux/BSD it returns `freedesktop::Service::start(ctx)` (`alerts.rs:38`).
- `Service::start` (`alerts.rs:148`) spawns a dedicated thread ("desktop alerts").
- `run` (`alerts.rs:187`) connects to the **session D-Bus** with `zbus::blocking::connection::Builder`, destination `org.freedesktop.Notifications`, path `/org/freedesktop/Notifications`, interface `org.freedesktop.Notifications`; calls `GetCapabilities` once, then `Notify` per alert (`"MailCraft", 0, "", title, body, actions, hints, expire=-1`).
- `listen` (`alerts.rs:239`) receives `ActionInvoked` signals on a second thread and maps notification ids back to alert keys (remembered `(u32, u64)` pairs) to wake the window via `egui::Context::request_repaint`.
- Custom MailCraft code layered on top: per-alert channel (max 16 waiting), markup escaping of alert bodies (`escape_markup`, `alerts.rs:95`), freedesktop sound/category hints per kind (`hints`, `alerts.rs:226`).

**Library vs custom**: a **third-party library**, `zbus` (pure-Rust D-Bus, v5, `async-io` + `blocking-api`), a direct dependency in `apps/mailcraft/Cargo.toml:192` under the Linux/BSD cfg target. The manifest comment (`apps/mailcraft/Cargo.toml:168`) explicitly chooses "zbus rather than libdbus, so neither building nor installing needs a system library". MailCraft's contribution is the client logic around zbus (threading, queueing, escaping, hints, click mapping), not a handwritten D-Bus wire implementation.

**Sources**
- `apps/mailcraft/src/alerts.rs` (freedesktop module, `host`, `Service`, `run`, `listen`, `hints`, `escape_markup`)
- `apps/mailcraft/Cargo.toml` (zbus dependency + rationale comment)
- `crates/host/src/platform.rs` (the cross-platform `Platform::notify`/`schedule_reminder` trait, defaulting to `Unsupported`; the desktop host wires alerts through `apps/mailcraft/src/alerts.rs` instead)
