// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import uniffi.mailcraft_mobile.Core

/**
 * Every word the owner reads goes through here: the English text in, the interface language's
 * out (English when there is no translation). The calls are `t("…")` and `tn(n, "…", "…")` with the
 * texts as string literals, which `cargo xtask i18n` gathers into the catalogues.
 */
internal interface Texts {
    fun t(msgid: String): String

    /** The singular or plural form that [n] takes, with `{n}` filled in. */
    fun tn(n: ULong, one: String, other: String): String
}

/** The Rust core's catalogue. */
internal class CoreTexts(private val core: Core) : Texts {
    override fun t(msgid: String): String = core.t(msgid)

    override fun tn(n: ULong, one: String, other: String): String = core.tn(n, one, other)
}

/** English as written, for screenshots and tests that run without the native library. */
internal object EnglishTexts : Texts {
    override fun t(msgid: String): String = msgid

    override fun tn(n: ULong, one: String, other: String): String =
        (if (n == 1uL) one else other).replace("{n}", n.toString())
}
