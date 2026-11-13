// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.LabeledRecord
import uniffi.mailcraft_mobile.PeopleWindow
import uniffi.mailcraft_mobile.PersonDetail
import uniffi.mailcraft_mobile.PersonRecord

/** One line of the People list: the name, and what tells two people of one name apart. */
internal data class PersonRow(val id: ULong, val name: String, val detail: String)

/** What the owner has seen of the people so far, a window at a time, and how many there are. */
internal data class PeopleList(val rows: List<PersonRow>, val total: Int) {
    /** Everything that matches is in [rows]. */
    val complete: Boolean get() = rows.size >= total

    /** This list with the window that follows it; a person already listed is not listed twice. */
    fun append(next: PeopleList): PeopleList {
        val seen = rows.mapTo(HashSet()) { it.id }
        return PeopleList(rows + next.rows.filter { seen.add(it.id) }, next.total)
    }
}

internal sealed interface PeopleState {
    data object Loading : PeopleState

    class Ready(val list: PeopleList) : PeopleState

    class Failed(val message: String) : PeopleState
}

/** A label and a value on a person's page: a number, an address. */
internal data class ContactLine(val label: String, val value: String)

/** A person's page. Read-only: the app offers no editing in this version. */
internal data class PersonCard(
    val id: ULong,
    val name: String,
    val company: String,
    val title: String,
    val department: String,
    val emails: List<ContactLine>,
    val phones: List<ContactLine>,
    val addresses: List<ContactLine>,
) {
    /** The job and the company on one line, as far as the card says them. */
    val subtitle: String get() = listOf(title, department, company).filter { it.isNotBlank() }.joinToString(" · ")
}

internal sealed interface PersonState {
    data object Loading : PersonState

    class Ready(val card: PersonCard) : PersonState

    class Failed(val message: String) : PersonState
}

/** The people the core hands over at a time. */
internal const val PEOPLE_WINDOW = 100u

/** The longest search the app passes on. */
private const val MAX_SEARCH = 200

internal fun personRow(record: PersonRecord): PersonRow = PersonRow(
    id = record.id,
    name = record.name.ifBlank { record.emails.firstOrNull().orEmpty() },
    detail = record.company.ifBlank { record.emails.firstOrNull().orEmpty() },
)

private fun line(record: LabeledRecord) = ContactLine(record.label, record.value)

internal fun personCard(detail: PersonDetail): PersonCard = PersonCard(
    id = detail.id,
    name = detail.name.ifBlank { detail.emails.firstOrNull()?.value.orEmpty() },
    company = detail.company,
    title = detail.title,
    department = detail.department,
    emails = detail.emails.map(::line),
    phones = detail.phones.map(::line),
    addresses = detail.addresses.map(::line),
)

internal fun peopleList(window: PeopleWindow): PeopleList =
    PeopleList(window.people.map(::personRow), window.total.toInt())

/** The letter a person is filed under: the first letter of the name, capitalised, or `#`. */
internal fun sectionOf(name: String): String {
    val first = name.trim().firstOrNull { it.isLetterOrDigit() } ?: return "#"
    return if (first.isLetter()) first.uppercase() else "#"
}

/** What the list draws: a heading for each letter, then its people. */
internal sealed interface PeopleItem {
    class Heading(val letter: String) : PeopleItem

    class Person(val row: PersonRow) : PeopleItem
}

internal fun peopleItems(rows: List<PersonRow>): List<PeopleItem> {
    val out = ArrayList<PeopleItem>(rows.size + SECTION_GUESS)
    var last: String? = null
    for (row in rows) {
        val letter = sectionOf(row.name)
        if (letter != last) {
            out.add(PeopleItem.Heading(letter))
            last = letter
        }
        out.add(PeopleItem.Person(row))
    }
    return out
}

private const val SECTION_GUESS = 26

/** The first window of the people whose name, company or address holds [query]. */
internal suspend fun loadPeople(
    core: Core,
    query: String,
    offset: Int,
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
): PeopleState = withContext(dispatcher) {
    try {
        val at = offset.coerceAtLeast(0).toUInt()
        PeopleState.Ready(peopleList(core.people(query.take(MAX_SEARCH), at, PEOPLE_WINDOW)))
    } catch (_: CoreException) {
        PeopleState.Failed(core.t("The people could not be read."))
    }
}

internal suspend fun loadPerson(
    core: Core,
    id: ULong,
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
): PersonState = withContext(dispatcher) {
    try {
        PersonState.Ready(personCard(core.person(id)))
    } catch (_: CoreException) {
        PersonState.Failed(core.t("This person could not be read."))
    }
}
