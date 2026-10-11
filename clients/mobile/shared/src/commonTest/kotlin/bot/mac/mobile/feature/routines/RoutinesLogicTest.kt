package bot.mac.mobile.feature.routines

import kotlin.test.Test
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.jsonPrimitive
import kotlin.test.assertEquals

class RoutinesLogicTest {
    @Test
    fun setEnabledRpcPreservesRequestedStateAndRoutineId() {
        val disable = routineSetEnabledParams("routine-1", false)
        val enable = routineSetEnabledParams("routine-2", true)

        assertEquals("routine-1", disable.getValue("routine_id").jsonPrimitive.content)
        assertEquals(false, disable.getValue("enabled").jsonPrimitive.boolean)
        assertEquals("routine-2", enable.getValue("routine_id").jsonPrimitive.content)
        assertEquals(true, enable.getValue("enabled").jsonPrimitive.boolean)
    }
}
