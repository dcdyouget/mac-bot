package bot.mac.mobile.feature.skills

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class SkillsLogicTest {
    @Test
    fun onlyDraftSkillsCanBePublished() {
        assertTrue(skillCanPublish("draft"))
        assertFalse(skillCanPublish("user"))
        assertFalse(skillCanPublish("builtin"))
        assertFalse(skillCanPublish(null))
    }
}
