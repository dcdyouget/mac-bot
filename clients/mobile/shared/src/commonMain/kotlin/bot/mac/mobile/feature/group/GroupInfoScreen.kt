package bot.mac.mobile.feature.group

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
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
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.design.FontScaleToolbar
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.design.BotAvatar
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.ui.MarkdownText
import bot.mac.mobile.core.ui.LucideIcon
import bot.mac.mobile.core.ui.LucideIconKind
import bot.mac.mobile.feature.chat.StatusLabel
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.feature_announcement_empty
import bot.mac.mobile.resources.feature_announcement_edit
import bot.mac.mobile.resources.feature_announcement_updated
import bot.mac.mobile.resources.feature_artifact_author
import bot.mac.mobile.resources.feature_bot
import bot.mac.mobile.resources.feature_home_copy
import bot.mac.mobile.resources.feature_home_open
import bot.mac.mobile.resources.feature_home_value
import bot.mac.mobile.resources.common_back
import bot.mac.mobile.resources.group_info
import bot.mac.mobile.resources.group_announcement
import bot.mac.mobile.resources.group_members
import bot.mac.mobile.resources.group_artifacts
import bot.mac.mobile.resources.group_info_preview
import bot.mac.mobile.resources.common_copy_path
import bot.mac.mobile.resources.common_provided_by
import bot.mac.mobile.resources.group_announcement_edit_title
import bot.mac.mobile.resources.group_announcement_empty
import bot.mac.mobile.resources.group_announcement_meta
import bot.mac.mobile.resources.group_open_home
import bot.mac.mobile.resources.search_filter_bots
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
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
    val projectChat = state.chats.firstOrNull { it.str("id") == project?.str("chat_id") }
    val chatMemberIds = projectChat?.arr("member_bot_ids").orEmpty()
        .mapNotNull { (it as? JsonPrimitive)?.content?.takeIf(String::isNotBlank) }
    val groupMembers = announcement?.arr("members")?.takeIf { it.isNotEmpty() }
        ?: project?.arr("members")?.takeIf { it.isNotEmpty() }
        ?: chatMemberIds.map { botId ->
            val status = botsById[botId]?.obj("status")?.str("summary").orEmpty()
            buildJsonObject { put("bot_id", botId); put("state", status.ifBlank { "idle" }) }
        }
    val members = groupMembers
        .mapNotNull { it as? kotlinx.serialization.json.JsonObject }
        .mapNotNull { member ->
            val botId = member.str("bot_id")
            botsById[botId]?.let { bot -> member to bot }
        }
    val artifacts = announcement?.arr("artifacts").orEmpty()
        .mapNotNull { it as? kotlinx.serialization.json.JsonObject }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        FontScaleToolbar {
            Row(
                Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = onBack, modifier = Modifier.size(48.dp)) {
                    LucideIcon(LucideIconKind.ArrowLeft, Modifier.size(20.dp), contentDescription = stringResource(Res.string.common_back))
                }
                Text(
                    text = stringResource(Res.string.group_info),
                    modifier = Modifier.weight(1f).padding(start = 8.dp),
                    style = MaterialTheme.typography.titleLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
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
                GroupAvatar(repository, groupMembers, state.bots, 64.dp)
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
                title = stringResource(Res.string.group_announcement),
                detail = announcement?.obj("text_updated_by")?.str("bot_id")?.let { botId ->
                    botsById[botId]?.str("name")?.takeIf(String::isNotBlank)
                }?.let { author ->
                    val updatedAt = announcement?.str("text_updated_at").orEmpty()
                    if (updatedAt.isBlank()) author else stringResource(Res.string.group_announcement_meta, author, updatedAt)
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
                                stringResource(Res.string.group_announcement_empty),
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
                    OutlinedButton(onClick = { onEditAnnouncement(projectId) }, modifier = Modifier.weight(1f), contentPadding = PaddingValues(horizontal = 4.dp, vertical = 0.dp)) {
                        Text(stringResource(Res.string.group_announcement_edit_title), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    OutlinedButton(
                        onClick = { if (homePath.isNotBlank()) onOpenHome(homePath) },
                        enabled = homePath.isNotBlank(),
                        modifier = Modifier.weight(1f),
                        contentPadding = PaddingValues(horizontal = 4.dp, vertical = 0.dp),
                    ) { Text(stringResource(Res.string.group_open_home), maxLines = 1, overflow = TextOverflow.Ellipsis) }
                    OutlinedButton(
                        onClick = { if (homePath.isNotBlank()) onCopyHome(homePath) },
                        enabled = homePath.isNotBlank(),
                        modifier = Modifier.weight(1f),
                        contentPadding = PaddingValues(horizontal = 4.dp, vertical = 0.dp),
                    ) { Text(stringResource(Res.string.common_copy_path), maxLines = 1, overflow = TextOverflow.Ellipsis) }
                }
                Text(stringResource(Res.string.feature_home_value, homePath), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }

            InfoSection(
                title = stringResource(Res.string.group_members),
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
                                Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(vertical = 4.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                BotAvatar(repository, bot, main = bot.boolean("is_main"), status = member.str("state"), size = 32.dp)
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

            InfoSection(title = stringResource(Res.string.group_artifacts), detail = artifacts.size.toString()) {
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
                                ?: stringResource(Res.string.search_filter_bots)
                            Row(
                                Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(vertical = 8.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                ArtifactGlyph()
                                Column(Modifier.weight(1f).padding(start = 12.dp, end = 8.dp)) {
                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        Text(artifact.str("title").ifBlank { path }, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                        Spacer(Modifier.width(6.dp))
                                        Text(stringResource(Res.string.common_provided_by, author), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
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
private fun ArtifactGlyph() {
    Box(
        Modifier.size(40.dp).clip(RoundedCornerShape(10.dp)).background(MaterialTheme.colorScheme.surfaceContainerHigh),
        contentAlignment = Alignment.Center,
    ) {
        LucideIcon(LucideIconKind.FileText, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.primary)
    }
}
