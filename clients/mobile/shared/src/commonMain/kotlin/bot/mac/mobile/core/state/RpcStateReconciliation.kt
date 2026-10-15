package bot.mac.mobile.core.state

import bot.mac.mobile.core.network.MainEvent
import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonObject

/** A bounded request-local journal; never persisted or shared across Host generations. */
internal class RpcStateFence(val initial: MobileState, val epoch: Long) {
    val events = mutableListOf<MainEvent>()
    var overflow = false

    fun record(event: MainEvent) {
        if (overflow) return
        if (events.size == 4096) {
            events.clear()
            overflow = true
        } else events += event
    }
}

/**
 * Replay intervening events onto the response snapshot (including deletes for
 * entities not yet cached), then retain entities changed by newer local commits.
 * Untouched response entities still populate initial loads and history pages.
 */
internal fun reconcileRpc(
    fence: RpcStateFence,
    current: MobileState,
    method: String,
    params: JsonObject,
    response: JsonObject,
): MobileState {
    val initial = fence.initial
    var incoming = RpcStateReducer.apply(initial, method, params, response)
    fence.events.forEach { event ->
        incoming = when (event) {
            is MainEvent.Persistent -> StateReducer.event(incoming, event.event, event.data)
            is MainEvent.Ephemeral -> StateReducer.event(incoming, event.event, event.data)
            is MainEvent.Hello -> incoming
        }
    }
    fun <T> value(before: T, now: T, result: T): T = if (now == before) result else now
    fun <T> map(before: Map<String, T>, now: Map<String, T>, result: Map<String, T>): Map<String, T> =
        (before.keys + now.keys + result.keys).mapNotNull { key ->
            value(before[key], now[key], result[key])?.let { key to it }
        }.toMap()
    fun items(before: List<JsonObject>, now: List<JsonObject>, result: List<JsonObject>, key: String = "id"): List<JsonObject> =
        map(before.associateBy { it.str(key) }, now.associateBy { it.str(key) }, result.associateBy { it.str(key) }).values.toList()
    fun collections(before: Map<String, List<JsonObject>>, now: Map<String, List<JsonObject>>, result: Map<String, List<JsonObject>>, key: String): Map<String, List<JsonObject>> =
        (before.keys + now.keys + result.keys).associateWith { id ->
            items(before[id].orEmpty(), now[id].orEmpty(), result[id].orEmpty(), key).sortedBy { it.long(if (key == "aseq") "aseq" else "seq") }
        }
    return current.copy(
        bots = items(initial.bots, current.bots, incoming.bots),
        chats = items(initial.chats, current.chats, incoming.chats),
        projects = items(initial.projects, current.projects, incoming.projects),
        assignments = items(initial.assignments, current.assignments, incoming.assignments),
        messages = collections(initial.messages, current.messages, incoming.messages, "id"),
        announcements = map(initial.announcements, current.announcements, incoming.announcements),
        approvals = items(initial.approvals, current.approvals, incoming.approvals),
        questions = items(initial.questions, current.questions, incoming.questions),
        skills = items(initial.skills, current.skills, incoming.skills, "name"),
        routines = items(initial.routines, current.routines, incoming.routines),
        providers = items(initial.providers, current.providers, incoming.providers),
        models = items(initial.models, current.models, incoming.models, "ref"),
        memoryEntries = items(initial.memoryEntries, current.memoryEntries, incoming.memoryEntries),
        workbench = value(initial.workbench, current.workbench, incoming.workbench),
        settings = value(initial.settings, current.settings, incoming.settings),
        memoryQuota = value(initial.memoryQuota, current.memoryQuota, incoming.memoryQuota),
        userProfile = value(initial.userProfile, current.userProfile, incoming.userProfile),
        traces = collections(initial.traces, current.traces, incoming.traces, "aseq"),
        traceFragments = map(initial.traceFragments, current.traceFragments, incoming.traceFragments),
        typing = map(initial.typing, current.typing, incoming.typing),
        error = null,
    )
}
