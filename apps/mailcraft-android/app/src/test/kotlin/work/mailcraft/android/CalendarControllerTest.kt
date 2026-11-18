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
import org.junit.Assert.assertFalse
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
    var events: List<CalendarEvent> = emptyList()
    var calendars = listOf(CalendarInfo(1uL, 0uL, "Calendar", 0xFF2E8B57.toInt(), true))
    var detail: EventDetailModel? = detail()

    /** The time the core gives on the day asked for, with the detail; null when there is none. */
    var times: Moments? = null
    var outcome: Outcome = Outcome.Done()
    var gate: CompletableDeferred<Unit>? = null
    var detailGate: CompletableDeferred<Unit>? = null
    var runGate: CompletableDeferred<Unit>? = null

    override suspend fun events(range: DateRange): List<CalendarEvent>? {
        ranges.add(range)
        // What the calendar held when the read began, however long it takes to come back.
        val held = events
        gate?.takeIf { ranges.size == 1 }?.await()
        return held
    }

    override suspend fun calendars() = calendars

    override suspend fun detail(id: ULong, day: java.time.LocalDate): OpenedEvent? {
        // Both are what the core held when the read began: one state of the event.
        val held = detail?.let { OpenedEvent(it, times) }
        detailGate?.await()
        return held
    }

    override suspend fun run(command: String, params: String): Outcome {
        commands.add(command to params)
        runGate?.await()
        return outcome
    }

    override suspend fun importIcs(name: String, bytes: ByteArray): Outcome {
        commands.add("import" to "$name:${bytes.size}")
        return outcome
    }
}

class CalendarControllerTest {
    private val backend = FakeBackend()
    private val zone: ZoneId = ZoneId.of("Europe/Berlin")

    private fun controller() = CalendarController(
        backend = backend,
        scope = CoroutineScope(Dispatchers.Unconfined),
        zone = { zone },
        weekStart = { DayOfWeek.MONDAY },
        now = { NOW },
        texts = EnglishTexts,
    )

    @Test
    fun theTabReadsTheAgendaRangeAndTheCalendars() {
        val c = controller()
        backend.events = listOf(event(1, "2026-11-03T09:00", "2026-11-03T10:00"))
        c.refresh()
        assertEquals(listOf(DateRange(LocalDate.parse("2026-11-02"), LocalDate.parse("2026-12-01"))), backend.ranges)
        assertEquals(Loaded.Ready(backend.events), c.state.value.events)
        assertEquals(1, c.state.value.calendars.size)
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
        // The day view reads the day on either side too, so that a swipe shows them as they come in.
        assertEquals(DateRange(LocalDate.parse("2026-12-23"), LocalDate.parse("2026-12-25")), backend.ranges.last())
        c.goToday()
        assertEquals(LocalDate.parse("2026-11-02"), c.state.value.anchor)
    }

    @Test
    fun theWeekViewReadsItsWeekAndTheWeeksBesideIt() {
        val c = controller()
        c.refresh()
        c.setMode(CalendarMode.Week)
        // 2 November 2026 is a Monday.
        assertEquals(DateRange(LocalDate.parse("2026-10-26"), LocalDate.parse("2026-11-15")), backend.ranges.last())
        c.step(1)
        assertEquals(LocalDate.parse("2026-11-09"), c.state.value.anchor)
        c.goToday()
        assertEquals(LocalDate.parse("2026-11-02"), c.state.value.anchor)
        assertEquals(CalendarMode.Week, c.state.value.mode)
    }

    @Test
    fun aDayFarOutsideThePagesIsHeldToThem() {
        val c = controller()
        c.setMode(CalendarMode.Day)
        c.goTo(LocalDate.parse("1800-01-01"))
        assertEquals(CalendarPaging.heldToPages(LocalDate.parse("1800-01-01")), c.state.value.anchor)
        assertTrue(c.state.value.anchor.year == 1900)
        // A swipe further out asks for nothing more.
        val reads = backend.ranges.size
        c.goTo(c.state.value.anchor.minusDays(1))
        assertEquals(reads, backend.ranges.size)
    }

    @Test
    fun aSwipeGoesToItsDayAndASwipeBackToTheSameDayReadsNothing() {
        val c = controller()
        c.refresh()
        c.setMode(CalendarMode.Day)
        val reads = backend.ranges.size
        c.goTo(LocalDate.parse("2026-11-03"))
        assertEquals(LocalDate.parse("2026-11-03"), c.state.value.anchor)
        assertEquals(CalendarMode.Day, c.state.value.mode)
        assertEquals(reads + 1, backend.ranges.size)
        c.goTo(LocalDate.parse("2026-11-03"))
        assertEquals(reads + 1, backend.ranges.size)
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

    @Test
    fun anEarlierReadOfTheSameRangeThatComesBackLaterIsDropped() {
        val c = controller()
        backend.gate = CompletableDeferred()
        backend.events = listOf(event(1, "2026-11-03T09:00", "2026-11-03T10:00", title = "Before the change"))
        c.refresh()
        val after = listOf(event(2, "2026-11-03T11:00", "2026-11-03T12:00", title = "After the change"))
        backend.events = after
        c.refresh()
        assertEquals(Loaded.Ready(after), c.state.value.events)
        backend.gate?.complete(Unit)
        assertEquals(Loaded.Ready(after), c.state.value.events)
    }

    @Test
    fun anEventReadAfterTheOwnerWentElsewhereIsNotShown() {
        val c = controller()
        c.refresh()
        backend.detailGate = CompletableDeferred()
        c.open(event(1, "2026-11-03T14:00", "2026-11-03T15:00"))
        c.newEvent()
        c.change((c.state.value.page as CalendarPage.Editor).draft.copy(title = "Being written"))
        backend.detailGate?.complete(Unit)
        assertEquals("Being written", (c.state.value.page as CalendarPage.Editor).draft.title)
    }

    @Test
    fun saveWaitsForTheSaveOnItsWaySoAnEventIsNeverMadeTwice() {
        val c = controller()
        c.refresh()
        c.newEvent()
        c.change((c.state.value.page as CalendarPage.Editor).draft.copy(title = "Lunch"))
        backend.runGate = CompletableDeferred()
        c.save()
        assertTrue(c.state.value.saving)
        c.save()
        assertEquals(1, backend.commands.size)
        backend.runGate?.complete(Unit)
        assertFalse(c.state.value.saving)
        assertEquals(CalendarPage.Browse, c.state.value.page)
    }

    @Test
    fun anEarlierSaveThatFinishesLeavesALaterEditorAsItIs() {
        val c = controller()
        c.refresh()
        c.newEvent()
        c.change((c.state.value.page as CalendarPage.Editor).draft.copy(title = "A"))
        backend.runGate = CompletableDeferred()
        c.save()
        c.close()
        c.newEvent()
        c.change((c.state.value.page as CalendarPage.Editor).draft.copy(title = "B"))
        assertFalse(c.state.value.saving)
        backend.outcome = Outcome.Refused("No.")
        backend.runGate?.complete(Unit)
        val page = c.state.value.page as CalendarPage.Editor
        assertEquals("B", page.draft.title)
        // A's refusal is said, but not written on B's editor.
        assertNull(page.problem)
        assertEquals("No.", c.state.value.notice)
    }

    @Test
    fun anEventReadAgainAfterAnAnswerIsNotShownOverALaterPage() {
        val c = controller()
        c.refresh()
        backend.detail = detail(guest = true)
        val invite = event(1, "2026-11-03T14:00", "2026-11-03T15:00", invited = true)
        c.open(invite)
        backend.detailGate = CompletableDeferred()
        c.answer(invite, Answer.Accepted)
        c.answerAsk(send = false)
        c.close()
        c.newEvent()
        backend.detailGate?.complete(Unit)
        assertTrue(c.state.value.page is CalendarPage.Editor)
    }

    @Test
    fun aDraftKeptWhileTheAppWasAwayOpensTheEditorAgain() {
        val c = controller()
        val draft = detail().draft(null).copy(title = "Half written")
        c.restore(draft)
        assertEquals(draft, (c.state.value.page as CalendarPage.Editor).draft)
    }

    @Test
    fun anAnswerForOneTimeOfASeriesOpenedOnALaterDayNamesThatDay() {
        val c = controller()
        c.refresh()
        // The series began on 3 November; the time opened is a fortnight later.
        backend.detail = detail(guest = true).copy(repeat = RepeatDraft(Repeat.Weekly, 1, setOf(DayOfWeek.TUESDAY)))
        backend.times = LocalDateTime.parse("2026-11-17T14:00") to LocalDateTime.parse("2026-11-17T15:00")
        c.open(event(1, "2026-11-17T14:00", "2026-11-17T15:00", invited = true))
        val page = c.state.value.page as CalendarPage.Detail
        val asEvent = page.event.asEvent(page.occurrence)
        assertEquals(LocalDateTime.parse("2026-11-17T14:00"), asEvent.start)
        assertTrue(asEvent.repeats)
        c.answer(asEvent, Answer.Accepted)
        c.answerAsk(send = true, occurrenceOnly = true)
        assertEquals(
            "calendar.respond" to "{\"id\":1,\"response\":\"accept\",\"send\":true,\"scope\":\"occurrence\",\"occurrence\":\"2026-11-17\"}",
            backend.commands.last(),
        )
    }

    @Test
    fun aMeetingTheServerKeepsIsNeverOfferedADeletionThatTellsNobody() {
        val meeting = detail(people = true, onServer = true)
        assertTrue(deleteOptions(meeting, EnglishTexts).all { it.second == true })
        val series = meeting.copy(repeat = RepeatDraft(Repeat.Daily))
        assertEquals(listOf(false, true), deleteOptions(series, EnglishTexts).map { it.third })
        assertTrue(deleteOptions(series, EnglishTexts).all { it.second == true })
        // Nobody to tell: the plain choices, which send nothing.
        assertTrue(deleteOptions(detail(), EnglishTexts).all { it.second == null })
    }

    @Test
    fun takingEveryoneOffAServerMeetingStillAsksBeforeWritingToThem() {
        val c = controller()
        backend.calendars = listOf(CalendarInfo(1uL, 5uL, "Work", 0, true))
        c.refresh()
        val meeting = detail(people = true, onServer = true)
        c.edit(meeting, null)
        val bare = (c.state.value.page as CalendarPage.Editor).draft.copy(attendees = "")
        assertTrue(bare.hadPeople)
        c.change(bare)
        c.save()
        assertTrue(backend.commands.isEmpty())
        assertTrue(c.state.value.ask is Ask.Invite)
        c.answerAsk(send = true)
        assertTrue(backend.commands.single().second.contains("\"send\":true"))
    }

    @Test
    fun onlyANewMeetingMayBeKeptHereAndInviteNobody() {
        val fresh = EventDraft.blank(LocalDate.parse("2026-11-03"), NOW, 1uL)
        assertEquals(2, inviteLabels(Ask.Invite(fresh)).size)
        val changed = detail(people = true, onServer = true).draft(null)
        assertEquals(listOf("Send invitations"), inviteLabels(Ask.Invite(changed)))
    }

    @Test
    fun aNewEventOpenedBeforeTheCalendarsLoadedSubmitsTheCalendarItShows() {
        val c = controller()
        c.newEvent()
        assertEquals(0uL, (c.state.value.page as CalendarPage.Editor).draft.calendar)
        backend.calendars = listOf(CalendarInfo(7uL, 0uL, "Personal", 0, true))
        c.refresh()
        val draft = (c.state.value.page as CalendarPage.Editor).draft
        assertEquals(7uL, draft.calendar)
        assertTrue(draft.params(null).contains("\"calendar\":7"))
    }

    @Test
    fun theOpenedRowsOwnTimesAreCarriedIntoTheDetailAndTheEditor() {
        val c = controller()
        backend.detail = detail().copy(
            start = LocalDateTime.parse("2026-03-01T10:00"),
            end = LocalDateTime.parse("2026-03-01T11:00"),
            repeat = RepeatDraft(Repeat.Weekly, 1, setOf(DayOfWeek.SUNDAY)),
        )
        backend.times = LocalDateTime.parse("2026-04-05T11:00") to LocalDateTime.parse("2026-04-05T12:00")
        c.open(event(1, "2026-04-05T11:00", "2026-04-05T12:00"))
        val page = c.state.value.page as CalendarPage.Detail
        assertEquals(LocalDateTime.parse("2026-04-05T11:00"), page.event.span(page.occurrence, page.times).first)
        c.edit(page.event, page.occurrence, page.times)
        val draft = (c.state.value.page as CalendarPage.Editor).draft
        assertEquals(java.time.LocalTime.of(11, 0), draft.startTime)
        // Not touched, so not sent: the series is not moved an hour by looking at it.
        assertFalse(draft.copy(title = "Renamed").params(null).contains("\"start\""))
    }

    @Test
    fun aTimeOfASeriesTheCoreNoLongerHasIsGoneAndNeverShiftedFromTheBaseline() {
        val c = controller()
        // The series moved to 15:00 since the row (14:00 on the 17th) was read: no time on that day.
        backend.detail = detail().copy(repeat = RepeatDraft(Repeat.Weekly, 1, setOf(DayOfWeek.TUESDAY)))
        backend.times = null
        c.open(event(1, "2026-11-17T14:00", "2026-11-17T15:00"))
        assertTrue(c.state.value.page is CalendarPage.Browse)
        assertEquals("The event is gone.", c.state.value.notice)
    }

    @Test
    fun anAllDayTimeOfASeriesTheCoreNoLongerHasIsGoneToo() {
        val c = controller()
        // Only that day was declined: the core gives the series with no time on it.
        backend.detail = detail().copy(allDay = true, repeat = RepeatDraft(Repeat.Daily))
        backend.times = null
        c.open(event(1, "2026-11-17T00:00", "2026-11-18T00:00"))
        assertTrue(c.state.value.page is CalendarPage.Browse)
        assertEquals("The event is gone.", c.state.value.notice)
    }

    @Test
    fun theDetailAndItsTimesOfASeriesComeFromOneReadAndAreReadTogetherAgain() {
        val c = controller()
        backend.detail = detail().copy(repeat = RepeatDraft(Repeat.Weekly, 1, setOf(DayOfWeek.TUESDAY)))
        backend.times = LocalDateTime.parse("2026-11-17T15:00") to LocalDateTime.parse("2026-11-17T16:00")
        // The row said 14:00 (an older state): what is shown is the core's own 15:00.
        c.open(event(1, "2026-11-17T14:00", "2026-11-17T15:00"))
        val page = c.state.value.page as CalendarPage.Detail
        assertEquals(backend.times, page.times)
        backend.times = LocalDateTime.parse("2026-11-17T16:00") to LocalDateTime.parse("2026-11-17T17:00")
        c.answer(page.event.asEvent(page.occurrence, page.times), Answer.Accepted)
        c.answerAsk(send = false, occurrenceOnly = true)
        assertEquals(backend.times, (c.state.value.page as? CalendarPage.Detail)?.times)
    }

    private fun inviteLabels(ask: Ask.Invite): List<String> = inviteOptions(EnglishTexts, ask).map { it.first }
}
