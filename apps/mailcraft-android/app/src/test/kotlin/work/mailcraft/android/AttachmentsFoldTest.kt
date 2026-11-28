// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(InternalComposeUiApi::class)
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.InternalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.node.RootForTest
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

private val files = listOf(item("Quarterly report.pdf", index = 0u), item("Meeting invitation.ics", index = 1u, calendar = true))

class AttachmentPrefsTest {
    @Test
    fun theDefaultIsToFold() = assertEquals(true, ReadingPrefs().foldAttachments)

    @Test
    fun prefsFromAnOlderVersionReadAsFold() {
        val old = mapOf("light_messages" to false, "header_open" to true)
        val read = readingPrefsFrom { key, default -> old[key] ?: default }
        assertEquals(true, read.foldAttachments)
        assertEquals(false, read.lightMessages)
        assertEquals(true, read.headerOpen)
    }

    @Test
    fun aStoredShowStaysShow() {
        assertEquals(false, readingPrefsFrom { key, default -> if (key == "fold_attachments") false else default }.foldAttachments)
    }
}

/** The folded row starts folded, unfolds on a tap, and the next message is folded again. */
class AttachmentsFoldTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        snapshotHandler = noPictures(),
    )

    private fun states(node: SemanticsNode): List<String> =
        listOfNotNull(node.config.getOrNull(SemanticsProperties.StateDescription)) + node.children.flatMap { states(it) }

    @Composable
    private fun Probe(seen: MutableList<List<String>>, tap: Boolean, content: @Composable () -> Unit) {
        val view = LocalView.current
        var tapped = false
        Box(
            Modifier.onGloballyPositioned {
                val root = (view as? RootForTest)?.semanticsOwner?.rootSemanticsNode ?: return@onGloballyPositioned
                seen += states(root)
                if (tap && !tapped) {
                    tapped = true
                    root.children.firstNotNullOfOrNull { first(it) }?.config?.getOrNull(SemanticsActions.OnClick)?.action?.invoke()
                }
            },
        ) { content() }
    }

    private fun first(node: SemanticsNode): SemanticsNode? =
        if (node.config.getOrNull(SemanticsProperties.StateDescription) != null) node else node.children.firstNotNullOfOrNull { first(it) }

    /** What the reading pane does: the state is kept per message id by the caller. */
    @Composable
    private fun Folding(id: ULong) {
        var open by remember(id) { mutableStateOf(false) }
        FoldedAttachments(EnglishTexts, id, files, AttachmentActions.None, open) { open = it }
    }

    /** Each message keeps its own state: unfolding the first leaves the second (another message) folded. */
    @Test
    fun foldedThenUnfoldedAndAnotherMessageStaysFolded() {
        val seen = mutableListOf<List<String>>()
        paparazzi.snapshot {
            ShellShots.Fixed {
                Probe(seen, tap = true) {
                    androidx.compose.foundation.layout.Column {
                        Folding(1uL)
                        Folding(2uL)
                    }
                }
            }
        }
        assertEquals(listOf("Folded", "Folded"), seen.first())
        assertEquals(listOf("Unfolded", "Folded"), seen.last())
    }
}
