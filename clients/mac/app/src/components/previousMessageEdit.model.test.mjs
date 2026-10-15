import assert from 'node:assert/strict'
import test from 'node:test'
import { previousEditableMessage } from './previousMessageEdit.model.ts'

const message = (id, sender, overrides = {}) => ({
  id, chat_id: 'chat-1', sender, mentions: [], blocks: [{ type: 'text', markdown: `text ${id}` }], ...overrides,
})

test('finds the latest own plain-text message after bot messages', () => {
  assert.deepEqual(previousEditableMessage([
    message('mine', { kind: 'user' }), message('bot', { kind: 'bot', bot_id: 'bot-1' }),
  ]), { id: 'mine', text: 'text mine' })
})

test('does not skip an ineligible latest own message to edit an older one', () => {
  assert.equal(previousEditableMessage([
    message('older', { kind: 'user' }),
    message('latest', { kind: 'user' }, { mentions: [{ kind: 'bot', bot_id: 'bot-1' }] }),
  ]), null)
})

test('only allows plain text and rejects deleted or streaming messages', () => {
  assert.equal(previousEditableMessage([message('file', { kind: 'user' }, { blocks: [{ type: 'file' }] })]), null)
  assert.equal(previousEditableMessage([message('deleted', { kind: 'user' }, { deleted: true })]), null)
  assert.equal(previousEditableMessage([message('streaming', { kind: 'user' }, { streaming: true })]), null)
})
