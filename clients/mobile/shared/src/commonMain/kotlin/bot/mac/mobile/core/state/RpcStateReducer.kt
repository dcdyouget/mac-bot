package bot.mac.mobile.core.state

import bot.mac.mobile.core.protocol.*
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
                val objects = result.objects(plural)
                next = when (plural) {
                    "bots" -> next.copy(bots = objects); "chats" -> next.copy(chats = objects); "projects" -> next.copy(projects = objects)
                    "approvals" -> next.copy(approvals = objects); "questions" -> next.copy(questions = objects); "skills" -> next.copy(skills = objects); "routines" -> next.copy(routines = objects)
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
