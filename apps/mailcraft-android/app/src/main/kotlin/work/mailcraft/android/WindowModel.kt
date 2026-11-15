// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import java.time.LocalDateTime
import java.time.ZoneId
import java.util.Locale
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import uniffi.mailcraft_mobile.Core

/**
 * What the window keeps when Android makes it again (the phone turned, the language changed) and,
 * through [saved], when the app was put away and comes back: a calendar file handed over and not yet
 * taken in, and the Calendar tab with the event the owner is writing.
 */
// Android makes the model through its one-argument constructor, which @JvmOverloads provides.
internal class WindowModel @JvmOverloads constructor(
    private val saved: SavedStateHandle,
    io: CoroutineDispatcher = Dispatchers.IO,
) : ViewModel() {
    val handoff = Handoff(viewModelScope, io, saved.get<Boolean>(PENDING_FILE) == true) {
        saved[PENDING_FILE] = it
    }

    /** The phone's zone and language as the window last had them; the controller reads them. */
    @Volatile var zone: ZoneId = ZoneId.systemDefault()

    @Volatile var locale: Locale = Locale.getDefault()

    private var calendar: CalendarController? = null
    private var calendarCore: Core? = null
    private var keeping: Job? = null

    /** The Calendar tab's controller, made once for [core] and kept while the window is made again. */
    fun calendar(core: Core, texts: Texts): CalendarController {
        calendar?.takeIf { calendarCore === core }?.let { return it }
        val made = CalendarController(
            backend = CoreCalendarBackend(core, texts),
            scope = viewModelScope,
            zone = { zone },
            weekStart = { weekStartOf(locale) },
            now = { LocalDateTime.now(zone) },
            texts = texts,
        )
        saved.get<HashMap<String, String>>(DRAFT)?.let { SavedDraft.read(it) }?.let { made.restore(it) }
        keeping?.cancel()
        keeping = viewModelScope.launch {
            made.state.map { (it.page as? CalendarPage.Editor)?.draft }.distinctUntilChanged().collect { draft ->
                saved[DRAFT] = draft?.let { SavedDraft.write(it) }
            }
        }
        calendar = made
        calendarCore = core
        return made
    }

    private companion object {
        const val PENDING_FILE = "handoff.pending"
        const val DRAFT = "calendar.draft"
    }
}
