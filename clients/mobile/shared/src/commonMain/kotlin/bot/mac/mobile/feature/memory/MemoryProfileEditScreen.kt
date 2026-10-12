package bot.mac.mobile.feature.memory

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.FontScaleToolbar
import bot.mac.mobile.core.design.LocalMobilePalette
import bot.mac.mobile.core.network.NetworkError
import bot.mac.mobile.core.platform.PlatformBackHandler
import bot.mac.mobile.core.protocol.*
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.json.JsonObject
import org.jetbrains.compose.resources.stringResource

@Composable
fun MemoryProfileEditScreen(repository: MobileRepository, field: String, onBack: () -> Unit) {
    val hostId = repository.activeHost.collectAsState().value?.id.orEmpty()
    val state by repository.state.collectAsState()
    val scope = rememberCoroutineScope()
    val validField = isProfileField(field)
    val maxLength = profileFieldMaxLength(field)
    var editState by remember(hostId, field) { mutableStateOf(MemoryProfileEditState(JsonObject(emptyMap()))) }
    var profileLoaded by remember(hostId, field) { mutableStateOf(false) }
    var requestInFlight by remember(hostId, field) { mutableStateOf(false) }
    var saving by remember(hostId, field) { mutableStateOf(false) }
    var saved by remember(hostId, field) { mutableStateOf(false) }
    var error by remember(hostId, field) { mutableStateOf<String?>(null) }
    var expandedLanguage by remember { mutableStateOf(false) }
    val saveMutex = remember(hostId, field) { Mutex() }
    val loadingError = stringResource(Res.string.feature_loading_error)
    val palette = LocalMobilePalette.current
    val profile = state.userProfile
    val value = editState.drafts[field].orEmpty()
    val conflict = editState.conflict

    LaunchedEffect(hostId, field) {
        profileLoaded = false
        if (!validField || hostId.isBlank()) {
            error = loadingError
        } else {
            runCatching { repository.callOnHost(hostId, "memory.profile.get").obj("profile") }
                .onSuccess { editState = MemoryProfileEditState(it); profileLoaded = true }
                .onFailure { error = loadingError }
        }
    }
    LaunchedEffect(profile.long("version"), profileLoaded, requestInFlight) {
        if (profileLoaded && profile.isNotEmpty()) editState = editState.receiveRemote(profile, requestInFlight)
    }
    LaunchedEffect(saved) {
        if (saved) {
            delay(450)
            onBack()
        }
    }
    fun save(submitted: String? = null, submissionState: MemoryProfileEditState? = null) {
        if (!profileLoaded || !validField || saving || saved) return
        val submittedValue = submitted ?: value
        val stateAtSubmission = submissionState ?: editState
        scope.launch {
            saveMutex.withLock {
                if (saving || saved || !profileLoaded) return@withLock
                if (submissionState == null && !editState.isCurrentSubmission(field, submittedValue)) return@withLock
                val cleanValue = if (field == "language") submittedValue.takeIf(String::isNotBlank) else submittedValue.trim().takeIf(String::isNotBlank)
                if (cleanValue == stateAtSubmission.base.str(field).takeIf(String::isNotBlank)) {
                    onBack()
                    return@withLock
                }
                saving = true
                requestInFlight = true
                error = null
                runCatching {
                    saveProfileField(repository, hostId, field, cleanValue, stateAtSubmission.expectedVersion)
                }.onSuccess { updatedProfile ->
                    val updatedState = editState.saveSucceeded(updatedProfile, field, submittedValue)
                    editState = updatedState
                    saved = updatedState.drafts[field].orEmpty() == updatedState.base.str(field)
                }.onFailure { failure ->
                    val remote = (failure as? NetworkError)?.details?.obj("details")?.obj("current")
                    if ((failure as? NetworkError)?.code == "conflict" && remote != null) editState = editState.copy(conflict = remote)
                    else error = loadingError
                }
                requestInFlight = false
                saving = false
            }
        }
    }
    fun handleBack() {
        if (profileLoaded && value != editState.base.str(field) && !saved) save(value)
        else onBack()
    }
    val label = stringResource(when (field) {
        "name" -> Res.string.memory_profile_name
        "language" -> Res.string.memory_profile_language
        else -> Res.string.memory_profile_identity
    })
    PlatformBackHandler(enabled = true, onBack = ::handleBack)
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).imePadding()) {
        FontScaleToolbar {
            Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = ::handleBack, enabled = !saving && !saved) { Text(stringResource(Res.string.common_back)) }
                Text(label, Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
                TextButton(onClick = { save() }, enabled = profileLoaded && !saving && !saved) {
                    if (saved) SaveCheck(palette.accent) else Text(stringResource(Res.string.memory_profile_save_action), style = MaterialTheme.typography.titleLarge)
                }
            }
        }
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            conflict?.let { remote ->
                Surface(Modifier.fillMaxWidth(), color = palette.warning.copy(alpha = 0.14f), shape = RoundedCornerShape(10.dp)) {
                    Row(Modifier.padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(stringResource(Res.string.memory_profile_conflict_banner, remote.long("version")), Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = palette.warning)
                        TextButton(onClick = {
                            scope.launch {
                                runCatching { repository.callOnHost(hostId, "memory.profile.get").obj("profile") }
                                    .onSuccess { editState = editState.useRemote(it) }
                                    .onFailure { error = loadingError }
                            }
                        }) { Text(stringResource(Res.string.memory_profile_conflict_reload), color = palette.warning) }
                    }
                }
            }
            if (field == "language") {
                val languageLabel = when (value) {
                    "zh-CN" -> stringResource(Res.string.memory_profile_language_zh)
                    "en-US" -> stringResource(Res.string.memory_profile_language_en)
                    "ja-JP" -> stringResource(Res.string.memory_profile_language_ja)
                    "ko-KR" -> stringResource(Res.string.memory_profile_language_ko)
                    else -> value.ifBlank { stringResource(Res.string.memory_profile_language_system_option) }
                }
                Box {
                    OutlinedButton(onClick = { expandedLanguage = true }, enabled = profileLoaded && !saving && !saved, modifier = Modifier.fillMaxWidth()) {
                        Text(languageLabel, Modifier.weight(1f))
                        Text("⌄", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    DropdownMenu(expanded = expandedLanguage, onDismissRequest = { expandedLanguage = false }) {
                        listOf(
                            null to stringResource(Res.string.memory_profile_language_system_option),
                            "zh-CN" to stringResource(Res.string.memory_profile_language_zh),
                            "en-US" to stringResource(Res.string.memory_profile_language_en),
                            "ja-JP" to stringResource(Res.string.memory_profile_language_ja),
                            "ko-KR" to stringResource(Res.string.memory_profile_language_ko),
                        ).forEach { (tag, optionLabel) ->
                            DropdownMenuItem(text = { Text(optionLabel) }, onClick = {
                                expandedLanguage = false
                                val updatedState = editState.edit("language", tag.orEmpty())
                                editState = updatedState
                                saved = false
                                save(tag.orEmpty(), updatedState)
                            })
                        }
                    }
                }
            } else {
                var hadFocus by rememberSaveable(hostId, field) { mutableStateOf(false) }
                OutlinedTextField(
                    value = value,
                    onValueChange = { text -> if (maxLength != null && text.unicodeScalarCount() <= maxLength) { saved = false; editState = editState.edit(field, text) } },
                    modifier = Modifier.fillMaxWidth().onFocusChanged { focus ->
                        if (focus.isFocused) hadFocus = true
                        else if (hadFocus && profileLoaded) { hadFocus = false; save(value) }
                    },
                    label = { Text(stringResource(if (field == "name") Res.string.memory_profile_name_placeholder else Res.string.memory_profile_identity_placeholder)) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { save(value) }),
                    enabled = profileLoaded && !saving && !saved,
                )
                Text(stringResource(Res.string.memory_profile_character_count, value.unicodeScalarCount(), maxLength ?: 0), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            if (saved) Row(verticalAlignment = Alignment.CenterVertically) { SaveCheck(palette.accent); Text(stringResource(Res.string.memory_profile_save_success), color = palette.accent, style = MaterialTheme.typography.bodySmall) }
            Text(stringResource(Res.string.memory_profile_hint), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        }
    }
}
