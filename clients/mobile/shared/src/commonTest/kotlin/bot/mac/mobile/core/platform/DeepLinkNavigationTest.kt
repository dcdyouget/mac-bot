package bot.mac.mobile.core.platform

import bot.mac.mobile.core.state.MobileState
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class DeepLinkNavigationTest {
    @Test
    fun missingResourceOnAnotherHostResetsOldRouteBeforeSwitchAndStaysHome() = runTest {
        var activeHost = "host-a"
        var route = "settings"
        val order = mutableListOf<String>()
        val result = resolveDeepLinkNavigation(
            link = MobileDeepLink(DeepLinkKind.APPROVAL, "missing-approval", "host-b"),
            currentHostId = activeHost,
            knownHostIds = setOf("host-a", "host-b"),
            onHostSwitch = { hostId ->
                route = "home"
                order += "reset:$hostId"
            },
            selectHost = { hostId ->
                activeHost = hostId
                order += "select:$hostId"
            },
            stateForHost = { MobileState() },
        )

        assertNull(result)
        assertEquals("host-b", activeHost)
        assertEquals("home", route)
        assertEquals(listOf("reset:host-b", "select:host-b"), order)
    }

    @Test
    fun validDeepLinkOnAnotherHostReturnsDestinationAfterSwitch() = runTest {
        var activeHost = "host-a"
        val result = resolveDeepLinkNavigation(
            link = MobileDeepLink(DeepLinkKind.CHAT, "chat-b", "host-b", "message-b"),
            currentHostId = activeHost,
            knownHostIds = setOf("host-a", "host-b"),
            onHostSwitch = {},
            selectHost = { activeHost = it },
            stateForHost = { MobileState(chats = listOf(kotlinx.serialization.json.buildJsonObject { put("id", "chat-b") })) },
        )

        assertEquals("host-b", activeHost)
        assertEquals(DeepLinkDestination(DeepLinkKind.CHAT, "chat-b", "message-b"), result)
    }
}
