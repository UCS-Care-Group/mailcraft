// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.Context

/** Where the owner's two swipes are kept: on this device only (nothing is backed up). */
internal interface SwipePrefs {
    fun load(): SwipeConfig

    fun save(config: SwipeConfig)
}

/** [SwipePrefs] in the app's private preferences. A store that cannot be read gives the defaults. */
internal class DeviceSwipePrefs(context: Context) : SwipePrefs {
    private val prefs = runCatching { context.getSharedPreferences(FILE, Context.MODE_PRIVATE) }.getOrNull()

    override fun load(): SwipeConfig = runCatching {
        SwipeConfig.fromCodes(prefs?.getString(START, null), prefs?.getString(END, null))
    }.getOrDefault(SwipeConfig())

    override fun save(config: SwipeConfig) {
        runCatching { prefs?.edit()?.putString(START, config.start.code)?.putString(END, config.end.code)?.apply() }
    }

    private companion object {
        const val FILE = "mail_list"
        const val START = "swipe_start"
        const val END = "swipe_end"
    }
}

/** Keeps the swipes in memory: for tests and screenshots. */
internal class MemorySwipePrefs(private var config: SwipeConfig = SwipeConfig()) : SwipePrefs {
    override fun load(): SwipeConfig = config

    override fun save(config: SwipeConfig) {
        this.config = config
    }
}
