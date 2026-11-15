// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.runtime.Composable
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test

/** Add Account, the approval help, the permission question and Accounts in Settings: light and dark, at the system's text size and twice that. */
class AccountScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    // A long page is drawn on a taller screen, so that the shot holds all of it at twice the text size.
    private fun shoot(name: String, night: Boolean, fontScale: Float, tall: Boolean = false, content: @Composable () -> Unit) {
        val device = DeviceConfig.PIXEL_5
        paparazzi.unsafeUpdateConfig(
            device.copy(
                nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT,
                fontScale = fontScale,
                screenHeight = if (tall) TALL else device.screenHeight,
            ),
        )
        paparazzi.snapshot(name) { content() }
    }

    private val asking = QuestionState.Asking(PermissionAsk(1u, PermissionKind.Tasks))

    @Test
    fun addLight100() = shoot("add_account_light_100", false, 1f) { AccountShots.AddForm() }

    @Test
    fun addLight200() = shoot("add_account_light_200", false, 2f) { AccountShots.AddForm() }

    @Test
    fun addDark100() = shoot("add_account_dark_100", true, 1f) { AccountShots.AddForm() }

    @Test
    fun addDark200() = shoot("add_account_dark_200", true, 2f) { AccountShots.AddForm() }

    @Test
    fun conditionalAccessLight100() = shoot("add_account_conditional_access_light_100", false, 1f) { AccountShots.AddForm(Notice.OrganisationBlocks) }

    @Test
    fun conditionalAccessDark200() = shoot("add_account_conditional_access_dark_200", true, 2f, tall = true) { AccountShots.AddForm(Notice.OrganisationBlocks) }

    @Test
    fun waitingLight100() = shoot("add_account_waiting_light_100", false, 1f) { AccountShots.Waiting() }

    @Test
    fun approvalLight100() = shoot("approval_help_light_100", false, 1f) { AccountShots.Approval() }

    @Test
    fun approvalLight200() = shoot("approval_help_light_200", false, 2f, tall = true) { AccountShots.Approval() }

    @Test
    fun approvalDark100() = shoot("approval_help_dark_100", true, 1f) { AccountShots.Approval(copied = true) }

    @Test
    fun approvalDark200() = shoot("approval_help_dark_200", true, 2f, tall = true) { AccountShots.Approval() }

    @Test
    fun questionLight100() = shoot("permission_question_light_100", false, 1f) { AccountShots.Question(asking) }

    @Test
    fun questionLight200() = shoot("permission_question_light_200", false, 2f) { AccountShots.Question(asking) }

    @Test
    fun questionDark100() = shoot("permission_question_dark_100", true, 1f) { AccountShots.Question(asking) }

    @Test
    fun questionDark200() = shoot("permission_question_dark_200", true, 2f) { AccountShots.Question(asking) }

    @Test
    fun questionToldDark100() = shoot("permission_question_told_dark_100", true, 1f) {
        AccountShots.Question(QuestionState.Told(PermissionAsk(1u, PermissionKind.SharedMail), grantNotice(EnglishTexts, GrantResult.NeedsApproval).orEmpty()))
    }

    @Test
    fun questionWaitingLight100() = shoot("permission_question_waiting_light_100", false, 1f) {
        AccountShots.Question(QuestionState.Waiting(PermissionAsk(1u, PermissionKind.SharedSend)))
    }

    @Test
    fun accountsLight100() = shoot("settings_accounts_light_100", false, 1f) { AccountShots.Accounts() }

    @Test
    fun accountsLight200() = shoot("settings_accounts_light_200", false, 2f, tall = true) { AccountShots.Accounts() }

    @Test
    fun accountsDark100() = shoot("settings_accounts_dark_100", true, 1f) { AccountShots.Accounts() }

    @Test
    fun accountsDark200() = shoot("settings_accounts_dark_200", true, 2f, tall = true) { AccountShots.Accounts() }

    @Test
    fun removeLight100() = shoot("remove_account_light_100", false, 1f) { AccountShots.Remove() }

    @Test
    fun removeDark200() = shoot("remove_account_dark_200", true, 2f) { AccountShots.Remove() }

    @Test
    fun settingsRowsLight100() = shoot("settings_with_accounts_light_100", false, 1f) { AccountShots.SettingsRows() }

    @Test
    fun noAccountMailDark100() = shoot("mail_no_account_dark_100", true, 1f) { AccountShots.NoAccountMail() }
}

// The goldens are drawn on one machine and checked on another; anti-aliasing may differ by a hair.
private const val TOLERANCE = 0.1
private const val TALL = 5200
