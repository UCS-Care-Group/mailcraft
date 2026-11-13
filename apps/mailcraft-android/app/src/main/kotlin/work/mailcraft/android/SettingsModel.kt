// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.Locale

/**
 * The switches that belong to this phone alone. They are never sent anywhere and never reach the
 * mailbox. The two picture switches are the ones the reading pane and the lists read, so there is
 * one place for each (the Rust side's `DeviceSettings`, which the app keeps in its private storage).
 */
internal data class Switches(
    /** Download the pictures of every message without asking. Off. */
    val downloadPictures: Boolean = false,
    /** Show sender images (Gravatar, favicon.im, Google). Off. */
    val senderImages: Boolean = false,
    /** Notify for new mail only. Off. */
    val newMailOnly: Boolean = false,
    /** Notify only for mail that would alert: Primary or Focused. Off. */
    val primaryOnly: Boolean = false,
)

/** The windows of Mail to keep offline, shortest first, as the engine's `account.edit` takes them. */
internal val OFFLINE_CODES = listOf("3d", "1w", "2w", "1m", "3m", "6m", "12m", "2y", "5y", "10y", "all")

/** What a phone keeps offline until the owner says otherwise: one month. */
internal const val DEFAULT_OFFLINE = "1m"

/** A window in words; a code that isn't one is said as it is. */
internal fun offlineLabel(texts: Texts, code: String): String = when (code) {
    "3d" -> texts.t("3 days")
    "1w" -> texts.t("1 week")
    "2w" -> texts.t("2 weeks")
    "1m" -> texts.t("1 month")
    "3m" -> texts.t("3 months")
    "6m" -> texts.t("6 months")
    "12m" -> texts.t("12 months")
    "2y" -> texts.t("2 years")
    "5y" -> texts.t("5 years")
    "10y" -> texts.t("10 years")
    "all" -> texts.t("All mail")
    else -> code
}

/** Whether [code] keeps more than [than] (a code that isn't a window keeps nothing more). */
internal fun keepsMore(codes: List<String>, code: String, than: String): Boolean {
    val a = codes.indexOf(code)
    val b = codes.indexOf(than)
    return a >= 0 && b >= 0 && a > b
}

/** The room that is kept free beyond what a sync needs: the phone must go on working. */
internal const val FREE_RESERVE = 500L * 1024 * 1024

/** What a sync needs and what the phone has, for the question put before it starts. */
internal data class SpaceWarning(val needed: Long, val free: Long)

/**
 * Whether a sync needing about [needed] bytes may go on without asking: null when there is room
 * for it and [reserve] besides, else the two sizes for the question. A phone that can't say how
 * much it has ([free] below zero) is not asked: the question would have nothing true to say.
 */
internal fun spaceCheck(needed: Long, free: Long, reserve: Long = FREE_RESERVE): SpaceWarning? {
    if (free < 0) return null
    val need = needed.coerceAtLeast(0)
    return if (free < need + reserve) SpaceWarning(need, free) else null
}

/** A size in words that fit a line: 1.5 GB, 40 MB, 0 B. Units are the same in every language. */
internal fun sizeText(bytes: Long): String {
    val b = bytes.coerceAtLeast(0)
    return when {
        b >= GIB -> String.format(Locale.ROOT, "%.1f GB", b.toDouble() / GIB)
        b >= MIB -> String.format(Locale.ROOT, "%.0f MB", b.toDouble() / MIB)
        b >= KIB -> String.format(Locale.ROOT, "%.0f KB", b.toDouble() / KIB)
        else -> "$b B"
    }
}

private const val KIB = 1024L
private const val MIB = 1024L * KIB
private const val GIB = 1024L * MIB

/** The most characters of a signature. */
internal const val MAX_SIGNATURE = 4_000

/** One account's signature in the box: what is kept, what is typed, and whether the box may change it. */
internal data class SignatureState(
    val saved: String = "",
    val draft: String = "",
    /** False for a signature with a picture or its own layout, which a plain box would flatten. */
    val editable: Boolean = true,
    /** Saving it did not go through. */
    val failed: Boolean = false,
) {
    val changed: Boolean get() = draft != saved
}

/**
 * The question before a sync that needs more room than there is: [forWindow] is the window the
 * owner chose (null when it is a sync that asks by itself), and the sync waits for a yes.
 */
internal data class SpaceAsk(val account: ULong, val forWindow: String?, val warning: SpaceWarning)

/** Everything Settings draws that isn't the list of accounts. */
internal data class SettingsUi(
    val switches: Switches = Switches(),
    val codes: List<String> = OFFLINE_CODES,
    /** Each account's window; one not here yet is [DEFAULT_OFFLINE] until the core says. */
    val keepOffline: Map<ULong, String> = emptyMap(),
    val signatures: Map<ULong, SignatureState> = emptyMap(),
    val space: SpaceAsk? = null,
    /** Changing a window did not go through. */
    val keepFailed: Boolean = false,
)

/** What Settings asks of the app: one entry for each thing the owner can do there. */
@Suppress("LongParameterList")
internal class SettingsActions(
    /** The pages that hold accounts are on screen with these: what is kept for each is read. */
    val onShown: (List<AccountItem>) -> Unit = {},
    val onSwitches: (Switches) -> Unit = {},
    val onKeepOffline: (AccountItem, String) -> Unit = { _, _ -> },
    val onSpaceYes: () -> Unit = {},
    val onSpaceNo: () -> Unit = {},
    val onSignatureDraft: (AccountItem, String) -> Unit = { _, _ -> },
    val onSignatureSave: (AccountItem) -> Unit = {},
    /** The system's own settings for a notification channel. */
    val onChannelSettings: (String) -> Unit = {},
    /** A web address of the project's (https only). */
    val onOpenLink: (String) -> Unit = {},
)
