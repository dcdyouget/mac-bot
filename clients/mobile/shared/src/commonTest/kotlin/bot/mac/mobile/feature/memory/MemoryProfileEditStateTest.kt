package bot.mac.mobile.feature.memory

import bot.mac.mobile.core.protocol.protocolJson
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.network.ConnectionSnapshot
import bot.mac.mobile.core.network.HostProfile
import bot.mac.mobile.core.network.NetworkError
import bot.mac.mobile.core.network.ScreenConnection
import bot.mac.mobile.core.network.ScreenFrame
import bot.mac.mobile.core.platform.PickedFile
import bot.mac.mobile.core.state.HostEvent
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.state.MobileState
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

class MemoryProfileEditStateTest {
    private fun profile(version: Int, name: String, language: String = "en-US", identity: String = "") =
        protocolJson.parseToJsonElement("""{"version":$version,"name":"$name","language":"$language","identity":"$identity"}""").jsonObject

    @Test
    fun cleanEditorAdoptsLiveProfileUpdate() {
        val state = MemoryProfileEditState(profile(1, "Sam"))

        val next = state.receiveRemote(profile(2, "Samantha"))

        assertEquals("Samantha", next.drafts["name"])
        assertEquals(2, next.expectedVersion)
        assertEquals(null, next.conflict)
    }

    @Test
    fun incomingUpdatePreservesDirtyDraftAndShowsConflict() {
        val state = MemoryProfileEditState(profile(1, "Sam")).edit("name", "My draft")

        val next = state.receiveRemote(profile(2, "Server value"))

        assertEquals("My draft", next.drafts["name"])
        assertEquals("Server value", next.conflict?.str("name"))
        assertEquals(1, next.expectedVersion)
    }

    @Test
    fun saveResponseAdvancesVersionWithoutReplacingOtherDrafts() {
        val state = MemoryProfileEditState(profile(3, "Sam")).edit("name", " Samantha ").edit("identity", "local identity")

        val next = state.saveSucceeded(profile(4, "Samantha"), "name", " Samantha ")

        assertEquals(4, next.expectedVersion)
        assertEquals("Samantha", next.drafts["name"])
        assertEquals("local identity", next.drafts["identity"])
        assertEquals(null, next.conflict)
    }

    @Test
    fun clearingAFieldNormalizesTheDraftToTheServerEmptyValue() {
        val state = MemoryProfileEditState(profile(3, "Sam")).edit("name", " ")

        val next = state.saveSucceeded(profile(4, ""), "name", " ")

        assertEquals("", next.drafts["name"])
        assertEquals("", next.base.str("name"))
        assertEquals(true, next.isCurrentSubmission("name", ""))
    }

    @Test
    fun editMadeDuringSaveIsNotOverwrittenByOlderResponse() {
        val state = MemoryProfileEditState(profile(3, "Sam")).edit("name", "First")

        val newerDraft = state.edit("name", "Second")
        val next = newerDraft.saveSucceeded(profile(4, "First"), "name", "First")

        assertEquals(false, newerDraft.isCurrentSubmission("name", "First"))
        assertEquals("Second", next.drafts["name"])
        assertNotEquals(next.drafts["name"], next.base.str("name"))
    }

    @Test
    fun explicitReloadReplacesDraftsAndClearsConflict() {
        val state = MemoryProfileEditState(profile(1, "Sam")).edit("name", "Draft")
            .receiveRemote(profile(2, "Remote"))

        val next = state.useRemote(requireNotNull(state.conflict))

        assertEquals("Remote", next.drafts["name"])
        assertEquals(null, next.conflict)
        assertEquals(2, next.expectedVersion)
    }

    @Test
    fun profileUpdateSendsExpectedVersionAndJsonNullWhenClearing() {
        val params = profileUpdateParams("name", null, expectedVersion = 7)

        assertEquals(7, params["expected_version"]?.toString()?.toLong())
        assertEquals(JsonNull, params["name"])
        assertEquals(2, params.size)
    }

    @Test
    fun remoteEventIgnoredDuringSaveIsRecheckedAfterSaveCompletes() {
        val state = MemoryProfileEditState(profile(3, "Sam")).edit("identity", "local")
        val incoming = profile(5, "Sam", identity = "remote")

        val whileSaving = state.receiveRemote(incoming, requestInFlight = true)
        val afterSave = whileSaving.receiveRemote(incoming, requestInFlight = false)

        assertEquals("local", afterSave.drafts["identity"])
        assertEquals("remote", afterSave.conflict?.str("identity"))
    }

    @Test
    fun editPageFieldContractKeepsTextLimitsAndLanguagePickerDistinct() {
        assertEquals(listOf("name", "language", "identity"), MemoryProfileEditState.profileFields)
        assertTrue(isProfileField("name"))
        assertTrue(isProfileField("language"))
        assertTrue(isProfileField("identity"))
        assertEquals(20, profileFieldMaxLength("name"))
        assertEquals(null, profileFieldMaxLength("language"))
        assertEquals(100, profileFieldMaxLength("identity"))
        assertEquals(null, profileFieldMaxLength("timezone"))
        assertEquals(false, isProfileField("timezone"))
    }

    @Test
    fun dedicatedEditorSaveCallsProfileUpdateWithVersionAndReturnsUpdatedProfile() = runTest {
        val repository = FakeProfileRepository()
        repository.profile = profile(8, "Android Verify Fix")

        val updated = saveProfileField(repository, "host-1", "name", "Android Verify Fix", expectedVersion = 7)

        assertEquals("Android Verify Fix", updated.str("name"))
        assertEquals("memory.profile.update", repository.lastMethod)
        assertEquals(7, repository.lastParams?.get("expected_version")?.toString()?.toLong())
        assertEquals("Android Verify Fix", repository.lastParams?.str("name"))
    }

    @Test
    fun dedicatedEditorCanClearLanguageToFollowSystemUsingJsonNull() = runTest {
        val repository = FakeProfileRepository()

        saveProfileField(repository, "host-1", "language", null, expectedVersion = 4)

        assertEquals("memory.profile.update", repository.lastMethod)
        assertEquals(4, repository.lastParams?.get("expected_version")?.toString()?.toLong())
        assertEquals(JsonNull, repository.lastParams?.get("language"))
    }

    @Test
    fun dedicatedEditorSavePropagatesVersionConflictForConflictUi() = runTest {
        val latest = profile(9, "Server value")
        val repository = FakeProfileRepository().apply {
            failure = NetworkError("conflict", "Profile changed", buildJsonObject {
                put("details", buildJsonObject { put("current", latest) })
            })
        }

        val error = assertFailsWith<NetworkError> {
            saveProfileField(repository, "host-1", "name", "My value", expectedVersion = 8)
        }

        assertEquals("conflict", error.code)
        assertEquals("Server value", error.details?.obj("details")?.obj("current")?.str("name"))
    }

    private class FakeProfileRepository : MobileRepository {
        private val mutableState = MutableStateFlow(MobileState())
        private val mutableHost = MutableStateFlow(HostProfile(id = "host-1", name = "Host", addresses = listOf("localhost:7788"), password = ""))
        private val mutableStatuses = MutableStateFlow<Map<String, ConnectionSnapshot>>(emptyMap())
        private val mutableEvents = MutableSharedFlow<HostEvent>()
        var profile: JsonObject = buildJsonObject { put("version", 1) }
        var lastMethod: String? = null
        var lastParams: JsonObject? = null
        var failure: Throwable? = null

        override val state = mutableState
        override val hostStatuses = mutableStatuses
        override val hostEvents = mutableEvents
        override val activeHost = mutableHost

        override suspend fun call(method: String, params: JsonObject): JsonObject {
            lastMethod = method
            lastParams = params
            failure?.let { throw it }
            return buildJsonObject { put("profile", profile) }
        }

        override fun createScreen(botId: String, quality: String, tabId: String?, onFrame: suspend (ScreenFrame) -> Unit): ScreenConnection =
            error("screen is not used by profile editor tests")

        override suspend fun uploadFile(file: PickedFile): JsonObject = error("upload is not used by profile editor tests")
        override suspend fun fetchText(path: String, params: Map<String, String>): String = error("fetch is not used by profile editor tests")
        override suspend fun fetchBytes(path: String, params: Map<String, String>): ByteArray = error("fetch is not used by profile editor tests")
        override suspend fun refresh() = Unit
    }
}
