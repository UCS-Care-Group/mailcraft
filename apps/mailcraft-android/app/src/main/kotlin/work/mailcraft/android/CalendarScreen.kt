// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalMaterial3Api::class)
@file:Suppress("TooManyFunctions")

package work.mailcraft.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.time.format.TextStyle
import java.util.Locale

private val Gutter = 16.dp
private val Small = 8.dp
private val Hairline = 1.dp
private val Bar = 4.dp
private val HourHeight = 64.dp
private val HourGutter = 76.dp
private val MonthCell = 64.dp
private val Dot = 6.dp
private val DayBadge = 32.dp
private const val LARGE_TEXT = 1.3f
private const val MAX_DOTS = 3
private const val BLOCK_ALPHA = 0.22f
private const val DIM_ALPHA = 0.45f
private const val DEFAULT_SCROLL_MINUTE = 7 * MINUTES_IN_HOUR

@Composable
internal fun rememberLocale(): Locale = LocalConfiguration.current.locales.get(0) ?: Locale.getDefault()

/** The Calendar tab: the page that is open, or the three views with their bar. */
@Composable
internal fun CalendarScreen(
    texts: Texts,
    state: CalendarUiState,
    weekStart: DayOfWeek,
    zone: ZoneId,
    actions: CalendarActions,
    modifier: Modifier = Modifier,
) {
    BackHandler(enabled = state.page != CalendarPage.Browse) { actions.onClose() }
    LaunchedEffect(Unit) { actions.onShown() }
    when (val page = state.page) {
        is CalendarPage.Detail -> EventDetailPage(texts, page, state, actions, modifier)
        is CalendarPage.Editor -> EventEditorPage(texts, page, state, actions, modifier)
        CalendarPage.Browse -> Browse(texts, state, weekStart, zone, actions, modifier)
    }
    state.ask?.let { AskDialog(texts, it, actions) }
}

@Composable
private fun Browse(
    texts: Texts,
    state: CalendarUiState,
    weekStart: DayOfWeek,
    zone: ZoneId,
    actions: CalendarActions,
    modifier: Modifier,
) {
    val locale = rememberLocale()
    Box(modifier = modifier.fillMaxSize()) {
        Column(modifier = Modifier.fillMaxSize()) {
            Controls(texts, state, locale, actions)
            HorizontalDivider()
            Box(modifier = Modifier.weight(1f)) {
                when (val loaded = state.events) {
                    Loaded.Loading -> Centered(texts.t("Loading…"))
                    is Loaded.Failed -> Centered(loaded.message)
                    is Loaded.Ready -> when (state.mode) {
                        CalendarMode.Agenda -> Agenda(texts, loaded.events, state, locale, actions)
                        CalendarMode.Day -> DayView(texts, loaded.events, state, zone, locale, actions)
                        CalendarMode.Month -> MonthView(loaded.events, state, weekStart, locale, actions)
                    }
                }
            }
            state.notice?.let { Notice(texts, it, actions.onDismissNotice) }
        }
        FloatingActionButton(
            onClick = actions.onNew,
            modifier = Modifier.align(Alignment.BottomEnd).padding(Gutter),
        ) {
            Icon(CalendarIcons.Add, contentDescription = texts.t("New event"))
        }
    }
}

@Composable
private fun Notice(texts: Texts, text: String, onDismiss: () -> Unit) {
    Surface(color = MaterialTheme.colorScheme.secondaryContainer, modifier = Modifier.fillMaxWidth()) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(start = Gutter)) {
            Text(text, modifier = Modifier.weight(1f).padding(vertical = Small))
            TextButton(onClick = onDismiss) { Text(texts.t("OK")) }
        }
    }
}

/** The title of what is shown, for the bar and for screen readers. */
internal fun rangeTitle(mode: CalendarMode, anchor: LocalDate, locale: Locale): String = when (mode) {
    CalendarMode.Month -> anchor.format(DateTimeFormatter.ofPattern("LLLL y", locale))
    CalendarMode.Day -> anchor.format(DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM).withLocale(locale))
    CalendarMode.Agenda -> anchor.format(DateTimeFormatter.ofPattern("LLLL y", locale))
}

@Composable
private fun Controls(texts: Texts, state: CalendarUiState, locale: Locale, actions: CalendarActions) {
    Column(
        modifier = Modifier.padding(horizontal = Gutter, vertical = Small),
        verticalArrangement = Arrangement.spacedBy(Small),
    ) {
        SingleChoiceSegmentedButtonRow(modifier = Modifier.fillMaxWidth()) {
            val modes = CalendarMode.entries
            modes.forEachIndexed { index, mode ->
                SegmentedButton(
                    selected = state.mode == mode,
                    onClick = { actions.onMode(mode) },
                    shape = SegmentedButtonDefaults.itemShape(index = index, count = modes.size),
                ) {
                    Text(modeLabel(mode, texts), maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { actions.onStep(-1) }) {
                Icon(CalendarIcons.Previous, contentDescription = texts.t("Previous"))
            }
            Text(
                text = rangeTitle(state.mode, state.anchor, locale),
                style = MaterialTheme.typography.titleMedium,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            TextButton(onClick = actions.onToday) { Text(texts.t("Today"), maxLines = 1) }
            IconButton(onClick = { actions.onStep(1) }) {
                Icon(AppIcons.Next, contentDescription = texts.t("Next"))
            }
        }
    }
}

private fun modeLabel(mode: CalendarMode, texts: Texts): String = when (mode) {
    CalendarMode.Agenda -> texts.t("Agenda")
    CalendarMode.Day -> texts.t("Day")
    CalendarMode.Month -> texts.t("Month")
}

/** When an event is, as a line: its times, or "All day", and a note when it goes on past the day. */
@Composable
internal fun timeLine(texts: Texts, e: CalendarEvent, day: LocalDate, locale: Locale): String {
    if (e.allDay) return texts.t("All day")
    val fmt = DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).withLocale(locale)
    val from = if (e.firstDay.isBefore(day)) texts.t("Continues") else e.start.toLocalTime().format(fmt)
    val to = if (e.lastDay.isAfter(day)) texts.t("Continues") else e.end.toLocalTime().format(fmt)
    return "$from – $to"
}

private fun answerLabel(answer: Answer, texts: Texts): String = when (answer) {
    Answer.None -> texts.t("Not answered")
    Answer.Accepted -> texts.t("Accepted")
    Answer.Tentative -> texts.t("Tentatively accepted")
    Answer.Declined -> texts.t("Declined")
}

@Composable
private fun Agenda(
    texts: Texts,
    events: List<CalendarEvent>,
    state: CalendarUiState,
    locale: Locale,
    actions: CalendarActions,
) {
    val days = agendaDays(events, state.range(DayOfWeek.MONDAY), state.today)
    val fmt = DateTimeFormatter.ofLocalizedDate(FormatStyle.FULL).withLocale(locale)
    if (days.isEmpty()) {
        Centered(texts.t("Nothing on in these days."))
        return
    }
    LazyColumn(contentPadding = PaddingValues(bottom = FabClearance)) {
        days.forEach { day ->
            item(key = "day-${day.date}") {
                val today = if (day.date == state.today) " – " + texts.t("Today") else ""
                Text(
                    text = day.date.format(fmt) + today,
                    style = MaterialTheme.typography.titleSmall,
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = Small),
                )
            }
            if (day.events.isEmpty()) {
                item(key = "none-${day.date}") {
                    Text(
                        text = texts.t("Nothing on."),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = Gutter, vertical = Small),
                    )
                }
            }
            items(day.events, key = { "e-${day.date}-${it.id}-${it.start}" }) { e ->
                AgendaRow(texts, e, day.date, locale, actions)
            }
        }
    }
}

private val FabClearance = 88.dp
private val ShortBlock = 40.dp

@Composable
private fun AgendaRow(texts: Texts, e: CalendarEvent, day: LocalDate, locale: Locale, actions: CalendarActions) {
    val tap = Modifier.fillMaxWidth().clickable { actions.onOpen(e) }
    Column(modifier = tap.padding(horizontal = Gutter, vertical = Small)) {
        Row(modifier = Modifier.height(IntrinsicSize.Min), horizontalArrangement = Arrangement.spacedBy(Small)) {
            Box(modifier = Modifier.width(Bar).fillMaxHeight().background(Color(e.color), RoundedCornerShape(Bar)))
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = e.title.ifBlank { texts.t("(No title)") },
                    fontWeight = FontWeight.Bold,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(timeLine(texts, e, day, locale), style = MaterialTheme.typography.bodyMedium)
                if (e.location.isNotBlank()) {
                    Text(
                        text = e.location,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                if (e.cancelled) {
                    Text(
                        texts.t("Cancelled"),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.error,
                    )
                } else if (e.invited) {
                    Text(
                        text = answerLabel(e.answer, texts),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.secondary,
                    )
                }
            }
        }
        if (e.invited && !e.cancelled && e.answer == Answer.None) {
            AnswerButtons(texts) { actions.onAnswer(e, it) }
        }
    }
    HorizontalDivider(thickness = Hairline)
}

/** Accept, Tentative, Decline: side by side, and one under the other when the text is large. */
@Composable
internal fun AnswerButtons(texts: Texts, onAnswer: (Answer) -> Unit) {
    val large = LocalDensity.current.fontScale > LARGE_TEXT
    val choices = listOf(
        Answer.Accepted to texts.t("Accept"),
        Answer.Tentative to texts.t("Tentative"),
        Answer.Declined to texts.t("Decline"),
    )
    if (large) {
        Column(modifier = Modifier.padding(top = Small), verticalArrangement = Arrangement.spacedBy(Small)) {
            choices.forEach { (answer, label) ->
                OutlinedButton(onClick = { onAnswer(answer) }, modifier = Modifier.fillMaxWidth()) { Text(label) }
            }
        }
    } else {
        Row(modifier = Modifier.padding(top = Small), horizontalArrangement = Arrangement.spacedBy(Small)) {
            choices.forEach { (answer, label) ->
                OutlinedButton(
                    onClick = { onAnswer(answer) },
                    modifier = Modifier.weight(1f),
                    contentPadding = PaddingValues(horizontal = Small),
                ) {
                    Text(label, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
        }
    }
}

@Composable
private fun DayView(
    texts: Texts,
    events: List<CalendarEvent>,
    state: CalendarUiState,
    zone: ZoneId,
    locale: Locale,
    actions: CalendarActions,
) {
    val layout = dayLayout(events, state.anchor, zone)
    val density = LocalDensity.current
    val startMinute = layout.placed.minOfOrNull { it.startMinute }?.minus(MINUTES_IN_HOUR) ?: DEFAULT_SCROLL_MINUTE
    val hours = startMinute.coerceAtLeast(0).toFloat() / MINUTES_IN_HOUR
    val initial = with(density) { (HourHeight * hours).roundToPx() }
    val scroll = rememberScrollState(initial = initial)
    Column(modifier = Modifier.fillMaxSize()) {
        layout.allDay.forEach { e ->
            Row(
                modifier = Modifier.fillMaxWidth().clickable { actions.onOpen(e) }
                    .padding(horizontal = Gutter, vertical = Bar),
                horizontalArrangement = Arrangement.spacedBy(Small),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(modifier = Modifier.size(Dot).background(Color(e.color), CircleShape))
                Text(e.title.ifBlank { texts.t("(No title)") }, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(texts.t("All day"), style = MaterialTheme.typography.labelSmall)
            }
        }
        Box(modifier = Modifier.weight(1f).verticalScroll(scroll)) {
            DayGrid(texts, layout, state.anchor, locale, actions)
        }
    }
}

@Composable
private fun DayGrid(texts: Texts, layout: DayLayout, date: LocalDate, locale: Locale, actions: CalendarActions) {
    val total = HourHeight * (layout.totalMinutes.toFloat() / MINUTES_IN_HOUR)
    val fmt = DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).withLocale(locale)
    BoxWithConstraints(modifier = Modifier.fillMaxWidth().height(total + FabClearance)) {
        layout.hours.forEach { mark ->
            val top = HourHeight * (mark.minute.toFloat() / MINUTES_IN_HOUR)
            Text(
                text = LocalTime.of(mark.hour, 0).format(fmt),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                modifier = Modifier.offset(y = top).width(HourGutter).padding(horizontal = Bar),
            )
            HorizontalDivider(modifier = Modifier.offset(x = HourGutter, y = top + Small), thickness = Hairline)
        }
        val width = maxWidth - HourGutter - Gutter
        layout.placed.forEach { p ->
            val columnWidth = width / p.columns
            val top = HourHeight * (p.startMinute.toFloat() / MINUTES_IN_HOUR)
            val height = HourHeight * ((p.endMinute - p.startMinute).toFloat() / MINUTES_IN_HOUR)
            Column(
                modifier = Modifier
                    .offset(x = HourGutter + columnWidth * p.column, y = top + Small)
                    .size(columnWidth - Bar, height)
                    .clip(RoundedCornerShape(Bar))
                    .background(Color(p.event.color).copy(alpha = BLOCK_ALPHA))
                    .clickable { actions.onOpen(p.event) }
                    .padding(Bar),
            ) {
                val title = p.event.title.ifBlank { texts.t("(No title)") }
                val time = timeLine(texts, p.event, date, locale)
                if (height < ShortBlock) {
                    // A block too short for two lines keeps both in one.
                    Text(
                        "$title, $time",
                        style = MaterialTheme.typography.labelMedium,
                        fontWeight = FontWeight.Bold,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                } else {
                    Text(
                        title,
                        style = MaterialTheme.typography.labelMedium,
                        fontWeight = FontWeight.Bold,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(time, style = MaterialTheme.typography.labelSmall, maxLines = 1)
                }
            }
        }
    }
}

@Composable
private fun MonthView(
    events: List<CalendarEvent>,
    state: CalendarUiState,
    weekStart: DayOfWeek,
    locale: Locale,
    actions: CalendarActions,
) {
    val grid = CalendarPaging.monthGrid(state.anchor, weekStart)
    Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        Row(modifier = Modifier.fillMaxWidth().padding(horizontal = Small)) {
            CalendarPaging.weekDays(weekStart).forEach { d ->
                Text(
                    text = d.getDisplayName(TextStyle.SHORT, locale),
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Clip,
                    modifier = Modifier.weight(1f).padding(vertical = Bar),
                    textAlign = TextAlign.Center,
                )
            }
        }
        grid.forEach { week ->
            Row(modifier = Modifier.fillMaxWidth().padding(horizontal = Small)) {
                week.forEach { day ->
                    MonthCell(day, events.filter { it.isOn(day) }, state, Modifier.weight(1f), actions)
                }
            }
        }
    }
}

@Composable
private fun MonthCell(
    day: LocalDate,
    here: List<CalendarEvent>,
    state: CalendarUiState,
    modifier: Modifier,
    actions: CalendarActions,
) {
    val inMonth = day.month == state.anchor.month
    val today = day == state.today
    val badge = if (today) MaterialTheme.colorScheme.primary else Color.Transparent
    Column(
        modifier = modifier.height(MonthCell).clickable { actions.onOpenDay(day) }.padding(vertical = Bar),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(
            modifier = Modifier.size(DayBadge).background(badge, CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                text = day.dayOfMonth.toString(),
                style = MaterialTheme.typography.bodyMedium,
                color = when {
                    today -> MaterialTheme.colorScheme.onPrimary
                    inMonth -> MaterialTheme.colorScheme.onSurface
                    else -> MaterialTheme.colorScheme.onSurface.copy(alpha = DIM_ALPHA)
                },
                maxLines = 1,
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(Bar), modifier = Modifier.padding(top = Bar)) {
            here.take(MAX_DOTS).forEach { e ->
                val tint = Color(e.color).copy(alpha = if (inMonth) 1f else DIM_ALPHA)
                Box(modifier = Modifier.size(Dot).background(tint, CircleShape))
            }
        }
    }
}
