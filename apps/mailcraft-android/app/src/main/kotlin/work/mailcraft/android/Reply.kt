// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalLayoutApi::class)

package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Icon
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.ComposeStart

/** A way of answering a message, or continuing a draft, which the composer takes. */
internal enum class Reply { Reply, ReplyAll, Forward, Edit }

private val ReplyGap = 8.dp
private val ReplyPad = PaddingValues(horizontal = 12.dp)
private val ReplyIconSize = 18.dp
private val ReplyIconGap = 6.dp
private val ReplyTouch = 48.dp

/** What a button of the bar starts writing: an answer to message [id], or a draft continued. */
internal fun startFor(reply: Reply, id: ULong): ComposeStart = when (reply) {
    Reply.Reply -> ComposeStart.Reply(id)
    Reply.ReplyAll -> ComposeStart.ReplyAll(id)
    Reply.Forward -> ComposeStart.Forward(id)
    Reply.Edit -> ComposeStart.Draft(id)
}

/** One slim row of icon and short label, a 48 dp target each; large text wraps the row rather than clip a label. */
@Composable
internal fun ReplyBar(texts: Texts, draft: Boolean, onReply: (Reply) -> Unit) {
    Surface(tonalElevation = 2.dp) {
        FlowRow(
            modifier = Modifier.fillMaxWidth().padding(horizontal = ReplyGap),
            horizontalArrangement = Arrangement.SpaceEvenly,
        ) {
            (if (draft) listOf(Reply.Edit) else Reply.entries.filter { it != Reply.Edit }).forEach { reply ->
                TextButton(
                    onClick = { onReply(reply) },
                    modifier = Modifier.heightIn(min = ReplyTouch),
                    contentPadding = ReplyPad,
                ) {
                    Icon(replyIcon(reply), contentDescription = null, modifier = Modifier.size(ReplyIconSize))
                    Spacer(Modifier.width(ReplyIconGap))
                    Text(replyLabel(texts, reply))
                }
            }
        }
    }
}

private fun replyIcon(reply: Reply) = when (reply) {
    Reply.Reply, Reply.ReplyAll -> ListIcons.Reply
    Reply.Forward -> AppIcons.Next
    Reply.Edit -> ListIcons.Drafts
}

internal fun replyLabel(texts: Texts, reply: Reply): String = when (reply) {
    Reply.Reply -> texts.t("Reply")
    Reply.ReplyAll -> texts.t("Reply all")
    Reply.Forward -> texts.t("Forward")
    Reply.Edit -> texts.t("Edit draft")
}
