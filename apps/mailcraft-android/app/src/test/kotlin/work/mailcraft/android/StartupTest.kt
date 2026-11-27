// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** Opening the core off the main thread: loading until it is open, and nothing reaches it before. */
class StartupTest {
    // Kept to compare by identity: with assertions on, coroutines debug mode renames a thread to
    // "test-io @coroutine#1" while a coroutine runs on it, so its name is racy to read.
    private val ioThread = AtomicReference<Thread?>(null)
    private val io = Executors.newSingleThreadExecutor { Thread(it, "test-io").also(ioThread::set) }
        .asCoroutineDispatcher()
    private val main = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)

    private fun <T : Any> settled(startup: Startup<T>): Phase<T> = runBlocking {
        withTimeout(TimeUnit.SECONDS.toMillis(10)) { startup.state.first { it !is Phase.Loading } }
    }

    @Test
    fun loadingUntilOpenedThenReadyAndWiredBeforeAnybodySeesIt() {
        val startup = Startup<String>(main, io)
        val opening = CountDownLatch(1)
        val mayFinish = CountDownLatch(1)
        // Written on the io thread, read here: shared safely, or this thread may see it unset.
        val openedOn = AtomicReference<Thread?>(null)
        val wiredWhileLoading = CopyOnWriteArrayList<Boolean>()
        startup.begin(
            open = {
                openedOn.set(Thread.currentThread())
                opening.countDown()
                mayFinish.await(10, TimeUnit.SECONDS)
                "core"
            },
            ready = {
                wiredWhileLoading.add(startup.state.value == Phase.Loading && startup.readyOrNull() == null)
                true
            },
        )
        assertTrue(opening.await(10, TimeUnit.SECONDS))
        // Opening: the window shows it is loading, and a save on stop (or anything else) finds no core.
        assertEquals(Phase.Loading, startup.state.value)
        assertNull(startup.readyOrNull())
        mayFinish.countDown()
        val phase = settled(startup)
        assertTrue(phase is Phase.Ready)
        assertEquals("core", startup.readyOrNull())
        // Opened on the background thread, never the caller's.
        assertNotEquals(Thread.currentThread(), openedOn.get())
        assertSame(ioThread.get(), openedOn.get())
        // Wired before it was said to be ready.
        assertEquals(listOf(true), wiredWhileLoading)
    }

    @Test
    fun itOpensOnceHoweverOftenItIsBegun() {
        val startup = Startup<String>(main, io)
        var opened = 0
        repeat(3) { startup.begin(open = { opened++; "core" }, ready = { true }) }
        settled(startup)
        assertEquals(1, opened)
        assertSame(startup.readyOrNull(), startup.readyOrNull())
    }

    @Test
    fun anOpeningOrAWiringThatFailsIsFailedAndNeverReady() {
        val none = Startup<String>(main, io)
        none.begin(open = { null }, ready = { true })
        assertEquals(Phase.Failed, settled(none))
        assertNull(none.readyOrNull())
        val unwired = Startup<String>(main, io)
        unwired.begin(open = { "core" }, ready = { false })
        assertEquals(Phase.Failed, settled(unwired))
        assertNull(unwired.readyOrNull())
    }
}
