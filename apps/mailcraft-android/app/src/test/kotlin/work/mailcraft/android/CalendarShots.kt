// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MagicNumber", "MaxLineLength", "LongParameterList")

package work.mailcraft.android

import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.time.ZoneId

/** Invented people and meetings (example.org) for the Calendar screenshots; the day is fixed. */
internal object CalendarShots {
    val today: LocalDate = LocalDate.parse("2026-11-04")
    private val zone = ZoneId.of("UTC")
    private const val GREEN = 0xFF2E8B57.toInt()
    private const val ORANGE = 0xFFE07B1F.toInt()
    private const val PURPLE = 0xFF7E57C2.toInt()

    private fun e(
        id: Int,
        start: String,
        end: String,
        title: String,
        color: Int = GREEN,
        location: String = "",
        allDay: Boolean = false,
        invited: Boolean = false,
        answer: Answer = Answer.None,
        repeats: Boolean = false,
    ) = CalendarEvent(
        id = id.toULong(),
        calendar = 1uL,
        color = color,
        title = title,
        location = location,
        start = LocalDateTime.parse(start),
        end = LocalDateTime.parse(end),
        allDay = allDay,
        repeats = repeats,
        meeting = invited,
        cancelled = false,
        invited = invited,
        answer = answer,
    )

    val events = listOf(
        e(1, "2026-11-04T09:00", "2026-11-04T09:30", "Stand-up", location = "Room 4", repeats = true),
        e(2, "2026-11-04T10:00", "2026-11-04T11:00", "Design review with Ada Example", ORANGE, "Room 2"),
        e(3, "2026-11-04T10:30", "2026-11-04T11:30", "One to one with Bo Example", PURPLE),
        e(4, "2026-11-04T12:00", "2026-11-04T13:00", "Lunch with Cy Example", GREEN, "Cafe Example"),
        e(5, "2026-11-04T14:00", "2026-11-04T15:00", "Board review", ORANGE, "Room 1", invited = true),
        e(6, "2026-11-05T00:00", "2026-11-07T00:00", "Team offsite", PURPLE, allDay = true),
        e(7, "2026-11-05T08:30", "2026-11-05T09:15", "Dentist", GREEN),
        e(8, "2026-11-06T16:00", "2026-11-06T17:00", "Budget call", ORANGE, invited = true, answer = Answer.Tentative),
        e(9, "2026-11-09T09:00", "2026-11-09T10:00", "Planning", GREEN, repeats = true),
        e(10, "2026-11-12T13:00", "2026-11-12T14:00", "Review with Di Example", PURPLE, invited = true, answer = Answer.Accepted),
        e(11, "2026-11-20T00:00", "2026-11-21T00:00", "Ed Example's birthday", GREEN, allDay = true),
    )

    private val calendars = listOf(
        CalendarInfo(1uL, 1uL, "Calendar", GREEN, true),
        CalendarInfo(2uL, 1uL, "Birthdays", PURPLE, false),
    )

    fun state(mode: CalendarMode, anchor: LocalDate = today, page: CalendarPage = CalendarPage.Browse) = CalendarUiState(
        mode = mode,
        anchor = anchor,
        today = today,
        events = Loaded.Ready(events),
        calendars = calendars,
        page = page,
    )

    val detail = EventDetailModel(
        id = 5uL,
        calendar = 1uL,
        title = "Board review",
        location = "Room 1",
        description = "Quarterly numbers and the plan for the new year.",
        start = LocalDateTime.parse("2026-11-04T14:00"),
        end = LocalDateTime.parse("2026-11-04T15:00"),
        allDay = false,
        organizer = "Bo Example",
        attendees = listOf(
            AttendeeItem("Bo Example", "bo@example.org", false, Answer.Accepted),
            AttendeeItem("Ada Example", "ada@example.org", false, Answer.None),
            AttendeeItem("Cy Example", "cy@example.org", true, Answer.Tentative),
        ),
        repeat = RepeatDraft(),
        repeatText = "",
        reminder = 15,
        invited = true,
        answer = Answer.None,
        cancelled = false,
        editable = false,
        onServer = true,
    )

    val draft = EventDraft(
        id = null,
        calendar = 1uL,
        title = "Planning session",
        location = "Room 4",
        description = "Bring the draft.",
        attendees = "ada@example.org; bo@example.org",
        startDate = LocalDate.parse("2026-11-09"),
        startTime = LocalTime.of(9, 0),
        endDate = LocalDate.parse("2026-11-09"),
        endTime = LocalTime.of(10, 0),
        reminder = 15,
        repeat = RepeatDraft(Repeat.Weekly, 1, setOf(DayOfWeek.MONDAY), RepeatEnd.After(10)),
    )

    @Composable
    fun Shell(state: CalendarUiState) {
        MailCraftTheme {
            Surface {
                val base = ShellShots.data
                val pane = CalendarPane(state, DayOfWeek.MONDAY, zone, CalendarActions())
                AppShell(EnglishTexts, base, ShellShots.actions, pane, initialNav = NavState(tab = Tab.Calendar))
            }
        }
    }
}
