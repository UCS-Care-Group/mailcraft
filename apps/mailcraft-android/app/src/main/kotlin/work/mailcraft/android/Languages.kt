// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.os.LocaleList
import androidx.appcompat.app.AppCompatDelegate
import androidx.core.os.LocaleListCompat

/** A language the app has a catalogue for, by its code and its own name for itself. */
internal data class LanguageOption(val tag: String, val nativeName: String)

/** The languages offered; `res/xml/locales_config.xml` says the same to the system's own settings. */
internal val LANGUAGES: List<LanguageOption> = listOf(
    LanguageOption("en", "English"),
    LanguageOption("de", "Deutsch"),
    LanguageOption("es", "Español"),
    LanguageOption("fr", "Français"),
    LanguageOption("it", "Italiano"),
    LanguageOption("nl", "Nederlands"),
    LanguageOption("ro", "Română"),
)

/**
 * The language the core is to speak for a locale's language code: one the app offers, or English.
 * (The system has already put the owner's own choice, or its own language, into the configuration.)
 */
internal fun coreLanguage(language: String?): String {
    val code = language?.lowercase(java.util.Locale.ROOT)?.substringBefore('-')?.substringBefore('_')
    return LANGUAGES.firstOrNull { it.tag == code }?.tag ?: LANGUAGES.first().tag
}

/** Which of [LANGUAGES] the owner has chosen in the app (or the system's per-app setting): "" for none. */
internal fun chosenTag(applicationLocales: List<String>): String =
    applicationLocales.firstNotNullOfOrNull { tag ->
        val code = tag.substringBefore('-').lowercase(java.util.Locale.ROOT)
        LANGUAGES.firstOrNull { it.tag == code }?.tag
    }.orEmpty()

/**
 * The owner's choice of language, kept by the system: on Android 13 and later it is the per-app
 * language of `LocaleManager` (the same one the system's settings show), earlier it is AppCompat's
 * own storage of it. Choosing recreates the window in the new language.
 */
internal object LanguageChoice {
    fun current(): String {
        val locales = AppCompatDelegate.getApplicationLocales()
        return chosenTag((0 until locales.size()).mapNotNull { locales.get(it)?.toLanguageTag() })
    }

    fun choose(tag: String) {
        val wanted = if (tag.isEmpty()) LocaleListCompat.getEmptyLocaleList() else LocaleListCompat.forLanguageTags(tag)
        AppCompatDelegate.setApplicationLocales(wanted)
    }

    /** The language of the window's own configuration, which is the owner's choice when there is one. */
    fun inUse(locales: LocaleList): String = coreLanguage(locales.takeIf { !it.isEmpty }?.get(0)?.language)
}
