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
import uniffi.mailcraft_mobile.ComposeParagraph
import uniffi.mailcraft_mobile.ComposeSpan
import uniffi.mailcraft_mobile.ListMark
import uniffi.mailcraft_mobile.RecipientLine

private const val TOLERANCE = 0.1
private val PhoneTall = DeviceConfig.PIXEL_5.copy(screenHeight = 2_400)

/** The composer as another app's share leaves it: invented files and text, `example.org` only. */
internal object ShareShots {
    private const val LINK = "https://example.org/garden/north-bed"
    private const val BODY = "Worth a look\n$LINK"

    private fun ui(files: List<ComposeFile>, text: String = BODY, subject: String = "North bed", skipped: String? = null): ComposeUi {
        val link = text.indexOf(LINK)
        val spans = if (link >= 0) listOf(ComposeSpan(link.toUInt(), (link + LINK.length).toUInt(), bold = false, italic = false, underline = true, link = LINK)) else emptyList()
        val paragraphs = if (text.isEmpty()) emptyList() else listOf(ComposeParagraph(0u, text.length.toUInt(), ListMark.NONE, 0u.toUByte(), 0u, false))
        val view = composeView(text = text, subject = subject, files = files, spans = spans, paragraphs = paragraphs, signature = "", dirty = true)
        return ComposeUi(
            view = view,
            body = TextFieldValue(text, TextRange(text.length)),
            subject = subject,
            people = mapOf(RecipientLine.TO to emptyList(), RecipientLine.CC to emptyList(), RecipientLine.BCC to emptyList()),
            accounts = Cast.accounts,
            skipped = skipped,
            edits = 1,
        )
    }

    fun oneFile() = ui(listOf(ComposeFile(0u, "Planting plan for the north bed.pdf", "application/pdf", 1_240_000uL)))

    fun severalFiles() = ui(
        listOf(
            ComposeFile(0u, "IMG_0412.jpg", "image/jpeg", 3_200_000uL),
            ComposeFile(1u, "IMG_0413.jpg", "image/jpeg", 2_900_000uL),
            ComposeFile(2u, "Soil notes.txt", "text/plain", 2_400uL),
            ComposeFile(3u, "picture.png", "image/png", 480_000uL),
        ),
        text = "",
        subject = "",
    )

    /** Nothing could be attached: the composer opens empty and says why, in the words the app uses. */
    fun refused(): ComposeUi {
        val why = refusalText(
            EnglishTexts,
            listOf(Refusal(Why.TooLarge, "disk-image.iso"), Refusal(Why.Unreadable), Refusal(Why.TooMany, count = 3)),
        )
        return ui(emptyList(), text = "", subject = "", skipped = why)
    }
}

@Composable
private fun Screen(ui: ComposeUi) {
    CompositionLocalProvider(LocalDecodeInPlace provides true) {
        MailCraftTheme { Surface { ComposerScreen(EnglishTexts, ui, ComposerActions.None, ComposerPickers.None) } }
    }
}

/** The composer opened from a share: one file with a link, several files, and a share that was refused. */
class ComposerShareScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(deviceConfig = PhoneTall, renderingMode = SessionParams.RenderingMode.NORMAL, maxPercentDifference = TOLERANCE)

    private fun shoot(name: String, ui: ComposeUi) {
        for ((suffix, night, scale) in listOf(Triple("light_100", false, 1f), Triple("dark_200", true, 2f))) {
            paparazzi.unsafeUpdateConfig(PhoneTall.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = scale))
            paparazzi.snapshot("composer_share_${name}_phone_$suffix") { Screen(ui) }
        }
    }

    @Test
    fun oneFile() = shoot("one_file", ShareShots.oneFile())

    @Test
    fun severalFiles() = shoot("several_files", ShareShots.severalFiles())

    @Test
    fun refused() = shoot("refused", ShareShots.refused())
}
