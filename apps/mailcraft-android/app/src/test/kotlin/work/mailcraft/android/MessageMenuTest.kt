// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(InternalComposeUiApi::class)
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
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
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

private fun tools(triaged: MutableList<Triage>, left: MutableList<Unit>) = ReadingTools(
    MailListContext(EnglishTexts, ShellShots.NOW, SwipeConfig(), emptyList(), ShellShots.categories),
    { triage, _ -> triaged += triage },
    { left += Unit },
    ReadingPrefs(),
    {},
)

/** The message menu keeps every action the toolbar and its dropdown had, and the bars follow the reading view. */
class MessageMenuTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        snapshotHandler = noPictures(),
    )

    private val view = Corpus.plain.view

    private fun entries(dark: Boolean, picks: MutableList<SheetPage> = mutableListOf(), tools: ReadingTools = tools(mutableListOf(), mutableListOf())) =
        menuEntries(EnglishTexts, tools, view, dark, light = false, onTriaged = { _, _, _ -> }, onLight = {}, pick = { picks += it })

    @Test
    fun everyActionOfTheOldToolbarAndDropdownIsInTheMenu() {
        val labels = entries(dark = true).map { it.label }
        listOf(
            "Archive", "Delete", "Move to…", "Move to Junk", "Mark as unread", "Categories…", "Show on a light background",
        ).forEach { assertTrue("$it in $labels", it in labels) }
        assertTrue(labels.any { it == "Flag" || it == "Clear flag" })
        // The page switch is for a dark window only, as it was.
        assertFalse(entries(dark = false).any { it.label.startsWith("Show on") })
    }

    @Test
    fun theRowsRunTheTriageAndPickersTheToolbarRan() {
        val triaged = mutableListOf<Triage>()
        val left = mutableListOf<Unit>()
        val picks = mutableListOf<SheetPage>()
        val all = entries(dark = false, picks = picks, tools = tools(triaged, left))
        listOf("Archive", "Delete", "Move to Junk", "Move to…", "Categories…").forEach { label -> all.first { it.label == label }.onClick() }
        assertEquals(listOf(Triage.Archive, Triage.Delete, Triage.Junk), triaged)
        assertEquals(3, left.size)
        assertEquals(listOf(SheetPage.Move, SheetPage.Categories), picks)
    }

    @Test
    fun theBottomBarIsHiddenWhileAMessageIsOpenOnAPhone() {
        assertTrue(showBottomBar(expanded = false, reading = false))
        assertFalse(showBottomBar(expanded = false, reading = true))
        assertFalse(showBottomBar(expanded = true, reading = false))
    }

    private fun find(node: SemanticsNode, description: String): SemanticsNode? =
        node.children.firstNotNullOfOrNull { c ->
            if (c.config.getOrNull(SemanticsProperties.ContentDescription)?.contains(description) == true) c else find(c, description)
        }

    @Composable
    private fun Tap(description: String, content: @Composable () -> Unit) {
        val view = LocalView.current
        var done = false
        Box(
            Modifier.onGloballyPositioned {
                val root = (view as? RootForTest)?.semanticsOwner?.rootSemanticsNode ?: return@onGloballyPositioned
                if (!done) {
                    done = true
                    find(root, description)?.config?.getOrNull(SemanticsActions.OnClick)?.action?.invoke()
                }
            },
        ) { content() }
    }

    @Test
    fun aTapOutsideThePanelClosesIt() {
        var closed = 0
        val reading = tools(mutableListOf(), mutableListOf())
        paparazzi.snapshot {
            ShellShots.Fixed {
                Tap("Close menu") {
                    MessageMenu(
                        EnglishTexts, reading, view, darkWindow = false, light = false, open = true,
                        onClose = { closed++ }, onTriaged = { _, _, _ -> }, onLight = {},
                    )
                }
            }
        }
        assertEquals(1, closed)
    }
}
