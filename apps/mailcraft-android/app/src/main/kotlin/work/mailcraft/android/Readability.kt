// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlin.math.pow

/**
 * The colours of a message, adjusted to stay readable on the paper they are drawn on. A message asks
 * for colours as `0xRRGGBB`; a pair whose contrast falls short is changed (the text only, towards
 * black or white, whichever reads better) and a pair that reads well is left as the sender wrote it.
 *
 * In a dark reading of a message written for a white page (the dark theme, its light page off, #1966)
 * the message's colours are recoloured by the desktop's rule (`crates/ui-egui/src/reading/colors.rs`):
 * a light background has its lightness turned over, keeping its hue, muted ([darkBackground]); a dark
 * or strong one stays; and a text colour that then falls short has its lightness turned over too
 * before it is moved towards black or white ([darkInk]). Pictures are never recoloured.
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
    fun plainInk(paper: Int): Int = targetFor(paper)

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

    private const val RGB = 0xFFFFFF
    private const val MAX_FLOOR = 0.4
    private const val MUTE = 0.6

    /**
     * [rgb] as a background in a dark reading: one lighter than a half has its lightness turned
     * over, white landing on the lightness of [surface] (the reader's dark paper) and a half
     * staying a half, its hue kept and muted a little; one a half or darker (black, a navy footer,
     * a saturated red) stays as the sender wrote it.
     */
    fun darkBackground(rgb: Int, surface: Int): Int {
        val (h, s, l) = hsl(rgb)
        if (l <= MIDDLE) return rgb and RGB
        val floor = hsl(surface).third.coerceIn(0.0, MAX_FLOOR)
        return fromHsl(h, s * MUTE, floor + (1.0 - l) * (1.0 - TWO * floor))
    }

    /**
     * [ink] on [paper] in a dark reading: itself when it reads, else its lightness turned over (a
     * dark blue a light blue), else [adjust]ed.
     */
    fun darkInk(ink: Int, paper: Int): Int {
        val own = ink and RGB
        if (contrast(own, paper) >= MIN_CONTRAST) return own
        val (h, s, l) = hsl(own)
        val turned = fromHsl(h, s, 1.0 - l)
        return adjust(if (contrast(turned, paper) > contrast(own, paper)) turned else own, paper)
    }
}

private const val HSL_BYTE = 255
private const val HSL_RED = 16
private const val HSL_GREEN = 8
private const val HSL_ROUND = 0.5
private const val HUE_SIXTHS = 6.0
private const val TWO = 2.0
private const val THREE = 3
private const val FOUR = 4
private const val TINY = 1e-6

/** Hue (0 to 6), saturation and lightness, as the desktop reckons them. */
internal fun hsl(rgb: Int): Triple<Double, Double, Double> {
    val r = (rgb shr HSL_RED and HSL_BYTE) / HSL_BYTE.toDouble()
    val g = (rgb shr HSL_GREEN and HSL_BYTE) / HSL_BYTE.toDouble()
    val b = (rgb and HSL_BYTE) / HSL_BYTE.toDouble()
    val max = maxOf(r, g, b)
    val min = minOf(r, g, b)
    val l = (max + min) / TWO
    val d = max - min
    if (d < TINY) return Triple(0.0, 0.0, l)
    val s = d / (1.0 - kotlin.math.abs(TWO * l - 1.0)).coerceAtLeast(TINY)
    val h = when (max) {
        r -> ((g - b) / d).mod(HUE_SIXTHS)
        g -> (b - r) / d + TWO
        else -> (r - g) / d + FOUR
    }
    return Triple(h, s.coerceIn(0.0, 1.0), l)
}

/** The colour of hue [h] (0 to 6), saturation [s] and lightness [l]. */
internal fun fromHsl(h: Double, s: Double, l: Double): Int {
    val c = (1.0 - kotlin.math.abs(TWO * l - 1.0)) * s
    val x = c * (1.0 - kotlin.math.abs(h.mod(TWO) - 1.0))
    val (r, g, b) = when (h.mod(HUE_SIXTHS).toInt()) {
        0 -> Triple(c, x, 0.0)
        1 -> Triple(x, c, 0.0)
        2 -> Triple(0.0, c, x)
        THREE -> Triple(0.0, x, c)
        FOUR -> Triple(x, 0.0, c)
        else -> Triple(c, 0.0, x)
    }
    val m = l - c / TWO
    fun byte(v: Double): Int = ((v + m) * HSL_BYTE + HSL_ROUND).toInt().coerceIn(0, HSL_BYTE)
    return (byte(r) shl HSL_RED) or (byte(g) shl HSL_GREEN) or byte(b)
}
