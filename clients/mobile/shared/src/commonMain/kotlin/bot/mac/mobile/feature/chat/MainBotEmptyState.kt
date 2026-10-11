package bot.mac.mobile.feature.chat

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.LocalMobilePalette
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.onboarding_empty_desc
import bot.mac.mobile.resources.onboarding_empty_title
import bot.mac.mobile.resources.onboarding_suggest_daily
import bot.mac.mobile.resources.onboarding_suggest_login
import bot.mac.mobile.resources.onboarding_suggest_research
import org.jetbrains.compose.resources.stringResource

@Composable
internal fun MainBotEmptyState(onSuggestion: (String) -> Unit) {
    val palette = LocalMobilePalette.current
    val suggestions = listOf(
        stringResource(Res.string.onboarding_suggest_login),
        stringResource(Res.string.onboarding_suggest_research),
        stringResource(Res.string.onboarding_suggest_daily),
    )
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 20.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Surface(
            modifier = Modifier.fillMaxWidth(),
            color = palette.sidebar,
            shape = RoundedCornerShape(16.dp),
        ) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(Res.string.onboarding_empty_title), style = MaterialTheme.typography.titleMedium)
                Text(
                    stringResource(Res.string.onboarding_empty_desc),
                    style = MaterialTheme.typography.bodyMedium,
                    color = palette.textSecondary,
                )
            }
        }
        suggestions.forEach { suggestion ->
            OutlinedButton(
                onClick = { onSuggestion(suggestion) },
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(12.dp),
            ) {
                Text(suggestion, modifier = Modifier.fillMaxWidth(), textAlign = TextAlign.Start)
            }
        }
    }
}

/** Applies a suggestion to the draft and then requests composer focus; it never sends it. */
internal fun applyMainBotSuggestion(
    suggestion: String,
    setDraft: (String) -> Unit,
    requestFocus: () -> Unit,
) {
    setDraft(suggestion)
    requestFocus()
}
