// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.TableView

/** A row of a layout table or a data table as tall as a message can make it is held, never a crash (#1966). */
class TallRowsTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        snapshotHandler = noPictures(),
    )

    private val env = BodyEnv(EnglishTexts, emptyMap(), emptyMap(), Color.Gray, {}, {})

    private var density = 1f

    /** The height in px [table] is drawn at in [room]. */
    private fun heightOf(table: TableView, room: Dp = 360.dp): Int {
        var height = -1
        paparazzi.snapshot {
            density = LocalDensity.current.density
            Box(modifier = Modifier.onGloballyPositioned { height = it.size.height }) {
                TableBlock(table, AlignView.LEFT, env, Paper.of(WHITE, LINK), room)
            }
        }
        return height
    }

    @Test
    fun aTallTintedLayoutCellIsHeld() {
        val height = heightOf(TablesRtl.tintedNote(6_000))
        assertTrue("height $height", height in 1..MAX_BOX_HEIGHT)
    }

    @Test
    fun aTallTintedLayoutCellInADataCellIsHeld() {
        val note = B.cell(B.tableBlock(TablesRtl.tintedNote(6_000)))
        val height = heightOf(B.table(layout = false, border = true, rows = listOf(listOf(note))))
        assertTrue("height $height", height in 1..MAX_BOX_HEIGHT)
    }

    @Test
    fun aPictureInADataCellKeepsItsWidth() {
        // A picture of 200 by 100 dp beside a short word in 328 dp: its column has the 216 dp it asks
        // for, and the picture all 200 dp of it (100 dp tall), not its cell's half of the room (74 dp).
        val picture = B.cell(B.image("https://cdn-images.example/tray.png", "Seed tray", 200u, 100u))
        val table = B.table(layout = false, border = true, rows = listOf(listOf(picture, B.cell(B.p("Tray", 0u)))))
        val height = heightOf(table, 328.dp)
        assertTrue("height $height", height >= 100 * density)
    }

    private companion object {
        const val WHITE = 0xFFFFFF
        const val LINK = 0x0645AD
    }
}
