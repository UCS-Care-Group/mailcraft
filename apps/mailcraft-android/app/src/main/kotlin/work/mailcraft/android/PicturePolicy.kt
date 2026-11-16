// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

/** Whether a message may show the pictures it refers to on the web. */
internal enum class PictureAccess { Blocked, Allowed }

/**
 * What the owner said about pictures, kept on this device: the switch for all mail, the senders and
 * domains pictures are always downloaded from, and whether sender images are on. Everything is off
 * until the owner turns it on.
 */
internal data class PicturePrefs(
    val all: Boolean = false,
    val senders: Set<String> = emptySet(),
    val domains: Set<String> = emptySet(),
    val senderImages: Boolean = false,
) {
    /** Whether pictures from [address] are always downloaded. */
    fun senderRemembered(address: String): Boolean = PictureAddresses.sender(address)?.let { it in senders } == true

    /** Whether pictures from everybody at [address]'s domain (that domain itself) are always downloaded. */
    fun domainRemembered(address: String): Boolean =
        PictureAddresses.sender(address)?.let(PictureAddresses::domain)?.let { it in domains } == true

    fun remembered(address: String): Boolean = senderRemembered(address) || domainRemembered(address)

    /** These prefs with [address]'s sender remembered, within the cap. */
    fun withSender(address: String): PicturePrefs {
        val kept = PictureAddresses.sender(address) ?: return this
        return if (senders.size >= MAX_REMEMBERED) this else copy(senders = senders + kept)
    }

    /** These prefs with [address]'s domain remembered, within the cap. */
    fun withDomain(address: String): PicturePrefs {
        val kept = PictureAddresses.sender(address)?.let(PictureAddresses::domain) ?: return this
        return if (domains.size >= MAX_REMEMBERED) this else copy(domains = domains + kept)
    }

    companion object {
        /** Most senders whose pictures are always downloaded, and most domains. */
        const val MAX_REMEMBERED = 500
    }
}

/** Addresses as the rules compare them: lower case, one `@`, no space or control character. */
internal object PictureAddresses {
    private const val MAX_ADDRESS = 320

    /** The sender's address in lower case, or null when it is none. */
    fun sender(address: String): String? {
        val a = address.trim().lowercase()
        val plain = a.length <= MAX_ADDRESS && a.none { it.isWhitespace() || it.isISOControl() }
        val at = a.lastIndexOf('@')
        return if (plain && at > 0 && at < a.length - 1) a else null
    }

    /** The domain of a sender's address in lower case. */
    fun domain(address: String): String? = address.substringAfterLast('@', "").trim('.').ifEmpty { null }
}

/** What the rules need to know of the message that the mailbox says: nothing the sender wrote but their address. */
internal data class MessageFacts(
    val address: String,
    val junk: Boolean,
    val vouched: Boolean,
    val known: Boolean,
)

/** One way of allowing the pictures of a message. */
internal enum class AllowScope { Message, Sender, Domain }

/** The desktop's rules for pictures on the web, as plain functions. */
internal object PicturePolicy {
    /**
     * Whether pictures may be shown: not for mail in Junk E-mail but message by message by the
     * owner's own word, and otherwise for all mail when the owner said so, or for a sender or domain
     * the owner remembered when the receiving server vouches for the sender (a `From:` is anybody's to write).
     */
    fun access(prefs: PicturePrefs, facts: MessageFacts, asked: Boolean): PictureAccess {
        val trusted = !facts.junk && (prefs.all || (prefs.remembered(facts.address) && facts.vouched))
        return if (trusted || asked) PictureAccess.Allowed else PictureAccess.Blocked
    }

    /**
     * The choices the banner offers: this message always; the sender and the domain only where
     * remembering them does anything, which is not in Junk E-mail and not for a sender nothing vouches for.
     */
    fun choices(facts: MessageFacts): List<AllowScope> =
        if (facts.junk || !facts.vouched || PictureAddresses.sender(facts.address) == null) {
            listOf(AllowScope.Message)
        } else {
            AllowScope.entries
        }

    /** A remembered sender or domain stands for the message but nothing vouches for the address. */
    fun rememberedButUnvouched(prefs: PicturePrefs, facts: MessageFacts): Boolean =
        !facts.junk && !prefs.all && prefs.remembered(facts.address) && !facts.vouched

    /**
     * Whether a sender's picture may be looked up for a message: the owner turned sender images on;
     * never for mail in Junk E-mail; only when the server vouches for the sender; and for somebody
     * the owner knows or whose mail's pictures the owner lets come ([access]).
     */
    fun senderImage(prefs: PicturePrefs, facts: MessageFacts, asked: Boolean): Boolean =
        prefs.senderImages && !facts.junk && facts.vouched &&
            (facts.known || access(prefs, facts, asked) == PictureAccess.Allowed)

    /** Whether an allowed download waits for the owner's tap: the system's data saving is on. */
    fun waits(dataSaving: Boolean, tapped: Boolean): Boolean = dataSaving && !tapped
}
