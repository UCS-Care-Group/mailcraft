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
        val mine = generation.incrementAndGet()
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
            if (opened.view.unread && remember(marked, id)) source.markRead(id)
            loadPictures(mine, id, opened)
        }
    }

    /**
     * The owner left message [id] (back, another tab or folder, or the selection cleared). What is still
     * on its way is not asked for any more, and what arrives is dropped and not kept. When another message
     * has been opened since, nothing changes.
     */
    fun close(id: ULong) {
        if (state.value.id != id) return
        generation.incrementAndGet()
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

    /** The picture's state, or null when the owner left the message first (nothing is asked, nothing is kept). */
    private suspend fun download(mine: Int, id: ULong, url: String): PictureState? {
        if (mine != generation.get()) return null
        // Checked again with the permit in hand: a download queued behind others may be a long wait.
        var leftOut = false
        val got = flights.withPermit {
            when {
                mine != generation.get() -> null
                !budget.take(id) -> {
                    leftOut = true
                    null
                }
                else -> source.download(url)
            }
        }
        return when {
            leftOut -> PictureState.LeftOut
            got == null || mine != generation.get() -> null
            got is Fetched.Got -> {
                budget.add(id, got.bytes.size)
                if (looksLikeAPicture(got.bytes)) {
                    withContext(io) { cache.put(url, got.bytes) }
                    PictureState.Ready(got.bytes)
                } else {
                    PictureState.Failed
                }
            }
            else -> PictureState.Failed
        }
    }

    @Synchronized
    private fun remember(set: LinkedHashSet<ULong>, id: ULong): Boolean {
        val added = set.add(id)
        if (set.size > MAX_REMEMBERED_MESSAGES) set.remove(set.first())
        return added
    }

    private companion object {
        /** How many pictures are on their way at a time. */
        const val AT_A_TIME = 4
        const val MAX_PICTURE_REFS = 200
        const val MAX_REMEMBERED_MESSAGES = 500
    }
}
