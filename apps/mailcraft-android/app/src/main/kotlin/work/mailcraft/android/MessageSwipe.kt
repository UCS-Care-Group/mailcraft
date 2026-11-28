// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.os.Build
import android.view.HapticFeedbackConstants
import androidx.compose.animation.core.AnimationSpec
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.gestures.FlingBehavior
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.ScrollScope
import androidx.compose.foundation.gestures.rememberScrollableState
import androidx.compose.foundation.gestures.scrollable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlin.math.abs
import kotlinx.coroutines.launch

/** How long a swiped-to message stays open before it can count as read: passing by quickly never marks it. */
internal const val SWIPE_READ_MS = 700L

/**
 * The messages either side of the open one, in the list it was opened from, and the way to one of them.
 * [go] is asked with the row to open; a null side has no message (an end of the list).
 */
internal class MessageSwipe(val previous: InboxRow?, val next: InboxRow?, val go: (InboxRow) -> Unit)

private val EdgeGive = 24.dp
private val Commit = 96.dp
private const val SLIDE_MS = 160
private const val COMMIT_FRACTION = 0.25f
private const val END_RESISTANCE = 0.3f

/**
 * The open message with its neighbours either side of it, drawn as real pages while a sideways drag is on:
 * [before] to the left, [after] to the right, each laid out from what was read ahead. The message follows
 * the finger: drag left and it slides left, the next one coming in from the right. Wider content that
 * scrolls sideways takes the drag first (nested scrolling hands this only what is left once it is at its
 * edge); a drag begun in the system's back edge never gets here. At an end of the list the message gives a
 * little and a haptic says so. With no [swipe] there is no list to move in and it is [content] alone.
 */
@Composable
@Suppress("LongMethod", "LongParameterList")
internal fun SwipeStrip(
    texts: Texts,
    swipe: MessageSwipe?,
    onMoved: (InboxRow) -> Unit,
    modifier: Modifier,
    before: @Composable () -> Unit,
    after: @Composable () -> Unit,
    content: @Composable () -> Unit,
) {
    if (swipe == null) {
        Box(modifier) { content() }
        return
    }
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    val density = LocalDensity.current
    val current by rememberUpdatedState(swipe)
    val moved by rememberUpdatedState(onMoved)
    var width by remember { mutableFloatStateOf(0f) }
    // Where the open message is, in pixels from its place: negative is to the left, with the finger.
    var shift by remember { mutableFloatStateOf(0f) }
    val give = with(density) { EdgeGive.toPx() }
    val commit = with(density) { Commit.toPx() }

    suspend fun slideTo(to: Float, spec: AnimationSpec<Float>) =
        animate(shift, to, animationSpec = spec) { v, _ -> shift = v }

    suspend fun go(to: InboxRow, off: Float) {
        slideTo(off, tween(SLIDE_MS))
        moved(to)
        current.go(to)
        // (The message swiped to is the open one now, in the place it had come to.)
        shift = 0f
    }

    val state = rememberScrollableState { delta ->
        // (This delta is the finger's: dragging left is negative, and moves on to the next message.)
        val room = if (delta < 0) current.next != null else current.previous != null
        shift = if (room) shift + delta else (shift + delta * END_RESISTANCE).coerceIn(-give, give)
        delta
    }
    val fling = remember {
        object : FlingBehavior {
            override suspend fun ScrollScope.performFling(initialVelocity: Float): Float {
                val s = current
                val far = maxOf(commit, width * COMMIT_FRACTION)
                val next = s.next
                val earlier = s.previous
                when {
                    shift <= -far && next != null -> go(next, -width)
                    shift >= far && earlier != null -> go(earlier, width)
                    else -> {
                        val noRoom = if (shift < 0) next == null else earlier == null
                        if (abs(shift) > give / 2 && noRoom) view.performHapticFeedback(rejectFeel())
                        slideTo(0f, spring(stiffness = Spring.StiffnessMedium))
                    }
                }
                return 0f
            }
        }
    }
    val nextLabel = texts.t("Next message")
    val previousLabel = texts.t("Previous message")
    Box(
        modifier
            .onSizeChanged { width = it.width.toFloat() }
            .semantics {
                customActions = listOfNotNull(
                    swipe.next?.let { row ->
                        CustomAccessibilityAction(nextLabel) { scope.launch { go(row, -width) }; true }
                    },
                    swipe.previous?.let { row ->
                        CustomAccessibilityAction(previousLabel) { scope.launch { go(row, width) }; true }
                    },
                )
            }
            .scrollable(state, Orientation.Horizontal, flingBehavior = fling, overscrollEffect = null),
    ) {
        Box(Modifier.fillMaxSize().graphicsLayer { translationX = shift }) { content() }
        // (Only while a drag is on: a neighbour is not laid out at rest, and a screen reader never meets it.)
        if (shift > 0f && swipe.previous != null) {
            Box(Modifier.fillMaxSize().graphicsLayer { translationX = shift - width }.clearAndSetSemantics {}) {
                before()
            }
        }
        if (shift < 0f && swipe.next != null) {
            Box(Modifier.fillMaxSize().graphicsLayer { translationX = shift + width }.clearAndSetSemantics {}) {
                after()
            }
        }
    }
}

/**
 * The haptic for "that can't go further". `REJECT` (Android 11) is the one the system names for it; like every
 * `performHapticFeedback` it follows the owner's Touch feedback switch under Vibration and haptics, and the
 * ringer's silent mode does not mute it. Android 10 has no such constant: the long press's, the same way.
 */
internal fun rejectFeel(): Int =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
        HapticFeedbackConstants.REJECT
    } else {
        HapticFeedbackConstants.LONG_PRESS
    }
