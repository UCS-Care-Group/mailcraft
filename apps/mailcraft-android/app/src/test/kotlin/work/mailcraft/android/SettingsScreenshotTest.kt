// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.runtime.Composable
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test

/** Each Settings section, light and dark, at the system's text size and twice that. */
class SettingsScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    // A long page is drawn on a taller screen, so that the shot holds all of it at twice the text size.
    private fun shoot(name: String, night: Boolean, fontScale: Float, height: Int? = null, content: @Composable () -> Unit) {
        val device = DeviceConfig.PIXEL_5
        paparazzi.unsafeUpdateConfig(
            device.copy(
                nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT,
                fontScale = fontScale,
                screenHeight = height ?: device.screenHeight,
            ),
        )
        paparazzi.snapshot(name) { content() }
    }

    private fun four(name: String, tallAtTwice: Boolean = false, content: @Composable () -> Unit) {
        shoot("${name}_light_100", false, 1f, content = content)
        shoot("${name}_dark_100", true, 1f, content = content)
        shoot("${name}_light_200", false, 2f, if (tallAtTwice) TALL else null, content)
        shoot("${name}_dark_200", true, 2f, if (tallAtTwice) TALL else null, content)
    }

    @Test
    fun rowsLight100() = shoot("settings_rows_light_100", false, 1f) { SettingsShots.Page(null) }

    @Test
    fun rowsDark200() = shoot("settings_rows_dark_200", true, 2f, TALL) { SettingsShots.Page(null) }

    @Test
    fun mailOfflineLight100() = shoot("settings_offline_light_100", false, 1f, TALL) { SettingsShots.Page(SettingsPage.MailOffline) }

    @Test
    fun mailOfflineDark100() = shoot("settings_offline_dark_100", true, 1f, TALL) { SettingsShots.Page(SettingsPage.MailOffline) }

    @Test
    fun mailOfflineLight200() = shoot("settings_offline_light_200", false, 2f, OFFLINE_TALL) { SettingsShots.Page(SettingsPage.MailOffline) }

    @Test
    fun mailOfflineDark200() = shoot("settings_offline_dark_200", true, 2f, OFFLINE_TALL) { SettingsShots.Page(SettingsPage.MailOffline) }

    @Test
    fun notifications() = four("settings_notifications", tallAtTwice = true) { SettingsShots.Page(SettingsPage.Notifications) }

    @Test
    fun pictures() = four("settings_pictures", tallAtTwice = true) { SettingsShots.Page(SettingsPage.Pictures) }

    @Test
    fun senderImages() = four("settings_sender_images", tallAtTwice = true) { SettingsShots.Page(SettingsPage.SenderImages) }

    @Test
    fun signatures() = four("settings_signatures", tallAtTwice = true) { SettingsShots.Page(SettingsPage.Signatures) }

    @Test
    fun about() = four("settings_about", tallAtTwice = true) { SettingsShots.Page(SettingsPage.About) }

    @Test
    fun licences() = four("settings_licences", tallAtTwice = true) { SettingsShots.Licences() }

    @Test
    fun spaceQuestion() = four("settings_space_question") { SettingsShots.Space() }

    @Test
    fun bigSpaceQuestionDark200() = shoot("settings_space_question_big_dark_200", true, 2f) { SettingsShots.BigSpace() }
}

// The goldens are drawn on one machine and checked on another; anti-aliasing may differ by a hair.
private const val TOLERANCE = 0.1
private const val TALL = 5200
private const val OFFLINE_TALL = 8000
