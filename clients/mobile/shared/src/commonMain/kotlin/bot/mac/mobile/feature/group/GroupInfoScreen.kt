package bot.mac.mobile.feature.group

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.ui.MarkdownText
import bot.mac.mobile.feature.chat.StatusLabel
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.feature_announcement_empty
import bot.mac.mobile.resources.feature_announcement_edit
import bot.mac.mobile.resources.feature_announcement_updated
import bot.mac.mobile.resources.feature_artifact_author
import bot.mac.mobile.resources.feature_back
import bot.mac.mobile.resources.feature_bot
import bot.mac.mobile.resources.feature_home_copy
import bot.mac.mobile.resources.feature_home_open
import bot.mac.mobile.resources.feature_home_value
import bot.mac.mobile.resources.group_info_title
import bot.mac.mobile.resources.group_info_announcement
import bot.mac.mobile.resources.group_info_members
import bot.mac.mobile.resources.group_info_artifacts
import bot.mac.mobile.resources.group_info_preview
import org.jetbrains.compose.resources.stringResource

/** Group information page backed by the project, announcement, bot, and artifact snapshots. */
@Composable
fun GroupInfoScreen(
    repository: MobileRepository,
    projectId: String,
    onBack: () -> Unit,
    onEditAnnouncement: (projectId: String) -> Unit,
    onOpenHome: (path: String) -> Unit,
    onCopyHome: (path: String) -> Unit,
    onOpenArtifact: (artifactId: String, pathOrUrl: String, projectId: String?) -> Unit,
) {
    val state by repository.state.collectAsState()
    val project = state.projects.firstOrNull { it.str("id") == projectId }
    val announcement = state.announcements[projectId]
    val botsById = state.bots.associateBy { it.str("id") }
    val homePath = project?.str("home_path").orEmpty()
    val members = (announcement?.arr("members") ?: project?.arr("members")).orEmpty()
        .mapNotNull { it as? kotlinx.serialization.json.JsonObject }
        .mapNotNull { member ->
            val botId = member.str("bot_id")
            botsById[botId]?.let { bot -> member to bot }
        }
    val artifacts = announcement?.arr("artifacts").orEmpty()
        .mapNotNull { it as? kotlinx.serialization.json.JsonObject }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            OutlinedButton(onClick = onBack, modifier = Modifier.size(48.dp)) {
                Text(stringResource(Res.string.feature_back), maxLines = 1, overflow = TextOverflow.Clip)
            }
            Text(
                text = stringResource(Res.string.group_info_title),
                modifier = Modifier.weight(1f).padding(start = 8.dp),
                style = MaterialTheme.typography.titleLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }

        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Column(
                Modifier.fillMaxWidth().padding(vertical = 4.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                ProjectAvatar(project?.str("name").orEmpty(), Modifier.size(64.dp))
                Text(
                    project?.str("name").orEmpty(),
                    style = MaterialTheme.typography.headlineSmall,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                project?.str("status")?.takeIf(String::isNotBlank)?.let { StatusLabel(it) }
            }

            InfoSection(
                title = stringResource(Res.string.group_info_announcement),
                detail = announcement?.obj("text_updated_by")?.str("bot_id")?.let { botId ->
                    botsById[botId]?.str("name")?.takeIf(String::isNotBlank)
                },
            ) {
                val announcementText = announcement?.str("text").orEmpty()
                Surface(
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(12.dp),
                    color = MaterialTheme.colorScheme.surfaceContainerHigh,
                ) {
                    Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        if (announcementText.isNotBlank()) {
                            MarkdownText(announcementText, Modifier.fillMaxWidth())
                        } else {
                            Text(
                                stringResource(Res.string.feature_announcement_empty),
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                style = MaterialTheme.typography.bodyMedium,
                            )
                        }
                        announcement?.str("text_updated_at")?.takeIf(String::isNotBlank)?.let {
                            Text(stringResource(Res.string.feature_announcement_updated, it), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedButton(onClick = { onEditAnnouncement(projectId) }, modifier = Modifier.weight(1f)) {
                        Text(stringResource(Res.string.feature_announcement_edit), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    OutlinedButton(
                        onClick = { if (homePath.isNotBlank()) onOpenHome(homePath) },
                        enabled = homePath.isNotBlank(),
                        modifier = Modifier.weight(1f),
                    ) { Text(stringResource(Res.string.feature_home_open), maxLines = 1, overflow = TextOverflow.Ellipsis) }
                    OutlinedButton(
                        onClick = { if (homePath.isNotBlank()) onCopyHome(homePath) },
                        enabled = homePath.isNotBlank(),
                        modifier = Modifier.weight(1f),
                    ) { Text(stringResource(Res.string.feature_home_copy), maxLines = 1, overflow = TextOverflow.Ellipsis) }
                }
                Text(stringResource(Res.string.feature_home_value, homePath), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }

            InfoSection(
                title = stringResource(Res.string.group_info_members),
                detail = members.size.toString(),
            ) {
                Surface(
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(12.dp),
                    color = MaterialTheme.colorScheme.surfaceContainer,
                ) {
                    Column(Modifier.padding(horizontal = 14.dp)) {
                        members.forEachIndexed { index, (member, bot) ->
                            if (index > 0) HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                            Row(
                                Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(vertical = 8.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                MemberAvatar(bot.str("name"), Modifier.size(36.dp))
                                Column(Modifier.weight(1f).padding(start = 12.dp)) {
                                    Text(bot.str("name").ifBlank { member.str("bot_id") }, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    member.str("role_note").takeIf(String::isNotBlank)?.let {
                                        Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    }
                                }
                                StatusLabel(member.str("state"))
                            }
                        }
                    }
                }
            }

            InfoSection(title = stringResource(Res.string.group_info_artifacts), detail = artifacts.size.toString()) {
                Surface(
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(12.dp),
                    color = MaterialTheme.colorScheme.surfaceContainer,
                ) {
                    Column(Modifier.padding(horizontal = 14.dp)) {
                        artifacts.forEachIndexed { index, artifact ->
                            if (index > 0) HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                            val artifactId = artifact.str("artifact_id").ifBlank { artifact.str("id") }
                            val path = artifact.str("path_or_url")
                            val author = botsById[artifact.str("bot_id")]?.str("name")
                                ?.takeIf(String::isNotBlank)
                                ?: stringResource(Res.string.feature_bot)
                            Row(
                                Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(vertical = 8.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                ArtifactGlyph()
                                Column(Modifier.weight(1f).padding(start = 12.dp, end = 8.dp)) {
                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        Text(artifact.str("title").ifBlank { path }, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                        Spacer(Modifier.width(6.dp))
                                        Text(stringResource(Res.string.feature_artifact_author, author), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    }
                                    Text(path, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                }
                                OutlinedButton(
                                    onClick = { onOpenArtifact(artifactId, path, projectId) },
                                    enabled = artifactId.isNotBlank() && path.isNotBlank(),
                                ) { Text(stringResource(Res.string.group_info_preview)) }
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun InfoSection(title: String, detail: String?, content: @Composable ColumnScope.() -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(title, modifier = Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
            detail?.takeIf(String::isNotBlank)?.let { Text(it, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        }
        content()
    }
}

@Composable
private fun ProjectAvatar(name: String, modifier: Modifier = Modifier) {
    val first = name.trim().firstOrNull()?.toString().orEmpty()
    Box(
        modifier.clip(CircleShape).background(MaterialTheme.colorScheme.primaryContainer),
        contentAlignment = Alignment.Center,
    ) {
        Text(first, color = MaterialTheme.colorScheme.onPrimaryContainer, style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.Medium)
    }
}

@Composable
private fun MemberAvatar(name: String, modifier: Modifier = Modifier) {
    Box(modifier.clip(CircleShape).background(MaterialTheme.colorScheme.secondaryContainer), contentAlignment = Alignment.Center) {
        Text(name.trim().firstOrNull()?.toString().orEmpty(), color = MaterialTheme.colorScheme.onSecondaryContainer, style = MaterialTheme.typography.titleMedium)
    }
}

@Composable
private fun ArtifactGlyph() {
    Box(
        Modifier.size(40.dp).clip(RoundedCornerShape(10.dp)).background(MaterialTheme.colorScheme.surfaceContainerHigh),
        contentAlignment = Alignment.Center,
    ) {
        Text("▤", color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.titleMedium)
    }
}
