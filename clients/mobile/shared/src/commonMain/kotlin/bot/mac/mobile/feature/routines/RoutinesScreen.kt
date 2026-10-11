package bot.mac.mobile.feature.routines

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.BotAvatar
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.objects
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.common_cancel
import bot.mac.mobile.resources.common_back
import bot.mac.mobile.resources.common_save
import bot.mac.mobile.resources.common_retry
import bot.mac.mobile.resources.feature_loading
import bot.mac.mobile.resources.feature_name
import bot.mac.mobile.resources.feature_no_records
import bot.mac.mobile.resources.feature_readonly
import bot.mac.mobile.resources.routines_add_time
import bot.mac.mobile.resources.routines_bot
import bot.mac.mobile.resources.routines_create_hint
import bot.mac.mobile.resources.routines_delete
import bot.mac.mobile.resources.routines_delete_confirm
import bot.mac.mobile.resources.routines_deliver_to
import bot.mac.mobile.resources.routines_details
import bot.mac.mobile.resources.routines_dm
import bot.mac.mobile.resources.routines_empty
import bot.mac.mobile.resources.routines_error
import bot.mac.mobile.resources.routines_filter_all
import bot.mac.mobile.resources.routines_filter_paused
import bot.mac.mobile.resources.routines_filter_running
import bot.mac.mobile.resources.routines_edit
import bot.mac.mobile.resources.routines_history
import bot.mac.mobile.resources.routines_instruction
import bot.mac.mobile.resources.routines_new
import bot.mac.mobile.resources.routines_no_bots
import bot.mac.mobile.resources.routines_no_groups
import bot.mac.mobile.resources.routines_pause
import bot.mac.mobile.resources.routines_resume
import bot.mac.mobile.resources.routines_schedule_cron
import bot.mac.mobile.resources.routines_schedule_label
import bot.mac.mobile.resources.routines_select_bot
import bot.mac.mobile.resources.routines_status_done
import bot.mac.mobile.resources.routines_status_failed
import bot.mac.mobile.resources.routines_status_running
import bot.mac.mobile.resources.routines_status_skipped
import bot.mac.mobile.resources.routines_timezone
import bot.mac.mobile.resources.routines_title
import bot.mac.mobile.resources.routines_try
import bot.mac.mobile.resources.routines_when
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

private suspend fun <T> routineRequest(onError: (String) -> Unit, block: suspend () -> T): T? = try {
    block()
} catch (cancelled: CancellationException) {
    throw cancelled
} catch (failure: Throwable) {
    onError(failure.message.orEmpty())
    null
}

private data class RoutineScheduleDraft(val label: String = "", val cron: String = "")

@Composable
fun RoutinesScreen(repository: MobileRepository, onOpenAssignment: (String) -> Unit, onBack: () -> Unit) {
    val state by repository.state.collectAsState()
    var selectedRoutineId by remember { mutableStateOf<String?>(null) }
    var creating by remember { mutableStateOf(false) }
    var editing by remember { mutableStateOf(false) }
    var filter by remember { mutableStateOf("all") }
    var history by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var actionError by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }
    var confirmDelete by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(Unit) {
        routineRequest({ actionError = it }) { repository.call("routine.list") }
        loading = false
    }

    val selectedRoutine = state.routines.firstOrNull { it.str("id") == selectedRoutineId }
    val openList = {
        selectedRoutineId = null
        creating = false
        editing = false
        history = emptyList()
    }
    val refresh: () -> Unit = {
        scope.launch {
            loading = true
            routineRequest({ actionError = it }) { repository.call("routine.list") }
            loading = false
        }
        Unit
    }

    when {
        selectedRoutine != null || creating -> RoutineDetailScreen(
            repository = repository,
            routine = selectedRoutine,
            bots = state.bots,
            projects = state.projects.filter { it.str("status") != "archived" },
            history = history,
            editing = editing || creating,
            creating = creating,
            onBack = openList,
            onEdit = { editing = true },
            onToggle = { enabled ->
                selectedRoutineId?.let { id -> scope.launch {
                    actionError = null
                    routineRequest({ actionError = it }) {
                        repository.call("routine.set_enabled", buildJsonObject { put("routine_id", id); put("enabled", enabled) })
                        repository.call("routine.list")
                    }
                } }
            },
            onSave = { botId, name, instructions, schedules, timezone, projectId ->
                scope.launch {
                    actionError = null
                    val saved = routineRequest({ actionError = it }) {
                        if (creating) {
                            repository.call("routine.create", routineCreateParams(botId, name, instructions, schedules, timezone, projectId))
                        } else {
                            repository.call("routine.update", routineUpdateParams(selectedRoutine!!.str("id"), botId, name, instructions, schedules, timezone, projectId))
                        }
                    }
                    if (saved != null) {
                        creating = false
                        editing = false
                        val id = saved.obj("routine").str("id").ifBlank { selectedRoutineId.orEmpty() }
                        selectedRoutineId = id.takeIf { it.isNotBlank() }
                        routineRequest({ actionError = it }) { repository.call("routine.list") }
                        selectedRoutineId?.let { routineId ->
                            routineRequest({ actionError = it }) {
                                history = repository.call("routine.runs", buildJsonObject { put("routine_id", routineId) }).objects("runs")
                            }
                        }
                    }
                }
            },
            onTry = {
                selectedRoutineId?.let { id -> scope.launch {
                    actionError = null
                    routineRequest({ actionError = it }) { repository.call("routine.test_run", buildJsonObject { put("routine_id", id) }) }
                    routineRequest({ actionError = it }) {
                        history = repository.call("routine.runs", buildJsonObject { put("routine_id", id) }).objects("runs")
                        repository.call("routine.list")
                    }
                } }
            },
            onDelete = { confirmDelete = true },
            onOpenAssignment = onOpenAssignment,
            actionError = actionError,
        )
        else -> RoutineListScreen(
            repository = repository,
            routines = state.routines,
            bots = state.bots,
            filter = filter,
            loading = loading,
            error = actionError,
            onBack = onBack,
            onFilter = { filter = it },
            onCreate = { creating = true; editing = true; actionError = null },
            onOpen = { routine ->
                selectedRoutineId = routine.str("id")
                actionError = null
                editing = false
                scope.launch {
                    history = routineRequest({ actionError = it }) {
                        repository.call("routine.runs", buildJsonObject { put("routine_id", routine.str("id")) }).objects("runs")
                    }.orEmpty()
                }
            },
            onToggle = { routine, enabled ->
                scope.launch {
                    actionError = null
                    routineRequest({ actionError = it }) {
                        repository.call("routine.set_enabled", buildJsonObject { put("routine_id", routine.str("id")); put("enabled", enabled) })
                        repository.call("routine.list")
                    }
                }
            },
            onRetry = refresh,
        )
    }

    if (confirmDelete && selectedRoutine != null) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text(stringResource(Res.string.routines_delete)) },
            text = { Text(stringResource(Res.string.routines_delete_confirm)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmDelete = false
                    scope.launch {
                        actionError = null
                        val deleted = routineRequest({ actionError = it }) {
                            repository.call("routine.delete", buildJsonObject { put("routine_id", selectedRoutine.str("id")) })
                        }
                        if (deleted != null) {
                            routineRequest({ actionError = it }) { repository.call("routine.list") }
                            openList()
                        }
                    }
                }) { Text(stringResource(Res.string.routines_delete)) }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text(stringResource(Res.string.common_cancel)) } },
        )
    }
}

@Composable
private fun RoutineListScreen(
    repository: MobileRepository,
    routines: List<JsonObject>,
    bots: List<JsonObject>,
    filter: String,
    loading: Boolean,
    error: String?,
    onBack: () -> Unit,
    onFilter: (String) -> Unit,
    onCreate: () -> Unit,
    onOpen: (JsonObject) -> Unit,
    onToggle: (JsonObject, Boolean) -> Unit,
    onRetry: () -> Unit,
) {
    val backLabel = stringResource(Res.string.common_back)
    val createLabel = stringResource(Res.string.routines_new)
    val filtered = when (filter) {
        "running" -> routines.filter { it.boolean("enabled") == true }
        "paused" -> routines.filter { it.boolean("enabled") != true }
        else -> routines
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack, modifier = Modifier.semantics { contentDescription = backLabel }) { Text("‹", style = MaterialTheme.typography.headlineSmall) }
            Text(stringResource(Res.string.routines_title), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
            IconButton(onClick = onCreate, modifier = Modifier.semantics { contentDescription = createLabel }) { Text("＋", style = MaterialTheme.typography.titleLarge) }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            listOf("all" to Res.string.routines_filter_all, "running" to Res.string.routines_filter_running, "paused" to Res.string.routines_filter_paused).forEach { (value, label) ->
                FilterChip(selected = filter == value, onClick = { onFilter(value) }, label = { Text(stringResource(label)) })
            }
        }
        error?.let {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(Res.string.routines_error, it), Modifier.weight(1f), color = MaterialTheme.colorScheme.error)
                TextButton(onClick = onRetry) { Text(stringResource(Res.string.common_retry)) }
            }
        }
        HorizontalDivider()
        when {
            loading -> Text(stringResource(Res.string.feature_loading), Modifier.padding(20.dp))
            filtered.isEmpty() -> Column(Modifier.fillMaxWidth().padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(stringResource(Res.string.routines_empty), style = MaterialTheme.typography.bodyLarge)
                TextButton(onClick = onCreate) { Text("＋ ${stringResource(Res.string.routines_new)}") }
            }
            else -> LazyColumn(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(0.dp)) {
                items(filtered, key = { it.str("id").ifBlank { "routine:${it.hashCode()}" } }) { routine ->
                    RoutineRow(
                        repository = repository,
                        routine = routine,
                        bot = bots.firstOrNull { it.str("id") == routine.str("bot_id") },
                        onOpen = { onOpen(routine) },
                        onToggle = { onToggle(routine, it) },
                    )
                    HorizontalDivider()
                }
            }
        }
    }
}

@Composable
private fun RoutineRow(repository: MobileRepository, routine: JsonObject, bot: JsonObject?, onOpen: () -> Unit, onToggle: (Boolean) -> Unit) {
    val enabled = routine.boolean("enabled") == true
    val schedule = routine.objects("schedules").firstOrNull()
    val subtitle = schedule?.str("label").orEmpty().ifBlank { schedule?.str("cron").orEmpty() }
    Row(
        Modifier.fillMaxWidth().height(72.dp).clickable(onClick = onOpen).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(Modifier.size(40.dp), contentAlignment = Alignment.Center) {
            BotAvatar(repository, bot, main = bot?.boolean("is_main") == true, status = bot?.obj("status")?.str("active").orEmpty())
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(routine.str("name"), maxLines = 1, style = MaterialTheme.typography.titleSmall)
            Text(
                listOf(subtitle, bot?.str("name").orEmpty()).filter { it.isNotBlank() }.joinToString(" · "),
                maxLines = 1,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Switch(checked = enabled, onCheckedChange = onToggle)
    }
}

@Composable
private fun RoutineDetailScreen(
    repository: MobileRepository,
    routine: JsonObject?,
    bots: List<JsonObject>,
    projects: List<JsonObject>,
    history: List<JsonObject>,
    editing: Boolean,
    creating: Boolean,
    onBack: () -> Unit,
    onEdit: () -> Unit,
    onToggle: (Boolean) -> Unit,
    onSave: (String, String, String, List<RoutineScheduleDraft>, String, String?) -> Unit,
    onTry: () -> Unit,
    onDelete: () -> Unit,
    onOpenAssignment: (String) -> Unit,
    actionError: String?,
) {
    val backLabel = stringResource(Res.string.common_back)
    if (!creating && routine == null) {
        Column(Modifier.fillMaxSize().padding(16.dp)) {
            Button(onClick = onBack) { Text(stringResource(Res.string.common_back)) }
            Text(stringResource(Res.string.feature_loading), Modifier.padding(top = 16.dp))
        }
        return
    }
    var name by remember(routine?.str("id"), creating) { mutableStateOf(routine?.str("name").orEmpty()) }
    var instructions by remember(routine?.str("id"), creating) { mutableStateOf(routine?.str("instructions").orEmpty()) }
    var timezone by remember(routine?.str("id"), creating) { mutableStateOf(routine?.str("timezone").orEmpty().ifBlank { "Asia/Shanghai" }) }
    var botId by remember(routine?.str("id"), creating, bots) { mutableStateOf(routine?.str("bot_id").orEmpty().ifBlank { bots.firstOrNull()?.str("id").orEmpty() }) }
    var projectId by remember(routine?.str("id"), creating) { mutableStateOf(routine?.str("project_id").takeUnless { it.isNullOrBlank() }) }
    var schedules by remember(routine?.str("id"), creating) {
        mutableStateOf(routine?.objects("schedules")?.map { RoutineScheduleDraft(it.str("label"), it.str("cron")) }
            ?.ifEmpty { listOf(RoutineScheduleDraft()) } ?: listOf(RoutineScheduleDraft()))
    }
    var botMenu by remember { mutableStateOf(false) }
    var projectMenu by remember { mutableStateOf(false) }
    val selectedBot = bots.firstOrNull { it.str("id") == botId }
    val selectedProject = projects.firstOrNull { it.str("id") == projectId }
    val enabled = routine?.boolean("enabled") == true
    val nextRun = routine?.str("next_run_at").orEmpty().takeIf { it.isNotBlank() }

    Column(Modifier.fillMaxSize().imePadding().background(MaterialTheme.colorScheme.background)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack, modifier = Modifier.semantics { contentDescription = backLabel }) { Text("‹", style = MaterialTheme.typography.headlineSmall) }
            Text(if (creating) stringResource(Res.string.routines_new) else routine?.str("name").orEmpty(), Modifier.weight(1f), maxLines = 1, style = MaterialTheme.typography.titleLarge)
            if (!creating && !editing) IconButton(onClick = onEdit) { Text(stringResource(Res.string.routines_edit), style = MaterialTheme.typography.labelLarge) }
        }
        HorizontalDivider()
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            actionError?.let { Text(stringResource(Res.string.routines_error, it), color = MaterialTheme.colorScheme.error) }
            if (!creating && routine != null) {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(routine.str("name"), style = MaterialTheme.typography.titleMedium)
                        nextRun?.let { Text(it.routineDisplayTime(), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                    }
                    Text(if (enabled) stringResource(Res.string.routines_pause) else stringResource(Res.string.routines_resume), style = MaterialTheme.typography.labelMedium)
                    Switch(checked = enabled, onCheckedChange = onToggle)
                }
                if (editing) {
                    OutlinedTextField(name, { name = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_name)) }, singleLine = true)
                }
            } else {
                OutlinedTextField(name, { name = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_name)) }, singleLine = true)
            }

            RoutineSectionTitle(stringResource(Res.string.routines_bot))
            Box {
                Row(
                    Modifier.fillMaxWidth().clickable(enabled = creating || editing) { botMenu = true }.padding(vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Box(Modifier.size(40.dp)) { BotAvatar(repository, selectedBot, main = selectedBot?.boolean("is_main") == true) }
                    Column(Modifier.weight(1f)) {
                        Text(selectedBot?.str("name").orEmpty().ifBlank { stringResource(if (creating) Res.string.routines_select_bot else Res.string.routines_no_bots) }, style = MaterialTheme.typography.bodyLarge)
                        if (!creating && !editing) Text(stringResource(Res.string.feature_readonly), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    if (creating || editing) Text("⌄", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                DropdownMenu(expanded = botMenu, onDismissRequest = { botMenu = false }) {
                    bots.forEach { bot ->
                        DropdownMenuItem(text = { Text(bot.str("name")) }, onClick = { botId = bot.str("id"); botMenu = false })
                    }
                }
            }

            RoutineSectionTitle(stringResource(Res.string.routines_when))
            schedules.forEachIndexed { index, schedule ->
                if (editing) {
                    OutlinedTextField(
                        value = schedule.label,
                        onValueChange = { value -> schedules = schedules.replaceAt(index) { copy(label = value) } },
                        modifier = Modifier.fillMaxWidth(),
                        label = { Text(stringResource(Res.string.routines_schedule_label)) },
                        singleLine = true,
                    )
                    OutlinedTextField(
                        value = schedule.cron,
                        onValueChange = { value -> schedules = schedules.replaceAt(index) { copy(cron = value) } },
                        modifier = Modifier.fillMaxWidth(),
                        label = { Text(stringResource(Res.string.routines_schedule_cron)) },
                        singleLine = true,
                    )
                    if (schedules.size > 1) TextButton(onClick = { schedules = schedules.filterIndexed { itemIndex, _ -> itemIndex != index } }) { Text(stringResource(Res.string.routines_delete)) }
                } else {
                    Card(Modifier.fillMaxWidth()) {
                        Row(Modifier.fillMaxWidth().padding(14.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            Text("◷", color = MaterialTheme.colorScheme.primary)
                            Column {
                                Text(schedule.label.ifBlank { schedule.cron }, style = MaterialTheme.typography.bodyLarge)
                                if (schedule.label.isNotBlank() && schedule.cron.isNotBlank()) Text(schedule.cron, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    }
                }
            }
            if (editing) TextButton(onClick = { schedules = schedules + RoutineScheduleDraft() }) { Text("＋ ${stringResource(Res.string.routines_add_time)}") }
            if (editing) {
                OutlinedTextField(timezone, { timezone = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.routines_timezone)) }, singleLine = true)
            }

            RoutineSectionTitle(stringResource(Res.string.routines_deliver_to))
            Box {
                Row(
                    Modifier.fillMaxWidth().clickable(enabled = editing) { projectMenu = true }.padding(vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Text("◎", color = MaterialTheme.colorScheme.primary)
                    Column(Modifier.weight(1f)) {
                        Text(selectedProject?.str("name").orEmpty().ifBlank { stringResource(Res.string.routines_dm) }, style = MaterialTheme.typography.bodyLarge)
                        Text(selectedBot?.str("name").orEmpty(), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    if (editing) Text("⌄", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                DropdownMenu(expanded = projectMenu, onDismissRequest = { projectMenu = false }) {
                    DropdownMenuItem(text = { Text(stringResource(Res.string.routines_dm)) }, onClick = { projectId = null; projectMenu = false })
                    projects.forEach { project ->
                        DropdownMenuItem(text = { Text(project.str("name")) }, onClick = { projectId = project.str("id"); projectMenu = false })
                    }
                }
            }
            if (editing && projects.isEmpty()) Text(stringResource(Res.string.routines_no_groups), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)

            RoutineSectionTitle(stringResource(Res.string.routines_instruction))
            if (editing) {
                OutlinedTextField(instructions, { instructions = it }, Modifier.fillMaxWidth(), minLines = 4, label = { Text(stringResource(Res.string.routines_instruction)) })
            } else {
                Card(Modifier.fillMaxWidth()) { Text(instructions.ifBlank { "—" }, Modifier.padding(14.dp), style = MaterialTheme.typography.bodyMedium) }
            }

            if (!creating) {
                RoutineSectionTitle(stringResource(Res.string.routines_history))
                if (history.isEmpty()) Text(stringResource(Res.string.feature_no_records), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                history.forEach { run ->
                    RoutineRunRow(run, onOpenAssignment)
                }
            }

            if (editing) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    TextButton(onClick = onBack, modifier = Modifier.weight(1f)) { Text(stringResource(Res.string.common_cancel)) }
                    Button(
                        onClick = {
                            val cleanSchedules = schedules.filter { it.label.isNotBlank() && it.cron.isNotBlank() }
                            onSave(botId, name.trim(), instructions.trim(), cleanSchedules, timezone.trim(), projectId)
                        },
                        enabled = botId.isNotBlank() && name.isNotBlank() && instructions.isNotBlank() && schedules.any { it.label.isNotBlank() && it.cron.isNotBlank() } && timezone.isNotBlank(),
                        modifier = Modifier.weight(1f),
                    ) { Text(stringResource(Res.string.common_save)) }
                }
            } else {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = onTry, modifier = Modifier.weight(1f)) { Text(stringResource(Res.string.routines_try)) }
                    TextButton(onClick = onDelete, modifier = Modifier.weight(1f)) { Text(stringResource(Res.string.routines_delete), color = MaterialTheme.colorScheme.error) }
                }
            }
            Spacer(Modifier.height(8.dp))
        }
    }
}

@Composable
private fun RoutineSectionTitle(text: String) {
    Text(text, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary)
}

@Composable
private fun RoutineRunRow(run: JsonObject, onOpenAssignment: (String) -> Unit) {
    val status = run.str("status")
    val statusLabel = when (status.lowercase()) {
        "running" -> stringResource(Res.string.routines_status_running)
        "done" -> stringResource(Res.string.routines_status_done)
        "failed" -> stringResource(Res.string.routines_status_failed)
        "skipped" -> stringResource(Res.string.routines_status_skipped)
        else -> status
    }
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.fillMaxWidth().padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text(if (status == "done") "✓" else if (status == "failed") "!" else "◷", color = if (status == "failed") MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary)
                Text(statusLabel, Modifier.weight(1f).padding(start = 8.dp), style = MaterialTheme.typography.bodyMedium)
                Text(run.str("started_at").routineDisplayTime(), style = MaterialTheme.typography.labelSmall)
            }
            run.str("error").takeIf { it.isNotBlank() }?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
            run.str("assignment_id").takeIf { it.isNotBlank() }?.let { assignmentId ->
                TextButton(onClick = { onOpenAssignment(assignmentId) }) { Text(stringResource(Res.string.routines_details)) }
            }
        }
    }
}

private fun List<RoutineScheduleDraft>.replaceAt(index: Int, transform: RoutineScheduleDraft.() -> RoutineScheduleDraft): List<RoutineScheduleDraft> =
    mapIndexed { itemIndex, schedule -> if (itemIndex == index) schedule.transform() else schedule }

private fun String.routineDisplayTime(): String = replace('T', ' ').replace(Regex("\\.\\d+Z?$"), "").removeSuffix("Z").take(16)

private fun routineCreateParams(botId: String, name: String, instructions: String, schedules: List<RoutineScheduleDraft>, timezone: String, projectId: String?): JsonObject =
    buildJsonObject {
        put("bot_id", botId)
        put("name", name)
        put("instructions", instructions)
        put("schedules", schedules.toScheduleJson())
        put("timezone", timezone)
        if (projectId != null) put("project_id", projectId)
    }

private fun routineUpdateParams(id: String, botId: String, name: String, instructions: String, schedules: List<RoutineScheduleDraft>, timezone: String, projectId: String?): JsonObject =
    buildJsonObject {
        put("routine_id", id)
        put("patch", buildJsonObject {
            put("bot_id", botId)
            put("name", name)
            put("instructions", instructions)
            put("schedules", schedules.toScheduleJson())
            put("timezone", timezone)
            put("project_id", projectId?.let(::JsonPrimitive) ?: JsonNull)
        })
    }

private fun List<RoutineScheduleDraft>.toScheduleJson(): JsonArray = buildJsonArray {
    forEach { schedule -> add(buildJsonObject { put("cron", schedule.cron); put("label", schedule.label) }) }
}
