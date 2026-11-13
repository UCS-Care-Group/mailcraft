// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test
import uniffi.mailcraft_mobile.ComposeFile
import uniffi.mailcraft_mobile.ComposeKind
import uniffi.mailcraft_mobile.ComposeParagraph
import uniffi.mailcraft_mobile.ComposeSpan
import uniffi.mailcraft_mobile.ComposeView
import uniffi.mailcraft_mobile.ListMark
import uniffi.mailcraft_mobile.RecipientLine
import uniffi.mailcraft_mobile.SuggestionRecord

private const val TOLERANCE = 0.1
private val PhoneTall = DeviceConfig.PIXEL_5.copy(screenHeight = 2_400)

/** The composer on invented messages: a new one, a reply that quotes and a forward with a file. */
internal object ComposerShots {
    private class Part(val text: String, val list: ListMark = ListMark.NONE, val signature: Boolean = false)

    private fun bodyOf(vararg parts: Part): Triple<String, List<ComposeParagraph>, Int> {
        val paragraphs = mutableListOf<ComposeParagraph>()
        val text = StringBuilder()
        parts.forEachIndexed { index, part ->
            if (index > 0) text.append('\n')
            val start = text.length
            text.append(part.text)
            paragraphs += ComposeParagraph(start.toUInt(), text.length.toUInt(), part.list, 0u.toUByte(), 0u, part.signature)
        }
        return Triple(text.toString(), paragraphs, 0)
    }

    private fun ui(view: ComposeView, caret: Int, vararg extra: Pair<String, String>) = ComposeUi(
        view = view,
        body = TextFieldValue(view.text, TextRange(caret)),
        subject = view.subject,
        people = mapOf(RecipientLine.TO to view.to, RecipientLine.CC to view.cc, RecipientLine.BCC to view.bcc),
        accounts = Cast.accounts,
        saved = extra.isEmpty(),
    )

    fun newMessage(): ComposeUi {
        val (text, paragraphs, _) = bodyOf(
            Part("Hello Ada,"),
            Part(""),
            Part("Here is the planting list for Saturday:"),
            Part("ferns", ListMark.BULLET),
            Part("moss", ListMark.BULLET),
            Part(""),
            Part("-- ", signature = true),
            Part("Avery Stone", signature = true),
        )
        val bold = text.indexOf("planting list")
        val view = composeView(
            text = text, to = listOf(Cast.ada, Cast.broken), cc = listOf(Cast.bo), subject = "Garden plan", paragraphs = paragraphs,
            spans = listOf(ComposeSpan(bold.toUInt(), (bold + "planting list".length).toUInt(), bold = true, italic = false, underline = false, link = null)),
            signature = "Avery", dirty = true,
        )
        return ui(view, caret = 10)
    }

    fun reply(): ComposeUi {
        val (text, paragraphs, _) = bodyOf(Part("Thanks, ten works for me."), Part(""), Part("-- ", signature = true), Part("Avery Stone", signature = true))
        val view = composeView(
            text = text, kind = ComposeKind.REPLY, to = listOf(Cast.ada), subject = "RE: Garden plan", paragraphs = paragraphs, signature = "Avery",
            quote = "From: Ada Quill\nSent: Wednesday, 14 October 2026 09:00\nTo: Avery Stone\nSubject: Garden plan\n\nSee you at ten at the north gate. Bring gloves and the spare spade.",
            dirty = true,
        )
        return ui(view, caret = 25)
    }

    fun forward(): ComposeUi {
        val (text, paragraphs, _) = bodyOf(Part("FYI"), Part(""), Part("-- ", signature = true), Part("Avery Stone", signature = true))
        val view = composeView(
            text = text, kind = ComposeKind.FORWARD, subject = "FW: Garden plan", paragraphs = paragraphs, signature = "Avery", account = 2uL,
            files = listOf(ComposeFile(0u, "Planting plan for the north bed.pdf", "application/pdf", 1_240_000uL)),
            quote = "From: Ada Quill\nSent: Wednesday, 14 October 2026 09:00\nSubject: Garden plan\n\nSee you at ten at the north gate.",
            dirty = true,
        )
        return ui(view, caret = 3, "typing" to "bo").copy(
            typed = mapOf(RecipientLine.TO to "bo"),
            suggestFor = RecipientLine.TO,
            suggestions = listOf(SuggestionRecord("Bo Marsh", "bo@example.org", contact = true), SuggestionRecord("", "bobbin@example.org", contact = false)),
        )
    }
}

@Composable
private fun Screen(ui: ComposeUi) {
    CompositionLocalProvider(LocalDecodeInPlace provides true) {
        MailCraftTheme { Surface { ComposerScreen(EnglishTexts, ui, ComposerActions.None, ComposerPickers.None) } }
    }
}

/** The composer on a phone: light at the system's text size, dark at twice that. */
class ComposerPhoneScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(deviceConfig = PhoneTall, renderingMode = SessionParams.RenderingMode.NORMAL, maxPercentDifference = TOLERANCE)

    private fun shoot(name: String, ui: ComposeUi) {
        for ((suffix, night, scale) in listOf(Triple("light_100", false, 1f), Triple("dark_200", true, 2f))) {
            paparazzi.unsafeUpdateConfig(PhoneTall.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = scale))
            paparazzi.snapshot("composer_${name}_phone_$suffix") { Screen(ui) }
        }
    }

    @Test
    fun newMessage() = shoot("new", ComposerShots.newMessage())

    @Test
    fun reply() = shoot("reply", ComposerShots.reply())

    @Test
    fun forward() = shoot("forward", ComposerShots.forward())
}

/** The composer on a tablet: a column in the middle of the window. */
class ComposerTabletScreenshotTest {
    private val tablet = DeviceConfig.NEXUS_10

    @get:Rule
    val paparazzi = Paparazzi(deviceConfig = tablet, renderingMode = SessionParams.RenderingMode.NORMAL, maxPercentDifference = TOLERANCE)

    private fun shoot(name: String, ui: ComposeUi) {
        for ((suffix, night, scale) in listOf(Triple("light_100", false, 1f), Triple("dark_200", true, 2f))) {
            paparazzi.unsafeUpdateConfig(tablet.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = scale))
            paparazzi.snapshot("composer_${name}_tablet_$suffix") { Screen(ui) }
        }
    }

    @Test
    fun newMessage() = shoot("new", ComposerShots.newMessage())

    @Test
    fun reply() = shoot("reply", ComposerShots.reply())

    @Test
    fun forward() = shoot("forward", ComposerShots.forward())
}
