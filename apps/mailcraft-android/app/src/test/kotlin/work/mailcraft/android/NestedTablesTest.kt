// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(InternalComposeUiApi::class)
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.ui.InternalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.node.RootForTest
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.CellView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.TableView

/** A layout table in a cell of a data table is planned, sized and measured in that cell (#1966). */
class NestedTablesTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5.copy(screenWidth = 2_400),
        renderingMode = SessionParams.RenderingMode.NORMAL,
        snapshotHandler = noPictures(),
    )

    private val env = BodyEnv(EnglishTexts, emptyMap(), emptyMap(), Color.Gray, {}, {})

    /** Where each text of [table], drawn in [room], stands, in dp, by its first word. */
    private fun boundsOf(table: TableView, room: Dp): Map<String, Rect> {
        var found = emptyMap<String, Rect>()
        paparazzi.snapshot {
            val view = LocalView.current
            val density = LocalDensity.current.density
            Box(
                modifier = Modifier.onGloballyPositioned {
                    val root = (view as? RootForTest)?.semanticsOwner?.unmergedRootSemanticsNode
                    if (root != null) found = texts(root, density)
                },
            ) {
                TableBlock(table, AlignView.LEFT, env, Paper.of(WHITE, LINK), room)
            }
        }
        return found
    }

    private fun texts(root: SemanticsNode, density: Float): Map<String, Rect> {
        val out = HashMap<String, Rect>()
        val cuts = HashMap<String, Boolean>()
        fun walk(n: SemanticsNode) {
            val text = n.config.getOrNull(SemanticsProperties.Text)?.joinToString("") { it.text }
            if (!text.isNullOrBlank()) {
                val layouts = ArrayList<TextLayoutResult>()
                n.config.getOrNull(SemanticsActions.GetTextLayoutResult)?.action?.invoke(layouts)
                cuts[text.substringBefore(' ')] = layouts.any { it.didOverflowHeight }
                val b = n.boundsInRoot
                out[text.substringBefore(' ')] = Rect(b.left / density, b.top / density, b.right / density, b.bottom / density)
            }
            n.children.forEach(::walk)
        }
        walk(root)
        cut = cuts
        return out
    }

    private fun note(text: String): CellView =
        B.cell(B.tableBlock(B.table(layout = true, rows = listOf(listOf(B.cell(B.p(text, 0u), background = 0xFFF4D6u))))))

    @Test
    fun aColouredNoteBesideLongTextStaysInItsRow() {
        val long = "Alpha " + "seed trays and twine for the north bed ".repeat(4)
        val wrapped = "Bravo " + "the delivery leaves the shop on Tuesday morning ".repeat(4)
        val table = B.table(
            layout = false,
            border = true,
            rows = listOf(listOf(B.cell(B.p(long, 0u)), note(wrapped)), listOf(B.cell(B.p("Charlie", 0u)), B.cell(B.p("Delta", 0u)))),
        )
        val at = boundsOf(table, 328.dp)
        val note = at.getValue("Bravo")
        val next = at.getValue("Delta")
        assertTrue("the note ($note) runs into the next row ($next)", note.bottom <= next.top)
    }

    @Test
    fun aColouredBoxAsksForWhatItHoldsNotTheRoom() {
        val table = B.table(
            layout = false,
            border = true,
            rows = listOf(listOf(B.cell(B.p("Seed trays, a pack of ten", 0u)), note("In stock"), B.cell(B.p("€12.00", 0u)))),
        )
        val item = boundsOf(table, 328.dp).getValue("Seed")
        assertTrue("the item's column was squeezed to ${item.width} dp", item.width >= 120f)
    }

    @Test
    fun aLayoutTableWithItsOwnWidthInAWideCellKeepsItsColumns() {
        val half = { t: String -> B.cell(B.p(t, 0u), width = LengthView.Px(180u)) }
        val inner = B.table(layout = true, width = LengthView.Px(380u), rows = listOf(listOf(half("Echo"), half("Foxtrot"))))
        val table = B.table(layout = false, border = true, rows = listOf(listOf(B.cell(B.tableBlock(inner)), B.cell(B.p("Golf", 0u)))))
        val at = boundsOf(table, 800.dp)
        assertEquals("its two columns were stacked", at.getValue("Echo").top, at.getValue("Foxtrot").top, 0.5f)
    }

    /** Whether the text that starts with [word] was drawn whole (none of its lines cut at the foot). */
    private var cut = emptyMap<String, Boolean>()

    @Test
    fun aNoteOfItsOwnWidthInAWiderColumnIsDrawnWhole() {
        // A coloured note 150 px wide in a column wider than that: it is measured as it is drawn.
        val words = "Hotel " + "the delivery leaves the shop on Tuesday morning ".repeat(3) + "Zulu"
        val note = B.cell(B.tableBlock(B.table(layout = true, width = LengthView.Px(150u), rows = listOf(listOf(B.cell(B.p(words, 0u), background = 0xFFF4D6u))))))
        val table = B.table(
            layout = false,
            border = true,
            rows = listOf(listOf(B.cell(B.p("India " + "seed trays and twine ".repeat(6), 0u)), note), listOf(B.cell(B.p("Juliet", 0u)), B.cell(B.p("Kilo " + "pots and twine ".repeat(8), 0u)))),
        )
        // The second column is made wider than the note by the row under it.
        val at = boundsOf(table, 360.dp)
        assertTrue("the note (${at.getValue("Hotel")}) runs into the next row (${at.getValue("Kilo")})", at.getValue("Hotel").bottom <= at.getValue("Kilo").top)
        assertEquals("the note's last lines were cut", false, cut["Hotel"])
    }

    @Test
    fun aRowThatDoesNotFitItsCellIsStackedThere() {
        // Two columns planned side by side in the 800 dp the table is composed in, in a cell much narrower.
        val inner = B.table(layout = true, rows = listOf(listOf(B.cell(B.p("Lima " + "plants for the shade ".repeat(3), 0u)), B.cell(B.p("Mike " + "ferns and hostas ".repeat(3), 0u)))))
        val wide = "November " + "seed trays, pots and twine for the north bed ".repeat(6)
        val table = B.table(layout = false, border = true, rows = listOf(listOf(B.cell(B.p(wide, 0u)), B.cell(B.tableBlock(inner)))))
        val at = boundsOf(table, 800.dp)
        val second = at.getValue("Mike")
        val stacked = second.top >= at.getValue("Lima").bottom
        assertTrue("the second cell was squeezed to ${second.width} dp beside the first", stacked || second.width >= 100f)
        assertEquals("a cell's last lines were cut", false, cut["Mike"])
    }

    private companion object {
        const val WHITE = 0xFFFFFF
        const val LINK = 0x0645AD
    }
}
