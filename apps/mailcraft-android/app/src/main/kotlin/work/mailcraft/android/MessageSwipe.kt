// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.gestures.FlingBehavior
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.ScrollScope
import androidx.compose.foundation.gestures.rememberScrollableState
import androidx.compose.foundation.gestures.scrollable
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlin.math.abs
import kotlinx.coroutines.launch

/** How long a swiped-to message stays open before it counts as read: passing by quickly doesn't mark it. */
internal const val SWIPE_READ_MS = 700L

/**
 * The messages either side of the open one, in the list it was opened from, and the way to one of them.
 * [go] is asked with the row to open; a null side has no message (an end of the list).
 */
internal class MessageSwipe(val previous: InboxRow?, val next: InboxRow?, val go: (InboxRow) -> Unit) {
    companion object {
        val None = MessageSwipe(null, null) {}
    }
}

private val EdgeGive = 24.dp
private val Commit = 96.dp
private const val SLIDE_OUT_MS = 120
private const val SLIDE_IN_MS = 160
private const val COMMIT_FRACTION = 0.25f
private const val END_RESISTANCE = 0.3f

/**
 * A sideways drag on the message moves to the next one (drag left) or the one before (drag right), with
 * a short slide. Wider content that scrolls sideways takes the drag first (nested scrolling hands this only
 * what is left once it is at its edge); a drag begun in the system's back edge never gets here. At an
 * end of the list the message gives a little and a light tick says so. Only the open message is drawn.
 */
@Composable
internal fun Modifier.swipesBetweenMessages(
    texts: Texts,
    swipe: MessageSwipe?,
    onMoved: (InboxRow) -> Unit,
): Modifier = if (swipe == null) this else swiping(texts, swipe, onMoved)

@Composable
private fun Modifier.swiping(texts: Texts, swipe: MessageSwipe, onMoved: (InboxRow) -> Unit): Modifier {
    val scope = rememberCoroutineScope()
    val haptic = LocalHapticFeedback.current
    val density = LocalDensity.current
    val current by rememberUpdatedState(swipe)
    val moved by rememberUpdatedState(onMoved)
    var width by remember { mutableFloatStateOf(0f) }
    var shift by remember { mutableFloatStateOf(0f) }
    val give = with(density) { EdgeGive.toPx() }
    val commit = with(density) { Commit.toPx() }

    suspend fun slideTo(to: Float, spec: androidx.compose.animation.core.AnimationSpec<Float>) =
        animate(shift, to, animationSpec = spec) { v, _ -> shift = v }

    suspend fun go(to: InboxRow, off: Float) {
        slideTo(off, tween(SLIDE_OUT_MS))
        moved(to)
        current.go(to)
        shift = -off
        slideTo(0f, tween(SLIDE_IN_MS))
    }

    val state = rememberScrollableState { delta ->
        // (Dragging left gives a positive delta, and moves on to the next message.)
        val room = if (delta > 0) current.next != null else current.previous != null
        shift = if (room) shift - delta else (shift - delta * END_RESISTANCE).coerceIn(-give, give)
        delta
    }
    val fling = remember {
        object : FlingBehavior {
            override suspend fun ScrollScope.performFling(initialVelocity: Float): Float {
                val s = current
                val far = maxOf(commit, width * COMMIT_FRACTION)
                when {
                    shift <= -far && s.next != null -> go(s.next, -width)
                    shift >= far && s.previous != null -> go(s.previous, width)
                    else -> {
                        val noRoom = if (shift < 0) s.next == null else s.previous == null
                        val atEnd = abs(shift) > give / 2 && noRoom
                        if (atEnd) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                        slideTo(0f, spring(stiffness = Spring.StiffnessMedium))
                    }
                }
                return 0f
            }
        }
    }
    val next = texts.t("Next message")
    val before = texts.t("Previous message")
    return this
        .onSizeChanged { width = it.width.toFloat() }
        .semantics {
            customActions = listOfNotNull(
                swipe.next?.let { row -> CustomAccessibilityAction(next) { scope.launch { go(row, -width) }; true } },
                swipe.previous?.let { row ->
                    CustomAccessibilityAction(before) { scope.launch { go(row, width) }; true }
                },
            )
        }
        .scrollable(state, Orientation.Horizontal, flingBehavior = fling, overscrollEffect = null)
        .graphicsLayer { translationX = shift }
}
