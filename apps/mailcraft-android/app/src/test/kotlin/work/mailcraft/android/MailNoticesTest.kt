// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.NotificationTopic

/** When the right to notify is asked for, and how the core's keys become the system's numbers. */
class MailNoticesTest {
    private val android13 = 33
    private val android12 = 32

    @Test
    fun theRightToNotifyIsAskedOnceThereIsAnAccountAndNotAtAFirstStart() {
        assertFalse(shouldAskForNotifications(android13, hasAccount = false, askedBefore = false, granted = false))
        assertTrue(shouldAskForNotifications(android13, hasAccount = true, askedBefore = false, granted = false))
    }

    @Test
    fun aNoIsTheOwnersAnswerAndIsNotAskedAgain() {
        assertFalse(shouldAskForNotifications(android13, hasAccount = true, askedBefore = true, granted = false))
    }

    @Test
    fun nothingIsAskedWhenItIsGivenOrBeforeAndroid13() {
        assertFalse(shouldAskForNotifications(android13, hasAccount = true, askedBefore = false, granted = true))
        assertFalse(shouldAskForNotifications(android12, hasAccount = true, askedBefore = false, granted = false))
    }

    @Test
    fun aReminderIsPostedOnlyWhileTheRemindersSwitchIsOnAndMailIsPostedEitherWay() {
        assertTrue(postsNotification(NotificationTopic.REMINDER, remindersOn = true))
        assertFalse(postsNotification(NotificationTopic.REMINDER, remindersOn = false))
        assertTrue(postsNotification(NotificationTopic.MAIL, remindersOn = true))
        assertTrue(postsNotification(NotificationTopic.MAIL, remindersOn = false))
    }

    @Test
    fun keysBecomeNumbersThatKeepNeighboursApart() {
        assertEquals(7, notificationId(7uL))
        assertNotEquals(notificationId(7uL), notificationId(8uL))
        // A key above 32 bits still gives a number, and not the same as its low half alone.
        assertNotEquals(notificationId(7uL), notificationId((1uL shl 32) or 7uL))
        assertEquals(notificationId(ULong.MAX_VALUE), notificationId(ULong.MAX_VALUE))
    }
}
