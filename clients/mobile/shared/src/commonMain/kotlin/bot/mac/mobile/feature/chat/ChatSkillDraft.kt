package bot.mac.mobile.feature.chat

import kotlinx.serialization.json.JsonObject
import bot.mac.mobile.core.protocol.Message
import bot.mac.mobile.core.protocol.str
import kotlin.time.Instant

private const val PERSISTED_SKILL_DRAFT_MARKER = "macbot-chat-draft-v1\n"

internal data class ChatSkillDraft(
    val skillName: String?,
    val text: String,
)

internal data class ChatMessageSkillPrefix(val skillName: String, val body: String)

/** Parses the persisted `/<skill>` prefix from a sent message without treating prose slashes as skills. */
internal fun parseChatMessageSkillPrefix(value: String): ChatMessageSkillPrefix? {
    if (!value.startsWith('/')) return null
    val end = value.indexOfFirst(Char::isWhitespace).let { if (it < 0) value.length else it }
    if (end <= 1) return null
    val name = value.substring(1, end)
    if (name.any { !(it in 'a'..'z' || it in '0'..'9' || it == '-') }) return null
    return ChatMessageSkillPrefix(name, value.substring(end).trimStart())
}

/** Unknown or disabled skills stay ordinary message text; a historical successful load is authoritative. */
internal fun resolveChatMessageSkillPrefix(
    value: String,
    enabledSkillNames: Set<String>,
    loadedNames: Set<String>,
): ChatMessageSkillPrefix? = parseChatMessageSkillPrefix(value)?.takeIf {
    it.skillName in enabledSkillNames || it.skillName in loadedNames
}

/** Returns unique valid names; a missing/old-server field and malformed entries are safely ignored. */
internal fun loadedSkillNames(message: JsonObject): List<String> {
    return Message(message).loadedSkills.map { it.name }.distinct()
}

/** Stores a selected skill separately from editable prose while preserving the old draft format. */
internal fun serializeChatSkillDraft(skillName: String?, text: String): String =
    skillName?.takeIf { it.isNotBlank() }?.let { "$PERSISTED_SKILL_DRAFT_MARKER$it\n$text" } ?: text

/** Restores both new selected-skill drafts and older drafts that already contain a skill prefix. */
internal fun parseChatSkillDraft(value: String): ChatSkillDraft {
    if (value.startsWith(PERSISTED_SKILL_DRAFT_MARKER)) {
        val payload = value.removePrefix(PERSISTED_SKILL_DRAFT_MARKER)
        val separator = payload.indexOf('\n')
        if (separator > 0) {
            val name = payload.substring(0, separator)
            if (name.isNotBlank()) return ChatSkillDraft(name, payload.substring(separator + 1))
        }
        return ChatSkillDraft(null, value)
    }

    if (value.startsWith('/')) {
        val separator = value.indexOf(' ')
        if (separator > 1) {
            val name = value.substring(1, separator)
            if (name.isNotBlank()) return ChatSkillDraft(name, value.substring(separator + 1))
        }
    }
    return ChatSkillDraft(null, value)
}

/** The server's existing forced-skill contract reads `/<skill>` from the start of message text. */
internal fun serializeChatMessageText(skillName: String?, text: String): String {
    val body = text.trim()
    val skill = skillName?.takeIf { it.isNotBlank() } ?: return body
    return if (body.isEmpty()) "/$skill" else "/$skill $body"
}

/** A slash picker is open only while the whole input is an unfinished skill token. */
internal fun isSlashSkillQuery(text: String, selectedSkill: String?): Boolean =
    selectedSkill == null && text.startsWith("/") && !text.substringAfter('/', "").contains(' ')

/** Retry loading options when a picker stayed open while the Host was reconnecting. */
internal fun shouldLoadSlashSkills(text: String, selectedSkill: String?, connected: Boolean): Boolean =
    connected && isSlashSkillQuery(text, selectedSkill)

internal fun filterSlashSkills(skills: List<JsonObject>, query: String): List<JsonObject> =
    skills.filter { skill ->
        skill["enabled"]?.toString() != "false" &&
            skill["name"]?.toString()?.trim('"')?.contains(query, ignoreCase = true) == true
    }

internal fun chatMessageDeepLink(hostId: String, chatId: String, messageId: String): String =
    "macbot://chat/${encodeUriComponent(chatId)}?host_id=${encodeUriComponent(hostId)}&message_id=${encodeUriComponent(messageId)}"

internal fun shouldSendChatShortcut(isEnter: Boolean, isKeyDown: Boolean, ctrlPressed: Boolean, metaPressed: Boolean): Boolean =
    isEnter && isKeyDown && (ctrlPressed || metaPressed)

private fun encodeUriComponent(value: String): String = buildString {
    value.encodeToByteArray().forEach { byte ->
        val unsigned = byte.toInt() and 0xff
        val char = unsigned.toChar()
        if (char in 'a'..'z' || char in 'A'..'Z' || char in '0'..'9' || char in "-_.~") append(char)
        else {
            append('%')
            append(HEX[unsigned ushr 4])
            append(HEX[unsigned and 0x0f])
        }
    }
}

private const val HEX = "0123456789ABCDEF"
/** Recent skills are based only on the server's successful invocation timestamp. */
internal fun recentSlashSkills(
    skills: List<JsonObject>,
    now: Instant,
    limit: Int = 3,
): List<JsonObject> = skills.asSequence()
    .filter { it["enabled"]?.toString() != "false" }
    .mapNotNull { skill ->
        val invokedAt = runCatching { Instant.parse(skill.str("last_invoked_at")) }.getOrNull() ?: return@mapNotNull null
        val age = now.toEpochMilliseconds() - invokedAt.toEpochMilliseconds()
        if (age !in 0..RECENT_SKILL_WINDOW_MS) null else invokedAt.toEpochMilliseconds() to skill
    }
    .sortedByDescending { it.first }
    .take(limit.coerceAtLeast(0))
    .map { it.second }
    .toList()

private const val RECENT_SKILL_WINDOW_MS = 7L * 24 * 60 * 60 * 1000
