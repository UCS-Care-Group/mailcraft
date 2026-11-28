// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.LineHeightView

/** A line is never shorter than its text, whatever line height the message asks for (#1967). */
class LineHeightTest {
    private fun block(lineHeight: LineHeightView?, vararg sizes: UByte?) = BlockView(
        BlockKindView.Paragraph, sizes.map { B.span("text", size = it) }, 0u, AlignView.CENTER, 0u, false, 0u,
        lineHeight, 0u, 0u, null, B.noLine,
    )

    @Test
    fun aFooterOfLinksWithAnExactLineSmallerThanItsTextGetsRoomForIt() {
        // `font-size:12px; line-height:10px; mso-line-height-rule:exactly`, as invoice footers write it.
        val height = lineHeightOf(block(LineHeightView.Px(10u), 12u, 12u), BASE_SIZE).value
        assertTrue("$height", height >= 12 * 1.2f)
    }

    @Test
    fun aScaleOrDefaultLineIsMeasuredAgainstTheLargestText() {
        assertTrue(lineHeightOf(block(LineHeightView.Scale(100u), 12u, 40u), BASE_SIZE).value >= 40 * 1.2f)
        assertTrue(lineHeightOf(block(null, 40u), BASE_SIZE).value >= 40 * 1.2f)
        // Text that asks for no size is the body size.
        assertTrue(lineHeightOf(block(LineHeightView.Px(4u), null), BASE_SIZE).value >= BASE_SIZE * 1.2f)
    }

    @Test
    fun aRoomyLineIsKeptAsAsked() {
        assertEquals(30f, lineHeightOf(block(LineHeightView.Px(30u), 12u), BASE_SIZE).value)
        assertEquals(BASE_SIZE * 1.4f, lineHeightOf(block(null, null), BASE_SIZE).value)
    }
}
