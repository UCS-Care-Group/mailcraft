// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import java.time.LocalDate
import org.junit.Rule
import org.junit.Test

private val NOV_4: LocalDate = CalendarShots.today

/** The Calendar tab on a phone: its three views, an invitation and the editor, light and dark, 100% and 200%. */
class PhoneCalendarScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private fun shoot(name: String, night: Boolean, fontScale: Float, state: CalendarUiState) {
        paparazzi.unsafeUpdateConfig(
            DeviceConfig.PIXEL_5.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = fontScale),
        )
        paparazzi.snapshot(name) { CalendarShots.Shell(state) }
    }

    @Test
    fun agendaLight100() = shoot("calendar_phone_agenda_light_100", false, 1f, CalendarShots.state(CalendarMode.Agenda))

    @Test
    fun agendaDark200() = shoot("calendar_phone_agenda_dark_200", true, 2f, CalendarShots.state(CalendarMode.Agenda))

    @Test
    fun dayLight100() = shoot("calendar_phone_day_light_100", false, 1f, CalendarShots.state(CalendarMode.Day))

    @Test
    fun dayDark200() = shoot("calendar_phone_day_dark_200", true, 2f, CalendarShots.state(CalendarMode.Day))

    @Test
    fun monthLight100() = shoot("calendar_phone_month_light_100", false, 1f, CalendarShots.state(CalendarMode.Month))

    @Test
    fun monthDark200() = shoot("calendar_phone_month_dark_200", true, 2f, CalendarShots.state(CalendarMode.Month))

    @Test
    fun invitationLight100() = shoot(
        "calendar_phone_invitation_light_100",
        false,
        1f,
        CalendarShots.state(CalendarMode.Agenda, page = CalendarPage.Detail(CalendarShots.detail, NOV_4)),
    )

    @Test
    fun invitationDark200() = shoot(
        "calendar_phone_invitation_dark_200",
        true,
        2f,
        CalendarShots.state(CalendarMode.Agenda, page = CalendarPage.Detail(CalendarShots.detail, NOV_4)),
    )

    @Test
    fun editorLight100() = shoot(
        "calendar_phone_editor_light_100",
        false,
        1f,
        CalendarShots.state(CalendarMode.Agenda, page = CalendarPage.Editor(CalendarShots.draft)),
    )

    @Test
    fun editorDark200() = shoot(
        "calendar_phone_editor_dark_200",
        true,
        2f,
        CalendarShots.state(CalendarMode.Agenda, page = CalendarPage.Editor(CalendarShots.draft)),
    )
}

/** The same on a tablet. */
class TabletCalendarScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.NEXUS_10,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private fun shoot(name: String, night: Boolean, fontScale: Float, state: CalendarUiState) {
        paparazzi.unsafeUpdateConfig(
            DeviceConfig.NEXUS_10.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = fontScale),
        )
        paparazzi.snapshot(name) { CalendarShots.Shell(state) }
    }

    @Test
    fun agendaLight100() = shoot("calendar_tablet_agenda_light_100", false, 1f, CalendarShots.state(CalendarMode.Agenda))

    @Test
    fun dayDark100() = shoot("calendar_tablet_day_dark_100", true, 1f, CalendarShots.state(CalendarMode.Day))

    @Test
    fun monthLight200() = shoot("calendar_tablet_month_light_200", false, 2f, CalendarShots.state(CalendarMode.Month))

    @Test
    fun editorDark100() = shoot(
        "calendar_tablet_editor_dark_100",
        true,
        1f,
        CalendarShots.state(CalendarMode.Agenda, page = CalendarPage.Editor(CalendarShots.draft)),
    )
}
