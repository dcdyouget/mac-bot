package bot.mac.mobile.feature.memory

import bot.mac.mobile.core.protocol.protocolJson
import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.jsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals

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
}
