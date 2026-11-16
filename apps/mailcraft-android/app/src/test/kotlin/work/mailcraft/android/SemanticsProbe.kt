// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(InternalComposeUiApi::class)

package work.mailcraft.android

import androidx.compose.runtime.Composable
import androidx.compose.ui.InternalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.node.RootForTest
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull

/** What a screen reader is given for one node of the merged semantics tree. */
internal class Said(
    val heading: Boolean,
    val text: String,
    val description: String?,
    val tableRows: Int?,
    val tableColumns: Int?,
    val cellRow: Int?,
    val cellColumn: Int?,
    val actions: List<String>,
)

/**
 * The semantics tree a screen reader is given, read from what was really drawn (no parallel model of
 * it). Wrap the content in [Watch] inside a Paparazzi snapshot; [said] holds the nodes, in reading
 * order, as they were once everything was laid out (the tree is gone when the snapshot returns).
 */
internal class SemanticsProbe {
    var said: List<Said> = emptyList()
        private set

    @Composable
    fun Watch(content: @Composable () -> Unit) {
        val view = LocalView.current
        androidx.compose.foundation.layout.Box(
            modifier = Modifier.onGloballyPositioned {
                val root = (view as? RootForTest)?.semanticsOwner?.rootSemanticsNode
                if (root != null) said = flatten(root).map(::read)
            },
        ) {
            content()
        }
    }

    private fun flatten(root: SemanticsNode): List<SemanticsNode> {
        val out = ArrayList<SemanticsNode>()
        fun walk(n: SemanticsNode) {
            out += n
            n.children.forEach(::walk)
        }
        walk(root)
        return out
    }

    private fun read(n: SemanticsNode): Said {
        val config = n.config
        val table = config.getOrNull(SemanticsProperties.CollectionInfo)
        val cell = config.getOrNull(SemanticsProperties.CollectionItemInfo)
        return Said(
            heading = config.getOrNull(SemanticsProperties.Heading) != null,
            text = config.getOrNull(SemanticsProperties.Text)?.joinToString("") { it.text } ?: "",
            description = config.getOrNull(SemanticsProperties.ContentDescription)?.joinToString(" "),
            tableRows = table?.rowCount,
            tableColumns = table?.columnCount,
            cellRow = cell?.rowIndex,
            cellColumn = cell?.columnIndex,
            actions = config.getOrNull(SemanticsActions.CustomActions)?.map { it.label } ?: emptyList(),
        )
    }
}
