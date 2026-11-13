// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.ZoneId
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException

/** A calendar of the owner's, as the editor lists it. */
internal data class CalendarInfo(
    val id: ULong,
    val account: ULong,
    val name: String,
    val color: Int,
    val editable: Boolean,
)

/** An attendee of an event, as the detail page lists it. */
internal data class AttendeeItem(val name: String, val address: String, val optional: Boolean, val answer: Answer)

/** One event whole. [repeatText] says how it repeats (empty when it doesn't); the draft is made from this. */
internal data class EventDetailModel(
    val id: ULong,
    val calendar: ULong,
    val title: String,
    val location: String,
    val description: String,
    val start: LocalDateTime,
    val end: LocalDateTime,
    val allDay: Boolean,
    val organizer: String,
    val attendees: List<AttendeeItem>,
    val repeat: RepeatDraft,
    val repeatText: String,
    val reminder: Int?,
    val invited: Boolean,
    val answer: Answer,
    val cancelled: Boolean,
    val editable: Boolean,
    val onServer: Boolean,
) {
    /** The people are named, so that a change or a deletion may write to them. */
    val hasPeople: Boolean get() = attendees.isNotEmpty()

    fun draft(occurrence: LocalDate?): EventDraft {
        val last = if (allDay) end.minusNanos(1).toLocalDate().coerceAtLeast(start.toLocalDate()) else end.toLocalDate()
        return EventDraft(
            id = id,
            calendar = calendar,
            title = title,
            location = location,
            description = description,
            attendees = attendees.joinToString("; ") { it.address },
            allDay = allDay,
            startDate = start.toLocalDate(),
            startTime = if (allDay) DEFAULT_START else start.toLocalTime(),
            endDate = last,
            endTime = if (allDay) DEFAULT_END else end.toLocalTime(),
            reminder = reminder,
            repeat = repeat,
            occurrence = occurrence?.takeIf { repeat.repeat != null || repeat.kept },
            repeatText = repeatText,
        )
    }
}

/** What a command came to: nothing to say, or the words to show. */
internal sealed interface Outcome {
    data class Done(val note: String? = null) : Outcome

    data class Refused(val reason: String) : Outcome
}

/** What the controller needs of the core; a fake in the tests. */
internal interface CalendarBackend {
    suspend fun events(range: DateRange): List<CalendarEvent>?

    suspend fun calendars(): List<CalendarInfo>

    suspend fun detail(id: ULong): EventDetailModel?

    suspend fun run(command: String, params: String): Outcome

    suspend fun importIcs(name: String, bytes: ByteArray): Outcome

    /** The zone the owner's clock is in; the core's calendar is shown by it. */
    suspend fun useZone(id: String)
}

/** What the browse views have of the range: loading, the events, or a failure. */
internal sealed interface Loaded {
    data object Loading : Loaded

    data class Ready(val events: List<CalendarEvent>) : Loaded

    data class Failed(val message: String) : Loaded
}

/** What is on the Calendar tab besides the views. */
internal sealed interface CalendarPage {
    data object Browse : CalendarPage

    data class Detail(val event: EventDetailModel, val occurrence: LocalDate) : CalendarPage

    data class Editor(val draft: EventDraft, val problem: String? = null) : CalendarPage
}

/** Everything the Calendar tab draws. */
internal data class CalendarUiState(
    val mode: CalendarMode = CalendarMode.Agenda,
    val anchor: LocalDate,
    val today: LocalDate,
    val events: Loaded = Loaded.Loading,
    val calendars: List<CalendarInfo> = emptyList(),
    val page: CalendarPage = CalendarPage.Browse,
    val ask: Ask? = null,
    val notice: String? = null,
) {
    fun range(weekStart: DayOfWeek): DateRange = CalendarPaging.range(mode, anchor, weekStart)
}

/**
 * The Calendar tab's logic: what range is shown, what the owner is doing (reading, editing, being
 * asked), and the commands that come of it, run through [backend]. It holds no view and reads no clock
 * but [now]; a command's answer is shown as a notice and the range is read again after it.
 */
// The tab's intents, one function each; they share one state and one backend, so they stay together.
@Suppress("TooManyFunctions")
internal class CalendarController(
    private val backend: CalendarBackend,
    private val scope: CoroutineScope,
    private val zone: () -> ZoneId,
    private val weekStart: () -> DayOfWeek,
    private val now: () -> LocalDateTime,
    private val texts: Texts,
) {
    private val _state = MutableStateFlow(CalendarUiState(anchor = now().toLocalDate(), today = now().toLocalDate()))
    val state: StateFlow<CalendarUiState> = _state

    private var appliedZone: String? = null

    val zoneId: ZoneId get() = zone()

    val firstDay: DayOfWeek get() = weekStart()

    /** Reads what the views need again (after a change, a sync, or coming back to the tab). */
    fun refresh() {
        val today = now().toLocalDate()
        _state.update { it.copy(today = today) }
        val shown = _state.value
        val range = shown.range(weekStart())
        scope.launch {
            val zoneId = zone().id
            if (zoneId != appliedZone) {
                backend.useZone(zoneId)
                appliedZone = zoneId
            }
            val events = backend.events(range)
            val calendars = backend.calendars()
            _state.update { current ->
                // Only a read of the range still shown is kept: the owner may have turned the page since.
                if (current.range(weekStart()) != range) {
                    current
                } else {
                    current.copy(
                        events = events?.let { Loaded.Ready(it) }
                            ?: Loaded.Failed(texts.t("The calendar could not be read.")),
                        calendars = calendars,
                    )
                }
            }
        }
    }

    private fun show(change: (CalendarUiState) -> CalendarUiState) {
        _state.update { change(it).copy(events = Loaded.Loading, notice = null) }
        refresh()
    }

    fun setMode(mode: CalendarMode) = show { it.copy(mode = mode) }

    fun goToday() = show { it.copy(anchor = now().toLocalDate()) }

    fun step(direction: Int) = show { it.copy(anchor = CalendarPaging.step(it.mode, it.anchor, direction)) }

    /** A day of the month grid or of the agenda: the day view on it. */
    fun openDay(day: LocalDate) = show { it.copy(mode = CalendarMode.Day, anchor = day) }

    fun dismissNotice() = _state.update { it.copy(notice = null) }

    fun open(event: CalendarEvent) {
        scope.launch {
            val detail = backend.detail(event.id)
            _state.update {
                if (detail == null) {
                    it.copy(notice = texts.t("The event is gone."))
                } else {
                    it.copy(page = CalendarPage.Detail(detail, event.firstDay), notice = null)
                }
            }
        }
    }

    fun close() = _state.update { it.copy(page = CalendarPage.Browse, ask = null) }

    fun newEvent(day: LocalDate? = null) {
        val current = _state.value
        val calendar = current.calendars.firstOrNull { it.editable }?.id ?: 0uL
        val draft = EventDraft.blank(day ?: current.anchor, now(), calendar)
        _state.update { it.copy(page = CalendarPage.Editor(draft), notice = null) }
    }

    fun edit(detail: EventDetailModel, occurrence: LocalDate?) {
        _state.update { it.copy(page = CalendarPage.Editor(detail.draft(occurrence))) }
    }

    fun change(draft: EventDraft) {
        _state.update { current ->
            val page = current.page
            if (page is CalendarPage.Editor) current.copy(page = CalendarPage.Editor(draft)) else current
        }
    }

    /** Saves the draft, asking first when saving would make a server write to the people invited. */
    fun save() {
        val page = _state.value.page as? CalendarPage.Editor ?: return
        val draft = page.draft
        if (draft.endBeforeStart) {
            _state.update { it.copy(page = page.copy(problem = texts.t("The event ends before it starts."))) }
            return
        }
        val account = _state.value.calendars.firstOrNull { it.id == draft.calendar }?.account ?: 0uL
        if (draft.hasPeople && account != 0uL) {
            _state.update { it.copy(ask = Ask.Invite(draft)) }
        } else {
            commit(draft, send = if (draft.hasPeople) false else null)
        }
    }

    private fun commit(draft: EventDraft, send: Boolean?) {
        val command = if (draft.isNew) "calendar.new" else "calendar.edit"
        run(command, draft.params(send), done = CalendarPage.Browse)
    }

    /** The owner's answer to a question of [Ask]; [send] is whether people are written to. */
    fun answerAsk(send: Boolean?, occurrenceOnly: Boolean = false) {
        val ask = _state.value.ask ?: return
        _state.update { it.copy(ask = null) }
        when (ask) {
            is Ask.Invite -> commit(ask.draft, send ?: false)
            is Ask.Reply -> run(
                "calendar.respond",
                answerParams(ask.event.id, ask.answer, send ?: false, ask.event.firstDay.takeIf { occurrenceOnly }),
                done = null,
            )
            is Ask.Delete -> run(
                "calendar.delete",
                deleteParams(ask.detail.id, ask.occurrence.takeIf { occurrenceOnly }, send),
                done = CalendarPage.Browse,
            )
        }
    }

    fun cancelAsk() = _state.update { it.copy(ask = null) }

    /** An answer to an invitation: asked first, because the organiser is told only when the owner says so. */
    fun answer(event: CalendarEvent, answer: Answer) {
        if (answer == Answer.None) return
        _state.update { it.copy(ask = Ask.Reply(event, answer)) }
    }

    fun delete(detail: EventDetailModel, occurrence: LocalDate?) {
        _state.update { it.copy(ask = Ask.Delete(detail, occurrence)) }
    }

    /** An .ics file the owner opened: its events go into the calendar. */
    fun importIcs(name: String, bytes: ByteArray) {
        scope.launch {
            val outcome = backend.importIcs(name, bytes)
            report(outcome)
            refresh()
        }
    }

    private fun run(command: String, params: String, done: CalendarPage?) {
        scope.launch {
            val outcome = backend.run(command, params)
            if (outcome is Outcome.Done && done != null) _state.update { it.copy(page = done) }
            if (outcome is Outcome.Done && done == null) {
                // An answer given from the detail page or the agenda: the page shows it afresh.
                reopen()
            }
            report(outcome)
            refresh()
        }
    }

    private suspend fun reopen() {
        val page = _state.value.page as? CalendarPage.Detail ?: return
        val detail = backend.detail(page.event.id)
        _state.update { it.copy(page = if (detail == null) CalendarPage.Browse else page.copy(event = detail)) }
    }

    /** Says [note] to the owner. */
    fun report(note: String) = _state.update { it.copy(notice = note) }

    private fun report(outcome: Outcome) {
        val note = when (outcome) {
            is Outcome.Done -> outcome.note
            is Outcome.Refused -> outcome.reason
        }
        if (note != null) _state.update { it.copy(notice = note) }
        // A refused save keeps the editor open, with the reason on it.
        if (outcome is Outcome.Refused) {
            _state.update { current ->
                val page = current.page
                if (page is CalendarPage.Editor) current.copy(page = page.copy(problem = outcome.reason)) else current
            }
        }
    }
}

/** The Rust core as the controller's backend. */
internal class CoreCalendarBackend(private val core: Core, private val texts: Texts) : CalendarBackend {
    override suspend fun events(range: DateRange): List<CalendarEvent>? = try {
        core.calendar(range.from.toString(), range.to.toString()).events.mapNotNull { r ->
            val start = parseCoreTime(r.start)
            val end = parseCoreTime(r.end)
            if (start == null || end == null) {
                null
            } else {
                CalendarEvent(
                    id = r.event,
                    calendar = r.calendar,
                    color = (OPAQUE or r.color.toInt()),
                    title = r.title,
                    location = r.location,
                    start = start,
                    end = end,
                    allDay = r.allDay,
                    repeats = r.repeats,
                    meeting = r.meeting,
                    cancelled = r.cancelled,
                    invited = r.invited,
                    answer = Answer.fromCode(r.answer),
                )
            }
        }
    } catch (_: CoreException) {
        null
    }

    override suspend fun calendars(): List<CalendarInfo> = try {
        core.calendars().calendars.map { CalendarInfo(
            it.id,
            it.account,
            it.name,
            OPAQUE or it.color.toInt(),
            it.editable,
        ) }
    } catch (_: CoreException) {
        emptyList()
    }

    override suspend fun detail(id: ULong): EventDetailModel? = try {
        val e = core.event(id)
        val start = parseCoreTime(e.start)
        val end = parseCoreTime(e.end)
        if (start == null || end == null) {
            null
        } else {
            val allDay = e.allDay
            val rule = e.recurrence
            EventDetailModel(
                id = e.event,
                calendar = e.calendar,
                title = e.title,
                location = e.location,
                description = e.description,
                start = start,
                // A whole-day event's end is its last day: the draft and the views read the moment after it.
                end = if (allDay) end.plusDays(1) else end,
                allDay = allDay,
                organizer = e.organizer?.let { it.name.ifBlank { it.address } }.orEmpty(),
                attendees = e.attendees.map { AttendeeItem(
                    it.name,
                    it.address,
                    it.optional,
                    Answer.fromCode(it.response),
                ) },
                repeat = rule?.let { repeatOf(RuleFacts(it.freq, it.interval, it.byDay, it.nth, it.count, it.until)) }
                    ?: RepeatDraft(),
                repeatText = rule?.let { describeRepeat(it.freq, it.interval, texts) }.orEmpty(),
                reminder = e.reminderMinutes?.toInt(),
                invited = e.invited,
                answer = Answer.fromCode(e.answer),
                cancelled = e.cancelled,
                editable = e.editable,
                onServer = e.onServer,
            )
        }
    } catch (_: CoreException) {
        null
    }

    override suspend fun run(command: String, params: String): Outcome = try {
        core.execute(command, params)
        Outcome.Done()
    } catch (e: CoreException) {
        Outcome.Refused(reasonOf(e) ?: texts.t("That could not be done."))
    }

    override suspend fun useZone(id: String) {
        try {
            core.execute("view.timeZone", "{\"zone\":${jsonString(id)}}")
        } catch (_: CoreException) {
            // A zone the core doesn't know: the calendar stays in the one it had.
        }
    }

    override suspend fun importIcs(name: String, bytes: ByteArray): Outcome = try {
        val said = core.execute("file.import", fileImportParams(name, bytes))
        Outcome.Done(importedNote(said, texts))
    } catch (e: CoreException) {
        Outcome.Refused(reasonOf(e) ?: texts.t("That file could not be read as a calendar."))
    }

    private companion object {
        /** Alpha of an opaque colour: the core's colours are `0xRRGGBB`. */
        const val OPAQUE = 0xFF000000.toInt()
    }
}

/** The words a refused command gave, when it gave any. */
internal fun reasonOf(e: CoreException): String? = when (e) {
    is CoreException.Failed -> e.reason.takeIf { it.isNotBlank() }
    is CoreException.BadRequest -> e.reason.takeIf { it.isNotBlank() }
    else -> null
}
