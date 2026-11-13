// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ReadabilityTest {
    private val white = 0xFFFFFF
    private val black = 0x000000
    private val darkPaper = 0x101416

    @Test
    fun contrastIsOneForTheSameColourAndTwentyOneForBlackOnWhite() {
        assertEquals(1.0, Readability.contrast(0x336699, 0x336699), 0.001)
        assertEquals(21.0, Readability.contrast(black, white), 0.001)
    }

    @Test
    fun aPairThatReadsIsLeftAsTheSenderWroteIt() {
        assertEquals(0x1B5E8C, Readability.adjust(0x1B5E8C, white))
        assertEquals(0xF0F0F0, Readability.adjust(0xF0F0F0, darkPaper))
    }

    @Test
    fun darkTextOnADarkPaperIsLightenedJustEnough() {
        val adjusted = Readability.adjust(0x222222, darkPaper)
        assertTrue(Readability.contrast(adjusted, darkPaper) >= Readability.MIN_CONTRAST)
        assertTrue("a barely-lightened colour stays close: ${adjusted.toString(16)}", Readability.luminance(adjusted) < 0.5)
    }

    @Test
    fun lightTextOnAWhitePaperIsDarkened() {
        val adjusted = Readability.adjust(0xEEEEEE, white)
        assertTrue(Readability.contrast(adjusted, white) >= Readability.MIN_CONTRAST)
        assertTrue(Readability.luminance(adjusted) < Readability.luminance(0xEEEEEE))
    }

    @Test
    fun theSameColourAsItsPaperBecomesReadableOnEveryPaper() {
        for (paper in listOf(white, black, darkPaper, 0x808080, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00)) {
            val adjusted = Readability.adjust(paper, paper)
            assertTrue("paper ${paper.toString(16)}", Readability.contrast(adjusted, paper) >= Readability.MIN_CONTRAST)
        }
    }

    @Test
    fun plainInkFollowsThePaper() {
        assertTrue(Readability.luminance(Readability.plainInk(white)) < 0.1)
        assertTrue(Readability.luminance(Readability.plainInk(darkPaper)) > 0.8)
    }

    @Test
    fun junkInTheHighBitsChangesNothing() {
        // A colour comes as 0xRRGGBB; anything above is not read.
        assertEquals(Readability.contrast(0x123456, white), Readability.contrast(0x7F123456, white), 0.0)
    }
}
