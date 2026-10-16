import assert from 'node:assert/strict'
import test from 'node:test'
import { projectMessageDeltas, streamingMessageContentMode } from './streamingMessages.model.ts'
import { mergeChatMessages } from '../groupConversation.model.mjs'

const placeholder = {
  id: 'reply-1', chat_id: 'chat-a', sender: { kind: 'bot', bot_id: 'bot-a' },
  streaming: true, deleted: false, blocks: [], fallback_text: '',
}

test('streams into the same placeholder and final content replaces the transient projection', () => {
  assert.equal(streamingMessageContentMode(placeholder), 'placeholder')
  const streamed = projectMessageDeltas([placeholder], { 'reply-1': 'hello' }, 'chat-a')
  assert.equal(streamed.length, 1)
  assert.equal(streamed[0].id, 'reply-1')
  assert.equal(streamed[0].blocks[0].markdown, 'hello')
  assert.equal(streamingMessageContentMode(streamed[0]), 'text')

  const final = { ...placeholder, streaming: false, fallback_text: 'hello world', blocks: [{ type: 'text', markdown: 'hello world' }] }
  const reconciled = mergeChatMessages(streamed, [final])
  const messages = projectMessageDeltas(reconciled, {}, 'chat-a')
  assert.equal(messages.length, 1)
  assert.equal(messages[0].fallback_text, 'hello world')
})

test('a later reply with a new ID remains a separate bubble', () => {
  const streamed = projectMessageDeltas([placeholder], { 'reply-1': 'hello' }, 'chat-a')
  const next = { ...placeholder, id: 'reply-2', streaming: false, fallback_text: 'next' }
  const messages = projectMessageDeltas([...streamed, next], { 'reply-1': 'hello' }, 'chat-a')
  assert.deepEqual(messages.map((message) => message.id), ['reply-1', 'reply-2'])
})

test('does not project a delta into a different chat', () => {
  const otherChat = { ...placeholder, chat_id: 'chat-b' }
  const messages = projectMessageDeltas([otherChat], { 'reply-1': 'wrong chat' }, 'chat-a')
  assert.equal(messages[0], otherChat)
  assert.deepEqual(messages[0].blocks, [])
})

test('projects deltas after existing text while preserving rich blocks and canonical input', () => {
  const canonical = {
    ...placeholder,
    fallback_text: '先说明',
    blocks: [{ type: 'text', markdown: '先说明' }, { type: 'completion', summary: '已完成' }],
  }
  const first = projectMessageDeltas([canonical], { 'reply-1': '，再补充' }, 'chat-a')
  const repeated = projectMessageDeltas([canonical], { 'reply-1': '，再补充' }, 'chat-a')
  assert.deepEqual(first[0].blocks, [
    { type: 'text', markdown: '先说明，再补充' },
    { type: 'completion', summary: '已完成' },
  ])
  assert.equal(first[0].fallback_text, '先说明，再补充')
  assert.deepEqual(repeated, first)
  assert.equal(canonical.blocks[0].markdown, '先说明')
  assert.equal(streamingMessageContentMode(first[0]), 'blocks')
})
