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
import kotlinx.serialization.json.JsonObject
import kotlin.time.Instant

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

    @Test
    fun slashRecentSkillsUseOnlySuccessfulInvocationsFromTheLastSevenDays() {
        val skills = Json.parseToJsonElement(
            """[
                {"name":"old","last_invoked_at":"2026-10-04T11:00:00Z"},
                {"name":"edit-recent","updated_at":"2026-10-12T11:00:00Z"},
                {"name":"called-first","last_invoked_at":"2026-10-11T11:00:00Z"},
                {"name":"called-later","last_invoked_at":"2026-10-12T10:00:00Z"},
                {"name":"disabled","enabled":false,"last_invoked_at":"2026-10-12T11:00:00Z"}
            ]""",
        ).jsonArray.map { it.jsonObject }

        assertEquals(
            listOf("called-later", "called-first"),
            recentSlashSkills(skills, Instant.parse("2026-10-12T11:00:00Z")).map { it.str("name") },
        )
    }

    @Test
    fun sentSkillPrefixBecomesChipBodyAndOnlyValidPrefixIsRecognized() {
        assertEquals(ChatMessageSkillPrefix("web-auth-patterns", "@编码 补上找回密码"), parseChatMessageSkillPrefix("/web-auth-patterns @编码 补上找回密码"))
        assertEquals(ChatMessageSkillPrefix("web-auth-patterns", ""), parseChatMessageSkillPrefix("/web-auth-patterns"))
        assertNull(parseChatMessageSkillPrefix("plain /web-auth-patterns"))
        assertNull(parseChatMessageSkillPrefix("/UpperCase do work"))
    }

    @Test
    fun sentPrefixIsOnlyAChipForEnabledOrHistoricallyLoadedSkills() {
        val enabled = setOf("web-auth-patterns")
        assertEquals(
            ChatMessageSkillPrefix("web-auth-patterns", "do work"),
            resolveChatMessageSkillPrefix("/web-auth-patterns do work", enabled, emptySet()),
        )
        assertNull(resolveChatMessageSkillPrefix("/unknown do work", enabled, emptySet()))
        assertNull(resolveChatMessageSkillPrefix("/disabled do work", enabled, emptySet()))
        assertEquals(
            ChatMessageSkillPrefix("deleted-skill", "do work"),
            resolveChatMessageSkillPrefix("/deleted-skill do work", enabled, setOf("deleted-skill")),
        )
        assertNull(resolveChatMessageSkillPrefix("/tmp/report.md", enabled, emptySet()))
        assertNull(resolveChatMessageSkillPrefix("please use /web-auth-patterns", enabled, emptySet()))
    }

    @Test
    fun copiedMessageLinkEncodesEachIdentifierAsAUriComponent() {
        assertEquals(
            "macbot://chat/chat%2Fid?host_id=host%20id&message_id=msg%3F1",
            chatMessageDeepLink("host id", "chat/id", "msg?1"),
        )
    }

    @Test
    fun onlyCtrlOrMetaEnterOnKeyDownSendsTheComposer() {
        assertTrue(shouldSendChatShortcut(isEnter = true, isKeyDown = true, ctrlPressed = true, metaPressed = false))
        assertTrue(shouldSendChatShortcut(isEnter = true, isKeyDown = true, ctrlPressed = false, metaPressed = true))
        assertFalse(shouldSendChatShortcut(isEnter = true, isKeyDown = true, ctrlPressed = false, metaPressed = false))
        assertFalse(shouldSendChatShortcut(isEnter = true, isKeyDown = false, ctrlPressed = true, metaPressed = false))
        assertFalse(shouldSendChatShortcut(isEnter = false, isKeyDown = true, ctrlPressed = true, metaPressed = false))
    }

    @Test
    fun loadedSkillsAreOptionalTolerantAndGroupedByNameAcrossBots() {
        val message = Json.parseToJsonElement(
            """{"loaded_skills":[
                {"name":"web-auth-patterns","bot_id":"bot-a","assignment_id":"task-a","loaded_at":"2026-10-12T09:00:00Z","future_field":true},
                {"name":"web-auth-patterns","bot_id":"bot-b","assignment_id":null,"loaded_at":"2026-10-12T09:01:00Z"},
                {"name":"pdf-tools","bot_id":"bot-b","assignment_id":"task-b","loaded_at":"2026-10-12T09:02:00Z"},
                {"name":"invalid","loaded_at":"2026-10-12T09:03:00Z"},
                null
            ]}""",
        ).jsonObject

        assertEquals(listOf("web-auth-patterns", "pdf-tools"), loadedSkillNames(message))
        assertEquals(emptyList(), loadedSkillNames(JsonObject(emptyMap())))
        assertEquals(emptyList(), loadedSkillNames(Json.parseToJsonElement("""{"loaded_skills":"unsupported"}""").jsonObject))
    }
}
