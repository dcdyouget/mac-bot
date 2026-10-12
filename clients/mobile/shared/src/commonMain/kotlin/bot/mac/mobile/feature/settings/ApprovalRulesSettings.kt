package bot.mac.mobile.feature.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.network.RandomIdGenerator
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.approval_mode_auto
import bot.mac.mobile.resources.approval_mode_auto_hint
import bot.mac.mobile.resources.approval_mode_ask
import bot.mac.mobile.resources.approval_rule_add
import bot.mac.mobile.resources.approval_rule_all_bots
import bot.mac.mobile.resources.approval_rule_builtin
import bot.mac.mobile.resources.approval_save_failed
import bot.mac.mobile.resources.approval_rule_kind_allow
import bot.mac.mobile.resources.approval_rule_kind_ask
import bot.mac.mobile.resources.approval_rule_scope
import bot.mac.mobile.resources.approval_rule_title
import bot.mac.mobile.resources.approval_rule_user_added
import bot.mac.mobile.resources.approval_rules_title
import bot.mac.mobile.resources.common_cancel
import bot.mac.mobile.resources.common_delete
import bot.mac.mobile.resources.common_save
import bot.mac.mobile.resources.feature_loading
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlin.time.Clock
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

@Composable
fun ApprovalRulesSettings(repository: MobileRepository) {
    val state by repository.state.collectAsState()
    val activeHost by repository.activeHost.collectAsState()
    val hostId = activeHost?.id
    val scope = rememberCoroutineScope()
    val approvals = state.settings["approvals"] as? JsonObject
    val mode = approvals?.str("mode").orEmpty().ifBlank { "always_allow" }
    val rules = approvals?.arr("rules")?.mapNotNull { it as? JsonObject }.orEmpty()
    var dialogOpen by remember(hostId) { mutableStateOf(false) }
    var error by remember(hostId) { mutableStateOf<String?>(null) }
    var saving by remember(hostId) { mutableStateOf(false) }
    val updateMutex = remember(hostId) { Mutex() }

    fun update(change: (String, List<JsonObject>) -> Pair<String, List<JsonObject>>) {
        val requestHost = hostId ?: return
        if (saving) return
        saving = true
        scope.launch {
            try {
                updateMutex.withLock {
                    if (repository.activeHost.value?.id != requestHost) return@withLock
                    val current = repository.state.value.settings["approvals"] as? JsonObject ?: return@withLock
                    val currentMode = current.str("mode").ifBlank { "always_allow" }
                    val currentRules = current.arr("rules").mapNotNull { it as? JsonObject }
                    val (nextMode, nextRules) = change(currentMode, currentRules)
                    error = null
                    runCatching {
                        repository.callOnHost(requestHost, "settings.update", buildJsonObject {
                            put("patch", buildJsonObject {
                                put("approvals", buildJsonObject {
                                    put("mode", nextMode)
                                    put("rules", JsonArray(nextRules))
                                })
                            })
                        })
                    }.onFailure { if (repository.activeHost.value?.id == requestHost) error = "save_failed" }
                }
            } finally {
                if (repository.activeHost.value?.id == requestHost) saving = false
            }
        }
    }

    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(stringResource(Res.string.approval_rules_title), style = MaterialTheme.typography.titleMedium)
        if (approvals == null) {
            Text(stringResource(Res.string.feature_loading), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            return@Column
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(
                selected = mode == "always_allow",
                onClick = { if (!saving && mode != "always_allow") update { _, currentRules -> "always_allow" to currentRules } },
                enabled = !saving,
                label = { Text(stringResource(Res.string.approval_mode_auto)) },
            )
            FilterChip(
                selected = mode == "require",
                onClick = { if (!saving && mode != "require") update { _, currentRules -> "require" to currentRules } },
                enabled = !saving,
                label = { Text(stringResource(Res.string.approval_mode_ask)) },
            )
        }
        if (mode == "always_allow") {
            Text(stringResource(Res.string.approval_mode_auto_hint), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        rules.forEach { rule ->
            val builtin = rule["builtin"]?.toString() == "true"
            val botId = rule.str("bot_id").takeIf { it.isNotBlank() }
            val botName = botId?.let { id -> state.bots.firstOrNull { it.str("id") == id }?.str("name")?.takeIf(String::isNotBlank) ?: id }
            Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Column(Modifier.weight(1f)) {
                    Text(rule.str("text"), style = MaterialTheme.typography.bodyMedium)
                    Text(
                        "${if (rule.str("kind") == "auto_allow") stringResource(Res.string.approval_rule_kind_allow) else stringResource(Res.string.approval_rule_kind_ask)} · ${botName ?: stringResource(Res.string.approval_rule_all_bots)} · ${if (builtin) stringResource(Res.string.approval_rule_builtin) else stringResource(Res.string.approval_rule_user_added)}",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (!builtin) TextButton(enabled = !saving, onClick = { update { currentMode, currentRules -> currentMode to currentRules.filterNot { it.str("id") == rule.str("id") } } }) {
                    Text(stringResource(Res.string.common_delete), color = MaterialTheme.colorScheme.error)
                }
            }
        }
        TextButton(enabled = !saving, onClick = { dialogOpen = true }) { Text(stringResource(Res.string.approval_rule_add)) }
    }
    if (error != null) Text(stringResource(Res.string.approval_save_failed), Modifier.padding(top = 4.dp), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)

    if (dialogOpen) {
        var text by remember { mutableStateOf("") }
        var kind by remember { mutableStateOf("ask_first") }
        var botId by remember { mutableStateOf<String?>(null) }
        var botMenu by remember { mutableStateOf(false) }
        AlertDialog(
            onDismissRequest = { dialogOpen = false },
            title = { Text(stringResource(Res.string.approval_rule_title)) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(text, { text = it }, label = { Text(stringResource(Res.string.approval_rule_title)) }, singleLine = true)
                    Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        FilterChip(kind == "ask_first", { kind = "ask_first" }, label = { Text(stringResource(Res.string.approval_rule_kind_ask)) })
                        FilterChip(kind == "auto_allow", { kind = "auto_allow" }, label = { Text(stringResource(Res.string.approval_rule_kind_allow)) })
                    }
                    val scopeLabel = state.bots.firstOrNull { it.str("id") == botId }?.str("name")
                        ?: stringResource(Res.string.approval_rule_all_bots)
                    TextButton(onClick = { botMenu = true }) { Text("${stringResource(Res.string.approval_rule_scope)}: $scopeLabel") }
                    DropdownMenu(expanded = botMenu, onDismissRequest = { botMenu = false }) {
                        DropdownMenuItem(text = { Text(stringResource(Res.string.approval_rule_all_bots)) }, onClick = { botId = null; botMenu = false })
                        state.bots.forEach { bot -> DropdownMenuItem(text = { Text(bot.str("name")) }, onClick = { botId = bot.str("id"); botMenu = false }) }
                    }
                }
            },
            confirmButton = {
                Button(enabled = text.isNotBlank() && !saving, onClick = {
                    val newRule = buildJsonObject {
                        put("id", RandomIdGenerator.nextId())
                        put("kind", kind)
                        put("text", text.trim())
                        put("created_at", Clock.System.now().toString())
                        put("bot_id", botId?.let(::JsonPrimitive) ?: JsonNull)
                        put("builtin", false)
                    }
                    update { currentMode, currentRules -> currentMode to (currentRules + newRule) }
                    dialogOpen = false
                }) { Text(stringResource(Res.string.common_save)) }
            },
            dismissButton = { TextButton(onClick = { dialogOpen = false }) { Text(stringResource(Res.string.common_cancel)) } },
        )
    }
}
