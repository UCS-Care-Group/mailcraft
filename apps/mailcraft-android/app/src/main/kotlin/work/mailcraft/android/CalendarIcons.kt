// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MagicNumber")

package work.mailcraft.android

import androidx.compose.ui.graphics.vector.ImageVector

/** The Calendar tab's icons, drawn here as the rest of the app's are: our own strokes, nobody's artwork. */
internal object CalendarIcons {
    /** A plus, for a new event. */
    val Add: ImageVector by lazy {
        icon("Add") {
            line(12f, 5f, 12f, 19f)
            line(5f, 12f, 19f, 12f)
        }
    }

    /** A chevron that points back, for the previous page. */
    val Previous: ImageVector by lazy {
        icon("Previous") {
            moveTo(15f, 5f)
            lineTo(8f, 12f)
            lineTo(15f, 19f)
        }
    }

    /** A cross, for closing a page. */
    val Close: ImageVector by lazy {
        icon("Close") {
            line(6f, 6f, 18f, 18f)
            line(18f, 6f, 6f, 18f)
        }
    }
}
