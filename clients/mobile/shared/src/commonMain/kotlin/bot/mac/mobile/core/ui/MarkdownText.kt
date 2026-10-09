package bot.mac.mobile.core.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.ClickableText
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.platform.openExternalUrl
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.markdown_copy
import kotlinx.coroutines.launch
import org.jetbrains.compose.resources.stringResource

/** Renders the small Markdown subset used by protocol message fallback text. */
@Composable
fun MarkdownText(markdown: String, modifier: Modifier = Modifier) {
    val blocks = remember(markdown) { parseMarkdown(markdown) }
    val scope = rememberCoroutineScope()
    val linkColor = MaterialTheme.colorScheme.primary
    val codeBackground = MaterialTheme.colorScheme.surfaceVariant

    Column(
        modifier = modifier,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        blocks.forEach { block ->
            when (block) {
                is MarkdownBlock.Paragraph -> MarkdownInlineText(
                    text = block.text,
                    style = MaterialTheme.typography.bodyLarge,
                    linkColor = linkColor,
                    codeBackground = codeBackground,
                    onLink = { url -> scope.launch { openExternalUrl(url) } },
                )

                is MarkdownBlock.Heading -> MarkdownInlineText(
                    text = block.text,
                    style = headingStyle(block.level),
                    linkColor = linkColor,
                    codeBackground = codeBackground,
                    onLink = { url -> scope.launch { openExternalUrl(url) } },
                )

                is MarkdownBlock.Code -> MarkdownCodeBlock(
                    code = block.text,
                    language = block.language,
                    background = codeBackground,
                )
            }
        }
    }
}

@Composable
private fun MarkdownInlineText(
    text: String,
    style: TextStyle,
    linkColor: Color,
    codeBackground: Color,
    onLink: (String) -> Unit,
) {
    val annotated = remember(text, style, linkColor, codeBackground) {
        inlineMarkdown(text, linkColor, codeBackground)
    }
    ClickableText(
        text = annotated,
        style = style.copy(color = if(style.color == Color.Unspecified) LocalContentColor.current else style.color),
        onClick = { offset ->
            annotated.getStringAnnotations(URL_TAG, offset, offset)
                .firstOrNull()
                ?.let { onLink(it.item) }
        },
    )
}

@Composable
private fun MarkdownCodeBlock(code: String, language: String, background: Color) {
    val clipboard = LocalClipboardManager.current
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(background, MaterialTheme.shapes.medium)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(modifier = Modifier.fillMaxWidth()) {
            if (language.isNotBlank()) {
                Text(
                    text = language,
                    modifier = Modifier.weight(1f),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                androidx.compose.foundation.layout.Spacer(Modifier.weight(1f))
            }
            TextButton(onClick = { clipboard.setText(AnnotatedString(code)) }) {
                Text(stringResource(Res.string.markdown_copy))
            }
        }
        SelectionContainer {
            Text(
                text = code,
                style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

private const val URL_TAG = "markdown-url"

private sealed interface MarkdownBlock {
    data class Paragraph(val text: String) : MarkdownBlock
    data class Heading(val level: Int, val text: String) : MarkdownBlock
    data class Code(val language: String, val text: String) : MarkdownBlock
}

@Composable
private fun headingStyle(level: Int): TextStyle = when (level) {
    1 -> MaterialTheme.typography.headlineSmall.copy(fontWeight = FontWeight.Bold)
    2 -> MaterialTheme.typography.titleLarge.copy(fontWeight = FontWeight.Bold)
    3 -> MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.Bold)
    else -> MaterialTheme.typography.titleSmall.copy(fontWeight = FontWeight.Bold)
}

private fun parseMarkdown(source: String): List<MarkdownBlock> {
    val blocks = mutableListOf<MarkdownBlock>()
    val paragraph = mutableListOf<String>()
    var code: StringBuilder? = null
    var language = ""

    fun flushParagraph() {
        if (paragraph.isNotEmpty()) {
            blocks += MarkdownBlock.Paragraph(paragraph.joinToString("\n"))
            paragraph.clear()
        }
    }

    source.replace("\r\n", "\n").replace('\r', '\n').split('\n').forEach { line ->
        val trimmed = line.trimStart()
        if (code != null) {
            if (trimmed.startsWith("```")) {
                blocks += MarkdownBlock.Code(language, code.toString().trimEnd('\n'))
                code = null
                language = ""
            } else {
                code!!.append(line).append('\n')
            }
        } else if (trimmed.startsWith("```")) {
            flushParagraph()
            language = trimmed.removePrefix("```").trim()
            code = StringBuilder()
        } else if (line.isBlank()) {
            flushParagraph()
        } else {
            val heading = HEADING.matchEntire(trimmed)
            if (heading != null) {
                flushParagraph()
                blocks += MarkdownBlock.Heading(heading.groupValues[1].length, heading.groupValues[2])
            } else {
                paragraph += line
            }
        }
    }
    if (code != null) blocks += MarkdownBlock.Code(language, code.toString().trimEnd('\n'))
    flushParagraph()
    return blocks
}

private val HEADING = Regex("^(#{1,6})\\s+(.+?)\\s*#*\\s*$")

private fun inlineMarkdown(text: String, linkColor: Color, codeBackground: Color): AnnotatedString =
    buildAnnotatedString {
        val strong = SpanStyle(fontWeight = FontWeight.Bold)
        val italic = SpanStyle(fontStyle = androidx.compose.ui.text.font.FontStyle.Italic)
        val code = SpanStyle(fontFamily = FontFamily.Monospace, background = codeBackground)
        val link = SpanStyle(color = linkColor, textDecoration = TextDecoration.Underline)
        var index = 0

        while (index < text.length) {
            if (text[index] == '\\' && index + 1 < text.length) {
                append(text[index + 1])
                index += 2
                continue
            }

            if (text.startsWith("[", index)) {
                val labelEnd = text.indexOf(']', index + 1)
                val urlStart = labelEnd + 1
                val urlEnd = if (labelEnd >= 0 && urlStart < text.length && text[urlStart] == '(') {
                    text.indexOf(')', urlStart + 1)
                } else {
                    -1
                }
                if (labelEnd > index + 1 && urlEnd > urlStart + 1) {
                    val url = text.substring(urlStart + 1, urlEnd).substringBefore(' ').trim()
                    if (url.isNotBlank()) {
                        val start = length
                        withStyle(link) { append(text, index + 1, labelEnd) }
                        addStringAnnotation(URL_TAG, url, start, length)
                        index = urlEnd + 1
                        continue
                    }
                }
            }

            val marker = text[index]
            if (marker == '`') {
                val end = text.indexOf('`', index + 1)
                if (end > index + 1) {
                    withStyle(code) { append(text, index + 1, end) }
                    index = end + 1
                    continue
                }
            }

            val doubleMarker = if (text.startsWith("**", index)) "**" else if (text.startsWith("__", index)) "__" else null
            if (doubleMarker != null) {
                val end = text.indexOf(doubleMarker, index + 2)
                if (end > index + 2) {
                    withStyle(strong) { append(text, index + 2, end) }
                    index = end + 2
                    continue
                }
            }

            if (marker == '*' || marker == '_') {
                val end = text.indexOf(marker, index + 1)
                if (end > index + 1 && !text[index + 1].isWhitespace()) {
                    withStyle(italic) { append(text, index + 1, end) }
                    index = end + 1
                    continue
                }
            }

            append(marker)
            index++
        }
    }
