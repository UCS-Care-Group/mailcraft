// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MagicNumber")

package work.mailcraft.android

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathBuilder
import androidx.compose.ui.graphics.vector.path
import androidx.compose.ui.unit.dp

/**
 * The app's icons, drawn here as plain strokes on a 24 by 24 grid: our own drawings, no picture
 * file and nobody else's artwork. Each is tinted by whoever shows it.
 */
internal object AppIcons {
    val Menu: ImageVector by lazy {
        icon("Menu") {
            line(4f, 7f, 20f, 7f)
            line(4f, 12f, 20f, 12f)
            line(4f, 17f, 20f, 17f)
        }
    }

    val Back: ImageVector by lazy {
        icon("Back") {
            line(20f, 12f, 5f, 12f)
            moveTo(11f, 6f)
            lineTo(5f, 12f)
            lineTo(11f, 18f)
        }
    }

    /** A sun: the day. */
    val Today: ImageVector by lazy {
        icon("Today") {
            circle(12f, 12f, 4f)
            line(12f, 2.5f, 12f, 5f)
            line(12f, 19f, 12f, 21.5f)
            line(2.5f, 12f, 5f, 12f)
            line(19f, 12f, 21.5f, 12f)
            line(5.3f, 5.3f, 7f, 7f)
            line(17f, 17f, 18.7f, 18.7f)
            line(18.7f, 5.3f, 17f, 7f)
            line(7f, 17f, 5.3f, 18.7f)
        }
    }

    /** An envelope. */
    val Mail: ImageVector by lazy {
        icon("Mail") {
            roundedBox(3f, 5f, 21f, 19f, 2f)
            moveTo(3.5f, 6.5f)
            lineTo(12f, 13f)
            lineTo(20.5f, 6.5f)
        }
    }

    /** A page of days with two rings. */
    val Calendar: ImageVector by lazy {
        icon("Calendar") {
            roundedBox(4f, 5f, 20f, 20f, 2f)
            line(4f, 10f, 20f, 10f)
            line(8.5f, 3f, 8.5f, 7f)
            line(15.5f, 3f, 15.5f, 7f)
            line(8f, 14f, 8.01f, 14f)
            line(12f, 14f, 12.01f, 14f)
            line(16f, 14f, 16.01f, 14f)
        }
    }

    /** Two people. */
    val People: ImageVector by lazy {
        icon("People") {
            circle(9f, 8f, 3.2f)
            moveTo(3f, 20f)
            arcTo(6f, 6f, 0f, false, true, 15f, 20f)
            circle(17f, 9f, 2.4f)
            moveTo(17f, 14f)
            arcTo(4.5f, 4.5f, 0f, false, true, 21.5f, 18.5f)
        }
    }

    /** A list with its ticks. */
    val Tasks: ImageVector by lazy {
        icon("Tasks") {
            moveTo(3.5f, 6.5f)
            lineTo(5f, 8f)
            lineTo(7.5f, 5f)
            moveTo(3.5f, 12.5f)
            lineTo(5f, 14f)
            lineTo(7.5f, 11f)
            moveTo(3.5f, 18.5f)
            lineTo(5f, 20f)
            lineTo(7.5f, 17f)
            line(11f, 7f, 20.5f, 7f)
            line(11f, 13f, 20.5f, 13f)
            line(11f, 19f, 20.5f, 19f)
        }
    }

    /** Three sliders. */
    val Settings: ImageVector by lazy {
        icon("Settings") {
            line(3f, 6f, 20f, 6f)
            line(3f, 12f, 20f, 12f)
            line(3f, 18f, 20f, 18f)
            circle(8f, 6f, 2.2f)
            circle(15f, 12f, 2.2f)
            circle(9f, 18f, 2.2f)
        }
    }

    /** A folder. */
    val Folder: ImageVector by lazy {
        icon("Folder") {
            moveTo(3f, 7f)
            lineTo(3f, 18f)
            arcTo(1.5f, 1.5f, 0f, false, false, 4.5f, 19.5f)
            lineTo(19.5f, 19.5f)
            arcTo(1.5f, 1.5f, 0f, false, false, 21f, 18f)
            lineTo(21f, 9f)
            arcTo(1.5f, 1.5f, 0f, false, false, 19.5f, 7.5f)
            lineTo(12f, 7.5f)
            lineTo(10f, 4.5f)
            lineTo(4.5f, 4.5f)
            arcTo(1.5f, 1.5f, 0f, false, false, 3f, 6f)
            close()
        }
    }

    /** One person, for an account. */
    val Account: ImageVector by lazy {
        icon("Account") {
            circle(12f, 8f, 4f)
            moveTo(4f, 20.5f)
            arcTo(8f, 8f, 0f, false, true, 20f, 20.5f)
        }
    }

    /** A tick, for the choice that is on. */
    val Check: ImageVector by lazy {
        icon("Check") {
            moveTo(5f, 12.5f)
            lineTo(10f, 17.5f)
            lineTo(19f, 7f)
        }
    }

    /** A pointing chevron, for a row that opens another page. */
    val Next: ImageVector by lazy {
        icon("Next") {
            moveTo(9f, 5f)
            lineTo(16f, 12f)
            lineTo(9f, 19f)
        }
    }
}

internal fun icon(name: String, build: PathBuilder.() -> Unit): ImageVector =
    ImageVector.Builder(
        name = name,
        defaultWidth = 24.dp,
        defaultHeight = 24.dp,
        viewportWidth = 24f,
        viewportHeight = 24f,
    ).path(
        stroke = SolidColor(Color.Black),
        strokeLineWidth = 2f,
        strokeLineCap = StrokeCap.Round,
        strokeLineJoin = StrokeJoin.Round,
        pathBuilder = build,
    ).build()

internal fun PathBuilder.line(x1: Float, y1: Float, x2: Float, y2: Float) {
    moveTo(x1, y1)
    lineTo(x2, y2)
}

internal fun PathBuilder.circle(cx: Float, cy: Float, r: Float) {
    moveTo(cx - r, cy)
    arcToRelative(r, r, 0f, true, true, 2 * r, 0f)
    arcToRelative(r, r, 0f, true, true, -2 * r, 0f)
    close()
}

internal fun PathBuilder.roundedBox(left: Float, top: Float, right: Float, bottom: Float, r: Float) {
    moveTo(left + r, top)
    lineTo(right - r, top)
    arcTo(r, r, 0f, false, true, right, top + r)
    lineTo(right, bottom - r)
    arcTo(r, r, 0f, false, true, right - r, bottom)
    lineTo(left + r, bottom)
    arcTo(r, r, 0f, false, true, left, bottom - r)
    lineTo(left, top + r)
    arcTo(r, r, 0f, false, true, left + r, top)
    close()
}
