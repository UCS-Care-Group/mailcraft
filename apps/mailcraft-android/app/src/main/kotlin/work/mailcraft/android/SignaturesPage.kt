// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.FromChoice

private val PagePadding = 16.dp
private val FieldGap = 8.dp
private const val SIGNATURE_LINES = 3

/** One signature of an account, as the Signatures page reads and writes it. */
internal class AccountSignature(val account: FromChoice, val text: String)

/** The signatures of the accounts; the core keeps them and puts the right one into each new message. */
internal interface SignatureSource {
    suspend fun load(): List<AccountSignature>

    suspend fun save(account: ULong, text: String): Boolean
}

/** [SignatureSource] over the Rust core. */
internal class CoreSignatures(
    private val core: Core,
    private val dispatcher: CoroutineDispatcher = Dispatchers.IO,
) : SignatureSource {
    override suspend fun load(): List<AccountSignature> = withContext(dispatcher) {
        try {
            core.composeAccounts().filter { !it.shared }.map { AccountSignature(it, core.accountSignature(it.account)) }
        } catch (_: CoreException) {
            emptyList()
        }
    }

    override suspend fun save(account: ULong, text: String): Boolean = withContext(dispatcher) {
        try {
            core.setAccountSignature(account, text)
            true
        } catch (_: CoreException) {
            false
        }
    }
}

/** Settings > Signatures: a text for each account, used in new messages, replies and forwards. */
@Composable
internal fun SignaturesPage(texts: Texts, source: SignatureSource?) {
    val loaded by produceState<List<AccountSignature>?>(null, source) { value = source?.load().orEmpty() }
    val list = loaded
    if (list == null) Text(texts.t("Loading"), modifier = Modifier.padding(PagePadding)) else SignaturesList(texts, list, source)
}

/** The signatures once they are read. */
@Composable
internal fun SignaturesList(texts: Texts, list: List<AccountSignature>, source: SignatureSource?) {
    if (list.isEmpty()) {
        Text(
            texts.t("Add an account to write a signature for it."),
            modifier = Modifier.padding(PagePadding),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    } else {
        list.forEach { SignatureEditor(texts, it, source) }
    }
}

@Composable
private fun SignatureEditor(texts: Texts, item: AccountSignature, source: SignatureSource?) {
    var text by remember(item) { mutableStateOf(item.text) }
    var saved by remember(item) { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    Column(Modifier.fillMaxWidth().padding(PagePadding), verticalArrangement = Arrangement.spacedBy(FieldGap)) {
        Text(item.account.address, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
        OutlinedTextField(
            value = text,
            onValueChange = {
                text = it
                saved = false
            },
            modifier = Modifier.fillMaxWidth(),
            minLines = SIGNATURE_LINES,
            label = { Text(texts.t("Signature")) },
        )
        Button(onClick = { scope.launch { saved = source?.save(item.account.account, text) == true } }) {
            Text(if (saved) texts.t("Saved") else texts.t("Save"))
        }
    }
}
