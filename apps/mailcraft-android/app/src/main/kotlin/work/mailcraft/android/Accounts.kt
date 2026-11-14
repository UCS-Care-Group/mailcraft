// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.Permission
import uniffi.mailcraft_mobile.PermissionOutcome
import uniffi.mailcraft_mobile.PermissionState
import uniffi.mailcraft_mobile.SignInOutcome

/** A permission that is asked for by itself, after the owner has been asked, never with a sign-in. */
internal enum class PermissionKind {
    Tasks,
    SharedMail,
    SharedSend,
    ;

    fun label(texts: Texts): String = when (this) {
        Tasks -> texts.t("To Do tasks")
        SharedMail -> texts.t("Shared mailboxes")
        SharedSend -> texts.t("Sending from a shared mailbox")
    }

    /** The question put to the owner, in plain words. */
    fun question(texts: Texts): String = when (this) {
        Tasks -> texts.t("Keep your To Do tasks in step?")
        SharedMail -> texts.t("Open shared mailboxes?")
        SharedSend -> texts.t("Send from shared mailboxes?")
    }

    /** What saying yes lets MailCraft do, and what it costs. */
    fun explanation(texts: Texts): String = when (this) {
        Tasks -> texts.t("MailCraft will read and change your Microsoft To Do lists and tasks. Microsoft's page opens in your browser to ask you. Until you say yes, your tasks stay on this phone.")
        SharedMail -> texts.t("MailCraft will read and change the shared mailboxes you have access to. Microsoft's page opens in your browser to ask you.")
        SharedSend -> texts.t("MailCraft will send mail from a shared mailbox, as it or on its behalf. Microsoft's page opens in your browser to ask you.")
    }

    fun toCore(): Permission = when (this) {
        Tasks -> Permission.TASKS
        SharedMail -> Permission.SHARED_MAIL
        SharedSend -> Permission.SHARED_SEND
    }

    companion object {
        fun of(permission: Permission): PermissionKind = when (permission) {
            Permission.TASKS -> Tasks
            Permission.SHARED_MAIL -> SharedMail
            Permission.SHARED_SEND -> SharedSend
        }
    }
}

/** Whether an account's sign-in covers a permission, as the Accounts page says it. */
internal enum class PermissionStatus {
    Checking,
    Allowed,
    NotAllowed,
    SignInNeeded,
    Unknown,
}

/** How a sign-in for an account ended, as the screens read it (the core's words, without its types). */
internal sealed interface SignInResult {
    class SignedIn(val account: ULong, val email: String, val name: String) : SignInResult

    data object NeedsApproval : SignInResult

    data object Declined : SignInResult

    data object TimedOut : SignInResult

    data object Cancelled : SignInResult

    data object OrganisationBlocks : SignInResult

    data object NoBrowser : SignInResult

    class Failed(val reason: String) : SignInResult
}

/** How asking for a permission ended. */
internal sealed interface GrantResult {
    data object Granted : GrantResult

    data object NeedsApproval : GrantResult

    data object OrganisationBlocks : GrantResult

    data object NotForThisAccount : GrantResult

    data object Declined : GrantResult

    data object TimedOut : GrantResult

    data object Cancelled : GrantResult

    data object NoBrowser : GrantResult

    class Failed(val reason: String) : GrantResult
}

/** What the screens ask of the core about accounts: one seam, so that they run without the library. */
internal interface AccountsApi {
    suspend fun signIn(email: String): SignInResult

    suspend fun grant(account: ULong, permission: PermissionKind): GrantResult

    suspend fun status(account: ULong, permission: PermissionKind): PermissionStatus

    /** Removes the account and its sign-in; true when it is gone. */
    suspend fun remove(account: ULong): Boolean

    /** The address of Microsoft's page where an administrator approves MailCraft, or null. */
    fun approvalLink(email: String): String?

    fun cancel()
}

/** [AccountsApi] over the Rust core. What it hands the screens carries no token: the core keeps those. */
internal class CoreAccounts(private val core: Core, private val handback: SignInHandback) : AccountsApi {
    override suspend fun signIn(email: String): SignInResult =
        try {
            when (val outcome = core.signInMicrosoft(email)) {
                is SignInOutcome.SignedIn -> SignInResult.SignedIn(outcome.account, outcome.email, outcome.name)
                is SignInOutcome.NeedsApproval -> SignInResult.NeedsApproval
                is SignInOutcome.Declined -> SignInResult.Declined
                is SignInOutcome.TimedOut -> SignInResult.TimedOut
                is SignInOutcome.Cancelled -> SignInResult.Cancelled
                is SignInOutcome.OrganisationBlocks -> SignInResult.OrganisationBlocks
                is SignInOutcome.NoBrowser -> SignInResult.NoBrowser
                is SignInOutcome.Failed -> SignInResult.Failed(outcome.reason)
            }
        } catch (_: CoreException) {
            SignInResult.Failed("")
        } finally {
            handback.disarm()
        }

    override suspend fun grant(account: ULong, permission: PermissionKind): GrantResult =
        try {
            when (val outcome = core.grantPermission(account, permission.toCore())) {
                is PermissionOutcome.Granted -> GrantResult.Granted
                is PermissionOutcome.NeedsApproval -> GrantResult.NeedsApproval
                is PermissionOutcome.OrganisationBlocks -> GrantResult.OrganisationBlocks
                is PermissionOutcome.NotForThisAccount -> GrantResult.NotForThisAccount
                is PermissionOutcome.Declined -> GrantResult.Declined
                is PermissionOutcome.TimedOut -> GrantResult.TimedOut
                is PermissionOutcome.Cancelled -> GrantResult.Cancelled
                is PermissionOutcome.NoBrowser -> GrantResult.NoBrowser
                is PermissionOutcome.Failed -> GrantResult.Failed(outcome.reason)
            }
        } catch (_: CoreException) {
            GrantResult.Failed("")
        } finally {
            handback.disarm()
        }

    override suspend fun status(account: ULong, permission: PermissionKind): PermissionStatus =
        try {
            when (core.permissionState(account, permission.toCore())) {
                PermissionState.ALLOWED -> PermissionStatus.Allowed
                PermissionState.NOT_ALLOWED -> PermissionStatus.NotAllowed
                PermissionState.SIGN_IN_NEEDED -> PermissionStatus.SignInNeeded
                PermissionState.UNKNOWN -> PermissionStatus.Unknown
            }
        } catch (_: CoreException) {
            PermissionStatus.Unknown
        }

    override suspend fun remove(account: ULong): Boolean =
        try {
            core.removeAccount(account)
            true
        } catch (_: CoreException) {
            false
        }

    override fun approvalLink(email: String): String? =
        try {
            core.adminApprovalLink(email)
        } catch (_: CoreException) {
            null
        }

    override fun cancel() = core.cancelSignIn()
}

/** Why a sign-in that brought no account is shown as a notice on the form. */
internal sealed interface Notice {
    data object Declined : Notice

    data object TimedOut : Notice

    data object Cancelled : Notice

    data object OrganisationBlocks : Notice

    data object NoBrowser : Notice

    class Failed(val reason: String) : Notice

    /** The notice in plain words; what a person can do next is in it. */
    fun text(texts: Texts): String = when (this) {
        Declined -> texts.t("Microsoft's page came back without signing you in. You can try again.")
        TimedOut -> texts.t("Nothing came back from Microsoft's sign-in page. It may have been closed, or left open too long. You can try again.")
        Cancelled -> texts.t("The sign-in was cancelled.")
        OrganisationBlocks -> texts.t("Your organisation's rules (Conditional Access) don't allow MailCraft to sign you in on this phone. This is your organisation's policy, not a fault in MailCraft or in your phone. Ask the people who run your organisation's mail whether sign-in from this kind of app can be allowed.")
        NoBrowser -> texts.t("MailCraft couldn't open a browser for Microsoft's page. Install or enable a browser, then try again.")
        is Failed -> texts.t("The sign-in didn't go through.") + reason.takeIf { it.isNotBlank() }?.let { "\n$it" }.orEmpty()
    }
}

/** What the Add Account screen is showing. */
internal sealed interface AddAccountState {
    /** The form: the address typed (it is only a hint for Microsoft's page), and why the last try ended. */
    data class Form(val email: String = "", val notice: Notice? = null) : AddAccountState

    /** Microsoft's page is open in the browser and the app waits for it. */
    data class Waiting(val email: String) : AddAccountState

    /** An administrator has to approve MailCraft first: what to do next. */
    data class Approval(val email: String, val link: String?, val copied: Boolean = false) : AddAccountState

    /** An account is in: the screen closes. */
    data class Done(val account: ULong) : AddAccountState
}

/** The screen after a sign-in ended with [result]; [link] is the page for an administrator. */
internal fun afterSignIn(email: String, result: SignInResult, link: String?): AddAccountState = when (result) {
    is SignInResult.SignedIn -> AddAccountState.Done(result.account)
    SignInResult.NeedsApproval -> AddAccountState.Approval(email, link)
    SignInResult.Declined -> AddAccountState.Form(email, Notice.Declined)
    SignInResult.TimedOut -> AddAccountState.Form(email, Notice.TimedOut)
    SignInResult.Cancelled -> AddAccountState.Form(email, Notice.Cancelled)
    SignInResult.OrganisationBlocks -> AddAccountState.Form(email, Notice.OrganisationBlocks)
    SignInResult.NoBrowser -> AddAccountState.Form(email, Notice.NoBrowser)
    is SignInResult.Failed -> AddAccountState.Form(email, Notice.Failed(result.reason))
}

/** What the owner is told when asking for a permission ended with [result]; null for a yes or a call off. */
internal fun grantNotice(texts: Texts, result: GrantResult): String? = when (result) {
    GrantResult.Granted, GrantResult.Cancelled -> null
    GrantResult.NeedsApproval -> texts.t("Your organisation needs an administrator to approve this first. Ask your administrator, then try again.")
    GrantResult.OrganisationBlocks -> texts.t("Your organisation's rules don't allow this for your account. This is your organisation's policy, not a fault in MailCraft.")
    GrantResult.NotForThisAccount -> texts.t("Microsoft doesn't offer this for this kind of account.")
    GrantResult.Declined -> texts.t("Microsoft's page came back without giving it. You can try again.")
    GrantResult.TimedOut -> texts.t("Nothing came back from Microsoft's page. You can try again.")
    GrantResult.NoBrowser -> texts.t("MailCraft couldn't open a browser for Microsoft's page.")
    is GrantResult.Failed -> texts.t("That didn't go through.") + result.reason.takeIf { it.isNotBlank() }?.let { "\n$it" }.orEmpty()
}

/** One line for the Accounts page: where a permission stands. */
internal fun statusText(texts: Texts, status: PermissionStatus): String = when (status) {
    PermissionStatus.Checking -> texts.t("Checking…")
    PermissionStatus.Allowed -> texts.t("Allowed")
    PermissionStatus.NotAllowed -> texts.t("Not allowed yet")
    PermissionStatus.SignInNeeded -> texts.t("Sign in again first")
    PermissionStatus.Unknown -> texts.t("Couldn't check")
}

/**
 * The message the owner passes on to an administrator, around the address of Microsoft's page. It says
 * what the program is, what is asked, and what the administrator will see; nothing of the owner is in it.
 */
internal fun adminMessage(texts: Texts, link: String): String =
    texts.t(
        "Hello,\n\nI would like to use MailCraft with my work account. MailCraft is an open-source mail and calendar program that runs on my own phone. Our organisation lets only an administrator approve a new program, so it needs your approval once, for the whole organisation. Microsoft's page for that is here:\n\n{link}\n\nAfter you approve, your browser may show a page that can't be reached. That is expected: it is an address on your own computer, where nothing is listening, and the approval has been given by then.\n\nThank you",
    ).replace("{link}", link)

/** One permission question waiting for the owner's answer. */
internal data class PermissionAsk(val account: ULong, val permission: PermissionKind)

/** Where a permission question has got to. */
internal sealed interface QuestionState {
    val ask: PermissionAsk

    /** The question is on screen. */
    data class Asking(override val ask: PermissionAsk) : QuestionState

    /** The owner said yes and Microsoft's page is open in the browser. */
    data class Waiting(override val ask: PermissionAsk) : QuestionState

    /** It didn't end in a yes: [text] says why and what to do. */
    data class Told(override val ask: PermissionAsk, val text: String) : QuestionState
}

/** Which permission of which account a status is for. */
internal data class StatusKey(val account: ULong, val permission: PermissionKind)

/** Everything the account screens draw that isn't the list of accounts. */
internal data class AccountsUi(
    val add: AddAccountState = AddAccountState.Form(),
    val statuses: Map<StatusKey, PermissionStatus> = emptyMap(),
    val question: QuestionState? = null,
    /** The account the owner is asked to confirm removing. */
    val removing: AccountItem? = null,
    /** The last try at removing it failed: the account is still there. */
    val removeFailed: Boolean = false,
)

/** What the account screens ask of the app: one entry for each thing the owner can do there. */
@Suppress("LongParameterList")
internal class AccountActions(
    val onEmail: (String) -> Unit = {},
    val onSignIn: () -> Unit = {},
    val onCancelSignIn: () -> Unit = {},
    val onCopyMessage: (String) -> Unit = {},
    /** Leaves Add Account: the form is as new the next time. */
    val onLeaveAdd: () -> Unit = {},
    /** A sign-in again for an account the owner has; the address is the form's hint. */
    val onSignInAgain: (AccountItem) -> Unit = {},
    val onAsk: (AccountItem, PermissionKind) -> Unit = { _, _ -> },
    val onAllow: () -> Unit = {},
    val onNotNow: () -> Unit = {},
    val onCancelGrant: () -> Unit = {},
    val onRemoveAsk: (AccountItem) -> Unit = {},
    val onRemove: () -> Unit = {},
    val onRemoveCancel: () -> Unit = {},
    /** The Accounts page is on screen with these accounts: where each permission stands is asked. */
    val onShown: (List<AccountItem>) -> Unit = {},
)
