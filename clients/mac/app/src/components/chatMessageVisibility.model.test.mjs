import assert from 'node:assert/strict'
import test from 'node:test'
import { isMessageVisibleInChat, mainBotIdForChat, mainChatMessageOwnershipIssue } from './chatMessageVisibility.model.ts'
import { taskCardAvatarOwnerBotId } from './pendingChatActions.model.ts'

test('main Bot DM hides worker messages while keeping main Bot and system activity', () => {
  const chatId = 'chat-main'
  const mainBotId = 'bot_main'

  assert.equal(isMessageVisibleInChat({ chat_id: chatId, sender: { kind: 'bot', bot_id: mainBotId } }, chatId, mainBotId), true)
  assert.equal(isMessageVisibleInChat({ chat_id: chatId, sender: { kind: 'bot', bot_id: 'bot-worker' } }, chatId, mainBotId), false)
  assert.equal(isMessageVisibleInChat({ chat_id: chatId, sender: { kind: 'bot' } }, chatId, mainBotId), false)
  assert.equal(isMessageVisibleInChat({ chat_id: chatId, sender: { kind: 'system' } }, chatId, mainBotId), true)
  assert.equal(isMessageVisibleInChat({ chat_id: chatId, sender: { kind: 'user' } }, chatId, mainBotId), true)
})

test('does not hide another Bot when viewing its own DM or a group conversation', () => {
  const workerMessage = { chat_id: 'chat-worker', sender: { kind: 'bot', bot_id: 'bot-worker' } }
  assert.equal(isMessageVisibleInChat(workerMessage, 'chat-worker'), true)
  assert.equal(isMessageVisibleInChat({ ...workerMessage, chat_id: 'chat-group' }, 'chat-group'), true)
})

test('rejects cached messages that belong to a different chat', () => {
  assert.equal(isMessageVisibleInChat({ chat_id: 'chat-other', sender: { kind: 'bot', bot_id: 'bot_main' } }, 'chat-main', 'bot_main'), false)
  assert.equal(isMessageVisibleInChat({ sender: { kind: 'bot', bot_id: 'bot_main' } }, 'chat-main', 'bot_main'), true)
})

test('main chat without bot_id resolves the canonical main Bot from state', () => {
  const chat = { id: 'chat-main', kind: 'main', bot_id: null }
  const bots = {
    bot_main: { id: 'bot_main', is_main: true },
    bot_research: { id: 'bot_research', is_main: false },
  }
  const mainBotId = mainBotIdForChat(chat, bots)

  assert.equal(mainBotId, 'bot_main')
  assert.equal(isMessageVisibleInChat({ chat_id: 'chat-main', sender: { kind: 'bot', bot_id: 'bot_main' } }, chat.id, mainBotId), true)
  assert.equal(isMessageVisibleInChat({ chat_id: 'chat-main', sender: { kind: 'bot', bot_id: 'bot_research' } }, chat.id, mainBotId), false)
})

test('main chat resolves its sender from is_main even when chat.bot_id points at a worker', () => {
  const chat = { id: 'chat-main', kind: 'main', bot_id: 'bot_research' }
  const bots = {
    bot_main: { id: 'bot_main', is_main: true },
    bot_research: { id: 'bot_research', is_main: false },
  }

  assert.equal(mainBotIdForChat(chat, bots), 'bot_main')
})

test('main chat uses protocol canonical id while Bot records are not loaded yet', () => {
  assert.equal(mainBotIdForChat({ id: 'chat-main', kind: 'main', bot_id: null }, {}), 'bot_main')
})

test('project chat remains unfiltered even when its lead is the main Bot', () => {
  const chat = { id: 'chat-project', kind: 'project', project_id: 'project-1', bot_id: 'bot_main' }
  const bots = { bot_main: { id: 'bot_main', is_main: true } }

  assert.equal(mainBotIdForChat(chat, bots), undefined)
  assert.equal(isMessageVisibleInChat({ chat_id: chat.id, sender: { kind: 'bot', bot_id: 'bot_worker' } }, chat.id), true)
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

test('main chat hides and diagnoses a main-sender reply owned by a worker assignment', () => {
  const message = {
    id: 'worker-assignment-reply',
    chat_id: 'chat-main',
    sender: { kind: 'bot', bot_id: 'bot-main' },
    assignment_id: 'assignment-worker',
  }
  const assignments = { 'assignment-worker': { bot_id: 'bot-worker' } }

  assert.equal(isMessageVisibleInChat(message, 'chat-main', 'bot-main', assignments), false)
  assert.deepEqual(mainChatMessageOwnershipIssue(message, 'chat-main', 'bot-main', assignments), {
    messageId: 'worker-assignment-reply',
    chatId: 'chat-main',
    senderBotId: 'bot-main',
    assignmentId: 'assignment-worker',
    assignmentBotId: 'bot-worker',
    reason: 'foreign_assignment',
  })
})

test('main chat does not report worker assignments that are not loaded', () => {
  const message = {
    id: 'cached-reply',
    chat_id: 'chat-main',
    sender: { kind: 'bot', bot_id: 'bot-main' },
    assignment_id: 'assignment-not-loaded',
  }

  assert.equal(isMessageVisibleInChat(message, 'chat-main', 'bot-main'), true)
  assert.equal(mainChatMessageOwnershipIssue(message, 'chat-main', 'bot-main'), null)
})
