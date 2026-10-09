package bot.mac.mobile.feature.connect

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.state.ClientRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import org.jetbrains.compose.resources.stringResource

@Composable fun ConnectScreen(repository: ClientRepository, onBack: () -> Unit) {
    val hosts by repository.hosts.collectAsState()
    var name by remember { mutableStateOf("") }
    var addresses by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var editing by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    Column(Modifier.fillMaxSize().padding(16.dp)) {
        Row { TextButton(onClick = onBack) { Text(stringResource(Res.string.common_back)) } }
        Text(stringResource(Res.string.host_welcome), style = MaterialTheme.typography.headlineSmall)
        Text(stringResource(Res.string.host_hint), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(vertical = 12.dp))
        LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            item { OutlinedTextField(name, { name = it }, label = { Text(stringResource(Res.string.host_name)) }, singleLine = true, modifier = Modifier.fillMaxWidth()) }
            item { OutlinedTextField(addresses, { addresses = it }, label = { Text(stringResource(Res.string.host_addresses)) }, supportingText = { Text(stringResource(Res.string.host_address_hint)) }, minLines = 2, modifier = Modifier.fillMaxWidth()) }
            item { OutlinedTextField(password, { password = it }, label = { Text(stringResource(Res.string.host_password)) }, visualTransformation = PasswordVisualTransformation(), singleLine = true, modifier = Modifier.fillMaxWidth()) }
            item {
                error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                Button(enabled = !saving && name.isNotBlank() && addresses.isNotBlank() && password.isNotEmpty(), modifier = Modifier.fillMaxWidth(), onClick = {
                    saving = true
                    scope.launch {
                        runCatching { repository.saveHost(name, addresses.lines().map { it.trim() }.filter { it.isNotEmpty() }, password, editing) }.onSuccess { password = ""; onBack() }.onFailure { error = it.message }
                        saving = false
                    }
                }) { Text(stringResource(Res.string.host_connect)) }
            }
            item { HorizontalDivider(Modifier.padding(vertical = 12.dp)); Text(stringResource(Res.string.host_saved), style = MaterialTheme.typography.titleMedium) }
            items(hosts, key = { it.id }) { host ->
                ElevatedCard(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(12.dp)) {
                        Text(host.name, style = MaterialTheme.typography.titleMedium)
                        Text(host.addresses.joinToString("\n"), color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Row {
                            TextButton(onClick = { scope.launch { runCatching { repository.selectHost(host.id) }.onFailure { error = it.message }; onBack() } }) { Text(stringResource(Res.string.host_switch)) }
                            TextButton(onClick = { editing = host.id; name = host.name; addresses = host.addresses.joinToString("\n"); password = "" }) { Text(stringResource(Res.string.host_edit)) }
                            TextButton(onClick = { scope.launch { runCatching { repository.deleteHost(host.id) }.onFailure { error = it.message } } }) { Text(stringResource(Res.string.host_delete)) }
                        }
                    }
                }
            }
        }
    }
}
