// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test
import uniffi.mailcraft_mobile.FaceView
import uniffi.mailcraft_mobile.SpanView

/** A span's face and weight become the phone's system fonts (#1966); the core walks the family list. */
class BodyFontsTest {
    @Test
    fun facesBecomeSystemFamilies() {
        assertNull(familyOf(FaceView.OWN, code = false))
        assertEquals(FontFamily.Serif, familyOf(FaceView.SERIF, code = false))
        assertEquals(FontFamily.Monospace, familyOf(FaceView.MONO, code = false))
        assertEquals(FontFamily.Monospace, familyOf(FaceView.SERIF, code = true))
        assertNotNull(familyOf(FaceView.CONDENSED, code = false))
    }

    private fun weightOf(span: SpanView) = styleOf(span, Paper(0xFFFFFF, 0, 0), linked = false).fontWeight

    @Test
    fun weightsAreKeptRegularToo() {
        assertEquals(FontWeight(300), weightOf(B.span("light", weight = 300u)))
        assertEquals(FontWeight.Bold, weightOf(B.span("bold", bold = true)))
        assertEquals(FontWeight(900), weightOf(B.span("black", bold = true, weight = 900u)))
        // Regular is said, so a heading that asks for `font-weight:normal` is regular, as on the desktop.
        assertEquals(FontWeight.Normal, weightOf(B.span("regular")))
        // Out of range is held to it.
        assertEquals(FontWeight(1000), weightOf(B.span("past", weight = 2000u)))
    }
}
