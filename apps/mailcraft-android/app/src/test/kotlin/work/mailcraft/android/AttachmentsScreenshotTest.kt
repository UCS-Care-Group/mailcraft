// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test
import uniffi.mailcraft_mobile.OpenVerdict

private const val TOLERANCE = 0.1
private val Phone = DeviceConfig.PIXEL_5.copy(screenHeight = 2_000)

private val invented = listOf(
    item("Quarterly report.pdf", index = 0u, size = 1_240_000),
    item("Budget with macros.xlsm", OpenVerdict.MACROS, index = 1u, size = 86_000),
    item("Newsletter page.html", OpenVerdict.SCRIPTS, index = 2u, size = 12_300),
    item("Meeting invitation.ics", index = 3u, calendar = true, size = 1_900),
    item("setup.apk", OpenVerdict.BLOCKED, index = 4u, size = 9_800_000),
    item("A very long name for a scan of the signed agreement between the two companies, second copy.pdf", index = 5u, size = 3_400_000),
)

/** The attachment rows in the reading pane and the picture preview, on invented files. */
class AttachmentsScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = Phone,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private fun shoot(name: String, night: Boolean, fontScale: Float, content: @Composable () -> Unit) {
        paparazzi.unsafeUpdateConfig(Phone.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = fontScale))
        paparazzi.snapshot(name) { CompositionLocalProvider(LocalDecodeInPlace provides true) { MailCraftTheme { Surface { content() } } } }
    }

    private fun pane(): ReadingUi {
        val entry = Corpus.plain
        val facts = MessageFacts(entry.view.from.address, junk = false, vouched = true, known = false)
        return ReadingUi(id = entry.view.id, load = LoadState.Ready, opened = OpenedMessage(entry.view, facts, emptyMap(), invented))
    }

    @Test
    fun rowsLight100() = shoot("attachments_rows_light_100", false, 1f) { ReadingPane(EnglishTexts, 1uL, pane(), ReadingActions.None) }

    @Test
    fun rowsDark200() = shoot("attachments_rows_dark_200", true, 2f) { ReadingPane(EnglishTexts, 1uL, pane(), ReadingActions.None) }

    private fun preview() = Preview(1uL, item("Garden plan.png"), B.png)

    @Test
    fun previewLight100() = shoot("attachments_preview_light_100", false, 1f) { PreviewContent(EnglishTexts, preview(), AttachmentActions.None) }

    @Test
    fun previewDark200() = shoot("attachments_preview_dark_200", true, 2f) { PreviewContent(EnglishTexts, preview(), AttachmentActions.None) }
}
