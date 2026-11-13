// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

/** What the Mail to keep offline row says: the window of the first account, or that there is no account. */
internal fun offlineSummary(texts: Texts, ui: SettingsUi, accounts: List<AccountItem>): String {
    val first = accounts.firstOrNull() ?: return texts.t("No account yet")
    return offlineLabel(texts, ui.keepOffline[first.id] ?: DEFAULT_OFFLINE)
}

/** The summary under the Notifications row. */
internal fun notificationsSummary(texts: Texts, switches: Switches): String = when {
    switches.newMailOnly && switches.primaryOnly -> texts.t("New mail, Primary or Focused only")
    switches.newMailOnly -> texts.t("New mail only")
    switches.primaryOnly -> texts.t("Primary or Focused only")
    else -> texts.t("Mail and reminders")
}

/** The summary under the Pictures row. */
internal fun picturesSummary(texts: Texts, switches: Switches): String =
    if (switches.downloadPictures) texts.t("Downloaded automatically") else texts.t("Asked first")

/** The summary under the Sender images row. */
internal fun senderImagesSummary(texts: Texts, switches: Switches): String =
    if (switches.senderImages) texts.t("On") else texts.t("Off")

/** The summary under the Signatures row. */
internal fun signaturesSummary(texts: Texts, ui: SettingsUi, accounts: List<AccountItem>): String {
    if (accounts.isEmpty()) return texts.t("No account yet")
    val set = accounts.count { ui.signatures[it.id]?.saved?.isNotBlank() == true }
    return if (set == 0) texts.t("None") else texts.tn(set.toULong(), "{n} account has one", "{n} accounts have one")
}
