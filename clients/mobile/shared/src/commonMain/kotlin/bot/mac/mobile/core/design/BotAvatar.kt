package bot.mac.mobile.core.design

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.platform.platformScreenImageDecoder
import bot.mac.mobile.core.protocol.FileRef
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import kotlinx.coroutines.CancellationException
import kotlinx.serialization.json.JsonObject

/** Renders the protocol avatar and falls back to the bean when an image is unavailable. */
@Composable
fun BotAvatar(
    repository: MobileRepository,
    bot: JsonObject?,
    main: Boolean = false,
    status: String = "idle",
) {
    val avatar = bot?.obj("avatar")
    val kind = avatar?.str("kind").orEmpty()
    val color = avatar?.get("color")?.toString()?.trim('"')?.toIntOrNull() ?: 0
    val emoji = avatar?.str("emoji").orEmpty()
    val file = avatar?.obj("file")
    val fileRef = remember(file) { file?.let(::FileRef) }
    val imageKey = fileRef?.let { "${it.root}:${it.rootId}:${it.path}" }
    var image by remember(imageKey) { mutableStateOf<ImageBitmap?>(null) }
    val decoder = remember { platformScreenImageDecoder() }

    LaunchedEffect(kind, imageKey) {
        image = null
        if (kind != "image" || fileRef == null || imageKey.isNullOrBlank()) return@LaunchedEffect
        try {
            val bytes = repository.fetchBytes(
                "/api/v1/files",
                mapOf("root" to fileRef.root, "root_id" to fileRef.rootId, "path" to fileRef.path),
            )
            image = decoder.decodeJpeg(bytes)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
            image = null
        }
    }

    when {
        kind == "emoji" && emoji.isNotBlank() -> EmojiAvatar(emoji)
        kind == "image" && image != null -> Image(
            bitmap = image!!,
            contentDescription = null,
            modifier = Modifier.size(42.dp).clip(CircleShape),
            contentScale = ContentScale.Crop,
        )
        else -> BeanAvatar(color = color, main = main, status = status)
    }
}

@Composable
private fun EmojiAvatar(emoji: String) {
    Box(
        modifier = Modifier.size(42.dp).clip(CircleShape).background(MaterialTheme.colorScheme.secondaryContainer),
        contentAlignment = Alignment.Center,
    ) {
        Text(emoji, maxLines = 1, style = MaterialTheme.typography.titleLarge)
    }
}
