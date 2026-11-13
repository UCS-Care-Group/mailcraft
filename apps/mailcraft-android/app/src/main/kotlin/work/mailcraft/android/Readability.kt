// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlin.math.pow

/**
 * The colours of a message, adjusted to stay readable on the paper they are drawn on. A message asks
 * for colours as `0xRRGGBB`; a pair whose contrast falls short is changed (the text only, towards
 * black or white, whichever reads better) and a pair that reads well is left as the sender wrote it.
 */
internal object Readability {
    /** The contrast ratio a text colour must have against its paper (WCAG's AA for body text). */
    const val MIN_CONTRAST = 4.5

    private const val BYTE = 255
    private const val RED_SHIFT = 16
    private const val GREEN_SHIFT = 8
    private const val LINEAR_KNEE = 0.03928
    private const val LINEAR_SLOPE = 12.92
    private const val GAMMA_OFFSET = 0.055
    private const val GAMMA_SCALE = 1.055
    private const val GAMMA = 2.4
    private const val WEIGHT_RED = 0.2126
    private const val WEIGHT_GREEN = 0.7152
    private const val WEIGHT_BLUE = 0.0722
    private const val FLARE = 0.05
    private const val STEPS = 20
    private const val NEAR_BLACK = 0x14181B
    private const val NEAR_WHITE = 0xF2F4F5
    private const val MIDDLE = 0.5
    private const val BLACK = 0x000000
    private const val WHITE = 0xFFFFFF

    private fun channel(value: Int): Double {
        val c = value / BYTE.toDouble()
        return if (c <= LINEAR_KNEE) c / LINEAR_SLOPE else ((c + GAMMA_OFFSET) / GAMMA_SCALE).pow(GAMMA)
    }

    /** The relative luminance of [rgb] (0 black, 1 white). */
    fun luminance(rgb: Int): Double =
        WEIGHT_RED * channel(rgb shr RED_SHIFT and BYTE) +
            WEIGHT_GREEN * channel(rgb shr GREEN_SHIFT and BYTE) +
            WEIGHT_BLUE * channel(rgb and BYTE)

    /** The contrast ratio of two colours, 1 (none) to 21. */
    fun contrast(a: Int, b: Int): Double {
        val la = luminance(a)
        val lb = luminance(b)
        return (maxOf(la, lb) + FLARE) / (minOf(la, lb) + FLARE)
    }

    /** Whether [paper] is a light colour, so that dark text is what it takes. */
    fun isLight(paper: Int): Boolean = luminance(paper) > LIGHT_PAPER

    private const val LIGHT_PAPER = 0.4

    /** The text colour for text that asks for none on [paper]. */
    fun plainInk(paper: Int): Int = if (isLight(paper)) NEAR_BLACK else NEAR_WHITE

    /**
     * [ink] as it is drawn on [paper]: itself when the pair reads, else moved towards the end of the
     * scale that reads better on that paper, only as far as it takes, so that a brand colour stays one.
     */
    fun adjust(ink: Int, paper: Int): Int {
        if (contrast(ink, paper) >= MIN_CONTRAST) return ink
        val target = targetFor(paper)
        for (step in 1..STEPS) {
            val mixed = mix(ink, target, step / STEPS.toDouble())
            if (contrast(mixed, paper) >= MIN_CONTRAST) return mixed
        }
        return target
    }

    /** The end of the scale to move towards: the softer black or white when it reads, else the pure one. */
    private fun targetFor(paper: Int): Int {
        val soft = if (contrast(NEAR_BLACK, paper) >= contrast(NEAR_WHITE, paper)) NEAR_BLACK else NEAR_WHITE
        if (contrast(soft, paper) >= MIN_CONTRAST) return soft
        return if (contrast(BLACK, paper) >= contrast(WHITE, paper)) BLACK else WHITE
    }

    private fun mix(from: Int, to: Int, amount: Double): Int {
        fun part(shift: Int): Int {
            val a = from shr shift and BYTE
            val b = to shr shift and BYTE
            return (a + (b - a) * amount + MIDDLE).toInt().coerceIn(0, BYTE)
        }
        return (part(RED_SHIFT) shl RED_SHIFT) or (part(GREEN_SHIFT) shl GREEN_SHIFT) or part(0)
    }
}
