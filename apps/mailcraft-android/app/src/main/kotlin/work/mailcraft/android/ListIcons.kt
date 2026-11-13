// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MagicNumber")

package work.mailcraft.android

import androidx.compose.ui.graphics.vector.ImageVector

/** The mail list's icons, drawn like [AppIcons]: our own strokes on a 24 by 24 grid. */
internal object ListIcons {
    /** A magnifying glass. */
    val Search: ImageVector by lazy {
        icon("Search") {
            circle(10.5f, 10.5f, 6.5f)
            line(15.5f, 15.5f, 21f, 21f)
        }
    }

    /** A funnel. */
    val Filter: ImageVector by lazy {
        icon("Filter") {
            moveTo(3.5f, 5f)
            lineTo(20.5f, 5f)
            lineTo(14f, 12.5f)
            lineTo(14f, 19f)
            lineTo(10f, 17f)
            lineTo(10f, 12.5f)
            close()
        }
    }

    /** A cross. */
    val Close: ImageVector by lazy {
        icon("Close") {
            line(6f, 6f, 18f, 18f)
            line(18f, 6f, 6f, 18f)
        }
    }

    /** A flag on a pole. */
    val Flag: ImageVector by lazy {
        icon("Flag") {
            line(5.5f, 21f, 5.5f, 3.5f)
            moveTo(5.5f, 4.5f)
            lineTo(19f, 4.5f)
            lineTo(15.5f, 9f)
            lineTo(19f, 13.5f)
            lineTo(5.5f, 13.5f)
        }
    }

    /** A paperclip. */
    val Attachment: ImageVector by lazy {
        icon("Attachment") {
            moveTo(19f, 11.5f)
            lineTo(11.5f, 19f)
            arcTo(4.5f, 4.5f, 0f, false, true, 5f, 12.5f)
            lineTo(13f, 4.5f)
            arcTo(3f, 3f, 0f, false, true, 17.3f, 8.8f)
            lineTo(9.5f, 16.6f)
            arcTo(1.5f, 1.5f, 0f, false, true, 7.4f, 14.5f)
            lineTo(14f, 7.9f)
        }
    }

    /** A box with a lid. */
    val Archive: ImageVector by lazy {
        icon("Archive") {
            roundedBox(3f, 4f, 21f, 9f, 1.5f)
            moveTo(4.5f, 9f)
            lineTo(4.5f, 18f)
            arcTo(1.5f, 1.5f, 0f, false, false, 6f, 19.5f)
            lineTo(18f, 19.5f)
            arcTo(1.5f, 1.5f, 0f, false, false, 19.5f, 18f)
            lineTo(19.5f, 9f)
            line(9.5f, 13f, 14.5f, 13f)
        }
    }

    /** A bin. */
    val Delete: ImageVector by lazy {
        icon("Delete") {
            line(4f, 6.5f, 20f, 6.5f)
            moveTo(9f, 6.5f)
            lineTo(9f, 4f)
            lineTo(15f, 4f)
            lineTo(15f, 6.5f)
            moveTo(6f, 6.5f)
            lineTo(7f, 20f)
            lineTo(17f, 20f)
            lineTo(18f, 6.5f)
            line(10f, 10.5f, 10f, 16f)
            line(14f, 10.5f, 14f, 16f)
        }
    }

    /** A circle struck through: not wanted. */
    val Junk: ImageVector by lazy {
        icon("Junk") {
            circle(12f, 12f, 8.5f)
            line(6f, 6f, 18f, 18f)
        }
    }

    /** An open envelope: read. */
    val Read: ImageVector by lazy {
        icon("Read") {
            moveTo(3f, 10f)
            lineTo(12f, 3.5f)
            lineTo(21f, 10f)
            lineTo(21f, 19f)
            arcTo(1.5f, 1.5f, 0f, false, true, 19.5f, 20.5f)
            lineTo(4.5f, 20.5f)
            arcTo(1.5f, 1.5f, 0f, false, true, 3f, 19f)
            close()
            moveTo(3.5f, 10.5f)
            lineTo(12f, 16f)
            lineTo(20.5f, 10.5f)
        }
    }

    /** A folder with an arrow: move. */
    val Move: ImageVector by lazy {
        icon("Move") {
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
            line(8f, 13.5f, 16f, 13.5f)
            moveTo(13f, 10.5f)
            lineTo(16f, 13.5f)
            lineTo(13f, 16.5f)
        }
    }

    /** A tag: categories. */
    val Tag: ImageVector by lazy {
        icon("Tag") {
            moveTo(3.5f, 4.5f)
            lineTo(12f, 4.5f)
            lineTo(20.5f, 13f)
            lineTo(13f, 20.5f)
            lineTo(3.5f, 11f)
            close()
            circle(8f, 9f, 1.2f)
        }
    }
}
