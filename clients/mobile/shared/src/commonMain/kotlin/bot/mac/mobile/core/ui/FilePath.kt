package bot.mac.mobile.core.ui

import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.longOrNull

internal data class FilePreviewEntry(
    val key: String,
    val name: String,
    val path: String,
    val isDirectory: Boolean,
    val size: Long,
    val modifiedAt: String,
)

internal data class FilePreviewScopeKey(
    val hostId: String?,
    val rootId: String,
    val initialPath: String,
)

internal fun filePreviewScopeKey(hostId: String?, rootId: String, initialPath: String) =
    FilePreviewScopeKey(hostId, rootId, initialPath)

/** Returns the containing directory, or null when the preview should return to its caller. */
internal fun parentFilePath(path: String): String? {
    if (path.startsWith("http://") || path.startsWith("https://")) return null
    val normalized = normalizeRelativeFilePath(path) ?: return null
    if (normalized.isEmpty()) return null
    return normalized.substringBeforeLast('/', "")
}

/** Resolve a server-provided relative entry path beneath the currently opened directory. */
internal fun resolveFileEntryPath(parentPath: String, entryPath: String): String? {
    val child = normalizeRelativeFilePath(entryPath) ?: return null
    val parent = normalizeRelativeFilePath(parentPath) ?: return null
    if (child.isEmpty()) return null

    // Some compatible servers may already return root-relative paths.
    val combined = when {
        parent.isEmpty() || child == parent || child.startsWith("$parent/") -> child
        else -> listOf(parent, child).filter(String::isNotEmpty).joinToString("/")
    }
    return normalizeRelativeFilePath(combined)?.takeIf(String::isNotEmpty)
}

private fun normalizeRelativeFilePath(path: String): String? {
    if (path.startsWith('/') || path.startsWith('\\') || path.contains('\\')) return null
    if (path.length >= 2 && path[0].isLetter() && path[1] == ':') return null
    val segments = mutableListOf<String>()
    for (segment in path.split('/')) {
        when (segment) {
            "", "." -> Unit
            ".." -> return null
            else -> segments += segment
        }
    }
    return segments.joinToString("/")
}

/** Accept the current string response and the older object response without trusting malformed fields. */
internal fun parseFilePreviewEntries(parentPath: String, array: JsonArray): List<FilePreviewEntry> {
    val result = mutableListOf<FilePreviewEntry>()
    val keyCounts = mutableMapOf<String, Int>()
    array.forEachIndexed { index, element ->
        val fields = element as? JsonObject
        val rawPath = when {
            element is JsonPrimitive && element.isString -> element.contentOrNull
            fields != null -> fields.str("path").ifBlank { fields.str("name") }
            else -> null
        } ?: return@forEachIndexed
        val resolvedPath = resolveFileEntryPath(parentPath, rawPath) ?: return@forEachIndexed
        val name = fields?.str("name")?.takeIf(String::isNotBlank)
            ?: resolvedPath.substringAfterLast('/')
        val baseKey = resolvedPath.ifBlank { name }.ifBlank { "entry-$index" }
        val duplicateIndex = keyCounts.getOrDefault(baseKey, 0)
        keyCounts[baseKey] = duplicateIndex + 1
        val key = if (duplicateIndex == 0) baseKey else "$baseKey#$duplicateIndex"
        result += FilePreviewEntry(
            key = key,
            name = name,
            path = resolvedPath,
            isDirectory = fields?.get("is_dir")?.let { (it as? JsonPrimitive)?.booleanOrNull } ?: false,
            size = fields?.get("size")?.let { (it as? JsonPrimitive)?.longOrNull } ?: 0L,
            modifiedAt = fields?.str("modified_at").orEmpty(),
        )
    }
    return result
}
