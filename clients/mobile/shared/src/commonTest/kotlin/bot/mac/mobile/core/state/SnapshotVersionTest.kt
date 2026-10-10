package bot.mac.mobile.core.state

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class SnapshotVersionTest {
    @Test
    fun newerRevisionWinsWhenServerCursorIsUnchanged() {
        assertTrue(acceptsSnapshot(SnapshotVersion(8, 4), SnapshotVersion(7, 4), durableSeq = 4))
        assertFalse(acceptsSnapshot(SnapshotVersion(7, 4), SnapshotVersion(8, 4), durableSeq = 4))
    }

    @Test
    fun resetCanLowerCursorOnlyAtANewerRevision() {
        assertTrue(acceptsSnapshot(SnapshotVersion(12, 3), SnapshotVersion(11, 99), durableSeq = 3))
        assertFalse(acceptsSnapshot(SnapshotVersion(11, 3), SnapshotVersion(11, 99), durableSeq = 3))
        assertFalse(acceptsSnapshot(SnapshotVersion(12, 2), SnapshotVersion(11, 99), durableSeq = 3))
    }
}
