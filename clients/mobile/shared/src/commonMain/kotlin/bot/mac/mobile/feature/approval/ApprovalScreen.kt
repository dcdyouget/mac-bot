package bot.mac.mobile.feature.approval

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.BorderStroke
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.FontScaleToolbar
import bot.mac.mobile.core.network.NetworkError
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.generated.Approval as ApprovalWireModel
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.ui.MarkdownText
import bot.mac.mobile.resources.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun ApprovalScreen(repository: MobileRepository, onBack: () -> Unit = {}, focusedApprovalId: String? = null) {
    val state by repository.state.collectAsState()
    val scope = rememberCoroutineScope()
    var error by remember { mutableStateOf<String?>(null) }
    val pendingApprovals = state.approvals.filter { it.str("state") == "pending" }
    val pendingQuestions = state.questions.filter { it.str("state") == "pending" }
    val focusedApproval = pendingApprovals.firstOrNull { it.str("id") == focusedApprovalId }
    LaunchedEffect(Unit) {
        try {
            repository.call("approval.list", buildJsonObject { put("state", kotlinx.serialization.json.buildJsonArray { add(JsonPrimitive("pending")) }) })
            repository.call("workbench.get", buildJsonObject {})
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Throwable) {
            error = failure.message
        }
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(16.dp)) {
        FontScaleToolbar { Row(Modifier.fillMaxWidth()) { Button(onClick = onBack) { Text(stringResource(Res.string.common_back)) }; Text(stringResource(Res.string.status_need_approval), Modifier.padding(start = 12.dp), style = MaterialTheme.typography.titleLarge) } }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(12.dp), modifier = Modifier.fillMaxSize().padding(top = 12.dp)) {
            items(pendingApprovals, key = { it.str("id").ifBlank { "approval:${it.hashCode()}" } }) { approval ->
                ApprovalCard(approval, approvalOriginName(approval, state)) { id, decision ->
                    scope.launch {
                        runCatching { repository.call("approval.decide", buildJsonObject { put("approval_id", id); put("decision", decision) }) }
                            .onFailure { error = it.message }
                    }
                }
            }
            items(pendingQuestions, key = { it.str("id").ifBlank { "question:${it.hashCode()}" } }) { question ->
                QuestionCard(
                    question = question,
                    onAnswer = { id, optionIndex, optionIndexes, text ->
                        try {
                            repository.call("question.answer", buildJsonObject {
                                put("question_id", id)
                                optionIndex?.let { put("option_index", it) }
                                optionIndexes?.let { indexes -> put("option_indexes", kotlinx.serialization.json.buildJsonArray { indexes.forEach { add(JsonPrimitive(it)) } }) }
                                text?.let { put("text", it) }
                            })
                        } catch (cancelled: CancellationException) {
                            throw cancelled
                        } catch (failure: Throwable) {
                            // An approval-sheet question can expire while this screen is open.
                            // Refresh its authoritative state so the card stops accepting answers.
                            if ((failure as? NetworkError)?.code == "conflict") {
                                val assignmentId = question.str("assignment_id")
                                val chatId = question.str("chat_id")
                                try {
                                    if (assignmentId.isNotBlank()) repository.call("question.list", buildJsonObject { put("assignment_id", assignmentId) })
                                    else if (chatId.isNotBlank()) repository.call("question.list", buildJsonObject { put("chat_id", chatId) })
                                } catch (cancelled: CancellationException) {
                                    throw cancelled
                                } catch (_: Throwable) {
                                    // Preserve the original answer error if refresh fails.
                                }
                            }
                            throw failure
                        }
                    },
                    onFailure = { error = it.message },
                )
            }
            if (pendingApprovals.isEmpty() && pendingQuestions.isEmpty()) item { Text(stringResource(Res.string.feature_no_approvals), Modifier.padding(24.dp)) }
        }
        focusedApproval?.let { approval ->
            ModalBottomSheet(onDismissRequest = onBack) {
                ApprovalCard(approval, approvalOriginName(approval, state)) { id, decision ->
                    scope.launch {
                        runCatching { repository.call("approval.decide", buildJsonObject { put("approval_id", id); put("decision", decision) }) }
                            .onFailure { error = it.message }
                    }
                }
                Spacer(Modifier.height(24.dp))
            }
        }
    }
}

@Composable
fun ApprovalCard(approval: JsonObject, sourceName: String = "", onDecide: (approvalId: String, decision: String) -> Unit) {
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.errorContainer).padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        val tool = approval.str("tool")
        val toolLabel = if (tool.isBlank()) stringResource(Res.string.trace_filter_tools) else tool
        Text(stringResource(Res.string.block_approval_title, toolLabel), style = MaterialTheme.typography.titleMedium)
        Text(approval.str("summary"))
        MarkdownText(approval.str("detail"), Modifier.fillMaxWidth())
        ApprovalWireModel(approval).matchedRuleText?.takeIf(String::isNotBlank)?.let { matchedRuleText ->
            Text(
                stringResource(Res.string.block_approval_meta, sourceName.ifBlank { stringResource(Res.string.feature_bot) }, matchedRuleText),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        val id = approval.str("id")
        if (approval.str("state") == "pending") {
            val alwaysAllowSupported = ApprovalWireModel(approval).alwaysAllowSupported ?: true
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { onDecide(id, "allow_once") }) { Text(stringResource(Res.string.block_approval_allow_once)) }
                if (alwaysAllowSupported) Button(onClick = { onDecide(id, "always_allow") }) { Text(stringResource(Res.string.block_approval_always_allow)) }
                Button(onClick = { onDecide(id, "deny") }) { Text(stringResource(Res.string.block_approval_deny)) }
            }
        }
    }
}

private fun approvalOriginName(approval: JsonObject, state: bot.mac.mobile.core.state.MobileState): String {
    val assignmentId = ApprovalWireModel(approval).assignmentId.orEmpty()
    val projectId = state.assignments.firstOrNull { it.str("id") == assignmentId }?.str("project_id").orEmpty()
    return state.projects.firstOrNull { it.str("id") == projectId }?.str("name")?.takeIf(String::isNotBlank)
        ?: state.bots.firstOrNull { it.str("id") == ApprovalWireModel(approval).botId }?.str("name")?.takeIf(String::isNotBlank)
        .orEmpty()
}

@Composable
fun QuestionCard(
    question: JsonObject,
    onAnswer: suspend (questionId: String, optionIndex: Int?, optionIndexes: List<Int>?, text: String?) -> Unit,
    onFailure: (Throwable) -> Unit = {},
) {
    var freeText by remember(question.str("id")) { mutableStateOf("") }
    var selectedIndexes by remember(question.str("id")) { mutableStateOf(emptySet<Int>()) }
    var submitting by remember(question.str("id")) { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val id = question.str("id")
    val pending = question.str("state") == "pending"
    fun answer(optionIndex: Int? = null, optionIndexes: List<Int>? = null, text: String? = null) {
        if (submitting || !pending) return
        submitting = true
        scope.launch {
            try {
                onAnswer(id, optionIndex, optionIndexes, text)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                onFailure(failure)
            } finally {
                submitting = false
            }
        }
    }
    Column(Modifier.fillMaxWidth().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("？${question.str("text")}", style = MaterialTheme.typography.titleMedium)
        val multi = question.boolean("multi")
        val state = question.str("state")
        val options = question.arr("options").map { it.toString().trim('"') }
        when (state) {
            "expired" -> {
                options.forEachIndexed { index, option ->
                    Text("${'A' + index}. $option", color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = .4f))
                }
                Text(stringResource(Res.string.block_question_expired), color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            "answered" -> {
                val answer = question.obj("answer")
                val indexes = answer.arr("option_indexes").mapNotNull { it.toString().trim('"').toIntOrNull() }
                val selected = indexes.ifEmpty { answer["option_index"]?.toString()?.trim('"')?.toIntOrNull()?.let(::listOf).orEmpty() }
                Text(
                    if (selected.isNotEmpty()) stringResource(Res.string.block_question_you_picked, selected.joinToString(", ") { ('A' + it).toString() })
                    else stringResource(Res.string.block_question_you_replied),
                    color = MaterialTheme.colorScheme.primary,
                )
                answer.str("text").takeIf(String::isNotBlank)?.let { Text(it) }
            }
            "pending" -> options.forEachIndexed { index, option ->
                if (multi) {
                    val selected = index in selectedIndexes
                    Surface(
                        modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
                        color = if (selected) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surface,
                        shape = RoundedCornerShape(10.dp),
                        border = BorderStroke(1.dp, if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant),
                        enabled = !submitting,
                        onClick = { selectedIndexes = if (selected) selectedIndexes - index else selectedIndexes + index },
                    ) {
                        Row(Modifier.padding(horizontal = 12.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            Text(if (selected) "✓" else "□", color = MaterialTheme.colorScheme.primary)
                            Text(option)
                        }
                    }
                } else Button(enabled = !submitting, onClick = { answer(optionIndex = index) }) { Text(option) }
            }
        }
        if (state == "pending" && multi && selectedIndexes.isNotEmpty()) {
            Button(enabled = !submitting, onClick = { answer(optionIndexes = selectedIndexes.sorted()) }) { Text(stringResource(Res.string.common_submit)) }
        }
        if (state == "pending" && question["allow_free_text"]?.toString() == "true") {
            OutlinedTextField(freeText, { freeText = it }, Modifier.fillMaxWidth(), enabled = !submitting, label = { Text(stringResource(Res.string.feature_answer)) })
            Button(onClick = { answer(text = freeText) }, enabled = !submitting && freeText.isNotBlank()) { Text(stringResource(Res.string.common_submit)) }
        }
    }
}

