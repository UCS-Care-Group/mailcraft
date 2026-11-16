// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext

/**
 * The reading pane's logic, apart from any screen: which message is open, what the owner allowed
 * for its pictures, and how each picture is coming along. Pictures that are part of the message come
 * at once; pictures on the web only once the owner allowed them (this message, its sender, its domain,
 * or all mail), only while the message is open, through the checked downloader, within caps, and not
 * at all while the system's data saving holds them back until the owner says so. What is kept in the
 * [cache] is not asked for again.
 */
@Suppress("TooManyFunctions")
internal class ReadingController(
    private val source: ReadSource,
    private val cache: PictureCache,
    private val prefsStore: PicturePrefsStore,
    private val scope: CoroutineScope,
    /** Where the picture cache's files are read and written: never the main thread. */
    private val io: CoroutineDispatcher = Dispatchers.IO,
) {
    private val state = MutableStateFlow(ReadingUi())

    val ui: StateFlow<ReadingUi> get() = state

    /** Moves on when another message is opened, so that an answer to an earlier question is dropped. */
    private val generation = AtomicInteger()

    /** Given when the generation moves on: a download already on its way ends, and nothing more is begun. */
    private var stop = Stop()
    private val asked = LinkedHashSet<ULong>()
    private val tapped = LinkedHashSet<ULong>()
    private val marked = LinkedHashSet<ULong>()
    private val budget = PictureBudget()
    private val flights = Semaphore(AT_A_TIME)

    /** Whether the owner said "Download pictures" for message [id] (it holds until the app closes). */
    @Synchronized
    fun isAsked(id: ULong): Boolean = id in asked

    /** Opens message [id] for a body [widthDp] wide in a dark or light window (again, when either changed). */
    fun open(id: ULong, widthDp: Int, dark: Boolean) {
        val mine = advance()
        // (Pictures that were on their way for an earlier look are asked for again by this one.)
        state.update {
            if (it.id != id) {
                ReadingUi(id = id)
            } else {
                it.copy(pictures = it.pictures.filterValues { p -> p !is PictureState.Pending })
            }
        }
        scope.launch {
            val opened = source.open(id, widthDp, dark)
            if (mine != generation.get()) return@launch
            if (opened == null) {
                state.update { it.copy(load = LoadState.Failed) }
                return@launch
            }
            val prefs = prefsStore.load()
            val access = PicturePolicy.access(prefs, opened.facts, isAsked(id))
            state.update { it.copy(load = LoadState.Ready, opened = opened, access = access, prefs = prefs) }
            if (opened.view.unread && remember(marked, id) && !source.markRead(id)) forget(id)
            loadPictures(mine, id, opened)
        }
    }

    /** Moves to the next generation; what was on its way for the last is stopped. */
    private fun advance(): Int = synchronized(this) {
        stop.give()
        stop = Stop()
        generation.incrementAndGet()
    }

    /**
     * The owner left message [id] (back, another tab or folder, or the selection cleared). What is still
     * on its way is not asked for any more, and what arrives is dropped and not kept. When another message
     * has been opened since, nothing changes.
     */
    fun close(id: ULong) {
        if (state.value.id != id) return
        advance()
        state.value = ReadingUi()
    }

    /** The owner said Download pictures, for this message alone, its sender, or its sender's domain. */
    fun allow(scopeOfAllow: AllowScope) {
        val current = state.value
        val id = current.id ?: return
        val facts = current.facts ?: return
        if (scopeOfAllow !in PicturePolicy.choices(facts)) return
        var prefs = prefsStore.load()
        when (scopeOfAllow) {
            AllowScope.Message -> remember(asked, id)
            AllowScope.Sender -> prefs = prefs.withSender(facts.address)
            AllowScope.Domain -> prefs = prefs.withDomain(facts.address)
        }
        prefsStore.save(prefs)
        reevaluate(prefs)
    }

    /** The owner tapped to download the pictures that data saving held back. */
    fun downloadNow() {
        val id = state.value.id ?: return
        remember(tapped, id)
        reevaluate(prefsStore.load())
    }

    /** The choices in Settings changed (the switch for all mail, or the lists): the open message follows them. */
    fun prefsChanged() = reevaluate(prefsStore.load())

    private fun reevaluate(prefs: PicturePrefs) {
        val current = state.value
        val id = current.id ?: return
        val opened = current.opened ?: return
        val mine = generation.get()
        state.update { it.copy(access = PicturePolicy.access(prefs, opened.facts, isAsked(id)), prefs = prefs) }
        scope.launch { loadPictures(mine, id, opened) }
    }

    private suspend fun loadPictures(mine: Int, id: ULong, opened: OpenedMessage) {
        val refs = pictureRefs(opened.view.blocks).distinctBy { it.part ?: it.src }.take(MAX_PICTURE_REFS)
        val allowed = state.value.access == PictureAccess.Allowed
        val holds = allowed && PicturePolicy.waits(source.dataSaving(), synchronized(this) { id in tapped })
        if (mine != generation.get()) return
        val web = refs.filter { it.part == null && isWebPicture(it.src) && !isTiny(it) }
        state.update { it.copy(waiting = holds && web.isNotEmpty()) }
        // Each picture is claimed (marked as on its way) here, where nothing else runs, and fetched on its own.
        for (ref in refs) {
            val part = ref.part
            if (part != null) {
                if (claim(mine, part, onlyNew = true)) scope.launch { loadPart(mine, id, part) }
            } else if (allowed && ref in web && claim(mine, ref.src, onlyNew = false)) {
                scope.launch { loadWeb(mine, id, ref.src, holds) }
            }
        }
    }

    /**
     * Marks [key] as on its way unless it is already (or, with [onlyNew] false, is waiting): whether
     * it was claimed.
     */
    private fun claim(mine: Int, key: String, onlyNew: Boolean): Boolean {
        if (mine != generation.get()) return false
        val there = state.value.pictures[key]
        if (there != null && (onlyNew || there !is PictureState.Waiting)) return false
        put(mine, key, PictureState.Pending)
        return true
    }

    private fun put(mine: Int, key: String, picture: PictureState) {
        if (mine == generation.get()) state.update { it.copy(pictures = it.pictures + (key to picture)) }
    }

    private suspend fun loadPart(mine: Int, id: ULong, part: String) {
        val bytes = source.part(id, part)
        put(mine, part, if (bytes == null) PictureState.Failed else PictureState.Ready(bytes))
    }

    /** What is kept is shown at once; else data saving holds it for a tap; else it is downloaded. */
    private suspend fun loadWeb(mine: Int, id: ULong, url: String, holds: Boolean) {
        if (mine != generation.get()) return
        val kept = withContext(io) { cache.get(url) }
        val state = when {
            kept != null -> PictureState.Ready(kept)
            holds -> PictureState.Waiting
            else -> download(mine, id, url)
        }
        if (state != null) put(mine, url, state)
    }

    /** Why a download did not go ahead, once its permit was in hand. */
    private enum class Turn { Go, Gone, Revoked, Saving, LeftOut }

    /** The owner's word and the budget, looked at again with the permit in hand (a long wait may have passed). */
    private suspend fun admit(mine: Int, id: ULong): Turn = stale(mine) ?: run {
        // Data saving may have been switched on meanwhile: without a tap the picture waits for one.
        val saving = source.dataSaving()
        // (That call can suspend: the generation and the owner's word are looked at once more after it.)
        stale(mine) ?: when {
            PicturePolicy.waits(saving, synchronized(this) { id in tapped }) -> Turn.Saving
            !budget.take(id) -> Turn.LeftOut
            else -> Turn.Go
        }
    }

    /** Why this look-up is over, or null while it is still the owner's wish. */
    private fun stale(mine: Int): Turn? = when {
        mine != generation.get() -> Turn.Gone
        state.value.access != PictureAccess.Allowed -> Turn.Revoked
        else -> null
    }

    /** The picture's state, or null when the owner left the message first (nothing is asked, nothing is kept). */
    private suspend fun download(mine: Int, id: ULong, url: String): PictureState? {
        if (mine != generation.get()) return null
        var turn = Turn.Go
        val got = flights.withPermit {
            turn = admit(mine, id)
            if (turn == Turn.Go) source.download(url, stopOf(mine)) else null
        }
        // A claim let go of: a later yes in this generation asks again.
        if (turn == Turn.Revoked) release(mine, url)
        if (turn == Turn.Saving && mine == generation.get()) state.update { it.copy(waiting = true) }
        return when {
            turn == Turn.Saving -> PictureState.Waiting
            turn == Turn.LeftOut -> PictureState.LeftOut
            got == null || mine != generation.get() -> null
            got is Fetched.Got -> keep(mine, id, url, got.bytes)
            else -> PictureState.Failed
        }
    }

    private suspend fun keep(mine: Int, id: ULong, url: String, bytes: ByteArray): PictureState? {
        budget.add(id, bytes.size)
        if (!cache.accepts(bytes)) return PictureState.Failed
        // (The generation is read again where the write happens, on the cache's own thread.)
        withContext(io) { if (mine == generation.get()) cache.put(url, bytes) }
        return if (mine == generation.get()) PictureState.Ready(bytes) else null
    }

    /** The signal of generation [mine]; one already given when that generation is past. */
    private fun stopOf(mine: Int): Stop = synchronized(this) {
        if (mine == generation.get()) stop else Stop().also { it.give() }
    }

    /** Lets go of a claim on [url] that was not fetched, so that it may be claimed again. */
    private fun release(mine: Int, url: String) {
        if (mine == generation.get()) state.update { it.copy(pictures = it.pictures - url) }
    }

    @Synchronized
    private fun remember(set: LinkedHashSet<ULong>, id: ULong): Boolean {
        val added = set.add(id)
        if (set.size > MAX_REMEMBERED_MESSAGES) set.remove(set.first())
        return added
    }

    /** A mark that failed is tried again when the message is opened again. */
    @Synchronized
    private fun forget(id: ULong) {
        marked.remove(id)
    }

    private companion object {
        /** How many pictures are on their way at a time. */
        const val AT_A_TIME = 4
        const val MAX_PICTURE_REFS = 200
        const val MAX_REMEMBERED_MESSAGES = 500
    }
}
