// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("TooManyFunctions")

package work.mailcraft.android

import android.app.DatePickerDialog
import android.app.TimePickerDialog
import android.content.Context
import android.text.format.DateFormat
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalTime
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.time.format.TextStyle
import java.util.Locale

private val Gutter = 16.dp
private val Small = 8.dp
private val Dot = 10.dp
private const val DESCRIPTION_LINES = 6
private const val FALLBACK_COLOR = 0xFF1B5E8C.toInt()

// ---- The detail page ----------------------------------------------------------------------------

@Composable
internal fun EventDetailPage(
    texts: Texts,
    page: CalendarPage.Detail,
    state: CalendarUiState,
    actions: CalendarActions,
    modifier: Modifier,
) {
    val e = page.event
    val color = state.calendars.firstOrNull { it.id == e.calendar }?.color ?: FALLBACK_COLOR
    Column(
        modifier = modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(Gutter),
        verticalArrangement = Arrangement.spacedBy(Small),
    ) {
        PageBar(texts, texts.t("Event"), onClose = actions.onClose)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Small)) {
            Box(modifier = Modifier.size(Dot).background(Color(color), CircleShape))
            Text(e.title.ifBlank { texts.t("(No title)") }, style = MaterialTheme.typography.headlineSmall)
        }
        DetailFacts(texts, page)
        DetailPeople(texts, e)
        if (e.description.isNotBlank()) {
            HorizontalDivider()
            Text(e.description)
        }
        if (e.invited && !e.cancelled) {
            HorizontalDivider()
            Text(answerText(e.answer, texts), style = MaterialTheme.typography.titleSmall)
            val asEvent = page.asEvent()
            AnswerButtons(texts) { actions.onAnswer(asEvent, it) }
        }
        if (e.editable) {
            HorizontalDivider()
            Row(horizontalArrangement = Arrangement.spacedBy(Small)) {
                Button(onClick = { actions.onEdit(e, page.occurrence) }) { Text(texts.t("Edit")) }
                OutlinedButton(onClick = { actions.onDelete(e, page.occurrence) }) { Text(texts.t("Delete")) }
            }
        }
    }
}

@Composable
private fun DetailFacts(texts: Texts, page: CalendarPage.Detail) {
    val e = page.event
    val locale = rememberLocale()
    val date = DateTimeFormatter.ofLocalizedDate(FormatStyle.FULL).withLocale(locale)
    val time = DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).withLocale(locale)
    val whenText = if (e.allDay) {
        page.occurrence.format(date) + " – " + texts.t("All day")
    } else {
        val from = e.start.toLocalTime().format(time)
        e.start.toLocalDate().format(date) + ", " + from + " – " + e.end.toLocalTime().format(time)
    }
    Text(whenText)
    if (e.repeatText.isNotBlank()) Text(e.repeatText, color = MaterialTheme.colorScheme.onSurfaceVariant)
    if (e.location.isNotBlank()) Labelled(texts.t("Location"), e.location)
    e.reminder?.let { Labelled(texts.t("Reminder"), reminderLabel(it, texts)) }
    if (e.organizer.isNotBlank()) Labelled(texts.t("Organiser"), e.organizer)
    if (e.cancelled) Text(texts.t("Cancelled"), color = MaterialTheme.colorScheme.error)
}

@Composable
private fun DetailPeople(texts: Texts, e: EventDetailModel) {
    if (e.attendees.isEmpty()) return
    Text(texts.t("People"), style = MaterialTheme.typography.titleSmall)
    e.attendees.forEach { a ->
        val optional = if (a.optional) " (" + texts.t("optional") + ")" else ""
        Text(
            text = a.name.ifBlank { a.address } + " – " + answerText(a.answer, texts) + optional,
            style = MaterialTheme.typography.bodyMedium,
        )
    }
}

/** The page's event as the agenda's row, for the answer commands. */
private fun CalendarPage.Detail.asEvent(): CalendarEvent = CalendarEvent(
    id = event.id,
    calendar = event.calendar,
    color = 0,
    title = event.title,
    location = event.location,
    start = event.start,
    end = event.end,
    allDay = event.allDay,
    repeats = event.repeat.repeat != null,
    meeting = event.hasPeople,
    cancelled = event.cancelled,
    invited = event.invited,
    answer = event.answer,
)

private fun answerText(answer: Answer, texts: Texts): String = when (answer) {
    Answer.None -> texts.t("Not answered")
    Answer.Accepted -> texts.t("Accepted")
    Answer.Tentative -> texts.t("Tentatively accepted")
    Answer.Declined -> texts.t("Declined")
}

@Composable
private fun Labelled(label: String, value: String) {
    Column {
        Text(label, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value)
    }
}

@Composable
private fun PageBar(texts: Texts, title: String, onClose: () -> Unit, content: (@Composable () -> Unit)? = null) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = onClose) { Icon(CalendarIcons.Close, contentDescription = texts.t("Close")) }
        Text(
            title,
            style = MaterialTheme.typography.titleMedium,
            modifier = Modifier.weight(1f),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        content?.invoke()
    }
}

// ---- The editor ---------------------------------------------------------------------------------

@Composable
internal fun EventEditorPage(
    texts: Texts,
    page: CalendarPage.Editor,
    state: CalendarUiState,
    actions: CalendarActions,
    modifier: Modifier,
) {
    val d = page.draft
    val locale = rememberLocale()
    val context = LocalContext.current
    Column(modifier = modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = Gutter)) {
        PageBar(
            texts = texts,
            title = if (d.isNew) texts.t("New event") else texts.t("Edit event"),
            onClose = actions.onClose,
            content = { TextButton(onClick = actions.onSave) { Text(texts.t("Save")) } },
        )
        page.problem?.let {
            Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(bottom = Small))
        }
        OutlinedTextField(
            value = d.title,
            onValueChange = { actions.onChange(d.copy(title = it)) },
            label = { Text(texts.t("Title")) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        SwitchRow(texts.t("All day"), d.allDay) { actions.onChange(d.copy(allDay = it)) }
        DateTimeRow(texts.t("Starts"), d.startDate, d.startTime, d.allDay, locale, context,
            onDate = { actions.onChange(d.withStartDate(it)) }, onTime = { actions.onChange(d.withStartTime(it)) })
        DateTimeRow(texts.t("Ends"), d.endDate, d.endTime, d.allDay, locale, context,
            onDate = { actions.onChange(d.copy(endDate = it)) }, onTime = { actions.onChange(d.copy(endTime = it)) })
        CalendarPicker(texts, d, state.calendars, actions)
        Choice(
            texts.t("Reminder"),
            reminderLabel(d.reminder, texts),
            reminderChoices.map { it to reminderLabel(it, texts) },
        ) {
            actions.onChange(d.copy(reminder = it))
        }
        RepeatEditor(texts, d, locale, context, actions)
        EditorTextFields(texts, d, actions)
    }
}

@Composable
private fun EditorTextFields(texts: Texts, d: EventDraft, actions: CalendarActions) {
        OutlinedTextField(
            value = d.location,
            onValueChange = { actions.onChange(d.copy(location = it)) },
            label = { Text(texts.t("Location")) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth().padding(top = Small),
        )
        OutlinedTextField(
            value = d.attendees,
            onValueChange = { actions.onChange(d.copy(attendees = it)) },
            label = { Text(texts.t("People to invite")) },
            supportingText = { Text(texts.t("Addresses, separated by semicolons")) },
            modifier = Modifier.fillMaxWidth().padding(top = Small),
        )
        OutlinedTextField(
            value = d.description,
            onValueChange = { actions.onChange(d.copy(description = it)) },
            label = { Text(texts.t("Notes")) },
            minLines = 2,
            maxLines = DESCRIPTION_LINES,
            modifier = Modifier.fillMaxWidth().padding(top = Small, bottom = Gutter),
        )
}

@Composable
private fun SwitchRow(label: String, value: Boolean, onChange: (Boolean) -> Unit) {
    Row(modifier = Modifier.fillMaxWidth().padding(vertical = Small), verticalAlignment = Alignment.CenterVertically) {
        Text(label, modifier = Modifier.weight(1f))
        Switch(checked = value, onCheckedChange = onChange)
    }
}

@Composable
private fun DateTimeRow(
    label: String,
    date: LocalDate,
    time: LocalTime,
    allDay: Boolean,
    locale: Locale,
    context: Context,
    onDate: (LocalDate) -> Unit,
    onTime: (LocalTime) -> Unit,
) {
    val dateFmt = DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM).withLocale(locale)
    val timeFmt = DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).withLocale(locale)
    Column(modifier = Modifier.padding(top = Small)) {
        Text(label, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Row(horizontalArrangement = Arrangement.spacedBy(Small)) {
            OutlinedButton(onClick = { pickDate(context, date, onDate) }, modifier = Modifier.weight(1f)) {
                Text(date.format(dateFmt), maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            if (!allDay) {
                OutlinedButton(onClick = { pickTime(context, time, onTime) }) {
                    Text(time.format(timeFmt), maxLines = 1)
                }
            }
        }
    }
}

/** The system's own date picker. */
private fun pickDate(context: Context, date: LocalDate, onPicked: (LocalDate) -> Unit) {
    DatePickerDialog(context, { _, year, month, day -> onPicked(LocalDate.of(year, month + 1, day)) },
        date.year, date.monthValue - 1, date.dayOfMonth).show()
}

/** The system's own time picker, in the clock style the phone is set to. */
private fun pickTime(context: Context, time: LocalTime, onPicked: (LocalTime) -> Unit) {
    TimePickerDialog(context, { _, hour, minute -> onPicked(LocalTime.of(hour, minute)) },
        time.hour, time.minute, DateFormat.is24HourFormat(context)).show()
}

/** A row that opens a menu of [options] and says which is chosen. */
@Composable
private fun <T> Choice(label: String, shown: String, options: List<Pair<T, String>>, onPick: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Column(modifier = Modifier.padding(top = Small)) {
        Text(label, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Box {
            OutlinedButton(onClick = { open = true }, modifier = Modifier.fillMaxWidth()) {
                Text(shown, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                options.forEach { (value, text) ->
                    DropdownMenuItem(text = { Text(text) }, onClick = {
                        open = false
                        onPick(value)
                    })
                }
            }
        }
    }
}

@Composable
private fun CalendarPicker(texts: Texts, d: EventDraft, calendars: List<CalendarInfo>, actions: CalendarActions) {
    val editable = calendars.filter { it.editable }
    // A change keeps its calendar; only a new event with a choice shows one.
    if (!d.isNew || editable.size < 2) return
    val current = editable.firstOrNull { it.id == d.calendar } ?: editable.first()
    Choice(
        texts.t("Calendar"),
        current.name,
        editable.map { it.id to it.name },
    ) { actions.onChange(d.copy(calendar = it)) }
}

@Composable
private fun RepeatEditor(texts: Texts, d: EventDraft, locale: Locale, context: Context, actions: CalendarActions) {
    if (d.occurrence != null) {
        Text(
            text = texts.t("Only this event is changed, not the series."),
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.padding(top = Small),
        )
        return
    }
    if (d.repeat.kept) {
        Labelled(
            texts.t("Repeats"),
            d.repeatText.ifBlank { texts.t("A rule this editor doesn't show; it is kept as it is.") },
        )
        return
    }
    val r = d.repeat
    val options: List<Pair<Repeat?, String>> = listOf(
        null to texts.t("Doesn't repeat"),
        Repeat.Daily to texts.t("Every day"),
        Repeat.Weekly to texts.t("Every week"),
        Repeat.Monthly to texts.t("Every month"),
        Repeat.Yearly to texts.t("Every year"),
    )
    Choice(texts.t("Repeats"), options.first { it.first == r.repeat }.second, options) { picked ->
        val days = if (picked == Repeat.Weekly) setOf(d.startDate.dayOfWeek) else emptySet()
        actions.onChange(d.copy(repeat = r.copy(repeat = picked, days = days)))
    }
    if (r.repeat == Repeat.Weekly) {
        Row(
            horizontalArrangement = Arrangement.spacedBy(Small),
            modifier = Modifier.padding(top = Small).horizontalScroll(rememberScrollState()),
        ) {
            CalendarPaging.weekDays(DayOfWeek.MONDAY).forEach { day ->
                FilterChip(
                    selected = day in r.days,
                    onClick = {
                        val days = if (day in r.days) r.days - day else r.days + day
                        actions.onChange(d.copy(repeat = r.copy(days = days)))
                    },
                    label = { Text(day.getDisplayName(TextStyle.NARROW, locale), maxLines = 1) },
                )
            }
        }
    }
    if (r.repeat != null) RepeatEnds(texts, d, locale, context, actions)
}

@Composable
private fun RepeatEnds(texts: Texts, d: EventDraft, locale: Locale, context: Context, actions: CalendarActions) {
    val r = d.repeat
    val ends: List<Pair<RepeatEnd, String>> = listOf(
        RepeatEnd.Never to texts.t("Never ends"),
        RepeatEnd.After(DEFAULT_COUNT) to
            texts.tn(DEFAULT_COUNT.toULong(), "Ends after {n} time", "Ends after {n} times"),
        RepeatEnd.On(d.startDate.plusMonths(1)) to texts.t("Ends on a day"),
    )
    val mediumDate = DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM).withLocale(locale)
    val shown = when (val e = r.end) {
        RepeatEnd.Never -> texts.t("Never ends")
        is RepeatEnd.After -> texts.tn(e.count.toULong(), "Ends after {n} time", "Ends after {n} times")
        is RepeatEnd.On -> texts.t("Ends on") + " " + e.day.format(mediumDate)
    }
    Choice(texts.t("Ends"), shown, ends) { actions.onChange(d.copy(repeat = r.copy(end = it))) }
    (r.end as? RepeatEnd.On)?.let { on ->
        OutlinedButton(
            onClick = {
                pickDate(context, on.day) { actions.onChange(d.copy(repeat = r.copy(end = RepeatEnd.On(it)))) }
            },
            modifier = Modifier.padding(top = Small),
        ) { Text(on.day.format(mediumDate)) }
    }
}

private const val DEFAULT_COUNT = 10


// ---- Questions ----------------------------------------------------------------------------------

/** What the owner is asked before a command that makes a server write to people. */
@Composable
internal fun AskDialog(texts: Texts, ask: Ask, actions: CalendarActions) {
    val (title, buttons) = when (ask) {
        is Ask.Invite -> texts.t("Send the invitations?") to listOf(
            texts.t("Send invitations") to { actions.onAnswerAsk(true, false) },
            texts.t("Keep it here, invite nobody") to { actions.onAnswerAsk(false, false) },
        )
        is Ask.Reply -> texts.t("Send a response?") to listOf(
            texts.t("Send the response now") to { actions.onAnswerAsk(true, false) },
            texts.t("Don't send a response") to { actions.onAnswerAsk(false, false) },
        ) + if (ask.event.repeats) {
            listOf(texts.t("Only for this time, and send it") to { actions.onAnswerAsk(true, true) })
        } else {
            emptyList()
        }
        is Ask.Delete -> deleteChoices(texts, ask, actions)
    }
    AlertDialog(
        onDismissRequest = actions.onCancelAsk,
        title = { Text(title) },
        text = {
            Column {
                buttons.forEach { (label, action) ->
                    TextButton(onClick = action, modifier = Modifier.fillMaxWidth()) { Text(label) }
                }
                TextButton(
                    onClick = actions.onCancelAsk,
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(texts.t("Cancel")) }
            }
        },
        confirmButton = {},
    )
}

private fun deleteChoices(
    texts: Texts,
    ask: Ask.Delete,
    actions: CalendarActions,
): Pair<String, List<Pair<String, () -> Unit>>> {
    val repeats = ask.detail.repeat.repeat != null || ask.detail.repeat.kept
    val people = ask.detail.hasPeople && ask.detail.onServer && !ask.detail.invited
    val out = ArrayList<Pair<String, () -> Unit>>()
    // The question names what is deleted; the people are written to only on the owner's word.
    if (people) {
        out.add(texts.t("Delete and tell the people") to { actions.onAnswerAsk(true, false) })
        out.add(texts.t("Delete, tell nobody") to { actions.onAnswerAsk(false, false) })
        if (repeats) out.add(texts.t("Only this time, and tell the people") to { actions.onAnswerAsk(true, true) })
    } else {
        out.add(texts.t("Delete") to { actions.onAnswerAsk(null, false) })
        if (repeats) out.add(texts.t("Only this time") to { actions.onAnswerAsk(null, true) })
    }
    return texts.t("Delete this event?") to out
}
