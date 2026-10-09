package bot.mac.mobile.feature.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
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
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.state.ClientRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
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
    val loadError = stringResource(Res.string.feature_loading_error)
    LaunchedEffect(Unit) {
        runCatching {
            repository.call("settings.get", buildJsonObject {})
            repository.call("provider.list", buildJsonObject {})
        }.onFailure { error = it.message ?: loadError }
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
                Text(stringResource(Res.string.feature_provider_readonly), style = MaterialTheme.typography.titleMedium)
            }
            items(state.providers, key = { it.str("id").ifBlank { "provider:${it.hashCode()}" } }) { provider ->
                Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant).padding(10.dp)) {
                    Text(provider.str("name"), style = MaterialTheme.typography.titleSmall)
                    Text(provider.str("api_kind"))
                    Text(provider.str("base_url"))
                    Text(if (provider["has_key"]?.toString() == "true") stringResource(Res.string.feature_key_configured) else stringResource(Res.string.feature_key_missing), style = MaterialTheme.typography.labelSmall)
                }
            }
            item {
                Spacer(Modifier.padding(4.dp))
                Button(onClick = { scope.launch { runCatching { repository.refresh() }.onFailure { error = it.message ?: loadError } } }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(Res.string.feature_refresh)) }
            }
        }
    }
}

@Composable
private fun SettingSwitch(label: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.weight(1f))
        Switch(checked = checked, onCheckedChange = onChange)
    }
}

