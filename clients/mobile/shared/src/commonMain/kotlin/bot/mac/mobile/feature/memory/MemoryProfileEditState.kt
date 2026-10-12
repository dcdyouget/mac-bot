package bot.mac.mobile.feature.memory

import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** Keeps local drafts separate from the last server snapshot while profile RPCs/events race. */
data class MemoryProfileEditState(
    val base: JsonObject,
    val drafts: Map<String, String> = profileFields.associateWith(base::str),
    val conflict: JsonObject? = null,
) {
    val expectedVersion: Long get() = base.long("version")

    fun edit(field: String, value: String) = copy(drafts = drafts + (field to value))

    fun isCurrentSubmission(field: String, submitted: String): Boolean = drafts[field].orEmpty() == submitted

    fun receiveRemote(remote: JsonObject, requestInFlight: Boolean = false): MemoryProfileEditState {
        if (remote.long("version") <= expectedVersion || requestInFlight) return this
        val hasLocalChanges = profileFields.any { drafts[it].orEmpty() != base.str(it) }
        return if (hasLocalChanges) copy(conflict = remote) else copy(base = remote, drafts = profileFields.associateWith(remote::str))
    }

    fun saveSucceeded(remote: JsonObject, field: String, submitted: String?): MemoryProfileEditState {
        val nextDrafts = drafts.toMutableMap()
        if (nextDrafts[field].orEmpty() == submitted.orEmpty()) nextDrafts[field] = remote.str(field)
        return copy(base = remote, drafts = nextDrafts, conflict = null)
    }

    fun useRemote(remote: JsonObject) = copy(base = remote, drafts = profileFields.associateWith(remote::str), conflict = null)

    companion object {
        val profileFields = listOf("name", "language", "identity")
    }
}

internal fun profileUpdateParams(field: String, value: String?, expectedVersion: Long): JsonObject = buildJsonObject {
    put("expected_version", expectedVersion)
    if (value == null) put(field, JsonNull) else put(field, value)
}
