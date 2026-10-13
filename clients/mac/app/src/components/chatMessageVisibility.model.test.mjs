import assert from 'node:assert/strict'
import test from 'node:test'
import { isMessageVisibleInChat } from './chatMessageVisibility.model.ts'
import { taskCardAvatarOwnerBotId } from './pendingChatActions.model.ts'

test('main Bot DM hides worker messages while keeping main Bot and system activity', () => {
  const chatId = 'chat-main'
  const mainBotId = 'bot-main'

  assert.equal(isMessageVisibleInChat({ chat_id: chatId, sender: { kind: 'bot', bot_id: mainBotId } }, chatId, mainBotId), true)
  assert.equal(isMessageVisibleInChat({ chat_id: chatId, sender: { kind: 'bot', bot_id: 'bot-worker' } }, chatId, mainBotId), false)
  assert.equal(isMessageVisibleInChat({ chat_id: chatId, sender: { kind: 'system' } }, chatId, mainBotId), true)
  assert.equal(isMessageVisibleInChat({ chat_id: chatId, sender: { kind: 'user' } }, chatId, mainBotId), true)
})

test('does not hide another Bot when viewing its own DM or a group conversation', () => {
  const workerMessage = { chat_id: 'chat-worker', sender: { kind: 'bot', bot_id: 'bot-worker' } }
  assert.equal(isMessageVisibleInChat(workerMessage, 'chat-worker'), true)
  assert.equal(isMessageVisibleInChat({ ...workerMessage, chat_id: 'chat-group' }, 'chat-group'), true)
})

test('rejects cached messages that belong to a different chat', () => {
  assert.equal(isMessageVisibleInChat({ chat_id: 'chat-other', sender: { kind: 'bot', bot_id: 'bot-main' } }, 'chat-main', 'bot-main'), false)
  assert.equal(isMessageVisibleInChat({ sender: { kind: 'bot', bot_id: 'bot-main' } }, 'chat-main', 'bot-main'), true)
})

test('hiding a worker reply keeps its following task card attributed to that worker', () => {
  const delegation = { id: 'delegation', chat_id: 'chat-main', sender: { kind: 'system' } }
  const workerReply = { id: 'worker-reply', chat_id: 'chat-main', sender: { kind: 'bot', bot_id: 'bot-worker' } }
  const taskCard = { id: 'task-card', chat_id: 'chat-main', sender: { kind: 'system' }, assignment_id: 'assignment-worker', blocks: [{ type: 'task_card', assignment_id: 'assignment-worker' }] }

  assert.equal(isMessageVisibleInChat(delegation, 'chat-main', 'bot-main'), true)
  assert.equal(isMessageVisibleInChat(workerReply, 'chat-main', 'bot-main'), false)
  assert.equal(isMessageVisibleInChat(taskCard, 'chat-main', 'bot-main'), true)
  assert.equal(taskCardAvatarOwnerBotId(workerReply, taskCard, { 'assignment-worker': { bot_id: 'bot-worker' } }), 'bot-worker')
})
