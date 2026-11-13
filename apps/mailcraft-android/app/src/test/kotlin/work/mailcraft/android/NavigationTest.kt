// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class NavigationTest {
    @Test
    fun fiveTabsInTheirOrder() {
        assertEquals(
            listOf("Today", "Mail", "Calendar", "People", "Tasks"),
            Tab.entries.map { it.label(EnglishTexts) },
        )
    }

    @Test
    fun settingsHasItsEightSections() {
        assertEquals(
            listOf(
                "Accounts", "Mail to keep offline", "Notifications", "Pictures", "Sender images",
                "Signatures", "Language", "About",
            ),
            SettingsPage.entries.map { it.label(EnglishTexts) },
        )
    }

    @Test
    fun settingsIsReachableFromEveryTabAndBackLeavesTheWayItCame() {
        Tab.entries.forEach { tab ->
            val inTab = NavState().select(tab)
            val inSettings = inTab.openSettings()
            assertEquals(true, inSettings.settings)
            assertEquals(inTab, inSettings.back())
            assertNull(inTab.back())
        }
    }

    @Test
    fun aPageGoesBackToSettingsThenToTheTab() {
        val page = NavState(tab = Tab.Calendar).openSettings().openPage(SettingsPage.Language)
        val settings = page.back()
        assertEquals(NavState(tab = Tab.Calendar, settings = true), settings)
        assertEquals(NavState(tab = Tab.Calendar), settings?.back())
    }

    @Test
    fun choosingATabLeavesSettings() {
        assertEquals(NavState(tab = Tab.Tasks), NavState().openPage(SettingsPage.About).select(Tab.Tasks))
    }

    @Test
    fun addAccountOpensOverWhereTheOwnerWasAndBackLeavesTheWayItCame() {
        val tab = NavState(tab = Tab.Calendar)
        assertEquals(tab, tab.openAddAccount().back())
        val settings = NavState(tab = Tab.Mail, settings = true, page = SettingsPage.Accounts)
        assertEquals(settings, settings.openAddAccount().back())
        assertEquals(settings, settings.openAddAccount().leaveAddAccount())
        assertNull(tab.back())
        // Opening Settings or choosing a tab leaves it.
        assertEquals(false, tab.openAddAccount().openSettings().adding)
        assertEquals(false, tab.openAddAccount().select(Tab.Today).adding)
    }

    @Test
    fun whatIsSavedComesBack() {
        val states = listOf(
            NavState(),
            NavState(tab = Tab.People),
            NavState(tab = Tab.Today, settings = true),
            NavState(tab = Tab.Mail, settings = true, page = SettingsPage.SenderImages),
            NavState(tab = Tab.Today, adding = true),
        )
        states.forEach { s ->
            val saved = listOf<Any?>(s.tab.ordinal, s.settings, s.page?.ordinal ?: -1, s.adding)
            assertEquals(s, NavState.fromSaved(saved))
            // (What was saved before Add Account existed has three parts and is read as before.)
            assertEquals(s.copy(adding = false), NavState.fromSaved(saved.take(3)))
        }
    }

    @Test
    fun whatCannotBeReadIsTheStart() {
        assertEquals(NavState(), NavState.fromSaved(emptyList()))
        assertEquals(NavState(), NavState.fromSaved(listOf("x", 3, null)))
        assertEquals(NavState(), NavState.fromSaved(listOf(99, false, 99)))
        assertEquals(NavState(), NavState.fromSaved(listOf(-4, false, -1)))
        // A page with no settings flag is settings still: there is nowhere else a page can be.
        assertEquals(
            NavState(settings = true, page = SettingsPage.About),
            NavState.fromSaved(listOf(1, false, SettingsPage.About.ordinal)),
        )
    }
}
