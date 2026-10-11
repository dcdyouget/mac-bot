package bot.mac.mobile.feature.chat

import bot.mac.mobile.core.platform.PickedFile
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class ChatAttachmentStateTest {
    @Test
    fun textAndAttachmentDraftKeysAreIsolatedByHostAndChat() {
        assertEquals("draft:host-a:chat-1", chatDraftKey("host-a", "chat-1"))
        assertEquals("draft:host-b:chat-1", chatDraftKey("host-b", "chat-1"))
        assertEquals("draft-attachments:host-a:chat-1", chatAttachmentsDraftKey("host-a", "chat-1"))
        assertFalse(chatDraftKey("host-a", "chat-1") == chatDraftKey("host-a", "chat-2"))
    }

    @Test
    fun onlyCompletedUploadsCanBeSentIncludingAttachmentOnlyMessages() {
        val uploading = ChatAttachmentItem("local-1", "image.png", 12, "image/png", ChatAttachmentStatus.UPLOADING)
        val failed = uploading.copy(status = ChatAttachmentStatus.ERROR, error = "offline")
        val ready = uploading.copy(status = ChatAttachmentStatus.READY, uploadId = "upload-1")

        assertFalse(canSendChatMessage("", listOf(uploading), readOnly = false))
        assertFalse(canSendChatMessage("caption", listOf(failed), readOnly = false))
        assertFalse(canSendChatMessage("", listOf(ready), readOnly = true))
        assertTrue(canSendChatMessage("", listOf(ready), readOnly = false))
        assertTrue(canSendChatMessage("caption", emptyList(), readOnly = false))
    }

    @Test
    fun successfulUploadDraftRestoresWithoutRetainingFileBytes() {
        val ready = ChatAttachmentItem(
            localId = "local-1",
            name = "photo.png",
            size = 3,
            mimeType = "image/png",
            status = ChatAttachmentStatus.READY,
            uploadId = "upload-1",
            source = PickedFile("photo.png", "image/png", byteArrayOf(1, 2, 3)),
        )

        val restored = decodeAttachmentDraft(encodeAttachmentDraft(listOf(ready))).single()

        assertEquals("photo.png", restored.name)
        assertEquals("upload-1", restored.uploadId)
        assertEquals(ChatAttachmentStatus.READY, restored.status)
        assertEquals(null, restored.source)
    }

    @Test
    fun staleUploadCompletionCannotUpdateAnotherHost() {
        assertTrue(isCurrentUploadHost("host-a", "host-a"))
        assertFalse(isCurrentUploadHost("host-a", "host-b"))
        assertFalse(isCurrentUploadHost("host-a", null))
    }
}
