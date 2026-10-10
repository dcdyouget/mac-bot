package bot.mac.mobile.feature.chat

import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonObject

/** Resolves the human-readable project name without weakening review-card actions. */
internal fun reviewProjectLabel(block: JsonObject, projects: List<JsonObject>): String {
    val projectId = block.str("project_id")
    val stateLabel = projects.firstOrNull { it.str("id") == projectId }?.str("name").orEmpty()
    return stateLabel.ifBlank { projectId }
}
