package bot.mac.mobile.feature.routines

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.common_close
import bot.mac.mobile.resources.routines_disabled
import bot.mac.mobile.resources.routines_details
import bot.mac.mobile.resources.routines_empty
import bot.mac.mobile.resources.routines_error
import bot.mac.mobile.resources.routines_enabled
import bot.mac.mobile.resources.routines_finished
import bot.mac.mobile.resources.routines_history
import bot.mac.mobile.resources.routines_last_run
import bot.mac.mobile.resources.routines_next_run
import bot.mac.mobile.resources.routines_pause
import bot.mac.mobile.resources.routines_resume
import bot.mac.mobile.resources.routines_title
import bot.mac.mobile.resources.routines_trigger
import bot.mac.mobile.resources.routines_status_running
import bot.mac.mobile.resources.routines_status_done
import bot.mac.mobile.resources.routines_status_failed
import bot.mac.mobile.resources.routines_status_skipped
import bot.mac.mobile.resources.routines_trigger_schedule
import bot.mac.mobile.resources.routines_trigger_test
import kotlinx.coroutines.launch
import kotlinx.coroutines.CancellationException
import kotlinx.serialization.json.JsonObject
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

@Composable
fun RoutinesScreen(repository: MobileRepository, onOpenAssignment: (String) -> Unit, onBack: () -> Unit) {
    val state by repository.state.collectAsState()
    var historyRoutine by remember { mutableStateOf<String?>(null) }
    var history by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var actionError by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(Unit) {
        routineRequest({ actionError = it }) { repository.call("routine.list") }
    }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Text("‹", style = MaterialTheme.typography.headlineSmall) }
            Text(stringResource(Res.string.routines_title), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
        }
        actionError?.let { Text(stringResource(Res.string.routines_error, it), color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp)) }
        HorizontalDivider()
        LazyColumn(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(state.routines, key = { it.str("id") ?: it.hashCode() }) { routine ->
                RoutineCard(routine, onToggle = { enabled ->
                    routine.str("id")?.let { id -> scope.launch {
                        routineRequest({ actionError = it }) {
                            repository.call("routine.set_enabled", buildJsonObject { put("routine_id", id); put("enabled", enabled) })
                            repository.call("routine.list")
                        }
                    } }
                }, onHistory = {
                    routine.str("id")?.let { id ->
                        historyRoutine = id
                        scope.launch {
                            routineRequest({ actionError = it }) { repository.call("routine.runs", buildJsonObject { put("routine_id", id) }) }
                                ?.let { response -> history = response.arr("runs").orEmpty().mapNotNull { it as? JsonObject } }
                        }
                    }
                }, onOpenAssignment = onOpenAssignment)
            }
            if (state.routines.isEmpty()) item { Text(stringResource(Res.string.routines_empty), Modifier.padding(24.dp)) }
        }
    }
    historyRoutine?.let { id ->
        AlertDialog(onDismissRequest = { historyRoutine = null }, title = { Text(stringResource(Res.string.routines_history)) }, text = {
            LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                items(history, key = { it.str("id") ?: it.hashCode() }) { run ->
                    Column {
                        Text(routineStatusLabel(run.str("status")), style = MaterialTheme.typography.titleSmall)
                        Text(run.str("started_at") ?: "", style = MaterialTheme.typography.labelSmall)
                        run.str("trigger").takeIf { it.isNotBlank() }?.let { Text(stringResource(Res.string.routines_trigger, routineTriggerLabel(it)), style = MaterialTheme.typography.labelSmall) }
                        run.str("finished_at").takeIf { it.isNotBlank() }?.let { Text(stringResource(Res.string.routines_finished, it), style = MaterialTheme.typography.labelSmall) }
                        run.str("assignment_id").takeIf { it.isNotBlank() }?.let { assignmentId ->
                            TextButton(onClick = { onOpenAssignment(assignmentId) }) { Text(stringResource(Res.string.routines_details)) }
                        }
                        run.str("error")?.takeIf { it.isNotBlank() }?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
                    }
                }
                if (history.isEmpty()) item { Text(stringResource(Res.string.routines_empty)) }
            }
        }, confirmButton = { TextButton(onClick = { historyRoutine = null }) { Text(stringResource(Res.string.common_close)) } })
    }
}

@Composable
private fun RoutineCard(routine: JsonObject, onToggle: (Boolean) -> Unit, onHistory: () -> Unit, onOpenAssignment: (String) -> Unit) {
    val enabled = routine.boolean("enabled") == true
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(5.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text(routine.str("name") ?: "", Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
                Button(onClick = { onToggle(toggleRoutineEnabled(enabled)) }) { Text(if (enabled) stringResource(Res.string.routines_pause) else stringResource(Res.string.routines_resume)) }
            }
            Text(if (enabled) stringResource(Res.string.routines_enabled) else stringResource(Res.string.routines_disabled), style = MaterialTheme.typography.labelSmall)
            Text(routine.str("instructions") ?: "", style = MaterialTheme.typography.bodyMedium)
            routine.arr("schedules").forEach { schedule ->
                val item = schedule as? JsonObject
                val scheduleText = item?.str("label").orEmpty().ifBlank { item?.str("cron").orEmpty() }
                if (scheduleText.isNotBlank()) Text("◷ $scheduleText", style = MaterialTheme.typography.bodySmall)
            }
            Text("${stringResource(Res.string.routines_next_run)}: ${routine.str("next_run_at").ifBlank { "—" }}", style = MaterialTheme.typography.labelSmall)
            (routine["last_run"] as? JsonObject)?.let { last ->
                val status = last.str("status")
                val started = last.str("started_at")
                if (status.isNotBlank() || started.isNotBlank()) Text("${stringResource(Res.string.routines_last_run)}: ${routineStatusLabel(status)} $started", style = MaterialTheme.typography.labelSmall)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                TextButton(onClick = onHistory) { Text(stringResource(Res.string.routines_history)) }
                ((routine["last_run"] as? JsonObject)?.str("assignment_id")).orEmpty().takeIf { it.isNotBlank() }?.let { assignmentId -> TextButton(onClick = { onOpenAssignment(assignmentId) }) { Text(stringResource(Res.string.routines_details)) } }
            }
        }
    }
}

internal fun toggleRoutineEnabled(enabled: Boolean): Boolean = !enabled

@Composable
private fun routineStatusLabel(status: String): String = when (status.lowercase()) {
    "running" -> stringResource(Res.string.routines_status_running)
    "done" -> stringResource(Res.string.routines_status_done)
    "failed" -> stringResource(Res.string.routines_status_failed)
    "skipped" -> stringResource(Res.string.routines_status_skipped)
    else -> status
}

@Composable
private fun routineTriggerLabel(trigger: String): String = when (trigger.lowercase()) {
    "schedule" -> stringResource(Res.string.routines_trigger_schedule)
    "test" -> stringResource(Res.string.routines_trigger_test)
    else -> trigger
}
