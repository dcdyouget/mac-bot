package bot.mac.mobile.feature.approval

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.ui.MarkdownText
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

@Composable
fun ApprovalScreen(repository: MobileRepository, onBack: () -> Unit = {}) {
    val state by repository.state.collectAsState()
    val scope = rememberCoroutineScope()
    var error by remember { mutableStateOf<String?>(null) }
    val pendingApprovals = state.approvals.filter { it.str("state") == "pending" }
    val pendingQuestions = state.questions.filter { it.str("state") == "pending" }
    LaunchedEffect(Unit) { runCatching { repository.call("approval.list", buildJsonObject { put("state", kotlinx.serialization.json.buildJsonArray { add(JsonPrimitive("pending")) }) }) }.onFailure { error = it.message } }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(16.dp)) {
        Row(Modifier.fillMaxWidth()) { Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }; Text(stringResource(Res.string.feature_approval), Modifier.padding(start = 12.dp), style = MaterialTheme.typography.titleLarge) }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(12.dp), modifier = Modifier.fillMaxSize().padding(top = 12.dp)) {
            items(pendingApprovals, key = { it.str("id").ifBlank { "approval:${it.hashCode()}" } }) { approval ->
                ApprovalCard(approval) { id, decision ->
                    scope.launch {
                        runCatching { repository.call("approval.decide", buildJsonObject { put("approval_id", id); put("decision", decision) }) }
                            .onFailure { error = it.message }
                    }
                }
            }
            items(pendingQuestions, key = { it.str("id").ifBlank { "question:${it.hashCode()}" } }) { question ->
                QuestionCard(question) { id, optionIndex, text ->
                    scope.launch {
                        runCatching {
                            repository.call("question.answer", buildJsonObject {
                                put("question_id", id)
                                optionIndex?.let { put("option_index", it) }
                                text?.let { put("text", it) }
                            })
                        }.onFailure { error = it.message }
                    }
                }
            }
            if (pendingApprovals.isEmpty() && pendingQuestions.isEmpty()) item { Text(stringResource(Res.string.feature_no_approvals), Modifier.padding(24.dp)) }
        }
    }
}

@Composable
fun ApprovalCard(approval: JsonObject, onDecide: (approvalId: String, decision: String) -> Unit) {
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.errorContainer).padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        val tool = approval.str("tool")
        val toolLabel = if (tool.isBlank()) stringResource(Res.string.feature_tool) else tool
        Text("${stringResource(Res.string.feature_approval_needed)} · $toolLabel", style = MaterialTheme.typography.titleMedium)
        Text(approval.str("summary"))
        MarkdownText(approval.str("detail"), Modifier.fillMaxWidth())
        val id = approval.str("id")
        if (approval.str("state") == "pending") {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { onDecide(id, "allow_once") }) { Text(stringResource(Res.string.feature_allow_once)) }
                Button(onClick = { onDecide(id, "always_allow") }) { Text(stringResource(Res.string.feature_always_allow)) }
                Button(onClick = { onDecide(id, "deny") }) { Text(stringResource(Res.string.feature_deny)) }
            }
        }
    }
}

@Composable
fun QuestionCard(question: JsonObject, onAnswer: (questionId: String, optionIndex: Int?, text: String?) -> Unit) {
    var freeText by remember(question.str("id")) { mutableStateOf("") }
    Column(Modifier.fillMaxWidth().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("？${question.str("text")}", style = MaterialTheme.typography.titleMedium)
        question["options"].let { element ->
            (element as? kotlinx.serialization.json.JsonArray)?.forEachIndexed { index, option ->
                Button(onClick = { onAnswer(question.str("id"), index, null) }) { Text(option.toString().trim('"')) }
            }
        }
        if (question["allow_free_text"]?.toString() == "true") {
            OutlinedTextField(freeText, { freeText = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(Res.string.feature_answer)) })
            Button(onClick = { onAnswer(question.str("id"), null, freeText) }) { Text(stringResource(Res.string.feature_submit)) }
        }
    }
}

