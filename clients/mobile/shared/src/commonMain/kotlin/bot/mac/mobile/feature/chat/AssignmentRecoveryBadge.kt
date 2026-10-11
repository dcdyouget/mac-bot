package bot.mac.mobile.feature.chat

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.LocalMobilePalette
import bot.mac.mobile.core.protocol.generated.Assignment
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.status_recovered
import kotlinx.serialization.json.JsonObject
import org.jetbrains.compose.resources.stringResource

/** Shows recovery provenance without replacing the assignment's current status. */
@Composable
fun AssignmentRecoveryBadge(assignment: JsonObject, modifier: Modifier = Modifier) {
    val recoveredAt = Assignment(assignment).recoveredAt
    if (!recoveredAt.isNullOrBlank()) {
        val palette = LocalMobilePalette.current
        Text(
            text = stringResource(Res.string.status_recovered),
            modifier = modifier
                .clip(RoundedCornerShape(8.dp))
                .background(palette.accentSoft)
                .padding(horizontal = 6.dp, vertical = 2.dp),
            style = MaterialTheme.typography.labelSmall,
            color = palette.accent,
            maxLines = 1,
        )
    }
}
