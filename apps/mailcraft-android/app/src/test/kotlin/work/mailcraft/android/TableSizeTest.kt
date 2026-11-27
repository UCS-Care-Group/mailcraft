// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(InternalComposeUiApi::class)
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.runtime.Composable
import androidx.compose.ui.InternalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.node.RootForTest
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.Constraints
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
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.TableView

/**
 * I1 and I2 of [SectionPolicy] and the data table: what a table says of its size is what it draws,
 * and the work of a table inside tables is bounded however deep a message nests them (#1966).
 */
class TableSizeTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        snapshotHandler = noPictures(),
    )

    private val env = BodyEnv(EnglishTexts, emptyMap(), emptyMap(), Color.Gray, {}, {})

    /** What [content] drawn in [w] px says and does: the height it said, the height drawn, what is cut. */
    private class Seen(val said: Int, val drawn: Int, val cut: List<String>)

    private fun seen(w: Int, content: @Composable (Dp) -> Unit): Seen {
        val out = ArrayList<Triple<Int, Int, Int>>()
        var cut = emptyList<String>()
        paparazzi.snapshot {
            val view = LocalView.current
            val room = with(LocalDensity.current) { w.toDp() }
            Box(
                modifier = Modifier.wrapContentHeight(unbounded = true).onGloballyPositioned { box ->
                    val root = (view as? RootForTest)?.semanticsOwner?.unmergedRootSemanticsNode
                    if (root != null) cut = cutTexts(root, box.size.height)
                },
            ) {
                Probe(listOf(w), out) { content(room) }
            }
        }
        val (_, said, drawn) = out.last()
        return Seen(said, drawn, cut)
    }

    /** The texts under [root] cut short, or drawn past the foot ([foot] px) of what holds them. */
    private fun cutTexts(root: SemanticsNode, foot: Int): List<String> {
        val out = ArrayList<String>()
        fun walk(n: SemanticsNode) {
            val text = n.config.getOrNull(SemanticsProperties.Text)?.joinToString("") { it.text }
            if (!text.isNullOrBlank()) {
                val layouts = ArrayList<TextLayoutResult>()
                n.config.getOrNull(SemanticsActions.GetTextLayoutResult)?.action?.invoke(layouts)
                // The words of a picture not shown stand in the picture's own box, the size the message gives it.
                val cut = layouts.any { it.didOverflowHeight } && text != ALT
                if (cut) out += "${text.take(SHOWN)} (cut short)"
                if (n.boundsInRoot.bottom > foot + 1) out += "${text.take(SHOWN)} (past the foot, ${n.boundsInRoot.bottom} > $foot)"
            }
            n.children.forEach(::walk)
        }
        walk(root)
        return out
    }

    /** For each width in px: the height [table] says it takes there, and the height it is drawn at. */
    private fun heights(table: TableView, widths: List<Int>): List<Triple<Int, Int, Int>> {
        val out = ArrayList<Triple<Int, Int, Int>>()
        paparazzi.snapshot {
            Probe(widths, out) { TableBlock(table, AlignView.LEFT, env, Paper.of(WHITE, LINK), 360.dp) }
        }
        return out
    }

    @Composable
    private fun Probe(widths: List<Int>, out: MutableList<Triple<Int, Int, Int>>, content: @Composable () -> Unit) {
        Layout(content = content) { measurables, constraints ->
            val m = measurables.first()
            widths.forEach { w -> out += Triple(w, m.minIntrinsicHeight(w), 0) }
            // A measure takes each child once: the last width is measured, the others asked.
            val w = widths.last()
            val pl = m.measure(Constraints(0, w, 0, Constraints.Infinity))
            out[out.lastIndex] = Triple(w, out.last().second, pl.height)
            layout(constraints.maxWidth, pl.height) { pl.place(0, 0) }
        }
    }

    private fun assertSaysWhatItDraws(table: TableView) {
        for (w in listOf(300, 450, 700, 990)) {
            val (_, said, drawn) = heights(table, listOf(w)).last()
            assertTrue("drawn at nothing at $w px", drawn > 0)
            assertEquals("at $w px it said $said and drew $drawn", said, drawn)
        }
    }

    private fun note(text: String, width: LengthView? = null) =
        B.cell(B.tableBlock(B.table(layout = true, width = width, rows = listOf(listOf(B.cell(B.p(text, 0u), background = 0xFFF4D6u))))))

    private val words = "the delivery leaves the shop on Tuesday morning ".repeat(3)

    @Test
    fun aLayoutTableSaysWhatItDraws() {
        val cell = { t: String -> B.cell(B.p(t, 0u)) }
        assertSaysWhatItDraws(B.table(layout = true, rows = listOf(listOf(cell("Seed trays $words"), B.cell(B.p(words, 0u), background = 0xFFF4D6u)), listOf(cell("Twine"), cell(words)))))
        assertSaysWhatItDraws(B.table(layout = true, width = LengthView.Px(150u), rows = listOf(listOf(cell(words)))))
        assertSaysWhatItDraws(B.table(layout = true, width = LengthView.Percent(60u), rows = listOf(listOf(cell(words), cell(words)))))
    }

    @Test
    fun aDataTableWithTablesInItSaysWhatItDraws() {
        assertSaysWhatItDraws(
            B.table(
                layout = false,
                border = true,
                rows = listOf(listOf(B.cell(B.p("Seed trays $words", 0u)), note(words, LengthView.Px(150u))), listOf(B.cell(B.p("Twine", 0u)), note(words))),
            ),
        )
    }

    private fun button(text: String, beside: Boolean) = B.tableBlock(
        B.table(layout = true, rows = listOf(listOf(B.cell(B.p(text, 0u), background = NAVY, padding = 8u)))).copy(inline = true),
    ).copy(beside = beside, space = 0u)

    private val picture = B.image("https://cdn-images.example/tray.png", ALT, 96u, 48u)

    private fun inner(width: LengthView?) = B.tableBlock(B.table(layout = true, width = width, rows = listOf(listOf(B.cell(B.p(words, 0u), background = TINT)))))

    /** What a cell can hold, by name. */
    private val children: Map<String, List<BlockView>> = mapOf(
        "text" to listOf(B.p(words, 0u)),
        "picture and text" to listOf(picture, B.p(words, 0u).copy(beside = true)),
        "boxes on a line" to listOf(button("Register", false), button("Map", true), button("Share a lift", true)),
        "px table" to listOf(inner(LengthView.Px(150u))),
        "percent table" to listOf(inner(LengthView.Percent(60u))),
        "table" to listOf(inner(null)),
        "picture" to listOf(picture),
    )

    /** [blocks] in a cell, beside a short one, of the kinds of table a cell can be in, by name. */
    private fun holders(blocks: List<BlockView>): Map<String, TableView> {
        val side = B.cell(B.p("Twine", 0u))
        val held = blocks.toTypedArray()
        return mapOf(
            "tinted layout cell" to B.table(layout = true, rows = listOf(listOf(B.cell(*held, background = TINT), side))),
            "untinted layout cell" to B.table(layout = true, rows = listOf(listOf(B.cell(*held), side))),
            "data cell" to B.table(layout = false, border = true, rows = listOf(listOf(B.cell(*held), side))),
        )
    }

    /** What is wrong with [table] drawn at each of [widths], for a message naming it [what]. */
    private fun faultsOf(what: String, table: TableView, widths: List<Int> = WIDTHS): List<String> = widths.flatMap { w ->
        val seen = seen(w) { room -> TableBlock(table, AlignView.LEFT, env, Paper.of(WHITE, LINK), room) }
        listOfNotNull(
            "$what at $w px said ${seen.said}, drew ${seen.drawn}".takeIf { seen.said != seen.drawn },
            "$what at $w px cut ${seen.cut}".takeIf { seen.cut.isNotEmpty() },
        )
    }

    @Test
    fun everyKindOfCellSaysWhatItDrawsAndCutsNothing() {
        val wrong = children.flatMap { (child, blocks) ->
            holders(blocks).flatMap { (holder, table) -> faultsOf("$child in a $holder", table) }
        }
        assertTrue(wrong.joinToString("\n"), wrong.isEmpty())
    }

    @Test
    fun aPictureAndTextInALineSayWhatTheyDraw() {
        val run = listOf(picture, B.p(words, 0u).copy(beside = true))
        val wrong = WIDTHS.mapNotNull { w ->
            val seen = seen(w) { room -> BlockColumn(run, env, Paper.of(WHITE, LINK), room) }
            "at $w px said ${seen.said}, drew ${seen.drawn}".takeIf { seen.said != seen.drawn }
        }
        assertTrue(wrong.joinToString("\n"), wrong.isEmpty())
    }

    @Test
    fun buttonsOnALineInAColouredOrDataCellAreDrawnWhole() {
        val held = holders(children.getValue("boxes on a line")).filterKeys { it != "untinted layout cell" }
        val wrong = held.flatMap { (holder, table) -> faultsOf("in a $holder", table, WIDTHS + 550) }
        assertTrue(wrong.joinToString("\n"), wrong.isEmpty())
    }

    /** [n] bordered tables one inside the next, the innermost with a sentence. */
    private fun nested(n: Int): TableView =
        (1 until n).fold(B.table(layout = false, border = true, rows = listOf(listOf(B.cell(B.p("the bottom line", 0u)))))) { inner, _ ->
            B.table(layout = false, border = true, rows = listOf(listOf(B.cell(B.p("a level", 0u), B.tableBlock(inner)))))
        }

    @Test
    fun twentyTablesInsideOneAnotherAreDrawnQuickly() {
        heights(nested(2), listOf(990))
        val start = System.nanoTime()
        val (_, said, drawn) = heights(nested(DEEP), listOf(990)).last()
        val took = (System.nanoTime() - start) / 1_000_000
        assertTrue("took $took ms", took < 2_000)
        assertEquals(said, drawn)
    }

    private companion object {
        const val DEEP = 20
        const val SHOWN = 24
        const val ALT = "Seed tray"
        const val TINT = 0xFFF4D6u
        const val NAVY = 0x1F3864u
        val WIDTHS = listOf(300, 450, 700, 990)
        const val WHITE = 0xFFFFFF
        const val LINK = 0x0645AD
    }
}
