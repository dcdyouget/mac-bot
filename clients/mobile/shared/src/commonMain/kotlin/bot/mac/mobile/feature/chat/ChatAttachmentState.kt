package bot.mac.mobile.feature.chat

import androidx.compose.ui.graphics.ImageBitmap
import bot.mac.mobile.core.platform.PickedFile
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

internal const val MAX_CHAT_ATTACHMENT_BYTES = 100 * 1024 * 1024

internal enum class ChatAttachmentStatus { UPLOADING, READY, ERROR }

internal data class ChatAttachmentItem(
    val localId: String,
    val name: String,
    val size: Long,
    val mimeType: String?,
    val status: ChatAttachmentStatus,
    val uploadId: String? = null,
    val error: String? = null,
    val source: PickedFile? = null,
    val preview: ImageBitmap? = null,
)

internal fun chatDraftKey(hostId: String, chatId: String): String = "draft:$hostId:$chatId"

internal fun chatAttachmentsDraftKey(hostId: String, chatId: String): String = "draft-attachments:$hostId:$chatId"

internal fun isCurrentUploadHost(requestHostId: String, activeHostId: String?): Boolean =
    requestHostId == activeHostId

internal fun canSendChatMessage(text: String, attachments: List<ChatAttachmentItem>, readOnly: Boolean): Boolean =
    !readOnly && attachments.all { it.status == ChatAttachmentStatus.READY && !it.uploadId.isNullOrBlank() } &&
        (text.isNotBlank() || attachments.isNotEmpty())

internal fun encodeAttachmentDraft(attachments: List<ChatAttachmentItem>): String = buildJsonArray {
    attachments.filter { it.status == ChatAttachmentStatus.READY && !it.uploadId.isNullOrBlank() }.forEach { item ->
        add(buildJsonObject {
            put("local_id", item.localId)
            put("name", item.name)
            put("size", item.size)
            put("mime_type", item.mimeType)
            put("upload_id", item.uploadId)
        })
    }
}.toString()

internal fun decodeAttachmentDraft(value: String?): List<ChatAttachmentItem> = runCatching {
    Json.parseToJsonElement(value ?: "[]").jsonArray.mapNotNull { raw ->
        val item = raw as? JsonObject ?: return@mapNotNull null
        val localId = item["local_id"]?.jsonPrimitive?.contentOrNull?.takeIf(String::isNotBlank) ?: return@mapNotNull null
        val name = item["name"]?.jsonPrimitive?.contentOrNull?.takeIf(String::isNotBlank) ?: return@mapNotNull null
        val uploadId = item["upload_id"]?.jsonPrimitive?.contentOrNull?.takeIf(String::isNotBlank) ?: return@mapNotNull null
        ChatAttachmentItem(
            localId = localId,
            name = name,
            size = item["size"]?.jsonPrimitive?.contentOrNull?.toLongOrNull() ?: 0L,
            mimeType = item["mime_type"]?.jsonPrimitive?.contentOrNull,
            status = ChatAttachmentStatus.READY,
            uploadId = uploadId,
        )
    }
}.getOrDefault(emptyList())
