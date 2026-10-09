package bot.mac.mobile.feature.routines

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class RoutinesLogicTest {
    @Test
    fun pauseAndResumeInvertEnabledState() {
        assertFalse(toggleRoutineEnabled(true))
        assertTrue(toggleRoutineEnabled(false))
    }
}
