// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.DayOfWeek
import java.util.Base64

/** The longest .ics file that is opened into the calendar: what the core reads of a calendar part. */
internal const val ICS_MAX_BYTES = 2 shl 20

/** A series' rule as the core gave it: freq (`daily`...), interval, days (`mon`...), nth weekday, count, end day. */
internal data class RuleFacts(
    val freq: String,
    val interval: UInt,
    val byDay: List<String>,
    val nth: Byte?,
    val count: UInt?,
    val until: String?,
)

/**
 * The editor's reading of a rule. What the editor can't show it keeps ([RepeatDraft.kept]).
 */
internal fun repeatOf(rule: RuleFacts): RepeatDraft {
    val repeat = Repeat.entries.firstOrNull { it.code == rule.freq } ?: return RepeatDraft(kept = true)
    val days = rule.byDay.mapNotNull { code ->
        DayOfWeek.entries.firstOrNull { it.name.take(CODE).equals(code, ignoreCase = true) }
    }
    val end = endOf(rule)
    val interval = rule.interval.toInt()
    val showable = rule.nth == null && days.size == rule.byDay.size &&
        (repeat == Repeat.Weekly || days.isEmpty()) && interval in 1..RepeatDraft.MAX_INTERVAL &&
        (rule.count == null || rule.until == null) && (rule.until == null || end is RepeatEnd.On)
    return RepeatDraft(
        repeat = repeat,
        interval = interval.coerceIn(1, RepeatDraft.MAX_INTERVAL),
        days = days.toSet(),
        end = end,
        kept = !showable,
    )
}

private fun endOf(rule: RuleFacts): RepeatEnd = when {
    rule.count != null -> RepeatEnd.After(rule.count.toInt().coerceIn(1, RepeatDraft.MAX_COUNT))
    rule.until != null -> parseCoreDate(rule.until)?.let { RepeatEnd.On(it) } ?: RepeatEnd.Never
    else -> RepeatEnd.Never
}

private const val CODE = 3

/** How a series repeats, in words; empty for an unknown freq. */
internal fun describeRepeat(freq: String, interval: UInt, texts: Texts): String {
    val n = interval.coerceAtLeast(1u).toULong()
    return when (freq) {
        "daily" -> if (n == 1uL) texts.t("Every day") else texts.tn(n, "Every {n} day", "Every {n} days")
        "weekly" -> if (n == 1uL) texts.t("Every week") else texts.tn(n, "Every {n} week", "Every {n} weeks")
        "monthly" -> if (n == 1uL) texts.t("Every month") else texts.tn(n, "Every {n} month", "Every {n} months")
        "yearly" -> if (n == 1uL) texts.t("Every year") else texts.tn(n, "Every {n} year", "Every {n} years")
        else -> ""
    }
}

private const val MINUTES_IN_DAY = 1440
private const val MINUTES_IN_WEEK = 10080

/** A reminder choice in words. */
internal fun reminderLabel(minutes: Int?, texts: Texts): String = when {
    minutes == null -> texts.t("No reminder")
    minutes == 0 -> texts.t("At the time of the event")
    minutes % MINUTES_IN_WEEK == 0 ->
        texts.tn((minutes / MINUTES_IN_WEEK).toULong(), "{n} week before", "{n} weeks before")
    minutes % MINUTES_IN_DAY == 0 ->
        texts.tn((minutes / MINUTES_IN_DAY).toULong(), "{n} day before", "{n} days before")
    minutes % MINUTES_IN_HOUR == 0 ->
        texts.tn((minutes / MINUTES_IN_HOUR).toULong(), "{n} hour before", "{n} hours before")
    else -> texts.tn(minutes.toULong(), "{n} minute before", "{n} minutes before")
}

/** The params of `file.import` for the bytes of a calendar file named [name]. */
internal fun fileImportParams(name: String, bytes: ByteArray): String =
    "{\"name\":${jsonString(icsFileName(name))},\"data\":\"${Base64.getEncoder().encodeToString(bytes)}\"}"

/** A name that is only a name, with the extension the core reads the kind from kept. */
internal fun icsFileName(name: String): String {
    val safe = safeFileName(name)
    return if (safe.lowercase().endsWith(".ics")) safe else "calendar.ics"
}

/** A whole number the core's JSON answer holds under [key]; 0 when it holds none. */
internal fun numberField(json: String, key: String): Int =
    Regex("\"" + Regex.escape(key) + "\"\\s*:\\s*(\\d{1,9})(?!\\d)")
        .find(json)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: 0

/** What an import of a calendar file did, in words. */
internal fun importedNote(said: String, texts: Texts): String {
    val events = numberField(said, "events")
    val tasks = numberField(said, "tasks")
    return when {
        events > 0 && tasks > 0 -> texts.tn(
            events.toULong(),
            "{n} event added to the calendar, and the tasks in the file.",
            "{n} events added to the calendar, and the tasks in the file.",
        )
        events > 0 -> texts.tn(
            events.toULong(),
            "{n} event added to the calendar.",
            "{n} events added to the calendar.",
        )
        else -> texts.t("The file holds no events.")
    }
}
