// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException

private val Gutter = 16.dp
private val RowHeight = 56.dp
private val Small = 8.dp

/** Move to contacts folder (#1144), while it is open over a person's page. */
internal sealed interface MoveState {
    /** The person [id], and where it can go (never where it is). */
    class Picking(val id: ULong, val places: List<PersonPlace>) : MoveState

    /** The owner picked [place], which has [name] (person [there]) with one of the same addresses already. */
    class Asking(val id: ULong, val place: PersonPlace, val there: ULong, val name: String) : MoveState
}

/** What the move page asks of the app. */
internal class MoveActions(
    val onMove: () -> Unit = {},
    val onPick: (PersonPlace) -> Unit = {},
    val onOpenThere: () -> Unit = {},
    val onMoveAnyway: () -> Unit = {},
    val onCancel: () -> Unit = {},
)

/** Where person [card] can be moved: every place [places] offers that has an id, other than where it is. */
internal fun moveTargets(card: PersonCard, places: PersonPlaces): List<PersonPlace> =
    places.places.filter { it.folder != null && it.folder != card.folder }

/** The picker for the person [person] shows, over [places]; null while either is still to come. */
internal fun picking(person: PersonState?, places: PersonPlaces?): MoveState.Picking? {
    val card = (person as? PersonState.Ready)?.card ?: return null
    return places?.let { MoveState.Picking(card.id, moveTargets(card, it)) }
}

/** Somebody already in [folder] with an address of person [id]: (their id, their name), or null. */
internal suspend fun alreadyThere(
    core: Core,
    id: ULong,
    folder: ULong,
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
): Pair<ULong, String>? = withContext(dispatcher) {
    try {
        core.alreadyIn(listOf(id), folder)?.let { it.id to it.name }
    } catch (_: CoreException) {
        null
    }
}

private val MOVED_ID = Regex("\"id\"\\s*:\\s*(\\d+)")

/** Moves person [id] into [folder] with `people.moveToFolder`: its id there, or null when the core refused. */
internal suspend fun movePerson(
    core: Core,
    id: ULong,
    folder: ULong,
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
): ULong? = withContext(dispatcher) {
    try {
        val said = core.execute("people.moveToFolder", "{\"ids\":[$id],\"folder\":$folder}")
        // Moved: the id it has now (another, when it left an account's folder); already there: the same.
        MOVED_ID.find(said)?.groupValues?.get(1)?.toULongOrNull() ?: id
    } catch (_: CoreException) {
        null
    }
}

/**
 * What the move page's buttons do over [state]: [picking] is the picker for the person shown; [moved]
 * gets the person to open after it (the one moved, the one already in the folder picked, or the same
 * one when the core refused) and whether that went as asked.
 */
internal fun moveActions(
    scope: CoroutineScope,
    core: Core,
    state: MutableState<MoveState?>,
    picking: () -> MoveState.Picking?,
    moved: (ULong, Boolean) -> Unit,
): MoveActions = MoveActions(
    onMove = { state.value = picking() },
    onPick = { place ->
        val open = state.value as? MoveState.Picking
        val folder = place.folder
        if (open != null && folder != null) {
            scope.launch {
                val there = alreadyThere(core, open.id, folder)
                if (state.value === open) {
                    if (there != null) {
                        state.value = MoveState.Asking(open.id, place, there.first, there.second)
                    } else {
                        move(core, open.id, folder, moved)
                    }
                }
            }
        }
    },
    onOpenThere = { (state.value as? MoveState.Asking)?.let { moved(it.there, true) } },
    onMoveAnyway = {
        val asking = state.value as? MoveState.Asking
        val folder = asking?.place?.folder
        if (asking != null && folder != null) scope.launch { move(core, asking.id, folder, moved) }
    },
    onCancel = { state.value = null },
)

/** The person page's menu: Move to contacts folder. */
@Composable
internal fun PersonMenu(texts: Texts, actions: MoveActions) {
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) { Icon(AppIcons.More, contentDescription = texts.t("More")) }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(
                text = { Text(texts.t("Move to contacts folder…")) },
                onClick = {
                    open = false
                    actions.onMove()
                },
            )
        }
    }
}

private suspend fun move(core: Core, id: ULong, folder: ULong, moved: (ULong, Boolean) -> Unit) {
    val now = movePerson(core, id, folder)
    moved(now ?: id, now != null)
}

/** The page that picks where the person goes, or asks about somebody there already. */
@Composable
internal fun MovePage(texts: Texts, move: MoveState, actions: MoveActions) {
    Column(modifier = Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = Gutter)) {
        Text(
            texts.t("Move to contacts folder"),
            style = MaterialTheme.typography.headlineSmall,
            modifier = Modifier.padding(horizontal = Gutter).semantics { heading() },
        )
        when (move) {
            is MoveState.Picking -> Picker(texts, move, actions)
            is MoveState.Asking -> Asking(texts, move, actions)
        }
    }
}

@Composable
private fun Picker(texts: Texts, move: MoveState.Picking, actions: MoveActions) {
    if (move.places.isEmpty()) {
        Text(texts.t("There is no other contacts folder to move to."), modifier = Modifier.padding(Gutter))
    }
    move.places.forEach { place ->
        Text(
            place.label,
            style = MaterialTheme.typography.bodyLarge,
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = RowHeight)
                .clickable(role = Role.Button) { actions.onPick(place) }
                .padding(horizontal = Gutter, vertical = 16.dp),
        )
        HorizontalDivider()
    }
    TextButton(onClick = actions.onCancel, modifier = Modifier.padding(horizontal = Small)) { Text(texts.t("Cancel")) }
}

@Composable
private fun Asking(texts: Texts, move: MoveState.Asking, actions: MoveActions) {
    val line = texts.t("{name} is already in {folder}.")
        .replace("{name}", move.name)
        .replace("{folder}", move.place.label)
    Text(line, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.padding(Gutter))
    Row(modifier = Modifier.padding(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(Small)) {
        Button(onClick = actions.onOpenThere) { Text(texts.t("Open that one")) }
        TextButton(onClick = actions.onMoveAnyway) { Text(texts.t("Move anyway")) }
    }
    TextButton(onClick = actions.onCancel, modifier = Modifier.padding(horizontal = Small)) { Text(texts.t("Cancel")) }
}
