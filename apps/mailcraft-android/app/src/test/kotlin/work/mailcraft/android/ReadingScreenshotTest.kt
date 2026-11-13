// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength", "LongParameterList")

package work.mailcraft.android

import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import kotlinx.coroutines.Dispatchers
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test

private const val TOLERANCE = 0.1

/** A phone tall enough to show a whole invented message, so that the shot shows everything it draws. */
private val Tall = DeviceConfig.PIXEL_5.copy(screenHeight = 3_400)

internal object ReadingShots {
    fun ui(
        entry: Corpus.Entry,
        junk: Boolean = false,
        vouched: Boolean = true,
        access: PictureAccess = PictureAccess.Blocked,
        pictures: Map<String, PictureState> = emptyMap(),
        waiting: Boolean = false,
    ) = ReadingUi(
        id = entry.view.id,
        load = LoadState.Ready,
        opened = Corpus.opened(entry, junk, vouched),
        access = access,
        pictures = pictures,
        waiting = waiting,
    )

    @Composable
    fun Pane(ui: ReadingUi) {
        // (The pictures are decoded where the drawing is, so that the shot has them.)
        CompositionLocalProvider(LocalDecoder provides Dispatchers.Unconfined) {
            MailCraftTheme {
                Surface {
                    ReadingPane(EnglishTexts, ui.id ?: 0uL, ui, ReadingActions.None)
                }
            }
        }
    }
}

/** The five corpus messages in the reading pane on a phone: light and dark, at the system's text size and at twice that. */
class ReadingScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = Tall,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private fun shoot(name: String, ui: ReadingUi) {
        for ((suffix, night, scale) in listOf(
            Triple("light_100", false, 1f),
            Triple("dark_100", true, 1f),
            Triple("light_200", false, 2f),
            Triple("dark_200", true, 2f),
        )) {
            paparazzi.unsafeUpdateConfig(Tall.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = scale))
            paparazzi.snapshot("reading_${name}_$suffix") { ReadingShots.Pane(ui) }
        }
    }

    @Test
    fun plain() = shoot("plain", ReadingShots.ui(Corpus.plain))

    @Test
    fun newsletterLayoutTable() = shoot("newsletter", ReadingShots.ui(Corpus.newsletter))

    @Test
    fun dataTable() = shoot("data_table", ReadingShots.ui(Corpus.dataTable))

    @Test
    fun quotedReply() = shoot("quoted_reply", ReadingShots.ui(Corpus.reply))

    @Test
    fun longLinks() = shoot("long_links", ReadingShots.ui(Corpus.longLinks))
}

/** The reading pane on a tablet: the list's place beside it is the list's own shot. */
class ReadingTabletScreenshotTest {
    private val tablet = DeviceConfig.NEXUS_10

    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = tablet,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    @Test
    fun newsletterOnATablet() {
        paparazzi.unsafeUpdateConfig(tablet.copy(nightMode = NightMode.NOTNIGHT, fontScale = 1f))
        paparazzi.snapshot("reading_tablet_newsletter_light_100") { ReadingShots.Pane(ReadingShots.ui(Corpus.newsletter)) }
    }

    @Test
    fun dataTableOnATabletDark() {
        paparazzi.unsafeUpdateConfig(tablet.copy(nightMode = NightMode.NIGHT, fontScale = 1f))
        paparazzi.snapshot("reading_tablet_data_dark_100") { ReadingShots.Pane(ReadingShots.ui(Corpus.dataTable)) }
    }
}

/** The banner, the placeholders and the pictures that came, on a phone. */
class PicturesScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = Tall,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private fun shoot(name: String, night: Boolean, fontScale: Float, content: @Composable () -> Unit) {
        paparazzi.unsafeUpdateConfig(Tall.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = fontScale))
        paparazzi.snapshot(name) { content() }
    }

    private fun placeholders(): Corpus.Entry {
        val blocks = listOf(
            B.p("A picture of a declared size is a plain box of that size:", 0u),
            B.image("https://img.shop.example/a.png", "Fern Hollow nursery", 560u, 120u),
            B.p("One with no size is its description, in line:"),
            B.image("https://img.shop.example/b.png", "A hardy fern in a pot"),
            B.p("A tracking pixel and a picture with no description are not shown at all:"),
            B.image("https://tracker.example/p.gif", "", 1u, 1u),
            B.image("https://img.shop.example/c.png", ""),
            B.p("A small box:"),
            B.image("https://img.shop.example/d.png", "Logo", 80u, 40u),
        )
        return Corpus.Entry("placeholders", Corpus.newsletter.view.copy(id = 20uL, blocks = blocks, remotePictures = 4u), emptyList())
    }

    @Test
    fun bannerAndPlaceholdersLight100() =
        shoot("pictures_banner_light_100", false, 1f) { ReadingShots.Pane(ReadingShots.ui(placeholders())) }

    @Test
    fun bannerAndPlaceholdersDark200() =
        shoot("pictures_banner_dark_200", true, 2f) { ReadingShots.Pane(ReadingShots.ui(placeholders())) }

    @Test
    fun bannerInJunkOffersThisMessageOnlyLight100() =
        shoot("pictures_banner_junk_light_100", false, 1f) { ReadingShots.Pane(ReadingShots.ui(placeholders(), junk = true)) }

    @Test
    fun bannerWhenNothingVouchesDark100() =
        shoot("pictures_banner_unvouched_dark_100", true, 1f) { ReadingShots.Pane(ReadingShots.ui(placeholders(), vouched = false)) }

    @Test
    fun dataSaverWaitsLight100() = shoot("pictures_waiting_light_100", false, 1f) {
        ReadingShots.Pane(
            ReadingShots.ui(
                placeholders(), access = PictureAccess.Allowed, waiting = true,
                pictures = mapOf("https://img.shop.example/a.png" to PictureState.Waiting),
            ),
        )
    }

    @Test
    fun dataSaverWaitsDark200() = shoot("pictures_waiting_dark_200", true, 2f) {
        ReadingShots.Pane(ReadingShots.ui(placeholders(), access = PictureAccess.Allowed, waiting = true))
    }

    @Test
    fun picturesThatCameLight100() = shoot("pictures_ready_light_100", false, 1f) {
        ReadingShots.Pane(
            ReadingShots.ui(
                placeholders(), access = PictureAccess.Allowed,
                pictures = mapOf(
                    "https://img.shop.example/a.png" to PictureState.Ready(B.png),
                    "https://img.shop.example/b.png" to PictureState.Failed,
                    "https://img.shop.example/d.png" to PictureState.Ready(B.png),
                ),
            ),
        )
    }

    @Test
    fun picturesThatCameDark100() = shoot("pictures_ready_dark_100", true, 1f) {
        ReadingShots.Pane(
            ReadingShots.ui(
                placeholders(), access = PictureAccess.Allowed,
                pictures = mapOf("https://img.shop.example/a.png" to PictureState.Ready(B.png)),
            ),
        )
    }

    private val shell = ShellShots.data.copy(
        pictures = PicturesUi(PicturePrefs(senders = setOf("news@shop.example"), domains = setOf("letters.example")), 12, 3_400_000),
    )

    @Test
    fun picturesSettingsLight100() = shoot("pictures_settings_light_100", false, 1f) {
        ShellShots.Shell(NavState().openPage(SettingsPage.Pictures), with = shell)
    }

    @Test
    fun picturesSettingsDark200() = shoot("pictures_settings_dark_200", true, 2f) {
        ShellShots.Shell(NavState().openPage(SettingsPage.Pictures), with = shell)
    }

    @Test
    fun senderImagesSettingsLight100() = shoot("sender_images_settings_light_100", false, 1f) {
        ShellShots.Shell(NavState().openPage(SettingsPage.SenderImages), with = shell)
    }

    @Test
    fun senderImagesSettingsDark200() = shoot("sender_images_settings_dark_200", true, 2f) {
        ShellShots.Shell(NavState().openPage(SettingsPage.SenderImages), with = shell.copy(pictures = shell.pictures.copy(prefs = PicturePrefs(senderImages = true))))
    }

    @Test
    fun rowsWithSenderImagesLight100() = shoot("sender_images_rows_light_100", false, 1f) {
        ShellShots.Shell(
            with = ShellShots.data.copy(avatars = ShellShots.rows.filterIndexed { i, _ -> i % 2 == 0 }.associate { it.id to B.png }).withRows(),
        )
    }
}

private fun ShellData.withRows() = copy(
    mail = ShellShots.listUi(
        rows = ShellShots.rows.mapIndexed { i, r -> r.copy(address = if (i % 2 == 0) "ada@example.org" else "bo@example.org") },
    ),
)
