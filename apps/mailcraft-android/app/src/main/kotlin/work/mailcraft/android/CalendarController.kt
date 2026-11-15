// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.ZoneId
import java.time.temporal.ChronoUnit
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
    /** [attendees] is not the whole list (the core hands over so many): it is shown, never sent back. */
    val peopleCut: Boolean = false,
) {
    /** The people are named, so that a change or a deletion may write to them. */
    val hasPeople: Boolean get() = attendees.isNotEmpty()

    /** It is a series, whether or not the editor can show its rule. */
    val repeats: Boolean get() = repeat.repeat != null || repeat.kept

    /**
     * The start and the end of the time on [occurrence]: a series' times are on their own days, which
     * are not the day the series began; any other event's are its own.
     */
    fun span(occurrence: LocalDate?, resolved: Moments? = null): Pair<LocalDateTime, LocalDateTime> {
        // The core has worked out a timed occurrence's clock times (a zone's clock may have changed since
        // the series began): the row that was opened has them, so they are used as they are.
        val own = resolved?.takeIf { repeats && !allDay && occurrence != null }
        if (own != null) return own
        val shift = occurrence?.takeIf { repeats }?.let { ChronoUnit.DAYS.between(start.toLocalDate(), it) } ?: 0L
        return start.plusDays(shift) to end.plusDays(shift)
    }

    /** The time on [occurrence] as the agenda's row of it, for the answer commands. */
    fun asEvent(occurrence: LocalDate?, resolved: Moments? = null): CalendarEvent {
        val (from, to) = span(occurrence, resolved)
        return CalendarEvent(
            id = id,
            calendar = calendar,
            color = 0,
            title = title,
            location = location,
            start = from,
            end = to,
            allDay = allDay,
            repeats = repeats,
            meeting = hasPeople,
            cancelled = cancelled,
            invited = invited,
            answer = answer,
        )
    }

    fun draft(occurrence: LocalDate?, resolved: Moments? = null): EventDraft {
        val only = occurrence?.takeIf { repeats }
        // One time of a series shows its own days; the draft gives the command the series' days back.
        val shift = only?.let { ChronoUnit.DAYS.between(start.toLocalDate(), it) } ?: 0L
        val (from, to) = if (only != null) span(only, resolved) else start to end
        val last = if (allDay) to.minusNanos(1).toLocalDate().coerceAtLeast(from.toLocalDate()) else to.toLocalDate()
        val required = attendees.filterNot { it.optional }.joinToString("; ") { it.address }
        val optional = attendees.filter { it.optional }.joinToString("; ") { it.address }
        return EventDraft(
            id = id,
            calendar = calendar,
            title = title,
            location = location,
            description = description,
            attendees = required,
            optional = optional,
            allDay = allDay,
            startDate = from.toLocalDate(),
            startTime = if (allDay) DEFAULT_START else from.toLocalTime(),
            endDate = last,
            endTime = if (allDay) DEFAULT_END else to.toLocalTime(),
            reminder = reminder,
            repeat = repeat,
            occurrence = only,
            repeatText = repeatText,
            seriesShift = shift,
            loaded = LoadedText(title, location, description, required, optional, peopleCut),
        ).let { d ->
            // What was shown is what a save compares with: times left alone aren't sent.
            val shown = d.loaded?.copy(
                start = d.start,
                end = d.end,
                allDay = allDay,
                seriesStart = start,
                seriesEnd = end,
            )
            d.copy(loaded = shown)
        }
    }
}

/** A start and an end. */
internal typealias Moments = Pair<LocalDateTime, LocalDateTime>

/** What a command came to: nothing to say, or the words to show. */
internal sealed interface Outcome {
    data class Done(val note: String? = null) : Outcome

    data class Refused(val reason: String) : Outcome
}

/** An event whole and its own start and end on one day; [times] is null when it has no time then. */
internal data class OpenedEvent(val event: EventDetailModel, val times: Moments?)

/** A timed time of a series has its own start and end from the core, or it is no longer there. */
internal fun OpenedEvent.hasItsTime(): Boolean = times != null || !event.repeats || event.allDay

/** What the controller needs of the core; a fake in the tests. */
internal interface CalendarBackend {
    suspend fun events(range: DateRange): List<CalendarEvent>?

    suspend fun calendars(): List<CalendarInfo>

    /** The event whole, with its time on [day], read together so that both are of one state of it. */
    suspend fun detail(id: ULong, day: LocalDate): OpenedEvent?

    suspend fun run(command: String, params: String): Outcome

    suspend fun importIcs(name: String, bytes: ByteArray): Outcome
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

    /** [times] are the opened row's own start and end, as the core worked them out. */
    data class Detail(val event: EventDetailModel, val occurrence: LocalDate, val times: Moments? = null) : CalendarPage

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
    /** A save of the editor on screen is on its way: Save waits for it. */
    val saving: Boolean = false,
) {
    fun range(weekStart: DayOfWeek): DateRange = CalendarPaging.range(mode, anchor, weekStart)

    /**
     * A new event opened before the calendars were read has no calendar of its own, while the editor
     * shows the first it can write to: the draft takes that one, so what is shown is what is saved.
     */
    fun withCalendarChosen(): CalendarUiState {
        val editor = page as? CalendarPage.Editor ?: return this
        val d = editor.draft
        val first = calendars.firstOrNull { it.editable }
        val chosen = if (d.isNew && calendars.none { it.editable && it.id == d.calendar }) first?.id else null
        return if (chosen == null) this else copy(page = editor.copy(draft = d.copy(calendar = chosen)))
    }
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

    val zoneId: ZoneId get() = zone()

    val firstDay: DayOfWeek get() = weekStart()

    /** The latest read of the range: an earlier one that comes back later is not kept. */
    private var reads = 0L

    /**
     * Which page the owner is on, counted up whenever they go to another (an event opened, the editor
     * opened or closed). What comes back from the core for an earlier page is not put on a later one.
     */
    private var visit = 0L

    private fun go(change: (CalendarUiState) -> CalendarUiState) {
        visit += 1
        _state.update { change(it).copy(saving = false) }
    }

    /** Reads what the views need again (after a change, a sync, or coming back to the tab). */
    fun refresh() {
        val today = now().toLocalDate()
        _state.update { it.copy(today = today) }
        val shown = _state.value
        val range = shown.range(weekStart())
        reads += 1
        val read = reads
        scope.launch {
            val events = backend.events(range)
            val calendars = backend.calendars()
            _state.update { current ->
                // Only the latest read of the range still shown is kept: the owner may have turned the
                // page since, or a change may have been read again after this read began.
                if (read != reads || current.range(weekStart()) != range) {
                    current
                } else {
                    current.copy(
                        events = events?.let { Loaded.Ready(it) }
                            ?: Loaded.Failed(texts.t("The calendar could not be read.")),
                        calendars = calendars,
                    ).withCalendarChosen()
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
        go { it }
        val opened = visit
        scope.launch {
            val got = backend.detail(event.id, event.firstDay)
            // The owner may have gone elsewhere (a new event, say) while the event was read.
            if (opened != visit) return@launch
            _state.update {
                // A time of a series that is no longer there (the series changed since the row was read) is gone.
                if (got == null || !got.hasItsTime()) {
                    it.copy(notice = texts.t("The event is gone."))
                } else {
                    it.copy(page = CalendarPage.Detail(got.event, event.firstDay, got.times), notice = null)
                }
            }
        }
    }

    fun close() = go { it.copy(page = CalendarPage.Browse, ask = null) }

    fun newEvent(day: LocalDate? = null) {
        val current = _state.value
        val calendar = current.calendars.firstOrNull { it.editable }?.id ?: 0uL
        val draft = EventDraft.blank(day ?: current.anchor, now(), calendar)
        go { it.copy(page = CalendarPage.Editor(draft), notice = null) }
    }

    fun edit(detail: EventDetailModel, occurrence: LocalDate?, times: Moments? = null) {
        go { it.copy(page = CalendarPage.Editor(detail.draft(occurrence, times))) }
    }

    /** The editor as it was before the window was made again (the draft the owner was writing). */
    fun restore(draft: EventDraft) {
        go { it.copy(page = CalendarPage.Editor(draft), ask = null) }
    }

    fun change(draft: EventDraft) {
        _state.update { current ->
            val page = current.page
            if (page is CalendarPage.Editor) current.copy(page = CalendarPage.Editor(draft)) else current
        }
    }

    /** Saves the draft, asking first when saving would make a server write to the people invited. */
    fun save() {
        if (_state.value.saving) return
        val page = _state.value.page as? CalendarPage.Editor ?: return
        val draft = page.draft
        if (draft.endBeforeStart) {
            _state.update { it.copy(page = page.copy(problem = texts.t("The event ends before it starts."))) }
            return
        }
        val account = _state.value.calendars.firstOrNull { it.id == draft.calendar }?.account ?: 0uL
        // Whoever was invited before is written to as well when the owner takes them all off.
        if ((draft.hasPeople || draft.hadPeople) && account != 0uL) {
            _state.update { it.copy(ask = Ask.Invite(draft)) }
        } else {
            commit(draft, send = if (draft.hasPeople) false else null)
        }
    }

    private fun commit(draft: EventDraft, send: Boolean?) {
        // One save at a time: a second press while the first is on its way would make the event twice.
        if (_state.value.saving) return
        _state.update { it.copy(saving = true) }
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
            report(outcome, editor = false)
            refresh()
        }
    }

    private fun run(command: String, params: String, done: CalendarPage?) {
        val from = visit
        scope.launch {
            val outcome = backend.run(command, params)
            // The page the command was given on is left only while the owner is still on it: a later
            // editor (another event being written) stays as it is.
            val still = from == visit
            if (still) _state.update { it.copy(saving = false) }
            if (outcome is Outcome.Done && done != null && still) go { it.copy(page = done) }
            if (outcome is Outcome.Done && done == null && still) {
                // An answer given from the detail page or the agenda: the page shows it afresh.
                reopen()
            }
            report(outcome, editor = still)
            refresh()
        }
    }

    private suspend fun reopen() {
        val page = _state.value.page as? CalendarPage.Detail ?: return
        val from = visit
        val got = backend.detail(page.event.id, page.occurrence)
        // The owner may have left the page while it was read again.
        if (from != visit) return
        val shown = if (got == null || !got.hasItsTime()) {
            CalendarPage.Browse
        } else {
            page.copy(event = got.event, times = got.times)
        }
        _state.update { it.copy(page = shown) }
    }

    /** Says [note] to the owner. */
    fun report(note: String) = _state.update { it.copy(notice = note) }

    private fun report(outcome: Outcome, editor: Boolean = true) {
        val note = when (outcome) {
            is Outcome.Done -> outcome.note
            is Outcome.Refused -> outcome.reason
        }
        if (note != null) _state.update { it.copy(notice = note) }
        // A refused save keeps the editor open, with the reason on it.
        if (outcome is Outcome.Refused && editor) {
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

    override suspend fun detail(id: ULong, day: LocalDate): OpenedEvent? = try {
        val got = core.eventOn(id, day.toString())
        val e = got.detail
        val start = parseCoreTime(e.start)
        val end = parseCoreTime(e.end)
        if (start == null || end == null) {
            null
        } else {
            val allDay = e.allDay
            val rule = e.recurrence
            val from = got.start?.let(::parseCoreTime)
            val to = got.end?.let(::parseCoreTime)
            OpenedEvent(EventDetailModel(
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
                peopleCut = e.peopleCut,
            ), if (from != null && to != null) from to to else null)
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
