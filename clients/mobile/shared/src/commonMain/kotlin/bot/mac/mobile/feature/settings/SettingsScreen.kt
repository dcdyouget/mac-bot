package bot.mac.mobile.feature.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.ui.text.input.PasswordVisualTransformation
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
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.state.ClientRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

@Composable
fun SettingsScreen(repository: MobileRepository, onBack: () -> Unit = {}) {
    val state by repository.state.collectAsState()
    val scope = rememberCoroutineScope()
    val clientRepository = repository as? ClientRepository
    var dark by remember { mutableStateOf(clientRepository?.theme?.value == "dark") }
    var notifications by remember { mutableStateOf(clientRepository?.notifications?.value ?: true) }
    var error by remember { mutableStateOf<String?>(null) }
    var editingProvider by remember { mutableStateOf<JsonObject?>(null) }
    var deletingProvider by remember { mutableStateOf<JsonObject?>(null) }
    var editingModel by remember { mutableStateOf<JsonObject?>(null) }
    var deletingModel by remember { mutableStateOf<JsonObject?>(null) }
    var providerName by remember { mutableStateOf("") }
    var providerKind by remember { mutableStateOf("openai-completions") }
    var providerBaseUrl by remember { mutableStateOf("") }
    var providerApiKey by remember { mutableStateOf("") }
    var providerError by remember { mutableStateOf<String?>(null) }
    var providerFeedback by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    var modelProviderId by remember { mutableStateOf("") }
    var modelId by remember { mutableStateOf("") }
    var modelDisplayName by remember { mutableStateOf("") }
    var modelContextWindow by remember { mutableStateOf("128000") }
    var modelMaxOutput by remember { mutableStateOf("8192") }
    var modelVision by remember { mutableStateOf(false) }
    var modelTools by remember { mutableStateOf(false) }
    var modelReasoning by remember { mutableStateOf(false) }
    var modelEnabled by remember { mutableStateOf(true) }
    var modelInputPrice by remember { mutableStateOf("") }
    var modelOutputPrice by remember { mutableStateOf("") }
    var modelCacheReadPrice by remember { mutableStateOf("") }
    var modelCacheWritePrice by remember { mutableStateOf("") }
    var modelError by remember { mutableStateOf<String?>(null) }
    val providerSaveError = stringResource(Res.string.feature_save_failed)
    val loadError = stringResource(Res.string.feature_loading_error)
    val providerTestOkTemplate = stringResource(Res.string.feature_provider_test_ok)
    val providerTestFailedTemplate = stringResource(Res.string.feature_provider_test_failed)
    val modelSaveError = stringResource(Res.string.feature_model_save_failed)
    val modelPriceInvalid = stringResource(Res.string.feature_model_price_invalid)
    LaunchedEffect(Unit) {
        runCatching {
            repository.call("settings.get", buildJsonObject {})
            repository.call("provider.list", buildJsonObject {})
        }.onFailure { error = it.message ?: loadError }
    }
    fun openProvider(provider: JsonObject?) {
        editingProvider = provider ?: buildJsonObject {}
        providerName = provider?.str("name").orEmpty()
        providerKind = provider?.str("api_kind").orEmpty().ifBlank { "openai-completions" }
        providerBaseUrl = provider?.str("base_url").orEmpty()
        providerApiKey = ""
        providerError = null
    }
    fun openModel(model: JsonObject?) {
        editingModel = model ?: buildJsonObject {}
        modelProviderId = model?.str("provider_id").orEmpty().ifBlank { state.providers.firstOrNull()?.str("id").orEmpty() }
        modelId = model?.str("model_id").orEmpty()
        modelDisplayName = model?.str("display_name").orEmpty()
        modelContextWindow = model?.str("context_window").orEmpty().ifBlank { "128000" }
        modelMaxOutput = model?.str("max_output").orEmpty().ifBlank { "8192" }
        modelVision = model?.obj("caps")?.boolean("vision") == true
        modelTools = model?.obj("caps")?.boolean("tools") == true
        modelReasoning = model?.obj("caps")?.boolean("reasoning") == true
        modelEnabled = model?.boolean("enabled") != false
        modelInputPrice = model?.obj("price")?.str("input_per_mtok").orEmpty()
        modelOutputPrice = model?.obj("price")?.str("output_per_mtok").orEmpty()
        modelCacheReadPrice = model?.obj("price")?.str("cache_read_per_mtok").orEmpty()
        modelCacheWritePrice = model?.obj("price")?.str("cache_write_per_mtok").orEmpty()
        modelError = null
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(16.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Button(onClick = onBack) { Text(stringResource(Res.string.feature_back)) }
            Text(stringResource(Res.string.feature_settings), Modifier.weight(1f).padding(start = 12.dp), style = MaterialTheme.typography.titleLarge)
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        LazyColumn(Modifier.weight(1f).fillMaxWidth().padding(top = 12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            item {
                Text(stringResource(Res.string.feature_appearance), style = MaterialTheme.typography.titleMedium)
                SettingSwitch(stringResource(Res.string.feature_dark_mode), dark) {
                    dark = it
                    scope.launch { clientRepository?.setTheme(if (it) "dark" else "light") }
                }
                SettingSwitch(stringResource(Res.string.feature_notifications), notifications) {
                    notifications = it
                    scope.launch { clientRepository?.setNotifications(it) }
                }
            }
            item {
                HorizontalDivider(Modifier.padding(vertical = 8.dp))
                Text(stringResource(Res.string.feature_host), style = MaterialTheme.typography.titleMedium)
                Text("${state.settings.str("host_name")} · ${state.settings.str("timezone")}")
                Text(stringResource(Res.string.feature_currency, state.settings.str("currency")), style = MaterialTheme.typography.bodySmall)
            }
            item {
                HorizontalDivider(Modifier.padding(vertical = 8.dp))
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(Res.string.feature_provider_management), Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
                    Button(onClick = { openProvider(null) }) { Text(stringResource(Res.string.feature_provider_create)) }
                }
            }
            items(state.providers, key = { it.str("id").ifBlank { "provider:${it.hashCode()}" } }) { provider ->
                Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant).padding(10.dp)) {
                    Text(provider.str("name"), style = MaterialTheme.typography.titleSmall)
                    Text(provider.str("api_kind"))
                    Text(provider.str("base_url"))
                    Text(if (provider.boolean("has_key")) stringResource(Res.string.feature_key_configured) else stringResource(Res.string.feature_key_missing), style = MaterialTheme.typography.labelSmall)
                    providerFeedback[provider.str("id")]?.let { Text(it, style = MaterialTheme.typography.labelSmall) }
                    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Button(onClick = { openProvider(provider) }) { Text(stringResource(Res.string.feature_edit)) }
                        Button(onClick = {
                            scope.launch {
                                runCatching { repository.call("provider.test", buildJsonObject { put("provider_id", provider.str("id")) }) }
                                    .onSuccess { result ->
                                        val message = if (result.boolean("ok")) providerTestOkTemplate.replace("%1\$s", result.str("latency_ms"))
                                        else providerTestFailedTemplate.replace("%1\$s", result.str("error"))
                                        providerFeedback = providerFeedback + (provider.str("id") to message)
                                    }
                                    .onFailure { providerFeedback = providerFeedback + (provider.str("id") to (it.message ?: loadError)) }
                            }
                        }) { Text(stringResource(Res.string.feature_provider_test)) }
                        Button(onClick = {
                            scope.launch {
                                runCatching { repository.call("model.refresh", buildJsonObject { put("provider_id", provider.str("id")) }) }
                                    .onFailure { error = it.message ?: loadError }
                            }
                        }) { Text(stringResource(Res.string.feature_model_refresh)) }
                        Button(onClick = { deletingProvider = provider }) { Text(stringResource(Res.string.feature_provider_delete)) }
                    }
                }
            }
            item {
                HorizontalDivider(Modifier.padding(vertical = 8.dp))
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(Res.string.feature_models), Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
                    Button(onClick = { openModel(null) }, enabled = state.providers.isNotEmpty()) { Text(stringResource(Res.string.feature_model_create)) }
                }
            }
            items(state.models, key = { it.str("ref").ifBlank { "model:${it.hashCode()}" } }) { model ->
                Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant).padding(10.dp)) {
                    val providerName = state.providers.firstOrNull { it.str("id") == model.str("provider_id") }?.str("name").orEmpty().ifBlank { model.str("provider_id") }
                    Text(model.str("display_name").ifBlank { model.str("model_id") }, style = MaterialTheme.typography.titleSmall)
                    Text(model.str("ref"))
                    Text(stringResource(Res.string.feature_model_provider_value, providerName), style = MaterialTheme.typography.labelSmall)
                    val visionLabel = stringResource(if (model.obj("caps").boolean("vision")) Res.string.feature_enabled else Res.string.feature_disabled)
                    val toolsLabel = stringResource(if (model.obj("caps").boolean("tools")) Res.string.feature_enabled else Res.string.feature_disabled)
                    val reasoningLabel = stringResource(if (model.obj("caps").boolean("reasoning")) Res.string.feature_enabled else Res.string.feature_disabled)
                    Text(
                        stringResource(
                            Res.string.feature_model_capabilities,
                            visionLabel,
                            toolsLabel,
                            reasoningLabel,
                        ),
                        style = MaterialTheme.typography.labelSmall,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Button(onClick = { openModel(model) }) { Text(stringResource(Res.string.feature_edit)) }
                        Button(onClick = { deletingModel = model }) { Text(stringResource(Res.string.feature_model_delete)) }
                    }
                }
            }
            item {
                Spacer(Modifier.padding(4.dp))
                Button(onClick = { scope.launch { runCatching { repository.refresh() }.onFailure { error = it.message ?: loadError } } }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.feature_refresh)) }
            }
        }
    }
    editingProvider?.let { provider ->
        AlertDialog(
            onDismissRequest = { editingProvider = null },
            title = { Text(stringResource(if (provider.str("id").isBlank()) Res.string.feature_provider_create else Res.string.feature_provider_edit)) },
            text = {
                Column(Modifier.imePadding().heightIn(max = 420.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(providerName, { providerName = it }, label = { Text(stringResource(Res.string.feature_provider_name)) })
                    Text(stringResource(Res.string.feature_provider_api_kind))
                    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        listOf(
                            "openai-completions" to Res.string.feature_provider_kind_openai_completions,
                            "openai-responses" to Res.string.feature_provider_kind_openai_responses,
                            "anthropic-messages" to Res.string.feature_provider_kind_anthropic,
                            "google-generative" to Res.string.feature_provider_kind_google,
                        ).forEach { (kind, label) ->
                            FilterChip(selected = providerKind == kind, onClick = { providerKind = kind }, label = { Text(stringResource(label)) })
                        }
                    }
                    OutlinedTextField(providerBaseUrl, { providerBaseUrl = it }, label = { Text(stringResource(Res.string.feature_provider_base_url)) })
                    OutlinedTextField(providerApiKey, { providerApiKey = it }, visualTransformation = PasswordVisualTransformation(), label = { Text(stringResource(Res.string.feature_provider_api_key)) })
                    providerError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                }
            },
            confirmButton = {
                TextButton(enabled = providerName.isNotBlank() && providerBaseUrl.isNotBlank(), onClick = {
                    scope.launch {
                        val patch = buildJsonObject {
                            put("name", providerName.trim())
                            put("api_kind", providerKind.trim())
                            put("base_url", providerBaseUrl.trim())
                            if (providerApiKey.isNotBlank()) put("api_key", providerApiKey)
                        }
                        runCatching {
                            if (provider.str("id").isBlank()) repository.call("provider.create", patch)
                            else repository.call("provider.update", buildJsonObject {
                                put("provider_id", provider.str("id"))
                                put("patch", buildJsonObject {
                                    put("name", providerName.trim())
                                    put("base_url", providerBaseUrl.trim())
                                    if (providerApiKey.isNotBlank()) put("api_key", providerApiKey)
                                })
                            })
                        }.onSuccess { editingProvider = null }.onFailure { providerError = it.message ?: providerSaveError }
                    }
                }) { Text(stringResource(Res.string.feature_provider_save)) }
            },
            dismissButton = { TextButton(onClick = { editingProvider = null }) { Text(stringResource(Res.string.common_cancel)) } },
        )
    }
    editingModel?.let { model ->
        AlertDialog(
            onDismissRequest = { editingModel = null },
            title = { Text(stringResource(if (model.str("ref").isBlank()) Res.string.feature_model_create else Res.string.feature_model_edit)) },
            text = {
                Column(Modifier.imePadding().heightIn(max = 420.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(stringResource(Res.string.feature_model_provider))
                    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        state.providers.forEach { provider ->
                            FilterChip(
                                selected = modelProviderId == provider.str("id"),
                                onClick = { modelProviderId = provider.str("id") },
                                label = { Text(provider.str("name").ifBlank { provider.str("id") }) },
                            )
                        }
                    }
                    OutlinedTextField(modelId, { modelId = it }, label = { Text(stringResource(Res.string.feature_model_id)) })
                    OutlinedTextField(modelDisplayName, { modelDisplayName = it }, label = { Text(stringResource(Res.string.feature_model_display_name)) })
                    OutlinedTextField(modelContextWindow, { modelContextWindow = it.filter(Char::isDigit) }, label = { Text(stringResource(Res.string.feature_model_context_window)) })
                    OutlinedTextField(modelMaxOutput, { modelMaxOutput = it.filter(Char::isDigit) }, label = { Text(stringResource(Res.string.feature_model_max_output)) })
                    Text(stringResource(Res.string.feature_model_capabilities_title))
                    SettingSwitch(stringResource(Res.string.feature_model_vision), modelVision) { modelVision = it }
                    SettingSwitch(stringResource(Res.string.feature_model_tools), modelTools) { modelTools = it }
                    SettingSwitch(stringResource(Res.string.feature_model_reasoning), modelReasoning) { modelReasoning = it }
                    SettingSwitch(stringResource(Res.string.feature_model_enabled), modelEnabled) { modelEnabled = it }
                    OutlinedTextField(modelInputPrice, { modelInputPrice = it }, label = { Text(stringResource(Res.string.feature_model_input_price)) })
                    OutlinedTextField(modelOutputPrice, { modelOutputPrice = it }, label = { Text(stringResource(Res.string.feature_model_output_price)) })
                    OutlinedTextField(modelCacheReadPrice, { modelCacheReadPrice = it }, label = { Text(stringResource(Res.string.feature_model_cache_read_price)) })
                    OutlinedTextField(modelCacheWritePrice, { modelCacheWritePrice = it }, label = { Text(stringResource(Res.string.feature_model_cache_write_price)) })
                    modelError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                }
            },
            confirmButton = {
                TextButton(enabled = modelProviderId.isNotBlank() && modelId.isNotBlank(), onClick = {
                    val priceInputs = listOf(modelInputPrice, modelOutputPrice, modelCacheReadPrice, modelCacheWritePrice).map { it.trim() }
                    val hasPrice = priceInputs.any(String::isNotBlank)
                    val parsedPrices = priceInputs.map { it.toDoubleOrNull() }
                    val invalidPrice = hasPrice && (priceInputs.any { it.isBlank() } || parsedPrices.any { it == null })
                    if (invalidPrice) {
                        modelError = modelPriceInvalid
                    } else scope.launch {
                        val payload = buildJsonObject {
                            if (model.str("ref").isNotBlank()) put("ref", model.str("ref"))
                            put("provider_id", modelProviderId)
                            put("model_id", modelId.trim())
                            if (modelDisplayName.isNotBlank()) put("display_name", modelDisplayName.trim())
                            modelContextWindow.toIntOrNull()?.let { put("context_window", it) }
                            modelMaxOutput.toIntOrNull()?.let { put("max_output", it) }
                            put("caps", buildJsonObject {
                                put("vision", modelVision)
                                put("tools", modelTools)
                                put("reasoning", modelReasoning)
                            })
                            put("enabled", modelEnabled)
                            if (!hasPrice) {
                                put("price", JsonNull)
                            } else {
                                put("price", buildJsonObject {
                                    put("input_per_mtok", parsedPrices[0]!!)
                                    put("output_per_mtok", parsedPrices[1]!!)
                                    put("cache_read_per_mtok", parsedPrices[2]!!)
                                    put("cache_write_per_mtok", parsedPrices[3]!!)
                                })
                            }
                        }
                        runCatching { repository.call("model.upsert", payload) }
                            .onSuccess { editingModel = null }
                            .onFailure { modelError = it.message ?: modelSaveError }
                    }
                }) { Text(stringResource(Res.string.feature_model_save)) }
            },
            dismissButton = { TextButton(onClick = { editingModel = null }) { Text(stringResource(Res.string.common_cancel)) } },
        )
    }
    deletingModel?.let { model ->
        AlertDialog(
            onDismissRequest = { deletingModel = null },
            title = { Text(stringResource(Res.string.feature_model_delete)) },
            text = { Text(stringResource(Res.string.feature_model_delete_confirm, model.str("display_name").ifBlank { model.str("ref") })) },
            confirmButton = {
                TextButton(onClick = {
                    scope.launch {
                        runCatching { repository.call("model.delete", buildJsonObject { put("ref", model.str("ref")) }) }
                            .onSuccess { deletingModel = null }
                            .onFailure { error = it.message ?: loadError; deletingModel = null }
                    }
                }) { Text(stringResource(Res.string.feature_delete_confirm_action)) }
            },
            dismissButton = { TextButton(onClick = { deletingModel = null }) { Text(stringResource(Res.string.common_cancel)) } },
        )
    }
    deletingProvider?.let { provider ->
        AlertDialog(
            onDismissRequest = { deletingProvider = null },
            title = { Text(stringResource(Res.string.feature_provider_delete)) },
            text = { Text(stringResource(Res.string.feature_provider_delete_confirm, provider.str("name"))) },
            confirmButton = {
                TextButton(onClick = {
                    scope.launch {
                        runCatching { repository.call("provider.delete", buildJsonObject { put("provider_id", provider.str("id")) }) }
                            .onSuccess { deletingProvider = null }
                            .onFailure { error = it.message ?: loadError; deletingProvider = null }
                    }
                }) { Text(stringResource(Res.string.feature_delete_confirm_action)) }
            },
            dismissButton = { TextButton(onClick = { deletingProvider = null }) { Text(stringResource(Res.string.common_cancel)) } },
        )
    }
}

@Composable
private fun SettingSwitch(label: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.weight(1f))
        Switch(checked = checked, onCheckedChange = onChange)
    }
}

