// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import java.time.LocalDateTime
import uniffi.mailcraft_mobile.Core

/** The Calendar tab as the shell takes it: its pane, and the tab a calendar file handed over asks for. */
internal class CalendarTab(val pane: CalendarPane, val requestTab: Tab?, val onTabRequested: () -> Unit)

/**
 * The Calendar tab, read again when [reload] moves on (a sync, an account, a command of its own), and
 * the calendar file [handedOver] taken in once the mailbox is [ready] to take it.
 */
@Composable
internal fun rememberCalendarTab(
    core: Core,
    texts: Texts,
    window: WindowModel?,
    reload: Int,
    ready: Boolean,
    handedOver: IcsRead?,
    onHandled: () -> Unit,
): CalendarTab {
    val calendar = rememberCalendar(core, texts, window)
    val calendarState by calendar.state.collectAsState()
    LaunchedEffect(calendar, reload, LocalMessageZone.current) { calendar.refresh() }
    var requestTab by remember { mutableStateOf<Tab?>(null) }
    LaunchedEffect(handedOver, ready) {
        val read = handedOver
        if (read != null && ready) {
            when (read) {
                is IcsRead.Got -> calendar.importIcs(read.file.name, read.file.bytes)
                else -> calendar.report(texts.t("That file could not be read as a calendar."))
            }
            requestTab = Tab.Calendar
            onHandled()
        }
    }
    return CalendarTab(CalendarPane(calendarState, CalendarActions.of(calendar)), requestTab) { requestTab = null }
}

/**
 * The Calendar tab's controller. Its dates are the phone's zone and language ([DeviceDateSettings]);
 * the core shows its calendar in the phone's zone by itself. The [window]'s controller outlives the
 * window, so that an event being written survives the phone being turned; without one (a preview)
 * it lives as long as the composition.
 */
@Composable
internal fun rememberCalendar(core: Core, texts: Texts, window: WindowModel?): CalendarController {
    val zone by rememberUpdatedState(LocalMessageZone.current)
    val locale by rememberUpdatedState(LocalMessageLocale.current)
    if (window != null) {
        // Plain fields the controller reads when it needs them; the first read has them already.
        window.zone = zone
        window.locale = locale
        return remember(window, core) { window.calendar(core, texts) }
    }
    val scope = rememberCoroutineScope()
    return remember(core, texts) {
        CalendarController(
            backend = CoreCalendarBackend(core, texts),
            scope = scope,
            zone = { zone },
            weekStart = { weekStartOf(locale) },
            now = { LocalDateTime.now(zone) },
            texts = texts,
        )
    }
}
