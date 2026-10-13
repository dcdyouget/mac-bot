import assert from 'node:assert/strict'
import test from 'node:test'
import { captureChatScrollPosition, restoreChatScrollTop } from './chatScrollPosition.model.ts'

test('captures the first visible message as an anchor and its viewport offset', () => {
  assert.deepEqual(captureChatScrollPosition({
    scrollTop: 540,
    viewportTop: 100,
    viewportBottom: 700,
    messages: [
      { messageId: 'above', top: 20, bottom: 95 },
      { messageId: 'partially-visible', top: 82, bottom: 145 },
      { messageId: 'next', top: 150, bottom: 210 },
    ],
  }), { anchorMessageId: 'partially-visible', anchorOffset: -18, scrollTop: 540 })
})

test('restores the same message offset after content above it changes height', () => {
  const position = { anchorMessageId: 'message-7', anchorOffset: 28, scrollTop: 800 }
  assert.equal(restoreChatScrollTop(position, 800, 100, 180), 852)
})

test('falls back to the captured scrollTop when the message anchor no longer exists', () => {
  assert.equal(restoreChatScrollTop({ anchorMessageId: 'deleted-message', anchorOffset: 12, scrollTop: 455 }, 0, 0, null), 455)
  assert.equal(restoreChatScrollTop({ anchorMessageId: null, anchorOffset: 0, scrollTop: -5 }, 0, 0, null), 0)
})
