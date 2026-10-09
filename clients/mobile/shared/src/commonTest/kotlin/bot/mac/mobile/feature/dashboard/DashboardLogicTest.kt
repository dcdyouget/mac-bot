package bot.mac.mobile.feature.dashboard

import kotlin.test.Test
import kotlin.test.assertEquals

class DashboardLogicTest {
    @Test
    fun heatmapUsesFourNonZeroLevels() {
        val thresholds = listOf(10.0, 20.0, 30.0)
        assertEquals(0, heatmapBucket(0.0, thresholds))
        assertEquals(1, heatmapBucket(10.0, thresholds))
        assertEquals(2, heatmapBucket(20.0, thresholds))
        assertEquals(3, heatmapBucket(30.0, thresholds))
        assertEquals(4, heatmapBucket(31.0, thresholds))
    }
}
