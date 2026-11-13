// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import java.io.File
import java.io.IOException
import java.nio.file.Files
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.OpenVerdict

internal fun item(
    name: String,
    verdict: OpenVerdict = OpenVerdict.OPEN,
    index: UInt = 0u,
    calendar: Boolean = false,
    size: Long = 2_048,
) = AttachmentItem(index, name, name, size, verdict, blocked = if (verdict == OpenVerdict.BLOCKED) "MailCraft doesn't open $name" else "", calendar = calendar)

/** A source over invented files that records what it is asked. */
internal class FakeAttachments : AttachmentSource {
    val reads = mutableListOf<UInt>()
    val opens = mutableListOf<Pair<UInt, Boolean>>()
    val imports = mutableListOf<UInt>()
    val files = mutableMapOf<UInt, AttachmentBytes>()
    var openAnswer: AttachmentOutcome = AttachmentOutcome.Done
    var importAnswer: AttachmentOutcome = AttachmentOutcome.Imported(1, 0)

    override suspend fun bytes(id: ULong, index: UInt): AttachmentBytes {
        reads += index
        return files[index] ?: AttachmentBytes.Failed("the attachment could not be read")
    }

    override suspend fun open(id: ULong, index: UInt, confirmed: Boolean): AttachmentOutcome {
        opens += index to confirmed
        return openAnswer
    }

    override suspend fun importCalendar(id: ULong, index: UInt): AttachmentOutcome {
        imports += index
        return importAnswer
    }
}

internal class FakeDevice : AttachmentDevice {
    val shared = mutableListOf<String>()
    val written = mutableListOf<Pair<String, Int>>()
    var canShare = true
    var canWrite = true

    override fun share(name: String, bytes: ByteArray): Boolean {
        if (canShare) shared += name
        return canShare
    }

    override fun write(target: String, bytes: ByteArray): Boolean {
        if (canWrite) written += target to bytes.size
        return canWrite
    }
}

class AttachmentRoutingTest {
    @Test
    fun anOrdinaryFileOpensSavesAndShares() {
        val file = item("report.pdf")
        assertEquals(listOf(AttachmentAction.Open, AttachmentAction.Save, AttachmentAction.Share), actionsOf(file))
        assertEquals(AttachmentAction.Open, primaryOf(file))
        assertFalse(asksFirst(file.verdict))
    }

    @Test
    fun aBlockedKindCanOnlyBeSaved() {
        for (name in listOf("run.exe", "setup.apk", "noextension", "a.mobileconfig")) {
            val file = item(name, OpenVerdict.BLOCKED)
            assertEquals(name, listOf(AttachmentAction.Save), actionsOf(file))
            assertEquals(AttachmentAction.Save, primaryOf(file))
        }
    }

    @Test
    fun anInstallerIsNeverOpenedOrSharedWhateverTheVerdictSays() {
        // (Defence in depth: the core blocks them, and the app does not rely on that alone.)
        for (name in listOf("a.apk", "B.APKS", "c.xapk", "d.aab")) {
            assertEquals(name, listOf(AttachmentAction.Save), actionsOf(item(name, OpenVerdict.OPEN)))
        }
    }

    @Test
    fun macrosAndScriptsAskFirst() {
        assertTrue(asksFirst(OpenVerdict.MACROS))
        assertTrue(asksFirst(OpenVerdict.SCRIPTS))
        assertFalse(asksFirst(OpenVerdict.OPEN))
        assertFalse(asksFirst(OpenVerdict.BLOCKED))
        // They can still be opened: after the question.
        assertTrue(AttachmentAction.Open in actionsOf(item("budget.xlsm", OpenVerdict.MACROS)))
    }

    @Test
    fun aCalendarFileGoesToTheCalendarFirst() {
        val file = item("invite.ics", calendar = true)
        assertEquals(AttachmentAction.AddToCalendar, primaryOf(file))
        assertEquals(listOf(AttachmentAction.AddToCalendar, AttachmentAction.Open, AttachmentAction.Save, AttachmentAction.Share), actionsOf(file))
    }

    @Test
    fun theKindOfAPictureIsToldByItsNameOnlyToTryItFirst() {
        for (name in listOf("a.png", "B.JPG", "c.jpeg", "d.gif", "e.WebP")) assertTrue(name, looksLikeAPictureByName(name))
        for (name in listOf("a.pdf", "png", "a.svg", "a.png.exe", "")) assertFalse(name, looksLikeAPictureByName(name))
    }

    @Test
    fun theMediaTypeComesFromTheKindElseTheGenericOne() {
        val known = mapOf("pdf" to "application/pdf")
        assertEquals("application/pdf", mimeFor("a.PDF") { known[it] })
        assertEquals("application/octet-stream", mimeFor("a.zzz") { known[it] })
        assertEquals("application/octet-stream", mimeFor("noextension") { known[it] })
    }
}

class StagedFilesTest {
    private val root: File = Files.createTempDirectory("attachments").toFile()

    @After
    fun clean() {
        root.deleteRecursively()
    }

    @Test
    fun aCopyIsInAFolderOfItsOwnUnderTheRoot() {
        val file = stagedFile(root, "0b1c", "report.pdf")
        assertEquals(File(File(root, "0b1c"), "report.pdf").canonicalPath, file.canonicalPath)
    }

    @Test
    fun noNameCanLeaveItsFolder() {
        for (hostile in listOf("../../etc/passwd", "..\\..\\x.pdf", "/abs/x.pdf", "a/../../b", "..", ".", "", "x\u0000y.txt")) {
            val file = stagedFile(root, "0b1c", hostile)
            val inside = file.canonicalFile.parentFile?.canonicalPath == File(root, "0b1c").canonicalPath
            assertTrue("$hostile -> $file", inside)
            assertFalse(file.name.contains('/') || file.name.contains('\\') || file.name.contains('\u0000'))
            assertTrue(file.name.isNotEmpty() && file.name != ".." && file.name != ".")
        }
    }

    @Test
    fun aFolderNameThatIsAPathIsRefused() {
        for (folder in listOf("../x", "a/b", "a\\b", "..")) {
            assertThrows(folder, IOException::class.java) { stagedFile(root, folder, "x.pdf") }
        }
    }

    @Test
    fun twoCopiesOfTheSameNameDoNotCollide() {
        val a = stagedFile(root, "one", "same.pdf")
        val b = stagedFile(root, "two", "same.pdf")
        assertTrue(a.canonicalPath != b.canonicalPath)
    }
}

/** A tap on a row does its first action. */
private fun AttachmentController.tap(id: ULong, item: AttachmentItem) = act(id, item, primaryOf(item))

class AttachmentControllerTest {
    private val source = FakeAttachments()
    private val device = FakeDevice()
    private val controller = AttachmentController(source, device, CoroutineScope(Dispatchers.Unconfined))
    private val png = byteArrayOf(1, 2, 3)

    @Test
    fun anOrdinaryFileIsHandedToTheCore() {
        controller.tap(7uL, item("report.pdf", index = 2u))
        assertEquals(listOf(2u to false), source.opens)
        assertNull(controller.ui.value.question)
        assertFalse(controller.ui.value.busy)
    }

    @Test
    fun aKindThatAsksIsOpenedOnlyAfterYes() {
        val file = item("budget.xlsm", OpenVerdict.MACROS, index = 1u)
        controller.tap(7uL, file)
        assertEquals(file, controller.ui.value.question?.item)
        assertTrue(source.opens.isEmpty())
        controller.dismiss()
        assertNull(controller.ui.value.question)
        assertTrue(source.opens.isEmpty())
        controller.tap(7uL, file)
        controller.confirm()
        assertEquals(listOf(1u to true), source.opens)
    }

    @Test
    fun aBlockedKindIsNeverAskedAboutOrOpenedAndItsBytesAreOnlyReadToSave() {
        val file = item("setup.apk", OpenVerdict.BLOCKED)
        source.files[0u] = AttachmentBytes.Got(png, "setup.apk", picture = false)
        controller.act(7uL, file, AttachmentAction.Open)
        controller.act(7uL, file, AttachmentAction.Share)
        assertTrue(source.opens.isEmpty() && source.reads.isEmpty() && device.shared.isEmpty())
        controller.tap(7uL, file)
        assertEquals("setup.apk", controller.ui.value.saving?.name)
    }

    @Test
    fun aPictureIsPreviewedInTheAppByItsBytes() {
        source.files[0u] = AttachmentBytes.Got(png, "photo.png", picture = true)
        controller.tap(7uL, item("photo.png"))
        val preview = controller.ui.value.preview
        assertNotNull(preview)
        assertTrue(source.opens.isEmpty())
        controller.dismiss()
        assertNull(controller.ui.value.preview)
    }

    @Test
    fun aNameThatSaysPictureIsNotPreviewedWhenTheBytesDoNotAgree() {
        source.files[0u] = AttachmentBytes.Got(png, "photo.png", picture = false)
        controller.tap(7uL, item("photo.png"))
        assertNull(controller.ui.value.preview)
        assertEquals(listOf(0u to false), source.opens)
    }

    @Test
    fun savingAsksWhereAndThenWrites() {
        source.files[0u] = AttachmentBytes.Got(png, "report.pdf", picture = false)
        controller.act(7uL, item("report.pdf"), AttachmentAction.Save)
        assertEquals("report.pdf", controller.ui.value.saving?.name)
        controller.saveTo("content://documents/report.pdf")
        assertEquals(listOf("content://documents/report.pdf" to 3), device.written)
        assertEquals(AttachmentNotice.Saved, controller.ui.value.notice)
        assertNull(controller.ui.value.saving)
    }

    @Test
    fun backingOutOfThePickerWritesNothing() {
        source.files[0u] = AttachmentBytes.Got(png, "report.pdf", picture = false)
        controller.act(7uL, item("report.pdf"), AttachmentAction.Save)
        controller.saveTo(null)
        assertTrue(device.written.isEmpty())
        assertNull(controller.ui.value.notice)
        // Nothing is held for a later save.
        controller.saveTo("content://x")
        assertTrue(device.written.isEmpty())
    }

    @Test
    fun aSaveThatFailsSaysSo() {
        device.canWrite = false
        source.files[0u] = AttachmentBytes.Got(png, "report.pdf", picture = false)
        controller.act(7uL, item("report.pdf"), AttachmentAction.Save)
        controller.saveTo("content://x")
        assertEquals(AttachmentNotice.SaveFailed, controller.ui.value.notice)
    }

    @Test
    fun sharingGivesTheSafeNameAndSaysWhenItCouldNot() {
        source.files[0u] = AttachmentBytes.Got(png, "report.pdf", picture = false)
        controller.act(7uL, item("../report.pdf"), AttachmentAction.Share)
        assertEquals(listOf("report.pdf"), device.shared)
        device.canShare = false
        controller.act(7uL, item("report.pdf"), AttachmentAction.Share)
        assertEquals(AttachmentNotice.ShareFailed, controller.ui.value.notice)
    }

    @Test
    fun theCoresRefusalIsToldInItsOwnWords() {
        source.files[0u] = AttachmentBytes.Failed("the attachment is too large")
        controller.act(7uL, item("big.pdf"), AttachmentAction.Share)
        assertEquals("the attachment is too large", (controller.ui.value.notice as AttachmentNotice.Said).why)
        source.openAnswer = AttachmentOutcome.NoApp
        controller.tap(7uL, item("a.pdf"))
        assertEquals(AttachmentNotice.NoApp, controller.ui.value.notice)
    }

    @Test
    fun aCalendarFileIsBroughtIn() {
        controller.tap(7uL, item("invite.ics", calendar = true, index = 3u))
        assertEquals(listOf(3u), source.imports)
        assertTrue(controller.ui.value.notice is AttachmentNotice.Imported)
        assertTrue(source.opens.isEmpty())
    }

    @Test
    fun leavingTheMessageLetsEverythingGo() {
        source.files[0u] = AttachmentBytes.Got(png, "report.pdf", picture = false)
        controller.act(7uL, item("report.pdf"), AttachmentAction.Save)
        controller.reset()
        assertEquals(AttachmentUi(), controller.ui.value)
        controller.saveTo("content://x")
        assertTrue(device.written.isEmpty())
    }

    @Test
    fun theWordsOfTheQuestionAreTheDesktops() {
        val macros = question(EnglishTexts, item("budget.xlsm", OpenVerdict.MACROS))
        assertTrue(macros.startsWith("Open budget.xlsm? Files like this can carry macros"))
        assertTrue(question(EnglishTexts, item("page.html", OpenVerdict.SCRIPTS)).contains("scripts in them run"))
    }
}
