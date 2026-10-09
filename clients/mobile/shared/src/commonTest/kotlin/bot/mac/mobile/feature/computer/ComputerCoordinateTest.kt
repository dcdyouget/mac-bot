package bot.mac.mobile.feature.computer

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.unit.IntSize
import bot.mac.mobile.core.network.ScreenFrame
import bot.mac.mobile.core.network.ScreenFrameHeader
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull

class ComputerCoordinateTest {
    @Test
    fun fitMappingClampsToFramePixels() {
        val frame = ScreenFrame(ScreenFrameHeader(1, "tab", 1000, 500, 0, "https://example.test"), ByteArray(0))
        val center = mapToFrame(Offset(500f, 250f), IntSize(1000, 500), frame, 1f, Offset.Zero)
        assertNotNull(center)
        assertEquals(500f, center.x)
        assertEquals(250f, center.y)
        val clamped = mapToFrame(Offset(-100f, 900f), IntSize(1000, 500), frame, 1f, Offset.Zero)
        assertNotNull(clamped)
        assertEquals(0f, clamped.x)
        assertEquals(500f, clamped.y)
    }

    @Test
    fun zoomAndPanAreInvertedBeforeFitMapping() {
        val frame = ScreenFrame(ScreenFrameHeader(1, "tab", 1000, 500, 0, ""), ByteArray(0))
        val point = mapToFrame(Offset(600f, 300f), IntSize(1000, 500), frame, 2f, Offset(100f, 50f))
        assertNotNull(point)
        assertEquals(500f, point.x)
        assertEquals(250f, point.y)
    }
}
