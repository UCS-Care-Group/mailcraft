// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.material3.DrawerValue
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test

/** The shell on a phone, light and dark, at the system's text size and at twice that. */
class PhoneShellScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private fun shoot(name: String, night: Boolean, fontScale: Float, nav: NavState = NavState(), drawer: DrawerValue = DrawerValue.Closed) {
        paparazzi.unsafeUpdateConfig(
            DeviceConfig.PIXEL_5.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = fontScale),
        )
        paparazzi.snapshot(name) { ShellShots.Shell(nav, drawer) }
    }

    @Test
    fun mailLight100() = shoot("phone_mail_light_100", night = false, fontScale = 1f)

    @Test
    fun mailLight200() = shoot("phone_mail_light_200", night = false, fontScale = 2f)

    @Test
    fun mailDark100() = shoot("phone_mail_dark_100", night = true, fontScale = 1f)

    @Test
    fun mailDark200() = shoot("phone_mail_dark_200", night = true, fontScale = 2f)

    @Test
    fun drawerLight100() = shoot("phone_drawer_light_100", night = false, fontScale = 1f, drawer = DrawerValue.Open)

    @Test
    fun drawerDark200() = shoot("phone_drawer_dark_200", night = true, fontScale = 2f, drawer = DrawerValue.Open)

    @Test
    fun settingsLight100() = shoot("phone_settings_light_100", night = false, fontScale = 1f, nav = NavState().openSettings())

    @Test
    fun settingsDark200() = shoot("phone_settings_dark_200", night = true, fontScale = 2f, nav = NavState().openSettings())

    @Test
    fun languageLight100() =
        shoot("phone_language_light_100", night = false, fontScale = 1f, nav = NavState().openPage(SettingsPage.Language))

    @Test
    fun todayDark100() = shoot("phone_today_dark_100", night = true, fontScale = 1f, nav = NavState(tab = Tab.Today))
}

/** The shell on a tablet (an expanded window): the rail of five items instead of the bottom bar. */
class TabletShellScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.NEXUS_10,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private fun shoot(name: String, night: Boolean, fontScale: Float) {
        paparazzi.unsafeUpdateConfig(
            DeviceConfig.NEXUS_10.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = fontScale),
        )
        paparazzi.snapshot(name) { ShellShots.Shell() }
    }

    @Test
    fun mailLight100() = shoot("tablet_mail_light_100", night = false, fontScale = 1f)

    @Test
    fun mailLight200() = shoot("tablet_mail_light_200", night = false, fontScale = 2f)

    @Test
    fun mailDark100() = shoot("tablet_mail_dark_100", night = true, fontScale = 1f)

    @Test
    fun mailDark200() = shoot("tablet_mail_dark_200", night = true, fontScale = 2f)
}

// The goldens are drawn on one machine and checked on another; anti-aliasing may differ by a hair.
private const val TOLERANCE = 0.1
