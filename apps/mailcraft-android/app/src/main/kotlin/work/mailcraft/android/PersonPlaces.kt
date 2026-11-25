// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.ContactFolders
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException

/**
 * A place a new person can be kept: this phone ([PHONE]) or a contacts folder; a null [folder] is an
 * account's main contacts folder that `people.new` makes when it is first used.
 */
internal data class PersonPlace(val folder: ULong?, val label: String)

/** This phone first, then each contacts folder the accounts may write; [default] is where `people.new` puts one. */
internal data class PersonPlaces(val places: List<PersonPlace>, val default: ULong?)

/** The draft, sent where the form showed it would go when no place was picked: [places]' default. */
internal fun PersonDraft.placed(places: PersonPlaces?): PersonDraft =
    if (folder != null) this else copy(folder = places?.default)

/** What `people.new`'s `folder` calls this phone. */
internal const val PHONE: ULong = 0u

internal fun personPlaces(phone: String, folders: ContactFolders): PersonPlaces {
    val listed = folders.folders.map { PersonPlace(it.id, it.name) }
    // No default named is this phone; a default without an id is a main folder not made yet.
    val at = folders.defaultIndex?.let { listed.getOrNull(it.toInt()) }
    return PersonPlaces(listOf(PersonPlace(PHONE, phone)) + listed, if (at != null) at.folder else PHONE)
}

/** Where a new person can go; null when the core could not say (the form then offers no choice). */
internal suspend fun loadPlaces(
    core: Core,
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
): PersonPlaces? = withContext(dispatcher) {
    try {
        personPlaces(core.t("This phone"), core.contactFolders())
    } catch (_: CoreException) {
        null
    }
}

/** The card's line about where the person is kept, and whether the account may only read it. */
internal fun keptInLine(texts: Texts, card: PersonCard): String {
    val place = card.keptIn.ifBlank { texts.t("This phone") }
    val line = if (card.readOnly) texts.t("Kept in: {place} (read only)") else texts.t("Kept in: {place}")
    return line.replace("{place}", place)
}

/** Where the person is kept, and whether a change waits to go to the account's server. */
@Composable
internal fun KeptIn(texts: Texts, card: PersonCard, gutter: Dp) {
    Text(
        keptInLine(texts, card),
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = gutter, end = gutter, top = 8.dp),
    )
    if (card.notSent) {
        Text(
            texts.t("Not sent yet"),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.tertiary,
            modifier = Modifier.padding(horizontal = gutter),
        )
    }
}
