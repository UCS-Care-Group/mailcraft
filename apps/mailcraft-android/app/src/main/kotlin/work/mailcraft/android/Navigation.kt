// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.listSaver

/** The five places of the bottom bar (and the tablet's rail), in their order. */
internal enum class Tab {
    Today,
    Mail,
    Calendar,
    People,
    Tasks,
    ;

    fun label(texts: Texts): String = when (this) {
        Today -> texts.t("Today")
        Mail -> texts.t("Mail")
        Calendar -> texts.t("Calendar")
        People -> texts.t("People")
        Tasks -> texts.t("Tasks")
    }
}

/** The pages of Settings: a row for each on its first page. */
internal enum class SettingsPage {
    Accounts,
    MailOffline,
    Notifications,
    Pictures,
    SenderImages,
    Signatures,
    Language,
    About,
    ;

    fun label(texts: Texts): String = when (this) {
        Accounts -> texts.t("Accounts")
        MailOffline -> texts.t("Mail to keep offline")
        Notifications -> texts.t("Notifications")
        Pictures -> texts.t("Pictures")
        SenderImages -> texts.t("Sender images")
        Signatures -> texts.t("Signatures")
        Language -> texts.t("Language")
        About -> texts.t("About")
    }
}

/**
 * Where the owner is: a tab, or Settings (its first page, or one of its pages). Settings is
 * reached from the drawer on every tab and `back` leaves it the way it came.
 */
internal data class NavState(
    val tab: Tab = Tab.Mail,
    val settings: Boolean = false,
    val page: SettingsPage? = null,
    /** Add Account is open over wherever the owner was. */
    val adding: Boolean = false,
) {
    fun select(tab: Tab): NavState = NavState(tab = tab)

    fun openSettings(): NavState = copy(settings = true, page = null, adding = false)

    fun openPage(page: SettingsPage): NavState = copy(settings = true, page = page, adding = false)

    fun openAddAccount(): NavState = copy(adding = true)

    fun leaveAddAccount(): NavState = copy(adding = false)

    /** One step back, or null when there is nothing to go back to (the system leaves the app). */
    fun back(): NavState? = when {
        adding -> copy(adding = false)
        page != null -> copy(page = null)
        settings -> copy(settings = false)
        else -> null
    }

    companion object {
        /** Saved across a change of configuration or the process's death; what can't be read is the start. */
        val Saver: Saver<NavState, Any> = listSaver(
            save = { listOf(it.tab.ordinal, it.settings, it.page?.ordinal ?: NONE, it.adding) },
            restore = { fromSaved(it) },
        )

        private const val NONE = -1

        /** Where [NavState.adding] is in the saved list; what was saved before it existed has no such part. */
        private const val ADDING_AT = 3

        internal fun fromSaved(saved: List<Any?>): NavState {
            val tab = (saved.getOrNull(0) as? Int)?.let { Tab.entries.getOrNull(it) } ?: Tab.Mail
            val settings = saved.getOrNull(1) as? Boolean ?: false
            val page = (saved.getOrNull(2) as? Int)?.let { SettingsPage.entries.getOrNull(it) }
            val adding = saved.getOrNull(ADDING_AT) as? Boolean ?: false
            return NavState(tab = tab, settings = settings || page != null, page = page, adding = adding)
        }
    }
}
