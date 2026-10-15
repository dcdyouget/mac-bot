package bot.mac.mobile.core.state

import bot.mac.mobile.core.protocol.*
import bot.mac.mobile.core.protocol.generated.MemoryProfileResult
import bot.mac.mobile.core.protocol.generated.MemoryUserDeleteResult
import bot.mac.mobile.core.protocol.generated.MemoryUserEntryResult
import bot.mac.mobile.core.protocol.generated.MemoryUserListResult
import bot.mac.mobile.core.protocol.generated.QuestionListResult
import kotlinx.serialization.json.*

object RpcStateReducer {
    fun apply(current: MobileState, method: String, params: JsonObject, result: JsonObject): MobileState {
        var next = current
        if (method == "bootstrap") return StateReducer.bootstrap(current, result)
        val lists = mapOf(
            "bots" to ("bot" to "bot.list"),
            "chats" to ("chat" to "chat.list"),
            "projects" to ("project" to "project.list"),
            "approvals" to ("approval" to "approval.list"),
            "questions" to ("question" to "question.list"),
            "skills" to ("skill" to "skill.list"),
            "routines" to ("routine" to "routine.list"),
        )
        lists.forEach { (plural, descriptor) ->
            val (singular, listMethod) = descriptor
            if (result[plural] is JsonArray && method == listMethod) {
                val objects = if (plural == "questions") QuestionListResult(result).questions.map { it.raw } else result.objects(plural)
                next = when (plural) {
                    "bots" -> next.copy(bots = if (params["include_hidden"] == JsonPrimitive(true)) objects else mergeById(next.bots.filter { it["hidden"] == JsonPrimitive(true) }, objects))
                    // Chat has no archived flag; an unfiltered response is the only
                    // authoritative whole-cache replacement.
                    "chats" -> next.copy(chats = if (params["include_archived"] == JsonPrimitive(true)) objects else mergeById(next.chats, objects))
                    "projects" -> next.copy(projects = if (params["status"] is JsonArray) mergeById(next.projects, objects) else objects)
                    "approvals" -> next.copy(approvals = if (params["state"] is JsonArray) mergeById(next.approvals, objects) else objects)
                    "questions" -> {
                        val chatId = params.str("chat_id")
                        val assignmentId = params.str("assignment_id")
                        if (chatId.isBlank() && assignmentId.isBlank()) next.copy(questions = objects)
                        else {
                            val replaced = next.questions.filterNot { question ->
                                (chatId.isBlank() || question.str("chat_id") == chatId) &&
                                    (assignmentId.isBlank() || question.str("assignment_id") == assignmentId) &&
                                    ((params["state"] as? JsonArray)?.let { question["state"] in it } ?: true)
                            }
                            next.copy(questions = mergeById(replaced, objects))
                        }
                    }
                    "skills" -> next.copy(skills = objects); "routines" -> next.copy(routines = if (params.str("bot_id").isNotBlank()) mergeById(next.routines, objects) else objects)
                    else -> next
                }
            }
            if (result[singular] is JsonObject) next = StateReducer.event(next, singular + ".updated", buildJsonObject { put(singular, result.getValue(singular)) })
        }
        if (method == "bot.create_from_template") {
            next = next.copy(bots = mergeById(next.bots, result.objects("bots")))
        }
        if (method == "skill.import") {
            next = next.copy(skills = mergeById(next.skills, result.objects("skills"), "name"))
        }
        if (result["dm_chat"] is JsonObject) next = StateReducer.event(next, "chat.created", buildJsonObject { put("chat", result.getValue("dm_chat")) })
        result.objects("dm_chats").forEach { next = StateReducer.event(next, "chat.created", buildJsonObject { put("chat", it) }) }
        if (result["message"] is JsonObject) next = StateReducer.event(next, "message.updated", buildJsonObject { put("message", result.getValue("message")) })
        if (result["assignment"] is JsonObject) next = StateReducer.event(next, "assignment.updated", buildJsonObject { put("assignment", result.getValue("assignment")) })
        if (result["announcement"] is JsonObject) next = StateReducer.event(next, "announcement.updated", buildJsonObject { put("announcement", result.getValue("announcement")) })
        if (result["settings"] is JsonObject) next = next.copy(settings = result.obj("settings"))
        if (result["providers"] is JsonArray) {
            next = next.copy(providers = result.objects("providers"))
            if (result["models"] is JsonArray) next = next.copy(models = result.objects("models"))
        }
        if (result["provider"] is JsonObject) {
            next = StateReducer.event(next, "provider.updated", buildJsonObject {
                put("provider", result.getValue("provider"))
                if (result["models"] is JsonArray) put("models", result.getValue("models"))
            })
        }
        if (method == "chat.history") { val id = params.str("chat_id"); next = next.copy(messages = next.messages + (id to mergeMessages(next.messages[id].orEmpty(), result.objects("messages")))) }
        if (method == "assignment.list") next = next.copy(assignments = mergeById(next.assignments, result.objects("items")))
        if (method == "trace.history") result.objects("items").forEach { next = StateReducer.event(next, "trace.item", buildJsonObject { put("item", it) }) }
        if (method == "trace.subscribe") result.objects("in_flight").forEach { flight ->
            val prefix = result.str("stream") + ":" + flight.str("request_id") + ":"
            next = next.copy(traceFragments = next.traceFragments + mapOf(prefix + "text" to flight.str("text"), prefix + "thinking" to flight.str("thinking")))
        }
        if (method == "workbench.get") {
            val assignments = result.objects("bots").flatMap { it.objects("assignments") } + result.objects("done_today")
            val waiting = result.objects("waiting")
            val approvals = waiting
                .filter { it.str("kind") == "approval" }
                .mapNotNull { it["approval"] as? JsonObject }
            val questions = waiting
                .filter { it.str("kind") == "question" }
                .mapNotNull { it["question"] as? JsonObject }
            next = next.copy(
                workbench = result,
                assignments = mergeById(next.assignments, assignments),
                approvals = mergeById(next.approvals, approvals),
                questions = mergeById(next.questions, questions),
            )
        }
        if (method == "model.upsert" && result["model"] is JsonObject) {
            next = next.copy(models = mergeById(next.models, listOf(result.getValue("model").jsonObject), "ref"))
        }
        if (method == "model.refresh") next = next.copy(models = mergeById(next.models, result.objects("models"), "ref"))
        when (method) {
            "memory.user.list" -> next = next.copy(
                memoryEntries = MemoryUserListResult(result).entries.map { it.raw },
                memoryQuota = MemoryUserListResult(result).quota.raw,
            )
            "memory.user.create", "memory.user.update" -> next = next.copy(
                memoryEntries = mergeById(next.memoryEntries, listOf(MemoryUserEntryResult(result).entry.raw)),
                memoryQuota = MemoryUserEntryResult(result).quota.raw,
            )
            "memory.user.delete" -> next = next.copy(
                memoryEntries = next.memoryEntries.filterNot { it.str("id") == params.str("entry_id") },
                memoryQuota = MemoryUserDeleteResult(result).quota.raw,
            )
            "memory.profile.get", "memory.profile.update" -> next = next.copy(userProfile = MemoryProfileResult(result).profile.raw)
        }
        if (method == "model.delete") {
            val ref = params.str("ref")
            next = next.copy(models = next.models.filterNot { it.str("ref") == ref })
        } else if (method.endsWith(".delete")) {
            val type = method.substringBefore('.')
            val key = if (type == "skill") "name" else type + "_id"
            next = StateReducer.event(next, type + ".deleted", buildJsonObject { put(key, params.str(key)) })
        }
        return next
    }

}
