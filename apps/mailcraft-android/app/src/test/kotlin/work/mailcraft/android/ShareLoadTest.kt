// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

private const val OWN = "work.mailcraft.android.files"
private const val MB = 1024 * 1024

/** A provider with invented files: it records what it was asked and how much room it was given. */
private class FakeShareReader(private val files: Map<String, ShareRead>) : ShareReader {
    val asked = mutableListOf<Pair<String, Int>>()

    override fun read(uri: String, cap: Int): ShareRead {
        asked += uri to cap
        return when (val r = files[uri]) {
            is ShareRead.Bytes -> if (r.bytes.size > cap) ShareRead.TooLarge(r.name) else r
            else -> r ?: ShareRead.Unreadable
        }
    }
}

private fun uri(n: Int) = "content://other.example/$n"

private fun bytes(name: String?, size: Int, first: ByteArray = byteArrayOf()) =
    ShareRead.Bytes(name, ByteArray(size).also { first.copyInto(it) })

private val PNG = byteArrayOf(0x89.toByte(), 'P'.code.toByte(), 'N'.code.toByte(), 'G'.code.toByte(), 13, 10, 26, 10)

/** Reading shared files under the caps, and saying why one was not attached. */
class ShareLoadTest {
    @Test
    fun oneFileIsReadWithASafeNameAndItsKindFromItsBytes() {
        val reader = FakeShareReader(mapOf(uri(1) to bytes("../../photo.png", 100, PNG)))
        val load = loadShared(listOf(uri(1)), reader, OWN)
        assertEquals(listOf("photo.png"), load.files.map { it.name })
        assertEquals(ShareKind.Png, load.files.single().kind)
        assertTrue(load.refusals.isEmpty())
    }

    @Test
    fun aPictureNameOverOtherBytesIsStillNotAPicture() {
        val reader = FakeShareReader(mapOf(uri(1) to bytes("holiday.png", 100)))
        assertEquals(ShareKind.Other, loadShared(listOf(uri(1)), reader, OWN).files.single().kind)
    }

    @Test
    fun severalFilesAreAllTaken() {
        val reader = FakeShareReader((1..3).associate { uri(it) to bytes("f$it.txt", 10) })
        val load = loadShared((1..3).map(::uri), reader, OWN)
        assertEquals(listOf("f1.txt", "f2.txt", "f3.txt"), load.files.map { it.name })
    }

    @Test
    fun filesBeyondTheCountAreRefusedUnreadAndCountedTogether() {
        val reader = FakeShareReader((1..30).associate { uri(it) to bytes("f$it", 1) })
        val load = loadShared((1..30).map(::uri), reader, OWN)
        assertEquals(MAX_SHARED_FILES, load.files.size)
        assertEquals(MAX_SHARED_FILES, reader.asked.size)
        val refusal = load.refusals.single()
        assertEquals(Why.TooMany, refusal.why)
        assertEquals(10, refusal.count)
    }

    @Test
    fun aFileOverTheCapIsRefusedByNameAndTheRestStillGoes() {
        val reader = FakeShareReader(mapOf(uri(1) to bytes("big.iso", MAX_SHARED_FILE + 1), uri(2) to bytes("small.txt", 5)))
        val load = loadShared(listOf(uri(1), uri(2)), reader, OWN)
        assertEquals(listOf("small.txt"), load.files.map { it.name })
        assertEquals(Why.TooLarge, load.refusals.single().why)
        assertEquals("big.iso", load.refusals.single().name)
    }

    @Test
    fun theTotalIsCappedAndTheRoomLeftIsWhatIsReadAtMost() {
        val reader = FakeShareReader(
            mapOf(
                uri(1) to bytes("a", 20 * MB),
                uri(2) to bytes("b", 20 * MB),
                uri(3) to bytes("c", 20 * MB),
                uri(4) to bytes("d", MB),
            ),
        )
        val load = loadShared((1..4).map(::uri), reader, OWN)
        // 20 + 20 fit in 50; the third has 10 MB of room and is more than that; the fourth fits in the 10 left.
        assertEquals(listOf("a", "b", "d"), load.files.map { it.name })
        assertEquals(listOf(Why.TotalTooLarge), load.refusals.map { it.why })
        assertEquals(10 * MB, reader.asked[2].second)
        assertTrue(load.files.sumOf { it.bytes.size } <= MAX_SHARED_TOTAL)
    }

    @Test
    fun aFullTotalReadsNothingMore() {
        val reader = FakeShareReader(mapOf(uri(1) to bytes("a", MAX_SHARED_FILE), uri(2) to bytes("b", MAX_SHARED_FILE), uri(3) to bytes("c", 1)))
        val load = loadShared((1..3).map(::uri), reader, OWN)
        assertEquals(2, load.files.size)
        assertEquals(2, reader.asked.size)
        assertEquals(Why.TotalTooLarge, load.refusals.single().why)
    }

    @Test
    fun anUnreadableFileIsToldAndTheRestStillGoes() {
        val reader = FakeShareReader(mapOf(uri(2) to bytes("ok.txt", 3)))
        val load = loadShared(listOf(uri(1), uri(2)), reader, OWN)
        assertEquals(listOf("ok.txt"), load.files.map { it.name })
        assertEquals(Why.Unreadable, load.refusals.single().why)
    }

    @Test
    fun privateAndForeignAddressesAreNeverOpened() {
        val reader = FakeShareReader(emptyMap())
        val load = loadShared(
            listOf("file:///data/data/work.mailcraft.android/files/x", "content://$OWN/attachments/y", "http://example.org/z"),
            reader,
            OWN,
        )
        assertTrue(reader.asked.isEmpty())
        assertTrue(load.files.isEmpty())
        assertEquals(List(3) { Why.NotAllowed }, load.refusals.map { it.why })
    }

    @Test
    fun theRefusalsAreSaidInPlainWords() {
        val text = refusalText(
            EnglishTexts,
            listOf(
                Refusal(Why.TooLarge, "big.iso"),
                Refusal(Why.TotalTooLarge, "next.zip"),
                Refusal(Why.Unreadable),
                Refusal(Why.NotAllowed, "x"),
                Refusal(Why.TooMany, count = 1),
                Refusal(Why.TooMany, count = 7),
            ),
        )
        assertTrue(text, text.contains("big.iso was not attached: it is larger than 25 MB."))
        assertTrue(text, text.contains("next.zip was not attached: the files on a message can be 50 MB together."))
        assertTrue(text, text.contains("A file could not be read, so it was not attached."))
        assertTrue(text, text.contains("x was not attached: it is not a file this app may read."))
        assertTrue(text, text.contains("1 more file was not attached: a message takes at most 20 files from a share."))
        assertTrue(text, text.contains("7 more files were not attached: a message takes at most 20 files from a share."))
    }
}
