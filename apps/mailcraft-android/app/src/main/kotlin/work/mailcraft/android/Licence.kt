// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

/** One piece of software the app carries: what it is for, and the licence it is used under. */
internal data class Licence(val name: String, val part: String, val licence: String)

/**
 * The licences of what is bundled in the app, for About. The Kotlin side is the tree that
 * `cargo xtask mobile` checks against the permissive allowlist on every build; the Rust side is
 * the libraries of the program's core, whose full list with versions is in the source repository's
 * `Cargo.lock`. A part used under a choice of licences names the one MailCraft uses it under.
 */
internal val BUNDLED_LICENCES: List<Licence> = listOf(
    Licence("MailCraft", "the program", "MIT OR Apache-2.0"),
    Licence("Jetpack Compose and AndroidX", "the screens, the window and the system's services", "Apache-2.0"),
    Licence("Material Components", "the look of the controls", "Apache-2.0"),
    Licence("Kotlin and kotlinx.coroutines", "the app's language and its background work", "Apache-2.0"),
    Licence("JNA", "the bridge between the app and the core", "Apache-2.0"),
    Licence("rustls-platform-verifier", "checking servers' certificates with the system's", "MIT OR Apache-2.0"),
    Licence("rustls and ring", "encrypted connections", "Apache-2.0 OR ISC OR MIT"),
    Licence("Rust and the crates of the core", "mail, calendar, contacts and tasks", "MIT OR Apache-2.0"),
    Licence("UniFFI", "the binding between the core and the app", "MPL-2.0"),
)
