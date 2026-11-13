// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MagicNumber", "MaxLineLength")

package work.mailcraft.android

import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.ZoneId
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

private val NOW = LocalDateTime.parse("2026-11-02T10:12:00")

internal fun detail(id: ULong = 1uL, answer: Answer = Answer.None, guest: Boolean = false, people: Boolean = false, onServer: Boolean = false) =
    EventDetailModel(
        id = id,
        calendar = 1uL,
        title = "Board review",
        location = "Room 4",
        description = "",
        start = LocalDateTime.parse("2026-11-03T14:00"),
        end = LocalDateTime.parse("2026-11-03T15:00"),
        allDay = false,
        organizer = "Bo Example",
        attendees = if (people) listOf(AttendeeItem("Ada Example", "ada@example.org", false, Answer.None)) else emptyList(),
        repeat = RepeatDraft(),
        repeatText = "",
        reminder = 15,
        invited = guest,
        answer = answer,
        cancelled = false,
        editable = !guest,
        onServer = onServer,
    )

private class FakeBackend : CalendarBackend {
    val ranges = ArrayList<DateRange>()
    val commands = ArrayList<Pair<String, String>>()
    val zones = ArrayList<String>()
    var events: List<CalendarEvent> = emptyList()
    var calendars = listOf(CalendarInfo(1uL, 0uL, "Calendar", 0xFF2E8B57.toInt(), true))
    var detail: EventDetailModel? = detail()
    var outcome: Outcome = Outcome.Done()
    var gate: CompletableDeferred<Unit>? = null

    override suspend fun events(range: DateRange): List<CalendarEvent>? {
        ranges.add(range)
        gate?.takeIf { ranges.size == 1 }?.await()
        return events
    }

    override suspend fun calendars() = calendars

    override suspend fun detail(id: ULong) = detail

    override suspend fun run(command: String, params: String): Outcome {
        commands.add(command to params)
        return outcome
    }

    override suspend fun importIcs(name: String, bytes: ByteArray): Outcome {
        commands.add("import" to "$name:${bytes.size}")
        return outcome
    }

    override suspend fun useZone(id: String) {
        zones.add(id)
    }
}

class CalendarControllerTest {
    private val backend = FakeBackend()
    private var zone: ZoneId = ZoneId.of("Europe/Berlin")

    private fun controller() = CalendarController(
        backend = backend,
        scope = CoroutineScope(Dispatchers.Unconfined),
        zone = { zone },
        weekStart = { DayOfWeek.MONDAY },
        now = { NOW },
        texts = EnglishTexts,
    )

    @Test
    fun theTabReadsTheAgendaRangeAndTheCalendarsAndTheZoneOnce() {
        val c = controller()
        backend.events = listOf(event(1, "2026-11-03T09:00", "2026-11-03T10:00"))
        c.refresh()
        assertEquals(listOf(DateRange(LocalDate.parse("2026-11-02"), LocalDate.parse("2026-12-01"))), backend.ranges)
        assertEquals(Loaded.Ready(backend.events), c.state.value.events)
        assertEquals(1, c.state.value.calendars.size)
        c.refresh()
        assertEquals(listOf("Europe/Berlin"), backend.zones)
        zone = ZoneId.of("Asia/Tokyo")
        c.refresh()
        assertEquals(listOf("Europe/Berlin", "Asia/Tokyo"), backend.zones)
    }

    @Test
    fun aFailedReadIsSaidAndNotShownAsAnEmptyCalendar() {
        val c = controller()
        backend.events = emptyList()
        val failing = object : CalendarBackend by backend {
            override suspend fun events(range: DateRange): List<CalendarEvent>? = null
        }
        val broken = CalendarController(failing, CoroutineScope(Dispatchers.Unconfined), { zone }, { DayOfWeek.MONDAY }, { NOW }, EnglishTexts)
        broken.refresh()
        assertEquals(Loaded.Failed("The calendar could not be read."), broken.state.value.events)
        assertTrue(c.state.value.events is Loaded.Loading)
    }

    @Test
    fun turningPagesAndChangingViewReadTheRangeTheyShow() {
        val c = controller()
        c.refresh()
        c.setMode(CalendarMode.Month)
        assertEquals(DateRange(LocalDate.parse("2026-10-26"), LocalDate.parse("2026-12-06")), backend.ranges.last())
        c.step(1)
        assertEquals(LocalDate.parse("2026-12-02"), c.state.value.anchor)
        c.openDay(LocalDate.parse("2026-12-24"))
        assertEquals(CalendarMode.Day, c.state.value.mode)
        assertEquals(DateRange(LocalDate.parse("2026-12-24"), LocalDate.parse("2026-12-24")), backend.ranges.last())
        c.goToday()
        assertEquals(LocalDate.parse("2026-11-02"), c.state.value.anchor)
    }

    @Test
    fun aReadOfAPageThatIsNoLongerShownIsDropped() {
        val c = controller()
        backend.gate = CompletableDeferred()
        backend.events = listOf(event(1, "2026-11-03T09:00", "2026-11-03T10:00", title = "Old page"))
        c.refresh()
        assertTrue(c.state.value.events is Loaded.Loading)
        backend.events = emptyList()
        c.step(1)
        assertEquals(Loaded.Ready(emptyList()), c.state.value.events)
        backend.gate?.complete(Unit)
        assertEquals(Loaded.Ready(emptyList()), c.state.value.events)
    }

    @Test
    fun aNewEventOpensTheEditorOnTheCalendarThatTakesIt() {
        val c = controller()
        backend.calendars = listOf(CalendarInfo(2uL, 0uL, "Holidays", 0, false), CalendarInfo(3uL, 0uL, "Mine", 0, true))
        c.refresh()
        c.newEvent(LocalDate.parse("2026-11-02"))
        val page = c.state.value.page as CalendarPage.Editor
        assertEquals(3uL, page.draft.calendar)
        assertEquals("10:30", page.draft.startTime.toString())
    }

    @Test
    fun anEventThatEndsBeforeItStartsIsNotSaved() {
        val c = controller()
        c.refresh()
        c.newEvent()
        val d = (c.state.value.page as CalendarPage.Editor).draft
        c.change(d.copy(title = "x", endTime = d.startTime.minusHours(2)))
        c.save()
        assertTrue(backend.commands.isEmpty())
        assertEquals("The event ends before it starts.", (c.state.value.page as CalendarPage.Editor).problem)
    }

    @Test
    fun aSavedEventGoesThroughTheNewCommandAndTheEditorCloses() {
        val c = controller()
        c.refresh()
        c.newEvent()
        c.change((c.state.value.page as CalendarPage.Editor).draft.copy(title = "Lunch"))
        c.save()
        assertEquals("calendar.new", backend.commands.single().first)
        assertTrue(backend.commands.single().second.contains("\"title\":\"Lunch\""))
        assertEquals(CalendarPage.Browse, c.state.value.page)
    }

    @Test
    fun peopleOnAServersCalendarAreWrittenToOnlyOnTheOwnersWord() {
        val c = controller()
        backend.calendars = listOf(CalendarInfo(1uL, 5uL, "Work", 0, true))
        c.refresh()
        c.newEvent()
        c.change((c.state.value.page as CalendarPage.Editor).draft.copy(title = "Sync", attendees = "ada@example.org"))
        c.save()
        assertTrue(backend.commands.isEmpty())
        assertTrue(c.state.value.ask is Ask.Invite)
        c.answerAsk(send = true)
        assertTrue(backend.commands.single().second.contains("\"send\":true"))
        assertNull(c.state.value.ask)
        // Asked again, and the owner says no: it is kept here, nobody is told.
        c.newEvent()
        c.change((c.state.value.page as CalendarPage.Editor).draft.copy(attendees = "ada@example.org"))
        c.save()
        c.answerAsk(send = false)
        assertTrue(backend.commands.last().second.contains("\"send\":false"))
    }

    @Test
    fun peopleOnACalendarOfThisPhoneAreNeverWrittenToAndNothingIsAsked() {
        val c = controller()
        c.refresh()
        c.newEvent()
        c.change((c.state.value.page as CalendarPage.Editor).draft.copy(attendees = "ada@example.org"))
        c.save()
        assertNull(c.state.value.ask)
        assertTrue(backend.commands.single().second.contains("\"send\":false"))
    }

    @Test
    fun aRefusedSaveKeepsTheEditorAndSaysWhy() {
        val c = controller()
        c.refresh()
        c.newEvent()
        backend.outcome = Outcome.Refused("This account may only read the calendar.")
        c.save()
        val page = c.state.value.page as CalendarPage.Editor
        assertEquals("This account may only read the calendar.", page.problem)
        assertEquals("This account may only read the calendar.", c.state.value.notice)
    }

    @Test
    fun anInvitationIsAnsweredAfterAskingWhetherTheOrganiserIsTold() {
        val c = controller()
        c.refresh()
        val invite = event(7, "2026-11-03T14:00", "2026-11-03T15:00", invited = true)
        c.answer(invite, Answer.Tentative)
        assertTrue(c.state.value.ask is Ask.Reply)
        c.answerAsk(send = false)
        assertEquals("calendar.respond" to "{\"id\":7,\"response\":\"tentative\",\"send\":false}", backend.commands.single())
        c.answer(invite, Answer.None)
        assertNull(c.state.value.ask)
        c.cancelAsk()
        assertEquals(1, backend.commands.size)
    }

    @Test
    fun anAnswerGivenOnTheDetailPageShowsTheEventAfresh() {
        val c = controller()
        c.refresh()
        backend.detail = detail(answer = Answer.None, guest = true)
        c.open(event(1, "2026-11-03T14:00", "2026-11-03T15:00", invited = true))
        backend.detail = detail(answer = Answer.Accepted, guest = true)
        c.answer(event(1, "2026-11-03T14:00", "2026-11-03T15:00", invited = true), Answer.Accepted)
        c.answerAsk(send = true)
        assertEquals(Answer.Accepted, (c.state.value.page as CalendarPage.Detail).event.answer)
    }

    @Test
    fun anEventThatIsGoneIsSaidAndNotOpened() {
        val c = controller()
        backend.detail = null
        c.open(event(1, "2026-11-03T14:00", "2026-11-03T15:00"))
        assertEquals(CalendarPage.Browse, c.state.value.page)
        assertEquals("The event is gone.", c.state.value.notice)
    }

    @Test
    fun deletingAMeetingOnAServerAsksWhetherThePeopleAreToldAndLeavesThePage() {
        val c = controller()
        val d = detail(people = true, onServer = true)
        c.delete(d, LocalDate.parse("2026-11-03"))
        assertTrue(c.state.value.ask is Ask.Delete)
        c.answerAsk(send = false)
        assertEquals("calendar.delete" to "{\"id\":1,\"send\":false}", backend.commands.single())
        assertEquals(CalendarPage.Browse, c.state.value.page)
    }

    @Test
    fun aCalendarFileIsImportedAndSaid() {
        val c = controller()
        backend.outcome = Outcome.Done("1 event added to the calendar.")
        c.importIcs("invite.ics", ByteArray(12))
        assertEquals("import" to "invite.ics:12", backend.commands.single())
        assertEquals("1 event added to the calendar.", c.state.value.notice)
        c.dismissNotice()
        assertNull(c.state.value.notice)
    }
}
