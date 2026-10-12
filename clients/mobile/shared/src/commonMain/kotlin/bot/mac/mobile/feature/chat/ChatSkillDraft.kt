package bot.mac.mobile.feature.chat

import kotlinx.serialization.json.JsonObject

private const val PERSISTED_SKILL_DRAFT_MARKER = "macbot-chat-draft-v1\n"

internal data class ChatSkillDraft(
    val skillName: String?,
    val text: String,
)

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
