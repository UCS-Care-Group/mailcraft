// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.InsetsView
import uniffi.mailcraft_mobile.LengthView

/** How a signature is sized and when it is stacked (#1966): the desktop's rules, on the phone. */
class SignatureRulesTest {
    private fun inset(left: Int, right: Int, top: Int = 0, bottom: Int = 0) = InsetsView(top.toUByte(), right.toUByte(), bottom.toUByte(), left.toUByte())

    private fun plan(table: uniffi.mailcraft_mobile.TableView, room: Float) =
        planColumns(columnSpecs(gridOf(table)), room, gapsOf(table).first, table.minColumn.toFloat(), told = table.width != null)

    @Test
    fun aLogoBarAndTextSignatureStaysSideBySideOnAPhone() {
        val table = (SignatureCorpus.logoBar.view.blocks[1].kind as BlockKindView.Table).table
        val widths = requireNotNull(plan(table, room = 361f))
        assertEquals(88f, widths[0], 0f)
        assertEquals(3f, widths[1], 0f)
        assertEquals(361f - 88f - 3f, widths[2], 0.01f)
    }

    @Test
    fun itStacksOnlyWhereTheTextWouldBeNarrowerThanTheDesktopsColumn() {
        val table = (SignatureCorpus.logoBar.view.blocks[1].kind as BlockKindView.Table).table
        val fixed = 88f + 3f
        assertNotNull(plan(table, room = fixed + 110f))
        assertNull(plan(table, room = fixed + 109f))
    }

    @Test
    fun aCellOfPicturesAsksForTheirWidthWithTheirPaddingAndItsOwn() {
        val logo = B.cell(B.image("cid:logo@example.org", "Logo", 88u, 88u))
        assertEquals(88f, picturesWidth(logo))
        val padded = B.cell(B.image("cid:logo@example.org", "Logo", 88u, 88u).copy(imagePadding = inset(4, 6))).copy(padding = inset(0, 10))
        assertEquals(108f, picturesWidth(padded))
        // Two in one line, with the desktop's gap between pictures (6); a line under them that is wider wins.
        val two = B.cell(B.image("a", "", 20u, 20u), B.image("b", "", 20u, 20u).copy(beside = true))
        assertEquals(46f, picturesWidth(two))
        assertNull(picturesWidth(B.cell(B.p("Ada Example"))))
        assertNull(picturesWidth(B.cell(B.image("a", "height only", null, 20u))))
        assertNull(picturesWidth(B.cell()))
    }

    @Test
    fun aTableOfOneColumnOfPicturesKeepsTheWidthItHas() {
        // A banner in a 600 wide table: the column is the table's, not its picture's (as on the desktop).
        val banner = B.table(layout = true, rows = listOf(listOf(B.cell(B.image("a", "Banner", 560u, 120u), padding = 8u))))
        val specs = columnSpecs(gridOf(banner))
        assertEquals(576f, specs[0].pictures)
        assertEquals(listOf(600f), planColumns(specs, 600f, 12f, banner.minColumn.toFloat()))
    }

    @Test
    fun aNarrowCellWithOnlyAColourKeepsItsWidth() {
        val bar = B.cell(width = LengthView.Px(3u), background = 0x2E7D32u)
        val specs = columnSpecs(gridOf(B.table(layout = true, rows = listOf(listOf(B.cell(B.p("Ada")), bar)))))
        assertEquals(false, specs[1].holds)
        assertEquals(3f, specs[1].fixed)
    }

    @Test
    fun aSocialIconBoxIsAsWideAsItsIconAndPadding() {
        val icons = SignatureCorpus.icons()
        val table = (icons.first().kind as BlockKindView.Table).table
        assertEquals(26f, boxWidth(table, gapsOf(table).first))
        assertEquals(26.dp, drawnWidth(table, 300.dp))
        // A box that holds text takes the line; one that says its width has it.
        val text = B.table(layout = true, rows = listOf(listOf(B.cell(B.p("Ada"))))).copy(inline = true)
        assertNull(boxWidth(text, 12f))
        assertEquals(300.dp, drawnWidth(text, 300.dp))
        assertEquals(120.dp, drawnWidth(text.copy(width = LengthView.Px(120u)), 300.dp))
    }

    @Suppress("LongParameterList")
    private fun spots(widths: List<Int>, right: List<Boolean>, room: Int, gaps: List<Int> = widths.map { 0 }, floats: Boolean = true, align: AlignView = AlignView.LEFT) =
        placeBoxes(widths, gaps, right, room, align, floats)

    @Test
    fun boxesFloatedRightStandFromTheRightEdgeTheFirstOutermost() {
        assertEquals(listOf(220 to 0, 140 to 0), spots(listOf(80, 80), listOf(true, true), room = 300))
        // Too many for a line: they wrap in source order, each line from the right edge.
        assertEquals(listOf(100 to 0, 100 to 1), spots(listOf(200, 200), listOf(true, true), room = 300))
    }

    @Test
    fun leftAndRightFloatsShareALineOnlyWhenBothFit() {
        assertEquals(listOf(0 to 0, 200 to 0), spots(listOf(120, 100), listOf(false, true), room = 300))
        // A right float as wide as the line goes on in a line below the left one.
        assertEquals(listOf(0 to 0, 0 to 1), spots(listOf(120, 300), listOf(false, true), room = 300))
    }

    @Test
    fun aPictureColumnCountsThePaddingItsCellIsDrawnWith() {
        val icon = B.cell(B.image("a", "icon", 20u, 20u)).copy(background = 0x2e7d32u)
        val table = B.table(layout = true, rows = listOf(listOf(icon)))
        // A coloured cell that says nothing of its padding is drawn with the box padding (12 a side).
        assertEquals(44f, columnSpecs(gridOf(table), table).single().pictures)
        val own = icon.copy(padding = inset(2, 2))
        val padded = B.table(layout = true, rows = listOf(listOf(own)))
        assertEquals(24f, columnSpecs(gridOf(padded), padded).single().pictures)
    }

    @Test
    fun linesOfBoxesStandOneUnderAnotherWithTheGapBetween() {
        val (tops, all) = lineTops(intArrayOf(20, 30, 10), between = 4)
        assertEquals(listOf(0, 24, 58), tops.toList())
        assertEquals(68, all)
        assertEquals(0, lineTops(IntArray(0), 4).second)
        // Lines taller than Compose can lay out are held to the most it can; the tops stay below that too.
        val (high, most) = lineTops(intArrayOf(MAX_LAYOUT_SIZE, MAX_LAYOUT_SIZE), 4)
        assertEquals(MAX_LAYOUT_SIZE, most)
        assertEquals(MAX_LAYOUT_SIZE, high[1])
        // A tall line of boxes is placed where it is: not held to a picture's most.
        assertEquals(MAX_BOX_HEIGHT + 4, lineTops(intArrayOf(MAX_BOX_HEIGHT, 10), 4).first[1])
    }

    @Test
    fun inlineBoxesKeepTheirGapsAndTheLinesAlignment() {
        // The gap stands before a box that follows another in its line, not before the first of a line.
        assertEquals(listOf(0 to 0, 124 to 0, 0 to 1), spots(listOf(120, 120, 120), listOf(false, false, false), room = 300, gaps = listOf(0, 4, 4), floats = false))
        assertEquals(listOf(45 to 0, 145 to 0), spots(listOf(100, 110), listOf(false, false), room = 300, floats = false, align = AlignView.CENTER))
        assertEquals(listOf(90 to 0, 190 to 0), spots(listOf(100, 110), listOf(false, false), room = 300, floats = false, align = AlignView.RIGHT))
        assertEquals(emptyList<Pair<Int, Int>>(), spots(emptyList(), emptyList(), room = 300))
    }

    @Test
    fun pictureCoversKeepThePictureAndGiveWayLeftAndRight() {
        val pad = inset(left = 10, right = 10, top = 4, bottom = 6)
        val full = paddingAround(pad, wanted = 88f, room = 300f)
        assertEquals(10f, full.left, 0f)
        assertEquals(6f, full.bottom, 0f)
        val short = paddingAround(pad, wanted = 88f, room = 98f)
        assertEquals(5f, short.left, 0.01f)
        assertEquals(5f, short.right, 0.01f)
        assertEquals(4f, short.top, 0f)
        assertEquals(0f, paddingAround(pad, wanted = 400f, room = 300f).left, 0f)
        assertEquals(0f, paddingAround(null, 88f, 300f).left, 0f)
    }

    @Test
    fun aPictureThatSaysOnlyItsHeightIsAsWideAsItsShapeGives() {
        // A 96 by 48 picture asked to be 24 high is 48 wide; one asked for a width has it.
        assertEquals(48f, shownWidth(PictureRef("a", null, null, 24), 96f, 48f), 0.01f)
        assertEquals(30f, shownWidth(PictureRef("a", null, 30, 24), 96f, 48f), 0f)
        assertEquals(96f, shownWidth(PictureRef("a", null, null, null), 96f, 48f), 0f)
        assertEquals(96f, shownWidth(PictureRef("a", null, null, 24), 96f, 0f), 0f)
    }

    @Test
    fun aPictureThatSaysBothItsSizesHasThatShapeElseItsOwn() {
        assertEquals(1f, shownTall(PictureRef("a", null, 88, 88), 96, 48), 0f)
        assertEquals(0.5f, shownTall(PictureRef("a", null, 88, null), 96, 48), 0f)
        assertEquals(0.5f, shownTall(PictureRef("a", null, null, 18), 96, 48), 0f)
        assertEquals(48f, shownTall(PictureRef("a", null, 0, 0), 0, 48), 0f)
    }

    @Test
    fun theRoomLeftInATableOfPicturesGoesToThePictureColumns() {
        // <table width=100%><td><img width=120 height=40></td><td align=right><img width=24 height=24></td>
        val logo = B.cell(B.image("cid:logo@example.org", "Logo", 120u, 40u))
        val icon = B.cell(B.image("cid:icon@example.org", "Icon", 24u, 24u))
        val table = B.table(layout = true, rows = listOf(listOf(logo, icon)), width = LengthView.Percent(100u)).copy(spacing = 0u)
        val widths = requireNotNull(plan(table, room = 336f))
        assertEquals(336f, widths.sum(), 0.01f)
        assertEquals(5f, widths[0] / widths[1], 0.001f)
        // A table that does not say its width has no room to spare (the desktop's): its columns sit tight.
        val told = B.cell(B.p("Ada Example"), width = LengthView.Px(100u))
        val mixed = B.table(layout = true, rows = listOf(listOf(told, icon))).copy(spacing = 0u)
        assertEquals(listOf(100f, 24f), plan(mixed, room = 336f))
        val social = B.table(layout = true, rows = listOf(List(3) { icon })).copy(spacing = 0u)
        assertEquals(listOf(24f, 24f, 24f), plan(social, room = 336f))
        // Beside a column told its share, the room left goes to the column of pictures.
        val half = B.cell(B.p("Ada Example"), width = LengthView.Percent(50u))
        val shared = B.table(layout = true, rows = listOf(listOf(half, icon)), width = LengthView.Percent(100u)).copy(spacing = 0u)
        assertEquals(listOf(168f, 168f), plan(shared, room = 336f))
        // Columns all told their widths share what is left by what they ask for, in a table told its
        // width; one that is not keeps them as they are (the desktop draws it no wider than they).
        val a = B.cell(B.p("A"), width = LengthView.Px(100u))
        val b = B.cell(B.p("B"), width = LengthView.Px(50u))
        val fixed = B.table(layout = true, rows = listOf(listOf(a, b)), width = LengthView.Percent(100u)).copy(spacing = 0u)
        assertEquals(listOf(200f, 100f), plan(fixed, room = 300f))
        assertEquals(listOf(100f, 50f), plan(fixed.copy(width = null), room = 300f))
        // A column that takes what is left still takes it all.
        val text = B.cell(B.p("Ada Example"))
        val withText = B.table(layout = true, rows = listOf(listOf(icon, text))).copy(spacing = 0u)
        assertEquals(listOf(24f, 312f), plan(withText, room = 336f))
    }

    @Test
    fun boxesOnALineAreAsWideAsTheyAndTheGapsBetweenThem() {
        assertEquals(68, boxesWidth(listOf(10, 20, 30), listOf(4, 4, 4)))
        assertEquals(0, boxesWidth(emptyList(), emptyList()))
        assertEquals(MAX_LAYOUT_SIZE, boxesWidth(listOf(MAX_LAYOUT_SIZE, MAX_LAYOUT_SIZE), listOf(0, 0)))
    }

    private fun box(float: Boolean, beside: Boolean, align: AlignView = AlignView.LEFT) =
        B.tableBlock(B.table(layout = true, rows = listOf(listOf(B.cell(B.p("x"))))).copy(inline = true, float = float), align)
            .copy(beside = beside)

    @Test
    fun aRunOfBoxesPartsWhereFloatedAndUnfloatedBoxesMeetAndAtTheDesktopsLongestRun() {
        // A centred button and an icon floated right: the button keeps its line, and stays centred.
        val lines = linesOf(listOf(box(false, false, AlignView.CENTER), box(true, true, AlignView.RIGHT)))
        assertEquals(listOf(1, 1), lines.map { it.blocks.size })
        val many = linesOf(List(70) { box(false, it > 0) })
        assertEquals(listOf(64, 6), many.map { it.blocks.size })
        // Text and pictures beside each other are not parted.
        val words = linesOf(List(70) { B.p("w").copy(beside = it > 0) })
        assertEquals(listOf(70), words.map { it.blocks.size })
    }

    @Test
    fun aPictureHeldToTheMostHeightIsNarrowedByAsMuch() {
        val size = boxSize(10, 360, 65_535f, Int.MAX_VALUE, keepShape = true)
        assertEquals(MAX_BOX_HEIGHT, size.height)
        assertEquals(3, size.width)
        // A stand-in keeps its width: it is a box, not a picture.
        assertEquals(10, boxSize(10, 360, 65_535f, Int.MAX_VALUE).width)
    }
}
