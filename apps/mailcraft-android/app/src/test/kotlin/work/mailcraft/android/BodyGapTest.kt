// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.TableView

/** A picture that was not downloaded never leaves empty space (#1966). */
class BodyGapTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5.copy(screenHeight = 6_000),
        renderingMode = SessionParams.RenderingMode.NORMAL,
        snapshotHandler = noPictures(),
    )

    private val env = BodyEnv(EnglishTexts, emptyMap(), emptyMap(), Color.Gray, {}, {})

    /** The height in dp [table] is drawn at, 360 dp wide. */
    private fun heightOf(table: TableView): Float {
        var height = -1f
        paparazzi.snapshot {
            val density = LocalDensity.current.density
            Measured({ height = it / density }) {
                TableBlock(table, AlignView.LEFT, env, Paper.of(WHITE, LINK), 360.dp)
            }
        }
        return height
    }

    @Composable
    private fun Measured(onHeight: (Float) -> Unit, content: @Composable () -> Unit) {
        Box(modifier = Modifier.onGloballyPositioned { onHeight(it.size.height.toFloat()) }) { content() }
    }

    @Test
    fun aTintedRowIsNoTallerThanTheSameRowUntinted() {
        val plain = heightOf(GapCorpus.table(tint = null))
        val tinted = heightOf(GapCorpus.table())
        assertTrue("not drawn", plain > 0f)
        assertEquals("a row's colour changed its height", plain, tinted, 1f)
    }

    @Test
    fun aPlaceholderKeepsItsShapeWithinTheRoomAndALimitedHeight() {
        assertEquals(DpSize(24.dp, 24.dp), placeholderSize(24, 24, 360.dp))
        assertEquals(DpSize(300.dp, 75.dp), placeholderSize(560, 140, 300.dp))
        assertEquals(DpSize(360.dp, MAX_PLACEHOLDER_HEIGHT), placeholderSize(600, 1200, 360.dp))
        assertEquals(DpSize(1.dp, 1.dp), placeholderSize(0, 0, 360.dp))
    }

    private companion object {
        const val WHITE = 0xFFFFFF
        const val LINK = 0x0645AD
    }
}
