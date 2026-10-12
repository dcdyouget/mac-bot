package bot.mac.mobile.core.ui

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonArray
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class FilePathTest {
    @Test
    fun previewScopeChangesAcrossHostsProjectsAndInitialFiles() {
        val initial = filePreviewScopeKey("host-a", "project-a", "product/readme.md")

        assertEquals(initial, filePreviewScopeKey("host-a", "project-a", "product/readme.md"))
        assertEquals(false, initial == filePreviewScopeKey("host-a", "project-b", "product/readme.md"))
        assertEquals(false, initial == filePreviewScopeKey("host-b", "project-a", "product/readme.md"))
        assertEquals(false, initial == filePreviewScopeKey("host-a", "project-a", "other/readme.md"))
    }

    @Test
    fun previewBackWalksFromFileThroughParentDirectoriesToCaller() {
        var path = "product/prototype/assets/theme.css"
        val expectedParents = listOf(
            "product/prototype/assets",
            "product/prototype",
            "product",
            "",
        )

        expectedParents.forEach { parent ->
            path = requireNotNull(parentFilePath(path))
            assertEquals(parent, path)
        }
        assertNull(parentFilePath(path))
        assertNull(parentFilePath("https://example.com/file"))
        assertNull(parentFilePath("product/../outside"))
    }

    @Test
    fun resolvesNestedRelativeAndRootRelativeEntries() {
        assertEquals(
            "product/prototype/assets/logo.png",
            resolveFileEntryPath("product/prototype", "assets/logo.png"),
        )
        assertEquals(
            "product/prototype/assets/logo.png",
            resolveFileEntryPath("product/prototype", "product/prototype/assets/logo.png"),
        )
        assertEquals("assets/logo.png", resolveFileEntryPath("", "assets/./logo.png"))
    }

    @Test
    fun rejectsPathsThatCanEscapeOrSwitchRoots() {
        assertNull(resolveFileEntryPath("product/prototype", "../secrets.txt"))
        assertNull(resolveFileEntryPath("product/prototype", "assets/../../secrets.txt"))
        assertNull(resolveFileEntryPath("product/prototype", "/etc/passwd"))
        assertNull(resolveFileEntryPath("product/../private", "file.txt"))
        assertNull(resolveFileEntryPath("product/prototype", "C:/private/file.txt"))
    }

    @Test
    fun parsesStringAndObjectEntriesWithNonEmptyUniqueKeys() {
        val entries = parseFilePreviewEntries(
            "product/prototype",
            Json.parseToJsonElement(
                """[
                    "assets/logo.png",
                    {"name":"assets","path":"assets","is_dir":true},
                    {"path":"product/prototype/readme.md","size":12},
                    {"name":"missing-path"},
                    {},
                    null,
                    {"name":"../escape"}
                ]""",
            ).jsonArray,
        )

        assertEquals(4, entries.size)
        assertEquals(
            listOf(
                "product/prototype/assets/logo.png",
                "product/prototype/assets",
                "product/prototype/readme.md",
                "product/prototype/missing-path",
            ),
            entries.map(FilePreviewEntry::path),
        )
        assertEquals(listOf("logo.png", "assets", "readme.md", "missing-path"), entries.map(FilePreviewEntry::name))
        assertEquals(listOf(false, true, false, false), entries.map(FilePreviewEntry::isDirectory))
        assertEquals(12L, entries[2].size)
        assertEquals(entries.size, entries.map(FilePreviewEntry::key).toSet().size)
        assertEquals(true, entries.all { it.key.isNotBlank() })
    }
}
