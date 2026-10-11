package bot.mac.mobile.feature.connect

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.BotAvatar
import bot.mac.mobile.core.design.FontScaleToolbar
import bot.mac.mobile.core.network.ConnectionStatus
import bot.mac.mobile.core.protocol.objects
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.ClientRepository
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.common_back
import bot.mac.mobile.resources.onboarding_create_failed
import bot.mac.mobile.resources.onboarding_empty_templates
import bot.mac.mobile.resources.onboarding_footer
import bot.mac.mobile.resources.onboarding_host_changed
import bot.mac.mobile.resources.onboarding_meet_main
import bot.mac.mobile.resources.onboarding_meet_main_desc
import bot.mac.mobile.resources.onboarding_retry
import bot.mac.mobile.resources.onboarding_template_loading
import bot.mac.mobile.resources.onboarding_template_fetch_failed
import bot.mac.mobile.resources.onboarding_template_only_main
import bot.mac.mobile.resources.host_connection_error
import bot.mac.mobile.resources.host_setup
import bot.mac.mobile.resources.host_unauthorized
import bot.mac.mobile.resources.host_version
import bot.mac.mobile.resources.onboarding_edit_connection
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

@Composable
fun OnboardingTemplatesScreen(
    repository: ClientRepository,
    onboardingHostId: String,
    onFinished: (String) -> Unit,
    onBack: () -> Unit,
) {
    val activeHost by repository.activeHost.collectAsState()
    val statuses by repository.hostStatuses.collectAsState()
    val status = statuses[onboardingHostId]?.status ?: ConnectionStatus.STOPPED
    val connectionError = statuses[onboardingHostId]?.error?.message
    var templates by remember(onboardingHostId) { mutableStateOf(emptyList<JsonObject>()) }
    var loading by remember(onboardingHostId) { mutableStateOf(false) }
    var fetchErrorDetails by remember(onboardingHostId) { mutableStateOf<String?>(null) }
    var applyError by remember(onboardingHostId) { mutableStateOf<String?>(null) }
    var applyErrorTemplateId by remember(onboardingHostId) { mutableStateOf<String?>(null) }
    var applying by remember(onboardingHostId) { mutableStateOf(false) }
    var retry by remember(onboardingHostId) { mutableStateOf(0) }
    val scope = rememberCoroutineScope()
    val footer = stringResource(Res.string.onboarding_footer)
    val hostChanged = activeHost?.id != onboardingHostId

    LaunchedEffect(onboardingHostId, activeHost?.id, status, retry) {
        if (!isCurrentOnboardingHost(onboardingHostId, repository.activeHost.value?.id) || status != ConnectionStatus.CONNECTED) {
            loading = false
            return@LaunchedEffect
        }
        loading = true
        fetchErrorDetails = null
        try {
            if (!isCurrentOnboardingHost(onboardingHostId, repository.activeHost.value?.id)) return@LaunchedEffect
            val result = repository.callOnHost(onboardingHostId, "bot.templates", buildJsonObject {})
            if (isCurrentOnboardingHost(onboardingHostId, repository.activeHost.value?.id)) {
                templates = result.objects("templates").filter { it.str("id").isNotBlank() }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Throwable) {
            if (isCurrentOnboardingHost(onboardingHostId, repository.activeHost.value?.id)) {
                fetchErrorDetails = failure.message.orEmpty()
            }
        } finally {
            if (isCurrentOnboardingHost(onboardingHostId, repository.activeHost.value?.id)) loading = false
        }
    }

    fun finishIfCurrent() {
        if (canUseOnboardingHost(onboardingHostId, repository.activeHost.value?.id, repository.hostStatuses.value[onboardingHostId]?.status ?: ConnectionStatus.STOPPED)) {
            onFinished(onboardingHostId)
        }
    }

    fun applyTemplate(templateId: String) {
        if (!canUseOnboardingHost(onboardingHostId, repository.activeHost.value?.id, repository.hostStatuses.value[onboardingHostId]?.status ?: ConnectionStatus.STOPPED) || applying) return
        applying = true
        applyError = null
        applyErrorTemplateId = templateId
        scope.launch {
            try {
                repository.callOnHost(onboardingHostId, "bot.create_from_template", buildJsonObject { put("template_id", templateId) })
                if (isCurrentOnboardingHost(onboardingHostId, repository.activeHost.value?.id)) finishIfCurrent()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                if (isCurrentOnboardingHost(onboardingHostId, repository.activeHost.value?.id)) applyError = failure.message.orEmpty()
            } finally {
                applying = false
            }
        }
    }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(horizontal = 16.dp)) {
        FontScaleToolbar {
            Row(Modifier.fillMaxWidth().height(56.dp), verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = onBack) { Text(stringResource(Res.string.common_back)) }
            }
        }
        BotAvatar(repository, bot = null, main = true, size = 64.dp)
        Text(stringResource(Res.string.onboarding_meet_main), Modifier.padding(top = 14.dp), style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
        Text(stringResource(Res.string.onboarding_meet_main_desc), Modifier.padding(top = 6.dp, bottom = 12.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)

        when {
            hostChanged -> Text(stringResource(Res.string.onboarding_host_changed), color = MaterialTheme.colorScheme.error)
            status != ConnectionStatus.CONNECTED -> {
                if (connectionError.isNullOrBlank()) {
                    Text(stringResource(Res.string.onboarding_template_loading), color = MaterialTheme.colorScheme.onSurfaceVariant)
                } else {
                    val message = when {
                        connectionError.contains("401") -> Res.string.host_unauthorized
                        connectionError.contains("403") -> Res.string.host_setup
                        connectionError.contains("protocol", ignoreCase = true) -> Res.string.host_version
                        else -> Res.string.host_connection_error
                    }
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(
                            if (message == Res.string.host_connection_error) stringResource(message, connectionError) else stringResource(message),
                            color = MaterialTheme.colorScheme.error,
                        )
                        OutlinedButton(onClick = onBack) { Text(stringResource(Res.string.onboarding_edit_connection)) }
                    }
                }
            }
            loading -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                Text(stringResource(Res.string.onboarding_template_loading), color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            fetchErrorDetails != null -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(Res.string.onboarding_template_fetch_failed), color = MaterialTheme.colorScheme.error)
                OutlinedButton(onClick = { retry++ }) { Text(stringResource(Res.string.onboarding_retry)) }
            }
            else -> LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (templates.isEmpty()) item { Text(stringResource(Res.string.onboarding_empty_templates), color = MaterialTheme.colorScheme.onSurfaceVariant) }
                items(templates, key = { it.str("id") }) { template ->
                    val templateId = template.str("id")
                    val members = template.objects("bots").map { it.str("name") }.filter(String::isNotBlank)
                    Card(
                        onClick = {
                            applyTemplate(templateId)
                        },
                        enabled = !applying && !hostChanged && status == ConnectionStatus.CONNECTED,
                        modifier = Modifier.fillMaxWidth().height(72.dp),
                        shape = RoundedCornerShape(16.dp),
                        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHigh),
                    ) {
                        Row(Modifier.fillMaxSize().padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            Row(horizontalArrangement = Arrangement.spacedBy((-8).dp)) {
                                members.take(3).forEach { member ->
                                    Box(Modifier.size(34.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primaryContainer), contentAlignment = Alignment.Center) {
                                        Text(member.take(1), color = MaterialTheme.colorScheme.onPrimaryContainer, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)
                                    }
                                }
                            }
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text(template.str("name"), style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                Text(members.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                }
                if (applyError != null) item {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(stringResource(Res.string.onboarding_create_failed, applyError.orEmpty()), color = MaterialTheme.colorScheme.error)
                        OutlinedButton(enabled = !applying && !hostChanged && status == ConnectionStatus.CONNECTED, onClick = { applyErrorTemplateId?.let(::applyTemplate) }) {
                            Text(stringResource(Res.string.onboarding_retry))
                        }
                    }
                }
            }
        }

        Spacer(Modifier.height(12.dp))
        Text(footer, Modifier.padding(bottom = 10.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Card(
            onClick = { finishIfCurrent() },
            enabled = !hostChanged && status == ConnectionStatus.CONNECTED && !applying,
            modifier = Modifier.fillMaxWidth().height(72.dp).padding(bottom = 12.dp),
            shape = RoundedCornerShape(16.dp),
            colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHigh),
        ) {
            Row(Modifier.fillMaxSize().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("+", color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.headlineSmall)
                Text(stringResource(Res.string.onboarding_template_only_main), style = MaterialTheme.typography.titleSmall)
            }
        }
    }
}
