// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import androidx.compose.runtime.Composable
import androidx.compose.runtime.produceState
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** A picture decoded to at most [MAX_SIDE] pixels a side, with the size it really has. */
internal class Decoded(val bitmap: ImageBitmap, val width: Int, val height: Int)

/** The longest side a picture is decoded to, in pixels, and the most pixels one is decoded from. */
internal const val MAX_SIDE = 2_048
internal const val MAX_PIXELS = 40_000_000L

/** The sample size that brings a picture of [width] by [height] pixels within [MAX_SIDE], a power of two. */
internal fun sampleSize(width: Int, height: Int): Int {
    var sample = 1
    while (maxOf(width, height) / sample > MAX_SIDE) sample *= 2
    return sample
}

/** Where pictures are decoded: off the main thread. (A test of what is drawn puts it on its own thread.) */
internal val LocalDecoder = staticCompositionLocalOf<CoroutineDispatcher> { Dispatchers.Default }

/** [bytes] decoded off the main thread: null until it is done (and for what is no picture the phone can draw). */
@Composable
internal fun rememberDecoded(bytes: ByteArray?): Decoded? {
    val dispatcher = LocalDecoder.current
    return produceState<Decoded?>(null, bytes, dispatcher) {
        value = if (bytes == null) null else withContext(dispatcher) { decode(bytes) }
    }.value
}

/** [bytes] as a picture, or null when they are not one the phone can draw within the caps. */
internal fun decode(bytes: ByteArray): Decoded? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    val (w, h) = bounds.outWidth to bounds.outHeight
    if (w <= 0 || h <= 0 || w.toLong() * h > MAX_PIXELS) return null
    val options = BitmapFactory.Options().apply {
        inSampleSize = sampleSize(w, h)
        inPreferredConfig = Bitmap.Config.ARGB_8888
    }
    val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options) ?: return null
    return Decoded(bitmap.asImageBitmap(), w, h)
}
