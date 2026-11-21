// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

/** Invented people, tasks and events (example.org) for the Today, People and Tasks screenshots. */
internal object PeopleTasksShots {
    private val rows = listOf(
        PersonRow(1u, "Ada Example", "Example Ltd"),
        PersonRow(2u, "Al Example", "al@example.org"),
        PersonRow(3u, "Bo Example", "Garden Club"),
        PersonRow(4u, "Cy Example", "cy@example.org"),
        PersonRow(5u, "Di Example", "Example Ltd"),
        PersonRow(6u, "Ed Example", ""),
        PersonRow(7u, "Élan Example", "Studio"),
        PersonRow(8u, "Fay Example", "fay@example.org"),
    )

    private val ada = PersonCard(
        id = 1u,
        name = "Ada Example",
        company = "Example Ltd",
        title = "Engineer",
        department = "Platform",
        emails = listOf(ContactLine("Work", "ada@example.org"), ContactLine("Home", "ada.home@example.org")),
        phones = listOf(ContactLine("Mobile", "+44 20 7946 0000"), ContactLine("Desk", "ext. 4417")),
        addresses = listOf(ContactLine("Work", "1 Example Street, Exampletown, EX1 1AA")),
    )

    val people = PeopleData("", PeopleState.Ready(PeopleList(rows, rows.size)))
    val person = PeopleData("", PeopleState.Ready(PeopleList(rows, rows.size)), 1uL, PersonState.Ready(ada))

    private fun task(id: ULong, title: String, due: String?, done: Boolean = false, important: Boolean = false) =
        TaskRow(id, title, due, done, important)

    private val phone = TaskPlace(0u, "This phone")
    private val account = TaskPlace(1u, "Ada Example")

    private val model = TasksModel(
        today = "2026-10-14",
        groups = listOf(
            TaskGroup(
                0u, "", "This phone", 3, 3,
                listOf(
                    task(1u, "Pay the garden club", "2026-10-12", important = true),
                    task(2u, "Water the plants", "2026-10-14"),
                    task(3u, "Plan the autumn trip with a very long title that has to wrap onto a second line", "2026-10-21"),
                ),
            ),
            TaskGroup(
                1u, "L1", "Ada Example · List 1", 2, 2,
                listOf(task(4u, "Send the report", "2026-10-15"), task(5u, "Order new chairs", null)),
            ),
            TaskGroup(1u, "L2", "Ada Example · List 2", 1, 1, listOf(task(6u, "Book the room", "2026-11-02"))),
        ),
        places = listOf(phone, account),
        offer = LocalOffer(3, listOf(account), answered = false),
        showCompleted = false,
    )

    val tasks = TasksData(TasksState.Ready(model))

    private val draft = TaskDraft(
        id = 2u, title = "Water the plants", notes = "Both shelves, and the fern in the hall.",
        due = "2026-10-14", important = true, account = 0u,
    )
    val editor = TasksData(TasksState.Ready(model), DraftState.Ready(draft.copy(base = draft)))
    val newTask = TasksData(TasksState.Ready(model), DraftState.Ready(newDraft(0u, "2026-10-15")))

    val today = TodayState.Ready(
        TodayModel(
            date = "2026-10-14",
            events = listOf(
                TodayEvent(1u, "Stand-up", "2026-10-14T16:30:00", false, "Room 4", true),
                TodayEvent(2u, "Garden club", "2026-10-15T18:00:00", false, "", false),
                TodayEvent(3u, "Open day", "2026-10-16", true, "Example Hall", false),
            ),
            tasks = listOf(task(1u, "Pay the garden club", "2026-10-12", important = true), task(2u, "Water the plants", "2026-10-14")),
            unread = 7,
        ),
    )

    fun shown(base: ShellData, today: TodayState = this.today, people: PeopleData = this.people, tasks: TasksData = this.tasks) =
        ShellData(
            accounts = base.accounts,
            folderName = base.folderName,
            folderId = base.folderId,
            mail = base.mail,
            version = base.version,
            chosenLanguage = base.chosenLanguage,
            today = today,
            people = people,
            tasks = tasks,
        )
}
