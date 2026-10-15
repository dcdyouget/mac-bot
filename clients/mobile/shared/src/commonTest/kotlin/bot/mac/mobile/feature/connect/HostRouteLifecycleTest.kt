package bot.mac.mobile.feature.connect

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class HostRouteLifecycleTest {
    @Test
    fun differentHostMustResetHostScopedRoute() {
        assertTrue(shouldResetHostScopedRoute("host-a", "host-b"))
    }

    @Test
    fun sameOrInitiallyUnknownHostDoesNotResetRoute() {
        assertFalse(shouldResetHostScopedRoute("host-a", "host-a"))
        assertFalse(shouldResetHostScopedRoute(null, "host-a"))
        assertFalse(shouldResetHostScopedRoute("host-a", null))
    }
}
