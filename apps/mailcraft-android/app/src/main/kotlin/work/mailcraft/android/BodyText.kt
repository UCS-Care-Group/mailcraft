// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp
import uniffi.mailcraft_mobile.SpanView

/** The paper a piece of a message is drawn on and the ink that reads on it, as `0xRRGGBB`. */
internal data class Paper(val rgb: Int, val ink: Int, val link: Int) {
    companion object {
        /** A paper of [rgb] of the message's own, with the default ink for it. */
        fun of(rgb: Int, link: Int): Paper = Paper(rgb, Readability.plainInk(rgb), Readability.adjust(link, rgb))
    }
}

/** A link's place in a text. */
internal class LinkRange(val start: Int, val end: Int, val link: LinkInfo)

/** Text with the styles of its spans, and the links in it. */
internal class StyledText(val text: AnnotatedString, val links: List<LinkRange>) {
    /** The link at character [offset], if any. */
    fun linkAt(offset: Int): LinkInfo? = links.firstOrNull { offset >= it.start && offset < it.end }?.link
}

/** The size of text that asks for none, in sp: the body text of the app. */
internal const val BASE_SIZE = 16

/** The largest size a message may ask for, in pixels; at 100% font scale this is the cap, and sp scales it after. */
internal const val MAX_SIZE = 48

/** A colour as the core sends it, `0xRRGGBB` in a `UInt`, as an `Int`. */
internal fun UInt.rgb(): Int = (this and 0xFFFFFFu).toInt()

private const val OPAQUE = 0xFF shl 24
private const val RGB_MASK = 0xFFFFFF

internal fun composeColor(rgb: Int): Color = Color(OPAQUE or (rgb and RGB_MASK))

/** The family that a span asks for, as far as this phone has it: the generic ones only. */
internal fun familyOf(font: String?, code: Boolean): FontFamily? {
    if (code) return FontFamily.Monospace
    val first = font?.split(',')?.firstOrNull()?.trim()?.lowercase()
    return when (first) {
        "serif" -> FontFamily.Serif
        "monospace" -> FontFamily.Monospace
        "sans-serif" -> FontFamily.SansSerif
        "cursive" -> FontFamily.Cursive
        else -> null
    }
}

/** The colour of a span's text on [paper]: its own, adjusted to read, else the paper's ink (or the link colour). */
private fun inkOf(span: SpanView, paper: Paper, linked: Boolean): Int {
    val behind = span.background?.rgb() ?: paper.rgb
    val own = span.color?.rgb()
    return when {
        own != null -> Readability.adjust(own, behind)
        linked -> Readability.adjust(paper.link, behind)
        span.background != null -> Readability.plainInk(behind)
        else -> paper.ink
    }
}

private fun decorationOf(span: SpanView, linked: Boolean): TextDecoration? {
    val lines = buildList {
        if (span.underline || linked) add(TextDecoration.Underline)
        if (span.strike) add(TextDecoration.LineThrough)
    }
    return if (lines.isEmpty()) null else TextDecoration.combine(lines)
}

/** The style of one span on [paper]: what it asks for, with the colours adjusted to read. */
internal fun styleOf(span: SpanView, paper: Paper, linked: Boolean): SpanStyle = SpanStyle(
    color = composeColor(inkOf(span, paper, linked)),
    background = span.background?.let { composeColor(it.rgb()) } ?: Color.Unspecified,
    fontWeight = if (span.bold) FontWeight.Bold else null,
    fontStyle = if (span.italic) FontStyle.Italic else null,
    textDecoration = decorationOf(span, linked),
    fontSize = span.size?.toInt()?.coerceIn(1, MAX_SIZE)?.sp ?: TextUnit.Unspecified,
    fontFamily = familyOf(span.font, span.code),
)

/** The spans as styled text. A link that is not followable ([links] has no word of it) is plain text. */
internal fun styled(spans: List<SpanView>, paper: Paper, links: Map<String, LinkInfo>): StyledText {
    val found = ArrayList<LinkRange>()
    val text = AnnotatedString.Builder()
    for (span in spans) {
        if (span.text.isEmpty()) continue
        val link = span.link?.let { links[it] }
        val start = text.length
        text.pushStyle(styleOf(span, paper, link != null))
        text.append(span.text)
        text.pop()
        if (link != null) found += LinkRange(start, text.length, link)
    }
    return StyledText(text.toAnnotatedString(), found)
}

/** Most links of one text that a screen reader is offered as actions. */
internal const val MAX_LINK_ACTIONS = 8

/** What a screen reader says for each link of a text: where it really goes. */
internal fun linkActionLabels(texts: Texts, links: List<LinkRange>): List<String> =
    links.map { it.link }.distinctBy { it.shown }.take(MAX_LINK_ACTIONS).map { "${texts.t("Open link")}: ${it.shown}" }

private fun linkAt(layout: TextLayoutResult?, text: StyledText, at: Offset): LinkInfo? =
    layout?.let { text.linkAt(it.getOffsetForPosition(at)) }

/**
 * Text that has links in it. A tap on a link follows it (the owner's phone opens it in the
 * browser, the composer or the dialer); a long press shows where it really goes first. A screen
 * reader is offered each link as an action that names its real destination.
 */
@Composable
internal fun LinkedText(
    styledText: StyledText,
    style: TextStyle,
    env: BodyEnv,
    modifier: Modifier = Modifier,
) {
    var layout by remember { mutableStateOf<TextLayoutResult?>(null) }
    val links = styledText.links
    var base = modifier
    if (links.isNotEmpty()) {
        val labels = linkActionLabels(env.texts, links)
        val distinct = links.map { it.link }.distinctBy { it.shown }.take(MAX_LINK_ACTIONS)
        base = base
            .pointerInput(styledText) {
                detectTapGestures(
                    onTap = { at -> linkAt(layout, styledText, at)?.let(env.onLink) },
                    onLongPress = { at -> linkAt(layout, styledText, at)?.let(env.onLinkDetails) },
                )
            }
            .semantics {
                customActions = distinct.mapIndexed { n, link ->
                    CustomAccessibilityAction(labels[n]) {
                        env.onLink(link)
                        true
                    }
                }
            }
    }
    BasicText(text = styledText.text, style = style, onTextLayout = { layout = it }, modifier = base)
}
