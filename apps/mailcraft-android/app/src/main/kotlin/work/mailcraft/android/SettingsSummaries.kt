// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

/** What the Mail to keep offline row says: the window of the first account, or that there is no account. */
internal fun offlineSummary(texts: Texts, ui: SettingsUi, accounts: List<AccountItem>): String {
    val first = accounts.firstOrNull() ?: return texts.t("No account yet")
    return offlineLabel(texts, ui.keepOffline[first.id] ?: DEFAULT_OFFLINE)
}

/** The summary under the Remove local data row. */
internal fun removeDataSummary(texts: Texts): String = texts.t("Mail, accounts and sign-ins on this phone")

/** The summary under the Notifications row. */
internal fun notificationsSummary(texts: Texts, switches: Switches): String = when {
    switches.newMailOnly && switches.primaryOnly -> texts.t("New mail only, Primary or Focused only")
    switches.newMailOnly -> texts.t("New mail only")
    switches.primaryOnly -> texts.t("Primary or Focused only")
    else -> texts.t("Sender and subject shown")
}

/** What the phone holds against its cap, in words. */
internal fun storageUsedText(texts: Texts, storage: StorageUse): String =
    texts.tn(storage.messages.toULong(), "{used} of {cap} used by {n} message", "{used} of {cap} used by {n} messages")
        .replace("{used}", sizeText(storage.usedBytes))
        .replace("{cap}", sizeText(storage.capMb * MIB_BYTES))

private const val MIB_BYTES = 1024L * 1024L

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
