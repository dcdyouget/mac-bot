package bot.mac.mobile.core.design

import androidx.compose.foundation.Image
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
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
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.sp
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
    size: Dp = 48.dp,
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
        kind == "emoji" && emoji.isNotBlank() -> Box(Modifier.size(size)) {
            EmojiAvatar(emoji, size)
            if (main) MainBotGlassesBadge(Modifier.align(Alignment.BottomEnd).size(size * 0.4f))
        }
        kind == "image" && image != null -> Box(Modifier.size(size)) {
            Image(
                bitmap = image!!,
                contentDescription = null,
                modifier = Modifier.fillMaxSize().clip(CircleShape),
                contentScale = ContentScale.Crop,
            )
            if (main) MainBotGlassesBadge(Modifier.align(Alignment.BottomEnd).size(size * 0.4f))
        }
        else -> BeanAvatar(color = color, main = main, status = status, avatarSize = size)
    }
}

@Composable
private fun MainBotGlassesBadge(modifier: Modifier = Modifier) {
    val background = MaterialTheme.colorScheme.surface
    Canvas(modifier) {
        drawCircle(background, radius = size.minDimension / 2)
        val frame = DesignTokens.avatarGlasses
        val stroke = (size.minDimension * 0.09f).coerceAtLeast(1.dp.toPx())
        val lensWidth = size.width * 0.34f
        val lensHeight = size.height * 0.30f
        val lensY = size.height * 0.36f
        val corner = CornerRadius(size.minDimension * 0.08f)
        drawRoundRect(frame, Offset(size.width * 0.12f, lensY), Size(lensWidth, lensHeight), corner, style = androidx.compose.ui.graphics.drawscope.Stroke(stroke))
        drawRoundRect(frame, Offset(size.width * 0.54f, lensY), Size(lensWidth, lensHeight), corner, style = androidx.compose.ui.graphics.drawscope.Stroke(stroke))
        drawLine(frame, Offset(size.width * 0.46f, size.height * 0.50f), Offset(size.width * 0.54f, size.height * 0.50f), stroke)
    }
}

@Composable
private fun EmojiAvatar(emoji: String, size: Dp) {
    Box(
        modifier = Modifier.size(size).clip(CircleShape).background(MaterialTheme.colorScheme.secondaryContainer),
        contentAlignment = Alignment.Center,
    ) {
        Text(emoji, maxLines = 1, style = MaterialTheme.typography.titleLarge.copy(fontSize = (size.value * 0.55f).sp))
    }
}
