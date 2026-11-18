// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Test

class FolderTreeTest {
    private fun f(id: ULong, parent: ULong?, name: String = "f$id") =
        FolderItem(id, parent, name, "other", 0u)

    private fun order(list: List<FolderItem>) = folderTree(list).map { it.id.toInt() to it.depth }

    @Test
    fun childrenFollowTheirParentAndOrderIsKept() {
        val tree = order(listOf(f(1u, null), f(2u, null), f(3u, 1u), f(4u, 3u), f(5u, 1u)))
        assertEquals(listOf(1 to 0, 3 to 1, 4 to 2, 5 to 1, 2 to 0), tree)
    }

    @Test
    fun aFolderWhoseParentIsMissingIsATopFolder() {
        assertEquals(listOf(7 to 0), order(listOf(f(7u, 99u))))
    }

    @Test
    fun aLoopOfParentsListsEachFolderOnce() {
        val tree = order(listOf(f(1u, 2u), f(2u, 1u), f(3u, 3u), f(4u, null)))
        assertEquals(listOf(1, 2, 3, 4), tree.map { it.first }.sorted())
        assertEquals(4, tree.size)
    }

    @Test
    fun repeatedIdsAreListedOnce() {
        assertEquals(1, order(listOf(f(1u, null), f(1u, null))).size)
    }

    @Test
    fun aVeryDeepChainIsNeitherAStackOverflowNorTooDeepOnScreen() {
        val chain = (1..20_000).map { f(it.toULong(), if (it == 1) null else (it - 1).toULong()) }
        val tree = folderTree(chain)
        assertEquals(20_000, tree.size)
        assertEquals(MAX_FOLDER_DEPTH, tree.last().depth)
    }

    @Test
    fun nothingIsNothing() {
        assertEquals(emptyList<FolderItem>(), folderTree(emptyList()))
    }
}
