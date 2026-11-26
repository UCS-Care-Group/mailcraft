// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import uniffi.mailcraft_mobile.NotificationRecord
import uniffi.mailcraft_mobile.NotificationTopic
import uniffi.mailcraft_mobile.NotifiedMessage

/** The buttons of a new-mail notification: the owner's choice, what each runs, and what an intent may ask. */
class NoticeButtonsTest {
    @Test
    fun deleteReplyAndArchiveByDefault() {
        assertEquals(listOf(NoticeButton.Delete, NoticeButton.Reply, NoticeButton.Archive), NoticeButtons().chosen)
        // Nothing kept yet, or a store that can't be read, is the default.
        assertEquals(NoticeButtons(), NoticeButtons.fromCodes(null))
    }

    @Test
    fun theChoiceIsKeptInItsOrderAndReadBack() {
        val chosen = NoticeButtons(listOf(NoticeButton.Flag, NoticeButton.MarkRead, NoticeButton.Junk))
        assertEquals("flag,markRead,junk", chosen.codes)
        assertEquals(chosen, NoticeButtons.fromCodes(chosen.codes))
        // None chosen is kept as none, not as the default.
        assertEquals(emptyList<NoticeButton>(), NoticeButtons.fromCodes(NoticeButtons(emptyList()).codes).chosen)
    }

    @Test
    fun unknownCodesAndRepeatsAreLeftOutAndNoMoreThanThreeAreRead() {
        val read = NoticeButtons.fromCodes("forward,archive, delete ,archive")
        assertEquals(listOf(NoticeButton.Archive, NoticeButton.Delete), read.chosen)
        assertEquals(3, NoticeButtons.fromCodes("delete,archive,reply,markRead,flag,junk").chosen.size)
    }

    @Test
    fun aButtonTickedGoesLastAndAFourthWaits() {
        val two = NoticeButtons(emptyList()).toggled(NoticeButton.Junk).toggled(NoticeButton.Reply)
        assertEquals(listOf(NoticeButton.Junk, NoticeButton.Reply), two.chosen)
        val three = two.toggled(NoticeButton.Flag)
        assertEquals(three, three.toggled(NoticeButton.Delete))
        assertEquals(listOf(NoticeButton.Reply, NoticeButton.Flag), three.toggled(NoticeButton.Junk).chosen)
    }

    @Test
    fun eachButtonRunsTheListsCommandAndReplyRunsNone() {
        assertEquals(
            listOf("mail.delete", "mail.archive", null, "mail.markRead", "mail.flag", "mail.junk"),
            NoticeButton.entries.map { it.triage()?.command() },
        )
        assertEquals(mapOf("ids" to listOf(9uL)), NoticeButton.Delete.triage()?.params(listOf(9uL)))
    }

    @Test
    fun anIntentAsksOnlyForAKnownButtonThatFitsItsActionWithAMessageAndAFolder() {
        assertEquals(NoticeAsk(NoticeButton.Delete, 9uL, 3uL), readNoticeAsk(ACTION_NOTICE_BUTTON, "delete", 9L, 3L))
        assertEquals(NoticeAsk(NoticeButton.Reply, 9uL, 3uL), readNoticeAsk(ACTION_NOTICE_REPLY, "reply", 9L, 3L))
        assertNull(readNoticeAsk(ACTION_NOTICE_BUTTON, "reply", 9L, 3L))
        assertNull(readNoticeAsk(ACTION_NOTICE_REPLY, "delete", 9L, 3L))
        assertNull(readNoticeAsk(ACTION_NOTICE_BUTTON, "deleteForever", 9L, 3L))
        assertNull(readNoticeAsk(ACTION_NOTICE_BUTTON, null, 9L, 3L))
        assertNull(readNoticeAsk(ACTION_NOTICE_BUTTON, "delete", null, 3L))
        assertNull(readNoticeAsk(ACTION_NOTICE_BUTTON, "delete", 9L, null))
        assertNull(readNoticeAsk("android.intent.action.VIEW", "delete", 9L, 3L))
        // A number past Long's top half is the core's own u64, read back as it was written.
        assertEquals(ULong.MAX_VALUE, readNoticeAsk(ACTION_NOTICE_BUTTON, "flag", -1L, 3L)?.message)
    }

    @Test
    fun eachMessageAndButtonHasARequestCodeOfItsOwn() {
        val codes = listOf(1uL, 2uL).flatMap { id -> NoticeButton.entries.map { noticeRequestCode(id, it) } }
        assertEquals(codes.size, codes.toSet().size)
    }

    @Test
    fun theNewMailOnlySwitchTakesAwayWhichMessageItIsAndSoItsButtons() {
        val about = NotifiedMessage(7uL, 3uL)
        val record = NotificationRecord(7uL, NotificationTopic.MAIL, "Ada Example", "Budget review", about)
        assertEquals(NotifiedMessage(7uL, 3uL), privateRecord(record, false, "New mail").message)
        assertNull(privateRecord(record, true, "New mail").message)
    }
}

/** What a notification says once its button was pressed: a short word, nothing of the message. */
class NoticeConfirmationTest {
    private fun said(button: NoticeButton, outcome: NoticeOutcome) = noticeConfirmation(button, outcome, EnglishTexts)

    @Test
    fun eachButtonSaysWhatItDid() {
        assertEquals("1 message deleted", said(NoticeButton.Delete, NoticeOutcome.Done))
        assertEquals("1 message archived", said(NoticeButton.Archive, NoticeOutcome.Done))
        assertEquals("1 message moved to Junk", said(NoticeButton.Junk, NoticeOutcome.Done))
        assertEquals("Marked as read", said(NoticeButton.MarkRead, NoticeOutcome.Done))
        assertEquals("Flagged", said(NoticeButton.Flag, NoticeOutcome.Done))
    }

    @Test
    fun aMessageMovedOrAFailureIsSaidInstead() {
        assertEquals("The message was moved or deleted already", said(NoticeButton.Delete, NoticeOutcome.Moved))
        assertEquals("That did not work.", said(NoticeButton.Archive, NoticeOutcome.Failed))
    }
}
