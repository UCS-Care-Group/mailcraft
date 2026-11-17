// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.LocalDate
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

/** What a month cell says to a screen reader: the whole date, whether it is today, and how many events are on it. */
internal fun monthCellDescription(texts: Texts, day: LocalDate, today: Boolean, events: Int, locale: Locale): String {
    val date = DateTimeFormatter.ofLocalizedDate(FormatStyle.FULL).withLocale(locale).format(day)
    val count = if (events == 0) texts.t("No events") else texts.tn(events.toULong(), "{n} event", "{n} events")
    return listOfNotNull(date, texts.t("Today").takeIf { today }, count).joinToString(", ")
}
