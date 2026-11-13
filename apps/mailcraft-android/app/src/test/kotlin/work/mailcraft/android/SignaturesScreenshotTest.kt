// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test
import uniffi.mailcraft_mobile.FromChoice

private val invented = listOf(
    AccountSignature(FromChoice(1uL, "Ada Example", "ada@example.org", false), "Ada Example\nExample Works"),
    AccountSignature(FromChoice(2uL, "Ada Example", "ada@example.test", false), ""),
)

/** Settings > Signatures, on invented accounts. */
class SignaturesScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = 0.1,
    )

    private fun shoot(name: String, night: Boolean, fontScale: Float, content: @Composable () -> Unit) {
        paparazzi.unsafeUpdateConfig(
            DeviceConfig.PIXEL_5.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = fontScale),
        )
        paparazzi.snapshot(name) { MailCraftTheme { Surface { content() } } }
    }

    @Test
    fun light100() = shoot("signatures_light_100", false, 1f) { SignaturesList(EnglishTexts, invented, null) }

    @Test
    fun dark200() = shoot("signatures_dark_200", true, 2f) { SignaturesList(EnglishTexts, invented, null) }

    @Test
    fun none() = shoot("signatures_none_light_100", false, 1f) { SignaturesList(EnglishTexts, emptyList(), null) }
}
