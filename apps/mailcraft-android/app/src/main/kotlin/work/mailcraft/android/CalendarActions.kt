// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.LocalDate

/** What the Calendar tab asks of the app: each is one of the controller's calls. */
// One lambda for each thing the tab lets the owner do: a list that long is the tab's whole vocabulary.
@Suppress("LongParameterList")
internal class CalendarActions(
    val onMode: (CalendarMode) -> Unit = {},
    val onToday: () -> Unit = {},
    val onStep: (Int) -> Unit = {},
    val onOpenDay: (LocalDate) -> Unit = {},
    val onOpen: (CalendarEvent) -> Unit = {},
    val onNew: () -> Unit = {},
    val onAnswer: (CalendarEvent, Answer) -> Unit = { _, _ -> },
    val onClose: () -> Unit = {},
    val onEdit: (EventDetailModel, LocalDate) -> Unit = { _, _ -> },
    val onDelete: (EventDetailModel, LocalDate) -> Unit = { _, _ -> },
    val onChange: (EventDraft) -> Unit = {},
    val onSave: () -> Unit = {},
    val onAnswerAsk: (Boolean?, Boolean) -> Unit = { _, _ -> },
    val onCancelAsk: () -> Unit = {},
    val onDismissNotice: () -> Unit = {},
    val onShown: () -> Unit = {},
) {
    companion object {
        fun of(c: CalendarController) = CalendarActions(
            onMode = c::setMode,
            onToday = c::goToday,
            onStep = c::step,
            onOpenDay = c::openDay,
            onOpen = c::open,
            onNew = { c.newEvent() },
            onAnswer = c::answer,
            onClose = c::close,
            onEdit = { detail, day -> c.edit(detail, day) },
            onDelete = { detail, day -> c.delete(detail, day) },
            onChange = c::change,
            onSave = c::save,
            onAnswerAsk = { send, occurrenceOnly -> c.answerAsk(send, occurrenceOnly) },
            onCancelAsk = c::cancelAsk,
            onDismissNotice = c::dismissNotice,
            onShown = c::refresh,
        )
    }
}
