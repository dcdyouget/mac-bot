package bot.mac.mobile.feature.group

enum class AnnouncementMarkdownAction { HEADING, BOLD, BULLET, NUMBERED, CODE, LINK }

data class AnnouncementMarkdownEdit(
    val text: String,
    val selectionStart: Int,
    val selectionEnd: Int,
)

/** Apply a Markdown toolbar action while preserving selected content and cursor intent. */
fun applyAnnouncementMarkdownAction(
    text: String,
    selectionStart: Int,
    selectionEnd: Int,
    action: AnnouncementMarkdownAction,
): AnnouncementMarkdownEdit {
    val start = minOf(selectionStart, selectionEnd).coerceIn(0, text.length)
    val end = maxOf(selectionStart, selectionEnd).coerceIn(start, text.length)
    val selected = text.substring(start, end)

    val wrapper = when (action) {
        AnnouncementMarkdownAction.BOLD -> "**" to "**"
        AnnouncementMarkdownAction.CODE -> "`" to "`"
        AnnouncementMarkdownAction.LINK -> "[" to "](url)"
        else -> null
    }
    if (wrapper != null) {
        val result = text.substring(0, start) + wrapper.first + selected + wrapper.second + text.substring(end)
        val contentStart = start + wrapper.first.length
        val cursorStart: Int
        val cursorEnd: Int
        if (action == AnnouncementMarkdownAction.LINK) {
            cursorStart = contentStart + selected.length + 2
            cursorEnd = cursorStart + 3
        } else if (selected.isNotEmpty()) {
            cursorStart = contentStart
            cursorEnd = contentStart + selected.length
        } else {
            cursorStart = contentStart
            cursorEnd = cursorStart
        }
        return AnnouncementMarkdownEdit(result, cursorStart, cursorEnd)
    }

    val prefix = when (action) {
        AnnouncementMarkdownAction.HEADING -> "# "
        AnnouncementMarkdownAction.BULLET -> "- "
        AnnouncementMarkdownAction.NUMBERED -> "1. "
        else -> error("Unsupported announcement toolbar action: $action")
    }
    val firstLineStart = text.lastIndexOf('\n', (start - 1).coerceAtLeast(0)).let { if (start == 0) 0 else it + 1 }
    val selectionEndForLines = if (end > start && end < text.length && text[end] == '\n') end - 1 else end
    val lastLineStart = text.lastIndexOf('\n', (selectionEndForLines - 1).coerceAtLeast(0)).let {
        if (selectionEndForLines == 0) 0 else it + 1
    }
    val lineStarts = buildList {
        add(firstLineStart)
        var lineStart = firstLineStart
        while (lineStart < lastLineStart) {
            val newline = text.indexOf('\n', lineStart)
            if (newline < 0 || newline + 1 > lastLineStart) break
            lineStart = newline + 1
            add(lineStart)
        }
    }
    val result = buildString(text.length + lineStarts.size * prefix.length) {
        var sourceIndex = 0
        lineStarts.forEach { insertionIndex ->
            append(text, sourceIndex, insertionIndex)
            append(prefix)
            sourceIndex = insertionIndex
        }
        append(text, sourceIndex, text.length)
    }
    val startShift = lineStarts.count { it <= start } * prefix.length
    val endShift = if (start == end) startShift else lineStarts.count { it < end } * prefix.length
    return AnnouncementMarkdownEdit(result, start + startShift, end + endShift)
}
