// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.BlockKindView
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
    fun aPictureBoxKeepsItsShapeWithinTheRoomAndALimitedHeight() {
        assertEquals(IntSize(24, 24), boxSize(24, 360, 1f, Int.MAX_VALUE))
        assertEquals(IntSize(300, 75), boxSize(560, 300, 0.25f, Int.MAX_VALUE))
        assertEquals(IntSize(360, 120), boxSize(600, 360, 2f, 120))
        assertEquals(IntSize(1, 1), boxSize(0, 0, 0f, 0))
    }

    private val extremes = listOf(Int.MIN_VALUE, -1, 0, 1, 24, 8_000, 40_000_000, Int.MAX_VALUE)
    private val shapes = listOf(-1f, 0f, 1e-7f, 1f, 4e7f, Float.MAX_VALUE, Float.POSITIVE_INFINITY, Float.NaN)

    @Test
    fun anyPictureBoxIsOneComposeCanLayOut() {
        for (wanted in extremes) for (room in extremes) boxesFit(wanted, room)
    }

    private fun boxesFit(wanted: Int, room: Int) {
        for (tall in shapes) for (cap in extremes) {
            val size = boxSize(wanted, room, tall, cap)
            assertTrue("$wanted $room $tall $cap -> $size", size.width >= 1 && size.height >= 1)
            Constraints.fixed(size.width, size.height)
        }
    }

    @Test
    fun aPlaceholderInLessRoomThanItWasToldKeepsItsShape() {
        val block = B.image("https://img.shop.example/a.png", "Fern Hollow nursery", 560u, 140u)
        val kind = block.kind as BlockKindView.Image
        var height = -1f
        paparazzi.snapshot {
            val density = LocalDensity.current.density
            Box(modifier = Modifier.width(100.dp)) {
                Measured({ height = it / density }) { PictureBlock(block, kind, env, Paper.of(WHITE, LINK), 360.dp) }
            }
        }
        assertEquals(25f, height, 1f)
    }

    private companion object {
        const val WHITE = 0xFFFFFF
        const val LINK = 0x0645AD
    }
}
