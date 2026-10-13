import assert from 'node:assert/strict'
import test from 'node:test'
import { collectPendingChatActions, taskCardOwnsFollowingAvatar } from './pendingChatActions.model.ts'

test('collects actionable approvals even when their message card is absent', () => {
  const actions = collectPendingChatActions('chat-main', [], {}, {
    approval1: { id: 'approval1', chat_id: 'chat-main', state: 'pending', tool: 'bot.create', summary: '创建调研员' },
    approval2: { id: 'approval2', chat_id: 'chat-other', state: 'pending', tool: 'bash', summary: 'ignored' },
    approval3: { id: 'approval3', chat_id: 'chat-main', state: 'allowed_once', tool: 'bash', summary: 'resolved' },
  }, {})
  assert.deepEqual(actions, [{ kind: 'approval', id: 'approval1', tool: 'bot.create', summary: '创建调研员', detail: '', risk: '', matchedRuleText: '' }])
})

test('collects linked reviews and questions plus pending orphan questions for this chat', () => {
  const q1 = { id: 'q1', chat_id: 'chat-main', text: '选哪个方案？', options: ['A', 'B'], state: 'pending' }
  const q2 = { id: 'q2', chat_id: 'chat-main', text: '补充一下？', options: [], state: 'pending' }
  const actions = collectPendingChatActions('chat-main', [{
    id: 'message-1',
    blocks: [
      { type: 'question', question_id: 'q1' },
      { type: 'review_card', state: 'pending', project_id: 'project-1' },
    ],
  }], { q1, q2 }, {}, { 'project-1': { name: '登录项目' } })
  assert.deepEqual(actions.map(({ kind, id, title, messageId }) => ({ kind, id, title, messageId })), [
    { kind: 'question', id: 'q1', title: '选哪个方案？', messageId: 'message-1' },
    { kind: 'review', id: 'message-1:1', title: '登录项目', messageId: 'message-1' },
    { kind: 'question', id: 'q2', title: '补充一下？', messageId: undefined },
  ])
})

test('moves the bot avatar onto its immediately following task-card row', () => {
  const previous = { id: 'message-1', sender: { kind: 'bot', bot_id: 'bot-main' }, blocks: [{ type: 'text', text: '我会开始处理。' }] }
  const task = { id: 'message-2', sender: { kind: 'bot', bot_id: 'bot-main' }, blocks: [{ type: 'task_card', assignment_id: 'assignment-1' }] }
  assert.equal(taskCardOwnsFollowingAvatar(previous, task), true)
  assert.equal(taskCardOwnsFollowingAvatar(previous, { ...task, sender: { kind: 'bot', bot_id: 'bot-other' } }), false)
  assert.equal(taskCardOwnsFollowingAvatar(previous, { ...task, sender: { kind: 'user' } }), false)
})
