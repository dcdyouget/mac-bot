package bot.mac.mobile.feature.group

import kotlin.test.Test
import kotlin.test.assertEquals

class AnnouncementMarkdownToolbarTest {
    @Test
    fun reversedSelectionFormatsTheOriginalText() {
        val edit = applyAnnouncementMarkdownAction("hello world", 11, 6, AnnouncementMarkdownAction.BOLD)

        assertEquals("hello **world**", edit.text)
        assertEquals(8, edit.selectionStart)
        assertEquals(13, edit.selectionEnd)
    }

    @Test
    fun listPrefixAppliesToEverySelectedLineAndKeepsItsText() {
        val edit = applyAnnouncementMarkdownAction("one\ntwo\nthree", 1, 7, AnnouncementMarkdownAction.BULLET)

        assertEquals("- one\n- two\nthree", edit.text)
        assertEquals(3, edit.selectionStart)
        assertEquals(11, edit.selectionEnd)
    }

    @Test
    fun headingAtLineStartLeavesCaretAfterPrefix() {
        val edit = applyAnnouncementMarkdownAction("title", 0, 0, AnnouncementMarkdownAction.HEADING)

        assertEquals("# title", edit.text)
        assertEquals(2, edit.selectionStart)
        assertEquals(2, edit.selectionEnd)
    }

    @Test
    fun codeWrapsSelectedTextWithoutDiscardingIt() {
        val edit = applyAnnouncementMarkdownAction("use value", 4, 9, AnnouncementMarkdownAction.CODE)

        assertEquals("use `value`", edit.text)
        assertEquals(5, edit.selectionStart)
        assertEquals(10, edit.selectionEnd)
    }

    @Test
    fun linkSelectionKeepsLabelAndPlacesCursorInUrl() {
        val edit = applyAnnouncementMarkdownAction("open docs now", 5, 9, AnnouncementMarkdownAction.LINK)

        assertEquals("open [docs](url) now", edit.text)
        assertEquals(12, edit.selectionStart)
        assertEquals(15, edit.selectionEnd)
    }
}
