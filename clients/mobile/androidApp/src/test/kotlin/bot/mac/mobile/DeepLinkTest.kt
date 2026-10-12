package bot.mac.mobile

import bot.mac.mobile.core.platform.DeepLinkKind
import bot.mac.mobile.core.platform.MobileDeepLink
import bot.mac.mobile.core.platform.containsDeepLinkResource
import bot.mac.mobile.core.platform.parseMobileDeepLink
import bot.mac.mobile.core.platform.resolveHost
import bot.mac.mobile.core.state.MobileState
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36])
class DeepLinkTest {
    @Test
    fun supportedKindsAndEncodedParametersAreParsed() {
        assertEquals(
            MobileDeepLink(DeepLinkKind.APPROVAL, "apr_123", "host-a"),
            parseMobileDeepLink("macbot://approval/apr_123?host_id=host%2Da"),
        )
        assertEquals(
            MobileDeepLink(DeepLinkKind.REVIEW, "prj_123", "host-a"),
            parseMobileDeepLink("macbot://review/prj_123?host_id=host-a"),
        )
        assertEquals(
            MobileDeepLink(DeepLinkKind.CHAT, "chat_main", null),
            parseMobileDeepLink("macbot://chat/chat_main"),
        )
        assertEquals(
            MobileDeepLink(DeepLinkKind.CHAT, "chat_main", "host-a", "msg_123"),
            parseMobileDeepLink("macbot://chat/chat_main?host_id=host-a&message_id=msg_123"),
        )
        assertEquals(
            MobileDeepLink(DeepLinkKind.COMPUTER, "bot_123", "host-a"),
            parseMobileDeepLink("macbot://computer/bot_123?host_id=host-a"),
        )
    }

    @Test
    fun malformedAndUnrecognizedLinksAreRejected() {
        listOf(
            "https://chat/chat_main",
            "macbot://unknown/chat_main",
            "macbot:chat/chat_main",
            "macbot://chat",
            "macbot://chat/",
            "macbot://chat/chat_main/extra",
            "macbot://chat/%2Fchat_main",
            "macbot://chat/../chat_main",
            "macbot://user@chat/chat_main",
            "macbot://chat:443/chat_main",
            "macbot://chat/chat_main?other=value",
            "macbot://chat/chat_main?host_id=host-a&host_id=host-b",
            "macbot://chat/chat_main?host_id=",
            "macbot://chat/chat_main#fragment",
            "macbot://chat/chat_main?host_id=%2Fbad",
            "macbot://chat/chat_main?message_id=bad%2Fid",
            "macbot://computer/bot_123?message_id=msg_123",
            "macbot://chat/chat_main?message_id=msg_123&message_id=msg_456",
        ).forEach { assertNull("must reject $it", parseMobileDeepLink(it)) }
    }

    @Test
    fun hostAndResourceMustBelongToKnownState() {
        val chatLink = requireNotNull(parseMobileDeepLink("macbot://chat/chat_main?host_id=host-a"))
        assertEquals("host-a", chatLink.resolveHost(setOf("host-a", "host-b"), "host-b"))
        assertNull(chatLink.resolveHost(setOf("host-b"), "host-b"))
        assertNull(chatLink.resolveHost(emptySet(), null))

        val state = MobileState(
            chats = listOf(buildJsonObject { put("id", "chat_main") }),
            projects = listOf(buildJsonObject { put("id", "prj_123") }),
            approvals = listOf(buildJsonObject { put("id", "apr_123") }),
            bots = listOf(buildJsonObject { put("id", "bot_123") }),
        )
        assertTrue(state.containsDeepLinkResource(chatLink))
        assertTrue(state.containsDeepLinkResource(MobileDeepLink(DeepLinkKind.REVIEW, "prj_123", "host-a")))
        assertTrue(state.containsDeepLinkResource(MobileDeepLink(DeepLinkKind.APPROVAL, "apr_123", "host-a")))
        assertTrue(state.containsDeepLinkResource(MobileDeepLink(DeepLinkKind.COMPUTER, "bot_123", "host-a")))
        assertFalse(state.containsDeepLinkResource(MobileDeepLink(DeepLinkKind.CHAT, "chat_other", "host-a")))
    }
}
