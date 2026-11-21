// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import java.time.DayOfWeek
import java.time.LocalDate

/**
 * The day, week or month view as pages to swipe through, one page for each day, week or month
 * ([CalendarPaging.pageOf]), so that the page shown after the phone is turned is the one before.
 * The controller's anchor stays the one truth: a swipe that comes to rest on another page asks for its
 * day through [onGoTo], and an anchor moved from elsewhere (Previous, Next, Today, a day opened) turns
 * the pager to it. A screen reader's page forward and back is a swipe like any other.
 */
@Composable
internal fun CalendarPager(
    mode: CalendarMode,
    anchor: LocalDate,
    weekStart: DayOfWeek,
    onGoTo: (LocalDate) -> Unit,
    modifier: Modifier = Modifier,
    page: @Composable (LocalDate) -> Unit,
) {
    // Each view pages through its own numbers, so a view of its own has a pager of its own.
    key(mode, weekStart) {
        val target = CalendarPaging.pageOf(mode, anchor, weekStart)
        val pager = rememberPagerState(initialPage = target) { CalendarPaging.pageCount(mode) }
        val shown by rememberUpdatedState(anchor)
        val goTo by rememberUpdatedState(onGoTo)
        LaunchedEffect(pager, target) {
            // A jump, not a slide: a slide that a second tap cut short would come to rest on the page
            // between, and that page would be taken for one the owner swiped to.
            if (pager.currentPage != target) pager.scrollToPage(target)
        }
        LaunchedEffect(pager) {
            snapshotFlow { pager.settledPage }.collect { settled ->
                val landing = settle(mode, shown, settled, weekStart) ?: return@collect
                goTo(landing.day)
                // A swipe past the pages there are is held at the last: the anchor may not move at all,
                // so the pager is turned back here rather than waiting for a change of anchor.
                if (landing.page != settled) pager.scrollToPage(landing.page)
            }
        }
        HorizontalPager(state = pager, modifier = modifier, key = { it }) { p ->
            page(CalendarPaging.shift(mode, anchor, p - CalendarPaging.pageOf(mode, anchor, weekStart)))
        }
    }
}

/** Where a swipe that came to rest lands: the day it asks for and the page that shows it. */
internal data class Landing(val day: LocalDate, val page: Int)

/**
 * Where the pager resting on [settled] while [shown] is the anchor lands; null when it rests on the
 * anchor's own page. The day is held to the pages there are ([CalendarPaging.heldToPages]), and the
 * page is the held day's, which is not [settled] when the swipe went past the last page.
 */
internal fun settle(mode: CalendarMode, shown: LocalDate, settled: Int, weekStart: DayOfWeek): Landing? {
    val here = CalendarPaging.pageOf(mode, shown, weekStart)
    if (settled == here) return null
    val day = CalendarPaging.heldToPages(CalendarPaging.shift(mode, shown, settled - here))
    return Landing(day, CalendarPaging.pageOf(mode, day, weekStart))
}
