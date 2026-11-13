// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

private const val SENDTO = "android.intent.action.SENDTO"
private const val VIEW = "android.intent.action.VIEW"
private const val SEND = "android.intent.action.SEND"
private const val SEND_MULTIPLE = "android.intent.action.SEND_MULTIPLE"

/** What an intent from another app is taken for, a shared file's name and kind, and which addresses may be read. */
class ShareIntakeTest {
    private fun route(action: String?, data: String? = null, text: String? = null, subject: String? = null, streams: List<String> = emptyList()) =
        routeIncoming(action, data, text, subject, streams)

    @Test
    fun aMailtoLinkFromSendtoOrViewIsPassedOnWhole() {
        for (action in listOf(SENDTO, VIEW)) {
            val link = "mailto:ada@example.org?cc=bo@example.org&subject=Hi"
            assertEquals(link, (route(action, data = " $link ") as Incoming.Mailto).link)
        }
        assertEquals("MAILTO:ada@example.org", (route(SENDTO, data = "MAILTO:ada@example.org") as Incoming.Mailto).link)
    }

    @Test
    fun aLinkThatIsNotMailtoIsNotOurs() {
        for (data in listOf("http://example.org/", "smsto:123", "javascript:alert(1)", "file:///etc/passwd", "mailtoo:x", "", null)) {
            assertNull("$data", route(SENDTO, data = data))
            assertNull("$data", route(VIEW, data = data))
        }
    }

    @Test
    fun extrasOfAMailtoAreNotUsed() {
        // Only the link counts: the extras another app adds to it are not read, as the desktop reads only the link.
        val got = route(SENDTO, data = "mailto:ada@example.org", text = "body from extras", subject = "subject from extras")
        assertTrue(got is Incoming.Mailto)
    }

    @Test
    fun sharedTextAndASubjectStartAMessage() {
        val got = route(SEND, text = "Worth a read\nhttps://example.org/page", subject = "A page") as Incoming.Share
        assertEquals("A page", got.subject)
        assertEquals("Worth a read\nhttps://example.org/page", got.text)
        assertTrue(got.files.isEmpty())
    }

    @Test
    fun oneFileOrSeveralAreAddresses() {
        val one = route(SEND, streams = listOf("content://other.example/1")) as Incoming.Share
        assertEquals(listOf("content://other.example/1"), one.files)
        assertNull(one.text)
        val many = route(SEND_MULTIPLE, streams = listOf("content://other.example/1", "content://other.example/2"), text = "see attached") as Incoming.Share
        assertEquals(2, many.files.size)
        assertEquals("see attached", many.text)
    }

    @Test
    fun aShareOfNothingIsNothing() {
        assertNull(route(SEND))
        assertNull(route(SEND, text = "   \n"))
        assertNull(route(SEND_MULTIPLE, subject = "only a subject"))
    }

    @Test
    fun otherActionsAreIgnored() {
        assertNull(route("android.intent.action.MAIN", text = "x", streams = listOf("content://other.example/1")))
        assertNull(route(null, text = "x"))
        assertNull(route("android.intent.action.EDIT", data = "mailto:ada@example.org"))
    }

    @Test
    fun hugeTextAndCrowdsOfFilesAreCutBeforeAnythingIsDoneWithThem() {
        val got = route(SEND_MULTIPLE, text = "a".repeat(MAX_SHARED_TEXT + 5_000), streams = List(5_000) { "content://other.example/$it" }) as Incoming.Share
        assertEquals(MAX_SHARED_TEXT, got.text!!.length)
        assertTrue(got.files.size <= 200)
        assertTrue(got.files.size > MAX_SHARED_FILES)
    }

    @Test
    fun onlyAnotherProgramsContentAddressesAreRead() {
        val own = "work.mailcraft.android.files"
        assertTrue(shareableUri("content://media/external/images/media/12", own))
        assertTrue(shareableUri("content://com.example.app.provider/files/a.pdf?x=1", own))
        for (bad in listOf(
            "file:///data/data/work.mailcraft.android/files/mailbox.mailcraft",
            "FILE:///etc/passwd",
            "content://work.mailcraft.android.files/attachments/x",
            "content://WORK.MAILCRAFT.ANDROID.FILES/attachments/x",
            "content://user@work.mailcraft.android.files/x",
            "content:///nothing",
            "content://",
            "http://example.org/a.png",
            "android.resource://work.mailcraft.android/raw/x",
            "",
        )) {
            assertFalse(bad, shareableUri(bad, own))
        }
    }

    @Test
    fun aNameIsTheLastPartWithNoControlOrDirectionCharacters() {
        assertEquals("passwd", safeShareName("../../etc/passwd"))
        assertEquals("evil.exe", safeShareName("C:\\Users\\a\\evil.exe"))
        assertEquals("a b.txt", safeShareName("a\u0000 b\r\n.txt"))
        assertEquals("fdp.exe", safeShareName("\u202Efdp.exe"))
        assertEquals("report.pdf", safeShareName("  ..report.pdf.. "))
        assertEquals("attachment", safeShareName(null))
        assertEquals("attachment", safeShareName(""))
        assertEquals("attachment", safeShareName(".."))
        assertEquals("attachment", safeShareName("a/"))
        assertEquals("attachment", safeShareName("\u0007\u202E"))
    }

    @Test
    fun aLongNameIsCutAndKeepsItsExtension() {
        val name = safeShareName("x".repeat(500) + ".pdf")
        assertEquals(MAX_NAME_CHARS, name.length)
        assertTrue(name.endsWith(".pdf"))
        // A cut never splits an emoji.
        val emoji = safeShareName("\uD83D\uDE00".repeat(80))
        assertTrue(emoji.length <= MAX_NAME_CHARS)
        assertTrue(emoji.none { it.isLowSurrogate() } || emoji.length % 2 == 0)
        assertFalse(emoji.last().isHighSurrogate())
        // A long "extension" is part of the name, not kept apart.
        assertTrue(safeShareName("a." + "z".repeat(300)).length <= MAX_NAME_CHARS)
    }

    @Test
    fun theKindComesFromTheBytes() {
        assertEquals(ShareKind.Png, kindOf(byteArrayOf(0x89.toByte(), 'P'.code.toByte(), 'N'.code.toByte(), 'G'.code.toByte(), 13, 10, 26, 10, 0)))
        assertEquals(ShareKind.Jpeg, kindOf(byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xFF.toByte(), 0xE0.toByte())))
        assertEquals(ShareKind.Gif, kindOf("GIF89a....".toByteArray()))
        assertEquals(ShareKind.Gif, kindOf("GIF87a".toByteArray()))
        assertEquals(ShareKind.Webp, kindOf("RIFF\u0000\u0000\u0000\u0000WEBPVP8 ".toByteArray(Charsets.ISO_8859_1)))
        // What a name or a sender says is no evidence; and short or empty data is never a picture.
        assertEquals(ShareKind.Other, kindOf("MZ\u0090\u0000 not a picture.png".toByteArray()))
        assertEquals(ShareKind.Other, kindOf("RIFF\u0000\u0000\u0000\u0000WAVEfmt ".toByteArray(Charsets.ISO_8859_1)))
        assertEquals(ShareKind.Other, kindOf(ByteArray(0)))
        assertEquals(ShareKind.Other, kindOf(byteArrayOf(0xFF.toByte(), 0xD8.toByte())))
        assertEquals(ShareKind.Other, kindOf("GIF8".toByteArray()))
    }

    @Test
    fun aNamelessPictureIsNamedForItsKind() {
        assertEquals("picture.png", safeShareName(null, ShareKind.Png))
        assertEquals("picture.jpg", safeShareName("", ShareKind.Jpeg))
        assertEquals("attachment", safeShareName(null, ShareKind.Other))
        assertEquals("photo.bin", safeShareName("photo.bin", ShareKind.Png))
    }
}
