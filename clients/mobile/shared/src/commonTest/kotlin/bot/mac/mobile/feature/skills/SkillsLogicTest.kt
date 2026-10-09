package bot.mac.mobile.feature.skills

import bot.mac.mobile.core.network.ConnectionSnapshot
import bot.mac.mobile.core.network.HostProfile
import bot.mac.mobile.core.network.ScreenConnection
import bot.mac.mobile.core.network.ScreenFrame
import bot.mac.mobile.core.platform.PickedFile
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.state.MobileState
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.coroutines.test.runTest

class SkillsLogicTest {
    @Test
    fun onlyDraftSkillsCanBePublished() {
        assertTrue(skillCanPublish("draft"))
        assertFalse(skillCanPublish("user"))
        assertFalse(skillCanPublish("builtin"))
        assertFalse(skillCanPublish(null))
    }

    @Test
    fun saveRefreshReadsTheUpdatedDetailAfterList() = runTest {
        val repository = FakeSkillRepository()

        val detail = saveSkillAndRefresh(
            repository = repository,
            existingName = "demo",
            name = "ignored-for-update",
            description = "",
            content = "# v2",
        )

        assertEquals("# v2", detail.str("content"))
        assertEquals(listOf("skill.update", "skill.list", "skill.get"), repository.calls)
    }

    @Test
    fun publishRefreshReadsThePublishedSourceAfterList() = runTest {
        val repository = FakeSkillRepository()

        val detail = publishSkillAndRefresh(repository, "demo")

        assertEquals("user", detail.str("source"))
        assertEquals(listOf("skill.publish", "skill.list", "skill.get"), repository.calls)
    }

    private class FakeSkillRepository : MobileRepository {
        private val mutableState = MutableStateFlow(MobileState())
        private val mutableHost = MutableStateFlow<HostProfile?>(null)
        private val mutableStatuses = MutableStateFlow<Map<String, ConnectionSnapshot>>(emptyMap())
        private val mutableEvents = MutableSharedFlow<bot.mac.mobile.core.state.HostEvent>()
        private var skill = buildJsonObject {
            put("name", "demo")
            put("source", "draft")
            put("content", "# v1")
        }
        val calls = mutableListOf<String>()

        override val state = mutableState
        override val hostStatuses = mutableStatuses
        override val hostEvents = mutableEvents
        override val activeHost = mutableHost

        override suspend fun call(method: String, params: JsonObject): JsonObject {
            calls += method
            when (method) {
                "skill.update" -> skill = JsonObject(skill + ("content" to params["content"]!!))
                "skill.publish" -> skill = JsonObject(skill + ("source" to kotlinx.serialization.json.JsonPrimitive("user")))
                "skill.list" -> return buildJsonObject { put("skills", kotlinx.serialization.json.JsonArray(listOf(skill))) }
                "skill.get" -> return buildJsonObject { put("skill", skill) }
            }
            return buildJsonObject { }
        }

        override fun createScreen(botId: String, quality: String, tabId: String?, onFrame: suspend (ScreenFrame) -> Unit): ScreenConnection =
            error("screen is not used by skill tests")

        override suspend fun uploadFile(file: PickedFile): JsonObject = error("upload is not used by skill tests")
        override suspend fun fetchText(path: String, params: Map<String, String>): String = error("fetch is not used by skill tests")
        override suspend fun fetchBytes(path: String, params: Map<String, String>): ByteArray = error("fetch is not used by skill tests")
        override suspend fun refresh() = Unit
    }
}
