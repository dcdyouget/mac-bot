package bot.mac.mobile.feature.chat

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import bot.mac.mobile.core.protocol.str

class ChatSkillDraftTest {
    @Test
    fun selectedSkillIsStoredSeparatelyAndRestored() {
        val stored = serializeChatSkillDraft("web-auth-patterns", "@编码 补上找回密码")

        assertEquals(ChatSkillDraft("web-auth-patterns", "@编码 补上找回密码"), parseChatSkillDraft(stored))
        assertEquals("/web-auth-patterns @编码 补上找回密码", serializeChatMessageText("web-auth-patterns", "@编码 补上找回密码"))
    }

    @Test
    fun legacySkillPrefixRestoresAsChipAndBody() {
        assertEquals(ChatSkillDraft("web-auth-patterns", "补上找回密码"), parseChatSkillDraft("/web-auth-patterns 补上找回密码"))
        assertEquals(ChatSkillDraft(null, "ordinary message"), parseChatSkillDraft("ordinary message"))
        assertNull(parseChatSkillDraft("/auth").skillName)
    }

    @Test
    fun skillWithoutBodyStillSerializesToExistingSkillPrefix() {
        assertEquals("/web-auth-patterns", serializeChatMessageText("web-auth-patterns", "  "))
        assertEquals("plain text", serializeChatMessageText(null, " plain text "))
    }

    @Test
    fun slashSkillLoadWaitsForConnectionAndStopsAfterSelectionOrInstructionBegins() {
        assertFalse(shouldLoadSlashSkills("/web", null, connected = false))
        assertTrue(shouldLoadSlashSkills("/web", null, connected = true))
        assertFalse(shouldLoadSlashSkills("/web-auth-patterns ", null, connected = true))
        assertFalse(shouldLoadSlashSkills("/web", "web-auth-patterns", connected = true))
    }

    @Test
    fun slashSkillOptionsMatchQueryAndHideDisabledSkills() {
        val skills = Json.parseToJsonElement(
            """[{"name":"web-auth-patterns"},{"name":"web-disabled","enabled":false},{"name":"prd-template"}]""",
        ).jsonArray.map { it.jsonObject }

        assertEquals(listOf("web-auth-patterns"), filterSlashSkills(skills, "WEB").map { it.str("name") })
    }
}
