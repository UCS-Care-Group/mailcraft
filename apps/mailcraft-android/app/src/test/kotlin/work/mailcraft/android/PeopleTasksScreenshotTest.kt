// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test

private fun page(tab: Tab, today: TodayState = PeopleTasksShots.today, people: PeopleData = PeopleTasksShots.people, tasks: TasksData = PeopleTasksShots.tasks) =
    NavState(tab = tab) to PeopleTasksShots.shown(ShellShots.data, today, people, tasks)

/** Today, People and Tasks on a phone, light and dark, at the system's text size and at twice that. */
class PhonePeopleTasksScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = 0.1,
    )

    private fun shoot(name: String, night: Boolean, fontScale: Float, shown: Pair<NavState, ShellData>) {
        paparazzi.unsafeUpdateConfig(
            DeviceConfig.PIXEL_5.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = fontScale),
        )
        paparazzi.snapshot(name) { ShellShots.Shell(shown.first, shown = shown.second) }
    }

    private val todayPage = page(Tab.Today)
    private val peoplePage = page(Tab.People)
    private val personPage = page(Tab.People, people = PeopleTasksShots.person)
    private val tasksPage = page(Tab.Tasks)
    private val editorPage = page(Tab.Tasks, tasks = PeopleTasksShots.editor)

    @Test fun todayLight100() = shoot("phone_today_light_100", false, 1f, todayPage)

    @Test fun todayLight200() = shoot("phone_today_light_200", false, 2f, todayPage)

    @Test fun todayDark100() = shoot("phone_today_dark_100", true, 1f, todayPage)

    @Test fun todayDark200() = shoot("phone_today_dark_200", true, 2f, todayPage)

    @Test fun peopleLight100() = shoot("phone_people_light_100", false, 1f, peoplePage)

    @Test fun peopleLight200() = shoot("phone_people_light_200", false, 2f, peoplePage)

    @Test fun peopleDark100() = shoot("phone_people_dark_100", true, 1f, peoplePage)

    @Test fun peopleDark200() = shoot("phone_people_dark_200", true, 2f, peoplePage)

    @Test fun personLight100() = shoot("phone_person_light_100", false, 1f, personPage)

    @Test fun personLight200() = shoot("phone_person_light_200", false, 2f, personPage)

    @Test fun personDark100() = shoot("phone_person_dark_100", true, 1f, personPage)

    @Test fun personDark200() = shoot("phone_person_dark_200", true, 2f, personPage)

    @Test fun tasksLight100() = shoot("phone_tasks_light_100", false, 1f, tasksPage)

    @Test fun tasksLight200() = shoot("phone_tasks_light_200", false, 2f, tasksPage)

    @Test fun tasksDark100() = shoot("phone_tasks_dark_100", true, 1f, tasksPage)

    @Test fun tasksDark200() = shoot("phone_tasks_dark_200", true, 2f, tasksPage)

    @Test fun editorLight100() = shoot("phone_task_editor_light_100", false, 1f, editorPage)

    @Test fun editorLight200() = shoot("phone_task_editor_light_200", false, 2f, editorPage)

    @Test fun editorDark100() = shoot("phone_task_editor_dark_100", true, 1f, editorPage)

    @Test fun editorDark200() = shoot("phone_task_editor_dark_200", true, 2f, editorPage)
}

/** The same on a tablet: the list and the page opened side by side, and Today in two columns. */
class TabletPeopleTasksScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.NEXUS_10,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = 0.1,
    )

    private fun shoot(name: String, night: Boolean, fontScale: Float, shown: Pair<NavState, ShellData>) {
        paparazzi.unsafeUpdateConfig(
            DeviceConfig.NEXUS_10.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = fontScale),
        )
        paparazzi.snapshot(name) { ShellShots.Shell(shown.first, shown = shown.second) }
    }

    private val todayPage = page(Tab.Today)
    private val peoplePage = page(Tab.People, people = PeopleTasksShots.person)
    private val tasksPage = page(Tab.Tasks, tasks = PeopleTasksShots.editor)

    @Test fun todayLight100() = shoot("tablet_today_light_100", false, 1f, todayPage)

    @Test fun todayLight200() = shoot("tablet_today_light_200", false, 2f, todayPage)

    @Test fun todayDark100() = shoot("tablet_today_dark_100", true, 1f, todayPage)

    @Test fun todayDark200() = shoot("tablet_today_dark_200", true, 2f, todayPage)

    @Test fun peopleLight100() = shoot("tablet_people_light_100", false, 1f, peoplePage)

    @Test fun peopleLight200() = shoot("tablet_people_light_200", false, 2f, peoplePage)

    @Test fun peopleDark100() = shoot("tablet_people_dark_100", true, 1f, peoplePage)

    @Test fun peopleDark200() = shoot("tablet_people_dark_200", true, 2f, peoplePage)

    @Test fun tasksLight100() = shoot("tablet_tasks_light_100", false, 1f, tasksPage)

    @Test fun tasksLight200() = shoot("tablet_tasks_light_200", false, 2f, tasksPage)

    @Test fun tasksDark100() = shoot("tablet_tasks_dark_100", true, 1f, tasksPage)

    @Test fun tasksDark200() = shoot("tablet_tasks_dark_200", true, 2f, tasksPage)
}
