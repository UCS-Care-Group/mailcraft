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
    fun plainInkReadsOnAMidTonePaper() {
        listOf(0x999999, 0x808080, 0x777777, white, darkPaper).forEach {
            assertTrue("on ${it.toString(16)}", Readability.contrast(Readability.plainInk(it), it) >= Readability.MIN_CONTRAST)
        }
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
    // The dark recolouring (#1966): a message's light backgrounds turned dark, its text turned to read on them.

    private fun lightness(rgb: Int): Double {
        val r = rgb shr 16 and 0xFF
        val g = rgb shr 8 and 0xFF
        val b = rgb and 0xFF
        return (maxOf(r, g, b) + minOf(r, g, b)) / 510.0
    }

    @Test
    fun whiteBecomesTheReadersDarkPaperAndAPaleGreyALittleLighter() {
        val turned = Readability.darkBackground(white, darkPaper)
        assertTrue(turned.toString(16), Readability.contrast(turned, darkPaper) < 1.05)
        val grey = Readability.darkBackground(0xF2F2F2, darkPaper)
        assertTrue(grey.toString(16), Readability.luminance(grey) > Readability.luminance(turned) && Readability.luminance(grey) < 0.05)
    }

    @Test
    fun darkAndStrongBackgroundsStayAsTheSenderWroteThem() {
        // Black, the navy of a footer, a saturated red highlight: lightness a half or less.
        for (rgb in listOf(black, 0x263D51, 0xFF0000, 0x2E7D32, 0x111111)) {
            assertEquals(rgb.toString(16), rgb, Readability.darkBackground(rgb, darkPaper))
        }
    }

    @Test
    fun aTintKeepsItsHueDarkened() {
        val note = Readability.darkBackground(0xFFF8E1, darkPaper)
        val r = note shr 16 and 0xFF
        val g = note shr 8 and 0xFF
        val b = note and 0xFF
        assertTrue(note.toString(16), r > b && g > b && Readability.luminance(note) < 0.08)
        // Orange as a background (lightness just over a half) is darkened only a little, still orange.
        val orange = Readability.darkBackground(0xF7A136, darkPaper)
        assertTrue(orange.toString(16), (orange shr 16 and 0xFF) > (orange and 0xFF) && lightness(orange) in 0.3..0.5)
    }

    @Test
    fun lightnessTurnsOverSmoothly() {
        var last = Readability.darkBackground(0, darkPaper)
        for (v in 1..255) {
            val now = Readability.darkBackground(v * 0x010101, darkPaper)
            assertTrue("$v: ${last.toString(16)} then ${now.toString(16)}", kotlin.math.abs((now shr 16) - (last shr 16)) <= 3)
            last = now
        }
    }

    @Test
    fun darkTextTurnsLightKeepingItsHueAndTextThatReadsStays() {
        assertEquals(white, Readability.darkInk(black, darkPaper))
        val blue = Readability.darkInk(0x1F497D, darkPaper)
        assertTrue(blue.toString(16), (blue and 0xFF) > (blue shr 16 and 0xFF) && Readability.contrast(blue, darkPaper) >= Readability.MIN_CONTRAST)
        // Orange on navy reads already; white on navy too.
        assertEquals(0xF7A136, Readability.darkInk(0xF7A136, 0x263D51))
        assertEquals(white, Readability.darkInk(white, 0x263D51))
    }

    @Test
    fun textJustShortOfReadingIsBroughtToRead() {
        for (paper in listOf(darkPaper, 0x263D51, 0xFF0000, 0x808080, black)) {
            for (ink in listOf(0x555555, 0x6B6B6B, 0x808080, 0x1C1C1C, 0x003366, 0xC00000, paper)) {
                val shown = Readability.darkInk(ink, paper)
                assertTrue("${ink.toString(16)} on ${paper.toString(16)}: ${shown.toString(16)}", Readability.contrast(shown, paper) >= Readability.MIN_CONTRAST)
            }
        }
    }

    @Test
    fun junkInTheHighBitsIsNotReadWhenTurning() {
        assertEquals(Readability.darkBackground(0xFFFFFF, darkPaper), Readability.darkBackground(0x7FFFFFFF, darkPaper))
        assertEquals(Readability.darkInk(0x000000, darkPaper), Readability.darkInk(0x7F000000, darkPaper))
    }
    @Test
    fun aPaperRecoloursOnlyInADarkReading() {
        val light = Paper.of(white, 0x1B5E8C)
        // Without a dark reading (the light theme, the light page, a message's own dark design) nothing is turned.
        assertEquals(white, light.inner(white).rgb)
        assertEquals(0xFFF59D, light.back(0xFFF59D))
        assertEquals(black, light.inkOn(black, white))
        val dark = Paper.of(darkPaper, 0x9CCAFF).copy(turn = darkPaper)
        val box = dark.inner(white)
        assertTrue(box.rgb.toString(16), Readability.contrast(box.rgb, darkPaper) < 1.05)
        assertEquals("a box inside a box is turned too", darkPaper, box.turn)
        assertTrue(Readability.contrast(box.ink, box.rgb) >= Readability.MIN_CONTRAST)
        assertEquals(white, box.inkOn(black, box.rgb))
    }
    @Test
    fun aLineThatWouldVanishIsLeftToTheReader() {
        val dark = Paper.of(darkPaper, 0x9CCAFF).copy(turn = darkPaper)
        // A white rule on the dark page turns dark too, and would not be seen: the reader's own line colour is used.
        assertEquals(null, dark.line(white))
        // An orange line is darkened a little and still seen.
        assertTrue(dark.line(0xF7A136) != null)
        // Without a dark reading a white rule on the dark page is drawn as written.
        assertEquals(white, Paper.of(darkPaper, 0x9CCAFF).line(white))
    }
}
