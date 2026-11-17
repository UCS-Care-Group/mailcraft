// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.VerticalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.Hyphens
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalTime
import java.time.ZoneOffset
import java.time.chrono.IsoChronology
import java.time.format.DateTimeFormatter
import java.time.format.DateTimeFormatterBuilder
import java.time.format.FormatStyle
import java.time.format.TextStyle
import java.util.Locale

/** The narrowest the hours' column of the week gets, and the most of the width it takes (a sixth). */
private val WeekGutter = 36.dp
private const val WEEK_GUTTER_SHARE = 6
private const val HOURS_IN_DAY = 24
private const val TODAY_ALPHA = 0.06f

/** A block of the week at least this wide has room for its times under its title. */
private val RoomForTimes = 96.dp
private val Thin = 2.dp
private val Small = 8.dp

/** What the week view draws of one of its days. */
private class WeekDay(val date: LocalDate, val layout: DayLayout, val today: Boolean) {
    val count: Int get() = layout.allDay.size + layout.placed.size
}

/**
 * The week that holds [anchor], from the locale's first day: a column for each day with its timed
 * events placed as the day view places them (side by side where they overlap), the whole-day events in
 * a row above, today tinted. On a phone the words are cut short to fit; a touch opens the event, a
 * touch on a day's head opens that day, and a screen reader hears each whole.
 */
@Composable
internal fun WeekView(
    texts: Texts,
    events: List<CalendarEvent>,
    anchor: LocalDate,
    today: LocalDate,
    weekStart: DayOfWeek,
    locale: Locale,
    actions: CalendarActions,
) {
    val days = remember(events, anchor, today, weekStart) {
        // By the wall clock (a day of 24 hours), so that the seven columns share one set of hour lines;
        // on the day the clocks change, the day view has that day's own hours.
        CalendarPaging.weekOf(anchor, weekStart).map { WeekDay(it, dayLayout(events, it, ZoneOffset.UTC), it == today) }
    }
    val hourFmt = hourFormat(locale)
    val labels = days.first().layout.hours.map { LocalTime.of(it.hour, 0).format(hourFmt) }
    val labelWidth = widestLabel(labels, MaterialTheme.typography.labelSmall)
    val earliest = days.flatMap { it.layout.placed }.minOfOrNull { it.startMinute }?.minus(MINUTES_IN_HOUR)
    val hours = (earliest ?: DEFAULT_SCROLL_MINUTE).coerceAtLeast(0).toFloat() / MINUTES_IN_HOUR
    val scroll = rememberScrollState(initial = with(LocalDensity.current) { (HourHeight * hours).roundToPx() })
    BoxWithConstraints(modifier = Modifier.fillMaxSize()) {
        val widest = (maxWidth / WEEK_GUTTER_SHARE).coerceAtLeast(WeekGutter)
        val gutter = (labelWidth + Bar * 2).coerceIn(WeekGutter, widest)
        val column = (maxWidth - gutter) / DAYS_IN_WEEK
        val allDayMost = maxHeight / ALL_DAY_SHARE
        Column(modifier = Modifier.fillMaxSize()) {
            WeekHeads(texts, days, gutter, column, locale, actions)
            if (days.any { it.layout.allDay.isNotEmpty() }) {
                WeekAllDay(texts, days, gutter, allDayMost, locale, actions)
            }
            HorizontalDivider()
            Box(modifier = Modifier.weight(1f).verticalScroll(scroll)) {
                WeekGrid(texts, days, labels, gutter, column, locale, actions)
            }
        }
    }
}

/** The clock's hours as the week's side shows them: "9 AM" where the locale's clock has twelve hours, "09" where 24. */
internal fun hourFormat(locale: Locale): DateTimeFormatter {
    val short = DateTimeFormatterBuilder.getLocalizedDateTimePattern(
        null,
        FormatStyle.SHORT,
        IsoChronology.INSTANCE,
        locale,
    )
    return DateTimeFormatter.ofPattern(if (short.contains('a')) "h a" else "HH", locale)
}

/** The days' names and dates across the top; a touch on one opens that day. */
@Composable
private fun WeekHeads(
    texts: Texts,
    days: List<WeekDay>,
    gutter: Dp,
    column: Dp,
    locale: Locale,
    actions: CalendarActions,
) {
    val style = MaterialTheme.typography.labelMedium
    // The short names ("Mon") when the narrowest column has room for them, else the narrow ones ("M").
    val short = days.map { it.date.dayOfWeek.getDisplayName(TextStyle.SHORT, locale) }
    val narrow = widestLabel(short, style) > column - Bar
    Row(modifier = Modifier.fillMaxWidth()) {
        Spacer(modifier = Modifier.width(gutter))
        days.forEach { day ->
            val name = day.date.dayOfWeek.getDisplayName(if (narrow) TextStyle.NARROW else TextStyle.SHORT, locale)
            Column(
                modifier = Modifier.weight(1f)
                    .clickable { actions.onOpenDay(day.date) }
                    .semantics(mergeDescendants = true) {
                        contentDescription = dayDescription(texts, day.date, day.today, day.count, locale)
                    }
                    .padding(vertical = Bar),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(
                    name,
                    style = style,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Clip,
                    textAlign = TextAlign.Center,
                )
                DateBadge(day)
            }
        }
    }
}

/** The day of the month, on the theme's accent when it is today. */
@Composable
private fun DateBadge(day: WeekDay) {
    val badge = if (day.today) MaterialTheme.colorScheme.primary else Color.Transparent
    Box(modifier = Modifier.size(DayBadge).background(badge, CircleShape), contentAlignment = Alignment.Center) {
        Text(
            text = day.date.dayOfMonth.toString(),
            style = MaterialTheme.typography.bodyMedium,
            color = if (day.today) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurface,
            maxLines = 1,
        )
    }
}

/** The whole-day events, each in its days' columns; at most a third of the view, scrolling beyond that. */
@Composable
private fun WeekAllDay(
    texts: Texts,
    days: List<WeekDay>,
    gutter: Dp,
    most: Dp,
    locale: Locale,
    actions: CalendarActions,
) {
    Row(modifier = Modifier.fillMaxWidth().heightIn(max = most).verticalScroll(rememberScrollState())) {
        Text(
            texts.t("All day"),
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.width(gutter).padding(horizontal = Bar),
        )
        days.forEach { day ->
            Column(
                modifier = Modifier.weight(1f).padding(horizontal = Hairline),
                verticalArrangement = Arrangement.spacedBy(Thin),
            ) {
                day.layout.allDay.forEach { e -> AllDayChip(texts, e, day.date, locale, actions) }
            }
        }
    }
}

@Composable
private fun AllDayChip(texts: Texts, e: CalendarEvent, day: LocalDate, locale: Locale, actions: CalendarActions) {
    Text(
        text = e.title.ifBlank { texts.t("(No title)") },
        style = MaterialTheme.typography.labelSmall,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        textDecoration = if (e.cancelled) TextDecoration.LineThrough else null,
        modifier = Modifier.fillMaxWidth()
            .clip(RoundedCornerShape(Bar))
            .background(Color(e.color).copy(alpha = BLOCK_ALPHA))
            .clickable { actions.onOpen(e) }
            .clearAndSetSemantics { contentDescription = blockDescription(texts, e, day, locale) }
            .padding(horizontal = Thin, vertical = Hairline),
    )
}

/** The hours of the seven days: the hour lines across, a line between days, the blocks in their columns. */
@Composable
private fun WeekGrid(
    texts: Texts,
    days: List<WeekDay>,
    labels: List<String>,
    gutter: Dp,
    column: Dp,
    locale: Locale,
    actions: CalendarActions,
) {
    val total = HourHeight * HOURS_IN_DAY
    Box(modifier = Modifier.fillMaxWidth().height(total + Small + CalendarFabRoom)) {
        days.forEachIndexed { i, day ->
            val left = gutter + column * i
            if (day.today) {
                Box(
                    modifier = Modifier.offset(x = left, y = Small).size(column, total)
                        .background(MaterialTheme.colorScheme.primary.copy(alpha = TODAY_ALPHA)),
                )
            }
            VerticalDivider(modifier = Modifier.offset(x = left, y = Small).height(total), thickness = Hairline)
        }
        HourLines(days.first().layout.hours, labels, gutter)
        days.forEachIndexed { i, day ->
            day.layout.placed.forEach { p ->
                val spoken = blockDescription(texts, p.event, day.date, locale)
                val left = gutter + column * i + Hairline
                EventBlock(p, left, column - Hairline, Hairline, spoken, { actions.onOpen(p.event) }) {
                    WeekBlockText(texts, p.event, day.date, locale, column / p.columns, it)
                }
            }
        }
    }
}

/** The words in a block of the week: its title, cut short to fit, and its times when the block has room for them. */
@Composable
private fun WeekBlockText(texts: Texts, e: CalendarEvent, date: LocalDate, locale: Locale, width: Dp, height: Dp) {
    Column(modifier = Modifier.padding(horizontal = Thin)) {
        Text(
            e.title.ifBlank { texts.t("(No title)") },
            // A narrow column breaks a long word with a hyphen rather than anywhere.
            style = MaterialTheme.typography.labelSmall.copy(hyphens = Hyphens.Auto),
            fontWeight = FontWeight.Bold,
            textDecoration = if (e.cancelled) TextDecoration.LineThrough else null,
            overflow = TextOverflow.Ellipsis,
            maxLines = if (height < ShortBlock) 1 else Int.MAX_VALUE,
        )
        if (width >= RoomForTimes && height >= ShortBlock * 2) {
            Text(timeLine(texts, e, date, locale), style = MaterialTheme.typography.labelSmall, maxLines = 1)
        }
    }
}
