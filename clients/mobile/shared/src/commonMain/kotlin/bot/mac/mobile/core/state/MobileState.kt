package bot.mac.mobile.core.state

import bot.mac.mobile.core.protocol.*
import bot.mac.mobile.core.network.ConnectionSnapshot
import bot.mac.mobile.core.network.MainEvent
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.*
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow

data class HostEvent(val hostId: String, val event: MainEvent, val state: MobileState)

@Serializable
data class MobileState(
    val lastSeq: Long = 0,
    val hello: JsonObject = buildJsonObject {},
    val bots: List<JsonObject> = emptyList(),
    val chats: List<JsonObject> = emptyList(),
    val projects: List<JsonObject> = emptyList(),
    val assignments: List<JsonObject> = emptyList(),
    val messages: Map<String, List<JsonObject>> = emptyMap(),
    val announcements: Map<String, JsonObject> = emptyMap(),
    val approvals: List<JsonObject> = emptyList(),
    val questions: List<JsonObject> = emptyList(),
    val skills: List<JsonObject> = emptyList(),
    val routines: List<JsonObject> = emptyList(),
    val workbench: JsonObject = buildJsonObject {},
    val settings: JsonObject = buildJsonObject {},
    val providers: List<JsonObject> = emptyList(),
    val models: List<JsonObject> = emptyList(),
    val traces: Map<String, List<JsonObject>> = emptyMap(),
    val traceFragments: Map<String, String> = emptyMap(),
    val typing: Map<String, Boolean> = emptyMap(),
    val connected: Boolean = false,
    val error: String? = null,
)
interface MobileRepository {
    val state: StateFlow<MobileState>
    val hostStatuses: StateFlow<Map<String, ConnectionSnapshot>>
    val hostEvents: SharedFlow<HostEvent>
    suspend fun call(method: String, params: JsonObject = buildJsonObject {}): JsonObject
    suspend fun callOnHost(hostId: String, method: String, params: JsonObject = buildJsonObject {}): JsonObject {
        check(activeHost.value?.id == hostId) { "Host is no longer active" }
        return call(method, params)
    }
    val activeHost: StateFlow<bot.mac.mobile.core.network.HostProfile?>
    fun createScreen(botId: String, quality: String = "auto", tabId: String? = null, onFrame: suspend (bot.mac.mobile.core.network.ScreenFrame) -> Unit): bot.mac.mobile.core.network.ScreenConnection
    suspend fun uploadFile(file: bot.mac.mobile.core.platform.PickedFile): JsonObject
    suspend fun fetchText(path: String, params: Map<String, String>): String
    suspend fun fetchBytes(path: String, params: Map<String, String>): ByteArray
    suspend fun refresh()
}
fun mergeById(old: List<JsonObject>, incoming: List<JsonObject>, key: String = "id"): List<JsonObject> =
    (old + incoming).associateBy { it.str(key) }.values.toList()
fun mergeMessages(old: List<JsonObject>, incoming: List<JsonObject>): List<JsonObject> =
    mergeById(old, incoming).sortedBy { it.long("seq") }
fun traceKey(item: JsonObject): String = item.str("assignment_id").ifBlank { item.str("chat_id") }

object StateReducer {
    fun bootstrap(current: MobileState, data: JsonObject): MobileState = current.copy(
        lastSeq = data.long("seq"), hello = data.obj("hello"), bots = data.objects("bots"), chats = data.objects("chats"),
        projects = data.objects("projects"), settings = data.obj("settings"), approvals = data.obj("pending").objects("approvals"),
        questions = data.obj("pending").objects("questions"), messages = emptyMap(), assignments = emptyList(),
        traces = emptyMap(), traceFragments = emptyMap(), announcements = emptyMap(),
        skills = emptyList(), routines = emptyList(), providers = emptyList(), models = emptyList(), workbench = buildJsonObject {},
    )
    fun event(current: MobileState, event: String, data: JsonObject, seq: Long? = null): MobileState {
        if (seq != null && seq <= current.lastSeq) return current
        var result = current
        fun remove(items: List<JsonObject>, id: String, key: String = "id") = items.filterNot { it.str(key) == id }
        when (event) {
            "bootstrap" -> return bootstrap(current, data)
            "chat.created", "chat.updated" -> result = current.copy(chats = mergeById(current.chats, listOf(data.obj("chat"))))
            "chat.deleted" -> result = current.copy(chats = remove(current.chats, data.str("chat_id")), messages = current.messages - data.str("chat_id"))
            "read.updated" -> result = current.copy(chats = current.chats.map { chat ->
                if (chat.str("id") != data.str("chat_id")) chat else JsonObject(chat + mapOf("last_read_seq" to JsonPrimitive(data.long("last_read_seq")), "unread" to JsonPrimitive(maxOf(0, chat.long("last_seq") - data.long("last_read_seq")))))
            })
            "message.created", "message.updated" -> {
                val message = data.obj("message"); val chatId = message.str("chat_id")
                val messages = mergeMessages(current.messages[chatId].orEmpty(), listOf(message))
                result = current.copy(messages = current.messages + (chatId to messages), chats = current.chats.map { chat ->
                    if (chat.str("id") != chatId || message.long("seq") < chat.long("last_seq")) chat else JsonObject(chat + mapOf(
                        "last_seq" to JsonPrimitive(message.long("seq")),
                        "last_message" to buildJsonObject { put("message_id", message.str("id")); put("text", message.str("fallback_text")); put("sender", message.obj("sender")); put("created_at", message.str("created_at")) }
                    ))
                })
            }
            "message.deleted" -> {
                val chatId = data.str("chat_id")
                result = current.copy(messages = current.messages + (chatId to remove(current.messages[chatId].orEmpty(), data.str("message_id"))))
            }
            "message.delta" -> {
                val chatId = data.str("chat_id")
                // Deltas are private-chat only. Group messages are atomic send_msg reports.
                if (current.chats.firstOrNull { it.str("id") == chatId }?.str("kind") != "project") {
                    val messages = current.messages[chatId].orEmpty().map { message ->
                        if (message.str("id") != data.str("message_id")) message else {
                            val text = data.str("text")
                            val blocks = message.objects("blocks").toMutableList()
                            val index = blocks.indexOfLast { it.str("type") == "text" }
                            if (index >= 0) blocks[index] = JsonObject(blocks[index] + ("markdown" to JsonPrimitive(blocks[index].str("markdown") + text)))
                            else blocks += buildJsonObject { put("type", "text"); put("markdown", text) }
                            JsonObject(message + mapOf("fallback_text" to JsonPrimitive(message.str("fallback_text") + text), "blocks" to JsonArray(blocks), "streaming" to JsonPrimitive(true)))
                        }
                    }
                    result = current.copy(messages = current.messages + (chatId to messages))
                }
            }
            "typing" -> result = current.copy(typing = current.typing + (data.str("chat_id") to data.boolean("on")))
            "bot.created", "bot.updated" -> result = current.copy(bots = mergeById(current.bots, listOf(data.obj("bot"))))
            "bot.deleted" -> result = current.copy(bots = remove(current.bots, data.str("bot_id")))
            "bot.status" -> result = current.copy(bots = current.bots.map { if (it.str("id") == data.str("bot_id")) JsonObject(it + ("status" to data.obj("status"))) else it })
            "project.created", "project.updated" -> result = current.copy(projects = mergeById(current.projects, listOf(data.obj("project"))))
            "announcement.updated" -> { val value = data.obj("announcement"); result = current.copy(announcements = current.announcements + (value.str("project_id") to value)) }
            "assignment.created", "assignment.updated" -> result = current.copy(assignments = mergeById(current.assignments, listOf(data.obj("assignment"))))
            "approval.requested", "approval.resolved" -> result = current.copy(approvals = mergeById(current.approvals, listOf(data.obj("approval"))))
            "question.asked", "question.answered" -> result = current.copy(questions = mergeById(current.questions, listOf(data.obj("question"))))
            "skill.updated" -> result = current.copy(skills = mergeById(current.skills, listOf(data.obj("skill")), "name"))
            "skill.deleted" -> result = current.copy(skills = remove(current.skills, data.str("name"), "name"))
            "routine.updated" -> result = current.copy(routines = mergeById(current.routines, listOf(data.obj("routine"))))
            "routine.run" -> {
                val run = data.obj("run")
                result = current.copy(routines = current.routines.map { if (it.str("id") == run.str("routine_id")) JsonObject(it + ("last_run" to run)) else it })
            }
            "artifact.registered" -> {
                val artifact = data.obj("artifact"); val projectId = artifact.str("project_id")
                val announcement = current.announcements[projectId] ?: buildJsonObject { put("project_id", projectId) }
                result = current.copy(announcements = current.announcements + (projectId to JsonObject(announcement + ("artifacts" to JsonArray(mergeById(announcement.objects("artifacts"), listOf(artifact)))))))
            }
            "routine.deleted" -> result = current.copy(routines = remove(current.routines, data.str("routine_id")))
            "settings.updated" -> result = current.copy(settings = data.obj("settings"))
            "provider.updated" -> result = current.copy(providers = mergeById(current.providers, listOf(data.obj("provider"))), models = mergeById(current.models, data.objects("models"), "ref"))
            "provider.deleted" -> result = current.copy(providers = remove(current.providers, data.str("provider_id")))
            "usage.tick" -> result = current.copy(assignments = current.assignments.map { if (it.str("id") == data.str("assignment_id")) JsonObject(it + ("usage" to data.obj("usage"))) else it })
            "host.status" -> result = current.copy(workbench = JsonObject(current.workbench + data))
            "trace.item" -> {
                val item = data.obj("item"); val key = traceKey(item)
                val traces = (current.traces[key].orEmpty() + item).associateBy { it.long("aseq") }.values.sortedBy { it.long("aseq") }
                val prefix = data.str("stream") + ":" + item.obj("data").str("request_id") + ":"
                result = current.copy(traces = current.traces + (key to traces), traceFragments = if (item.str("type") == "llm.response") current.traceFragments.filterKeys { !it.startsWith(prefix) } else current.traceFragments)
            }
            "trace.delta" -> {
                val key = data.str("stream") + ":" + data.str("request_id") + ":" + data.str("channel")
                result = current.copy(traceFragments = current.traceFragments + (key to (current.traceFragments[key].orEmpty() + data.str("text"))))
            }
            "trace.tool_output" -> {
                val key = data.str("stream") + ":" + data.str("call_id") + ":tool"
                result = current.copy(traceFragments = current.traceFragments + (key to (current.traceFragments[key].orEmpty() + data.str("chunk")).takeLast(8192)))
            }
        }
        return result.copy(lastSeq = seq ?: result.lastSeq)
    }
}
