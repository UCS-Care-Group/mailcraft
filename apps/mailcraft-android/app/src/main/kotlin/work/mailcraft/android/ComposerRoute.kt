// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import android.content.Context
import android.content.ContextWrapper
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** How long the owner has to stop changing the message before it is kept in Drafts. */
internal const val AUTOSAVE_MILLIS = 4_000L

/** The most files taken from the picker at once. */
private const val MAX_PICKED = 20

/**
 * The composer with the phone's pickers behind it: the system's document picker for files and for a picture
 * (no storage permission is held: the system hands over what the owner chose), read off the main thread
 * under the size cap, and the autosave that keeps the message in Drafts when the owner stops typing and
 * whenever the app is left.
 */
@Composable
internal fun ComposerRoute(
    texts: Texts,
    ui: ComposeUi,
    actions: ComposerActions,
    modifier: Modifier = Modifier,
    io: CoroutineDispatcher = Dispatchers.IO,
) {
    val context = LocalContext.current
    val reader = remember(context) { PickedFiles(context.applicationContext) }
    val scope = rememberCoroutineScope()
    val files = rememberLauncherForActivityResult(ActivityResultContracts.GetMultipleContents()) { chosen ->
        chosen.take(MAX_PICKED).forEach { address ->
            scope.launch {
                val picked = withContext(io) { reader.read(address.toString()) }
                actions.files.onAttach(picked?.name ?: "attachment", picked?.bytes)
            }
        }
    }
    val picture = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { chosen ->
        if (chosen != null) {
            scope.launch { actions.files.onPicture(withContext(io) { reader.read(chosen.toString()) }?.bytes) }
        }
    }
    LaunchedEffect(ui.edits) {
        if (ui.edits > 0) {
            delay(AUTOSAVE_MILLIS)
            actions.onSave()
        }
    }
    KeepWhenLeft(actions.onSave)
    ComposerScreen(texts, ui, actions, ComposerPickers(file = { files.launch("*/*") }, picture = { picture.launch("image/*") }), modifier)
}

/** Keeps the message when the app is no longer in front (another app, the home screen, the screen turning off). */
@Composable
private fun KeepWhenLeft(onSave: () -> Unit) {
    val lifecycle = activityOf(LocalContext.current)?.lifecycle
    // The observer lives as long as the lifecycle does; it calls whichever callback is the latest.
    val latest by rememberUpdatedState(onSave)
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_STOP) latest() }
        lifecycle?.addObserver(observer)
        onDispose { lifecycle?.removeObserver(observer) }
    }
}

private tailrec fun activityOf(context: Context): ComponentActivity? = when (context) {
    is ComponentActivity -> context
    is ContextWrapper -> activityOf(context.baseContext)
    else -> null
}
